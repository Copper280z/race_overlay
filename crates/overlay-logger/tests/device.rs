//! Against a real MyChron6. Ignored by default: run on a machine associated
//! with the logger's hotspot (or with `RACE_OVERLAY_MYCHRON_ADDR` pointing at
//! a relay) with `cargo test -p overlay-logger --test device -- --ignored`.
//!
//! None of these write the logger's clock.

use overlay_logger::*;
use std::{sync::atomic::AtomicBool, time::Duration};

fn addr() -> DeviceAddr {
    DeviceAddr::from_env().expect("valid RACE_OVERLAY_MYCHRON_ADDR")
}

fn session() -> Session {
    Session::open(&addr(), Timeouts::default()).expect("logger reachable")
}

#[test]
#[ignore = "needs a MyChron in range"]
fn descriptor_is_stable() {
    let first = probe(&addr(), Duration::from_secs(2))
        .unwrap()
        .expect("descriptor");
    let second = probe(&addr(), Duration::from_secs(2))
        .unwrap()
        .expect("descriptor");
    assert_eq!(first.fingerprint(), second.fingerprint());
    assert!(first.identity().starts_with(b"idn"));
}

#[test]
#[ignore = "needs a MyChron in range"]
fn lists_after_hello_only() {
    let logs = session().datalogs().unwrap();
    assert!(!logs.is_empty(), "logger holds no recordings");
    assert!(
        logs.iter()
            .all(|log| log.size > 0 && log.recorded.is_some())
    );
}

#[test]
#[ignore = "needs a MyChron in range"]
fn channel_blocks_match_the_capture() {
    let mut s = session();
    let (catalog, channels) = s.channel_catalog().unwrap();
    assert_eq!(catalog.hash.to_le_bytes(), [0x04, 0x32, 0xe9, 0x67]);
    assert_eq!(channels.len(), 97);
    assert_eq!(channels[0].short_name, "MClk");
    let tree = s.channel_tree().unwrap();
    assert_eq!(tree.hash.to_le_bytes(), [0x9c, 0x0a, 0x45, 0x18]);
    let (schema, entries) = s.live_schema().unwrap();
    assert_eq!(schema.hash.to_le_bytes(), [0x4c, 0x1f, 0x51, 0xf9]);
    assert_eq!(entries.len(), 97);
}

#[test]
#[ignore = "needs a MyChron in range"]
fn streams_live_frames_and_stops() {
    let mut s = session();
    s.user_profiles().unwrap();
    s.live_schema().unwrap();
    s.start_live().unwrap();
    let mut ticks = Vec::new();
    for _ in 0..10 {
        if let MainObject::Frame(frame) = s.main_object().unwrap() {
            ticks.extend(frame.tick_ms);
        }
        s.live_heartbeat().unwrap();
        std::thread::sleep(Duration::from_millis(110));
    }
    s.stop_live().unwrap();
    assert!(ticks.len() >= 5, "few live frames: {ticks:?}");
    assert!(ticks.windows(2).all(|w| w[1] >= w[0]));
}

#[test]
#[ignore = "needs a MyChron in range"]
fn downloads_the_smallest_recording_completely() {
    let mut s = session();
    let log = s
        .datalogs()
        .unwrap()
        .into_iter()
        .min_by_key(|log| log.size)
        .expect("a recording");
    let mut bytes = Vec::new();
    s.download_datalog(&log, &mut bytes, &mut |_, _| {}, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(bytes.len() as u64, log.size);
    assert_eq!(bytes[0], 0x78, "recordings are zlib streams");
    let mut inflated = Vec::new();
    std::io::Read::read_to_end(
        &mut flate2::read::ZlibDecoder::new(&bytes[..]),
        &mut inflated,
    )
    .unwrap();
    assert!(inflated.starts_with(b"<h"), "inflated recording is XRK");
}
