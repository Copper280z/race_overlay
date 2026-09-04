use crate::{FfmpegTools, MediaError};
use crossbeam_channel::{Receiver, Sender};
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewSize {
    pub width: u32,
    pub height: u32,
}
impl PreviewSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone)]
pub struct PreviewRequest {
    pub timestamp: f64,
    pub size: PreviewSize,
}

#[derive(Debug, Clone)]
pub struct PreviewFrame {
    pub timestamp: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

enum Job {
    Frame(PreviewRequest),
    Play {
        start: f64,
        fps: f64,
        size: PreviewSize,
        stop: Arc<AtomicBool>,
    },
}

pub struct PreviewHandle {
    jobs: Sender<Job>,
    frames: Receiver<Result<PreviewFrame, MediaError>>,
    cancelled: Arc<AtomicBool>,
}

pub struct PreviewPlayback {
    stop: Arc<AtomicBool>,
}
impl PreviewPlayback {
    pub fn pause(&self) {
        self.stop.store(true, Ordering::Release);
    }
    pub fn is_playing(&self) -> bool {
        !self.stop.load(Ordering::Acquire)
    }
}
impl Drop for PreviewPlayback {
    fn drop(&mut self) {
        self.pause();
    }
}

impl PreviewHandle {
    pub fn request(&self, timestamp: f64, size: PreviewSize) -> Result<(), MediaError> {
        if !timestamp.is_finite() || timestamp < 0.0 || size.width == 0 || size.height == 0 {
            return Err(MediaError::Invalid("invalid preview request".into()));
        }
        self.cancelled.store(false, Ordering::Release);
        self.jobs
            .send(Job::Frame(PreviewRequest { timestamp, size }))
            .map_err(|_| MediaError::Process("preview worker stopped".into()))
    }

