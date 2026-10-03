//! Pure decisions for MyChron downloads: what Track mode fetches, when it
//! syncs, and where files go in the library.

use super::settings::Since;
use chrono::NaiveDate;
use overlay_logger::Datalog;
use std::time::{Duration, Instant};

/// Whether a recording's date passes the Track mode filter. Undated
/// recordings only pass "any date": their age is unknown.
pub fn within(log: &Datalog, since: Since, today: NaiveDate) -> bool {
    let date = log.recorded.map(|t| t.date());
    match since {
        Since::AnyDate => true,
        Since::Today => date == Some(today),
        Since::LastWeek => date.is_some_and(|d| d <= today && (today - d).num_days() < 7),
    }
}

/// Recordings Track mode should download: not downloaded before and within
/// the date filter, oldest first.
pub fn to_fetch(
    logs: &[Datalog],
    downloaded: impl Fn(&Datalog) -> bool,
    since: Since,
    today: NaiveDate,
) -> Vec<&Datalog> {
    let mut fetch = logs
        .iter()
        .filter(|log| !downloaded(log) && within(log, since, today))
        .collect::<Vec<_>>();
    fetch.sort_by(|a, b| a.recorded.cmp(&b.recorded).then(a.name.cmp(&b.name)));
    fetch
}

/// Library path of a recording below its logger's folder:
/// `2026-08-30/2026-08-30 1533 KELLYS a_0089.xrz`. The logger's own file
/// name stays last so it can be matched back; the date and track make the
/// folder readable.
pub fn library_path(log: &Datalog) -> std::path::PathBuf {
    let day = log.recorded.map_or_else(
        || "Undated".to_owned(),
        |t| t.format("%Y-%m-%d").to_string(),
    );
    let mut name = String::new();
    if let Some(time) = log.recorded {
        name.push_str(&time.format("%Y-%m-%d %H%M ").to_string());
    }
    let track = sanitize(&log.track);
    if !track.is_empty() {
        name.push_str(&track);
        name.push(' ');
    }
    name.push_str(&sanitize(&log.name));
    std::path::Path::new(&day).join(name)
}

/// File-name-safe text on every platform.
pub fn sanitize(text: &str) -> String {
    let cleaned = text
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect::<String>();
    cleaned.trim().trim_matches('.').to_owned()
}

/// When Track mode syncs, fed whether the logger is in range.
///
/// A logger must stay in range for `settle` before the first sync, so a kart
/// passing the pits mid-session does not start one. While it stays, Track
/// mode checks again every `recheck` (a session ends some time after the
/// kart stops). Leaving range resets the cycle.
#[derive(Clone, Debug)]
pub struct Trigger {
    state: TriggerState,
    pub settle: Duration,
    pub recheck: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TriggerState {
    Away,
    Arrived(Instant),
    Synced(Instant),
}

impl Trigger {
    pub fn new(settle: Duration, recheck: Duration) -> Self {
        Self {
            state: TriggerState::Away,
            settle,
            recheck,
        }
    }

    /// Record an observation; `true` when a sync is due now.
    pub fn observe(&mut self, now: Instant, in_range: bool) -> bool {
        if !in_range {
            self.state = TriggerState::Away;
            return false;
        }
        match self.state {
            TriggerState::Away => {
                self.state = TriggerState::Arrived(now);
                self.settle.is_zero()
            }
            TriggerState::Arrived(since) => now.duration_since(since) >= self.settle,
            TriggerState::Synced(at) => now.duration_since(at) >= self.recheck,
        }
    }

    /// A sync finished (or failed while the logger stayed in range).
    pub fn synced(&mut self, now: Instant) {
        self.state = TriggerState::Synced(now);
    }

    /// The logger went away (e.g. the sync could not reach it).
    pub fn lost(&mut self) {
        self.state = TriggerState::Away;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;

    fn log(name: &str, recorded: Option<&str>) -> Datalog {
        let mut log = overlay_logger_fixture(name);
        log.recorded =
            recorded.map(|t| NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S").unwrap());
        log
    }

    fn overlay_logger_fixture(name: &str) -> Datalog {
        let mut device = overlay_logger::fake::FakeDevice::default();
        device.add_recording(name, "2026-01-01 00:00:00", "KELLYS", vec![1, 2, 3]);
        device.recordings.remove(0).log
    }

    #[test]
    fn date_filter_and_history() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 30).unwrap();
        let logs = vec![
            log("a_0003.xrz", Some("2026-08-30 15:00:00")),
            log("a_0002.xrz", Some("2026-08-30 09:00:00")),
            log("a_0001.xrz", Some("2026-08-20 09:00:00")),
            log("a_0000.xrz", None),
        ];
        let names = |since| {
            to_fetch(&logs, |l| l.name == "a_0003.xrz", since, today)
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
        };
        // Oldest first, never what was downloaded before.
        assert_eq!(
            names(Since::AnyDate),
            ["a_0000.xrz", "a_0001.xrz", "a_0002.xrz"]
        );
        assert_eq!(names(Since::Today), ["a_0002.xrz"]);
        assert_eq!(names(Since::LastWeek), ["a_0002.xrz"]);
        let week_old = log("a_0004.xrz", Some("2026-08-24 09:00:00"));
        assert!(within(&week_old, Since::LastWeek, today));
        let future = log("a_0005.xrz", Some("2026-09-01 09:00:00"));
        assert!(!within(&future, Since::LastWeek, today));
    }

    #[test]
    fn library_paths_are_readable_and_safe() {
        let mut named = log("a_0089.xrz", Some("2026-08-30 15:33:28"));
        named.track = "Kelly's: North/Loop".into();
        assert_eq!(
            library_path(&named),
            std::path::Path::new("2026-08-30")
                .join("2026-08-30 1533 Kelly's_ North_Loop a_0089.xrz")
        );
        let mut undated = log("a_0001.xrz", None);
        undated.track.clear();
        assert_eq!(
            library_path(&undated),
            std::path::Path::new("Undated").join("a_0001.xrz")
        );
        assert_eq!(sanitize(" ..x.. "), "x");
    }

    #[test]
    fn trigger_settles_rechecks_and_resets() {
        let start = Instant::now();
        let at = |s: u64| start + Duration::from_secs(s);
        let mut trigger = Trigger::new(Duration::from_secs(30), Duration::from_secs(300));
        assert!(!trigger.observe(at(0), false));
        // Arrives; a pass-by shorter than the settle time never syncs.
        assert!(!trigger.observe(at(10), true));
        assert!(!trigger.observe(at(20), false));
        assert!(!trigger.observe(at(25), true));
        assert!(!trigger.observe(at(50), true));
        assert!(trigger.observe(at(55), true));
        trigger.synced(at(60));
        assert!(!trigger.observe(at(200), true));
        assert!(trigger.observe(at(360), true));
        trigger.synced(at(361));
        // Leaving and returning starts over with the settle delay.
        assert!(!trigger.observe(at(400), false));
        assert!(!trigger.observe(at(401), true));
        assert!(trigger.observe(at(431), true));

        let mut eager = Trigger::new(Duration::ZERO, Duration::from_secs(60));
        assert!(eager.observe(at(0), true));
        eager.lost();
        assert!(eager.observe(at(1), true));
    }
}
