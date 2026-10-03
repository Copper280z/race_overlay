//! "STCP" framing on TCP port 2000 (`docs/mychron-protocol.md`, "TCP 2000
//! message framing"):
//!
//! ```text
//! '<' 'h' 'S' 'T' <tag:2> <payload_len:u32le> 0x00 '>'  <payload>  '<' 'S' 'T' <tag:2> <sum16:u16le> '>'
//! ```

use crate::error::{LoggerError, Result};

const HEADER_LEN: usize = 12;
const TRAILER_LEN: usize = 8;
const START: &[u8; 4] = b"<hST";

/// Largest payload accepted from the wire. Real frames are at most one
/// 65 476-byte download chunk; a bigger length means the stream is not
/// framed where we think it is.
pub const MAX_PAYLOAD: usize = 1 << 22;

/// Download chunk size the logger advertises (offset 20 of every echo) and
/// uses as the stride between file read offsets.
pub const CHUNK_SIZE: u32 = 0xFFC0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tag {
    /// Everything the logger sends, plus the client's hello, clock writes,
    /// acknowledgements, and download reads.
    Cp,
    /// Client object operations.
    Nc,
}

impl Tag {
    fn bytes(self) -> &'static [u8; 2] {
        match self {
            Self::Cp => b"CP",
            Self::Nc => b"NC",
        }
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"CP" => Some(Self::Cp),
            b"NC" => Some(Self::Nc),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub tag: Tag,
    pub payload: Vec<u8>,
}

/// Sum of all bytes modulo 0x10000.
pub fn sum16(bytes: &[u8]) -> u16 {
    bytes
        .iter()
        .fold(0u16, |sum, &byte| sum.wrapping_add(u16::from(byte)))
}

pub fn encode(tag: Tag, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + TRAILER_LEN);
    out.extend_from_slice(START);
    out.extend_from_slice(tag.bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0, b'>']);
    out.extend_from_slice(payload);
    out.extend_from_slice(b"<ST");
    out.extend_from_slice(tag.bytes());
    out.extend_from_slice(&sum16(payload).to_le_bytes());
    out.push(b'>');
    out
}

/// Incremental frame decoder over a TCP byte stream.
///
/// Bytes before a frame start are skipped and counted, so a stream that joins
/// mid-frame resynchronizes on the next header.
#[derive(Default)]
pub struct Decoder {
    buffer: Vec<u8>,
    skipped: usize,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes discarded while looking for a frame header.
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// The next complete frame, `None` when more bytes are needed.
    ///
    /// A frame whose trailer checksum disagrees with its payload is consumed
    /// and reported as [`LoggerError::Checksum`].
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        loop {
            let Some(start) = find(&self.buffer, START) else {
                let keep = self.buffer.len().min(START.len() - 1);
                self.discard(self.buffer.len() - keep);
                return Ok(None);
            };
            self.discard(start);
            if self.buffer.len() < HEADER_LEN {
                return Ok(None);
            }
            let header = &self.buffer[..HEADER_LEN];
            let length = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
            let Some(tag) = Tag::parse(&header[4..6])
                .filter(|_| header[10] == 0 && header[11] == b'>' && length <= MAX_PAYLOAD)
            else {
                self.discard(1);
                continue;
            };
            let end = HEADER_LEN + length + TRAILER_LEN;
            if self.buffer.len() < end {
                return Ok(None);
            }
            let trailer = &self.buffer[HEADER_LEN + length..end];
            if &trailer[..3] != b"<ST" || &trailer[3..5] != tag.bytes() || trailer[7] != b'>' {
                self.discard(1);
                return Err(LoggerError::Protocol("malformed frame trailer".into()));
            }
            let expected = u16::from_le_bytes([trailer[5], trailer[6]]);
            let payload = self.buffer[HEADER_LEN..HEADER_LEN + length].to_vec();
            self.buffer.drain(..end);
            let actual = sum16(&payload);
            if actual != expected {
                return Err(LoggerError::Checksum { expected, actual });
            }
            return Ok(Some(Frame { tag, payload }));
        }
    }

    fn discard(&mut self, count: usize) {
        self.skipped += count;
        self.buffer.drain(..count);
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Status word at offset 24 of an operation echo/header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// `1`: what the client puts in a request.
    Request,
    /// `0x0A09`: plain acknowledgement.
    Ack,
    /// `0x0A01`: acknowledgement seen on time-sync echoes before the clock
    /// was written (meaning not confirmed).
    AckUnsynced,
    /// `0x0A11`: a data message follows once the header is acknowledged.
    DataReady,
    /// `0x0A1D`: nothing to send (empty result or absent file).
    NoData,
    Other(u32),
}

impl Status {
    pub fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Request,
            0x0A09 => Self::Ack,
            0x0A01 => Self::AckUnsynced,
            0x0A11 => Self::DataReady,
            0x0A1D => Self::NoData,
            other => Self::Other(other),
        }
    }

    pub fn code(self) -> u32 {
        match self {
            Self::Request => 1,
            Self::Ack => 0x0A09,
            Self::AckUnsynced => 0x0A01,
            Self::DataReady => 0x0A11,
            Self::NoData => 0x0A1D,
            Self::Other(code) => code,
        }
    }
}

/// Bit set in a request's flags word for the time-sync (`kind` 1) rounds.
pub const FLAG_TIME_SYNC: u32 = 0x4000_0000;

/// Longest device path the 32-byte field holds.
pub const MAX_PATH: usize = 32;

