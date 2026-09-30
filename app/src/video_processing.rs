//! Shared application wiring for raw-video processing; algorithms live in the libraries.
use overlay_core::{
    MotionTrajectory, Quaternion, RawCameraVideo, VideoProcessingConfig, read_insta360_video,
};
use overlay_media::{
    AnalysisPreview, AnalysisPreviewConfig, DualVideoInfo, FfmpegTools, LensFramePair, MediaError,
    PreviewFrame, PreviewHandle, PreviewPlayback, PreviewSize, PreviewWorker, ProcessedPreview,
    VideoFrameProcessor,
};
use overlay_render::video::{LensImages, VideoProjector};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

pub fn raw_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("insv"))
}
pub fn default_processing(path: &Path) -> Option<VideoProcessingConfig> {
    raw_path(path).then(VideoProcessingConfig::default)
}
#[derive(Clone)]
struct Settings {
    config: VideoProcessingConfig,
    revision: u64,
}
type SharedSettings = Arc<Mutex<Settings>>;
pub struct RawProcessor {
    raw: RawCameraVideo,
    projector: VideoProjector,
    settings: SharedSettings,
    trajectory: Option<(f64, MotionTrajectory)>,
}
impl RawProcessor {
    fn new(path: &Path, settings: SharedSettings) -> Result<Self, MediaError> {
        let raw = read_insta360_video(path).map_err(MediaError::Invalid)?;
        Ok(Self {
            raw,
            projector: VideoProjector::new(),
            settings,
            trajectory: None,
        })
    }
    pub fn for_export(
        path: &Path,
        config: VideoProcessingConfig,
        info: &DualVideoInfo,
    ) -> Result<Self, MediaError> {
        config.validate().map_err(MediaError::Invalid)?;
        let mut processor = Self::new(
            path,
            Arc::new(Mutex::new(Settings {
                config: config.clone(),
                revision: 0,
            })),
        )?;
        if config.stabilization.enabled {
            processor.prepare_motion(&config)?;
            let last = ((info.duration * info.fps.as_f64()).ceil() - 1.) / info.fps.as_f64();
            if !processor.trajectory.as_ref().unwrap().1.covers(0., last) {
                return Err(MediaError::Invalid("Stabilized export requires gyro coverage over the complete video; disable stabilization or resolve the missing data".into()));
            }
        }
        Ok(processor)
    }
    fn prepare_motion(&mut self, config: &VideoProcessingConfig) -> Result<(), MediaError> {
        if let Some(error) = &self.raw.motion_error {
            return Err(MediaError::Invalid(format!(
                "Stabilization unavailable: {error}"
            )));
        }
        let sigma = config.stabilization.sigma_seconds;
        if self.trajectory.as_ref().is_none_or(|(s, _)| *s != sigma) {
            self.trajectory = Some((
                sigma,
                self.raw
                    .motion
                    .prepare(sigma)
                    .map_err(MediaError::Invalid)?,
            ));
        }
        Ok(())
    }
}
impl VideoFrameProcessor for RawProcessor {
    fn description(&self) -> String {
        if self.projector.backend() == "CPU" {
            "CPU projection (GPU unavailable)".into()
        } else {
            String::new()
        }
    }
    fn process(&mut self, frame: &LensFramePair, size: PreviewSize) -> Result<Vec<u8>, MediaError> {
        let config = self.settings.lock().unwrap().config.clone();
        config.validate().map_err(MediaError::Invalid)?;
        let correction = if config.stabilization.enabled {
            self.prepare_motion(&config)?;
            self.trajectory
                .as_ref()
                .unwrap()
                .1
                .correction(frame.timestamp)
                .ok_or_else(|| {
                    MediaError::Invalid(
                        "Stabilization unavailable: no valid camera gyro coverage at this time"
                            .into(),
                    )
                })?
        } else {
            Quaternion::IDENTITY
        };
        self.projector
            .render(
                &LensImages {
                    width: frame.width,
                    height: frame.height,
                    pixels: [&frame.pixels[0], &frame.pixels[1]],
                    timestamp: frame.timestamp,
                },
                &self.raw.calibration,
                &config,
                correction,
                (size.width, size.height),
            )
            .map_err(MediaError::Invalid)
    }
}