    pub fn request_frame(&self, request: PreviewRequest) -> Result<(), MediaError> {
        self.request(request.timestamp, request.size)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Start one continuously decoded muted FFmpeg stream. This avoids the
    /// process-launch overhead of seeking a new FFmpeg instance for each frame.
    pub fn play(
        &self,
        start: f64,
        fps: f64,
        size: PreviewSize,
    ) -> Result<PreviewPlayback, MediaError> {
        if !start.is_finite()
            || start < 0.0
            || !fps.is_finite()
            || fps <= 0.0
            || size.width == 0
            || size.height == 0
        {
            return Err(MediaError::Invalid(
                "invalid preview playback request".into(),
            ));
        }
        let stop = Arc::new(AtomicBool::new(false));
        self.jobs
            .send(Job::Play {
                start,
                fps,
                size,
                stop: stop.clone(),
            })
            .map_err(|_| MediaError::Process("preview worker stopped".into()))?;
        Ok(PreviewPlayback { stop })
    }

    pub fn try_recv(&self) -> Option<Result<PreviewFrame, MediaError>> {
        self.frames.try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<PreviewFrame, MediaError> {
        self.frames
            .recv_timeout(timeout)
            .map_err(|_| MediaError::Process("preview worker stopped or timed out".into()))?
    }
}

pub struct PreviewWorker;
impl PreviewWorker {
    pub fn spawn(tools: FfmpegTools, path: impl Into<PathBuf>) -> PreviewHandle {
        let (job_tx, job_rx) = crossbeam_channel::unbounded::<Job>();
        // A preview frame is several megabytes. Keep only a short queue so a
        // minimized or temporarily busy UI cannot accumulate unbounded video.
        let (frame_tx, frame_rx) = crossbeam_channel::bounded(3);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let path = path.into();
        thread::Builder::new()
            .name("overlay-preview".into())
            .spawn(move || worker_loop(tools, path, job_rx, frame_tx, flag))
            .expect("preview worker thread");
        PreviewHandle {
            jobs: job_tx,
            frames: frame_rx,
            cancelled,
        }
    }

    pub fn start(tools: FfmpegTools, path: impl Into<PathBuf>) -> PreviewHandle {
        Self::spawn(tools, path)
    }
}

fn worker_loop(
    tools: FfmpegTools,
    path: PathBuf,
    jobs: Receiver<Job>,
    frames: Sender<Result<PreviewFrame, MediaError>>,
    cancelled: Arc<AtomicBool>,
) {
    while let Ok(mut job) = jobs.recv() {
        // Collapse adjacent seeks to the latest position, but never discard a
        // playback command.
        if matches!(job, Job::Frame(_)) {
            while let Ok(newer) = jobs.try_recv() {
                let is_play = matches!(newer, Job::Play { .. });
                job = newer;
                if is_play {
                    break;
                }
            }
        }
        match job {
            Job::Frame(request) => decode_frame(&tools, &path, request, &frames, &cancelled),
            Job::Play {
                start,
                fps,
                size,
                stop,
            } => decode_playback(&tools, &path, start, fps, size, &frames, &stop),
        }
    }
}

fn scale_filter(size: PreviewSize) -> String {
    format!(
        "scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2",
        size.width, size.height, size.width, size.height
    )
}

fn decode_frame(
    tools: &FfmpegTools,
    path: &PathBuf,
    req: PreviewRequest,
    frames: &Sender<Result<PreviewFrame, MediaError>>,
    cancelled: &Arc<AtomicBool>,
) {
    let mut child = match Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{:.6}", req.timestamp))
        .args(["-i"])
        .arg(path)
        .args(["-an", "-frames:v", "1", "-vf"])
        .arg(scale_filter(req.size))
        .args(["-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = frames.send(Err(MediaError::Io(error)));
            return;
        }
    };
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (read_tx, read_rx) = crossbeam_channel::bounded(1);
    let (error_tx, error_rx) = crossbeam_channel::bounded(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::io::BufReader::new(stdout)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = read_tx.send(result);
    });
    thread::spawn(move || {
        let mut error = String::new();
        let _ = std::io::BufReader::new(stderr).read_to_string(&mut error);
        let _ = error_tx.send(error);
    });
    let bytes = loop {
        if cancelled.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        match read_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(value) => break Some(value),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => break None,
        }
    };
    let status = child.wait().ok();
    if let Some(Ok(bytes)) = bytes {
        if status.is_some_and(|status| status.success()) {
            let expected = req.size.width as usize * req.size.height as usize * 4;
            if bytes.len() >= expected {
                let _ = frames.send(Ok(PreviewFrame {
                    timestamp: req.timestamp,
                    width: req.size.width,
                    height: req.size.height,
                    rgba: bytes[..expected].to_vec(),
                }));
            } else {
                let _ = frames.send(Err(MediaError::Process(
                    "FFmpeg returned an incomplete preview frame".into(),
                )));
            }
        } else if !cancelled.load(Ordering::Acquire) {
            let error = error_rx
                .recv_timeout(Duration::from_millis(100))
                .unwrap_or_else(|_| "FFmpeg could not decode preview frame".into());
            let _ = frames.send(Err(MediaError::Process(error)));
        }
    }
    cancelled.store(false, Ordering::Release);
}

#[allow(clippy::too_many_arguments)]
fn decode_playback(
    tools: &FfmpegTools,
    path: &PathBuf,
    start: f64,
    fps: f64,
    size: PreviewSize,
    frames: &Sender<Result<PreviewFrame, MediaError>>,
    stop: &Arc<AtomicBool>,
) {
    let filter = format!("fps={fps:.6},{}", scale_filter(size));
    let mut child = match Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-ss"])
        .arg(format!("{start:.6}"))
        .args(["-i"])
        .arg(path)
        .args(["-an", "-vf"])
        .arg(filter)
        .args(["-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            let _ = frames.send(Err(MediaError::Io(error)));
            stop.store(true, Ordering::Release);
            return;
        }
    };
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (error_tx, error_rx) = crossbeam_channel::bounded(1);
    thread::spawn(move || {
        let mut error = String::new();
        let _ = stderr.read_to_string(&mut error);
        let _ = error_tx.send(error);
    });
    let frame_bytes = size.width as usize * size.height as usize * 4;
    let interval = Duration::from_secs_f64(1.0 / fps);
    let clock = Instant::now();
    let mut index = 0u64;
    loop {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let mut rgba = vec![0; frame_bytes];
        match stdout.read_exact(&mut rgba) {
            Ok(()) => {
                let timestamp = start + index as f64 / fps;
                let frame = Ok(PreviewFrame {
                    timestamp,
                    width: size.width,
                    height: size.height,
                    rgba,
                });
                match frames.try_send(frame) {
                    Ok(()) | Err(crossbeam_channel::TrySendError::Full(_)) => {}
                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => break,
                }
                index += 1;
                let deadline = clock + interval.mul_f64(index as f64);
                if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                    thread::sleep(wait);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(error) => {
                let _ = frames.send(Err(MediaError::Io(error)));
                break;
            }
        }
    }
    let _ = child.kill();
    let status = child.wait();
    if !stop.load(Ordering::Acquire)
        && status.is_ok_and(|status| !status.success())
        && let Ok(error) = error_rx.recv_timeout(Duration::from_millis(100))
        && !error.trim().is_empty()
    {
        let _ = frames.send(Err(MediaError::Process(error)));
    }
    stop.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FfmpegConfig, discover};
    use std::path::Path;

    #[test]
    #[ignore = "decodes the supplied 4K recording with FFmpeg"]
    fn continuous_preview_delivers_source_rate_frames() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let video = root.join("VID_20260830_124108_00_017.mp4");
        if !video.exists() {
            return;
        }
        let tools = discover(&FfmpegConfig::default()).unwrap();
        let preview = PreviewWorker::spawn(tools, video);
        let playback = preview.play(0.0, 30.0, PreviewSize::new(640, 360)).unwrap();
        let started = Instant::now();
        let frames = (0..12)
            .map(|_| preview.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect::<Vec<_>>();
        playback.pause();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!((frames[0].width, frames[0].height), (640, 360));
        assert!((frames[11].timestamp - frames[0].timestamp - 11.0 / 30.0).abs() < 1e-6);
    }
}
