//! Downloading recordings from AiM MyChron loggers over Wi-Fi: a browser for
//! what a logger holds, and Track mode, which fetches new sessions by itself
//! whenever the logger comes into range.
//!
//! [`MyChron`] lives on the application shell so it keeps working in both
//! Analysis and Overlay. It owns the settings and the view state; the
//! protocol work runs on [`worker`]'s thread, which exists only while the
//! window is open or Track mode is on.

mod keep_awake;
mod library;
mod policy;
pub mod settings;
pub mod window;
mod worker;

use crate::ui_kit::Tone;
use eframe::egui;
use overlay_logger::{
    DeviceAddr,
    wifi::{self, WifiControl},
};
use settings::MyChronSettings;
use std::{collections::VecDeque, path::PathBuf};
use worker::{Activity, Command, DeviceSummary, Event, Presence, Row};

const FEED_LINES: usize = 40;

/// Creates the Wi-Fi control the worker uses; replaced in tests.
type WifiFactory = std::sync::Arc<dyn Fn() -> Option<Box<dyn WifiControl>> + Send + Sync>;

/// A line in the activity feed.
#[derive(Clone, Debug)]
struct FeedLine {
    at: String,
    tone: Tone,
    text: String,
}

/// A downloaded recording waiting to be added to Analysis.
#[derive(Clone, Debug, PartialEq)]
pub struct Import {
    pub path: PathBuf,
    /// Track mode fetched it, as opposed to the user asking.
    pub automatic: bool,
}

pub struct MyChron {
    pub settings: MyChronSettings,
    open: bool,
    view: window::View,
    addr: Result<DeviceAddr, String>,
    wifi_factory: WifiFactory,
    timing: worker::Timing,
    /// For permission checks on the UI thread (macOS asks there).
    wifi: Option<Box<dyn WifiControl>>,
    interfaces: Vec<String>,
    worker: Option<worker::Handle>,
    /// What the worker was last configured with.
    configured: Option<(MyChronSettings, bool)>,
    presence: Presence,
    /// Recordings of the connected logger, newest first.
    rows: Vec<Row>,
    listed_for: Option<String>,
    activity: Activity,
    device: Option<DeviceSummary>,
    feed: VecDeque<FeedLine>,
    imports: Vec<Import>,
    last_sync: Option<String>,
    fetched: usize,
    keep_awake: keep_awake::KeepAwake,
}

impl MyChron {
    pub fn new(settings: MyChronSettings) -> Self {
        Self::with_wifi(
            settings,
            DeviceAddr::from_env(),
            std::sync::Arc::new(wifi::system),
            worker::Timing::default(),
        )
    }

    fn with_wifi(
        settings: MyChronSettings,
        addr: Result<DeviceAddr, String>,
        wifi_factory: WifiFactory,
        timing: worker::Timing,
    ) -> Self {
        Self {
            settings,
            open: false,
            view: window::View::default(),
            addr,
            wifi_factory,
            timing,
            wifi: None,
            interfaces: Vec::new(),
            worker: None,
            configured: None,
            presence: Presence::Absent,
            rows: Vec::new(),
            listed_for: None,
            activity: Activity::Idle,
            device: None,
            feed: VecDeque::new(),
            imports: Vec::new(),
            last_sync: None,
            fetched: 0,
            keep_awake: keep_awake::KeepAwake::default(),
        }
    }

