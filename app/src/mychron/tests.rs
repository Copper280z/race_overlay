//! The MyChron service and worker against the in-process fake logger.

use super::library::tests::compressed_recording;
use super::settings::{AfterDownload, Connection, MyChronSettings};
use super::worker::{Presence, Timing};
use super::*;
use overlay_logger::fake::{FakeDevice, FakeLogger};
use overlay_logger::wifi::{Permission, WifiError};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

fn fast() -> Timing {
    Timing {
        probe: Duration::from_millis(30),
        probe_timeout: Duration::from_millis(100),
        scan: Duration::from_millis(30),
        join: Duration::from_secs(2),
        refresh: Duration::from_secs(3600),
    }
}

fn logger(names: &[&str]) -> Arc<FakeLogger> {
    let mut device = FakeDevice::default();
    for (i, name) in names.iter().enumerate() {
        device.add_recording(
            name,
            &format!("2026-08-30 15:{:02}:00", 10 + i),
            "KELLYS",
            compressed_recording(i as u8 + 1),
        );
    }
    Arc::new(FakeLogger::start(device))
}

fn service(
    logger: &FakeLogger,
    library: &std::path::Path,
    wifi: Option<FakeWifi>,
    configure: impl FnOnce(&mut MyChronSettings),
) -> MyChron {
    let mut settings = MyChronSettings {
        library: Some(library.to_owned()),
        settle_seconds: 0,
        ..MyChronSettings::default()
    };
    configure(&mut settings);
    let factory: WifiFactory = Arc::new(move || {
        wifi.clone()
            .map(|wifi| Box::new(wifi) as Box<dyn WifiControl>)
    });
    MyChron::with_wifi(settings, Ok(logger.addr.clone()), factory, fast())
}

