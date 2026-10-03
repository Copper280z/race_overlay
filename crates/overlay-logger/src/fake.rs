//! An in-process MyChron that speaks the protocol on loopback ports, for
//! deterministic tests of this crate and of the app's download workflow.
//!
//! The device state is shared, so a test can add recordings, take the logger
//! "out of range", or script faults while clients are connected.

use crate::addr::DeviceAddr;
use crate::channels::{CATALOG_MAGIC, SCHEMA_MAGIC, TREE_MAGIC, catalog_payload, encode_block};
use crate::datalogs::{self, Datalog};
use crate::discovery::{KEEPALIVE, encode_descriptor};
use crate::live::FRAME_MAGIC;
use crate::records;
use crate::stcp::{CHUNK_SIZE, Decoder, OpHeader, Status, Tag, encode};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

/// A recording held by the fake logger.
#[derive(Clone, Debug)]
pub struct FakeRecording {
    pub log: Datalog,
    pub bytes: Vec<u8>,
}

/// Scriptable device state.
#[derive(Clone, Debug)]
pub struct FakeDevice {
    /// Answer UDP keepalives and accept TCP sessions.
    pub in_range: bool,
    /// Ignore this many keepalives (lost Wi-Fi replies).
    pub drop_replies: u32,
    pub identity: Vec<u8>,
    pub recordings: Vec<FakeRecording>,
    /// Other readable device paths (`0:/tkk/dev.ria`, …) and their bytes.
    pub files: Vec<(String, Vec<u8>)>,
    pub info_blob: Vec<u8>,
    pub user_profiles: Vec<u8>,
    /// `(id, kind)` operations that never answer.
    pub silent: BTreeSet<(u16, u16)>,
    /// Stop serving a file after this many bytes (simulates a dropped link).
    pub truncate_after: Option<u64>,
    /// Corrupt the checksum of the next data message.
    pub corrupt_next_data: bool,
    /// The next NC `0x03/2` returns the profile string instead of a frame,
    /// like the first session after power-on.
    pub fresh_boot: bool,
    /// Clock writes received, in order.
    pub clock_writes: Vec<Vec<u8>>,
    /// Device paths opened, in order.
    pub opened: Vec<String>,
    /// Datalog list requests served.
    pub listings: usize,
    pub live: bool,
    pub tick_ms: u32,
}

pub const SCHEMA_HASH: u32 = 0xf951_1f4c;

impl Default for FakeDevice {
    fn default() -> Self {
        let record = |tag: &[u8; 4], payload: &[u8]| records::encode(tag, b'a', payload);
        let mut info = record(b"iMST", b"idn\x01\x04\x00\x0e\x02\xb5\x01");
        info.extend(record(
            b"iHW ",
            b"WiFi=ESP32|Reg=usa|LSM6DSV16X|Led=PI33TB|MYC68B|M101|   \0",
        ));
        info.extend(record(
            b"iUSR",
            b"<hUSR 0000000092a>\r\ndevice=|\r\npilota=Test Driver|\r\nveicolo=KA100|\r\ncampionato=|\r\n<USR 008273>",
        ));
        info.extend(record(
            b"iPTH",
            b"media=0:,0:.N|\r\ntracks=0:/gps,0:/gps.N|\r\ntkk=0:/tkk,0:/tkk.N|\r\n",
        ));
        info.extend(record(b"iLCK", &[0; 47]));
        let mut users = Vec::new();
        for name in ["System", "Driver 1", "Driver 2", "Driver 3", "Driver 4"] {
            let mut entry = name.as_bytes().to_vec();
            entry.resize(64, 0);
            users.extend(entry);
        }
        Self {
            in_range: true,
            drop_replies: 0,
            identity: b"\xa8\x01\xb5\x01\x00\x00\x61\x3c\x16\x02".to_vec(),
            recordings: Vec::new(),
            files: vec![("0:/tkk/dev.ria".into(), b"Yard\0KELLYS\0".to_vec())],
            info_blob: info,
            user_profiles: users,
            silent: [(0x01, 2), (0x05, 2)].into_iter().collect(),
            truncate_after: None,
            corrupt_next_data: false,
            fresh_boot: true,
            clock_writes: Vec::new(),
            opened: Vec::new(),
            listings: 0,
            live: false,
            tick_ms: 1_000,
        }
    }
}

