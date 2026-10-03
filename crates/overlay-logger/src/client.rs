//! A command session on TCP port 2000.
//!
//! Every operation follows the transaction model in the protocol notes:
//! `NC` request → echo → response header → (client acks) → data message.
//! Any error leaves the stream at an unknown position, so the session refuses
//! further operations and the caller reconnects.

use crate::addr::DeviceAddr;
use crate::channels::{
    CATALOG_MAGIC, CatalogChannel, ChannelBlock, SCHEMA_MAGIC, SchemaEntry, TREE_MAGIC,
    catalog_channels, schema_entries,
};
use crate::datalogs::{self, Datalog};
use crate::error::{LoggerError, Result};
use crate::info::DeviceInfo;
use crate::live::{LiveFrame, MainObject, Snapshot};
use crate::records::c_string;
use crate::stcp::{Decoder, FLAG_TIME_SYNC, Frame, OpHeader, Request, Status, Tag, encode};
use chrono::{Datelike, Timelike};
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpStream},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// Operation ids and kinds (`docs/mychron-protocol.md`, "Observed operations").
pub mod op {
    pub const DEVICE_INFO: (u16, u16) = (0x10, 1);
    pub const TIME_NEGOTIATE: (u16, u16) = (0x06, 1);
    pub const FILE_OPEN: (u16, u16) = (0x02, 4);
    pub const CHANNEL_CATALOG: (u16, u16) = (0x02, 2);
    pub const USER_PROFILES: (u16, u16) = (0x24, 6);
    pub const DATALOGS: (u16, u16) = (0x24, 2);
    pub const CHANNEL_TREE: (u16, u16) = (0x08, 2);
    pub const LIVE_SCHEMA: (u16, u16) = (0x09, 2);
    pub const LIVE_START: (u16, u16) = (0x28, 6);
    pub const LIVE_STOP: (u16, u16) = (0x51, 2);
    pub const MAIN_OBJECT: (u16, u16) = (0x03, 2);
    pub const LIVE_HEARTBEAT: (u16, u16) = (0x53, 2);
    pub const LIVE_SNAPSHOT: (u16, u16) = (0x04, 2);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeouts {
    pub connect: Duration,
    /// Wait for each reply frame of an operation.
    pub reply: Duration,
    /// Wait for each download chunk.
    pub chunk: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(3),
            reply: Duration::from_secs(5),
            chunk: Duration::from_secs(10),
        }
    }
}

/// The frames of one completed operation.
#[derive(Clone, Debug)]
pub struct OpResult {
    pub echo: OpHeader,
    pub header: OpHeader,
    /// Data message body after its 4-byte status word, when one was sent.
    pub data: Option<Vec<u8>>,
}

impl OpResult {
    fn body(self, what: &str) -> Result<Vec<u8>> {
        self.data
            .ok_or_else(|| LoggerError::Protocol(format!("{what}: logger sent no data")))
    }
}

/// A clock value as the logger stores it: year, month, day, hour, minute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CivilTime {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl CivilTime {
    pub fn from_datetime<T: Datelike + Timelike>(time: &T) -> Self {
        Self {
            year: time.year().max(0) as u32,
            month: time.month(),
            day: time.day(),
            hour: time.hour(),
            minute: time.minute(),
        }
    }

    fn bytes(self) -> [u8; 20] {
        let mut out = [0u8; 20];
        for (i, value) in [self.year, self.month, self.day, self.hour, self.minute]
            .into_iter()
            .enumerate()
        {
            out[i * 4..i * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        out
    }
}

/// The CP-68 clock write of a time-sync round: UTC and local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockWrite {
    pub utc: CivilTime,
    pub local: CivilTime,
}

impl ClockWrite {
    pub fn now() -> Self {
        Self {
            utc: CivilTime::from_datetime(&chrono::Utc::now()),
            local: CivilTime::from_datetime(&chrono::Local::now()),
        }
    }

    pub fn payload(self) -> [u8; 68] {
        let mut payload = [0u8; 68];
        payload[8..28].copy_from_slice(&self.utc.bytes());
        payload[40..60].copy_from_slice(&self.local.bytes());
        payload
    }
}

/// One entry of the user-profile list (NC `0x24/6`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserProfile {
    /// Leading text of the entry, when it has any.
    pub name: String,
    pub raw: Vec<u8>,
}

/// Progress of a file transfer: bytes received so far and the file size.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64);

pub struct Session {
    stream: TcpStream,
    decoder: Decoder,
    timeouts: Timeouts,
    broken: bool,
}