enum Backend {
    Analysis(AnalysisPreview),
    Overlay(PreviewHandle),
    Raw {
        preview: ProcessedPreview,
        settings: SharedSettings,
        clock: Mutex<Option<Clock>>,
        coverage: Arc<Mutex<Option<(f64, f64)>>>,
    },
}
struct Clock {
    start: f64,
    at: Instant,
    stop: Arc<AtomicBool>,
    size: PreviewSize,
}
pub struct VideoPreview {
    backend: Backend,
}
pub enum VideoPlayback {
    Flat(PreviewPlayback),
    Raw(Arc<AtomicBool>),
}
impl VideoPlayback {
    pub fn pause(&self) {
        match self {
            Self::Flat(p) => p.pause(),
            Self::Raw(p) => p.store(true, Ordering::Release),
        }
    }
}
impl Drop for VideoPlayback {
    fn drop(&mut self) {
        self.pause();
    }
}
impl VideoPreview {
    pub fn spawn(
        tools: FfmpegTools,
        path: PathBuf,
        config: Option<VideoProcessingConfig>,
        source_fps: Option<f64>,
        overlay: bool,
    ) -> Self {
        let backend = if let Some(config) = config.or_else(|| default_processing(&path)) {
            let settings = Arc::new(Mutex::new(Settings {
                config,
                revision: 0,
            }));
            let shared = settings.clone();
            let file = path.clone();
            let coverage = Arc::new(Mutex::new(None));
            let worker_coverage = coverage.clone();
            let preview = ProcessedPreview::spawn(tools, path, move |info| {
                *worker_coverage.lock().unwrap() = Some((
                    info.duration,
                    ((info.duration * info.fps.as_f64()).ceil() - 1.) / info.fps.as_f64(),
                ));
                RawProcessor::new(&file, shared)
                    .map(|p| Box::new(p) as Box<dyn VideoFrameProcessor>)
            });
            Backend::Raw {
                preview,
                settings,
                clock: Mutex::new(None),
                coverage,
            }
        } else if overlay {
            Backend::Overlay(PreviewWorker::spawn(tools, path))
        } else {
            Backend::Analysis(AnalysisPreview::spawn_with_config(
                tools,
                path,
                AnalysisPreviewConfig {
                    output_fps: 30.,
                    source_fps,
                },
            ))
        };
        Self { backend }
    }
    pub fn set_config(&self, config: &VideoProcessingConfig) {
        if let Backend::Raw { settings, .. } = &self.backend {
            let mut s = settings.lock().unwrap();
            if s.config != *config {
                s.config = config.clone();
                s.revision = s.revision.wrapping_add(1);
            }
        }
    }
    pub fn request(&self, time: f64, size: PreviewSize) -> Result<(), MediaError> {
        match &self.backend {
            Backend::Analysis(p) => p.request(time, size),
            Backend::Overlay(p) => p.request(time, size),
            Backend::Raw {
                preview, settings, ..
            } => preview.request(time, size, settings.lock().unwrap().revision),
        }
    }
    pub fn play(
        &self,
        start: f64,
        fps: f64,
        size: PreviewSize,
    ) -> Result<VideoPlayback, MediaError> {
        match &self.backend {
            Backend::Overlay(p) => p.play(start, fps, size).map(VideoPlayback::Flat),
            Backend::Raw { clock, .. } => {
                let stop = Arc::new(AtomicBool::new(false));
                *clock.lock().unwrap() = Some(Clock {
                    start,
                    at: Instant::now(),
                    stop: stop.clone(),
                    size,
                });
                self.request(start, size)?;
                Ok(VideoPlayback::Raw(stop))
            }
            Backend::Analysis(_) => Err(MediaError::Invalid(
                "Analysis playback uses its linked clock".into(),
            )),
        }
    }
    pub fn try_recv(&self) -> Option<Result<PreviewFrame, MediaError>> {
        match &self.backend {
            Backend::Analysis(p) => p.try_recv(),
            Backend::Overlay(p) => p.try_recv(),
            Backend::Raw {
                preview,
                clock,
                coverage,
                ..
            } => {
                if let Some(clock) = clock.lock().unwrap().as_ref()
                    && !clock.stop.load(Ordering::Acquire)
                {
                    let mut time = clock.start + clock.at.elapsed().as_secs_f64();
                    if let Some((end, last)) = *coverage.lock().unwrap()
                        && time >= end
                    {
                        clock.stop.store(true, Ordering::Release);
                        time = last;
                    }
                    let _ = self.request(time, clock.size);
                }
                preview.try_recv()
            }
        }
    }
    pub fn playback_finished(&self) -> bool {
        match &self.backend {
            Backend::Raw { clock, .. } => clock
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|c| c.stop.load(Ordering::Acquire)),
            _ => false,
        }
    }
    pub fn status(&self) -> String {
        match &self.backend {
            Backend::Raw { preview, .. } => preview.status(),
            _ => String::new(),
        }
    }
}