impl FakeDevice {
    /// Add a recording with plausible list metadata; `bytes` is served as
    /// the file contents.
    pub fn add_recording(&mut self, name: &str, recorded: &str, track: &str, bytes: Vec<u8>) {
        let recorded = chrono::NaiveDateTime::parse_from_str(recorded, "%Y-%m-%d %H:%M:%S").ok();
        self.recordings.push(FakeRecording {
            log: Datalog {
                name: name.into(),
                size: bytes.len() as u64,
                recorded,
                laps: Some(5),
                best_lap_number: Some(3),
                best_lap_ms: Some(36_605),
                driver: String::new(),
                track: track.into(),
                vehicle: String::new(),
                championship: String::new(),
                venue_type: String::new(),
                mode: "speed".into(),
                track_type: "closed".into(),
                stop_reason: "stop".into(),
                max_speed_raw: Some(1_079_717_068),
                device: String::new(),
                track_position: Some((42.012_345_6, -71.065_432_1)),
                duration_ms: Some(141_383),
                valid: String::new(),
                other: Default::default(),
            },
            bytes,
        });
    }

    fn file(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(name) = path.strip_prefix("1:/mem/") {
            return self
                .recordings
                .iter()
                .find(|r| r.log.name == name)
                .map(|r| r.bytes.clone());
        }
        self.files
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, bytes)| bytes.clone())
    }

    fn object(&mut self, id: u16, kind: u16) -> Option<Vec<u8>> {
        if (id, kind) == (0x24, 2) {
            self.listings += 1;
        }
        Some(match (id, kind) {
            (0x24, 2) => datalogs::encode(
                &self
                    .recordings
                    .iter()
                    .map(|r| r.log.clone())
                    .collect::<Vec<_>>(),
            ),
            (0x24, 6) => self.user_profiles.clone(),
            (0x02, 2) => encode_block(
                CATALOG_MAGIC,
                0x67e9_3204,
                &[
                    catalog_payload(0, "MClk", "Master Clk"),
                    catalog_payload(1, "LAP", "Lap Time"),
                    catalog_payload(28, "RPM", "RPM"),
                ],
            ),
            (0x08, 2) => encode_block(
                TREE_MAGIC,
                0x1845_0a9c,
                &vec![vec![0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]; 3],
            ),
            (0x09, 2) => {
                let mut scaled = vec![0u8; 36];
                scaled[..4].copy_from_slice(&28u32.to_le_bytes());
                scaled[4..8].copy_from_slice(&0x14u32.to_le_bytes());
                scaled[20..24].copy_from_slice(&0.001f32.to_le_bytes());
                encode_block(
                    SCHEMA_MAGIC,
                    SCHEMA_HASH,
                    &[vec![0; 16], vec![0; 16], scaled],
                )
            }
            (0x03, 2) if self.fresh_boot => {
                self.fresh_boot = false;
                b"System\0\0\x15".to_vec()
            }
            (0x03, 2) => {
                self.tick_ms += 270;
                let mut frame = FRAME_MAGIC.to_vec();
                frame.extend_from_slice(&[0; 4]);
                frame.extend_from_slice(&self.tick_ms.to_le_bytes());
                frame.resize(496, 0);
                frame
            }
            (0x53, 2) => [FRAME_MAGIC.as_slice(), &[0; 4]].concat(),
            (0x04, 2) => records::encode(b"iMST", b'a', &self.tick_ms.to_le_bytes()),
            (0x28, 6) => {
                self.live = true;
                Vec::new()
            }
            (0x51, 2) => {
                self.live = false;
                Vec::new()
            }
            (0x52, 2) | (0x54, 2) => Vec::new(),
            _ => return None,
        })
    }
}

