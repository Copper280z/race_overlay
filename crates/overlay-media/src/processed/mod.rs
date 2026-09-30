//! Persistent paired-track decoding with caller-owned frame processing.
mod decoder;
mod export;
use crate::{FfmpegTools, MediaError, PreviewFrame, PreviewSize};
pub use decoder::{DualVideoDecoder, DualVideoInfo, LensFramePair, probe_dual_video};
pub use export::export_processed_video;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

pub trait VideoFrameProcessor: Send {
    fn description(&self) -> String {
        String::new()
    }
    fn process(&mut self, frame: &LensFramePair, size: PreviewSize) -> Result<Vec<u8>, MediaError>;
}
impl<F> VideoFrameProcessor for F
where
    F: FnMut(&LensFramePair, PreviewSize) -> Result<Vec<u8>, MediaError> + Send,
{
    fn process(&mut self, frame: &LensFramePair, size: PreviewSize) -> Result<Vec<u8>, MediaError> {
        self(frame, size)
    }
}
#[derive(Clone, Copy)]
struct Request {
    time: f64,
    size: PreviewSize,
    generation: u64,
}
type ResultSlot = Arc<Mutex<Option<(u64, Result<PreviewFrame, MediaError>)>>>;
pub struct ProcessedPreview {
    latest: Arc<Mutex<Option<Request>>>,
    signal: crossbeam_channel::Sender<()>,
    frames: ResultSlot,
    generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    last: Mutex<Option<(f64, PreviewSize, u64)>>,
    status: Arc<Mutex<String>>,
}
impl ProcessedPreview {
    pub fn spawn<F>(tools: FfmpegTools, path: PathBuf, create: F) -> Self
    where
        F: FnOnce(&DualVideoInfo) -> Result<Box<dyn VideoFrameProcessor>, MediaError>
            + Send
            + 'static,
    {
        let latest = Arc::new(Mutex::new(None::<Request>));
        let (signal, rx) = crossbeam_channel::bounded(1);
        let frames: ResultSlot = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new("Preparing raw video…".into()));
        let (requests, output, closed, version, message) = (
            latest.clone(),
            frames.clone(),
            stop.clone(),
            generation.clone(),
            status.clone(),
        );
        std::thread::spawn(move || {
            let initialized =
                probe_dual_video(&tools, &path).and_then(|info| create(&info).map(|p| (info, p)));
            let (info, mut processor) = match initialized {
                Ok(v) => v,
                Err(e) => {
                    *message.lock().unwrap() = e.to_string();
                    *output.lock().unwrap() = Some((u64::MAX, Err(e)));
                    return;
                }
            };
            *message.lock().unwrap() = processor.description();
            let mut decoder = None;
            let mut cached: Option<LensFramePair> = None;
            let mut last_target = None;
            while !closed.load(Ordering::Acquire) {
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(()) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                }
                let Some(request) = *requests.lock().unwrap() else {
                    continue;
                };
                let result = (|| {
                    if request.time >= info.duration {
                        return Err(MediaError::Invalid(
                            "No video coverage at this position".into(),
                        ));
                    }
                    if last_target.is_some_and(|t| request.time < t - 0.5 / info.fps.as_f64())
                        || cached
                            .as_ref()
                            .is_some_and(|f| request.time > f.timestamp + 1.)
                    {
                        decoder = None;
                        cached = None;
                    }
                    if decoder.is_none() {
                        decoder = Some(DualVideoDecoder::start(
                            &tools,
                            &path,
                            &info,
                            request.time,
                            Some(1024),
                        )?);
                    }
                    while cached
                        .as_ref()
                        .is_none_or(|f| f.timestamp + 0.5 / info.fps.as_f64() < request.time)
                    {
                        if closed.load(Ordering::Acquire) {
                            return Err(MediaError::Cancelled);
                        }
                        cached = decoder.as_mut().unwrap().next(&closed)?;
                        if cached.is_none() {
                            return Err(MediaError::Invalid(
                                "Video ends before this position".into(),
                            ));
                        }
                        if version.load(Ordering::Acquire) != request.generation {
                            return Err(MediaError::Cancelled);
                        }
                    }
                    last_target = Some(request.time);
                    let frame = cached.as_ref().unwrap();
                    *message.lock().unwrap() = "Preparing view and camera motion…".into();
                    let rgba = processor.process(frame, request.size)?;
                    if rgba.len() != request.size.width as usize * request.size.height as usize * 4
                    {
                        return Err(MediaError::Invalid(
                            "Video processor returned an incomplete image".into(),
                        ));
                    }
                    Ok(PreviewFrame {
                        timestamp: frame.timestamp,
                        width: request.size.width,
                        height: request.size.height,
                        rgba,
                    })
                })();
                *message.lock().unwrap() = processor.description();
                if !matches!(result, Err(MediaError::Cancelled)) {
                    *output.lock().unwrap() = Some((request.generation, result));
                }
            }
        });
        Self {
            latest,
            signal,
            frames,
            generation,
            stop,
            last: Mutex::new(None),
            status,
        }
    }
    pub fn request(&self, time: f64, size: PreviewSize, revision: u64) -> Result<(), MediaError> {
        if !time.is_finite()
            || time < 0.
            || size.width == 0
            || size.height == 0
            || u64::from(size.width) * u64::from(size.height) > 16_000_000
        {
            return Err(MediaError::Invalid(
                "Invalid processed preview request".into(),
            ));
        }
        let mut last = self.last.lock().unwrap();
        if last
            .is_none_or(|(t, s, r)| s != size || r != revision || time < t - 0.02 || time > t + 1.)
        {
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        *last = Some((time, size, revision));
        *self.latest.lock().unwrap() = Some(Request {
            time,
            size,
            generation: self.generation.load(Ordering::Acquire),
        });
        let _ = self.signal.try_send(());
        Ok(())
    }
    pub fn try_recv(&self) -> Option<Result<PreviewFrame, MediaError>> {
        let (generation, result) = self.frames.lock().unwrap().take()?;
        (generation == u64::MAX || generation == self.generation.load(Ordering::Acquire))
            .then_some(result)
    }
    pub fn status(&self) -> String {
        self.status.lock().unwrap().clone()
    }
}
impl Drop for ProcessedPreview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.signal.try_send(());
    }
}

#[cfg(test)]
mod tests;
