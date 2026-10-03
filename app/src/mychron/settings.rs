//! MyChron preferences, saved in eframe storage as application settings (not
//! workspace data). Unknown fields survive a load/save round trip.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

pub const STORAGE_KEY: &str = "race-overlay.mychron";

/// How the computer reaches a logger's hotspot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    /// The user (or the OS, for a known network) joins it; the app only
    /// looks for the logger.
    #[default]
    UserJoins,
    /// The app joins the logger's hotspot for each sync, then returns to the
    /// previous network.
    AppSwitches,
}

/// Which recordings Track mode downloads, by recording date.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Since {
    #[default]
    AnyDate,
    Today,
    LastWeek,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AfterDownload {
    #[default]
    Import,
    KeepInLibrary,
}

/// A logger the app has seen.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KnownLogger {
    /// From the logger's UDP descriptor; stable across power cycles.
    pub fingerprint: String,
    /// What the user calls it.
    pub name: String,
    /// Library sub-folder, fixed when first seen so renaming keeps the files
    /// and download history together.
    pub folder: String,
    /// Hotspot name, once known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssid: Option<String>,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl KnownLogger {
    pub fn new(fingerprint: &str, ssid: Option<&str>) -> Self {
        let short = &fingerprint[..fingerprint.len().min(8)];
        Self {
            fingerprint: fingerprint.to_owned(),
            name: ssid.map_or_else(|| "MyChron".to_owned(), str::to_owned),
            folder: format!("MyChron {short}"),
            ssid: ssid.map(str::to_owned),
            unknown: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MyChronSettings {
    pub track_mode: bool,
    pub connection: Connection,
    /// Wi-Fi interface used to switch networks; the system default when unset.
    pub interface: Option<String>,
    pub loggers: Vec<KnownLogger>,
    /// Track mode syncs only this logger (by fingerprint); any logger when unset.
    pub watch: Option<String>,
    pub since: Since,
    pub after_download: AfterDownload,
    /// Library folder; [`crate::app_paths::mychron_library_dir`] when unset.
    pub library: Option<PathBuf>,
    /// Seconds a logger must stay in range before Track mode syncs, so a
    /// kart passing the pits does not start a download.
    pub settle_seconds: u32,
    /// Minutes between re-checks while the logger stays in range.
    pub recheck_minutes: u32,
    pub keep_awake: bool,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}

impl Default for MyChronSettings {
    fn default() -> Self {
        Self {
            track_mode: false,
            connection: Connection::default(),
            interface: None,
            loggers: Vec::new(),
            watch: None,
            since: Since::default(),
            after_download: AfterDownload::default(),
            library: None,
            settle_seconds: 30,
            recheck_minutes: 5,
            keep_awake: false,
            unknown: BTreeMap::new(),
        }
    }
}

impl MyChronSettings {
    pub fn library_root(&self) -> PathBuf {
        self.library
            .clone()
            .unwrap_or_else(crate::app_paths::mychron_library_dir)
    }

    pub fn logger(&self, fingerprint: &str) -> Option<&KnownLogger> {
        self.loggers.iter().find(|l| l.fingerprint == fingerprint)
    }

    /// Remembers a logger the first time it is seen; fills in its hotspot
    /// name once known. Returns whether anything changed.
    pub fn remember(&mut self, fingerprint: &str, ssid: Option<&str>) -> bool {
        match self
            .loggers
            .iter_mut()
            .find(|l| l.fingerprint == fingerprint)
        {
            Some(known) => {
                if known.ssid.is_none() && ssid.is_some() {
                    known.ssid = ssid.map(str::to_owned);
                    if known.name == "MyChron" {
                        known.name = known.ssid.clone().unwrap_or_default();
                    }
                    true
                } else {
                    false
                }
            }
            None => {
                self.loggers.push(KnownLogger::new(fingerprint, ssid));
                true
            }
        }
    }
}

pub fn load(saved: Option<String>) -> MyChronSettings {
    saved
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save(settings: &MyChronSettings) -> String {
    serde_json::to_string(settings).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_keeps_unknown_fields() {
        let saved = r#"{"track_mode":true,"connection":"app_switches","future":1,
            "loggers":[{"fingerprint":"0123456789abcdef","name":"Kart","folder":"MyChron 01234567","extra":"x"}]}"#;
        let settings = load(Some(saved.into()));
        assert!(settings.track_mode);
        assert_eq!(settings.connection, Connection::AppSwitches);
        assert_eq!(settings.settle_seconds, 30);
        let again: Value = serde_json::from_str(&save(&settings)).unwrap();
        assert_eq!(again["future"], 1);
        assert_eq!(again["loggers"][0]["extra"], "x");
        assert_eq!(load(Some("not json".into())), MyChronSettings::default());
    }

    #[test]
    fn remembering_keeps_the_folder_when_the_name_is_learned_later() {
        let mut settings = MyChronSettings::default();
        assert!(settings.remember("0123456789abcdef", None));
        assert!(!settings.remember("0123456789abcdef", None));
        assert_eq!(settings.loggers[0].folder, "MyChron 01234567");
        assert!(settings.remember("0123456789abcdef", Some("AiM-MYC6-42")));
        let logger = &settings.loggers[0];
        assert_eq!(logger.name, "AiM-MYC6-42");
        assert_eq!(logger.folder, "MyChron 01234567");
    }
}