/// Poll like the UI does until `done`, or fail after a few seconds.
fn pump(mychron: &mut MyChron, ctx: &egui::Context, done: impl Fn(&MyChron) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(mychron) {
        assert!(
            Instant::now() < deadline,
            "timed out; feed: {:?}",
            mychron.feed
        );
        mychron.poll(ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn browsing_lists_and_downloads_chosen_sessions() {
    let logger = logger(&["a_0001.xrz", "a_0002.xrz"]);
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut mychron = service(&logger, library.path(), None, |_| {});
    mychron.poll(&ctx);
    assert!(mychron.worker.is_none(), "nothing runs until needed");

    mychron.open_window();
    pump(&mut mychron, &ctx, |m| m.rows.len() == 2);
    assert!(matches!(mychron.presence, Presence::Connected { .. }));
    assert_eq!(
        mychron.settings.loggers.len(),
        1,
        "the logger is remembered"
    );
    // Newest first.
    assert_eq!(mychron.rows[0].log.name, "a_0002.xrz");
    assert!(mychron.rows.iter().all(|row| row.downloaded.is_none()));

    mychron.send(Command::Download {
        names: vec!["a_0001.xrz".into()],
        import: true,
    });
    pump(&mut mychron, &ctx, |m| !m.imports.is_empty());
    let imports = mychron.take_imports();
    assert_eq!(imports.len(), 1);
    assert!(!imports[0].automatic);
    assert_eq!(
        std::fs::read(&imports[0].path).unwrap(),
        compressed_recording(1)
    );
    assert!(imports[0].path.starts_with(library.path()));
    let row = mychron
        .rows
        .iter()
        .find(|r| r.log.name == "a_0001.xrz")
        .unwrap();
    assert_eq!(row.downloaded.as_ref(), Some(&imports[0].path));

    // "Download only" stores without importing.
    mychron.send(Command::Download {
        names: vec!["a_0002.xrz".into()],
        import: false,
    });
    pump(&mut mychron, &ctx, |m| {
        m.rows.iter().all(|r| r.downloaded.is_some())
    });
    assert!(mychron.take_imports().is_empty());
    assert!(logger.device().clock_writes.is_empty());

    // Closing the window stops the worker.
    mychron.open = false;
    mychron.poll(&ctx);
    assert!(mychron.worker.is_none());
    assert!(mychron.rows.is_empty());
}

#[test]
fn browsing_keeps_the_logger_awake_and_the_list_fresh() {
    let logger = logger(&["a_0001.xrz"]);
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut mychron = service(&logger, library.path(), None, |_| {});
    mychron.timing.refresh = Duration::from_millis(100);
    mychron.open_window();
    pump(&mut mychron, &ctx, |m| m.rows.len() == 1);
    // A session recorded meanwhile shows up with the next refresh.
    logger.device().add_recording(
        "a_0002.xrz",
        "2026-08-30 16:00:00",
        "KELLYS",
        compressed_recording(2),
    );
    pump(&mut mychron, &ctx, |m| m.rows.len() == 2);
    pump(&mut mychron, &ctx, |_| logger.device().listings >= 4);
    // Each refresh reads the live frame that restarts the auto-off timer.
    assert!(!logger.device().fresh_boot);
    assert!(logger.device().tick_ms > 1_000);
    assert!(
        mychron.feed.is_empty(),
        "refreshes are silent: {:?}",
        mychron.feed
    );

    // Closed, nothing is sent.
    mychron.open = false;
    mychron.poll(&ctx);
    let listings = logger.device().listings;
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(logger.device().listings, listings);
}

#[test]
fn dropouts_never_blank_the_session_list() {
    let logger = logger(&["a_0001.xrz", "a_0002.xrz"]);
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut mychron = service(&logger, library.path(), None, |_| {});
    mychron.open_window();
    pump(&mut mychron, &ctx, |m| m.rows.len() == 2);
    let listings = logger.device().listings;

    // Two lost replies in a row: still connected, nothing reloaded.
    logger.device().drop_replies = 2;
    let until = Instant::now() + Duration::from_millis(600);
    while Instant::now() < until {
        mychron.poll(&ctx);
        assert!(matches!(mychron.presence, Presence::Connected { .. }));
        assert_eq!(mychron.rows.len(), 2);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(logger.device().drop_replies, 0, "the replies were dropped");
    assert_eq!(logger.device().listings, listings, "no reload");

    // Really out of range: the list stays, marked out of range.
    logger.device().in_range = false;
    pump(&mut mychron, &ctx, |m| m.presence == Presence::Absent);
    assert_eq!(mychron.rows.len(), 2);
    assert_eq!(mychron.logger_name().as_deref(), Some("MyChron"));

    // Back with a new session: refreshed in place, quietly.
    logger.device().add_recording(
        "a_0003.xrz",
        "2026-08-30 16:00:00",
        "KELLYS",
        compressed_recording(3),
    );
    logger.device().in_range = true;
    let until = Instant::now() + Duration::from_secs(10);
    while mychron.rows.len() != 3 {
        assert!(Instant::now() < until, "not refreshed");
        mychron.poll(&ctx);
        assert!(!mychron.rows.is_empty(), "the list blanked");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        mychron.feed.iter().all(|line| line.tone != Tone::Bad),
        "{:?}",
        mychron.feed
    );
}

#[test]
fn track_mode_fetches_each_new_session_once() {
    let logger = logger(&["a_0001.xrz", "a_0002.xrz"]);
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut mychron = service(&logger, library.path(), None, |s| s.track_mode = true);
    pump(&mut mychron, &ctx, |m| m.imports.len() == 2);
    let first = mychron.take_imports();
    assert!(first.iter().all(|import| import.automatic));
    pump(&mut mychron, &ctx, |m| m.last_sync.is_some());

    // The kart goes out and comes back with one more session.
    logger.device().in_range = false;
    pump(&mut mychron, &ctx, |m| m.presence == Presence::Absent);
    logger.device().add_recording(
        "a_0003.xrz",
        "2026-08-30 16:00:00",
        "KELLYS",
        compressed_recording(9),
    );
    logger.device().in_range = true;
    pump(&mut mychron, &ctx, |m| !m.imports.is_empty());
    let second = mychron.take_imports();
    assert_eq!(second.len(), 1);
    assert!(second[0].path.to_string_lossy().contains("a_0003.xrz"));

    // A fresh start reads the ledger and downloads nothing again.
    drop(mychron);
    logger.device().opened.clear();
    let mut restarted = service(&logger, library.path(), None, |s| s.track_mode = true);
    pump(&mut restarted, &ctx, |m| m.last_sync.is_some());
    assert!(restarted.take_imports().is_empty());
    assert!(logger.device().opened.is_empty(), "no file was opened");
}

#[test]
fn track_mode_can_keep_downloads_in_the_library() {
    let logger = logger(&["a_0001.xrz"]);
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut mychron = service(&logger, library.path(), None, |s| {
        s.track_mode = true;
        s.after_download = AfterDownload::KeepInLibrary;
    });
    pump(&mut mychron, &ctx, |m| m.fetched == 1);
    assert!(mychron.take_imports().is_empty());
}

#[test]
fn an_idle_track_mode_asks_for_no_repaints() {
    let logger = logger(&[]);
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let repaints = Arc::new(Mutex::new(0usize));
    let counter = Arc::clone(&repaints);
    ctx.set_request_repaint_callback(move |_| *counter.lock().unwrap() += 1);
    let mut mychron = service(&logger, library.path(), None, |s| {
        s.track_mode = true;
        s.recheck_minutes = 60;
    });
    pump(&mut mychron, &ctx, |m| m.last_sync.is_some());
    std::thread::sleep(Duration::from_millis(100));
    mychron.poll(&ctx);
    *repaints.lock().unwrap() = 0;
    // Several probe intervals pass with the logger present and nothing new.
    std::thread::sleep(Duration::from_millis(300));
    mychron.poll(&ctx);
    assert_eq!(*repaints.lock().unwrap(), 0);
}

/// Wi-Fi that "joins" the fake logger by bringing it into range.
#[derive(Clone)]
struct FakeWifi {
    logger: Arc<FakeLogger>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl WifiControl for FakeWifi {
    fn interfaces(&self) -> Vec<String> {
        vec!["en0".into()]
    }
    fn permission(&self) -> Permission {
        Permission::Granted
    }
    fn scan_loggers(&self, _: &str) -> Result<Vec<String>, WifiError> {
        Ok(vec!["AiM-MYC6-42".into()])
    }
    fn current(&self, _: &str) -> Result<Option<String>, WifiError> {
        Ok(Some(if self.logger.device().in_range {
            "AiM-MYC6-42".into()
        } else {
            "Paddock".into()
        }))
    }
    fn join(&self, interface: &str, ssid: &str) -> Result<(), WifiError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("join {interface} {ssid}"));
        self.logger.device().in_range = true;
        Ok(())
    }
    fn restore(&self, interface: &str, previous: Option<&str>) -> Result<(), WifiError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("restore {interface} {previous:?}"));
        self.logger.device().in_range = false;
        Ok(())
    }
}

#[test]
fn switch_mode_joins_downloads_and_returns_to_the_previous_network() {
    let logger = logger(&["a_0001.xrz"]);
    logger.device().in_range = false;
    let wifi = FakeWifi {
        logger: Arc::clone(&logger),
        calls: Arc::default(),
    };
    let calls = Arc::clone(&wifi.calls);
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut mychron = service(&logger, library.path(), Some(wifi), |s| {
        s.track_mode = true;
        s.connection = Connection::AppSwitches;
        s.recheck_minutes = 60;
    });
    pump(&mut mychron, &ctx, |m| m.imports.len() == 1);
    pump(&mut mychron, &ctx, |_| calls.lock().unwrap().len() == 2);
    assert_eq!(
        *calls.lock().unwrap(),
        ["join en0 AiM-MYC6-42", "restore en0 Some(\"Paddock\")"]
    );
    pump(&mut mychron, &ctx, |m| m.settings.loggers.len() == 1);
    assert_eq!(
        mychron.settings.loggers[0].ssid.as_deref(),
        Some("AiM-MYC6-42")
    );
    assert!(!logger.device().in_range, "left the logger's hotspot");
    // Still in range: no new join until the re-check is due.
    pump(&mut mychron, &ctx, |m| {
        matches!(m.presence, Presence::Visible { .. })
    });
    std::thread::sleep(Duration::from_millis(300));
    mychron.poll(&ctx);
    assert_eq!(
        calls.lock().unwrap().len(),
        2,
        "{:?}",
        calls.lock().unwrap()
    );

    // Quitting while joined returns to the previous network.
    mychron.send(Command::Connect);
    pump(&mut mychron, &ctx, |_| logger.device().in_range);
    mychron.shutdown();
    assert!(!logger.device().in_range);
    assert_eq!(calls.lock().unwrap().len(), 4);
}

/// Real recordings through the whole path: compressed like the logger stores
/// them, served by the fake logger, downloaded, verified, filed in the
/// library, and read back by the XRK importer.
#[test]
#[ignore = "needs the local mychron_data recordings"]
fn real_recordings_download_and_import() {
    use std::io::Write;
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mychron_data/8_30_26_autox");
    let mut device = FakeDevice::default();
    for (i, name) in ["a_0082.xrk", "a_0083.xrk"].iter().enumerate() {
        let xrk = std::fs::read(dir.join(name)).expect("local recording");
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&xrk).unwrap();
        device.add_recording(
            &name.replace(".xrk", ".xrz"),
            &format!("2026-08-30 12:{:02}:00", 40 + i),
            "Autocross",
            encoder.finish().unwrap(),
        );
    }
    let logger = Arc::new(FakeLogger::start(device));
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut mychron = service(&logger, library.path(), None, |s| s.track_mode = true);
    pump(&mut mychron, &ctx, |m| m.imports.len() == 2);
    for import in mychron.take_imports() {
        let dataset = overlay_core::AdapterRegistry::with_builtins()
            .load(
                "aim_xrk",
                overlay_core::SourceId::new(),
                &import.path,
                &serde_json::Value::Null,
            )
            .unwrap_or_else(|error| panic!("{}: {error}", import.path.display()));
        assert!(dataset.named("gps_speed").is_some());
        assert!(!dataset.laps.is_empty());
        eprintln!(
            "{} → {} channels, {} laps",
            import.path.strip_prefix(library.path()).unwrap().display(),
            dataset.channels.len(),
            dataset.laps.len()
        );
    }
}

