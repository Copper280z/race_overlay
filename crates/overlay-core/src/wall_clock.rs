//! Matching camera recordings to logger sessions by when they were recorded.
//!
//! Cameras name their files after the local start time
//! (`VID_20260830_124108_00_017.insv`) and loggers store a session date and
//! time (AiM: `08/30/2026` `12:42:36`). Both are read as *local wall-clock*
//! times and assumed to be in the same zone, which holds when both devices
//! were set up where the session took place. A match is only a pairing hint:
//! device clocks drift, so it says which files belong together, never how the
//! two clocks relate. That is still found from the signals themselves.

use crate::TelemetryDataset;
use std::{collections::BTreeMap, path::Path};

/// Wall-clock instants are seconds since 1970-01-01 00:00:00 *without* a time
/// zone, so two local times compare correctly with each other and with
/// nothing else.
pub type WallClock = f64;

/// Device clocks are rarely exact. Sessions closer than this are still
/// treated as overlapping.
pub const CLOCK_SLACK_SECONDS: f64 = 60.0;

/// Local start time encoded in a camera file name such as
/// `VID_20260830_124108_00_017.insv` (`YYYYMMDD_HHMMSS`).
pub fn camera_start_from_file_name(path: &Path) -> Option<WallClock> {
    let name = path.file_stem()?.to_str()?;
    let parts = name.split('_').collect::<Vec<_>>();
    parts.windows(2).find_map(|pair| {
        let (date, time) = (pair[0], pair[1]);
        let digits = |text: &str, length| {
            (text.len() == length && text.bytes().all(|b| b.is_ascii_digit()))
                .then(|| text.parse::<i64>().ok())
                .flatten()
        };
        let (date, time) = (digits(date, 8)?, digits(time, 6)?);
        civil_to_wall_clock(
            date / 10_000,
            date / 100 % 100,
            date % 100,
            time / 10_000,
            time / 100 % 100,
            time % 100,
        )
    })
}

/// Local start time recorded by a logger importer in its dataset metadata
/// (`date`, `time`). Dates are `MM/DD/YYYY`, or `DD/MM/YYYY` when the first
/// number cannot be a month.
pub fn logger_start_from_metadata(metadata: &BTreeMap<String, String>) -> Option<WallClock> {
    let numbers = |text: &str, separator| {
        text.trim()
            .split(separator)
            .map(|part| part.trim().parse::<i64>().ok())
            .collect::<Option<Vec<_>>>()
    };
    let date = numbers(metadata.get("date")?, '/')?;
    let time = numbers(metadata.get("time")?, ':')?;
    let [first, second, year] = date.as_slice() else {
        return None;
    };
    let [hour, minute, second_of_minute] = time.as_slice() else {
        return None;
    };
    let (first, second, year) = (*first, *second, *year);
    let (month, day) = if first > 12 {
        (second, first)
    } else {
        (first, second)
    };
    civil_to_wall_clock(year, month, day, *hour, *minute, *second_of_minute)
}

/// The first and last sample time of a dataset, in its own clock.
pub fn dataset_span(dataset: &TelemetryDataset) -> Option<(f64, f64)> {
    dataset
        .channels
        .values()
        .flat_map(|channel| channel.series.samples.iter())
        .map(|sample| sample.time)
        .filter(|time| time.is_finite())
        .fold(None, |span: Option<(f64, f64)>, time| {
            Some(span.map_or((time, time), |(low, high)| (low.min(time), high.max(time))))
        })
}

/// A recording's place on the wall clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallClockSpan {
    pub start: WallClock,
    pub seconds: f64,
}

impl WallClockSpan {
    pub fn new(start: WallClock, seconds: f64) -> Option<Self> {
        (start.is_finite() && seconds.is_finite() && seconds > 0.0)
            .then_some(Self { start, seconds })
    }

    /// Whether the two sessions ran at the same time, allowing for
    /// [`CLOCK_SLACK_SECONDS`] of clock disagreement.
    pub fn coincides_with(&self, other: &Self) -> bool {
        self.start < other.start + other.seconds + CLOCK_SLACK_SECONDS
            && other.start < self.start + self.seconds + CLOCK_SLACK_SECONDS
    }
}

/// Seconds since 1970-01-01 for a civil date and time, with no zone.
fn civil_to_wall_clock(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> Option<WallClock> {
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..61).contains(&second)
    {
        return None;
    }
    // Days from civil (Howard Hinnant's algorithm).
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_camera_start_from_the_file_name() {
        let start = camera_start_from_file_name(Path::new("/x/VID_20260830_124108_00_017.insv"));
        assert_eq!(start, civil_to_wall_clock(2026, 8, 30, 12, 41, 8),);
        // The year-2026 reference value, checked independently.
        assert_eq!(start, Some(1_788_093_668.0));
        assert!(camera_start_from_file_name(Path::new("clip.mp4")).is_none());
        assert!(camera_start_from_file_name(Path::new("VID_20261340_124108.insv")).is_none());
    }

    #[test]
    fn reads_logger_start_from_metadata() {
        let metadata = |date: &str, time: &str| {
            BTreeMap::from([
                ("date".to_owned(), date.to_owned()),
                ("time".to_owned(), time.to_owned()),
            ])
        };
        assert_eq!(
            logger_start_from_metadata(&metadata("08/30/2026", "12:42:36")),
            civil_to_wall_clock(2026, 8, 30, 12, 42, 36)
        );
        assert_eq!(
            logger_start_from_metadata(&metadata("30/08/2026", "12:42:36")),
            civil_to_wall_clock(2026, 8, 30, 12, 42, 36)
        );
        assert!(logger_start_from_metadata(&BTreeMap::new()).is_none());
        assert!(logger_start_from_metadata(&metadata("garbage", "12:42:36")).is_none());
    }

    #[test]
    fn sessions_coincide_when_they_run_at_the_same_time() {
        let video = WallClockSpan::new(civil_to_wall_clock(2026, 8, 30, 12, 41, 8).unwrap(), 190.0)
            .unwrap();
        let during = |hour, minute, second| {
            WallClockSpan::new(
                civil_to_wall_clock(2026, 8, 30, hour, minute, second).unwrap(),
                172.0,
            )
            .unwrap()
        };
        assert!(video.coincides_with(&during(12, 42, 36)));
        // Started before the video, ended just as it began.
        assert!(video.coincides_with(&during(12, 38, 30)));
        assert!(!video.coincides_with(&during(12, 34, 29)));
        assert!(!video.coincides_with(&during(12, 48, 19)));
        assert!(WallClockSpan::new(0.0, 0.0).is_none());
    }
}
