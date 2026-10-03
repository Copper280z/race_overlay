//! Channel catalog (NC `0x02/2`), channel tree (NC `0x08/2`) and live-frame
//! schema (NC `0x09/2`). Each is a 4-byte magic, a 4-byte hash, then
//! XRK-framed `<hM…>` records, one per channel.

use crate::error::{LoggerError, Result};
use crate::records::{self, Record, c_string};

pub const CATALOG_MAGIC: [u8; 4] = [0x68, 0x68, 0x68, 0x01];
pub const TREE_MAGIC: [u8; 4] = [0x68, 0x68, 0x69, 0x01];
pub const SCHEMA_MAGIC: [u8; 4] = [0x68, 0x68, 0x66, 0x01];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelBlock {
    pub magic: [u8; 4],
    /// Identifies the layout; the first live frame of a stream repeats the
    /// schema's hash.
    pub hash: u32,
    pub records: Vec<Record>,
}

impl ChannelBlock {
    pub fn parse(body: &[u8], expected_magic: [u8; 4]) -> Result<Self> {
        if body.len() < 8 || body[..4] != expected_magic {
            return Err(LoggerError::Protocol(format!(
                "channel block does not start with {}",
                hex(&expected_magic)
            )));
        }
        Ok(Self {
            magic: expected_magic,
            hash: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            records: records::parse(&body[8..]),
        })
    }
}

/// One entry of the channel catalog (112-byte records). Only the fields the
/// protocol notes confirm are decoded; the record keeps every byte.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogChannel {
    pub index: u16,
    /// e.g. `MClk`, `RPM`, `WSpd`.
    pub short_name: String,
    /// e.g. `Master Clk`, `WheelSpeed`.
    pub long_name: String,
    pub raw: Vec<u8>,
}

pub fn catalog_channels(block: &ChannelBlock) -> Vec<CatalogChannel> {
    block
        .records
        .iter()
        .filter(|record| record.payload.len() >= 56)
        .map(|record| {
            let p = &record.payload;
            CatalogChannel {
                index: u16::from_le_bytes([p[0], p[1]]),
                short_name: c_string(&p[24..32]),
                long_name: c_string(&p[32..56]),
                raw: p.clone(),
            }
        })
        .collect()
}

/// One live-frame schema record, indexed by catalog position.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaEntry {
    pub index: u32,
    /// Second word; its meaning is not established.
    pub param: u32,
    /// Per-channel scale (`float32` at `[20:24]` in the 36-byte form where
    /// `param` is `0x14`), e.g. 0.001 for channels that stream as milli-units.
    pub scale: Option<f32>,
    pub raw: Vec<u8>,
}

pub fn schema_entries(block: &ChannelBlock) -> Vec<SchemaEntry> {
    block
        .records
        .iter()
        .filter(|record| record.payload.len() >= 8)
        .map(|record| {
            let p = &record.payload;
            let word =
                |offset: usize| u32::from_le_bytes(p[offset..offset + 4].try_into().unwrap());
            let param = word(4);
            SchemaEntry {
                index: word(0),
                param,
                scale: (param == 0x14 && p.len() >= 24)
                    .then(|| f32::from_le_bytes(p[20..24].try_into().unwrap())),
                raw: p.clone(),
            }
        })
        .collect()
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Synthetic blocks for tests and the fake logger.
#[cfg(any(test, feature = "fake"))]
pub(crate) fn encode_block(magic: [u8; 4], hash: u32, payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = magic.to_vec();
    out.extend_from_slice(&hash.to_le_bytes());
    for payload in payloads {
        out.extend(crate::records::encode(b"M\0\0\0", 0, payload));
    }
    out
}

#[cfg(any(test, feature = "fake"))]
pub(crate) fn catalog_payload(index: u16, short: &str, long: &str) -> Vec<u8> {
    let mut p = vec![0u8; 112];
    p[..2].copy_from_slice(&index.to_le_bytes());
    p[24..24 + short.len()].copy_from_slice(short.as_bytes());
    p[32..32 + long.len()].copy_from_slice(long.as_bytes());
    p[56..60].copy_from_slice(b"@AIM");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_catalog_and_schema_records() {
        let catalog = encode_block(
            CATALOG_MAGIC,
            0x67e9_3204,
            &[
                catalog_payload(0, "MClk", "Master Clk"),
                catalog_payload(28, "RPM", "RPM"),
            ],
        );
        let block = ChannelBlock::parse(&catalog, CATALOG_MAGIC).unwrap();
        assert_eq!(block.hash, 0x67e9_3204);
        let channels = catalog_channels(&block);
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0].short_name, "MClk");
        assert_eq!(channels[0].long_name, "Master Clk");
        assert_eq!(channels[1].index, 28);
        assert!(ChannelBlock::parse(&catalog, SCHEMA_MAGIC).is_err());

        let mut scaled = vec![0u8; 36];
        scaled[..4].copy_from_slice(&5u32.to_le_bytes());
        scaled[4..8].copy_from_slice(&0x14u32.to_le_bytes());
        scaled[20..24].copy_from_slice(&0.001f32.to_le_bytes());
        let mut plain = vec![0u8; 16];
        plain[4..8].copy_from_slice(&3u32.to_le_bytes());
        let schema = encode_block(SCHEMA_MAGIC, 0xf951_1f4c, &[plain, scaled]);
        let entries = schema_entries(&ChannelBlock::parse(&schema, SCHEMA_MAGIC).unwrap());
        assert_eq!(entries[0].scale, None);
        assert_eq!(entries[0].param, 3);
        assert_eq!(entries[1].index, 5);
        assert_eq!(entries[1].scale, Some(0.001));
    }
}
