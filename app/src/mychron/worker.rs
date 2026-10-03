//! The MyChron background thread: watches for a logger, lists and downloads
//! recordings, and (in "switch Wi-Fi" mode) joins and leaves its hotspot.
//!
//! It only runs while the MyChron window is open or Track mode is on. It
//! wakes the UI once per event and never otherwise, so an idle window stays
//! idle.

use super::library::LoggerFolder;
use super::policy::{self, Trigger};
use super::settings::{AfterDownload, Connection, KnownLogger, MyChronSettings};
use crate::ui_kit::Tone;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use eframe::egui;
use overlay_logger::{
    ClockWrite, Datalog, DeviceAddr, LoggerError, Session, Timeouts, probe,
    wifi::{WifiControl, WifiError},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Consecutive unanswered probes before a connected logger counts as gone.
/// Single UDP replies get lost on Wi-Fi; treating each loss as a disconnect
/// would blank the session list.
const MISSES_BEFORE_GONE: u32 = 3;

#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// How often to look for the logger on the current network (one UDP
    /// packet).
    pub probe: Duration,
    /// How long to wait for a probe's answer; a busy logger can take most of
    /// a second.
    pub probe_timeout: Duration,
    /// How often to scan for the logger's hotspot in switch mode.
    pub scan: Duration,
    /// Time for the hotspot to hand out an address after joining.
    pub join: Duration,
    /// While the window shows a connected logger, how often to keep it awake
    /// and re-read its list so sessions recorded meanwhile appear. Must stay
    /// under the shortest auto-off a logger allows (2 minutes).
    pub refresh: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            probe: Duration::from_secs(3),
            probe_timeout: Duration::from_millis(1500),
            scan: Duration::from_secs(20),
            join: Duration::from_secs(20),
            refresh: Duration::from_secs(30),
        }
    }
}

