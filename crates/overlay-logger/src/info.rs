//! Device info blob (NC `0x10/1`, ~4 279 bytes): records tagged `iMST`,
//! `iHW `, `iUSR`, `iPTH`, and several whose contents are not decoded yet
//! (`iLCK`, `iSST`, `iLTS`, `iPRL`), which stay available as raw [`Record`]s.
//! Text records hold `key=value|` lines separated by CR/LF.

use crate::records::{self, Record};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    /// The blob exactly as received.
    pub raw: Vec<u8>,
    pub records: Vec<Record>,
    pub hardware: HardwareInfo,
    /// `USR` profile metadata, `key=value` (`device`, `pilota`, `veicolo`,
    /// `campionato`, `venue_type`, `desired_racem`, `vehicle_type`, …).
    pub user: BTreeMap<String, String>,
    /// `PTH` device path map, e.g. `tracks` → `0:/gps` (index `0:/gps.N`).
    pub paths: BTreeMap<String, DevicePath>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HardwareInfo {
    /// `key=value` parts of the `HW ` record (`WiFi`, `Reg`, `Led`).
    pub fields: BTreeMap<String, String>,
    /// Bare parts, in order (IMU part number, board id, …).
    pub parts: Vec<String>,
}

impl HardwareInfo {
    pub fn wifi_chip(&self) -> Option<&str> {
        self.fields.get("WiFi").map(String::as_str)
    }

    /// Regulatory region, e.g. `usa`.
    pub fn region(&self) -> Option<&str> {
        self.fields.get("Reg").map(String::as_str)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DevicePath {
    pub directory: String,
    pub index: String,
}

impl DeviceInfo {
    pub fn parse(blob: &[u8]) -> Self {
        let records = records::parse(blob);
        let mut info = Self::default();
        for record in &records {
            let text = String::from_utf8_lossy(&record.payload);
            match short_name(record) {
                "HW" => info.hardware = parse_hardware(&text),
                "USR" => info.user = key_values(&text),
                "PTH" => {
                    info.paths = key_values(&text)
                        .into_iter()
                        .map(|(name, value)| {
                            let (directory, index) = value.split_once(',').unwrap_or((&value, ""));
                            let path = DevicePath {
                                directory: directory.to_owned(),
                                index: index.to_owned(),
                            };
                            (name, path)
                        })
                        .collect()
                }
                _ => {}
            }
        }
        info.records = records;
        info.raw = blob.to_vec();
        info
    }

    /// A record by its name without the `i` prefix (`MST`, `HW`, `LCK`, …).
    pub fn record(&self, name: &str) -> Option<&Record> {
        self.records
            .iter()
            .find(|record| short_name(record) == name)
    }

    /// The `MST` record, which nests the same `idn` identity bytes the UDP
    /// descriptor carries (two build bytes differ; see the protocol notes).
    pub fn identity(&self) -> Option<&[u8]> {
        self.record("MST").map(|record| record.payload.as_slice())
    }

    pub fn driver(&self) -> Option<&str> {
        self.user_field("pilota")
    }

    pub fn vehicle(&self) -> Option<&str> {
        self.user_field("veicolo")
    }

    pub fn championship(&self) -> Option<&str> {
        self.user_field("campionato")
    }

    /// The user-assigned device name.
    pub fn device_name(&self) -> Option<&str> {
        self.user_field("device")
    }

    fn user_field(&self, key: &str) -> Option<&str> {
        self.user
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
}

fn short_name(record: &Record) -> &str {
    let name = record.name();
    name.strip_prefix('i').unwrap_or(name)
}

fn parse_hardware(text: &str) -> HardwareInfo {
    let mut hardware = HardwareInfo::default();
    for part in text.trim_end_matches('\0').split('|') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('=') {
            Some((key, value)) => {
                hardware.fields.insert(key.to_owned(), value.to_owned());
            }
            None => hardware.parts.push(part.to_owned()),
        }
    }
    hardware
}

/// `key=value` lines separated by CR/LF. A line may start with framing bytes
/// (`USR` nests another header before its first line), so the key is the run
/// of identifier characters just before `=`.
fn key_values(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.split(['\r', '\n']) {
        let Some((head, value)) = line.split_once('=') else {
            continue;
        };
        let key_start = head
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
            .map_or(0, |(index, c)| index + c.len_utf8());
        let key = &head[key_start..];
        if !key.is_empty() {
            let value = value.trim_end_matches(['\0', '|']).trim();
            map.insert(key.to_owned(), value.to_owned());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::encode;

    /// Records as a MyChron6 sent them (2026-10-02), with the long ones cut.
    #[test]
    fn decodes_a_real_info_blob() {
        let record = |tag: &[u8; 4], payload: &[u8]| encode(tag, b'a', payload);
        let mut blob = record(b"iMST", b"idn\x01\x38\x00\x0e\x02\xb5\x01");
        blob.extend(record(
            b"iHW ",
            b"WiFi=ESP32|Reg=usa|LSM6DSV16X|Led=PI33TB|MYC68B|M101|   \0",
        ));
        blob.extend(record(
            b"iUSR",
            b"<hUSR 0000000092a>\r\ndevice=|\r\npilota=Sam|\r\nveicolo=KA100|\r\n\
              campionato=|\r\nvehicle_type=|\r\n<USR 008273>",
        ));
        blob.extend(record(
            b"iPTH",
            b"media=0:,0:.N|\r\nsettings=0:/set,0:/set.N|\r\ntracks=0:/gps,0:/gps.N|\r\n",
        ));
        blob.extend(record(b"iLCK", &[0; 47]));
        let info = DeviceInfo::parse(&blob);
        assert_eq!(info.records.len(), 5);
        assert!(info.records.iter().all(|r| r.checksum_ok));
        assert_eq!(info.hardware.wifi_chip(), Some("ESP32"));
        assert_eq!(info.hardware.region(), Some("usa"));
        assert_eq!(info.hardware.parts, ["LSM6DSV16X", "MYC68B", "M101"]);
        assert_eq!(info.driver(), Some("Sam"));
        assert_eq!(info.vehicle(), Some("KA100"));
        assert_eq!(info.championship(), None);
        assert_eq!(info.user.len(), 5);
        assert_eq!(info.paths["tracks"].directory, "0:/gps");
        assert_eq!(info.paths["media"].index, "0:.N");
        assert_eq!(info.identity().unwrap()[..3], *b"idn");
        assert_eq!(info.record("LCK").unwrap().payload.len(), 47);
        assert_eq!(info.raw, blob);
    }
}
