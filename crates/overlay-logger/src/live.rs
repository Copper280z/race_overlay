//! Live objects: the "main" object (NC `0x03/2`), heartbeat (NC `0x53/2`)
//! and value snapshot (NC `0x04/2`).
//!
//! The slot-to-channel mapping of live frames is not established, so frames
//! are exposed with only their decoded header fields and raw bytes.

use crate::channels::SCHEMA_MAGIC;
use crate::records::{self, Record};

pub const FRAME_MAGIC: [u8; 4] = [0x6b, 0x6b, 0x6b, 0x01];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveFrame {
    /// The first frame of a stream repeats the live schema's hash, binding
    /// its layout to the block from NC `0x09/2`.
    pub schema_hash: Option<u32>,
    /// Device uptime in milliseconds (`frame[8:12]`); absent in the 8-byte
    /// heartbeat frame.
    pub tick_ms: Option<u32>,
    pub raw: Vec<u8>,
}

impl LiveFrame {
    pub fn parse(body: &[u8]) -> Option<Self> {
        if body.len() < 8 {
            return None;
        }
        let word = |offset: usize| {
            body.get(offset..offset + 4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        };
        let schema_hash = match body[..4].try_into().unwrap() {
            FRAME_MAGIC => None,
            SCHEMA_MAGIC => word(4),
            _ => return None,
        };
        Some(Self {
            schema_hash,
            tick_ms: word(8),
            raw: body.to_vec(),
        })
    }
}

/// What NC `0x03/2` returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MainObject {
    /// First session after power-on: the current profile name (`System`),
    /// with the raw bytes (a trailing byte varies between 0x15 and 0x01).
    Profile {
        name: String,
        raw: Vec<u8>,
    },
    Frame(LiveFrame),
    Empty,
    Unrecognized(Vec<u8>),
}

impl MainObject {
    pub fn parse(body: &[u8]) -> Self {
        if body.is_empty() {
            return Self::Empty;
        }
        if let Some(frame) = LiveFrame::parse(body) {
            return Self::Frame(frame);
        }
        let name_end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
        let name = &body[..name_end];
        if body.len() < 64 && !name.is_empty() && name.iter().all(u8::is_ascii_graphic) {
            return Self::Profile {
                name: String::from_utf8_lossy(name).into_owned(),
                raw: body.to_vec(),
            };
        }
        Self::Unrecognized(body.to_vec())
    }
}

/// NC `0x04/2`: a compact `<hiMST`-framed snapshot that changes in real time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub records: Vec<Record>,
    pub raw: Vec<u8>,
}

impl Snapshot {
    pub fn parse(body: &[u8]) -> Self {
        Self {
            records: records::parse(body),
            raw: body.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_main_object_replies() {
        assert_eq!(
            MainObject::parse(b"System\0\0\x15"),
            MainObject::Profile {
                name: "System".into(),
                raw: b"System\0\0\x15".to_vec()
            }
        );
        let mut frame = FRAME_MAGIC.to_vec();
        frame.extend_from_slice(&[0; 4]);
        frame.extend_from_slice(&1234u32.to_le_bytes());
        frame.resize(496, 0);
        let MainObject::Frame(parsed) = MainObject::parse(&frame) else {
            panic!("expected a frame");
        };
        assert_eq!((parsed.schema_hash, parsed.tick_ms), (None, Some(1234)));

        let mut first = SCHEMA_MAGIC.to_vec();
        first.extend_from_slice(&0xf951_1f4cu32.to_le_bytes());
        first.extend_from_slice(&99u32.to_le_bytes());
        let MainObject::Frame(parsed) = MainObject::parse(&first) else {
            panic!("expected a frame");
        };
        assert_eq!(parsed.schema_hash, Some(0xf951_1f4c));
        assert_eq!(parsed.tick_ms, Some(99));

        let heartbeat = LiveFrame::parse(&[0x6b, 0x6b, 0x6b, 1, 0, 0, 0, 0]).unwrap();
        assert_eq!(heartbeat.tick_ms, None);
        assert_eq!(MainObject::parse(&[]), MainObject::Empty);
    }
}