/// A running fake logger. Dropping it stops the servers.
pub struct FakeLogger {
    pub addr: DeviceAddr,
    device: Arc<Mutex<FakeDevice>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl FakeLogger {
    pub fn start(device: FakeDevice) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake TCP");
        listener.set_nonblocking(true).unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").expect("bind fake UDP");
        udp.set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let addr = DeviceAddr {
            host: "127.0.0.1".into(),
            tcp_port: listener.local_addr().unwrap().port(),
            udp_port: udp.local_addr().unwrap().port(),
        };
        let device = Arc::new(Mutex::new(device));
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();

        let (d, s) = (Arc::clone(&device), Arc::clone(&stop));
        threads.push(std::thread::spawn(move || {
            let mut buffer = [0u8; 64];
            while !s.load(Ordering::Relaxed) {
                if let Ok((n, from)) = udp.recv_from(&mut buffer) {
                    let mut device = lock(&d);
                    if device.drop_replies > 0 {
                        device.drop_replies -= 1;
                        continue;
                    }
                    if device.in_range && &buffer[..n] == KEEPALIVE {
                        let _ = udp.send_to(&encode_descriptor(&device.identity), from);
                    }
                }
            }
        }));

        let (d, s) = (Arc::clone(&device), Arc::clone(&stop));
        threads.push(std::thread::spawn(move || {
            let mut connections = Vec::new();
            while !s.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        if !lock(&d).in_range {
                            drop(stream);
                            continue;
                        }
                        let (d, s) = (Arc::clone(&d), Arc::clone(&s));
                        connections.push(std::thread::spawn(move || serve(stream, &d, &s)));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            for connection in connections {
                let _ = connection.join();
            }
        }));

        Self {
            addr,
            device,
            stop,
            threads,
        }
    }

    pub fn device(&self) -> MutexGuard<'_, FakeDevice> {
        lock(&self.device)
    }
}

impl Drop for FakeLogger {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn lock(device: &Mutex<FakeDevice>) -> MutexGuard<'_, FakeDevice> {
    device.lock().unwrap_or_else(|poison| poison.into_inner())
}

enum Pending {
    None,
    /// Header sent; data follows the client's ack.
    Data(Vec<u8>),
    /// Echo sent for a time-sync round; waiting for the clock write.
    Clock(u16, [u8; 64]),
    /// A file is open; CP-4 frames are offset reads.
    File(Vec<u8>),
}

fn serve(mut stream: TcpStream, device: &Mutex<FakeDevice>, stop: &AtomicBool) {
    stream
        .set_read_timeout(Some(Duration::from_millis(20)))
        .unwrap();
    let mut decoder = Decoder::default();
    let mut pending = Pending::None;
    let mut buffer = [0u8; 4096];
    while !stop.load(Ordering::Relaxed) {
        if !lock(device).in_range {
            return;
        }
        match stream.read(&mut buffer) {
            Ok(0) => return,
            Ok(n) => decoder.push(&buffer[..n]),
            Err(_) => continue,
        }
        while let Ok(Some(frame)) = decoder.next_frame() {
            let replies = handle(frame.tag, &frame.payload, &mut pending, device);
            for (payload, corrupt) in replies {
                let mut bytes = encode(Tag::Cp, &payload);
                if corrupt {
                    let at = bytes.len() - 3;
                    bytes[at] ^= 0xff;
                }
                if stream.write_all(&bytes).is_err() {
                    return;
                }
            }
        }
    }
}

fn reply_header(request: &[u8; 64], length: u32, status: Status) -> Vec<u8> {
    let mut header = *request;
    header[16..20].copy_from_slice(&length.to_le_bytes());
    header[20..24].copy_from_slice(&CHUNK_SIZE.to_le_bytes());
    header[24..28].copy_from_slice(&status.code().to_le_bytes());
    header.to_vec()
}