pub enum Command {
    Configure {
        settings: Box<MyChronSettings>,
        browsing: bool,
    },
    /// List the logger's recordings again.
    Refresh,
    Download {
        names: Vec<String>,
        import: bool,
    },
    /// Switch mode: join the visible logger's hotspot and stay.
    Connect,
    /// Switch mode: leave the hotspot joined by [`Command::Connect`].
    Disconnect,
    /// Write the computer's clock to the logger and read its device info.
    SetClock,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    Absent,
    /// Switch mode: the hotspot is visible but not joined.
    Visible {
        ssid: String,
    },
    /// The logger answers on the current network.
    Connected {
        fingerprint: String,
        ssid: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub log: Datalog,
    /// Library path, when it was downloaded before.
    pub downloaded: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Activity {
    Idle,
    Joining(String),
    Listing,
    Downloading {
        name: String,
        index: usize,
        count: usize,
        done: u64,
        total: u64,
    },
    Restoring,
    SettingClock,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSummary {
    pub driver: Option<String>,
    pub vehicle: Option<String>,
    pub region: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Event {
    Presence(Presence),
    Listed {
        fingerprint: String,
        rows: Vec<Row>,
    },
    Activity(Activity),
    Downloaded {
        row: Box<Row>,
        import: bool,
        automatic: bool,
    },
    DeviceInfo(DeviceSummary),
    /// A line for the activity feed.
    Log(Tone, String),
    /// Track mode finished a check.
    Synced {
        automatic: bool,
        downloaded: usize,
    },
}

pub struct Handle {
    pub commands: Sender<Command>,
    pub events: Receiver<Event>,
    /// Stops a download between chunks.
    pub cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    pub fn spawn(
        ctx: egui::Context,
        addr: DeviceAddr,
        wifi: Option<Box<dyn WifiControl>>,
        timing: Timing,
        settings: MyChronSettings,
        browsing: bool,
    ) -> Self {
        let (commands, command_rx) = crossbeam_channel::unbounded();
        let (event_tx, events) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut worker = Worker {
            commands: command_rx,
            events: event_tx,
            ctx,
            addr,
            wifi,
            timing,
            trigger: trigger_for(&settings),
            settings,
            browsing,
            presence: Presence::Absent,
            joined: None,
            listed: None,
            cancel: Arc::clone(&cancel),
            next_check: Instant::now(),
            next_scan: Instant::now(),
            last_command: Instant::now(),
            misses: 0,
            refresh_due: false,
        };
        let thread = std::thread::Builder::new()
            .name("mychron".into())
            .spawn(move || worker.run())
            .expect("spawn MyChron worker");
        Self {
            commands,
            events,
            cancel,
            thread: Some(thread),
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl Handle {
    /// Stop the worker and give it up to `wait` to leave a hotspot it joined,
    /// so quitting the app does not strand the computer on the logger's Wi-Fi.
    pub fn stop(mut self, wait: Duration) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let deadline = Instant::now() + wait;
            while !thread.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.commands.send(Command::Shutdown);
        // The thread restores Wi-Fi on the way out; don't block the UI on it.
        drop(self.thread.take());
    }
}

fn trigger_for(settings: &MyChronSettings) -> Trigger {
    Trigger::new(
        Duration::from_secs(u64::from(settings.settle_seconds)),
        Duration::from_secs(u64::from(settings.recheck_minutes.max(1)) * 60),
    )
}

/// A hotspot this worker joined, to leave again.
struct Joined {
    interface: String,
    ssid: String,
    previous: Option<String>,
    /// Joined on request ([`Command::Connect`]) rather than for one sync.
    manual: bool,
}

struct Worker {
    commands: Receiver<Command>,
    events: Sender<Event>,
    ctx: egui::Context,
    addr: DeviceAddr,
    wifi: Option<Box<dyn WifiControl>>,
    timing: Timing,
    settings: MyChronSettings,
    browsing: bool,
    trigger: Trigger,
    presence: Presence,
    joined: Option<Joined>,
    /// Logger and recordings of the latest listing.
    listed: Option<(String, Vec<Datalog>)>,
    cancel: Arc<AtomicBool>,
    next_check: Instant,
    next_scan: Instant,
    /// When a command session last ran.
    last_command: Instant,
    /// Unanswered probes in a row while connected.
    misses: u32,
    /// The listed logger came back; re-read its list quietly.
    refresh_due: bool,
}

impl Worker {
    fn run(&mut self) {
        loop {
            let wait = self.next_check.saturating_duration_since(Instant::now());
            match self.commands.recv_timeout(wait) {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if Instant::now() >= self.next_check {
                self.next_check = Instant::now() + self.timing.probe;
                self.check();
            }
        }
        self.leave(true);
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        self.ctx.request_repaint();
    }

    fn log(&self, tone: Tone, text: impl Into<String>) {
        self.emit(Event::Log(tone, text.into()));
    }

    fn activity(&self, activity: Activity) {
        self.emit(Event::Activity(activity));
    }

    fn set_presence(&mut self, presence: Presence) {
        if presence != self.presence {
            let returning = matches!(&presence, Presence::Connected { fingerprint, .. }
                if self.listed.as_ref().is_some_and(|(listed, _)| listed == fingerprint));
            if returning && !matches!(self.presence, Presence::Connected { .. }) {
                self.refresh_due = true;
            }
            self.presence = presence.clone();
            self.emit(Event::Presence(presence));
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Configure { settings, browsing } => {
                let timing_changed = settings.settle_seconds != self.settings.settle_seconds
                    || settings.recheck_minutes != self.settings.recheck_minutes;
                let mode_changed = settings.connection != self.settings.connection;
                let browsing_started = browsing && !self.browsing;
                self.settings = *settings;
                self.browsing = browsing;
                if timing_changed {
                    self.trigger.settle = trigger_for(&self.settings).settle;
                    self.trigger.recheck = trigger_for(&self.settings).recheck;
                }
                if mode_changed {
                    self.leave(true);
                    self.set_presence(Presence::Absent);
                    self.next_scan = Instant::now();
                }
                if !self.browsing && self.joined.as_ref().is_some_and(|j| j.manual) {
                    self.leave(true);
                }
                if browsing_started {
                    self.listed = None;
                    self.next_check = Instant::now();
                }
            }
            Command::Refresh => {
                self.next_scan = Instant::now();
                if let Presence::Connected { fingerprint, .. } = self.presence.clone() {
                    self.list(&fingerprint);
                } else {
                    self.next_check = Instant::now();
                }
            }
            Command::Download { names, import } => self.download_named(&names, import),
            Command::Connect => {
                if let Presence::Visible { ssid } = self.presence.clone()
                    && let Some(fingerprint) = self.join(&ssid, true)
                {
                    self.list(&fingerprint);
                }
            }
            Command::Disconnect => self.leave(true),
            Command::SetClock => self.set_clock(),
            Command::Shutdown => {}
        }
    }

    /// Is the logger here, and is a Track mode sync or a listing due?
    fn check(&mut self) {
        let switching = self.settings.connection == Connection::AppSwitches;
        let answer = probe(&self.addr, self.timing.probe_timeout);
        if matches!(answer, Ok(Some(_))) {
            self.misses = 0;
        } else if matches!(self.presence, Presence::Connected { .. })
            && self.misses + 1 < MISSES_BEFORE_GONE
        {
            // Probably a lost reply: confirm soon before calling it gone.
            self.misses += 1;
            self.next_check = Instant::now() + self.timing.probe / 3;
            return;
        } else {
            self.misses = 0;
        }
        match answer {
            Ok(Some(descriptor)) => {
                let fingerprint = descriptor.fingerprint();
                let ssid = self.joined.as_ref().map(|j| j.ssid.clone());
                self.set_presence(Presence::Connected { fingerprint, ssid });
            }
            Ok(None) | Err(_) if self.joined.is_some() => {
                // The hotspot we joined went away.
                self.log(Tone::Warn, "Lost the logger's Wi-Fi");
                self.leave(false);
                self.set_presence(Presence::Absent);
            }
            Ok(None) | Err(_) if switching => {
                if matches!(self.presence, Presence::Connected { .. }) {
                    self.set_presence(Presence::Absent);
                }
                if Instant::now() >= self.next_scan {
                    self.next_scan = Instant::now() + self.timing.scan;
                    match self.scan() {
                        Some(ssid) => self.set_presence(Presence::Visible { ssid }),
                        None => self.set_presence(Presence::Absent),
                    }
                }
            }
            Ok(None) | Err(_) => self.set_presence(Presence::Absent),
        }

        let watched = |fingerprint: &str| {
            self.settings
                .watch
                .as_ref()
                .is_none_or(|watch| watch == fingerprint)
        };
        let in_range = match &self.presence {
            Presence::Absent => false,
            Presence::Visible { .. } => true,
            Presence::Connected { fingerprint, .. } => watched(fingerprint),
        };
        if self.settings.track_mode && self.trigger.observe(Instant::now(), in_range) {
            self.sync_new();
        } else if self.browsing
            && let Presence::Connected { fingerprint, .. } = self.presence.clone()
        {
            let unlisted = self
                .listed
                .as_ref()
                .is_none_or(|(listed, _)| *listed != fingerprint);
            if unlisted {
                self.list(&fingerprint);
            } else if self.refresh_due || self.last_command.elapsed() >= self.timing.refresh {
                self.refresh(&fingerprint);
            }
        }
    }

    /// A logger hotspot to join: the watched logger's, or any.
    fn scan(&mut self) -> Option<String> {
        let wifi = self.wifi.as_ref()?;
        if !wifi.permission().allows_scanning() {
            return None;
        }
        let interface = self.interface()?;
        let wanted = self
            .settings
            .watch
            .as_ref()
            .and_then(|watch| self.settings.logger(watch))
            .and_then(|logger| logger.ssid.clone());
        match wifi.scan_loggers(&interface) {
            Ok(ssids) => ssids
                .into_iter()
                .find(|ssid| wanted.as_ref().is_none_or(|wanted| wanted == ssid)),
            Err(error) => {
                self.log(Tone::Warn, format!("Wi-Fi scan: {error}"));
                None
            }
        }
    }

    fn interface(&self) -> Option<String> {
        let wifi = self.wifi.as_ref()?;
        self.settings
            .interface
            .clone()
            .or_else(|| wifi.interfaces().into_iter().next())
    }

    /// Join `ssid` and wait for the logger to answer; its fingerprint.
    fn join(&mut self, ssid: &str, manual: bool) -> Option<String> {
        let wifi = self.wifi.as_ref()?;
        let interface = self.interface()?;
        self.activity(Activity::Joining(ssid.to_owned()));
        let previous = wifi.current(&interface).ok().flatten();
        let result = wifi.join(&interface, ssid);
        if let Err(error) = result {
            self.activity(Activity::Idle);
            self.log(Tone::Bad, format!("Could not join {ssid}: {error}"));
            if matches!(error, WifiError::NotFound(_)) {
                self.set_presence(Presence::Absent);
            }
            return None;
        }
        self.joined = Some(Joined {
            interface,
            ssid: ssid.to_owned(),
            previous,
            manual,
        });
        let deadline = Instant::now() + self.timing.join;
        while Instant::now() < deadline {
            if let Ok(Some(descriptor)) = probe(&self.addr, self.timing.probe_timeout) {
                let fingerprint = descriptor.fingerprint();
                self.activity(Activity::Idle);
                self.log(Tone::Info, format!("Joined {ssid}"));
                self.set_presence(Presence::Connected {
                    fingerprint: fingerprint.clone(),
                    ssid: Some(ssid.to_owned()),
                });
                return Some(fingerprint);
            }
            std::thread::sleep((self.timing.join / 40).max(Duration::from_millis(10)));
        }
        self.log(
            Tone::Bad,
            format!("Joined {ssid}, but the logger did not answer"),
        );
        self.leave(true);
        self.activity(Activity::Idle);
        None
    }

    /// Leave a hotspot this worker joined, returning to the previous network.
    fn leave(&mut self, announce: bool) {
        let Some(joined) = self.joined.take() else {
            return;
        };
        let Some(wifi) = self.wifi.as_ref() else {
            return;
        };
        if announce {
            self.activity(Activity::Restoring);
        }
        match wifi.restore(&joined.interface, joined.previous.as_deref()) {
            Ok(()) => self.log(
                Tone::Neutral,
                match &joined.previous {
                    Some(previous) => format!("Back on {previous}"),
                    None => format!("Left {}", joined.ssid),
                },
            ),
            Err(error) => self.log(Tone::Warn, format!("Could not restore Wi-Fi: {error}")),
        }
        if announce {
            self.activity(Activity::Idle);
        }
        if matches!(self.presence, Presence::Connected { .. }) {
            // Still in range, just not joined: Track mode keeps its re-check
            // schedule instead of treating this as a new arrival.
            self.set_presence(Presence::Visible { ssid: joined.ssid });
        }
    }

    fn folder(&self, fingerprint: &str) -> std::io::Result<LoggerFolder> {
        let folder = self
            .settings
            .logger(fingerprint)
            .map(|logger| logger.folder.clone())
            .unwrap_or_else(|| KnownLogger::new(fingerprint, None).folder);
        LoggerFolder::open(self.settings.library_root().join(folder), fingerprint)
    }

    fn session(&mut self) -> Result<Session, LoggerError> {
        self.last_command = Instant::now();
        // The first connection after the logger boots sometimes fails; the
        // next one succeeds.
        Session::open(&self.addr, Timeouts::default()).or_else(|error| {
            if !error.is_unreachable() {
                return Err(error);
            }
            std::thread::sleep(Duration::from_millis(500));
            Session::open(&self.addr, Timeouts::default())
        })
    }

    /// List the logger's recordings and publish them with download state.
    fn list(&mut self, fingerprint: &str) -> Option<Vec<Datalog>> {
        self.read_list(fingerprint, true)
    }

    /// Keep the logger awake and re-read its list, with no busy indicator.
    ///
    /// The logger's auto-off ignores keepalives, list requests, and the live
    /// heartbeat, but reading the live "main" object restarts it
    /// (`docs/mychron-protocol.md`, "Auto-off timer").
    fn refresh(&mut self, fingerprint: &str) {
        self.refresh_due = false;
        self.read_list(fingerprint, false);
    }

    fn read_list(&mut self, fingerprint: &str, announce: bool) -> Option<Vec<Datalog>> {
        if announce {
            self.activity(Activity::Listing);
        }
        let keep_awake = !announce;
        let result = self.session().and_then(|mut session| {
            if keep_awake {
                session.main_object()?;
            }
            let logs = session.datalogs();
            session.close();
            logs
        });
        if announce {
            self.activity(Activity::Idle);
        }
        let logs = match result {
            Ok(logs) => logs,
            Err(error) => {
                // Probes decide whether the logger is gone; a background
                // refresh just tries again next time.
                if announce {
                    self.log(Tone::Bad, format!("Could not list recordings: {error}"));
                }
                return None;
            }
        };
        let folder = self.folder(fingerprint).ok();
        let rows = logs
            .iter()
            .map(|log| Row {
                log: log.clone(),
                downloaded: folder.as_ref().and_then(|f| f.downloaded(log)),
            })
            .collect();
        self.listed = Some((fingerprint.to_owned(), logs.clone()));
        self.emit(Event::Listed {
            fingerprint: fingerprint.to_owned(),
            rows,
        });
        Some(logs)
    }

    /// Track mode: download every recording not downloaded before.
    fn sync_new(&mut self) {
        let mut joined_here = false;
        let fingerprint = match self.presence.clone() {
            Presence::Connected { fingerprint, .. } => fingerprint,
            Presence::Visible { ssid } => match self.join(&ssid, false) {
                Some(fingerprint) => {
                    joined_here = true;
                    fingerprint
                }
                None => {
                    self.trigger.synced(Instant::now());
                    return;
                }
            },
            Presence::Absent => return,
        };
        if self
            .settings
            .watch
            .as_ref()
            .is_some_and(|watch| *watch != fingerprint)
        {
            // Joined a different logger than the one being watched.
            if joined_here {
                self.leave(true);
            }
            self.trigger.synced(Instant::now());
            return;
        }
        let downloaded = match self.list(&fingerprint) {
            Some(logs) => {
                let folder = self.folder(&fingerprint);
                match folder {
                    Ok(folder) => {
                        let today = chrono::Local::now().date_naive();
                        let fetch = policy::to_fetch(
                            &logs,
                            |log| folder.downloaded(log).is_some(),
                            self.settings.since,
                            today,
                        )
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>();
                        let import = self.settings.after_download == AfterDownload::Import;
                        self.download(&fingerprint, &fetch, import, true)
                    }
                    Err(error) => {
                        self.log(Tone::Bad, format!("Library: {error}"));
                        0
                    }
                }
            }
            None => 0,
        };
        if joined_here {
            self.leave(true);
        }
        if matches!(self.presence, Presence::Absent) && !joined_here {
            self.trigger.lost();
        } else {
            self.trigger.synced(Instant::now());
        }
        self.emit(Event::Synced {
            automatic: true,
            downloaded,
        });
    }

    fn download_named(&mut self, names: &[String], import: bool) {
        let Some((fingerprint, logs)) = self.listed.clone() else {
            self.log(Tone::Warn, "Not connected to a logger");
            return;
        };
        let chosen = logs
            .into_iter()
            .filter(|log| names.contains(&log.name))
            .collect::<Vec<_>>();
        let downloaded = self.download(&fingerprint, &chosen, import, false);
        self.emit(Event::Synced {
            automatic: false,
            downloaded,
        });
    }

    /// Download `logs` into the library in one session; the count stored.
    fn download(
        &mut self,
        fingerprint: &str,
        logs: &[Datalog],
        import: bool,
        automatic: bool,
    ) -> usize {
        if logs.is_empty() {
            return 0;
        }
        let mut folder = match self.folder(fingerprint) {
            Ok(folder) => folder,
            Err(error) => {
                self.log(Tone::Bad, format!("Library: {error}"));
                return 0;
            }
        };
        self.cancel.store(false, Ordering::Relaxed);
        let mut stored = 0;
        let mut session = None;
        for (index, log) in logs.iter().enumerate() {
            if self.cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut bytes = Vec::with_capacity(log.size as usize);
            let events = self.events.clone();
            let ctx = self.ctx.clone();
            let name = log.name.clone();
            let count = logs.len();
            let mut progress = move |done: u64, total: u64| {
                let _ = events.send(Event::Activity(Activity::Downloading {
                    name: name.clone(),
                    index,
                    count,
                    done,
                    total,
                }));
                ctx.request_repaint();
            };
            if session.is_none() {
                session = match self.session() {
                    Ok(session) => Some(session),
                    Err(error) => {
                        self.log(Tone::Bad, format!("Could not connect: {error}"));
                        break;
                    }
                };
            }
            let result = session.as_mut().unwrap().download_datalog(
                log,
                &mut bytes,
                &mut progress,
                &self.cancel,
            );
            match result {
                Ok(_) => match folder.store(log, &bytes) {
                    Ok(path) => {
                        stored += 1;
                        self.log(Tone::Good, format!("Downloaded {}", describe(log)));
                        self.emit(Event::Downloaded {
                            row: Box::new(Row {
                                log: log.clone(),
                                downloaded: Some(path),
                            }),
                            import,
                            automatic,
                        });
                    }
                    Err(error) => {
                        self.log(Tone::Bad, format!("{} not saved: {error}", log.name));
                    }
                },
                Err(LoggerError::Cancelled) => {
                    self.log(Tone::Neutral, "Download cancelled");
                    break;
                }
                Err(error) => {
                    self.log(Tone::Bad, format!("{} failed: {error}", log.name));
                    // The session is out of step after an error; reconnect
                    // for the next file, or stop if the logger is gone.
                    session = None;
                    if error.is_unreachable() {
                        break;
                    }
                }
            }
        }
        if let Some(session) = session {
            session.close();
        }
        self.activity(Activity::Idle);
        stored
    }

    fn set_clock(&mut self) {
        if !matches!(self.presence, Presence::Connected { .. }) {
            return;
        }
        self.activity(Activity::SettingClock);
        let result = self.session().and_then(|mut session| {
            let clock = ClockWrite::now();
            let info = session.device_info(clock)?;
            session.time_negotiate(clock)?;
            session.close();
            Ok(info)
        });
        self.activity(Activity::Idle);
        match result {
            Ok(info) => {
                self.log(Tone::Good, "Logger clock set");
                self.emit(Event::DeviceInfo(DeviceSummary {
                    driver: info.driver().map(str::to_owned),
                    vehicle: info.vehicle().map(str::to_owned),
                    region: info.hardware.region().map(str::to_owned),
                }));
            }
            Err(error) => self.log(Tone::Bad, format!("Could not set the clock: {error}")),
        }
    }
}

/// `a_0091 · KELLYS · 5 laps`
pub fn describe(log: &Datalog) -> String {
    let mut parts = vec![
        log.name
            .trim_end_matches(".xrz")
            .trim_end_matches(".hrz")
            .to_owned(),
    ];
    if !log.track.is_empty() {
        parts.push(log.track.clone());
    }
    if let Some(laps) = log.laps {
        parts.push(format!("{laps} lap{}", if laps == 1 { "" } else { "s" }));
    }
    parts.join(" · ")
}