impl Session {
    /// Connect without any exchange.
    pub fn connect(addr: &DeviceAddr, timeouts: Timeouts) -> Result<Self> {
        let stream = TcpStream::connect_timeout(&addr.tcp()?, timeouts.connect)?;
        stream.set_nodelay(true)?;
        Ok(Self {
            stream,
            decoder: Decoder::default(),
            timeouts,
            broken: false,
        })
    }

    /// Connect and exchange the hello: all that file listing and transfers
    /// need.
    pub fn open(addr: &DeviceAddr, timeouts: Timeouts) -> Result<Self> {
        let mut session = Self::connect(addr, timeouts)?;
        session.hello()?;
        Ok(session)
    }

    pub fn timeouts(&self) -> Timeouts {
        self.timeouts
    }

    /// CP-8 hello `06 08` → `06 09`.
    pub fn hello(&mut self) -> Result<()> {
        self.guard(|s| {
            s.send(Tag::Cp, &[0, 0, 0, 0, 0x06, 0x08, 0, 0])?;
            let reply = s.read_cp(s.timeouts.reply)?;
            if reply.len() == 8 && reply[4..] == [0x06, 0x09, 0, 0] {
                Ok(())
            } else {
                Err(LoggerError::Protocol(format!(
                    "unexpected hello reply {}",
                    crate::channels::hex(&reply)
                )))
            }
        })
    }

    /// Run one operation: request, echo, header, and (when the header
    /// announces data) the acknowledgement and data message.
    pub fn op(&mut self, request: &Request) -> Result<OpResult> {
        let payload = request.payload()?;
        self.guard(|s| {
            s.send(Tag::Nc, &payload)?;
            let echo = OpHeader::parse(&s.read_cp(s.timeouts.reply)?)?;
            let header = OpHeader::parse(&s.read_cp(s.timeouts.reply)?)?;
            let data = s.read_data(&header)?;
            Ok(OpResult { echo, header, data })
        })
    }

    /// A time-sync round (`kind` 1): the request carries
    /// [`FLAG_TIME_SYNC`], and the client writes its clock after the echo.
    /// **This sets the logger's clock.**
    pub fn time_sync(&mut self, id: u16, clock: ClockWrite) -> Result<OpResult> {
        let request = Request {
            flags: FLAG_TIME_SYNC,
            ..Request::new(id, 1)
        };
        let payload = request.payload()?;
        self.guard(|s| {
            s.send(Tag::Nc, &payload)?;
            let echo = OpHeader::parse(&s.read_cp(s.timeouts.reply)?)?;
            s.send(Tag::Cp, &clock.payload())?;
            let mut next = s.read_cp(s.timeouts.reply)?;
            if next.len() == 4 {
                // Optional ACK0 after the clock write.
                next = s.read_cp(s.timeouts.reply)?;
            }
            let header = OpHeader::parse(&next)?;
            let data = s.read_data(&header)?;
            Ok(OpResult { echo, header, data })
        })
    }

    /// NC `0x10/1`: device info, delivered after a clock write. **Sets the
    /// logger's clock.**
    pub fn device_info(&mut self, clock: ClockWrite) -> Result<DeviceInfo> {
        let body = self
            .time_sync(op::DEVICE_INFO.0, clock)?
            .body("device info")?;
        Ok(DeviceInfo::parse(&body))
    }

    /// NC `0x06/1`: the second time-negotiation round (no data). **Sets the
    /// logger's clock.**
    pub fn time_negotiate(&mut self, clock: ClockWrite) -> Result<()> {
        self.time_sync(op::TIME_NEGOTIATE.0, clock).map(|_| ())
    }

    /// NC `0x24/6`: user profiles, as 64-byte entries.
    pub fn user_profiles(&mut self) -> Result<Vec<UserProfile>> {
        let body = self.simple(op::USER_PROFILES)?.unwrap_or_default();
        Ok(body
            .chunks(64)
            .map(|entry| UserProfile {
                name: c_string(
                    &entry[..entry
                        .iter()
                        .position(|b| !(b.is_ascii_graphic() || *b == b' '))
                        .unwrap_or(entry.len())],
                ),
                raw: entry.to_vec(),
            })
            .collect())
    }

    /// NC `0x24/2`: every recording stored on the logger.
    pub fn datalogs(&mut self) -> Result<Vec<Datalog>> {
        match self.simple(op::DATALOGS)? {
            Some(body) => datalogs::parse(&body),
            None => Ok(Vec::new()),
        }
    }