fn data_message(body: &[u8]) -> Vec<u8> {
    [&[0u8; 4][..], body].concat()
}

fn chunk(file: &[u8], offset: u32, limit: Option<u64>) -> Vec<u8> {
    let start = (offset as usize).min(file.len());
    let mut end = (start + CHUNK_SIZE as usize).min(file.len());
    if let Some(limit) = limit {
        end = end.min(limit as usize).max(start);
    }
    [&offset.to_le_bytes()[..], &file[start..end]].concat()
}

/// Replies to one client frame, each with a "corrupt checksum" flag.
fn handle(
    tag: Tag,
    payload: &[u8],
    pending: &mut Pending,
    device: &Mutex<FakeDevice>,
) -> Vec<(Vec<u8>, bool)> {
    let mut device = lock(device);
    match tag {
        Tag::Cp if payload.len() == 8 && payload[4..6] == [0x06, 0x08] => {
            vec![(vec![0, 0, 0, 0, 0x06, 0x09, 0, 0], false)]
        }
        Tag::Cp if payload.len() == 68 => {
            device.clock_writes.push(payload.to_vec());
            let Pending::Clock(id, request) = std::mem::replace(pending, Pending::None) else {
                return Vec::new();
            };
            if id == 0x10 {
                let body = device.info_blob.clone();
                let header = reply_header(&request, body.len() as u32, Status::DataReady);
                *pending = Pending::Data(body);
                vec![(vec![0; 4], false), (header, false)]
            } else {
                vec![(reply_header(&request, 0, Status::NoData), false)]
            }
        }
        Tag::Cp if payload.len() == 4 => {
            let offset = u32::from_le_bytes(payload.try_into().unwrap());
            match std::mem::replace(pending, Pending::None) {
                Pending::Data(body) => {
                    let corrupt = std::mem::take(&mut device.corrupt_next_data);
                    vec![(data_message(&body), corrupt)]
                }
                Pending::File(file) => {
                    let corrupt = std::mem::take(&mut device.corrupt_next_data);
                    let reply = chunk(&file, offset, device.truncate_after);
                    *pending = Pending::File(file);
                    vec![(reply, corrupt)]
                }
                other => {
                    *pending = other;
                    Vec::new()
                }
            }
        }
        Tag::Nc if payload.len() == 64 => {
            let request: [u8; 64] = payload.try_into().unwrap();
            let Ok(header) = OpHeader::parse(payload) else {
                return Vec::new();
            };
            *pending = Pending::None;
            if device.silent.contains(&(header.id, header.kind)) {
                return Vec::new();
            }
            let echo = reply_header(&request, 0, Status::Ack);
            if header.kind == 1 && header.flags & crate::stcp::FLAG_TIME_SYNC != 0 {
                *pending = Pending::Clock(header.id, request);
                return vec![(echo, false)];
            }
            if (header.id, header.kind) == (0x02, 4) {
                let path = header.path().unwrap_or_default();
                device.opened.push(path.clone());
                return match device.file(&path) {
                    Some(file) if !file.is_empty() => {
                        let size = file.len() as u32;
                        *pending = Pending::File(file);
                        vec![
                            (echo, false),
                            (reply_header(&request, size, Status::DataReady), false),
                        ]
                    }
                    _ => vec![
                        (echo, false),
                        (reply_header(&request, 0, Status::NoData), false),
                    ],
                };
            }
            match device.object(header.id, header.kind) {
                None => Vec::new(),
                Some(body) if body.is_empty() => {
                    vec![
                        (echo, false),
                        (reply_header(&request, 0, Status::DataReady), false),
                    ]
                }
                Some(body) => {
                    let length = body.len() as u32;
                    *pending = Pending::Data(body);
                    vec![
                        (echo, false),
                        (reply_header(&request, length, Status::DataReady), false),
                    ]
                }
            }
        }
        _ => Vec::new(),
    }
}