    /// Stop background work before the app exits.
    pub fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop(std::time::Duration::from_secs(5));
        }
        self.keep_awake.set(false);
    }

    pub fn open_window(&mut self) {
        self.open = true;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Start, configure, or stop the worker, and take in its events.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let needed = (self.open || self.settings.track_mode) && self.addr.is_ok();
        if needed && self.worker.is_none() {
            self.worker = Some(worker::Handle::spawn(
                ctx.clone(),
                self.addr.clone().unwrap_or_default(),
                (self.wifi_factory)(),
                self.timing,
                self.settings.clone(),
                self.open,
            ));
            self.configured = Some((self.settings.clone(), self.open));
        } else if !needed && self.worker.is_some() {
            self.worker = None;
            self.configured = None;
            self.presence = Presence::Absent;
            self.activity = Activity::Idle;
            self.rows.clear();
            self.listed_for = None;
        }
        if let Some(worker) = &self.worker {
            let wanted = (self.settings.clone(), self.open);
            if self.configured.as_ref() != Some(&wanted) {
                worker.send(Command::Configure {
                    settings: Box::new(wanted.0.clone()),
                    browsing: wanted.1,
                });
                self.configured = Some(wanted);
            }
        }
        let events = self
            .worker
            .as_ref()
            .map(|worker| worker.events.try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            self.apply(event);
        }
        self.keep_awake
            .set(self.settings.track_mode && self.settings.keep_awake);
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Presence(presence) => {
                match &presence {
                    Presence::Connected { fingerprint, ssid } => {
                        self.settings.remember(fingerprint, ssid.as_deref());
                        if self.listed_for.as_ref() != Some(fingerprint) {
                            self.rows.clear();
                            self.device = None;
                        }
                    }
                    // The last list stays visible (downloads disabled) until
                    // a different logger connects.
                    Presence::Absent | Presence::Visible { .. } => {}
                }
                self.presence = presence;
            }
            Event::Listed { fingerprint, rows } => {
                let mut rows = rows;
                rows.sort_by(|a, b| {
                    b.log
                        .recorded
                        .cmp(&a.log.recorded)
                        .then(b.log.name.cmp(&a.log.name))
                });
                self.view.retain_selection(&rows);
                self.rows = rows;
                self.listed_for = Some(fingerprint);
            }
            Event::Activity(activity) => self.activity = activity,
            Event::Downloaded {
                row,
                import,
                automatic,
            } => {
                if let Some(existing) = self.rows.iter_mut().find(|r| r.log == row.log) {
                    existing.downloaded = row.downloaded.clone();
                }
                self.view.deselect(&row.log.name);
                if import && let Some(path) = row.downloaded {
                    self.imports.push(Import { path, automatic });
                }
                if automatic {
                    self.fetched += 1;
                }
            }
            Event::DeviceInfo(summary) => self.device = Some(summary),
            Event::Log(tone, text) => {
                self.feed.push_front(FeedLine {
                    at: chrono::Local::now().format("%H:%M").to_string(),
                    tone,
                    text,
                });
                self.feed.truncate(FEED_LINES);
            }
            Event::Synced {
                automatic,
                downloaded,
            } => {
                if automatic {
                    let now = chrono::Local::now().format("%H:%M").to_string();
                    if downloaded == 0 {
                        self.feed.push_front(FeedLine {
                            at: now.clone(),
                            tone: Tone::Neutral,
                            text: "No new sessions".into(),
                        });
                        self.feed.truncate(FEED_LINES);
                    }
                    self.last_sync = Some(now);
                }
            }
        }
    }

    /// Downloads waiting to be added to Analysis.
    pub fn take_imports(&mut self) -> Vec<Import> {
        std::mem::take(&mut self.imports)
    }

    /// Short state for the status bar, when there is something to say.
    pub fn status(&self) -> Option<(String, Tone)> {
        let busy = match &self.activity {
            Activity::Idle => None,
            Activity::Joining(_) => Some("Joining Wi-Fi".to_owned()),
            Activity::Listing => Some("Reading list".to_owned()),
            Activity::Downloading { index, count, .. } => {
                Some(format!("Downloading {} of {count}", index + 1))
            }
            Activity::Restoring => Some("Restoring Wi-Fi".to_owned()),
            Activity::SettingClock => Some("Setting clock".to_owned()),
        };
        if let Some(busy) = busy {
            return Some((format!("MyChron · {busy}"), Tone::Info));
        }
        let connected = matches!(self.presence, Presence::Connected { .. });
        if self.settings.track_mode {
            Some(match self.presence {
                Presence::Connected { .. } => ("Track mode · Connected".into(), Tone::Good),
                Presence::Visible { .. } => ("Track mode · In range".into(), Tone::Info),
                Presence::Absent => ("Track mode · Watching".into(), Tone::Neutral),
            })
        } else if connected && self.worker.is_some() {
            Some(("MyChron connected".into(), Tone::Good))
        } else {
            None
        }
    }

    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            worker.send(command);
        }
    }

    fn cancel(&self) {
        if let Some(worker) = &self.worker {
            worker
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn logger_name(&self) -> Option<String> {
        match &self.presence {
            Presence::Connected { fingerprint, ssid } => Some(
                self.settings
                    .logger(fingerprint)
                    .map(|logger| logger.name.clone())
                    .or_else(|| ssid.clone())
                    .unwrap_or_else(|| "MyChron".into()),
            ),
            Presence::Visible { ssid } => Some(ssid.clone()),
            Presence::Absent => self
                .listed_for
                .as_ref()
                .and_then(|fingerprint| self.settings.logger(fingerprint))
                .map(|logger| logger.name.clone()),
        }
    }
}

/// Fixed demo states for the visual snapshots.
#[cfg(test)]
impl MyChron {
    pub fn demo_for_test(&mut self, connected: bool, tab: window::Tab, downloading: bool) {
        // Nothing answers here, so the worker leaves the demo state alone.
        self.addr = Ok(DeviceAddr {
            host: "127.0.0.1".into(),
            tcp_port: 9,
            udp_port: 9,
        });
        self.open = true;
        self.view.tab = tab;
        self.settings.track_mode = tab == window::Tab::TrackMode;
        self.settings
            .remember("4127a9c0d1e2f3a4", Some("AiM-MYC6-4127"));
        self.settings.loggers[0].name = "Sam's KA100".into();
        if !connected {
            return;
        }
        self.presence = Presence::Connected {
            fingerprint: "4127a9c0d1e2f3a4".into(),
            ssid: Some("AiM-MYC6-4127".into()),
        };
        self.listed_for = Some("4127a9c0d1e2f3a4".into());
        let mut device = overlay_logger::fake::FakeDevice::default();
        for (i, (name, at, track, laps)) in [
            ("a_0094.xrz", "2026-09-27 15:41:07", "KELLYS", 8),
            ("a_0093.xrz", "2026-09-27 14:12:55", "KELLYS", 6),
            ("a_0092.xrz", "2026-09-27 11:03:40", "KELLYS", 2),
            ("a_0091.xrz", "2026-09-26 16:20:11", "GVKC", 11),
            ("a_0090.xrz", "2026-09-26 13:55:02", "GVKC", 9),
        ]
        .into_iter()
        .enumerate()
        {
            device.add_recording(name, at, track, vec![0; 380_000 + i * 61_000]);
            let log = &mut device.recordings[i].log;
            log.laps = Some(laps);
            log.best_lap_ms = Some(36_605 + i as u32 * 412);
            log.duration_ms = Some(u64::from(laps) * 37_800);
        }
        self.rows = device
            .recordings
            .into_iter()
            .enumerate()
            .map(|(i, recording)| Row {
                downloaded: (i >= 3).then(|| PathBuf::from(format!("/demo/{i}.xrz"))),
                log: recording.log,
            })
            .collect();
        self.view.selected = ["a_0094.xrz", "a_0093.xrz"].map(String::from).into();
        self.device = Some(DeviceSummary {
            driver: Some("Sam".into()),
            vehicle: None,
            region: Some("usa".into()),
        });
        if downloading {
            self.activity = Activity::Downloading {
                name: "a_0093.xrz".into(),
                index: 1,
                count: 2,
                done: 262_000,
                total: 441_000,
            };
        }
        self.last_sync = Some("15:44".into());
        self.fetched = 3;
        for (at, tone, text) in [
            ("15:44", Tone::Good, "Downloaded a_0094 · KELLYS · 8 laps"),
            ("15:43", Tone::Info, "Joined AiM-MYC6-4127"),
            ("14:16", Tone::Neutral, "No new sessions"),
            ("11:07", Tone::Warn, "Lost the logger's Wi-Fi"),
        ] {
            self.feed.push_back(FeedLine {
                at: at.into(),
                tone,
                text: text.into(),
            });
        }
    }
}

#[cfg(test)]
mod tests;
