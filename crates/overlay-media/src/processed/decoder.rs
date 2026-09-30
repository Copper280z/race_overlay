use crate::{FfmpegTools, MediaError, Rational};
use crossbeam_channel::{Receiver, Sender};
use serde::Deserialize;
use std::{
    io::{BufRead, BufReader, Read},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};
#[derive(Clone, Debug)]
pub struct DualVideoInfo {
    pub tracks: [u32; 2],
    pub time_bases: [Rational; 2],
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub full_range: bool,
    pub start: f64,
    pub duration: f64,
}
#[derive(Deserialize)]
struct Probe {
    streams: Vec<Track>,
}
#[derive(Deserialize)]
struct Track {
    index: u32,
    codec_type: Option<String>,
    codec_name: Option<String>,
    pix_fmt: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
    time_base: Option<String>,
    start_time: Option<String>,
    duration: Option<String>,
    color_transfer: Option<String>,
    color_range: Option<String>,
}
pub fn probe_dual_video(tools: &FfmpegTools, path: &Path) -> Result<DualVideoInfo, MediaError> {
    let output = Command::new(&tools.ffprobe)
        .args(["-v", "error", "-show_streams", "-of", "json"])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(MediaError::Process(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let parsed: Probe =
        serde_json::from_slice(&output.stdout).map_err(|e| MediaError::Invalid(e.to_string()))?;
    validate_tracks(parsed.streams)
}
fn validate_tracks(tracks: Vec<Track>) -> Result<DualVideoInfo, MediaError> {
    let tracks = tracks
        .into_iter()
        .filter(|s| s.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    let invalid = || {
        MediaError::Invalid("Raw video requires two matching square SDR HEVC/H.264 tracks with valid frame rate and timestamps".into())
    };
    if tracks.len() != 2 {
        return Err(invalid());
    }
    let a = &tracks[0];
    let b = &tracks[1];
    let fps = a
        .avg_frame_rate
        .as_deref()
        .and_then(Rational::parse)
        .ok_or_else(invalid)?;
    let start = |s: &Track| s.start_time.as_deref().and_then(|s| s.parse::<f64>().ok());
    let duration = |s: &Track| s.duration.as_deref().and_then(|s| s.parse::<f64>().ok());
    if a.width != a.height
        || a.width != b.width
        || a.height != b.height
        || a.width.unwrap_or(0) == 0
        || a.width.unwrap_or(0) > 8192
        || b.avg_frame_rate.as_deref().and_then(Rational::parse) != Some(fps)
        || fps.as_f64() <= 0.
        || fps.as_f64() > 60.
        || tracks.iter().any(|s| {
            !matches!(s.codec_name.as_deref(), Some("hevc" | "h264"))
                || !matches!(s.pix_fmt.as_deref(), Some("yuv420p" | "yuvj420p"))
                || s.color_transfer
                    .as_deref()
                    .is_some_and(|c| !matches!(c, "bt709" | "unknown"))
        })
    {
        return Err(invalid());
    }
    let time_bases = [a, b].map(|track| {
        track
            .time_base
            .as_deref()
            .and_then(Rational::parse)
            .filter(|t| t.as_f64() > 0.)
    });
    let [Some(first_time_base), Some(second_time_base)] = time_bases else {
        return Err(invalid());
    };
    let full_range = |track: &Track| {
        track.color_range.as_deref() == Some("pc")
            || (track.color_range.as_deref().is_none_or(|v| v == "unknown")
                && track.pix_fmt.as_deref() == Some("yuvj420p"))
    };
    if full_range(a) != full_range(b) {
        return Err(MediaError::Invalid("Lens color ranges do not match".into()));
    }
    let start_a = start(a).filter(|t| t.is_finite()).ok_or_else(invalid)?;
    if start(b).is_none_or(|v| !v.is_finite() || (v - start_a).abs() > 0.0001) {
        return Err(invalid());
    }
    let d = duration(a)
        .zip(duration(b))
        .map(|(a, b)| a.min(b))
        .filter(|v| v.is_finite() && *v > 0.)
        .ok_or_else(invalid)?;
    Ok(DualVideoInfo {
        tracks: [a.index, b.index],
        time_bases: [first_time_base, second_time_base],
        width: a.width.unwrap(),
        height: a.height.unwrap(),
        fps,
        full_range: full_range(a),
        start: start_a,
        duration: d,
    })
}
#[derive(Debug)]
pub struct LensFramePair {
    pub timestamp: f64,
    pub width: u32,
    pub height: u32,
    pub pixels: [Vec<u8>; 2],
}
struct TrackFrame {
    time: f64,
    pixels: Vec<u8>,
}
struct TrackDecoder {
    child: Child,
    frames: Option<Receiver<Result<TrackFrame, MediaError>>>,
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}
fn send<T: Send>(tx: &Sender<T>, mut value: T, stop: &AtomicBool) -> bool {
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        match tx.send_timeout(value, Duration::from_millis(20)) {
            Ok(()) => return true,
            Err(crossbeam_channel::SendTimeoutError::Timeout(v)) => value = v,
            Err(_) => return false,
        }
    }
}
fn pts(line: &str) -> Option<i64> {
    if !line.contains("showinfo") {
        return None;
    }
    line.split(" pts:")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}
impl TrackDecoder {
    fn start(
        tools: &FfmpegTools,
        path: &Path,
        track: u32,
        time_base: Rational,
        start: f64,
        size: (u32, u32),
    ) -> Result<Self, MediaError> {
        let mut command = Command::new(&tools.ffmpeg);
        #[cfg(target_os = "macos")]
        command.args(["-hwaccel", "videotoolbox"]);
        let mut child = command
            .args([
                "-hide_banner",
                "-loglevel",
                "info",
                "-nostdin",
                "-threads",
                "2",
                "-copyts",
                "-ss",
            ])
            .arg(format!("{start:.9}"))
            .arg("-i")
            .arg(path)
            .args([
                "-map",
                &format!("0:{track}"),
                "-an",
                "-sn",
                "-dn",
                "-filter_threads",
                "1",
                "-vf",
                &format!(
                    "scale={}:{}:flags=bilinear:in_color_matrix=bt709:out_range=full,format=rgba,showinfo",
                    size.0, size.1
                ),
                "-fps_mode",
                "passthrough",
                "-pix_fmt",
                "rgba",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let stop = Arc::new(AtomicBool::new(false));
        let (ptx, prx) = crossbeam_channel::bounded(4);
        let flag = stop.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if flag.load(Ordering::Acquire) {
                    break;
                }
                if let Some(time) = pts(&line)
                    && !send(&ptx, time as f64 * time_base.as_f64(), &flag)
                {
                    break;
                }
            }
        });
        let (tx, rx) = crossbeam_channel::bounded(2);
        let flag = stop.clone();
        let reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                if flag.load(Ordering::Acquire) {
                    break;
                }
                let mut pixels = vec![0; size.0 as usize * size.1 as usize * 4];
                match reader.read_exact(&mut pixels) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(e) => {
                        let _ = send(&tx, Err(MediaError::Io(e)), &flag);
                        break;
                    }
                }
                let time = loop {
                    if flag.load(Ordering::Acquire) {
                        return;
                    }
                    match prx.recv_timeout(Duration::from_millis(20)) {
                        Ok(t) => break t,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        Err(_) => {
                            let _ = send(
                                &tx,
                                Err(MediaError::Process(
                                    "Decoded video frame has no presentation timestamp".into(),
                                )),
                                &flag,
                            );
                            return;
                        }
                    }
                };
                if !send(&tx, Ok(TrackFrame { time, pixels }), &flag) {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            frames: Some(rx),
            stop,
            reader: Some(reader),
        })
    }
    fn next(&mut self, cancel: &AtomicBool) -> Result<Option<TrackFrame>, MediaError> {
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(MediaError::Cancelled);
            }
            match self
                .frames
                .as_ref()
                .expect("active decoder")
                .recv_timeout(Duration::from_millis(20))
            {
                Ok(v) => return v.map(Some),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(_) => {
                    if let Some(status) = self.child.try_wait()?
                        && !status.success()
                    {
                        return Err(MediaError::Process(format!(
                            "Raw video decoder failed: {status}"
                        )));
                    }
                    return Ok(None);
                }
            }
        }
    }
}
impl Drop for TrackDecoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.frames.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
pub struct DualVideoDecoder {
    tracks: [TrackDecoder; 2],
    size: (u32, u32),
    origin: f64,
}
impl DualVideoDecoder {
    pub fn start(
        tools: &FfmpegTools,
        path: &Path,
        info: &DualVideoInfo,
        start: f64,
        max_lens_size: Option<u32>,
    ) -> Result<Self, MediaError> {
        let width = max_lens_size.map_or(info.width, |n| info.width.min(n));
        let size = (width, width);
        let a = TrackDecoder::start(tools, path, info.tracks[0], info.time_bases[0], start, size)?;
        let b = TrackDecoder::start(tools, path, info.tracks[1], info.time_bases[1], start, size)?;
        Ok(Self {
            tracks: [a, b],
            size,
            origin: info.start,
        })
    }
    pub fn next(&mut self, cancel: &AtomicBool) -> Result<Option<LensFramePair>, MediaError> {
        let Some(a) = self.tracks[0].next(cancel)? else {
            return Ok(None);
        };
        let Some(b) = self.tracks[1].next(cancel)? else {
            return Ok(None);
        };
        if (a.time - b.time).abs() > 0.0001 {
            return Err(MediaError::Invalid(format!(
                "Lens timestamps do not match ({:.6}s / {:.6}s); no frame was synthesized",
                a.time, b.time
            )));
        }
        Ok(Some(LensFramePair {
            timestamp: a.time - self.origin,
            width: self.size.0,
            height: self.size.1,
            pixels: [a.pixels, b.pixels],
        }))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_showinfo_timestamps() {
        assert_eq!(
            pts("[Parsed_showinfo_2] n: 1 pts: 1001 pts_time:0.0333667 duration:1001"),
            Some(1001)
        );
        assert_eq!(pts("noise pts_time:1"), None);
    }
    #[test]
    fn rejects_single_track() {
        assert!(validate_tracks(Vec::new()).is_err());
    }
}
