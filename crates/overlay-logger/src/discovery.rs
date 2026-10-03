//! UDP port 36002: the `aim-ka` keepalive, answered by a 236-byte device
//! descriptor. A reply is the cheapest way to tell that a logger is in range.

use crate::addr::DeviceAddr;
use crate::error::{LoggerError, Result};
use std::{
    net::UdpSocket,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub const KEEPALIVE: &[u8; 6] = b"aim-ka";
pub const DESCRIPTOR_LEN: usize = 236;
/// Interval RaceCapture uses between keepalives.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_millis(1100);

const IDN_OFFSET: usize = 84;

/// The logger's reply to a keepalive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub raw: Vec<u8>,
}

impl Descriptor {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        // The leading word is the whole datagram's length (0xec = 236).
        let declared = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        (bytes.len() == DESCRIPTOR_LEN && declared == DESCRIPTOR_LEN).then(|| Self {
            raw: bytes.to_vec(),
        })
    }

    /// The `idn` record (offset 84): identical across power cycles, so it
    /// identifies a logger without the SSID.
    pub fn identity(&self) -> &[u8] {
        let rest = &self.raw[IDN_OFFSET..];
        if rest.starts_with(b"idn") && rest.len() >= 6 {
            let length = usize::from(u16::from_le_bytes([rest[4], rest[5]]));
            &rest[..(6 + length).min(rest.len())]
        } else {
            rest
        }
    }

    /// Short, stable hex fingerprint of [`Self::identity`].
    pub fn fingerprint(&self) -> String {
        // FNV-1a: stable across builds and platforms, unlike `DefaultHasher`.
        let hash = self
            .identity()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325u64, |hash, &byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
        format!("{hash:016x}")
    }

    /// Little-endian word at `offset`, for the undecoded counter fields.
    pub fn word(&self, offset: usize) -> Option<u32> {
        Some(u32::from_le_bytes(
            self.raw.get(offset..offset + 4)?.try_into().ok()?,
        ))
    }

    /// The two bytes (0x0E, 0x11) observed to change and persist across power
    /// cycles; meaning unknown.
    pub fn persisted_state(&self) -> (u8, u8) {
        (self.raw[0x0E], self.raw[0x11])
    }
}

/// Send one keepalive and wait up to `timeout` for the descriptor.
/// `Ok(None)` means no logger answered.
pub fn probe(addr: &DeviceAddr, timeout: Duration) -> Result<Option<Descriptor>> {
    let target = addr.udp()?;
    let bind = if target.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = UdpSocket::bind(bind)?;
    socket.connect(target)?;
    let deadline = Instant::now() + timeout;
    if let Err(error) = socket.send(KEEPALIVE) {
        // No route to the logger's network is the ordinary "not in range".
        let error = LoggerError::from(error);
        return if error.is_unreachable() {
            Ok(None)
        } else {
            Err(error)
        };
    }
    let mut buffer = [0u8; 2048];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        socket.set_read_timeout(Some(left))?;
        match socket.recv(&mut buffer) {
            Ok(n) => {
                if let Some(descriptor) = Descriptor::parse(&buffer[..n]) {
                    return Ok(Some(descriptor));
                }
            }
            Err(error) => {
                let error = LoggerError::from(error);
                return if error.is_unreachable() {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
        }
    }
}

/// Sends `aim-ka` every [`KEEPALIVE_INTERVAL`] until dropped, the way
/// RaceCapture does for the whole of a connection. Replies are ignored.
pub struct Keepalive {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Keepalive {
    pub fn start(addr: &DeviceAddr) -> Result<Self> {
        let target = addr.udp()?;
        let socket = UdpSocket::bind(if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        socket.connect(target)?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("mychron-keepalive".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    let _ = socket.send(KEEPALIVE);
                    let next = Instant::now() + KEEPALIVE_INTERVAL;
                    while !flag.load(Ordering::Relaxed) && Instant::now() < next {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Keepalive {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A descriptor with the given identity bytes, for tests and the fake logger.
#[cfg(any(test, feature = "fake"))]
pub(crate) fn encode_descriptor(identity: &[u8]) -> Vec<u8> {
    let mut raw = vec![0u8; DESCRIPTOR_LEN];
    raw[..4].copy_from_slice(&(DESCRIPTOR_LEN as u32).to_le_bytes());
    raw[4..8].copy_from_slice(&2u32.to_le_bytes());
    raw[0x0E] = 7;
    raw[0x11] = 2;
    let mut idn = b"idn\x01".to_vec();
    idn.extend_from_slice(&(identity.len() as u16).to_le_bytes());
    idn.extend_from_slice(identity);
    raw[IDN_OFFSET..IDN_OFFSET + idn.len()].copy_from_slice(&idn);
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_captured_descriptor() {
        // Leading bytes and `idn` start of a real MyChron6 reply.
        let mut raw = vec![0u8; DESCRIPTOR_LEN];
        let head = [0xec, 0, 0, 0, 2, 0, 0, 0, 0x0b, 0, 0, 1, 0, 0, 6, 0];
        raw[..16].copy_from_slice(&head);
        let idn = [
            0x69, 0x64, 0x6e, 0x01, 0x38, 0x00, 0xa8, 0x01, 0xb5, 0x01, 0, 0, 0x61, 0x3c, 0x16,
            0x02,
        ];
        raw[84..100].copy_from_slice(&idn);
        let descriptor = Descriptor::parse(&raw).expect("real descriptor");
        assert_eq!(descriptor.identity().len(), 6 + 0x38);
        assert!(Descriptor::parse(&raw[..235]).is_none());
    }

    #[test]
    fn descriptor_identity_ignores_drifting_bytes() {
        let a = Descriptor::parse(&encode_descriptor(b"\xa8\x01\xb5\x01")).unwrap();
        let mut drifted = a.raw.clone();
        drifted[0x0E] = 9;
        let b = Descriptor::parse(&drifted).unwrap();
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.identity(), b"idn\x01\x04\x00\xa8\x01\xb5\x01");
        assert_eq!(b.persisted_state(), (9, 2));
        let other = Descriptor::parse(&encode_descriptor(b"\xa8\x01\xb5\x02")).unwrap();
        assert_ne!(a.fingerprint(), other.fingerprint());
        assert!(Descriptor::parse(&a.raw[..200]).is_none());
    }

    #[test]
    fn probe_reports_absence_as_none() {
        // A bound socket that never answers.
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = DeviceAddr {
            host: "127.0.0.1".into(),
            tcp_port: 1,
            udp_port: silent.local_addr().unwrap().port(),
        };
        assert_eq!(probe(&addr, Duration::from_millis(100)).unwrap(), None);
    }
}