    /// NC `0x02/2`: the channel catalog block.
    pub fn channel_catalog(&mut self) -> Result<(ChannelBlock, Vec<CatalogChannel>)> {
        let block = self.block(op::CHANNEL_CATALOG, CATALOG_MAGIC)?;
        let channels = catalog_channels(&block);
        Ok((block, channels))
    }

    /// NC `0x08/2`: the channel tree block.
    pub fn channel_tree(&mut self) -> Result<ChannelBlock> {
        self.block(op::CHANNEL_TREE, TREE_MAGIC)
    }

    /// NC `0x09/2`: the live-frame schema block.
    pub fn live_schema(&mut self) -> Result<(ChannelBlock, Vec<SchemaEntry>)> {
        let block = self.block(op::LIVE_SCHEMA, SCHEMA_MAGIC)?;
        let entries = schema_entries(&block);
        Ok((block, entries))
    }

    /// NC `0x03/2`: the current profile name on the first session after
    /// power-on, a live frame afterwards.
    pub fn main_object(&mut self) -> Result<MainObject> {
        Ok(MainObject::parse(
            &self.simple(op::MAIN_OBJECT)?.unwrap_or_default(),
        ))
    }

    pub fn start_live(&mut self) -> Result<()> {
        self.simple(op::LIVE_START).map(|_| ())
    }

    pub fn stop_live(&mut self) -> Result<()> {
        let request = Request {
            stop: true,
            ..Request::new(op::LIVE_STOP.0, op::LIVE_STOP.1)
        };
        self.op(&request).map(|_| ())
    }

    /// One live frame via NC `0x03/2`.
    pub fn live_frame(&mut self) -> Result<LiveFrame> {
        match self.main_object()? {
            MainObject::Frame(frame) => Ok(frame),
            other => Err(LoggerError::Protocol(format!(
                "expected a live frame, got {other:?}"
            ))),
        }
    }

    /// NC `0x53/2`: the 8-byte heartbeat frame.
    pub fn live_heartbeat(&mut self) -> Result<Option<LiveFrame>> {
        Ok(self
            .simple(op::LIVE_HEARTBEAT)?
            .and_then(|body| LiveFrame::parse(&body)))
    }

    /// NC `0x04/2`: compact live value snapshot.
    pub fn live_snapshot(&mut self) -> Result<Snapshot> {
        Ok(Snapshot::parse(
            &self.simple(op::LIVE_SNAPSHOT)?.unwrap_or_default(),
        ))
    }

    /// Size of a device file, `None` when it does not exist. The logger
    /// pushes the first chunk once an open is acknowledged, so that chunk is
    /// read and discarded to keep the session in step.
    pub fn stat(&mut self, path: &str) -> Result<Option<u32>> {
        let request = Request::new(op::FILE_OPEN.0, op::FILE_OPEN.1).with_path(path);
        let payload = request.payload()?;
        self.guard(|s| {
            s.send(Tag::Nc, &payload)?;
            OpHeader::parse(&s.read_cp(s.timeouts.reply)?)?;
            let header = OpHeader::parse(&s.read_cp(s.timeouts.reply)?)?;
            if header.length == 0 {
                return Ok(None);
            }
            s.send(Tag::Cp, &[0; 4])?;
            s.read_chunk(0)?;
            Ok(Some(header.length))
        })
    }

