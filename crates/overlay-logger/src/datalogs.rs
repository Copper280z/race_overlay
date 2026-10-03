//! The saved-datalog list (NC `0x24/2`): CRLF CSV, one row per recording.
//!
//! ```text
//! name,size,date,hour,nlap,nbest,best,pilota,track_name,veicolo,campionato,
//! venue_type,mode,trk_type,motivolap,maxvel,device,track_lat,track_lon,
//! test_dur,pname,ptype,ptime,pdist,pmaxv,valid,
//! ```

use crate::error::{LoggerError, Result};
use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use std::collections::BTreeMap;

/// One recording stored on the logger.
#[derive(Clone, Debug, PartialEq)]
pub struct Datalog {
    /// File name in the logger's memory, e.g. `a_0089.xrz`.
    pub name: String,
    /// Exact byte size of the download.
    pub size: u64,
    /// Local wall-clock start time from `date` (`dd/mm/yyyy`) and `hour`.
    pub recorded: Option<NaiveDateTime>,
    pub laps: Option<u32>,
    /// Lap number of the best lap (`nbest`).
    pub best_lap_number: Option<u32>,
    /// Best lap time in milliseconds (`best`).
    pub best_lap_ms: Option<u32>,
    pub driver: String,
    pub track: String,
    pub vehicle: String,
    pub championship: String,
    pub venue_type: String,
    /// Logging mode, e.g. `speed`.
    pub mode: String,
    /// Track type, e.g. `closed`.
    pub track_type: String,
    /// Why the recording ended (`motivolap`), e.g. `stop`.
    pub stop_reason: String,
    /// `maxvel` exactly as sent. Its unit is unconfirmed (see the protocol
    /// notes), so it is not converted.
    pub max_speed_raw: Option<i64>,
    pub device: String,
    /// Track reference point in degrees (sent as degrees × 1e7).
    pub track_position: Option<(f64, f64)>,
    /// Recording length in milliseconds (`test_dur`).
    pub duration_ms: Option<u64>,
    pub valid: String,
    /// Non-empty columns not listed above (`pname`, `ptype`, …), by header.
    pub other: BTreeMap<String, String>,
}

impl Datalog {
    /// Device path the file is opened by.
    pub fn device_path(&self) -> String {
        format!("1:/mem/{}", self.name)
    }
}

const KNOWN: [&str; 21] = [
    "name",
    "size",
    "date",
    "hour",
    "nlap",
    "nbest",
    "best",
    "pilota",
    "track_name",
    "veicolo",
    "campionato",
    "venue_type",
    "mode",
    "trk_type",
    "motivolap",
    "maxvel",
    "device",
    "track_lat",
    "track_lon",
    "test_dur",
    "valid",
];

pub fn parse(body: &[u8]) -> Result<Vec<Datalog>> {
    let end = body.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(&body[..end]);
    let headers = reader
        .headers()
        .map_err(|error| LoggerError::Protocol(format!("datalog list: {error}")))?
        .iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if !headers.iter().any(|h| h == "name") || !headers.iter().any(|h| h == "size") {
        return Err(LoggerError::Protocol(
            "datalog list has no name/size columns".into(),
        ));
    }
    let mut logs = Vec::new();
    for row in reader.records() {
        let row = row.map_err(|error| LoggerError::Protocol(format!("datalog list: {error}")))?;
        let field = |name: &str| {
            headers
                .iter()
                .position(|h| h == name)
                .and_then(|i| row.get(i))
                .unwrap_or("")
                .to_owned()
        };
        let name = field("name");
        let Ok(size) = field("size").parse::<u64>() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let number = |name: &str| field(name).parse::<i64>().ok();
        let other = headers
            .iter()
            .zip(row.iter())
            .filter(|(header, value)| {
                !header.is_empty() && !value.is_empty() && !KNOWN.contains(&header.as_str())
            })
            .map(|(header, value)| (header.clone(), value.to_owned()))
            .collect();
        logs.push(Datalog {
            name,
            size,
            recorded: recorded(&field("date"), &field("hour")),
            laps: number("nlap").and_then(|v| u32::try_from(v).ok()),
            best_lap_number: number("nbest").and_then(|v| u32::try_from(v).ok()),
            best_lap_ms: number("best")
                .and_then(|v| u32::try_from(v).ok())
                .filter(|&v| v > 0),
            driver: field("pilota"),
            track: field("track_name"),
            vehicle: field("veicolo"),
            championship: field("campionato"),
            venue_type: field("venue_type"),
            mode: field("mode"),
            track_type: field("trk_type"),
            stop_reason: field("motivolap"),
            max_speed_raw: number("maxvel"),
            device: field("device"),
            track_position: number("track_lat")
                .zip(number("track_lon"))
                .filter(|&(lat, lon)| lat != 0 || lon != 0)
                .map(|(lat, lon)| (lat as f64 * 1e-7, lon as f64 * 1e-7)),
            duration_ms: number("test_dur").and_then(|v| u64::try_from(v).ok()),
            valid: field("valid"),
            other,
        });
    }
    Ok(logs)
}