pub fn controls(ui: &mut egui::Ui, config: &mut VideoProcessingConfig) -> bool {
    let before = config.clone();
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::Slider::new(&mut config.horizontal_fov_degrees, 30.0..=150.0).text("FOV °"));
        ui.add(
            egui::DragValue::new(&mut config.view.roll)
                .speed(0.25)
                .range(-180.0..=180.0)
                .prefix("Roll ° "),
        );
        if ui.button("Reset view").clicked() {
            config.reset_view();
        }
        egui::ComboBox::from_id_salt("video-seam")
            .selected_text(match config.seam.as_str() {
                "hard_cut" => "Hard cut",
                "feather" => "Feather",
                _ => "Advanced (unavailable)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut config.seam, "hard_cut".into(), "Hard cut");
                ui.selectable_value(&mut config.seam, "feather".into(), "Feather");
            });
        if config.seam == "feather" {
            ui.add(
                egui::DragValue::new(&mut config.feather_degrees)
                    .range(0.0..=20.0)
                    .speed(0.1)
                    .suffix("° blend"),
            );
        }
        ui.checkbox(&mut config.stabilization.enabled, "Stabilize");
        if config.stabilization.enabled {
            ui.add(
                egui::Slider::new(&mut config.stabilization.sigma_seconds, 0.05..=1.0)
                    .text("Smoothing s"),
            );
        }
        egui::ComboBox::from_id_salt("video-output")
            .selected_text(format!("Export {}p", config.output_height))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut config.output_height, 1080, "1080p");
                ui.selectable_value(&mut config.output_height, 2160, "2160p");
            });
    });
    *config != before
}
pub fn gestures(
    ui: &mut egui::Ui,
    response: &egui::Response,
    config: &mut VideoProcessingConfig,
) -> bool {
    let before = config.clone();
    if response.dragged_by(egui::PointerButton::Primary) {
        let delta = ui.input(|i| i.pointer.delta());
        config.drag(delta.x as f64, delta.y as f64, response.rect.width() as f64);
    }
    if response.hovered() {
        let scroll = ui.input_mut(|i| {
            let scroll = i.smooth_scroll_delta.y;
            i.smooth_scroll_delta = egui::Vec2::ZERO;
            scroll
        });
        if scroll != 0. {
            config.scroll(scroll as f64);
        }
    }
    *config != before
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn video_gestures_consume_scroll_only_over_image() {
        let context = egui::Context::default();
        let mut config = VideoProcessingConfig::default();
        let mut rect = egui::Rect::NOTHING;
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            let (r, _) = ui.allocate_exact_size(egui::vec2(300., 150.), egui::Sense::drag());
            rect = r;
        });
        output.textures_delta.clear();
        let input = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., 100.),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            let (_, response) = ui.allocate_exact_size(egui::vec2(300., 150.), egui::Sense::drag());
            assert!(gestures(ui, &response, &mut config));
            assert_eq!(ui.input(|i| i.smooth_scroll_delta), egui::Vec2::ZERO);
        });
        output.textures_delta.clear();
        assert!(config.horizontal_fov_degrees < 90.);
        assert_eq!(config.view, Default::default());
        let before = config.clone();
        let input = egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(rect.right_bottom() + egui::vec2(20., 20.)),
                egui::Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., 100.),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            let (_, response) = ui.allocate_exact_size(egui::vec2(300., 150.), egui::Sense::drag());
            assert!(!gestures(ui, &response, &mut config));
            assert!(ui.input(|i| i.smooth_scroll_delta.y) > 0.);
        });
        output.textures_delta.clear();
        assert_eq!(config, before);
    }
    #[test]
    #[ignore = "requires supplied X4 Air footage, FFmpeg and a GPU; exports a temporary one-second clip"]
    fn supplied_raw_video_exports_stabilized_cross_lens_view() {
        use std::io::{Read, Seek, SeekFrom, Write};
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../VID_20260830_124108_00_017.insv");
        let tools = overlay_media::discover(&overlay_media::FfmpegConfig::default()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("raw.insv");
        let output = dir.path().join("projected.mp4");
        let command = std::process::Command::new(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&source)
            .args([
                "-t", "1.001", "-map", "0:v", "-map", "0:a", "-c", "copy", "-f", "mp4",
            ])
            .arg(&input)
            .output()
            .unwrap();
        assert!(
            command.status.success(),
            "{}",
            String::from_utf8_lossy(&command.stderr)
        );
        let mut original = std::fs::File::open(&source).unwrap();
        original.seek(SeekFrom::End(-40)).unwrap();
        let mut size = [0; 4];
        original.read_exact(&mut size).unwrap();
        let size = u32::from_le_bytes(size) as usize;
        original.seek(SeekFrom::End(-(size as i64))).unwrap();
        let mut trailer = vec![0; size];
        original.read_exact(&mut trailer).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&input)
            .unwrap()
            .write_all(&trailer)
            .unwrap();
        let info = overlay_media::probe_dual_video(&tools, &input).unwrap();
        let mut config = VideoProcessingConfig::default();
        config.view.yaw = -90.;
        config.stabilization.enabled = true;
        config.seam = "feather".into();
        let mut processor = RawProcessor::for_export(&input, config, &info).unwrap();
        assert_eq!(processor.projector.backend(), "wgpu");
        overlay_media::export_processed_video(
            &tools,
            &input,
            &output,
            &overlay_media::ExportSettings {
                encoder: Some("libx264".into()),
                quality: Some(20),
                ..Default::default()
            },
            PreviewSize::new(1920, 1080),
            &mut processor,
            &overlay_media::CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let mut decoder =
            overlay_media::DualVideoDecoder::start(&tools, &input, &info, 0., None).unwrap();
        let frame = decoder.next(&AtomicBool::new(false)).unwrap().unwrap();
        let pixels = processor
            .process(&frame, PreviewSize::new(3840, 2160))
            .unwrap();
        assert_eq!(pixels.len(), 3840 * 2160 * 4);
        let metadata = overlay_media::probe_video(&tools, &output).unwrap();
        assert_eq!((metadata.width, metadata.height), (1920, 1080));
        let range = std::process::Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=color_range",
                "-of",
                "json",
            ])
            .arg(&output)
            .output()
            .unwrap();
        let range: serde_json::Value = serde_json::from_slice(&range.stdout).unwrap();
        assert_eq!(range["streams"][0]["color_range"], "pc");
        assert!((metadata.fps().unwrap() - 30000. / 1001.).abs() < 1e-9);
    }
}
