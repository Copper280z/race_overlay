//! The client against the in-process fake logger.

use crate::fake::{FakeDevice, FakeLogger, SCHEMA_HASH};
use crate::*;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn quick() -> Timeouts {
    Timeouts {
        connect: Duration::from_secs(1),
        reply: Duration::from_millis(500),
        chunk: Duration::from_millis(500),
    }
}

/// Bytes that span several download chunks and are not a repeating pattern.
fn recording(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

fn logger_with(recordings: &[(&str, usize)]) -> FakeLogger {
    let mut device = FakeDevice::default();
    for (i, (name, len)) in recordings.iter().enumerate() {
        device.add_recording(
            name,
            &format!("2026-08-30 15:{:02}:00", 10 + i),
            "KELLYS",
            recording(*len),
        );
    }
    FakeLogger::start(device)
}

#[test]
fn discovers_lists_and_downloads_byte_exact() {
    let logger = logger_with(&[("a_0089.xrz", 477_678), ("a_0090.xrz", 10)]);
    let descriptor = probe(&logger.addr, Duration::from_secs(1))
        .unwrap()
        .unwrap();
    assert_eq!(descriptor.raw.len(), DESCRIPTOR_LEN);

    let mut session = Session::open(&logger.addr, quick()).unwrap();
    let logs = session.datalogs().unwrap();
    assert_eq!(logs.len(), 2);
    assert_eq!(logs[0].name, "a_0089.xrz");
    assert_eq!(logs[0].size, 477_678);
    assert_eq!(logs[0].track, "KELLYS");

    let mut bytes = Vec::new();
    let mut steps = Vec::new();
    let size = session
        .download_datalog(
            &logs[0],
            &mut bytes,
            &mut |done, total| steps.push((done, total)),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(size, 477_678);
    assert_eq!(bytes, recording(477_678));
    // Progress starts at zero and then advances one 65 472-byte chunk at a time.
    assert_eq!(steps[0], (0, 477_678));
    assert_eq!(steps[1], (65_472, 477_678));
    assert_eq!(*steps.last().unwrap(), (477_678, 477_678));
    assert_eq!(steps.len(), 1 + 8);

    // The same connection stays usable afterwards, like the captured
    // session that re-listed after its download.
    assert_eq!(session.datalogs().unwrap().len(), 2);
    assert!(
        logger.device().clock_writes.is_empty(),
        "listing must not set the clock"
    );
}

#[test]
fn every_documented_operation_decodes() {
    let logger = logger_with(&[]);
    let mut session = Session::open(&logger.addr, quick()).unwrap();
    assert!(matches!(
        session.main_object().unwrap(),
        MainObject::Profile { ref name, .. } if name == "System"
    ));
    let info = session.device_info(ClockWrite::now()).unwrap();
    assert_eq!(info.hardware.wifi_chip(), Some("ESP32"));
    assert_eq!(info.driver(), Some("Test Driver"));
    assert_eq!(info.paths["tkk"].directory, "0:/tkk");
    session.time_negotiate(ClockWrite::now()).unwrap();
    assert_eq!(logger.device().clock_writes.len(), 2);

    let users = session.user_profiles().unwrap();
    assert_eq!(users.len(), 5);
    assert_eq!(users[1].name, "Driver 1");

    let (catalog, channels) = session.channel_catalog().unwrap();
    assert_eq!(catalog.magic, CATALOG_MAGIC);
    assert_eq!(channels[2].short_name, "RPM");
    assert_eq!(session.channel_tree().unwrap().records.len(), 3);
    let (schema, entries) = session.live_schema().unwrap();
    assert_eq!(schema.hash, SCHEMA_HASH);
    assert_eq!(entries[2].scale, Some(0.001));

    session.start_live().unwrap();
    assert!(logger.device().live);
    let first = session.live_frame().unwrap();
    let second = session.live_frame().unwrap();
    assert!(second.tick_ms.unwrap() > first.tick_ms.unwrap());
    assert_eq!(session.live_heartbeat().unwrap().unwrap().tick_ms, None);
    assert_eq!(session.live_snapshot().unwrap().records.len(), 1);
    session.stop_live().unwrap();
    assert!(!logger.device().live);

    assert_eq!(session.stat("0:/tkk/dev.ria").unwrap(), Some(12));
    assert_eq!(session.stat("0:/lgo/splash.bmp").unwrap(), None);
    let mut ria = Vec::new();
    session
        .read_file(
            "0:/tkk/dev.ria",
            &mut ria,
            &mut |_, _| {},
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(ria, b"Yard\0KELLYS\0");

    let empty = probe_object(&logger.addr, quick(), &stcp::Request::new(0x52, 2))
        .unwrap()
        .unwrap();
    assert_eq!(empty.header.status, stcp::Status::DataReady);
    assert!(empty.data.is_none());
    assert!(
        probe_object(&logger.addr, quick(), &stcp::Request::new(0x05, 2))
            .unwrap()
            .is_none()
    );
}

#[test]
fn absent_files_and_faults_are_reported() {
    let logger = logger_with(&[("a_0001.xrz", 200_000)]);
    let cancel = AtomicBool::new(false);
    let mut session = Session::open(&logger.addr, quick()).unwrap();
    let error = session
        .read_file(
            "1:/mem/a_9999.xrz",
            &mut Vec::new(),
            &mut |_, _| {},
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(error, LoggerError::NotFound(_)), "{error}");
    // Any error leaves the session unusable.
    assert!(matches!(session.datalogs(), Err(LoggerError::Protocol(_))));

    logger.device().truncate_after = Some(100_000);
    let mut session = Session::open(&logger.addr, quick()).unwrap();
    let log = session.datalogs().unwrap().remove(0);
    let error = session
        .download_datalog(&log, &mut Vec::new(), &mut |_, _| {}, &cancel)
        .unwrap_err();
    assert!(
        matches!(
            error,
            LoggerError::SizeMismatch {
                expected: 200_000,
                ..
            }
        ),
        "{error}"
    );
    logger.device().truncate_after = None;

    logger.device().corrupt_next_data = true;
    let mut session = Session::open(&logger.addr, quick()).unwrap();
    assert!(matches!(
        session.datalogs(),
        Err(LoggerError::Checksum { .. })
    ));

    let mut session = Session::open(&logger.addr, quick()).unwrap();
    let cancelled = AtomicBool::new(true);
    let mut written = Vec::new();
    let error = session
        .download_datalog(&log, &mut written, &mut |_, _| {}, &cancelled)
        .unwrap_err();
    assert!(matches!(error, LoggerError::Cancelled));
    assert_eq!(written.len(), 65_472, "stops after the chunk in flight");

    logger.device().silent.insert((0x24, 2));
    let mut session = Session::open(&logger.addr, quick()).unwrap();
    assert!(matches!(session.datalogs(), Err(LoggerError::Timeout)));
}

#[test]
fn out_of_range_loggers_are_unreachable() {
    let logger = logger_with(&[]);
    logger.device().in_range = false;
    assert_eq!(
        probe(&logger.addr, Duration::from_millis(150)).unwrap(),
        None
    );
    let error = Session::open(&logger.addr, quick()).err().unwrap();
    assert!(error.is_unreachable(), "{error}");
    logger.device().in_range = true;
    assert!(
        probe(&logger.addr, Duration::from_secs(1))
            .unwrap()
            .is_some()
    );
}

#[test]
fn keepalive_reaches_the_logger() {
    let logger = logger_with(&[]);
    let keepalive = Keepalive::start(&logger.addr).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    drop(keepalive);
}
