//! Timestamp-directed, multi-view preview decoding.
//!
//! Unlike [`crate::preview`], this backend keeps one FFmpeg decoder alive and
//! advances it as analysis requests move forward.  It is intentionally small:
//! one handle owns one decoder, so separate handles can decode separate laps
//! (or the same file) independently.

use crate::{FfmpegTools, MediaError, PreviewFrame, PreviewSize};
use crossbeam_channel::{Receiver, Sender};
use std::{
    collections::VecDeque,
    io::{BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

/// Configuration for the analysis decoder.  `output_fps` is capped at 30;
/// when `source_fps` is supplied, the lower rate is used.
#[derive(Debug, Clone, Copy)]
pub struct AnalysisPreviewConfig {
    pub output_fps: f64,
    pub source_fps: Option<f64>,
}

impl Default for AnalysisPreviewConfig {
    fn default() -> Self {
        Self {
            output_fps: 30.0,
            source_fps: None,
        }
    }
}

impl AnalysisPreviewConfig {
    fn fps(self) -> f64 {
        let requested = if self.output_fps.is_finite() {
            self.output_fps.clamp(1.0, 30.0)
        } else {
            30.0
        };
        self.source_fps
            .filter(|v| v.is_finite() && *v > 0.0)
            .map_or(requested, |v| requested.min(v))
    }
}

enum Job {
    Request {
        timestamp: f64,
        size: PreviewSize,
        generation: u64,
    },
}

/// A persistent, absolute-timestamp preview handle.
type FrameQueue = Arc<Mutex<VecDeque<(u64, Result<PreviewFrame, MediaError>)>>>;
pub struct AnalysisPreview {
    jobs: Sender<Job>,
    frames: FrameQueue,
    cancelled: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    last_request: Mutex<Option<(f64, PreviewSize)>>,
}

/// Alias emphasizing that requests are in exported-video (absolute) time.
pub type AbsolutePreview = AnalysisPreview;

impl AnalysisPreview {
    pub fn spawn(tools: FfmpegTools, path: impl Into<PathBuf>) -> Self {
        Self::spawn_with_config(tools, path, AnalysisPreviewConfig::default())
    }

    pub fn spawn_with_config(
        tools: FfmpegTools,
        path: impl Into<PathBuf>,
        config: AnalysisPreviewConfig,
    ) -> Self {
        let path = path.into();
        let (jobs, job_rx) = crossbeam_channel::unbounded();
        let frames = Arc::new(Mutex::new(VecDeque::with_capacity(3)));
        let cancelled = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let c = cancelled.clone();
        let g = generation.clone();
        let closed_worker = closed.clone();
        let frames_worker = frames.clone();
        thread::Builder::new()
            .name("overlay-analysis-preview".into())
            .spawn(move || {
                worker(
                    tools,
                    path,
                    config,
                    job_rx,
                    frames_worker,
                    c,
                    g,
                    closed_worker,
                )
            })
            .expect("analysis preview worker thread");
        Self {
            jobs,
            frames,
            cancelled,
            generation,
            closed,
            last_request: Mutex::new(None),
        }
    }

    pub fn request_absolute(&self, timestamp: f64, size: PreviewSize) -> Result<(), MediaError> {
        if !timestamp.is_finite()
            || timestamp < 0.0
            || size.width == 0
            || size.height == 0
            || u64::from(size.width) * u64::from(size.height) > 16_000_000
            || self.closed.load(Ordering::Acquire)
        {
            return Err(MediaError::Invalid(
                "invalid analysis preview request".into(),
            ));
        }
        self.cancelled.store(false, Ordering::Release);
        // A generation identifies a seek/resize, not every playback tick.
        // Continuous playback can consume slightly late frames without ever
        // accepting a frame belonging to an obsolete seek.
        let mut last = self
            .last_request
            .lock()
            .map_err(|_| MediaError::Process("preview request lock poisoned".into()))?;
        if last.is_none_or(|(t, s)| size != s || timestamp < t - 0.1 || timestamp > t + 1.0) {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        *last = Some((timestamp, size));
        let generation = self.generation.load(Ordering::Acquire);
        self.jobs
            .send(Job::Request {
                timestamp,
                size,
                generation,
            })
            .map_err(|_| MediaError::Process("analysis preview worker stopped".into()))
    }

    pub fn request(&self, timestamp: f64, size: PreviewSize) -> Result<(), MediaError> {
        self.request_absolute(timestamp, size)
    }

    pub fn try_recv(&self) -> Option<Result<PreviewFrame, MediaError>> {
        let current = self.generation.load(Ordering::Acquire);
        let mut queue = self.frames.lock().ok()?;
        while let Some((generation, frame)) = queue.pop_front() {
            if generation == current {
                return Some(frame);
            }
        }
        None
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        self.cancel();
    }
}

impl Drop for AnalysisPreview {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct Decoder {
    rx: Option<Receiver<Result<Vec<u8>, MediaError>>>,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    reader: Option<JoinHandle<()>>,
}

impl Decoder {
    fn start(
        tools: &FfmpegTools,
        path: &PathBuf,
        size: PreviewSize,
        fps: f64,
        start: f64,
    ) -> Result<Self, MediaError> {
        let filter = format!(
            "fps={fps:.6},scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2",
            size.width, size.height, size.width, size.height
        );
        let mut child = Command::new(&tools.ffmpeg)
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
            .map_err(MediaError::Io)?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let child_ref = Arc::new(Mutex::new(Some(child)));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = crossbeam_channel::bounded(2);
        let diagnostic_tx = tx.clone();
        let diagnostic_stop = stop.clone();
        let stop_ref = stop.clone();
        let bytes = size.width as usize * size.height as usize * 4;
        let reader = thread::spawn(move || {
            let mut input = BufReader::new(stdout);
            let mut buf = vec![0; bytes];
            loop {
                if stop_ref.load(Ordering::Acquire) {
                    break;
                }
                match input.read_exact(&mut buf) {
                    Ok(()) => loop {
                        match tx.send_timeout(Ok(buf.clone()), Duration::from_millis(20)) {
                            Ok(()) => break,
                            Err(crossbeam_channel::SendTimeoutError::Timeout(_))
                                if !stop_ref.load(Ordering::Acquire) =>
                            {
                                continue;
                            }
                            Err(_) => return,
                        }
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(e) => {
                        let _ = tx.send(Err(MediaError::Io(e)));
                        break;
                    }
                }
            }
            // The controller remains the sole child owner. A reader must not
            // steal it and wait while FFmpeg is blocked on a full output pipe.
        });
        // Always drain stderr so FFmpeg cannot block on diagnostics.
        thread::spawn(move || {
            let mut input = BufReader::new(stderr);
            let mut chunk = [0; 4096];
            let mut diagnostic = Vec::new();
            while let Ok(n) = input.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let keep = n.min(8192_usize.saturating_sub(diagnostic.len()));
                diagnostic.extend_from_slice(&chunk[..keep]);
            }
            if !diagnostic.is_empty() && !diagnostic_stop.load(Ordering::Acquire) {
                let _ = diagnostic_tx.try_send(Err(MediaError::Process(
                    String::from_utf8_lossy(&diagnostic).into_owned(),
                )));
            }
        });
        Ok(Self {
            rx: Some(rx),
            stop,
            child: child_ref,
            reader: Some(reader),
        })
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Disconnect the receiver first so a reader blocked on a full queue
        // can leave immediately.
        self.rx.take();
        let child = self.child.lock().ok().and_then(|mut guard| guard.take());
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn send_latest(frames: &FrameQueue, generation: u64, frame: Result<PreviewFrame, MediaError>) {
    if let Ok(mut queue) = frames.lock() {
        if queue.len() >= 3 {
            queue.pop_front();
        }
        queue.push_back((generation, frame));
    }
}

#[allow(clippy::too_many_arguments)]
fn worker(
    tools: FfmpegTools,
    path: PathBuf,
    config: AnalysisPreviewConfig,
    jobs: Receiver<Job>,
    frames: FrameQueue,
    cancelled: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
) {
    let fps = config.fps();
    let interval = 1.0 / fps;
    let mut decoder: Option<Decoder> = None;
    let mut size = None;
    let mut next = 0u64;
    let mut last_target = None;
    let mut buffer: VecDeque<(u64, Vec<u8>)> = VecDeque::with_capacity(8);
    while !closed.load(Ordering::Acquire) {
        let mut job = match jobs.recv_timeout(Duration::from_millis(50)) {
            Ok(job) => job,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if cancelled.load(Ordering::Acquire) {
                    decoder.take();
                    buffer.clear();
                    next = 0;
                }
                continue;
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        };
        while let Ok(newer) = jobs.try_recv() {
            job = newer;
        }
        let Job::Request {
            mut timestamp,
            size: mut wanted,
            generation: mut request_generation,
        } = job;
        if cancelled.load(Ordering::Acquire) {
            continue;
        }
        if size != Some(wanted)
            || last_target.is_some_and(|previous| timestamp + interval < previous)
            || timestamp > next as f64 * interval + 2.0
        {
            decoder.take();
            buffer.clear();
            next = 0;
        }
        size = Some(wanted);
        if decoder.is_none() {
            match Decoder::start(&tools, &path, wanted, fps, timestamp) {
                Ok(d) => {
                    next = (timestamp / interval).round().max(0.0) as u64;
                    decoder = Some(d)
                }
                Err(e) => {
                    send_latest(&frames, request_generation, Err(e));
                    continue;
                }
            }
        }
        loop {
            if closed.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire) {
                break;
            }
            while let Ok(newer) = jobs.try_recv() {
                let Job::Request {
                    timestamp: t,
                    size: s,
                    generation: g,
                } = newer;
                timestamp = t;
                wanted = s;
                request_generation = g;
                if s != size.unwrap()
                    || last_target.is_some_and(|previous| t + interval < previous)
                    || t > next as f64 * interval + 2.0
                {
                    decoder.take();
                    buffer.clear();
                    size = Some(s);
                    next = (t / interval).round().max(0.0) as u64;
                    decoder = Decoder::start(&tools, &path, s, fps, t).ok();
                }
            }
            if decoder.is_none() {
                break;
            }
            let target_index = (timestamp / interval).round().max(0.0) as u64;
            if next > target_index + 1
                || (!buffer.is_empty() && buffer.back().unwrap().0 >= target_index)
            {
                break;
            }
            match decoder
                .as_ref()
                .unwrap()
                .rx
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(20))
            {
                Ok(Ok(bytes)) => {
                    buffer.push_back((next, bytes));
                    next += 1;
                    while buffer.len() > 8 {
                        buffer.pop_front();
                    }
                }
                Ok(Err(e)) => {
                    send_latest(&frames, request_generation, Err(e));
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
        last_target = Some(timestamp);
        if request_generation != generation.load(Ordering::Acquire)
            || cancelled.load(Ordering::Acquire)
        {
            continue;
        }
        if let Some((index, rgba)) = buffer
            .iter()
            .min_by_key(|(i, _)| (*i as i64 - (timestamp / interval).round() as i64).unsigned_abs())
            .cloned()
        {
            send_latest(
                &frames,
                request_generation,
                Ok(PreviewFrame {
                    timestamp: index as f64 * interval,
                    width: wanted.width,
                    height: wanted.height,
                    rgba,
                }),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FfmpegConfig, discover};
    use std::process::Command;
    use std::thread;
    use tempfile::tempdir;
    #[test]
    fn fps_is_capped_and_respects_source() {
        assert_eq!(
            AnalysisPreviewConfig {
                output_fps: 60.0,
                source_fps: Some(24.0)
            }
            .fps(),
            24.0
        );
    }
    #[test]
    fn fps_defaults_to_thirty() {
        assert_eq!(AnalysisPreviewConfig::default().fps(), 30.0);
    }

    #[test]
    #[ignore = "requires a functioning local FFmpeg installation"]
    fn generated_clip_supports_late_and_backward_requests() {
        let tools = match discover(&FfmpegConfig::default()) {
            Ok(tools) => tools,
            Err(_) => return,
        };
        let dir = tempdir().unwrap();
        let clip = dir.path().join("analysis-preview.mp4");
        let status = Command::new(&tools.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=32x24:rate=10",
                "-t",
                "6",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .status()
            .unwrap();
        if !status.success() {
            return;
        }
        let preview = AnalysisPreview::spawn_with_config(
            tools.clone(),
            &clip,
            AnalysisPreviewConfig {
                output_fps: 10.0,
                source_fps: None,
            },
        );
        let wait_frame = |handle: &AnalysisPreview, target: f64| {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(frame) = handle.try_recv() {
                    let frame = frame.unwrap();
                    assert!(
                        (frame.timestamp - target).abs() <= 0.11,
                        "{} vs {target}",
                        frame.timestamp
                    );
                    break frame;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "no frame for {target}"
                );
                thread::sleep(Duration::from_millis(5));
            }
        };
        for t in [5.8, 0.2, 0.2, 4.5, 4.6, 4.7] {
            preview.request(t, PreviewSize::new(16, 16)).unwrap();
            let frame = wait_frame(&preview, t);
            assert_eq!(frame.rgba.len(), 16 * 16 * 4);
        }
        let other = AnalysisPreview::spawn_with_config(
            tools.clone(),
            &clip,
            AnalysisPreviewConfig {
                output_fps: 10.0,
                source_fps: None,
            },
        );
        other.request(2.3, PreviewSize::new(16, 16)).unwrap();
        preview.request(5.0, PreviewSize::new(16, 16)).unwrap();
        wait_frame(&other, 2.3);
        wait_frame(&preview, 5.0);
        preview.cancel();
        preview.shutdown();
        assert!(preview.request(0.0, PreviewSize::new(16, 16)).is_err());
        // Force the read-ahead queue full, then prove destructor cancellation
        // does not hang while joining its blocked reader.
        let decoder = Decoder::start(&tools, &clip, PreviewSize::new(16, 16), 10.0, 0.0).unwrap();
        thread::sleep(Duration::from_millis(100));
        let (tx, rx) = crossbeam_channel::bounded(1);
        thread::spawn(move || {
            drop(decoder);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(3))
            .expect("full-buffer decoder drop deadlocked");
    }

    #[test]
    #[ignore = "benchmarks two/four viewers using the supplied Studio export and local FFmpeg"]
    fn supplied_video_multi_viewer_throughput() {
        let tools = discover(&FfmpegConfig::default()).unwrap();
        let clip =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../VID_20260830_124108_00_017.mp4");
        assert!(clip.exists());
        for count in [2, 4] {
            let viewers = (0..count)
                .map(|_| AnalysisPreview::spawn(tools.clone(), &clip))
                .collect::<Vec<_>>();
            let mut times = vec![f64::NEG_INFINITY; count];
            let mut measured = std::time::Instant::now();
            for frame_index in 0..=90 {
                if frame_index == 1 {
                    measured = std::time::Instant::now();
                }
                let targets = (0..count)
                    .map(|i| 5.0 + i as f64 * 3.0 + frame_index as f64 / 30.0)
                    .collect::<Vec<_>>();
                for (viewer, t) in viewers.iter().zip(&targets) {
                    viewer.request(*t, PreviewSize::new(640, 360)).unwrap();
                }
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                loop {
                    for (i, viewer) in viewers.iter().enumerate() {
                        while let Some(frame) = viewer.try_recv() {
                            times[i] = frame.unwrap().timestamp;
                        }
                    }
                    if times
                        .iter()
                        .zip(&targets)
                        .all(|(a, b)| (a - b).abs() < 0.018)
                    {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "stalled viewers: {times:?}, expected {targets:?}"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
            }
            eprintln!(
                "{count} independent 640×360 viewers: {:.1} complete multi-view updates/s (90 frames after warmup)",
                90.0 / measured.elapsed().as_secs_f64()
            );
        }
    }
}