    /// Download any device path into `out`, returning the byte count.
    ///
    /// The open's header carries the file size; the logger pushes chunk 0
    /// after the header is acknowledged, then each further chunk is
    /// requested by its `u32le` offset. Fails with
    /// [`LoggerError::NotFound`] for an absent file and
    /// [`LoggerError::Cancelled`] when `cancel` is set between chunks.
    pub fn read_file(
        &mut self,
        path: &str,
        out: &mut dyn Write,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> Result<u64> {
        let request = Request::new(op::FILE_OPEN.0, op::FILE_OPEN.1).with_path(path);
        let payload = request.payload()?;
        self.guard(|s| {
            s.send(Tag::Nc, &payload)?;
            OpHeader::parse(&s.read_cp(s.timeouts.reply)?)?;
            let header = OpHeader::parse(&s.read_cp(s.timeouts.reply)?)?;
            let size = u64::from(header.length);
            if size == 0 {
                return Err(match header.status {
                    Status::NoData => LoggerError::NotFound(path.to_owned()),
                    status => LoggerError::Status(status.code()),
                });
            }
            progress(0, size);
            s.send(Tag::Cp, &[0; 4])?;
            let mut received = 0u64;
            loop {
                let chunk = s.read_chunk(received as u32)?;
                if chunk.is_empty() || received + chunk.len() as u64 > size {
                    return Err(LoggerError::SizeMismatch {
                        expected: size,
                        received: received + chunk.len() as u64,
                    });
                }
                out.write_all(&chunk)?;
                received += chunk.len() as u64;
                progress(received, size);
                if received == size {
                    return Ok(size);
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(LoggerError::Cancelled);
                }
                s.send(Tag::Cp, &(received as u32).to_le_bytes())?;
            }
        })
    }

    /// Download a recording listed by [`Self::datalogs`].
    pub fn download_datalog(
        &mut self,
        log: &Datalog,
        out: &mut dyn Write,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> Result<u64> {
        let size = self.read_file(&log.device_path(), out, progress, cancel)?;
        if size != log.size {
            return Err(LoggerError::SizeMismatch {
                expected: log.size,
                received: size,
            });
        }
        Ok(size)
    }

    pub fn close(self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn simple(&mut self, (id, kind): (u16, u16)) -> Result<Option<Vec<u8>>> {
        Ok(self.op(&Request::new(id, kind))?.data)
    }

    fn block(&mut self, id: (u16, u16), magic: [u8; 4]) -> Result<ChannelBlock> {
        let body = self
            .simple(id)?
            .ok_or_else(|| LoggerError::Protocol("channel block: logger sent no data".into()))?;
        ChannelBlock::parse(&body, magic)
    }

    /// Ack a response header and read the data message it announced.
    fn read_data(&mut self, header: &OpHeader) -> Result<Option<Vec<u8>>> {
        if header.length == 0 {
            return Ok(None);
        }
        self.send(Tag::Cp, &[0; 4])?;
        let mut data = self.read_cp(self.timeouts.reply)?;
        if data.len() != header.length as usize + 4 {
            return Err(LoggerError::Protocol(format!(
                "header announced {} bytes, data message holds {}",
                header.length,
                data.len().saturating_sub(4)
            )));
        }
        let status = u32::from_le_bytes(data[..4].try_into().unwrap());
        if status != 0 {
            return Err(LoggerError::Status(status));
        }
        data.drain(..4);
        Ok(Some(data))
    }

    /// A download chunk: `u32le` offset echo followed by the bytes.
    fn read_chunk(&mut self, offset: u32) -> Result<Vec<u8>> {
        let mut chunk = self.read_cp(self.timeouts.chunk)?;
        if chunk.len() < 4 {
            return Err(LoggerError::Protocol("short download chunk".into()));
        }
        let echoed = u32::from_le_bytes(chunk[..4].try_into().unwrap());
        if echoed != offset {
            return Err(LoggerError::Protocol(format!(
                "asked for offset {offset:#x}, logger sent {echoed:#x}"
            )));
        }
        chunk.drain(..4);
        Ok(chunk)
    }

    fn guard<T>(&mut self, run: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.broken {
            return Err(LoggerError::Protocol(
                "session is out of step after an earlier error; reconnect".into(),
            ));
        }
        let result = run(self);
        if result.is_err() {
            self.broken = true;
        }
        result
    }

    fn send(&mut self, tag: Tag, payload: &[u8]) -> Result<()> {
        self.stream.write_all(&encode(tag, payload))?;
        Ok(())
    }

    fn read_cp(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let frame = self.read_frame(timeout)?;
        if frame.tag != Tag::Cp {
            return Err(LoggerError::Protocol("logger sent an NC frame".into()));
        }
        Ok(frame.payload)
    }

    fn read_frame(&mut self, timeout: Duration) -> Result<Frame> {
        let deadline = Instant::now() + timeout;
        let mut buffer = [0u8; 65_536];
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(frame);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(LoggerError::Timeout);
            }
            self.stream.set_read_timeout(Some(left))?;
            let n = self.stream.read(&mut buffer)?;
            if n == 0 {
                return Err(LoggerError::Closed);
            }
            self.decoder.push(&buffer[..n]);
        }
    }
}

/// Run one operation on its own connection, as a probe of an object id.
/// `Ok(None)` when the logger never answers, which is how unknown ids behave.
pub fn probe_object(
    addr: &DeviceAddr,
    timeouts: Timeouts,
    request: &Request,
) -> Result<Option<OpResult>> {
    let mut session = Session::open(addr, timeouts)?;
    let result = match session.op(request) {
        Ok(result) => Ok(Some(result)),
        Err(LoggerError::Timeout) => Ok(None),
        Err(error) => Err(error),
    };
    session.close();
    result
}