/// A 64-byte `NC` operation request.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub id: u16,
    pub kind: u16,
    pub flags: u32,
    /// Device path for file operations, e.g. `1:/mem/a_0089.xrz`.
    pub path: Option<String>,
    /// Write `0xffffffff` at offset 32 (stream stop).
    pub stop: bool,
}

impl Request {
    pub fn new(id: u16, kind: u16) -> Self {
        Self {
            id,
            kind,
            ..Self::default()
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn payload(&self) -> Result<[u8; 64]> {
        let mut payload = [0u8; 64];
        payload[8..10].copy_from_slice(&self.id.to_le_bytes());
        payload[10..12].copy_from_slice(&self.kind.to_le_bytes());
        payload[12..16].copy_from_slice(&self.flags.to_le_bytes());
        payload[24..28].copy_from_slice(&Status::Request.code().to_le_bytes());
        if self.stop {
            payload[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        if let Some(path) = &self.path {
            let bytes = path.as_bytes();
            if bytes.len() > MAX_PATH || !path.is_ascii() || bytes.contains(&0) {
                return Err(LoggerError::Protocol(format!(
                    "device path {path:?} must be at most {MAX_PATH} ASCII bytes"
                )));
            }
            payload[32..32 + bytes.len()].copy_from_slice(bytes);
        }
        Ok(payload)
    }
}

/// A 64-byte operation echo or response header from the logger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpHeader {
    pub id: u16,
    pub kind: u16,
    pub flags: u32,
    /// Length of the data message body that follows; the file size for a
    /// file open.
    pub length: u32,
    /// Maximum chunk size the logger echoes (65 472).
    pub chunk: u32,
    pub status: Status,
    pub raw: Vec<u8>,
}

impl OpHeader {
    pub fn parse(payload: &[u8]) -> Result<Self> {
        if payload.len() != 64 {
            return Err(LoggerError::Protocol(format!(
                "expected a 64-byte operation header, got {} bytes",
                payload.len()
            )));
        }
        let u32_at =
            |offset: usize| u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap());
        Ok(Self {
            id: u16::from_le_bytes([payload[8], payload[9]]),
            kind: u16::from_le_bytes([payload[10], payload[11]]),
            flags: u32_at(12),
            length: u32_at(16),
            chunk: u32_at(20),
            status: Status::from_code(u32_at(24)),
            raw: payload.to_vec(),
        })
    }

    /// The NUL-padded path field at offset 32, when it holds text.
    pub fn path(&self) -> Option<String> {
        let field = &self.raw[32..64];
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        let text = &field[..end];
        (!text.is_empty() && text.iter().all(|b| b.is_ascii_graphic()))
            .then(|| String::from_utf8_lossy(text).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_matches_the_captured_bytes() {
        let frame = encode(Tag::Cp, &[0, 0, 0, 0, 0x06, 0x08, 0, 0]);
        let mut expected = b"<hSTCP".to_vec();
        expected.extend_from_slice(&[8, 0, 0, 0, 0, b'>', 0, 0, 0, 0, 6, 8, 0, 0]);
        expected.extend_from_slice(b"<STCP");
        expected.extend_from_slice(&[0x0e, 0x00, b'>']);
        assert_eq!(frame, expected);
    }

    #[test]
    fn decoder_resyncs_across_noise_and_partial_reads() {
        let mut stream = b"noise<h".to_vec();
        stream.extend(encode(Tag::Cp, b"first"));
        stream.extend(encode(Tag::Nc, &[0xff; 70]));
        let mut decoder = Decoder::default();
        let mut frames = Vec::new();
        for byte in stream {
            decoder.push(&[byte]);
            while let Some(frame) = decoder.next_frame().unwrap() {
                frames.push(frame);
            }
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].payload, b"first");
        assert_eq!(frames[1].tag, Tag::Nc);
        assert_eq!(decoder.skipped(), 7);
    }

    #[test]
    fn decoder_reports_bad_checksums_and_continues() {
        let mut bad = encode(Tag::Cp, b"payload");
        let sum_at = bad.len() - 3;
        bad[sum_at] ^= 1;
        bad.extend(encode(Tag::Cp, b"next"));
        let mut decoder = Decoder::default();
        decoder.push(&bad);
        assert!(matches!(
            decoder.next_frame(),
            Err(LoggerError::Checksum { .. })
        ));
        assert_eq!(decoder.next_frame().unwrap().unwrap().payload, b"next");
    }

    #[test]
    fn request_layout_and_header_parse() {
        let payload = Request::new(0x02, 4)
            .with_path("1:/mem/a_0089.xrz")
            .payload()
            .unwrap();
        assert_eq!(&payload[8..12], &[2, 0, 4, 0]);
        assert_eq!(&payload[24..28], &[1, 0, 0, 0]);
        assert_eq!(&payload[32..49], b"1:/mem/a_0089.xrz");
        let mut echo = payload;
        echo[20..24].copy_from_slice(&CHUNK_SIZE.to_le_bytes());
        echo[24..28].copy_from_slice(&0x0A09u32.to_le_bytes());
        let header = OpHeader::parse(&echo).unwrap();
        assert_eq!((header.id, header.kind), (2, 4));
        assert_eq!(header.status, Status::Ack);
        assert_eq!(header.chunk, 65_472);
        assert_eq!(header.path().as_deref(), Some("1:/mem/a_0089.xrz"));

        let stop = Request {
            stop: true,
            ..Request::new(0x51, 2)
        };
        assert_eq!(&stop.payload().unwrap()[32..36], &[0xff; 4]);
        assert!(
            Request::new(2, 4)
                .with_path("x".repeat(33))
                .payload()
                .is_err()
        );
    }
}