fn recorded(date: &str, hour: &str) -> Option<NaiveDateTime> {
    let date = NaiveDate::parse_from_str(date, "%d/%m/%Y").ok()?;
    let time = NaiveTime::parse_from_str(hour, "%H:%M:%S").ok()?;
    Some(date.and_time(time))
}

/// CSV body for a list of datalogs, in the logger's column order. Used by the
/// fake logger and by tests.
#[cfg(any(test, feature = "fake"))]
pub(crate) fn encode(logs: &[Datalog]) -> Vec<u8> {
    let mut out = String::from(
        "name,size,date,hour,nlap,nbest,best,pilota,track_name,veicolo,campionato,\
         venue_type,mode,trk_type,motivolap,maxvel,device,track_lat,track_lon,\
         test_dur,pname,ptype,ptime,pdist,pmaxv,valid,\r\n",
    );
    let opt = |v: Option<String>| v.unwrap_or_default();
    for log in logs {
        let (lat, lon) = log
            .track_position
            .map(|(lat, lon)| {
                (
                    ((lat * 1e7).round() as i64).to_string(),
                    ((lon * 1e7).round() as i64).to_string(),
                )
            })
            .unwrap_or_default();
        let fields = [
            log.name.clone(),
            log.size.to_string(),
            opt(log.recorded.map(|t| t.format("%d/%m/%Y").to_string())),
            opt(log.recorded.map(|t| t.format("%H:%M:%S").to_string())),
            opt(log.laps.map(|v| v.to_string())),
            opt(log.best_lap_number.map(|v| v.to_string())),
            opt(log.best_lap_ms.map(|v| v.to_string())),
            log.driver.clone(),
            log.track.clone(),
            log.vehicle.clone(),
            log.championship.clone(),
            log.venue_type.clone(),
            log.mode.clone(),
            log.track_type.clone(),
            log.stop_reason.clone(),
            opt(log.max_speed_raw.map(|v| v.to_string())),
            log.device.clone(),
            lat,
            lon,
            opt(log.duration_ms.map(|v| v.to_string())),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            log.valid.clone(),
        ];
        out.push_str(&fields.join(","));
        out.push_str(",\r\n");
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURED: &str = "name,size,date,hour,nlap,nbest,best,pilota,track_name,veicolo,campionato,venue_type,mode,trk_type,motivolap,maxvel,device,track_lat,track_lon,test_dur,pname,ptype,ptime,pdist,pmaxv,valid,\r\n\
        a_0089.xrz,477678,30/08/2026,15:33:28,2,,,,,,,,speed,closed,stop,1079717068,,420123456,-710654321,141383,,,,,,,\r\n\
        a_0034.hrz,90000,01/07/2025,09:05:00,6,4,36605,,KELLYS,,,,speed,closed,stop,1079717068,,,,200000,x,,,,,1,\r\n\0\0";

    #[test]
    fn parses_the_captured_row() {
        let logs = parse(CAPTURED.as_bytes()).unwrap();
        assert_eq!(logs.len(), 2);
        let log = &logs[0];
        assert_eq!(log.name, "a_0089.xrz");
        assert_eq!(log.size, 477_678);
        assert_eq!(log.recorded.unwrap().to_string(), "2026-08-30 15:33:28");
        assert_eq!(log.laps, Some(2));
        assert_eq!(log.best_lap_ms, None);
        assert_eq!(log.mode, "speed");
        assert_eq!(log.stop_reason, "stop");
        assert_eq!(log.max_speed_raw, Some(1_079_717_068));
        let (lat, lon) = log.track_position.unwrap();
        assert!((lat - 42.012_345_6).abs() < 1e-9 && (lon + 71.065_432_1).abs() < 1e-9);
        assert_eq!(log.duration_ms, Some(141_383));
        assert_eq!(log.device_path(), "1:/mem/a_0089.xrz");

        let older = &logs[1];
        assert_eq!(older.best_lap_ms, Some(36_605));
        assert_eq!(older.best_lap_number, Some(4));
        assert_eq!(older.track, "KELLYS");
        assert_eq!(older.track_position, None);
        assert_eq!(older.other.get("pname").map(String::as_str), Some("x"));
        assert_eq!(older.valid, "1");
    }

    #[test]
    fn encoding_round_trips() {
        let logs = parse(CAPTURED.as_bytes()).unwrap();
        let again = parse(&encode(&logs)).unwrap();
        assert_eq!(again.len(), logs.len());
        assert_eq!(again[0].recorded, logs[0].recorded);
        assert_eq!(again[0].track_position, logs[0].track_position);
        assert_eq!(again[1].best_lap_ms, logs[1].best_lap_ms);
    }

    #[test]
    fn rejects_bodies_that_are_not_a_datalog_list() {
        assert!(parse(b"System\0\0\x15").is_err());
        assert!(parse(b"").is_err());
    }
}
