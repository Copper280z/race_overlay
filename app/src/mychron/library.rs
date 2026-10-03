//! The download library: one folder per logger holding its recordings and a
//! ledger of what was downloaded, so Track mode never fetches a recording
//! twice. The ledger lives beside the files, so it survives reinstalling the
//! app and moves with the folder.

use super::policy;
use overlay_logger::Datalog;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const LEDGER_FILE: &str = "mychron-ledger.json";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub downloads: Vec<LedgerEntry>,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// The logger's file name, e.g. `a_0089.xrz`.
    pub name: String,
    pub size: u64,
    /// Recording start as listed by the logger (`YYYY-MM-DDTHH:MM:SS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded: Option<String>,
    /// Relative to the logger's folder.
    pub path: PathBuf,
    /// Local time of the download (RFC 3339).
    pub downloaded_at: String,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}

fn recorded_key(log: &Datalog) -> Option<String> {
    log.recorded
        .map(|t| t.format("%Y-%m-%dT%H:%M:%S").to_string())
}

impl LedgerEntry {
    /// The same recording: loggers reuse file names after their memory is
    /// cleared, so size and start time must match too.
    pub fn matches(&self, log: &Datalog) -> bool {
        self.name == log.name && self.size == log.size && self.recorded == recorded_key(log)
    }
}

/// One logger's folder in the library.
#[derive(Clone, Debug)]
pub struct LoggerFolder {
    pub root: PathBuf,
    pub ledger: Ledger,
}

impl LoggerFolder {
    /// Open (or start) the folder. A damaged ledger is an error rather than
    /// an empty history, which would re-download everything.
    pub fn open(root: PathBuf, fingerprint: &str) -> io::Result<Self> {
        let ledger_path = root.join(LEDGER_FILE);
        let ledger = match fs::read(&ledger_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {error}", ledger_path.display()),
                )
            })?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ledger {
                fingerprint: fingerprint.to_owned(),
                ..Ledger::default()
            },
            Err(error) => return Err(error),
        };
        Ok(Self { root, ledger })
    }

    /// Where a recording was downloaded to, if it was.
    pub fn downloaded(&self, log: &Datalog) -> Option<PathBuf> {
        self.ledger
            .downloads
            .iter()
            .rev()
            .find(|entry| entry.matches(log))
            .map(|entry| self.root.join(&entry.path))
    }

    /// Write a downloaded recording and record it in the ledger. The file
    /// appears under its final name only once complete and verified.
    pub fn store(&mut self, log: &Datalog, bytes: &[u8]) -> io::Result<PathBuf> {
        verify_recording(bytes)?;
        let relative = policy::library_path(log);
        let path = self.root.join(&relative);
        write_atomically(&path, bytes)?;
        self.ledger.downloads.retain(|entry| !entry.matches(log));
        self.ledger.downloads.push(LedgerEntry {
            name: log.name.clone(),
            size: log.size,
            recorded: recorded_key(log),
            path: relative,
            downloaded_at: chrono::Local::now().to_rfc3339(),
            unknown: BTreeMap::new(),
        });
        let ledger = serde_json::to_vec_pretty(&self.ledger).map_err(io::Error::other)?;
        write_atomically(&self.root.join(LEDGER_FILE), &ledger)?;
        Ok(path)
    }
}

/// Logger recordings are a zlib stream around an XRK; one that does not
/// inflate to XRK records was damaged in transfer.
pub fn verify_recording(bytes: &[u8]) -> io::Result<()> {
    let invalid = |message: &str| io::Error::new(io::ErrorKind::InvalidData, message.to_owned());
    let mut head = [0u8; 2];
    let mut decoder = flate2::read::ZlibDecoder::new(bytes);
    decoder
        .read_exact(&mut head)
        .map_err(|_| invalid("download is not a compressed recording"))?;
    if &head != b"<h" {
        return Err(invalid("download does not contain an XRK recording"));
    }
    io::copy(&mut decoder, &mut io::sink())
        .map_err(|_| invalid("download is damaged: the compressed data is incomplete"))?;
    Ok(())
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut partial = path.as_os_str().to_owned();
    partial.push(".part");
    let partial = PathBuf::from(partial);
    let mut file = fs::File::create(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&partial, path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal deflated XRK.
    pub fn compressed_recording(seed: u8) -> Vec<u8> {
        let mut xrk = b"<hCHS\0".to_vec();
        xrk.extend((0..2_000).map(|i| (i as u8).wrapping_mul(seed)));
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&xrk).unwrap();
        encoder.finish().unwrap()
    }

    fn listed(name: &str, bytes: &[u8], recorded: &str) -> Datalog {
        let mut device = overlay_logger::fake::FakeDevice::default();
        device.add_recording(name, recorded, "KELLYS", bytes.to_vec());
        device.recordings.remove(0).log
    }

    #[test]
    fn stores_atomically_and_remembers_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("MyChron 01234567");
        let bytes = compressed_recording(3);
        let log = listed("a_0089.xrz", &bytes, "2026-08-30 15:33:28");
        let mut folder = LoggerFolder::open(root.clone(), "0123456789abcdef").unwrap();
        assert_eq!(folder.downloaded(&log), None);
        let path = folder.store(&log, &bytes).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(folder.downloaded(&log), Some(path.clone()));
        assert!(!path.with_extension("xrz.part").exists());

        // Same name after the logger's memory was cleared: a new recording.
        let reused = listed(
            "a_0089.xrz",
            &compressed_recording(5),
            "2026-09-02 10:00:00",
        );
        let reopened = LoggerFolder::open(root.clone(), "ignored").unwrap();
        assert_eq!(reopened.ledger.fingerprint, "0123456789abcdef");
        assert_eq!(reopened.downloaded(&log), Some(path));
        assert_eq!(reopened.downloaded(&reused), None);

        // Fields from newer versions survive a rewrite.
        let ledger_path = root.join(LEDGER_FILE);
        let mut json: Value = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        json["future"] = Value::from(7);
        json["downloads"][0]["note"] = Value::from("kept");
        fs::write(&ledger_path, serde_json::to_vec(&json).unwrap()).unwrap();
        let mut reopened = LoggerFolder::open(root.clone(), "").unwrap();
        reopened.store(&reused, &compressed_recording(5)).unwrap();
        let json: Value = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        assert_eq!(json["future"], 7);
        assert_eq!(json["downloads"][0]["note"], "kept");
        assert_eq!(json["downloads"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn rejects_damaged_downloads_and_ledgers() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = compressed_recording(3);
        let log = listed("a_0001.xrz", &bytes, "2026-08-30 15:33:28");
        let mut folder = LoggerFolder::open(dir.path().to_owned(), "f").unwrap();
        assert!(folder.store(&log, &bytes[..bytes.len() / 2]).is_err());
        assert!(folder.store(&log, b"plain bytes").is_err());
        assert_eq!(folder.downloaded(&log), None);
        assert!(
            fs::read_dir(dir.path()).unwrap().next().is_none(),
            "nothing written"
        );

        fs::write(dir.path().join(LEDGER_FILE), b"{not json").unwrap();
        assert!(LoggerFolder::open(dir.path().to_owned(), "f").is_err());
    }
}
