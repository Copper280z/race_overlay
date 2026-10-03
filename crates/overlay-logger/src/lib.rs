//! AiM MyChron6 Wi-Fi link: the logger's command protocol, discovery, and
//! the Wi-Fi control needed to reach its hotspot.
//!
//! `docs/mychron-protocol.md` describes the protocol this crate implements.
//! Decoded structures are typed; parts the protocol notes leave undecoded
//! are kept as validated raw records rather than guessed at.

mod addr;
mod channels;
mod client;
mod datalogs;
mod discovery;
mod error;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
mod info;
mod live;
mod records;
pub mod stcp;
pub mod wifi;

#[cfg(test)]
mod tests;

pub use addr::{ADDRESS_ENV, DeviceAddr};
pub use channels::{
    CATALOG_MAGIC, CatalogChannel, ChannelBlock, SCHEMA_MAGIC, SchemaEntry, TREE_MAGIC,
};
pub use client::{
    CivilTime, ClockWrite, OpResult, Progress, Session, Timeouts, UserProfile, op, probe_object,
};
pub use datalogs::Datalog;
pub use discovery::{DESCRIPTOR_LEN, Descriptor, KEEPALIVE_INTERVAL, Keepalive, probe};
pub use error::{LoggerError, Result};
pub use info::{DeviceInfo, DevicePath, HardwareInfo};
pub use live::{FRAME_MAGIC, LiveFrame, MainObject, Snapshot};
pub use records::Record;
