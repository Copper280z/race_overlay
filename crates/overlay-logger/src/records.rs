//! The record framing nested inside logger messages, the same as in `.xrk`
//! files:
//!
//! ```text
//! '<' 'h' TAG:4 len:u32le marker '>' payload '<' TAG:4 sum16:u16le '>'
//! ```
//!
//! Channel blocks use tags like `M\0\0\0`; the device info blob uses `iMST`,
//! `iHW `, `iUSR`, … with marker `a`. Checksums are reported rather than
//! enforced, so a record is never dropped for a disagreement alone.

use crate::stcp::sum16;

const HEADER_LEN: usize = 12;
const TRAILER_LEN: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Tag text as sent, including padding (`"iHW "`, `"M\0\0\0"`).
    pub tag: String,
    pub payload: Vec<u8>,
    /// The trailer's sum16 matches the payload.
    pub checksum_ok: bool,
}

impl Record {
    /// Tag with trailing spaces and NULs removed.
    pub fn name(&self) -> &str {
        self.tag.trim_end_matches([' ', '\0'])
    }
}

/// Every well-framed record in `data`, skipping bytes between them.
pub(crate) fn parse(data: &[u8]) -> Vec<Record> {
    let mut records = Vec::new();
    let mut pos = 0;
    while pos + HEADER_LEN + TRAILER_LEN <= data.len() {
        let header = &data[pos..pos + HEADER_LEN];
        if header[0] != b'<' || header[1] != b'h' || header[11] != b'>' {
            pos += 1;
            continue;
        }
        let tag = &header[2..6];
        let length = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
        let body_start = pos + HEADER_LEN;
        let Some(end) = body_start
            .checked_add(length)
            .and_then(|footer| footer.checked_add(TRAILER_LEN))
            .filter(|&end| end <= data.len())
        else {
            pos += 1;
            continue;
        };
        let trailer = &data[end - TRAILER_LEN..end];
        if trailer[0] != b'<' || &trailer[1..5] != tag || trailer[7] != b'>' {
            pos += 1;
            continue;
        }
        let payload = &data[body_start..body_start + length];
        let sum = u16::from_le_bytes([trailer[5], trailer[6]]);
        records.push(Record {
            tag: String::from_utf8_lossy(tag).into_owned(),
            payload: payload.to_vec(),
            checksum_ok: sum == sum16(payload),
        });
        pos = end;
    }
    records
}

/// One framed record, for tests and the fake logger.
#[cfg(any(test, feature = "fake"))]
pub(crate) fn encode(tag: &[u8; 4], marker: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = b"<h".to_vec();
    out.extend_from_slice(tag);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&[marker, b'>']);
    out.extend_from_slice(payload);
    out.push(b'<');
    out.extend_from_slice(tag);
    out.extend_from_slice(&sum16(payload).to_le_bytes());
    out.push(b'>');
    out
}

/// Text before the first NUL.
pub(crate) fn c_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_records_between_noise_and_flags_bad_checksums() {
        let mut data = b"xx".to_vec();
        data.extend(encode(b"iHW ", b'a', b"WiFi=ESP32|"));
        data.extend_from_slice(b"\r\n");
        data.extend(encode(b"M\0\0\0", 0, &[1, 2, 3]));
        data.extend(encode(b"M\0\0\0", 0, &[4]));
        let last = data.len() - 2;
        data[last] ^= 0xff;
        let records = parse(&data);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0].name(), "iHW");
        assert_eq!(records[0].payload, b"WiFi=ESP32|");
        assert_eq!(records[1].name(), "M");
        assert!(records[0].checksum_ok && records[1].checksum_ok && !records[2].checksum_ok);
    }
}