/// The service against a real logger (address from
/// `RACE_OVERLAY_MYCHRON_ADDR`, default `11.0.0.1`): browse, download the
/// newest recording, and import it; then Track mode limited to today.
#[test]
#[ignore = "needs a MyChron in range"]
fn real_logger_browse_download_and_track() {
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let real = |track_mode: bool| {
        let settings = MyChronSettings {
            library: Some(library.path().to_owned()),
            track_mode,
            settle_seconds: 0,
            since: settings::Since::Today,
            ..MyChronSettings::default()
        };
        MyChron::with_wifi(
            settings,
            DeviceAddr::from_env(),
            Arc::new(|| None),
            Timing::default(),
        )
    };
    let mut mychron = real(false);
    mychron.open_window();
    pump(&mut mychron, &ctx, |m| !m.rows.is_empty());
    let newest = mychron.rows[0].log.clone();
    eprintln!(
        "{} recordings; newest {}",
        mychron.rows.len(),
        worker::describe(&newest)
    );
    mychron.send(Command::Download {
        names: vec![newest.name.clone()],
        import: true,
    });
    pump(&mut mychron, &ctx, |m| !m.imports.is_empty());
    let path = mychron.take_imports().remove(0).path;
    let dataset = overlay_core::AdapterRegistry::with_builtins()
        .load(
            "aim_xrk",
            overlay_core::SourceId::new(),
            &path,
            &serde_json::Value::Null,
        )
        .unwrap();
    eprintln!(
        "{} → {} channels, {} laps",
        path.strip_prefix(library.path()).unwrap().display(),
        dataset.channels.len(),
        dataset.laps.len()
    );
    assert!(!dataset.channels.is_empty());
    mychron.shutdown();

    let today = chrono::Local::now().date_naive();
    let expected = mychron
        .rows
        .iter()
        .filter(|row| row.downloaded.is_none() && row.log.recorded.map(|t| t.date()) == Some(today))
        .count();
    let mut track = real(true);
    pump(&mut track, &ctx, |m| m.last_sync.is_some());
    assert_eq!(track.take_imports().len(), expected);
    eprintln!(
        "Track mode (today only): {expected} new; feed {:?}",
        track.feed
    );
}

/// With the window open, the app keeps a real logger from switching itself
/// off. Set the logger's auto-off to its 2-minute minimum first.
#[test]
#[ignore = "needs a MyChron in range, with auto-off at 2 minutes"]
fn real_logger_stays_on_while_browsing() {
    let library = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let settings = MyChronSettings {
        library: Some(library.path().to_owned()),
        ..MyChronSettings::default()
    };
    let addr = DeviceAddr::from_env();
    let mut mychron =
        MyChron::with_wifi(settings, addr.clone(), Arc::new(|| None), Timing::default());
    mychron.open_window();
    pump(&mut mychron, &ctx, |m| !m.rows.is_empty());
    let until = Instant::now() + Duration::from_secs(240);
    while Instant::now() < until {
        mychron.poll(&ctx);
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        matches!(mychron.presence, Presence::Connected { .. }),
        "feed: {:?}",
        mychron.feed
    );
    mychron.shutdown();
    let descriptor = overlay_logger::probe(&addr.unwrap(), Duration::from_secs(2)).unwrap();
    assert!(descriptor.is_some(), "the logger switched itself off");
}
