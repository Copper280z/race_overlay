//! A pretend MyChron on this computer, for trying the app without a logger:
//!
//! ```sh
//! cargo run -p overlay-logger --features fake --example fake_logger -- mychron_data/gvkc
//! RACE_OVERLAY_MYCHRON_ADDR=<printed address> cargo run -p race-overlay
//! ```
//!
//! Serves every `.xrk` (compressed on the fly, as the logger stores them) and
//! `.xrz` in the given folders. Press Enter to toggle whether it is "in range".

use overlay_logger::fake::{FakeDevice, FakeLogger};
use std::{io::Write, path::PathBuf};

fn main() {
    let folders = std::env::args()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if folders.is_empty() {
        eprintln!("usage: fake_logger <folder with .xrk/.xrz files>...");
        std::process::exit(2);
    }
    let mut device = FakeDevice::default();
    let mut files = folders
        .iter()
        .flat_map(|folder| std::fs::read_dir(folder).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("xrk") || e.eq_ignore_ascii_case("xrz"))
        })
        .collect::<Vec<_>>();
    files.sort();
    for (i, path) in files.iter().enumerate() {
        let bytes = std::fs::read(path).expect("read recording");
        let bytes = if bytes.first() == Some(&0x78) {
            bytes
        } else {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&bytes).unwrap();
            encoder.finish().unwrap()
        };
        let track = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        let stem = path.file_stem().unwrap().to_string_lossy();
        device.add_recording(
            &format!("{stem}.xrz"),
            &format!("2026-08-30 {:02}:{:02}:00", 9 + i / 6, i % 6 * 10),
            &track,
            bytes,
        );
    }
    let logger = FakeLogger::start(device);
    let addr = &logger.addr;
    println!("serving {} recordings", files.len());
    println!(
        "RACE_OVERLAY_MYCHRON_ADDR={}:{}:{}",
        addr.host, addr.tcp_port, addr.udp_port
    );
    println!("Enter toggles in range / out of range; Ctrl-C quits.");
    for _ in std::io::stdin().lines() {
        let mut device = logger.device();
        device.in_range = !device.in_range;
        println!(
            "{}",
            if device.in_range {
                "in range"
            } else {
                "out of range"
            }
        );
    }
    // No terminal input (e.g. started in the background): keep serving.
    loop {
        std::thread::park();
    }
}
