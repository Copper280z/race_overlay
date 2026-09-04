use crossbeam_channel::{Receiver, Sender, unbounded};
use eframe::egui;
use overlay_core::{
    AdapterRegistry, CameraCalibration, ChannelBinding, ChannelDescriptor, ChannelRef,
    CorrelationConfig, NormalizedRect, ProjectDocument, ProjectV1, Quantity, SourceConfig,
    SourceId, SyntheticConfig, TelemetryDataset, Unit, UnitSystem, WidgetConfig, WidgetId,
    accelerometer_bias_for_gravity, add_derived_inertial_channels, correlate_channel_series,
    guided_sensor_to_vehicle_from_forward_with_trim, median_gravity, zero_phase_low_pass,
};
use overlay_media::{
    AlignmentResult, CancelToken, ExportProgress, ExportSettings, FfmpegConfig, FfmpegTools,
    PreviewHandle, PreviewPlayback, PreviewSize, PreviewWorker, RgbaFrame, VideoMetadata,
    WidgetGeometry, align_audio, discover, export_video, extract_mono_pcm, probe_video,
};
use overlay_render::{
    AlignedDatasets, RenderOptions, RenderSize, prepare_project_widgets,
    render_prepared_project_widgets, render_project_widgets,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    thread,
};

const PREVIEW_W: u32 = 960;
const PREVIEW_H: u32 = 540;
const INSTA360_IMU_SAMPLE_RATE_HZ: f64 = 1_000.0;
const UNIT_SYSTEM_STORAGE_KEY: &str = "race-overlay.default-unit-system";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportCodecChoice {
    MatchSource,
    H264,
    H265,
}

impl ExportCodecChoice {
    fn label(self) -> &'static str {
        match self {
            Self::MatchSource => "Match source",
            Self::H264 => "H.264 (most compatible)",
            Self::H265 => "H.265 / HEVC (smaller files)",
        }
    }

    fn codec(self) -> Option<overlay_media::Encoder> {
        match self {
            Self::MatchSource => None,
            Self::H264 => Some(overlay_media::Encoder::H264),
            Self::H265 => Some(overlay_media::Encoder::H265),
        }
    }
}

enum WorkerEvent {
    SourceLoaded(SourceId, Result<TelemetryDataset, String>),
    Synced(SourceId, Result<AlignmentResult, String>),
    CorrelationEstimated(u64, Result<CorrelationEstimate, String>),
    ExportProgress(ExportProgress),
    ExportFinished(Result<PathBuf, String>),
}

/// An alignment candidate returned by the correlation worker. This is
/// deliberately transient: only the explicit Apply action writes the target
/// offset into the project document.
#[derive(Clone, Debug)]
struct CorrelationEstimate {
    target_source: SourceId,
    target_channel: ChannelRef,
    reference_source: SourceId,
    reference_channel: ChannelRef,
    raw_lag_seconds: f64,
    adjustment_seconds: f64,
    target_offset_seconds: f64,
    reference_offset_seconds: f64,
    target_current_offset_seconds: f64,
    coefficient: f64,
    samples: usize,
    overlap_seconds: f64,
    resolution_seconds: f64,
}

pub struct RaceOverlayApp {
    tools: Option<FfmpegTools>,
    tools_error: Option<String>,
    project: Option<ProjectDocument>,
    project_path: Option<PathBuf>,
    metadata: Option<VideoMetadata>,
    datasets: Vec<TelemetryDataset>,
    preview: Option<PreviewHandle>,
    preview_playback: Option<PreviewPlayback>,
    video_texture: Option<egui::TextureHandle>,
    overlay_texture: Option<egui::TextureHandle>,
    pending_overlay_image: Option<egui::ColorImage>,
    current_time: f64,
    playing: bool,
    selected_source: Option<usize>,
    selected_widget: Option<usize>,
    resizing_widget: bool,
    status: String,
    tx: Sender<WorkerEvent>,
    rx: Receiver<WorkerEvent>,
    export_cancel: Option<CancelToken>,
    export_dialog_open: bool,
    export_codec: ExportCodecChoice,
    export_match_bitrate: bool,
    export_bitrate_mbps: f64,
    export_apple_compatible: bool,
    export_fast_start: bool,
    export_force_yuv420p: bool,
    syncing_sources: Vec<SourceId>,
    export_fraction: f32,
    calibration_start: f64,
    calibration_end: f64,
    calibration_source_time: bool,
    calibration_auto_stationary: bool,
    calibration_forward_axis: usize,
    calibration_roll_deg: f64,
    calibration_pitch_deg: f64,
    calibration_yaw_deg: f64,
    calibration_low_pass_hz: f64,
    show_data_plot: bool,
    plot_channels: Vec<ChannelRef>,
    plot_window_seconds: f64,
    plot_filter_enabled: bool,
    plot_filter_hz: f64,
    plot_robust_scale: bool,
    remove_source_confirm: Option<SourceId>,
    unit_system: UnitSystem,
    correlation_reference_source: Option<SourceId>,
    correlation_target_channel: Option<ChannelRef>,
    correlation_reference_channel: Option<ChannelRef>,
    correlation_min_adjustment: f64,
    correlation_max_adjustment: f64,
    correlation_absolute: bool,
    correlation_result: Option<CorrelationEstimate>,
    correlation_running: bool,
    correlation_job: u64,
}

impl RaceOverlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tx, rx) = unbounded();
        let (tools, tools_error) = match discover(&FfmpegConfig::default()) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e.to_string())),
        };
        Self {
            tools,
            tools_error,
            project: None,
            project_path: None,
            metadata: None,
            datasets: vec![],
            preview: None,
            preview_playback: None,
            video_texture: None,
            overlay_texture: None,
            pending_overlay_image: None,
            current_time: 0.0,
            playing: false,
            selected_source: None,
            selected_widget: None,
            resizing_widget: false,
            status: "Open the Insta360 Studio MP4 to begin".into(),
            tx,
            rx,
            export_cancel: None,
            export_dialog_open: false,
            export_codec: ExportCodecChoice::MatchSource,
            export_match_bitrate: true,
            export_bitrate_mbps: 20.0,
            export_apple_compatible: true,
            export_fast_start: true,
            export_force_yuv420p: true,
            syncing_sources: vec![],
            export_fraction: 0.0,
            calibration_start: 0.0,
            calibration_end: 2.0,
            calibration_source_time: false,
            calibration_auto_stationary: true,
            calibration_forward_axis: 0,
            calibration_roll_deg: 0.0,
            calibration_pitch_deg: 0.0,
            calibration_yaw_deg: 0.0,
            calibration_low_pass_hz: 8.0,
            show_data_plot: false,
            plot_channels: vec![],
            plot_window_seconds: 10.0,
            plot_filter_enabled: false,
            plot_filter_hz: 8.0,
            plot_robust_scale: true,
            remove_source_confirm: None,
            unit_system: cc
                .storage
                .and_then(|storage| storage.get_string(UNIT_SYSTEM_STORAGE_KEY))
                .as_deref()
                .and_then(parse_unit_system)
                .unwrap_or_default(),
            correlation_reference_source: None,
            correlation_target_channel: None,
            correlation_reference_channel: None,
            correlation_min_adjustment: -5.0,
            correlation_max_adjustment: 5.0,
            correlation_absolute: false,
            correlation_result: None,
            correlation_running: false,
            correlation_job: 0,
        }
    }

    fn project(&self) -> Option<&ProjectV1> {
        self.project.as_ref().map(ProjectDocument::v1)
    }

    fn project_mut(&mut self) -> Option<&mut ProjectV1> {
        match self.project.as_mut()? {
            ProjectDocument::V1(p) => Some(p),
        }
    }

    /// Retire any in-flight estimate as well as the visible candidate. A
    /// worker may still finish, but its generation will no longer match.
    fn invalidate_correlation(&mut self) {
        self.correlation_job = self.correlation_job.wrapping_add(1);
        self.correlation_running = false;
        self.correlation_result = None;
    }

    fn duration(&self) -> f64 {
        self.metadata
            .as_ref()
            .and_then(|m| m.duration)
            .unwrap_or(1.0)
            .max(0.001)
    }

    fn initialize_video(&mut self, path: PathBuf) -> Result<(), String> {
        self.stop_playback();
        let tools = self.tools.as_ref().ok_or_else(|| {
            self.tools_error
                .clone()
                .unwrap_or_else(|| "FFmpeg unavailable".into())
        })?;
        let metadata = probe_video(tools, &path).map_err(|e| e.to_string())?;
        self.export_bitrate_mbps = metadata
            .bit_rate
            .map(|rate| rate as f64 / 1_000_000.0)
            .unwrap_or(20.0)
            .clamp(1.0, 200.0);
        self.export_codec = ExportCodecChoice::MatchSource;
        self.export_match_bitrate = true;
        self.preview = Some(PreviewWorker::spawn(tools.clone(), &path));
        self.metadata = Some(metadata);
        self.video_texture = None;
        self.overlay_texture = None;
        self.current_time = 0.0;
        self.request_preview();
        Ok(())
    }

    fn open_video(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv"])
            .pick_file()
        else {
            return;
        };
        match self.initialize_video(path.clone()) {
            Ok(()) => {
                let mut p = ProjectV1::new(path);
                p.widgets = default_widgets_for(self.unit_system);
                self.project = Some(p.into());
                self.project_path = None;
                self.datasets.clear();
                self.invalidate_correlation();
                self.selected_source = None;
                self.selected_widget = Some(0);
                self.status = "Video opened. Add the matching LRV/INSV or synthetic data.".into();
                self.refresh_overlay();
            }
            Err(e) => self.status = e,
        }
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Race Overlay project", &["json"])
            .pick_file()
        else {
            return;
        };
        match ProjectDocument::load(&path) {
            Ok(doc) => {
                let video = doc.v1().video_path.clone();
                if let Err(e) = self.initialize_video(video) {
                    self.status = format!("Project loaded, but its video could not be opened: {e}");
                    return;
                }
                self.datasets.clear();
                self.invalidate_correlation();
                self.project_path = Some(path);
                self.project = Some(doc);
                self.calibration_low_pass_hz = self
                    .project()
                    .map(|project| project.camera_calibration.low_pass_hz)
                    .unwrap_or(8.0);
                self.selected_widget = None;
                self.selected_source = None;
                let sources = self.project().unwrap().sources.clone();
                for source in sources {
                    self.load_source_async(source);
                }
                self.status = "Project opened; loading telemetry…".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn save_project(&mut self, save_as: bool) {
        let path = if !save_as {
            self.project_path.clone()
        } else {
            None
        }
        .or_else(|| {
            rfd::FileDialog::new()
                .set_file_name("race.race-overlay.json")
                .save_file()
        });
        let (Some(path), Some(project)) = (path, self.project.as_ref()) else {
            return;
        };
        match project.save_atomic(&path) {
            Ok(()) => {
                self.project_path = Some(path);
                self.status = "Project saved".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn add_file_source(&mut self, adapter: &str) {
        if self.project.is_none() {
            self.status = "Open a video before adding telemetry".into();
            return;
        }
        let filter = match adapter {
            "insta360" => ("Insta360 recording", vec!["lrv", "insv"]),
            "aim_xrk" => ("AiM MyChron recording", vec!["xrk"]),
            _ => ("CSV telemetry", vec!["csv", "txt"]),
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter(filter.0, &filter.1)
            .pick_file()
        else {
            return;
        };
        let source = SourceConfig {
            id: SourceId::new(),
            name: path
                .file_stem()
                .and_then(|v| v.to_str())
                .unwrap_or(adapter)
                .into(),
            adapter: adapter.into(),
            path,
            alignment: Default::default(),
            settings: json!({}),
            unknown: BTreeMap::new(),
        };
        self.project_mut().unwrap().sources.push(source.clone());
        self.selected_source = Some(self.project().unwrap().sources.len() - 1);
        self.load_source_async(source);
    }

    fn add_synthetic(&mut self) {
        let duration = self.duration();
        let source = SourceConfig {
            id: SourceId::new(),
            name: "Synthetic race data".into(),
            adapter: "synthetic".into(),
            path: PathBuf::new(),
            alignment: Default::default(),
            settings: serde_json::to_value(SyntheticConfig {
                duration_seconds: duration,
                sample_rate_hz: 50.0,
            })
            .unwrap(),
            unknown: BTreeMap::new(),
        };
        if let Some(project) = self.project_mut() {
            project.sources.push(source.clone());
            self.selected_source = Some(project.sources.len() - 1);
            self.load_source_async(source);
        } else {
            self.status = "Open a video before adding telemetry".into();
        }
    }

    /// Remove a telemetry source and every reference owned by it. The video,
    /// project, and widgets themselves intentionally remain in place.
    fn remove_source(&mut self, source_index: usize) {
        let Some(source) = self
            .project()
            .and_then(|project| project.sources.get(source_index))
            .cloned()
        else {
            self.remove_source_confirm = None;
            return;
        };
        let source_id = source.id;
        let source_name = source.name;
        self.invalidate_correlation();
        if self.correlation_reference_source == Some(source_id)
            || self
                .correlation_target_channel
                .as_ref()
                .is_some_and(|channel| channel.source_id == source_id)
            || self
                .correlation_reference_channel
                .as_ref()
                .is_some_and(|channel| channel.source_id == source_id)
        {
            self.correlation_reference_source = None;
            self.correlation_target_channel = None;
            self.correlation_reference_channel = None;
        }

        if let Some(project) = self.project_mut() {
            debug_assert_eq!(
                remove_source_from_project(project, source_id),
                Some(source_index)
            );
        }
        self.datasets
            .retain(|dataset| dataset.source_id != source_id);
        self.plot_channels
            .retain(|reference| reference.source_id != source_id);
        self.syncing_sources.retain(|id| *id != source_id);
        self.remove_source_confirm = None;
        self.selected_source = match self.selected_source {
            Some(index) if index == source_index => self.project().and_then(|project| {
                (!project.sources.is_empty()).then_some(index.min(project.sources.len() - 1))
            }),
            Some(index) if index > source_index => Some(index - 1),
            selected => selected,
        };
        self.status = format!("Removed telemetry source: {source_name}");
        self.refresh_overlay();
    }

    /// Reload from the original source so changing or disabling a filter never
    /// compounds a previously filtered dataset.
    fn apply_source_low_pass(&mut self, source_id: SourceId) {
        let source = self
            .project()
            .and_then(|project| project.sources.iter().find(|source| source.id == source_id))
            .cloned();
        if let Some(source) = source {
            self.load_source_async(source);
        }
    }

    fn load_source_async(&mut self, source: SourceConfig) {
        self.status = format!("Loading {}…", source.name);
        let tx = self.tx.clone();
        thread::spawn(move || {
            let registry = AdapterRegistry::with_builtins();
            let result = registry
                .load(&source.adapter, source.id, &source.path, &source.settings)
                .map_err(|e| e.to_string());
            let _ = tx.send(WorkerEvent::SourceLoaded(source.id, result));
        });
    }

    fn auto_sync(&mut self) {
        let Some(index) = self.selected_source else {
            self.status = "Select an Insta360 source first".into();
            return;
        };
        let Some(source_id) = self
            .project()
            .and_then(|project| project.sources.get(index))
            .map(|source| source.id)
        else {
            return;
        };
        self.sync_source(source_id);
    }

    fn sync_source(&mut self, source_id: SourceId) {
        if self.syncing_sources.contains(&source_id) {
            return;
        }
        let Some((source, video)) = self.project().and_then(|project| {
            project
                .sources
                .iter()
                .find(|source| source.id == source_id)
                .map(|source| (source.clone(), project.video_path.clone()))
        }) else {
            return;
        };
        let Some(tools) = self.tools.clone() else {
            self.status = "FFmpeg is required for audio synchronization".into();
            return;
        };
        self.syncing_sources.push(source_id);
        let tx = self.tx.clone();
        self.status = "Synchronizing telemetry to the exported video…".into();
        thread::spawn(move || {
            let result = (|| -> Result<AlignmentResult, String> {
                let reference =
                    extract_mono_pcm(&tools, &source.path, 2_000).map_err(|e| e.to_string())?;
                let candidate =
                    extract_mono_pcm(&tools, &video, 2_000).map_err(|e| e.to_string())?;
                if reference.is_empty() || candidate.is_empty() {
                    return Err("Both recordings need audio for automatic sync".into());
                }
                Ok(align_audio(&reference, &candidate, 2_000))
            })();
            let _ = tx.send(WorkerEvent::Synced(source.id, result));
        });
    }

    fn calibrate_selected(&mut self) {
        let Some(source_index) = self.selected_source else {
            self.status = "Select the camera telemetry source".into();
            return;
        };
        let Some(project) = self.project() else {
            return;
        };
        let source = project.sources[source_index].clone();
        let Some(dataset_index) = self.datasets.iter().position(|d| d.source_id == source.id)
        else {
            self.status = "That source is still loading".into();
            return;
        };
        let dataset = &self.datasets[dataset_index];
        let accel_axes = ["raw_accel_x", "raw_accel_y", "raw_accel_z"];
        let gyro_axes = ["raw_gyro_x", "raw_gyro_y", "raw_gyro_z"];
        let Some(accel) = accel_axes
            .map(|name| dataset.named(name))
            .into_iter()
            .collect::<Option<Vec<_>>>()
        else {
            self.status = "Calibration requires the camera accelerometer channels".into();
            return;
        };
        let Some(gyro) = gyro_axes
            .map(|name| dataset.named(name))
            .into_iter()
            .collect::<Option<Vec<_>>>()
        else {
            self.status = "Calibration requires the camera gyroscope channels".into();
            return;
        };
        let alignment = if self.calibration_source_time {
            0.0
        } else {
            source.alignment.offset_seconds
        };
        let mut start = self.calibration_start + alignment;
        let mut end = self.calibration_end + alignment;
        let mut accel_window = imu_window(&accel, start, end);
        let mut gyro_window = imu_window(&gyro, start, end);
        let selected_is_moving = imu_motion_metrics(&accel_window, &gyro_window)
            .is_some_and(|(accel_rms, gyro_rms)| accel_rms > 1.0 || gyro_rms > 0.15);
        let mut used_automatic_interval = false;
        if self.calibration_auto_stationary
            && selected_is_moving
            && let Some((quiet_start, quiet_end)) =
                quietest_imu_interval(&accel, &gyro, (end - start).max(1.0))
        {
            start = quiet_start;
            end = quiet_end;
            accel_window = imu_window(&accel, start, end);
            gyro_window = imu_window(&gyro, start, end);
            used_automatic_interval = true;
        }
        let Some(gravity) = median_gravity(accel_window.iter().copied()) else {
            self.status = "No IMU samples were found in that stationary interval".into();
            return;
        };
        let gyro_bias = median_gravity(gyro_window.iter().copied()).unwrap_or([0.0; 3]);
        let Some(rotation) = guided_sensor_to_vehicle_from_forward_with_trim(
            gravity,
            forward_axis_vector(self.calibration_forward_axis),
            self.calibration_roll_deg.to_radians(),
            self.calibration_pitch_deg.to_radians(),
            self.calibration_yaw_deg.to_radians(),
            [false; 3],
        ) else {
            self.status =
                "The selected forward axis is vertical. Choose a different camera-forward axis."
                    .into();
            return;
        };
        let accelerometer_bias = accelerometer_bias_for_gravity(gravity, rotation);
        let calibration = CameraCalibration {
            sensor_to_vehicle: rotation,
            accelerometer_bias,
            gyroscope_bias: gyro_bias,
            low_pass_hz: self.calibration_low_pass_hz,
            notes: Some(format!(
                "Stationary source {:.3}–{:.3}s; forward {}; trim roll {:.1}°, pitch {:.1}°, yaw {:.1}°; gravity [{:.3}, {:.3}, {:.3}]",
                start,
                end,
                forward_axis_label(self.calibration_forward_axis),
                self.calibration_roll_deg,
                self.calibration_pitch_deg,
                self.calibration_yaw_deg,
                gravity[0],
                gravity[1],
                gravity[2]
            )),
        };
        self.project_mut().unwrap().camera_calibration = calibration.clone();
        add_derived_inertial_channels(
            &mut self.datasets[dataset_index],
            &calibration,
            INSTA360_IMU_SAMPLE_RATE_HZ,
        );
        self.auto_bind_g_meter(source.id);
        let (accel_rms, gyro_rms) = imu_motion_metrics(&accel_window, &gyro_window)
            .unwrap_or((f64::INFINITY, f64::INFINITY));
        self.status = if accel_rms > 1.0 || gyro_rms > 0.15 {
            format!(
                "Calibration applied, but the interval appears to be moving (accel RMS {accel_rms:.2} m/s², gyro RMS {gyro_rms:.2} rad/s). Choose a stationary interval."
            )
        } else if used_automatic_interval {
            format!(
                "The selected video interval was moving; calibration used the quietest raw-recording interval automatically ({:.1} Hz filter)",
                calibration.low_pass_hz
            )
        } else {
            format!(
                "Calibration applied at {:.1} Hz; derived G and turn-rate channels updated",
                calibration.low_pass_hz
            )
        };
        self.refresh_overlay();
    }

    fn auto_bind_g_meter(&mut self, source_id: SourceId) {
        let Some(dataset) = self.datasets.iter().find(|d| d.source_id == source_id) else {
            return;
        };
        let x = dataset.named("lateral_g").map(|c| c.descriptor.id);
        let y = dataset.named("longitudinal_g").map(|c| c.descriptor.id);
        let (Some(x), Some(y)) = (x, y) else { return };
        let Some(project) = self.project_mut() else {
            return;
        };
        if let Some(widget) = project.widgets.iter_mut().find(|w| w.kind == "xy_dot") {
            widget.bindings = vec![
                ChannelBinding::new(
                    "x",
                    ChannelRef {
                        source_id,
                        channel_id: x,
                    },
                ),
                ChannelBinding::new(
                    "y",
                    ChannelRef {
                        source_id,
                        channel_id: y,
                    },
                ),
            ];
        }
    }

    fn request_preview(&mut self) {
        if let Some(preview) = &self.preview
            && let Err(e) = preview.request(
                self.current_time.max(0.0),
                PreviewSize::new(PREVIEW_W, PREVIEW_H),
            )
        {
            self.status = e.to_string();
        }
    }

    fn start_playback(&mut self) {
        let fps = self
            .metadata
            .as_ref()
            .and_then(VideoMetadata::fps)
            .unwrap_or(30.0)
            .clamp(15.0, 60.0);
        let result = self.preview.as_ref().map(|preview| {
            preview.play(
                self.current_time,
                fps,
                PreviewSize::new(PREVIEW_W, PREVIEW_H),
            )
        });
        match result {
            Some(Ok(playback)) => {
                self.preview_playback = Some(playback);
                self.playing = true;
            }
            Some(Err(error)) => self.status = error.to_string(),
            None => {}
        }
    }

    fn stop_playback(&mut self) {
        if let Some(playback) = self.preview_playback.take() {
            playback.pause();
        }
        self.playing = false;
    }

    fn source_offsets(&self) -> HashMap<SourceId, f64> {
        self.project()
            .map(|p| {
                p.sources
                    .iter()
                    .map(|s| (s.id, s.alignment.offset_seconds))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn refresh_overlay(&mut self) {
        let Some(project) = self.project() else {
            self.overlay_texture = None;
            return;
        };
        let aligned = AlignedDatasets {
            datasets: &self.datasets,
            source_offsets: self.source_offsets(),
            ..Default::default()
        };
        let result = render_project_widgets(
            &project.widgets,
            &aligned,
            RenderSize::new(PREVIEW_W, PREVIEW_H),
            self.current_time,
            RenderOptions {
                crop: false,
                full_size: false,
            },
        );
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [result.image.width as usize, result.image.height as usize],
            &result.image.pixels,
        );
        if let Some(texture) = self.overlay_texture.as_mut() {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.pending_overlay_image = Some(image);
        }
    }

    fn poll_workers(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::SourceLoaded(id, Ok(mut dataset)) => {
                    // A source may have been removed while its adapter was
                    // loading. Do not let that late result resurrect it.
                    if !self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        continue;
                    }
                    let saved_calibration = self.project().and_then(|project| {
                        project
                            .sources
                            .iter()
                            .find(|source| source.id == id && source.adapter == "insta360")
                            .and_then(|_| {
                                project
                                    .camera_calibration
                                    .notes
                                    .as_ref()
                                    .map(|_| project.camera_calibration.clone())
                            })
                    });
                    let source_filter = self.project().and_then(|project| {
                        project
                            .sources
                            .iter()
                            .find(|source| source.id == id)
                            .map(|source| source_low_pass_settings(&source.settings))
                    });
                    let source_cutoff_hz =
                        source_filter.and_then(|(enabled, cutoff_hz)| enabled.then_some(cutoff_hz));
                    let filtered_channels = match prepare_loaded_dataset(
                        &mut dataset,
                        source_cutoff_hz,
                        saved_calibration.as_ref(),
                    ) {
                        Ok(count) => count,
                        Err(error) => {
                            self.status = error;
                            0
                        }
                    };
                    if let Some(old) = self.datasets.iter().position(|d| d.source_id == id) {
                        self.datasets[old] = dataset;
                    } else {
                        self.datasets.push(dataset);
                    }
                    // Reloading a source (including applying its source-level
                    // filter) changes the samples an estimate was based on.
                    self.invalidate_correlation();
                    let count = self
                        .datasets
                        .iter()
                        .find(|d| d.source_id == id)
                        .map(|d| d.channels.len())
                        .unwrap_or(0);
                    self.status = if filtered_channels == 0 {
                        format!("Telemetry loaded: {count} channels")
                    } else {
                        format!(
                            "Telemetry loaded: {count} channels; {filtered_channels} low-pass filtered"
                        )
                    };
                    self.repair_missing_bindings(id);
                    self.auto_bind_unbound_widgets();
                    self.refresh_overlay();
                    let should_auto_sync = self.project().is_some_and(|project| {
                        project.sources.iter().any(|source| {
                            source.id == id
                                && source.adapter == "insta360"
                                && source.alignment.offset_seconds.abs() < 1e-9
                        })
                    }) && self
                        .datasets
                        .iter()
                        .find(|dataset| dataset.source_id == id)
                        .and_then(dataset_duration)
                        .is_some_and(|duration| duration > self.duration() + 1.0);
                    if should_auto_sync {
                        self.sync_source(id);
                    }
                }
                WorkerEvent::SourceLoaded(id, Err(e)) => {
                    if self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        self.status = e;
                    }
                }
                WorkerEvent::Synced(id, Ok(result)) => {
                    self.syncing_sources.retain(|source_id| *source_id != id);
                    if !self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        continue;
                    }
                    // align_audio reports the exported clip relative to raw audio.
                    // Project offsets map video time back into the raw source timeline.
                    let offset = -result.offset_seconds;
                    if let Some(source) = self
                        .project_mut()
                        .and_then(|p| p.sources.iter_mut().find(|s| s.id == id))
                    {
                        source.alignment.offset_seconds = offset;
                    }
                    self.invalidate_correlation();
                    self.status = format!(
                        "Telemetry synchronized to exported-video time (peak {:.2}, confidence {:.2}){}",
                        result.normalized_peak,
                        result.confidence,
                        if result.auto_acceptable() {
                            ""
                        } else {
                            "; verify manually"
                        }
                    );
                    self.refresh_overlay();
                }
                WorkerEvent::Synced(id, Err(e)) => {
                    self.syncing_sources.retain(|source_id| *source_id != id);
                    if !self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        continue;
                    }
                    self.status = format!("Automatic telemetry alignment failed: {e}");
                }
                WorkerEvent::CorrelationEstimated(job, result) => {
                    let Ok(estimate) = result else {
                        if job == self.correlation_job {
                            self.correlation_running = false;
                            self.status = format!("Correlation failed: {}", result.unwrap_err());
                        }
                        continue;
                    };
                    if job != self.correlation_job {
                        continue;
                    }
                    self.correlation_running = false;
                    // A source may have been removed, or a newer estimate may
                    // have superseded this one, while the worker was running.
                    // In either case the result is no longer safe to expose.
                    let still_loaded = self.project().is_some_and(|project| {
                        project.sources.iter().any(|source| {
                            source.id == estimate.target_source
                                && project
                                    .sources
                                    .iter()
                                    .any(|other| other.id == estimate.reference_source)
                        })
                    }) && self.datasets.iter().any(|dataset| {
                        dataset.source_id == estimate.target_source
                            && dataset
                                .channel(estimate.target_channel.channel_id)
                                .is_some()
                    }) && self.datasets.iter().any(|dataset| {
                        dataset.source_id == estimate.reference_source
                            && dataset
                                .channel(estimate.reference_channel.channel_id)
                                .is_some()
                    });
                    if !still_loaded {
                        continue;
                    }
                    self.status = format!(
                        "Correlation: lag {:+.3}s, adjustment {:+.3}s, r {:+.3} · {:.2}s overlap · {} samples",
                        estimate.raw_lag_seconds,
                        estimate.adjustment_seconds,
                        estimate.coefficient,
                        estimate.overlap_seconds,
                        estimate.samples
                    );
                    self.correlation_result = Some(estimate);
                }
                WorkerEvent::ExportProgress(p) => {
                    self.export_fraction = p
                        .duration_seconds
                        .map(|d| (p.encoded_seconds / d).clamp(0.0, 1.0) as f32)
                        .unwrap_or(0.0);
                }
                WorkerEvent::ExportFinished(result) => {
                    self.export_cancel = None;
                    self.export_fraction = 0.0;
                    self.status = match result {
                        Ok(path) => format!("Export finished: {}", path.display()),
                        Err(e) => format!("Export failed: {e}"),
                    };
                }
            }
        }
        let mut playback_advanced = false;
        if let Some(preview) = &self.preview {
            while let Some(frame) = preview.try_recv() {
                match frame {
                    Ok(frame) => {
                        if self.playing {
                            self.current_time = frame.timestamp.min(self.duration());
                            playback_advanced = true;
                        } else if (frame.timestamp - self.current_time).abs() > 0.75 {
                            continue;
                        }
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [frame.width as usize, frame.height as usize],
                            &frame.rgba,
                        );
                        if let Some(texture) = self.video_texture.as_mut() {
                            texture.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            self.video_texture = Some(ctx.load_texture(
                                "video-preview",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
        }
        if playback_advanced {
            self.refresh_overlay();
        }
    }

    fn auto_bind_unbound_widgets(&mut self) {
        let channels: Vec<_> = self
            .datasets
            .iter()
            .flat_map(|d| {
                d.channels
                    .values()
                    .map(move |c| (d.source_id, c.descriptor.id, c.descriptor.name.clone()))
            })
            .collect();
        let Some(project) = self.project() else {
            return;
        };
        let mut bindings = Vec::new();
        for (widget_index, widget) in project.widgets.iter().enumerate() {
            if !widget.bindings.is_empty() {
                continue;
            }
            if widget.kind == "track_map" {
                if let Some((latitude, longitude)) = select_gps_coordinate_channels(&self.datasets)
                {
                    bindings.push((widget_index, "latitude", latitude));
                    bindings.push((widget_index, "longitude", longitude));
                }
            } else if widget.kind == "xy_dot" {
                let x = channels
                    .iter()
                    .find(|c| c.2 == "lateral_g")
                    .or_else(|| channels.iter().find(|c| c.2 == "gps_lateral_acceleration"));
                let y = channels
                    .iter()
                    .find(|c| c.2 == "longitudinal_g")
                    .or_else(|| channels.iter().find(|c| c.2 == "gps_inline_acceleration"));
                if let (Some(x), Some(y)) = (x, y) {
                    bindings.push((
                        widget_index,
                        "x",
                        ChannelRef {
                            source_id: x.0,
                            channel_id: x.1,
                        },
                    ));
                    bindings.push((
                        widget_index,
                        "y",
                        ChannelRef {
                            source_id: y.0,
                            channel_id: y.1,
                        },
                    ));
                }
            } else {
                let preferred = preferred_channels(&widget.kind);
                let channel = preferred
                    .iter()
                    .find_map(|name| channels.iter().find(|channel| channel.2 == **name))
                    .or_else(|| {
                        (widget.kind == "numeric")
                            .then(|| channels.first())
                            .flatten()
                    });
                if let Some(channel) = channel {
                    bindings.push((
                        widget_index,
                        "value",
                        ChannelRef {
                            source_id: channel.0,
                            channel_id: channel.1,
                        },
                    ));
                }
            }
        }
        for (widget_index, slot, reference) in bindings {
            self.bind_widget_channel(widget_index, slot, reference);
        }
    }

    fn repair_missing_bindings(&mut self, source_id: SourceId) {
        let Some(dataset) = self
            .datasets
            .iter()
            .find(|item| item.source_id == source_id)
        else {
            return;
        };
        let named = |name: &str| {
            dataset.named(name).map(|channel| ChannelRef {
                source_id,
                channel_id: channel.descriptor.id,
            })
        };
        let lateral = named("lateral_g").or_else(|| named("gps_lateral_acceleration"));
        let longitudinal = named("longitudinal_g").or_else(|| named("gps_inline_acceleration"));
        let combined = named("combined_g");
        let speed = named("speed").or_else(|| named("gps_speed"));
        let rpm = named("rpm");
        let gear = named("gear");
        let water_temperature = named("water_temperature");
        let exhaust_temperature = named("exhaust_temperature");
        let lap_time = named("lap_time");
        let delta = named("best_today_diff").or_else(|| named("predictive_time"));
        let steering_angle = named("steering_angle");
        let (latitude, longitude) = select_gps_coordinate_channels(std::slice::from_ref(dataset))
            .map_or((None, None), |(latitude, longitude)| {
                (Some(latitude), Some(longitude))
            });
        let widgets = self
            .project()
            .map(|project| project.widgets.clone())
            .unwrap_or_default();
        let mut repairs = Vec::new();
        for (widget_index, widget) in widgets.iter().enumerate() {
            for binding in &widget.bindings {
                if binding.channel.source_id != source_id
                    || dataset.channel(binding.channel.channel_id).is_some()
                {
                    continue;
                }
                let replacement = match (widget.kind.as_str(), binding.slot.as_str()) {
                    ("xy_dot", "x") => lateral.clone(),
                    ("xy_dot", "y") => longitudinal.clone(),
                    ("radial", "value") => speed.clone(),
                    ("bar", "value") => rpm.clone().or_else(|| combined.clone()),
                    ("numeric" | "gear", "value") => gear.clone().or_else(|| combined.clone()),
                    ("tachometer" | "shift_lights", "value") => rpm.clone(),
                    ("temperature", "value") => water_temperature
                        .clone()
                        .or_else(|| exhaust_temperature.clone()),
                    ("lap_timer", "value") => lap_time.clone(),
                    ("delta", "value") => delta.clone(),
                    ("center_bar", "value") => steering_angle.clone(),
                    ("track_map", "latitude") => latitude.clone(),
                    ("track_map", "longitude") => longitude.clone(),
                    _ => None,
                };
                repairs.push((widget_index, binding.slot.clone(), replacement));
            }
        }
        for (widget_index, slot, replacement) in repairs {
            if let Some(reference) = replacement {
                self.bind_widget_channel(widget_index, &slot, reference);
            } else if let Some(widget) = self
                .project_mut()
                .and_then(|project| project.widgets.get_mut(widget_index))
            {
                widget.bindings.retain(|binding| {
                    binding.slot != slot || binding.channel.source_id != source_id
                });
            }
        }
    }

    fn add_widget(&mut self, kind: &str) {
        let count = self.project().map(|p| p.widgets.len()).unwrap_or(0);
        let mut widget = make_widget(kind, count);
        apply_unbound_widget_unit_default(&mut widget, self.unit_system);
        if let Some(project) = self.project_mut() {
            project.widgets.push(widget);
            self.selected_widget = Some(project.widgets.len() - 1);
            self.auto_bind_unbound_widgets();
            self.refresh_overlay();
        }
    }

    /// Add a useful MyChron layout without disturbing widgets the user has
    /// already placed. When a source is loaded, bind each dashboard tile to
    /// its intended channel immediately (especially water vs. exhaust temp).
    fn add_mychron_dashboard(&mut self) {
        let base = self.project().map(|project| project.widgets.len());
        let Some(base) = base else { return };
        let mut water = make_widget("temperature", base);
        water.rect = NormalizedRect::new(0.04, 0.05, 0.22, 0.12);
        water.style["label"] = json!("WATER");
        let mut egt = make_widget("temperature", base + 1);
        egt.rect = NormalizedRect::new(0.28, 0.05, 0.22, 0.12);
        egt.style["label"] = json!("EGT");
        let mut lap = make_widget("lap_timer", base + 2);
        lap.rect = NormalizedRect::new(0.72, 0.05, 0.24, 0.12);
        let mut delta = make_widget("delta", base + 3);
        delta.rect = NormalizedRect::new(0.72, 0.19, 0.24, 0.12);
        let mut shift = make_widget("shift_lights", base + 4);
        shift.rect = NormalizedRect::new(0.30, 0.61, 0.40, 0.08);
        let mut tach = make_widget("tachometer", base + 5);
        tach.rect = NormalizedRect::new(0.30, 0.70, 0.40, 0.20);
        let mut steering = make_widget("center_bar", base + 6);
        steering.rect = NormalizedRect::new(0.24, 0.90, 0.52, 0.07);
        let mut speed = make_widget("radial", base + 7);
        speed.rect = NormalizedRect::new(0.76, 0.63, 0.20, 0.34);
        let mut gear = make_widget("gear", base + 8);
        gear.rect = NormalizedRect::new(0.46, 0.43, 0.08, 0.14);
        let mut g_meter = make_widget("xy_dot", base + 9);
        g_meter.rect = NormalizedRect::new(0.04, 0.61, 0.20, 0.36);
        g_meter.style["min"] = json!(-2.5);
        g_meter.style["max"] = json!(2.5);
        for widget in [&mut water, &mut egt, &mut speed] {
            apply_unbound_widget_unit_default(widget, self.unit_system);
        }
        let selected = {
            let Some(project) = self.project_mut() else {
                return;
            };
            project.widgets.extend([
                water, egt, lap, delta, shift, tach, steering, speed, gear, g_meter,
            ]);
            project.widgets.len() - 1
        };
        self.selected_widget = Some(selected);

        let mychron_source_id = self.project().and_then(|project| {
            self.selected_source
                .and_then(|index| project.sources.get(index))
                .filter(|source| source.adapter == "aim_xrk")
                .or_else(|| {
                    project
                        .sources
                        .iter()
                        .rev()
                        .find(|source| source.adapter == "aim_xrk")
                })
                .map(|source| source.id)
        });
        let source = self
            .datasets
            .iter()
            .find(|dataset| Some(dataset.source_id) == mychron_source_id)
            .or_else(|| {
                self.datasets.iter().find(|dataset| {
                    dataset.named("rpm").is_some()
                        && (dataset.named("water_temperature").is_some()
                            || dataset.named("exhaust_temperature").is_some())
                })
            })
            .map(|dataset| {
                let named = |name: &str| {
                    dataset.named(name).map(|channel| ChannelRef {
                        source_id: dataset.source_id,
                        channel_id: channel.descriptor.id,
                    })
                };
                [
                    (base, "value", named("water_temperature")),
                    (base + 1, "value", named("exhaust_temperature")),
                    (base + 2, "value", named("lap_time")),
                    (
                        base + 3,
                        "value",
                        named("best_today_diff").or_else(|| named("predictive_time")),
                    ),
                    (base + 4, "value", named("rpm")),
                    (base + 5, "value", named("rpm")),
                    (base + 6, "value", named("steering_angle")),
                    (
                        base + 7,
                        "value",
                        named("speed").or_else(|| named("gps_speed")),
                    ),
                    (base + 8, "value", named("gear")),
                    (
                        base + 9,
                        "x",
                        named("lateral_g").or_else(|| named("gps_lateral_acceleration")),
                    ),
                    (
                        base + 9,
                        "y",
                        named("longitudinal_g").or_else(|| named("gps_inline_acceleration")),
                    ),
                ]
            });
        if let Some(bindings) = source {
            for (widget_index, slot, reference) in
                bindings
                    .into_iter()
                    .filter_map(|(widget_index, slot, reference)| {
                        reference.map(|reference| (widget_index, slot, reference))
                    })
            {
                self.bind_widget_channel(widget_index, slot, reference);
            }
        }
        // Keep the dashboard's concise labels after bind_widget_channel has
        // populated descriptive channel labels for ordinary widgets.
        if let Some(project) = self.project_mut() {
            if let Some(widget) = project.widgets.get_mut(base) {
                set_style(&mut widget.style, "label", json!("WATER"));
            }
            if let Some(widget) = project.widgets.get_mut(base + 1) {
                set_style(&mut widget.style, "label", json!("EGT"));
            }
        }
        self.refresh_overlay();
    }

    fn export_with_settings(&mut self, settings: ExportSettings) {
        if self.export_cancel.is_some() {
            return;
        }
        let (Some(project), Some(metadata), Some(tools)) = (
            self.project().cloned(),
            self.metadata.clone(),
            self.tools.clone(),
        ) else {
            self.status = "Open a video and configure FFmpeg before exporting".into();
            return;
        };
        let Some(output) = rfd::FileDialog::new()
            .add_filter("MPEG-4 video", &["mp4"])
            .set_file_name("race-overlay.mp4")
            .save_file()
        else {
            return;
        };
        let datasets = self.datasets.clone();
        let offsets: HashMap<_, _> = project
            .sources
            .iter()
            .map(|s| (s.id, s.alignment.offset_seconds))
            .collect();
        let tx = self.tx.clone();
        let cancel = CancelToken::new();
        self.export_cancel = Some(cancel.clone());
        self.status = "Exporting overlay…".into();
        thread::spawn(move || {
            let size = RenderSize::new(metadata.width, metadata.height);
            let aligned = AlignedDatasets {
                datasets: &datasets,
                source_offsets: offsets,
                cache_filtered_series: true,
                ..Default::default()
            };
            let Some((geometry, export_widgets)) = export_surface(&project.widgets, size) else {
                let _ = tx.send(WorkerEvent::ExportFinished(Err(
                    "There are no visible widgets to export".into(),
                )));
                return;
            };
            // Track geometry and any other static widget state are independent
            // of frame time. Preparing once avoids re-synchronizing and
            // selecting thousands of GPS points for every exported frame.
            let prepared_widgets = prepare_project_widgets(&export_widgets, &aligned);
            let fps = metadata.fps().unwrap_or(30.0);
            let frame_count = (metadata.duration.unwrap_or(0.0) * fps).ceil() as u64;
            let mut frame = 0u64;
            let mut provider = || -> Result<Option<RgbaFrame>, overlay_media::MediaError> {
                if frame >= frame_count || cancel.is_cancelled() {
                    return Ok(None);
                }
                let t = frame as f64 / fps;
                let rendered = render_prepared_project_widgets(
                    &prepared_widgets,
                    &aligned,
                    RenderSize::new(geometry.width, geometry.height),
                    t,
                    RenderOptions {
                        crop: false,
                        full_size: false,
                    },
                );
                frame += 1;
                RgbaFrame::new(
                    rendered.image.width,
                    rendered.image.height,
                    rendered.image.pixels,
                )
                .map(Some)
            };
            let result = export_video(
                &tools,
                &project.video_path,
                &output,
                &settings,
                geometry,
                &mut provider,
                &cancel,
                |p| {
                    let _ = tx.send(WorkerEvent::ExportProgress(p));
                },
            )
            .map(|_| output.clone())
            .map_err(|e| e.to_string());
            let _ = tx.send(WorkerEvent::ExportFinished(result));
        });
    }

    fn channel_choices(&self) -> Vec<(ChannelRef, String)> {
        self.datasets
            .iter()
            .flat_map(|dataset| {
                let source_name = self
                    .project()
                    .and_then(|p| p.sources.iter().find(|s| s.id == dataset.source_id))
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "source".into());
                dataset.channels.values().map(move |channel| {
                    (
                        ChannelRef {
                            source_id: dataset.source_id,
                            channel_id: channel.descriptor.id,
                        },
                        format!(
                            "{} / {} ({})",
                            source_name,
                            channel.descriptor.name,
                            channel.descriptor.unit.symbol()
                        ),
                    )
                })
            })
            .collect()
    }

    fn gps_channel_choices(&self, slot: &str) -> Vec<(ChannelRef, String)> {
        self.datasets
            .iter()
            .flat_map(|dataset| {
                let source_name = self
                    .project()
                    .and_then(|p| p.sources.iter().find(|s| s.id == dataset.source_id))
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "source".into());
                dataset.channels.values().filter_map(move |channel| {
                    let name = channel.descriptor.name.as_str();
                    let matching = match slot {
                        "latitude" => is_latitude_channel(name),
                        "longitude" => is_longitude_channel(name),
                        _ => false,
                    };
                    (matching && channel.descriptor.unit == Unit::Degree).then(|| {
                        (
                            ChannelRef {
                                source_id: dataset.source_id,
                                channel_id: channel.descriptor.id,
                            },
                            format!(
                                "{} / {} ({})",
                                source_name,
                                channel.descriptor.name,
                                channel.descriptor.unit.symbol()
                            ),
                        )
                    })
                })
            })
            .collect()
    }

    fn selected_channel_label(&self, binding: Option<&ChannelBinding>) -> String {
        let Some(binding) = binding else {
            return "Unbound".into();
        };
        self.channel_choices()
            .into_iter()
            .find(|(r, _)| *r == binding.channel)
            .map(|(_, label)| label)
            .unwrap_or_else(|| "Missing channel".into())
    }

    fn bind_widget_channel(&mut self, widget_index: usize, slot: &str, reference: ChannelRef) {
        let unit_system = self.unit_system;
        let descriptor = self
            .datasets
            .iter()
            .find(|dataset| dataset.source_id == reference.source_id)
            .and_then(|dataset| dataset.channel(reference.channel_id))
            .map(|channel| channel.descriptor.clone());
        let Some(widget) = self
            .project_mut()
            .and_then(|project| project.widgets.get_mut(widget_index))
        else {
            return;
        };
        let binding = widget_channel_binding(
            &widget.kind,
            slot,
            reference,
            descriptor.as_ref(),
            unit_system,
        );
        let display_unit = binding
            .display_unit
            .clone()
            .or_else(|| descriptor.as_ref().map(|item| item.unit.clone()));
        if let Some(existing) = widget.bindings.iter_mut().find(|item| item.slot == slot) {
            *existing = binding;
        } else {
            widget.bindings.push(binding);
        }
        if let Some(descriptor) = descriptor {
            if !matches!(widget.kind.as_str(), "xy_dot" | "track_map") {
                let label = descriptor.name.replace('_', " ").to_ascii_uppercase();
                set_style(&mut widget.style, "label", json!(label));
            }
            let display_unit = display_unit.as_ref().unwrap_or(&descriptor.unit);
            set_style(&mut widget.style, "unit", json!(display_unit.symbol()));
            let (min, max) = widget_suggested_range(
                &widget.kind,
                &descriptor.name,
                &descriptor.quantity,
                display_unit,
            );
            set_style(&mut widget.style, "min", json!(min));
            set_style(&mut widget.style, "max", json!(max));
        }
    }

    fn bind_track_map_coordinate(
        &mut self,
        widget_index: usize,
        slot: &str,
        reference: ChannelRef,
    ) {
        let source_id = reference.source_id;
        self.bind_widget_channel(widget_index, slot, reference);
        let Some(dataset) = self
            .datasets
            .iter()
            .find(|dataset| dataset.source_id == source_id)
        else {
            return;
        };
        let Some((latitude, longitude)) =
            select_gps_coordinate_channels(std::slice::from_ref(dataset))
        else {
            return;
        };
        match slot {
            "latitude" => self.bind_widget_channel(widget_index, "longitude", longitude),
            "longitude" => self.bind_widget_channel(widget_index, "latitude", latitude),
            _ => {}
        }
    }

    fn capture_track_map_point(&mut self, widget_index: usize, which: &str) -> bool {
        let Some(widget) = self
            .project()
            .and_then(|project| project.widgets.get(widget_index))
        else {
            return false;
        };
        let latitude = widget
            .bindings
            .iter()
            .find(|binding| binding.slot == "latitude")
            .map(|binding| binding.channel.clone());
        let longitude = widget
            .bindings
            .iter()
            .find(|binding| binding.slot == "longitude")
            .map(|binding| binding.channel.clone());
        let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
            self.status = "Bind GPS latitude and longitude before capturing a map point".into();
            return false;
        };
        if latitude.source_id != longitude.source_id {
            self.status = "GPS latitude and longitude must come from the same source".into();
            return false;
        }
        let offsets = self.source_offsets();
        let Some(lat) = coordinate_value_at(&self.datasets, &offsets, &latitude, self.current_time)
        else {
            self.status = "No GPS latitude sample at the current playhead".into();
            return false;
        };
        let Some(lon) =
            coordinate_value_at(&self.datasets, &offsets, &longitude, self.current_time)
        else {
            self.status = "No GPS longitude sample at the current playhead".into();
            return false;
        };
        if !(lat.abs() <= 90.0 && lon.abs() <= 180.0) {
            self.status = "The captured GPS coordinates are outside valid bounds".into();
            return false;
        }
        if let Some(widget) = self
            .project_mut()
            .and_then(|project| project.widgets.get_mut(widget_index))
        {
            set_style(&mut widget.style, &format!("{which}_latitude"), json!(lat));
            set_style(&mut widget.style, &format!("{which}_longitude"), json!(lon));
            self.status = format!("Captured {which} GPS location ({lat:.6}, {lon:.6})");
            true
        } else {
            false
        }
    }

    fn clear_track_map_point(&mut self, widget_index: usize, which: &str) -> bool {
        let Some(widget) = self
            .project_mut()
            .and_then(|project| project.widgets.get_mut(widget_index))
        else {
            return false;
        };
        remove_style(&mut widget.style, &format!("{which}_latitude"));
        remove_style(&mut widget.style, &format!("{which}_longitude"));
        true
    }

    fn apply_default_unit_system(&mut self) {
        let descriptors: HashMap<_, _> = self
            .datasets
            .iter()
            .flat_map(|dataset| {
                dataset.channels.values().map(move |channel| {
                    (
                        (dataset.source_id, channel.descriptor.id),
                        channel.descriptor.clone(),
                    )
                })
            })
            .collect();
        let system = self.unit_system;
        let Some(project) = self.project_mut() else {
            return;
        };
        for widget in &mut project.widgets {
            let Some(primary) = widget.bindings.first() else {
                apply_unbound_widget_unit_default(widget, system);
                continue;
            };
            let Some(descriptor) =
                descriptors.get(&(primary.channel.source_id, primary.channel.channel_id))
            else {
                continue;
            };
            let Some(target) = default_display_unit(&widget.kind, descriptor, system) else {
                continue;
            };
            retarget_widget_units(widget, &descriptors, &target);
        }
        self.refresh_overlay();
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open video").clicked() {
                self.open_video();
            }
            if ui.button("Open project").clicked() {
                self.open_project();
            }
            let previous_units = self.unit_system;
            egui::ComboBox::from_id_salt("default-unit-system")
                .selected_text(unit_system_label(self.unit_system))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.unit_system, UnitSystem::Metric, "Metric");
                    ui.selectable_value(&mut self.unit_system, UnitSystem::Imperial, "Imperial");
                });
            if self.unit_system != previous_units {
                self.apply_default_unit_system();
            }
            ui.add_enabled_ui(self.project.is_some(), |ui| {
                if ui.selectable_label(!self.show_data_plot, "Video").clicked() {
                    self.show_data_plot = false;
                }
                if ui
                    .selectable_label(self.show_data_plot, "Data graph")
                    .clicked()
                {
                    self.show_data_plot = true;
                }
                if ui.button("Save").clicked() {
                    self.save_project(false);
                }
                if ui.button("Save as…").clicked() {
                    self.save_project(true);
                }
                if ui.button("Export MP4…").clicked() {
                    self.export_dialog_open = true;
                }
            });
            if let Some(cancel) = &self.export_cancel {
                ui.add(egui::ProgressBar::new(self.export_fraction).desired_width(120.0));
                if ui.button("Cancel export").clicked() {
                    cancel.cancel();
                }
            }
        });
    }

    fn export_dialog(&mut self, ctx: &egui::Context) {
        if !self.export_dialog_open {
            return;
        }
        let Some(metadata) = self.metadata.clone() else {
            self.export_dialog_open = false;
            return;
        };
        let mut open = self.export_dialog_open;
        let mut start_export = false;
        egui::Window::new("Export video")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.heading("Source video");
                let codec = metadata
                    .codec
                    .as_deref()
                    .map(codec_display_name)
                    .unwrap_or("Unknown codec");
                let fps = metadata
                    .fps()
                    .map(|fps| format!("{fps:.3} fps"))
                    .unwrap_or_else(|| "unknown frame rate".into());
                let rate = metadata
                    .bit_rate
                    .map(|rate| format!("{:.1} Mb/s", rate as f64 / 1_000_000.0))
                    .unwrap_or_else(|| "unknown bitrate".into());
                ui.label(format!(
                    "{codec} • {}×{} • {fps} • {rate}",
                    metadata.width, metadata.height
                ));
                ui.add_space(8.0);
                ui.heading("Video codec");
                for choice in [
                    ExportCodecChoice::MatchSource,
                    ExportCodecChoice::H264,
                    ExportCodecChoice::H265,
                ] {
                    ui.radio_value(&mut self.export_codec, choice, choice.label());
                }
                ui.small(match self.export_codec {
                    ExportCodecChoice::MatchSource => {
                        "Uses the source codec family and Apple-compatible MP4 tagging."
                    }
                    ExportCodecChoice::H264 => {
                        "Best fallback for older Macs, editors, browsers, and sharing services."
                    }
                    ExportCodecChoice::H265 => {
                        "Efficient for 4K; requires HEVC-capable playback hardware/software."
                    }
                });
                ui.add_space(8.0);
                ui.heading("Quality");
                ui.checkbox(&mut self.export_match_bitrate, "Match source bitrate");
                ui.add_enabled_ui(!self.export_match_bitrate, |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.export_bitrate_mbps, 1.0..=200.0)
                            .logarithmic(true)
                            .suffix(" Mb/s")
                            .text("Video bitrate"),
                    );
                });
                let selected_rate = if self.export_match_bitrate {
                    metadata.bit_rate.map(|rate| rate as f64 / 1_000_000.0)
                } else {
                    Some(self.export_bitrate_mbps)
                };
                if let (Some(rate), Some(duration)) = (selected_rate, metadata.duration) {
                    let estimated_mb = rate * duration / 8.0;
                    ui.small(format!(
                        "Approximate video size: {:.0} MB (audio adds a small amount)",
                        estimated_mb
                    ));
                }
                ui.collapsing("Advanced compatibility", |ui| {
                    ui.checkbox(
                        &mut self.export_apple_compatible,
                        "Apple-compatible MP4 codec tags and color defaults",
                    );
                    ui.checkbox(
                        &mut self.export_force_yuv420p,
                        "Force widely compatible 8-bit YUV 4:2:0",
                    );
                    ui.checkbox(
                        &mut self.export_fast_start,
                        "Fast-start MP4 (index at beginning)",
                    );
                });
                ui.separator();
                ui.small("Resolution, frame rate, color metadata, and source audio are preserved. Hardware encoding is preferred when available.");
                ui.horizontal(|ui| {
                    if ui.button("Choose file and export…").clicked() {
                        start_export = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.export_dialog_open = false;
                    }
                });
            });
        self.export_dialog_open &= open;
        if start_export {
            let settings = ExportSettings {
                codec: self.export_codec.codec(),
                bitrate: (!self.export_match_bitrate)
                    .then_some((self.export_bitrate_mbps * 1_000_000.0).round() as u64),
                pixel_format: self.export_force_yuv420p.then(|| "yuv420p".into()),
                apple_compatible: self.export_apple_compatible,
                fast_start: self.export_fast_start,
                ..Default::default()
            };
            self.export_dialog_open = false;
            self.export_with_settings(settings);
        }
    }

    fn sources_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Data sources");
        ui.horizontal_wrapped(|ui| {
            if ui.button("+ Insta360").clicked() {
                self.add_file_source("insta360");
            }
            if ui.button("+ CSV").clicked() {
                self.add_file_source("generic_csv");
            }
            if ui.button("+ MyChron XRK").clicked() {
                self.add_file_source("aim_xrk");
            }
            if ui.button("+ Synthetic").clicked() {
                self.add_synthetic();
            }
        });
        let sources = self
            .project()
            .map(|p| p.sources.clone())
            .unwrap_or_default();
        for (index, source) in sources.iter().enumerate() {
            let loaded = self.datasets.iter().any(|d| d.source_id == source.id);
            if ui
                .selectable_label(
                    self.selected_source == Some(index),
                    format!("{} {}", if loaded { "●" } else { "○" }, source.name),
                )
                .clicked()
            {
                self.selected_source = Some(index);
                self.remove_source_confirm = None;
                self.invalidate_correlation();
            }
        }
        let mut remove_index = None;
        if let Some(index) = self.selected_source {
            if let Some(source) = sources.get(index) {
                ui.separator();
                ui.label(format!("Adapter: {}", source.adapter));
                if !source.path.as_os_str().is_empty() {
                    ui.small(source.path.display().to_string());
                }
                if ui
                    .button(format!("Remove source \u{201c}{}\u{201d}", source.name))
                    .clicked()
                {
                    self.remove_source_confirm = Some(source.id);
                }
                if self.remove_source_confirm == Some(source.id) {
                    ui.group(|ui| {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!(
                                "Remove \u{201c}{}\u{201d}? Its telemetry, plot selections, sync state, and widget bindings will be removed; the video and widgets stay.",
                                source.name
                            ),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Remove selected source").clicked() {
                                remove_index = Some(index);
                            }
                            if ui.button("Keep source").clicked() {
                                self.remove_source_confirm = None;
                            }
                        });
                    });
                }
                if source.adapter == "aim_xrk"
                    && let Some(dataset) = self
                        .datasets
                        .iter()
                        .find(|dataset| dataset.source_id == source.id)
                {
                    let venue = dataset
                        .metadata
                        .get("venue")
                        .map(String::as_str)
                        .unwrap_or("unknown venue");
                    ui.small(format!(
                        "{} lap marker(s) · {} · {} channels",
                        dataset.laps.len(),
                        venue,
                        dataset.channels.len()
                    ));
                }
                let syncing = self.syncing_sources.contains(&source.id);
                let source_duration = self
                    .datasets
                    .iter()
                    .find(|dataset| dataset.source_id == source.id)
                    .and_then(dataset_duration);
                if source_duration.is_some_and(|duration| {
                    duration > self.duration() + 5.0 && source.alignment.offset_seconds.abs() < 1.0
                }) {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        if syncing {
                            "Aligning the raw recording to the exported video…"
                        } else {
                            "Automatic alignment is unresolved; derived data may not match the video."
                        },
                    );
                }
                if source.adapter == "insta360"
                    && ui
                        .add_enabled(!syncing, egui::Button::new("Re-sync from audio"))
                        .clicked()
                {
                    self.auto_sync();
                }
                ui.collapsing("Advanced timing", |ui| {
                    ui.label("Normal controls always use exported-video time.");
                    let mut offset = source.alignment.offset_seconds;
                    if ui
                        .add(
                            egui::DragValue::new(&mut offset)
                                .speed(0.01)
                                .prefix("Raw source offset ")
                                .suffix(" s"),
                        )
                        .changed()
                    {
                        if let Some(config) = self
                            .project_mut()
                            .and_then(|project| project.sources.get_mut(index))
                        {
                            config.alignment.offset_seconds = offset;
                        }
                        self.invalidate_correlation();
                        self.refresh_overlay();
                    }
                    ui.small(format!("Video 0.000 s → raw source {offset:.3} s"));
                    ui.checkbox(
                        &mut self.calibration_source_time,
                        "Calibration interval uses raw source time",
                    );
                });
                let (mut low_pass_enabled, mut low_pass_hz) =
                    source_low_pass_settings(&source.settings);
                let mut low_pass_changed = false;
                ui.collapsing("Source low-pass (all imported continuous channels)", |ui| {
                    if source.adapter == "insta360" {
                        ui.small("Zero-phase, non-causal smoothing before camera-derived signals are calculated. Discrete channels are unchanged.");
                    } else {
                        ui.small("Zero-phase, non-causal smoothing for every imported continuous channel. Discrete channels such as gear are unchanged.");
                    }
                    ui.small("The displayed cutoff is the final two-pass -3 dB point. Enabled source, derived-G, and widget filters cascade.");
                    low_pass_changed |= ui
                        .checkbox(&mut low_pass_enabled, "Enable source smoothing")
                        .changed();
                    if low_pass_enabled {
                        low_pass_changed |= ui
                            .add(
                                egui::Slider::new(&mut low_pass_hz, 0.5..=50.0)
                                    .logarithmic(true)
                                    .text("Cutoff Hz"),
                            )
                            .changed();
                    }
                });
                if low_pass_changed
                    && let Some(config) = self
                        .project_mut()
                        .and_then(|project| project.sources.get_mut(index))
                {
                    set_source_low_pass_settings(
                        &mut config.settings,
                        low_pass_enabled,
                        low_pass_hz,
                    );
                    self.status =
                        "Source smoothing changed; click Reload & apply source smoothing".into();
                }
                if ui.button("Reload & apply source smoothing").clicked() {
                    self.apply_source_low_pass(source.id);
                }
                if source.adapter != "insta360" {
                    self.correlation_ui(ui, source.id);
                }
            }
            if let Some(source) = sources.get(index)
                && source.adapter == "insta360"
            {
                ui.collapsing("Camera calibration", |ui| {
                ui.label("Choose a stationary interval. Then select the camera sensor axis that points toward the kart nose; gravity determines up.");
                ui.small(if self.calibration_source_time {
                    "Interval below is raw source time (advanced mode)."
                } else {
                    "Interval below is exported-video time; negative values are allowed."
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.calibration_start)
                            .speed(0.1)
                            .prefix("From ")
                            .suffix(" s"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.calibration_end)
                            .speed(0.1)
                            .prefix("to ")
                            .suffix(" s"),
                    );
                });
                ui.checkbox(
                    &mut self.calibration_auto_stationary,
                    "Auto-find a stationary interval when this range is moving",
                );
                egui::ComboBox::from_label("Camera-forward axis")
                    .selected_text(forward_axis_label(self.calibration_forward_axis))
                    .show_ui(ui, |ui| {
                        for axis in 0..6 {
                            ui.selectable_value(
                                &mut self.calibration_forward_axis,
                                axis,
                                forward_axis_label(axis),
                            );
                        }
                    });
                ui.add(
                    egui::Slider::new(&mut self.calibration_roll_deg, -30.0..=30.0)
                        .step_by(0.1)
                        .text("Fine roll trim°"),
                );
                ui.add(
                    egui::Slider::new(&mut self.calibration_pitch_deg, -30.0..=30.0)
                        .step_by(0.1)
                        .text("Fine pitch trim°"),
                );
                ui.add(
                    egui::Slider::new(&mut self.calibration_yaw_deg, -90.0..=90.0)
                        .step_by(0.1)
                        .text("Fine yaw trim°"),
                );
                ui.small("Positive pitch raises the nose; positive roll raises the left side; positive yaw turns toward the left.");
                ui.add(
                    egui::Slider::new(&mut self.calibration_low_pass_hz, 0.5..=50.0)
                        .logarithmic(true)
                        .text("Derived G smoothing cutoff Hz"),
                );
                ui.small("Zero-phase, non-causal smoothing during calibration for derived longitudinal, lateral, and vertical G; combined G uses those results. The displayed cutoff is the final two-pass -3 dB point. Turn-rate channels are unchanged.");
                if source_low_pass_settings(&source.settings).0 {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Source smoothing is also enabled, so the two smoothing stages cascade for derived G.",
                    );
                }
                if ui.button("Apply calibration & recalculate derived G").clicked() {
                    self.calibrate_selected();
                }
                });
            }
            if let Some(source) = sources.get(index) {
                let channels = self
                    .datasets
                    .iter()
                    .find(|dataset| dataset.source_id == source.id)
                    .map(|dataset| {
                        dataset
                            .channels
                            .values()
                            .map(|channel| {
                                (
                                    ChannelRef {
                                        source_id: source.id,
                                        channel_id: channel.descriptor.id,
                                    },
                                    format!(
                                        "{} ({})",
                                        channel.descriptor.name,
                                        channel.descriptor.unit.symbol()
                                    ),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                ui.collapsing("Plot channels", |ui| {
                    for (reference, label) in channels {
                        let mut selected = self.plot_channels.contains(&reference);
                        if ui.checkbox(&mut selected, label).changed() {
                            if selected {
                                self.plot_channels.push(reference);
                            } else {
                                self.plot_channels.retain(|item| *item != reference);
                            }
                        }
                    }
                    if ui.button("Open data graph").clicked() {
                        self.show_data_plot = true;
                    }
                });
            }
        }
        if let Some(index) = remove_index {
            self.remove_source(index);
        }
    }

    fn correlation_ui(&mut self, ui: &mut egui::Ui, target_source: SourceId) {
        let target_choices = self.channel_choices_for_source(target_source);
        let reference_sources = self
            .project()
            .map(|project| {
                project
                    .sources
                    .iter()
                    .filter(|source| {
                        source.id != target_source
                            && self
                                .datasets
                                .iter()
                                .any(|dataset| dataset.source_id == source.id)
                    })
                    .map(|source| (source.id, source.name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if target_choices.is_empty() || reference_sources.is_empty() {
            ui.collapsing("Correlate to another source", |ui| {
                ui.small(
                    "Load a channel from this source and another source to enable correlation.",
                );
            });
            return;
        }

        if self
            .correlation_target_channel
            .as_ref()
            .is_none_or(|reference| {
                reference.source_id != target_source
                    || !target_choices.iter().any(|(item, _)| item == reference)
            })
        {
            self.correlation_target_channel = target_choices.first().map(|(item, _)| item.clone());
        }
        if self
            .correlation_reference_source
            .is_none_or(|source| !reference_sources.iter().any(|(id, _)| *id == source))
        {
            self.correlation_reference_source = reference_sources.first().map(|(id, _)| *id);
            self.correlation_reference_channel = None;
        }
        let reference_source = self.correlation_reference_source;
        let reference_choices = reference_source
            .map(|source_id| self.channel_choices_for_source(source_id))
            .unwrap_or_default();
        if self
            .correlation_reference_channel
            .as_ref()
            .is_none_or(|reference| {
                Some(reference.source_id) != reference_source
                    || !reference_choices.iter().any(|(item, _)| item == reference)
            })
        {
            self.correlation_reference_channel =
                reference_choices.first().map(|(item, _)| item.clone());
        }
        let sample_count = |reference: Option<&ChannelRef>| {
            reference.and_then(|reference| {
                self.datasets
                    .iter()
                    .find(|dataset| dataset.source_id == reference.source_id)
                    .and_then(|dataset| dataset.channel(reference.channel_id))
                    .map(|channel| channel.series.samples.len())
            })
        };
        let target_sample_count = sample_count(self.correlation_target_channel.as_ref());
        let reference_sample_count = sample_count(self.correlation_reference_channel.as_ref());

        let mut run = false;
        let mut apply = false;
        ui.collapsing("Correlate to another source", |ui| {
            ui.small("Estimate a time adjustment by matching two telemetry channels. Estimating never changes the project; use Apply candidate to persist the selected source offset.");
            let target_label = self
                .correlation_target_channel
                .as_ref()
                .and_then(|reference| target_choices.iter().find(|(item, _)| item == reference))
                .map(|(_, label)| label.as_str())
                .unwrap_or("Select target channel");
            ui.label("Target channel");
            egui::ComboBox::from_id_salt(("correlation-target-channel", target_source))
                .selected_text(target_label)
                .show_ui(ui, |ui| {
                    for (reference, label) in &target_choices {
                        if ui
                            .selectable_label(
                                self.correlation_target_channel.as_ref() == Some(reference),
                                label,
                            )
                            .clicked()
                        {
                            self.correlation_target_channel = Some(reference.clone());
                            self.invalidate_correlation();
                        }
                    }
                });

            let reference_source_label = reference_source
                .and_then(|source| reference_sources.iter().find(|(id, _)| *id == source))
                .map(|(_, name)| name.as_str())
                .unwrap_or("Select reference source");
            ui.label("Reference source");
            egui::ComboBox::from_id_salt(("correlation-reference-source", target_source))
                .selected_text(reference_source_label)
                .show_ui(ui, |ui| {
                    for (source, name) in &reference_sources {
                        if ui
                            .selectable_label(reference_source == Some(*source), name)
                            .clicked()
                        {
                            self.correlation_reference_source = Some(*source);
                            self.correlation_reference_channel = self
                                .channel_choices_for_source(*source)
                                .first()
                                .map(|(item, _)| item.clone());
                            self.invalidate_correlation();
                        }
                    }
                });
            let reference_label = self
                .correlation_reference_channel
                .as_ref()
                .and_then(|reference| reference_choices.iter().find(|(item, _)| item == reference))
                .map(|(_, label)| label.as_str())
                .unwrap_or("Select reference channel");
            ui.label("Reference channel");
            egui::ComboBox::from_id_salt(("correlation-reference-channel", target_source))
                .selected_text(reference_label)
                .show_ui(ui, |ui| {
                    for (reference, label) in &reference_choices {
                        if ui
                            .selectable_label(
                                self.correlation_reference_channel.as_ref() == Some(reference),
                                label,
                            )
                            .clicked()
                        {
                            self.correlation_reference_channel = Some(reference.clone());
                            self.invalidate_correlation();
                        }
                    }
                });
            if target_sample_count.is_some_and(|count| count < 2) {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "The selected target channel has fewer than two samples; choose a recorded sensor channel.",
                );
            }
            if reference_sample_count.is_some_and(|count| count < 24) {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "The selected reference channel has fewer than 24 samples; choose a denser sensor channel.",
                );
            }
            ui.horizontal(|ui| {
                let min_changed = ui
                    .add(
                    egui::DragValue::new(&mut self.correlation_min_adjustment)
                        .speed(0.05)
                        .prefix("Adjustment from ")
                        .suffix(" s"),
                )
                    .changed();
                let max_changed = ui
                    .add(
                    egui::DragValue::new(&mut self.correlation_max_adjustment)
                        .speed(0.05)
                        .prefix("to ")
                        .suffix(" s"),
                )
                    .changed();
                if min_changed || max_changed {
                    self.invalidate_correlation();
                }
            });
            ui.small("The search window is relative to the target source's current alignment; negative adjustments are allowed.");
            ui.small("If this window extends beyond the two channels' available time spans, it is automatically capped to the portion that can overlap.");
            if ui
                .checkbox(
                &mut self.correlation_absolute,
                "Prefer strongest absolute correlation (also allows inverted signals)",
            )
                .changed()
            {
                self.invalidate_correlation();
            }
            ui.horizontal(|ui| {
                let ready = self.correlation_target_channel.is_some()
                    && self.correlation_reference_channel.is_some()
                    && self.correlation_min_adjustment <= self.correlation_max_adjustment
                    && target_sample_count.is_some_and(|count| count >= 2)
                    && reference_sample_count.is_some_and(|count| count >= 24);
                if ui
                    .add_enabled(
                        ready && !self.correlation_running,
                        egui::Button::new("Estimate correlation"),
                    )
                    .clicked()
                {
                    run = true;
                }
                if self.correlation_running {
                    ui.spinner();
                    ui.label("Estimating…");
                }
            });
            if let Some(estimate) = &self.correlation_result
                && estimate.target_source == target_source
                && self.correlation_target_channel.as_ref() == Some(&estimate.target_channel)
                && self.correlation_reference_source == Some(estimate.reference_source)
                && self.correlation_reference_channel.as_ref() == Some(&estimate.reference_channel)
            {
                ui.group(|ui| {
                    ui.label(format!(
                        "Candidate: current target {:+.3}s → {:+.3}s",
                        estimate.target_current_offset_seconds,
                        estimate.target_offset_seconds
                    ));
                    ui.label(format!(
                        "Adjustment {:+.3}s · raw lag {:+.3}s · reference offset {:+.3}s",
                        estimate.adjustment_seconds,
                        estimate.raw_lag_seconds,
                        estimate.reference_offset_seconds
                    ));
                    ui.label(format!(
                        "Pearson r {:+.3} · {:.2}s overlap · {} samples · {:.3}s resolution",
                        estimate.coefficient,
                        estimate.overlap_seconds,
                        estimate.samples,
                        estimate.resolution_seconds
                    ));
                    if ui.button("Apply candidate offset").clicked() {
                        apply = true;
                    }
                });
            }
        });
        if run {
            self.start_correlation(target_source);
        }
        if apply {
            self.apply_correlation_candidate(target_source);
        }
    }

    fn channel_choices_for_source(&self, source_id: SourceId) -> Vec<(ChannelRef, String)> {
        self.datasets
            .iter()
            .find(|dataset| dataset.source_id == source_id)
            .map(|dataset| {
                let mut channels = dataset.channels.values().collect::<Vec<_>>();
                // Avoid arbitrary UUID-map ordering: dense continuous traces
                // are the safest correlation defaults, while sparse lap
                // metadata remains available when explicitly selected.
                channels.sort_by(|left, right| {
                    let left_continuous =
                        left.descriptor.interpolation == overlay_core::Interpolation::Linear;
                    let right_continuous =
                        right.descriptor.interpolation == overlay_core::Interpolation::Linear;
                    right_continuous
                        .cmp(&left_continuous)
                        .then_with(|| right.series.samples.len().cmp(&left.series.samples.len()))
                        .then_with(|| left.descriptor.name.cmp(&right.descriptor.name))
                });
                channels
                    .into_iter()
                    .map(|channel| {
                        let unit = channel.descriptor.unit.symbol();
                        let name_and_unit = if unit.is_empty() {
                            channel.descriptor.name.clone()
                        } else {
                            format!("{} ({unit})", channel.descriptor.name)
                        };
                        let span = channel
                            .series
                            .samples
                            .first()
                            .zip(channel.series.samples.last())
                            .map_or(0.0, |(first, last)| last.time - first.time);
                        (
                            ChannelRef {
                                source_id,
                                channel_id: channel.descriptor.id,
                            },
                            format!(
                                "{name_and_unit} · {} samples · {span:.1} s",
                                channel.series.samples.len()
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn start_correlation(&mut self, target_source: SourceId) {
        let Some(target_channel) = self.correlation_target_channel.clone() else {
            return;
        };
        let Some(reference_channel) = self.correlation_reference_channel.clone() else {
            return;
        };
        let Some(reference_source) = self.correlation_reference_source else {
            return;
        };
        if target_channel.source_id != target_source
            || reference_channel.source_id != reference_source
            || target_source == reference_source
        {
            self.status = "Choose channels from two different loaded sources".into();
            return;
        }
        let Some(target) = self
            .datasets
            .iter()
            .find(|dataset| dataset.source_id == target_source)
            .and_then(|dataset| dataset.channel(target_channel.channel_id))
            .cloned()
        else {
            self.status = "Target channel is unavailable".into();
            return;
        };
        let Some(reference) = self
            .datasets
            .iter()
            .find(|dataset| dataset.source_id == reference_source)
            .and_then(|dataset| dataset.channel(reference_channel.channel_id))
            .cloned()
        else {
            self.status = "Reference channel is unavailable".into();
            return;
        };
        let offsets = self.source_offsets();
        let target_offset = offsets.get(&target_source).copied().unwrap_or(0.0);
        let reference_offset = offsets.get(&reference_source).copied().unwrap_or(0.0);
        let (min_lag, max_lag) = correlation_lag_window(
            target_offset,
            reference_offset,
            self.correlation_min_adjustment,
            self.correlation_max_adjustment,
        );
        let absolute = self.correlation_absolute;
        self.correlation_job = self.correlation_job.wrapping_add(1);
        let job = self.correlation_job;
        self.correlation_running = true;
        self.correlation_result = None;
        self.status = "Estimating telemetry correlation…".into();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = correlate_channel_series(
                &reference.series,
                &target.series,
                &CorrelationConfig {
                    min_lag_seconds: min_lag,
                    max_lag_seconds: max_lag,
                    use_absolute_correlation: absolute,
                    ..CorrelationConfig::default()
                },
            )
            .map(|result| {
                let (adjustment_seconds, target_offset_seconds) = correlation_candidate_offsets(
                    result.target_minus_reference_seconds,
                    target_offset,
                    reference_offset,
                );
                CorrelationEstimate {
                    target_source,
                    target_channel,
                    reference_source,
                    reference_channel,
                    raw_lag_seconds: result.target_minus_reference_seconds,
                    adjustment_seconds,
                    target_offset_seconds,
                    reference_offset_seconds: reference_offset,
                    target_current_offset_seconds: target_offset,
                    coefficient: result.correlation_coefficient,
                    samples: result.sample_count,
                    overlap_seconds: result.overlap_seconds,
                    resolution_seconds: result.lag_resolution_seconds,
                }
            })
            .map_err(|error| error.to_string());
            let _ = tx.send(WorkerEvent::CorrelationEstimated(job, result));
        });
    }

    fn apply_correlation_candidate(&mut self, target_source: SourceId) {
        let Some(estimate) = self.correlation_result.clone() else {
            return;
        };
        if estimate.target_source != target_source {
            return;
        }
        let offsets = self.source_offsets();
        let Some(reference_offset) = offsets.get(&estimate.reference_source).copied() else {
            self.status = "The reference source is no longer available; estimate again".into();
            self.invalidate_correlation();
            return;
        };
        let Some(current_target_offset) = offsets.get(&target_source).copied() else {
            self.status = "The target source is no longer available; estimate again".into();
            self.invalidate_correlation();
            return;
        };
        if (reference_offset - estimate.reference_offset_seconds).abs() > 1e-9
            || (current_target_offset - estimate.target_current_offset_seconds).abs() > 1e-9
        {
            self.status = "A source offset changed; estimate correlation again".into();
            self.invalidate_correlation();
            return;
        }
        let target_offset = reference_offset + estimate.raw_lag_seconds;
        if let Some(source) = self.project_mut().and_then(|project| {
            project
                .sources
                .iter_mut()
                .find(|source| source.id == target_source)
        }) {
            source.alignment.offset_seconds = target_offset;
            self.status = format!(
                "Applied correlation offset {target_offset:+.3}s (r {:+.3})",
                estimate.coefficient
            );
            self.correlation_result = None;
            self.refresh_overlay();
        }
    }

    fn widgets_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Widgets");
        ui.horizontal_wrapped(|ui| {
            for (label, kind) in [
                ("Number", "numeric"),
                ("Bar", "bar"),
                ("Gauge", "radial"),
                ("G meter", "xy_dot"),
                ("Tachometer", "tachometer"),
                ("Temperature", "temperature"),
                ("Lap timer", "lap_timer"),
                ("Delta", "delta"),
                ("Shift lights", "shift_lights"),
                ("Center bar", "center_bar"),
                ("Gear", "gear"),
                ("Track map", "track_map"),
            ] {
                if ui.button(format!("+ {label}")).clicked() {
                    self.add_widget(kind);
                }
            }
            if ui.button("+ MyChron dashboard").clicked() {
                self.add_mychron_dashboard();
            }
        });
        let widgets = self
            .project()
            .map(|p| p.widgets.clone())
            .unwrap_or_default();
        for (index, widget) in widgets.iter().enumerate() {
            let label = style_string(&widget.style, "label").unwrap_or_else(|| widget.kind.clone());
            if ui
                .selectable_label(self.selected_widget == Some(index), label)
                .clicked()
            {
                self.selected_widget = Some(index);
            }
        }
        let Some(index) = self.selected_widget else {
            return;
        };
        if index >= widgets.len() {
            return;
        }
        ui.separator();
        let slot_names: &[&str] = if widgets[index].kind == "xy_dot" {
            &["x", "y"]
        } else if widgets[index].kind == "track_map" {
            &["latitude", "longitude"]
        } else {
            &["value"]
        };
        let mut refresh = false;
        for slot in slot_names {
            let current = widgets[index].bindings.iter().find(|b| b.slot == *slot);
            let current_label = self.selected_channel_label(current);
            let choices = if widgets[index].kind == "track_map" {
                self.gps_channel_choices(slot)
            } else {
                self.channel_choices()
            };
            egui::ComboBox::from_label(format!("{slot} channel"))
                .selected_text(current_label)
                .show_ui(ui, |ui| {
                    for (reference, label) in &choices {
                        let selected = current.is_some_and(|b| b.channel == *reference);
                        if ui.selectable_label(selected, label).clicked() {
                            if widgets[index].kind == "track_map" {
                                self.bind_track_map_coordinate(index, slot, reference.clone());
                            } else {
                                self.bind_widget_channel(index, slot, reference.clone());
                            }
                            refresh = true;
                        }
                    }
                });
        }
        let descriptors: HashMap<_, _> = self
            .datasets
            .iter()
            .flat_map(|dataset| {
                dataset.channels.values().map(move |channel| {
                    (
                        (dataset.source_id, channel.descriptor.id),
                        channel.descriptor.clone(),
                    )
                })
            })
            .collect();
        let unit_context = (widgets[index].kind != "track_map")
            .then(|| {
                widgets[index].bindings.first().and_then(|binding| {
                    let descriptor = descriptors
                        .get(&(binding.channel.source_id, binding.channel.channel_id))?;
                    let current = binding
                        .display_unit
                        .clone()
                        .unwrap_or_else(|| descriptor.unit.clone());
                    Some((current, descriptor.unit.compatible_units()))
                })
            })
            .flatten();
        let mut requested_unit = None;
        if let Some((current, units)) = &unit_context {
            egui::ComboBox::from_id_salt(("widget-display-unit", widgets[index].id))
                .selected_text(format!("Display unit: {}", unit_name(current)))
                .show_ui(ui, |ui| {
                    for unit in units {
                        if ui
                            .selectable_label(unit == current, unit_name(unit))
                            .clicked()
                        {
                            requested_unit = Some(unit.clone());
                        }
                    }
                });
        }
        let mut capture_start = false;
        let mut capture_finish = false;
        let mut clear_start = false;
        let mut clear_finish = false;
        if let Some(widget) = self.project_mut().and_then(|p| p.widgets.get_mut(index)) {
            if let Some(unit) = requested_unit {
                retarget_widget_units(widget, &descriptors, &unit);
                refresh = true;
            }
            let filterable = |binding: &ChannelBinding| {
                descriptors
                    .get(&(binding.channel.source_id, binding.channel.channel_id))
                    .is_some_and(|descriptor| {
                        descriptor.interpolation == overlay_core::Interpolation::Linear
                    })
            };
            if widget.bindings.iter().any(filterable) {
                ui.small("Zero-phase, non-causal smoothing only for this widget's bound continuous inputs. The displayed cutoff is the final two-pass -3 dB point; enabled upstream filters cascade.");
                let mut enabled = widget
                    .bindings
                    .iter()
                    .filter(|binding| filterable(binding))
                    .any(|binding| binding.low_pass_hz.is_some());
                let mut cutoff = widget
                    .bindings
                    .iter()
                    .filter(|binding| filterable(binding))
                    .find_map(|binding| binding.low_pass_hz)
                    .unwrap_or(8.0);
                if ui.checkbox(&mut enabled, "Widget low-pass").changed() {
                    for binding in widget
                        .bindings
                        .iter_mut()
                        .filter(|binding| filterable(binding))
                    {
                        binding.low_pass_hz = enabled.then_some(cutoff);
                    }
                    refresh = true;
                }
                if enabled
                    && ui
                        .add(
                            egui::Slider::new(&mut cutoff, 0.5..=50.0)
                                .logarithmic(true)
                                .text("Widget cutoff Hz"),
                        )
                        .changed()
                {
                    for binding in widget
                        .bindings
                        .iter_mut()
                        .filter(|binding| filterable(binding))
                    {
                        binding.low_pass_hz = Some(cutoff);
                    }
                    refresh = true;
                }
            } else if widget.kind != "track_map" && !widget.bindings.is_empty() {
                ui.small("This widget uses discrete data; low-pass is not applicable.");
            }
            if widget.kind != "track_map" {
                for binding in &mut widget.bindings {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} transform", binding.slot));
                        refresh |= ui
                            .add(
                                egui::DragValue::new(&mut binding.scale)
                                    .speed(0.01)
                                    .prefix("× "),
                            )
                            .changed();
                        refresh |= ui.checkbox(&mut binding.invert, "Invert").changed();
                    });
                }
            }
            if widget.kind == "track_map" {
                ui.separator();
                ui.label("GPS track map");
                let mut mode = style_string(&widget.style, "track_mode")
                    .or_else(|| style_string(&widget.style, "mode"))
                    .unwrap_or_else(|| "auto".into());
                egui::ComboBox::from_id_salt(("track-map-mode", widget.id))
                    .selected_text(track_map_mode_label(&mode))
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("auto", "Auto"),
                            ("circuit", "Circuit"),
                            ("point_to_point", "Point-to-point"),
                        ] {
                            if ui.selectable_label(mode == value, label).clicked() {
                                mode = value.into();
                            }
                        }
                    });
                if style_string(&widget.style, "track_mode").as_deref() != Some(mode.as_str()) {
                    set_style(&mut widget.style, "track_mode", json!(mode));
                    refresh = true;
                }
                let mut lap = style_number(&widget.style, "lap").unwrap_or(0.0);
                ui.horizontal(|ui| {
                    ui.label("Lap (0 = automatic)");
                    if ui
                        .add(egui::DragValue::new(&mut lap).range(0.0..=999.0).speed(1.0))
                        .changed()
                    {
                        set_style(&mut widget.style, "lap", json!(lap.round()));
                        refresh = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "Start: {}",
                        coordinate_pair_label(&widget.style, "start")
                    ));
                    let capture = ui.button("Capture");
                    capture_start = capture.clicked();
                    capture.on_hover_text(
                        "Capture GPS at the current video playhead (including this source's timing offset)",
                    );
                    clear_start = ui.button("Clear").clicked();
                });
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "Finish: {}",
                        coordinate_pair_label(&widget.style, "finish")
                    ));
                    let capture = ui.button("Capture");
                    capture_finish = capture.clicked();
                    capture.on_hover_text(
                        "Capture GPS at the current video playhead (including this source's timing offset)",
                    );
                    clear_finish = ui.button("Clear").clicked();
                });
                let mut line_width = style_number(&widget.style, "line_width").unwrap_or(2.0);
                if ui
                    .add(egui::Slider::new(&mut line_width, 0.5..=8.0).text("Track line width"))
                    .changed()
                {
                    set_style(&mut widget.style, "line_width", json!(line_width));
                    refresh = true;
                }
                let mut rotation = style_number(&widget.style, "rotation_degrees").unwrap_or(0.0);
                if ui
                    .add(egui::Slider::new(&mut rotation, -180.0..=180.0).text("Map rotation °"))
                    .changed()
                {
                    set_style(&mut widget.style, "rotation_degrees", json!(rotation));
                    refresh = true;
                }
                let mut padding = style_number(&widget.style, "padding").unwrap_or(0.10);
                if ui
                    .add(egui::Slider::new(&mut padding, 0.0..=0.30).text("Map padding"))
                    .changed()
                {
                    set_style(&mut widget.style, "padding", json!(padding));
                    refresh = true;
                }
                let mut show_markers = style_bool(&widget.style, "show_markers").unwrap_or(true);
                if ui
                    .checkbox(&mut show_markers, "Show start/finish markers")
                    .changed()
                {
                    set_style(&mut widget.style, "show_markers", json!(show_markers));
                    refresh = true;
                }
                ui.small("Circuit mode keeps one representative lap; point-to-point uses the start and finish markers.");
            }
            let mut opacity = style_number(&widget.style, "opacity").unwrap_or(1.0);
            if ui
                .add(egui::Slider::new(&mut opacity, 0.0..=1.0).text("Foreground opacity"))
                .changed()
            {
                set_style(&mut widget.style, "opacity", json!(opacity));
                refresh = true;
            }
            let mut background_opacity =
                style_number(&widget.style, "background_opacity").unwrap_or(opacity);
            if ui
                .add(
                    egui::Slider::new(&mut background_opacity, 0.0..=1.0)
                        .text("Background opacity"),
                )
                .changed()
            {
                set_style(
                    &mut widget.style,
                    "background_opacity",
                    json!(background_opacity),
                );
                refresh = true;
            }
            let mut label = style_string(&widget.style, "label").unwrap_or_default();
            if ui.text_edit_singleline(&mut label).changed() {
                set_style(&mut widget.style, "label", json!(label));
                refresh = true;
            }
            if unit_context.is_none() && widget.kind != "track_map" {
                let mut unit = style_string(&widget.style, "unit").unwrap_or_default();
                if ui.text_edit_singleline(&mut unit).changed() {
                    set_style(&mut widget.style, "unit", json!(unit));
                    refresh = true;
                }
            }
            if widget.kind != "track_map" {
                let mut min = style_number(&widget.style, "min").unwrap_or(0.0);
                let mut max = style_number(&widget.style, "max").unwrap_or(100.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut min).speed(0.1).prefix("Min "))
                        .changed()
                    {
                        set_style(&mut widget.style, "min", json!(min));
                        refresh = true;
                    }
                    if ui
                        .add(egui::DragValue::new(&mut max).speed(0.1).prefix("Max "))
                        .changed()
                    {
                        set_style(&mut widget.style, "max", json!(max));
                        refresh = true;
                    }
                });
            }
            ui.label("Position and size (normalized)");
            ui.horizontal(|ui| {
                refresh |= ui
                    .add(
                        egui::DragValue::new(&mut widget.rect.x)
                            .speed(0.005)
                            .range(0.0..=1.0)
                            .prefix("x "),
                    )
                    .changed();
                refresh |= ui
                    .add(
                        egui::DragValue::new(&mut widget.rect.y)
                            .speed(0.005)
                            .range(0.0..=1.0)
                            .prefix("y "),
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                refresh |= ui
                    .add(
                        egui::DragValue::new(&mut widget.rect.width)
                            .speed(0.005)
                            .range(0.02..=1.0)
                            .prefix("w "),
                    )
                    .changed();
                refresh |= ui
                    .add(
                        egui::DragValue::new(&mut widget.rect.height)
                            .speed(0.005)
                            .range(0.02..=1.0)
                            .prefix("h "),
                    )
                    .changed();
            });
        }
        if widgets[index].kind == "track_map" {
            if capture_start {
                refresh |= self.capture_track_map_point(index, "start");
            }
            if capture_finish {
                refresh |= self.capture_track_map_point(index, "finish");
            }
            if clear_start {
                refresh |= self.clear_track_map_point(index, "start");
            }
            if clear_finish {
                refresh |= self.clear_track_map_point(index, "finish");
            }
        }
        if ui.button("Delete widget").clicked() {
            if let Some(project) = self.project_mut() {
                project.widgets.remove(index);
            }
            self.selected_widget = None;
            refresh = true;
        }
        if refresh {
            self.refresh_overlay();
        }
    }

    fn data_plot_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("Telemetry graph");
            ui.add(
                egui::Slider::new(&mut self.plot_window_seconds, 1.0..=120.0)
                    .logarithmic(true)
                    .text("Window s"),
            );
            ui.checkbox(&mut self.plot_filter_enabled, "Preview low-pass");
            ui.add_enabled(
                self.plot_filter_enabled,
                egui::Slider::new(&mut self.plot_filter_hz, 0.5..=50.0)
                    .logarithmic(true)
                    .text("Hz"),
            );
            ui.checkbox(&mut self.plot_robust_scale, "Ignore extreme 1% for scale");
        });
        ui.small("Zero-phase, non-causal preview only; it does not modify source data or widgets. The displayed cutoff is the final two-pass -3 dB point.");
        let mut time = self.current_time;
        if ui
            .add(egui::Slider::new(&mut time, 0.0..=self.duration()).text("Video time"))
            .changed()
        {
            self.current_time = time;
            self.stop_playback();
            self.request_preview();
            self.refresh_overlay();
        }
        let half = self.plot_window_seconds * 0.5;
        let start = (self.current_time - half).max(0.0);
        let end = (start + self.plot_window_seconds).min(self.duration());
        let offsets = self.source_offsets();
        let mut plotted: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
        for reference in &self.plot_channels {
            let Some(dataset) = self
                .datasets
                .iter()
                .find(|dataset| dataset.source_id == reference.source_id)
            else {
                continue;
            };
            let Some(channel) = dataset.channel(reference.channel_id) else {
                continue;
            };
            let offset = offsets.get(&reference.source_id).copied().unwrap_or(0.0);
            let source_start = start + offset;
            let source_end = end + offset;
            let first = channel
                .series
                .samples
                .partition_point(|sample| sample.time < source_start);
            let last = channel
                .series
                .samples
                .partition_point(|sample| sample.time <= source_end);
            let slice = &channel.series.samples[first..last];
            if slice.is_empty() {
                continue;
            }
            let mut values = slice.iter().map(|sample| sample.value).collect::<Vec<_>>();
            if self.plot_filter_enabled && slice.len() > 2 {
                let elapsed = slice.last().unwrap().time - slice.first().unwrap().time;
                let rate = (slice.len() - 1) as f64 / elapsed.max(1e-9);
                values = zero_phase_low_pass(&values, rate, self.plot_filter_hz);
            }
            let display_unit = Unit::default_for(&channel.descriptor.quantity, self.unit_system)
                .filter(|unit| channel.descriptor.unit.is_compatible_with(unit))
                .unwrap_or_else(|| channel.descriptor.unit.clone());
            if display_unit != channel.descriptor.unit {
                values.iter_mut().for_each(|value| {
                    if let Ok(converted) = channel
                        .descriptor
                        .unit
                        .convert_value_to(*value, &display_unit)
                    {
                        *value = converted;
                    }
                });
            }
            let stride = (slice.len() / 2_000).max(1);
            let points = slice
                .iter()
                .zip(values)
                .step_by(stride)
                .map(|(sample, value)| (sample.time - offset, value))
                .collect();
            plotted.push((
                format!("{} ({})", channel.descriptor.name, display_unit.symbol()),
                points,
            ));
        }
        if plotted.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("Select one or more channels under Data sources → Plot channels.");
            });
            return;
        }
        let colors = [
            egui::Color32::from_rgb(0, 218, 255),
            egui::Color32::from_rgb(255, 174, 0),
            egui::Color32::from_rgb(255, 80, 135),
            egui::Color32::from_rgb(95, 225, 105),
            egui::Color32::from_rgb(180, 130, 255),
            egui::Color32::from_rgb(255, 245, 100),
        ];
        ui.horizontal_wrapped(|ui| {
            for (index, (name, points)) in plotted.iter().enumerate() {
                let min = points
                    .iter()
                    .map(|point| point.1)
                    .fold(f64::INFINITY, f64::min);
                let max = points
                    .iter()
                    .map(|point| point.1)
                    .fold(f64::NEG_INFINITY, f64::max);
                ui.colored_label(
                    colors[index % colors.len()],
                    format!("{name}: {min:.3}…{max:.3}"),
                );
            }
        });
        let mut range_values = plotted
            .iter()
            .flat_map(|(_, points)| points.iter().map(|point| point.1))
            .filter(|value| value.is_finite())
            .collect::<Vec<_>>();
        range_values.sort_by(f64::total_cmp);
        let (mut y_min, mut y_max) = if self.plot_robust_scale && range_values.len() >= 100 {
            (
                range_values[range_values.len() / 100],
                range_values[range_values.len() * 99 / 100],
            )
        } else {
            (
                *range_values.first().unwrap(),
                *range_values.last().unwrap(),
            )
        };
        if (y_max - y_min).abs() < 1e-12 {
            y_min -= 1.0;
            y_max += 1.0;
        } else {
            let margin = (y_max - y_min) * 0.08;
            y_min -= margin;
            y_max += margin;
        }
        let desired = egui::vec2(ui.available_width(), ui.available_height().max(260.0));
        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 4.0, egui::Color32::from_rgb(8, 12, 18));
        for index in 0..=5 {
            let fraction = index as f32 / 5.0;
            let y = egui::lerp(rect.bottom()..=rect.top(), fraction);
            ui.painter().line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0, egui::Color32::from_gray(45)),
            );
            let value = y_min + (y_max - y_min) * fraction as f64;
            ui.painter().text(
                egui::pos2(rect.left() + 4.0, y - 2.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{value:.2}"),
                egui::FontId::monospace(11.0),
                egui::Color32::GRAY,
            );
        }
        for index in 0..=10 {
            let x = egui::lerp(rect.left()..=rect.right(), index as f32 / 10.0);
            ui.painter().line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0, egui::Color32::from_gray(32)),
            );
        }
        for (index, (_, points)) in plotted.iter().enumerate() {
            let screen = points
                .iter()
                .filter(|point| point.1.is_finite())
                .map(|point| {
                    let x = egui::remap_clamp(
                        point.0 as f32,
                        start as f32..=end as f32,
                        rect.left()..=rect.right(),
                    );
                    let y = egui::remap_clamp(
                        point.1 as f32,
                        y_min as f32..=y_max as f32,
                        rect.bottom()..=rect.top(),
                    );
                    egui::pos2(x, y)
                })
                .collect::<Vec<_>>();
            ui.painter().add(egui::Shape::line(
                screen,
                egui::Stroke::new(1.6, colors[index % colors.len()]),
            ));
        }
        let playhead = egui::remap_clamp(
            self.current_time as f32,
            start as f32..=end as f32,
            rect.left()..=rect.right(),
        );
        ui.painter().line_segment(
            [
                egui::pos2(playhead, rect.top()),
                egui::pos2(playhead, rect.bottom()),
            ],
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
    }

    fn preview_ui(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let aspect = PREVIEW_W as f32 / PREVIEW_H as f32;
        let mut width = available.x;
        let mut height = width / aspect;
        if height > available.y - 60.0 {
            height = (available.y - 60.0).max(120.0);
            width = height * aspect;
        }
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        if let Some(texture) = &self.video_texture {
            ui.painter()
                .image(texture.id(), rect, uv, egui::Color32::WHITE);
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Video preview",
                egui::FontId::proportional(22.0),
                egui::Color32::GRAY,
            );
        }
        if let Some(texture) = &self.overlay_texture {
            ui.painter()
                .image(texture.id(), rect, uv, egui::Color32::WHITE);
        }
        if let Some(index) = self.selected_widget
            && let Some(widget) = self.project().and_then(|p| p.widgets.get(index))
        {
            let wr = normalized_to_screen(widget.rect, rect);
            ui.painter().rect_stroke(
                wr,
                2.0,
                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                egui::StrokeKind::Inside,
            );
            let handle = egui::Rect::from_center_size(wr.right_bottom(), egui::vec2(14.0, 14.0));
            ui.painter().rect_filled(handle, 2.0, egui::Color32::YELLOW);
        }
        if (response.drag_started() || response.clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let n = egui::pos2(
                (pos.x - rect.left()) / rect.width(),
                (pos.y - rect.top()) / rect.height(),
            );
            let hit = self.project().and_then(|p| {
                p.widgets
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, w)| point_in(n, w.rect))
                    .map(|(i, _)| i)
            });
            self.selected_widget = hit;
            self.resizing_widget = hit
                .and_then(|i| self.project().and_then(|p| p.widgets.get(i)))
                .is_some_and(|w| {
                    (n.x - (w.rect.x + w.rect.width)).abs() < 0.03
                        && (n.y - (w.rect.y + w.rect.height)).abs() < 0.05
                });
        }
        if response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            let nd = egui::vec2(delta.x / rect.width(), delta.y / rect.height());
            let resizing = self.resizing_widget;
            if let Some(index) = self.selected_widget
                && let Some(widget) = self.project_mut().and_then(|p| p.widgets.get_mut(index))
            {
                if resizing {
                    widget.rect.width = (widget.rect.width + nd.x).clamp(0.02, 1.0 - widget.rect.x);
                    widget.rect.height =
                        (widget.rect.height + nd.y).clamp(0.02, 1.0 - widget.rect.y);
                } else {
                    widget.rect.x = (widget.rect.x + nd.x).clamp(0.0, 1.0 - widget.rect.width);
                    widget.rect.y = (widget.rect.y + nd.y).clamp(0.0, 1.0 - widget.rect.height);
                }
                self.refresh_overlay();
            }
        }
        ui.horizontal(|ui| {
            if ui.button(if self.playing { "⏸" } else { "▶" }).clicked() {
                if self.playing {
                    self.stop_playback();
                    self.request_preview();
                } else {
                    self.start_playback();
                }
            }
            let mut time = self.current_time;
            if ui
                .add(egui::Slider::new(&mut time, 0.0..=self.duration()).show_value(false))
                .changed()
            {
                self.current_time = time;
                self.stop_playback();
                self.request_preview();
                self.refresh_overlay();
            }
            ui.monospace(format!(
                "{} / {}",
                format_time(self.current_time),
                format_time(self.duration())
            ));
        });
    }
}

impl eframe::App for RaceOverlayApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(
            UNIT_SYSTEM_STORAGE_KEY,
            match self.unit_system {
                UnitSystem::Metric => "metric",
                UnitSystem::Imperial => "imperial",
            }
            .into(),
        );
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_workers(&ctx);
        if let Some(image) = self.pending_overlay_image.take() {
            self.overlay_texture =
                Some(ctx.load_texture("telemetry-overlay", image, egui::TextureOptions::LINEAR));
        }
        if self.playing && self.current_time >= self.duration() {
            self.stop_playback();
        }
        egui::Panel::top("toolbar").show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                if let Some(error) = &self.tools_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("FFmpeg: {error}"));
                }
            });
        });
        egui::Panel::left("sources")
            .resizable(true)
            .default_size(270.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.sources_panel(ui));
            });
        egui::Panel::right("widgets")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.widgets_panel(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| {
            if self.show_data_plot {
                self.data_plot_ui(ui);
            } else {
                self.preview_ui(ui);
            }
        });
        self.export_dialog(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

fn default_widgets_for(system: UnitSystem) -> Vec<WidgetConfig> {
    let mut widgets = vec![
        make_widget("xy_dot", 0),
        make_widget("radial", 1),
        make_widget("bar", 2),
    ];
    for widget in &mut widgets {
        apply_unbound_widget_unit_default(widget, system);
    }
    widgets
}

fn apply_unbound_widget_unit_default(widget: &mut WidgetConfig, system: UnitSystem) {
    let (from, target) = match (widget.kind.as_str(), system) {
        ("radial", UnitSystem::Metric) => (Unit::MilePerHour, Unit::KilometerPerHour),
        ("radial", UnitSystem::Imperial) => (Unit::KilometerPerHour, Unit::MilePerHour),
        ("temperature", UnitSystem::Metric) => (Unit::Fahrenheit, Unit::Celsius),
        ("temperature", UnitSystem::Imperial) => (Unit::Celsius, Unit::Fahrenheit),
        _ => return,
    };
    if style_string(&widget.style, "unit").as_deref() != Some(from.symbol()) {
        return;
    }
    if let Some((scale, offset)) = unit_affine(&from, &target)
        && let (Some(min), Some(max)) = (
            style_number(&widget.style, "min"),
            style_number(&widget.style, "max"),
        )
    {
        let converted_min = min * scale + offset;
        let converted_max = max * scale + offset;
        set_style(
            &mut widget.style,
            "min",
            json!(converted_min.min(converted_max)),
        );
        set_style(
            &mut widget.style,
            "max",
            json!(converted_min.max(converted_max)),
        );
    }
    set_style(&mut widget.style, "unit", json!(target.symbol()));
}

fn preferred_channels(kind: &str) -> &'static [&'static str] {
    match kind {
        "radial" => &["speed", "gps_speed"],
        "bar" | "tachometer" | "shift_lights" => &["rpm"],
        "temperature" => &["water_temperature", "exhaust_temperature"],
        "lap_timer" => &["lap_time"],
        "delta" => &["best_today_diff", "predictive_time"],
        "center_bar" => &["steering_angle"],
        "gear" => &["gear"],
        _ => &["gear"],
    }
}

fn is_latitude_channel(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "latitude"
        || name == "lat"
        || name == "gps_lat"
        || name == "gps_latitude"
        || name.ends_with("_latitude")
}

fn is_longitude_channel(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "longitude"
        || name == "lon"
        || name == "lng"
        || name == "gps_lon"
        || name == "gps_lng"
        || name == "gps_longitude"
        || name.ends_with("_longitude")
}

fn select_gps_coordinate_channels(
    datasets: &[TelemetryDataset],
) -> Option<(ChannelRef, ChannelRef)> {
    datasets.iter().find_map(|dataset| {
        let latitude = dataset
            .channels
            .values()
            .filter(|channel| channel.descriptor.unit == Unit::Degree)
            .filter(|channel| is_latitude_channel(&channel.descriptor.name))
            .min_by_key(|channel| {
                if channel.descriptor.name == "gps_latitude" {
                    0
                } else if channel.descriptor.name == "latitude" {
                    1
                } else {
                    2
                }
            })?;
        let longitude = dataset
            .channels
            .values()
            .filter(|channel| channel.descriptor.unit == Unit::Degree)
            .filter(|channel| is_longitude_channel(&channel.descriptor.name))
            .min_by_key(|channel| {
                if channel.descriptor.name == "gps_longitude" {
                    0
                } else if channel.descriptor.name == "longitude" {
                    1
                } else {
                    2
                }
            })?;
        Some((
            ChannelRef {
                source_id: dataset.source_id,
                channel_id: latitude.descriptor.id,
            },
            ChannelRef {
                source_id: dataset.source_id,
                channel_id: longitude.descriptor.id,
            },
        ))
    })
}

fn coordinate_value_at(
    datasets: &[TelemetryDataset],
    offsets: &HashMap<SourceId, f64>,
    reference: &ChannelRef,
    video_time: f64,
) -> Option<f64> {
    let dataset = datasets
        .iter()
        .find(|dataset| dataset.source_id == reference.source_id)?;
    let channel = dataset.channel(reference.channel_id)?;
    let name = channel.descriptor.name.as_str();
    if channel.descriptor.unit != Unit::Degree
        || !(is_latitude_channel(name) || is_longitude_channel(name))
    {
        return None;
    }
    let source_time = video_time + offsets.get(&reference.source_id).copied().unwrap_or(0.0);
    channel
        .series
        .sample_at_default(source_time, channel.descriptor.interpolation)
        .filter(|value| value.is_finite())
}

fn track_map_mode_label(mode: &str) -> &'static str {
    match mode {
        "circuit" => "Circuit",
        "point_to_point" => "Point-to-point",
        _ => "Auto",
    }
}

fn widget_channel_binding(
    kind: &str,
    slot: &str,
    reference: ChannelRef,
    descriptor: Option<&ChannelDescriptor>,
    unit_system: UnitSystem,
) -> ChannelBinding {
    let mut binding = ChannelBinding::new(slot, reference);
    let Some(descriptor) = descriptor else {
        return binding;
    };
    if let Some(target) = default_display_unit(kind, descriptor, unit_system) {
        let _ = retarget_binding_unit(&mut binding, &descriptor.unit, &target);
    } else if kind == "delta"
        && descriptor.unit == Unit::Unitless
        && matches!(
            descriptor.name.as_str(),
            "best_today_diff" | "predictive_time"
        )
    {
        // A few AiM generations omit the unit tag for these millisecond
        // channels. Keep the known format quirk explicit and inspectable.
        binding.scale = 0.001;
        binding.display_unit = Some(Unit::Second);
    }
    binding
}

fn default_display_unit(
    kind: &str,
    descriptor: &ChannelDescriptor,
    system: UnitSystem,
) -> Option<Unit> {
    if matches!(kind, "lap_timer" | "delta") && descriptor.unit.is_compatible_with(&Unit::Second) {
        return Some(Unit::Second);
    }
    Unit::default_for(&descriptor.quantity, system)
        .filter(|target| descriptor.unit.is_compatible_with(target))
}

fn unit_affine(from: &Unit, to: &Unit) -> Option<(f64, f64)> {
    let zero = from.convert_value_to(0.0, to).ok()?;
    let one = from.convert_value_to(1.0, to).ok()?;
    Some((one - zero, zero))
}

fn retarget_binding_unit(binding: &mut ChannelBinding, source: &Unit, target: &Unit) -> bool {
    let current = binding.display_unit.as_ref().unwrap_or(source);
    let Some((scale, offset)) = unit_affine(current, target) else {
        return false;
    };
    binding.scale *= scale;
    binding.offset = binding.offset * scale + offset;
    binding.display_unit = Some(target.clone());
    true
}

fn retarget_widget_units(
    widget: &mut WidgetConfig,
    descriptors: &HashMap<(SourceId, overlay_core::ChannelId), ChannelDescriptor>,
    target: &Unit,
) {
    let range_transform = widget.bindings.first().and_then(|binding| {
        let descriptor =
            descriptors.get(&(binding.channel.source_id, binding.channel.channel_id))?;
        let current = binding.display_unit.as_ref().unwrap_or(&descriptor.unit);
        unit_affine(current, target)
    });
    for binding in &mut widget.bindings {
        let Some(descriptor) =
            descriptors.get(&(binding.channel.source_id, binding.channel.channel_id))
        else {
            continue;
        };
        retarget_binding_unit(binding, &descriptor.unit, target);
    }
    if let Some((scale, offset)) = range_transform {
        if let (Some(min), Some(max)) = (
            style_number(&widget.style, "min"),
            style_number(&widget.style, "max"),
        ) {
            let converted_min = min * scale + offset;
            let converted_max = max * scale + offset;
            set_style(
                &mut widget.style,
                "min",
                json!(converted_min.min(converted_max)),
            );
            set_style(
                &mut widget.style,
                "max",
                json!(converted_min.max(converted_max)),
            );
        }
        set_style(&mut widget.style, "unit", json!(target.symbol()));
    }
}

fn widget_suggested_range(kind: &str, name: &str, quantity: &Quantity, unit: &Unit) -> (f64, f64) {
    match kind {
        "temperature" if name == "exhaust_temperature" => {
            converted_range(0.0, 1000.0, &Unit::Celsius, unit)
        }
        "temperature" => converted_range(0.0, 120.0, &Unit::Celsius, unit),
        "delta" => converted_range(-10.0, 10.0, &Unit::Second, unit),
        "center_bar" => converted_range(-180.0, 180.0, &Unit::Degree, unit),
        "tachometer" | "shift_lights" => (0.0, 12_000.0),
        "lap_timer" => converted_range(0.0, 120.0, &Unit::Second, unit),
        _ => suggested_range(name, quantity, unit),
    }
}

fn converted_range(min: f64, max: f64, from: &Unit, to: &Unit) -> (f64, f64) {
    let Ok(min) = from.convert_value_to(min, to) else {
        return (min, max);
    };
    let Ok(max) = from.convert_value_to(max, to) else {
        return (min, max);
    };
    (min.min(max), min.max(max))
}

fn make_widget(kind: &str, index: usize) -> WidgetConfig {
    let (rect, label, unit, min, max) = match kind {
        "xy_dot" => (
            NormalizedRect::new(0.04, 0.63, 0.25, 0.30),
            "G FORCE",
            "g",
            -1.5,
            1.5,
        ),
        "radial" => (
            NormalizedRect::new(0.76, 0.65, 0.20, 0.28),
            "SPEED",
            "km/h",
            0.0,
            220.0,
        ),
        "bar" => (
            NormalizedRect::new(0.32, 0.83, 0.36, 0.10),
            "RPM",
            "rpm",
            0.0,
            10000.0,
        ),
        "tachometer" => (
            NormalizedRect::new(0.30, 0.68, 0.40, 0.22),
            "RPM",
            "rpm",
            0.0,
            12000.0,
        ),
        "temperature" => (
            NormalizedRect::new(0.04, 0.05, 0.20, 0.12),
            "TEMP",
            "°C",
            0.0,
            120.0,
        ),
        "lap_timer" => (
            NormalizedRect::new(0.72, 0.05, 0.24, 0.12),
            "LAP",
            "s",
            0.0,
            120.0,
        ),
        "delta" => (
            NormalizedRect::new(0.72, 0.19, 0.24, 0.12),
            "DELTA",
            "s",
            -10.0,
            10.0,
        ),
        "shift_lights" => (
            NormalizedRect::new(0.30, 0.61, 0.40, 0.08),
            "SHIFT",
            "rpm",
            0.0,
            12000.0,
        ),
        "center_bar" => (
            NormalizedRect::new(0.24, 0.84, 0.52, 0.10),
            "STEERING",
            "°",
            -180.0,
            180.0,
        ),
        "gear" => (
            NormalizedRect::new(0.46, 0.43, 0.08, 0.14),
            "GEAR",
            "",
            0.0,
            8.0,
        ),
        "track_map" => (
            NormalizedRect::new(0.68, 0.04, 0.28, 0.34),
            "TRACK MAP",
            "",
            0.0,
            1.0,
        ),
        _ => (
            NormalizedRect::new(0.04 + (index % 4) as f32 * 0.18, 0.05, 0.16, 0.12),
            "VALUE",
            "",
            0.0,
            100.0,
        ),
    };
    WidgetConfig {
        id: WidgetId::new(),
        kind: kind.into(),
        rect,
        bindings: vec![],
        style: json!({
            "opacity": 0.92,
            "background_opacity": 0.92,
            "background": [10, 15, 22, 210],
            "accent": [0, 218, 255, 255],
            "text": [255, 255, 255, 255],
            "label": label,
            "unit": unit,
            "min": min,
            "max": max,
            "format": match kind {
                "lap_timer" | "delta" => "{:.2}",
                "temperature" | "center_bar" => "{:.0}",
                _ => "{:.0}"
            },
            "track_mode": if kind == "track_map" { "auto" } else { "" },
            "lap": 0,
            "line_width": if kind == "track_map" { 2.0 } else { 0.0 },
            "rotation_degrees": 0.0,
            "padding": if kind == "track_map" { 0.10 } else { 0.0 },
            "show_markers": kind == "track_map",
        }),
        settings: json!({}),
        unknown: BTreeMap::new(),
    }
}

fn normalized_to_screen(r: NormalizedRect, parent: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            parent.left() + r.x * parent.width(),
            parent.top() + r.y * parent.height(),
        ),
        egui::vec2(r.width * parent.width(), r.height * parent.height()),
    )
}

fn point_in(p: egui::Pos2, r: NormalizedRect) -> bool {
    p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height
}

fn style_number(style: &Value, name: &str) -> Option<f64> {
    style.get(name).and_then(Value::as_f64)
}

fn style_bool(style: &Value, name: &str) -> Option<bool> {
    style.get(name).and_then(Value::as_bool)
}

fn coordinate_pair_label(style: &Value, which: &str) -> String {
    let lat = style_number(style, &format!("{which}_latitude"));
    let lon = style_number(style, &format!("{which}_longitude"));
    match (lat, lon) {
        (Some(lat), Some(lon)) => format!("{lat:.6}, {lon:.6}"),
        _ => "not set".into(),
    }
}

fn suggested_range(name: &str, quantity: &Quantity, unit: &Unit) -> (f64, f64) {
    match (quantity, unit) {
        (Quantity::Acceleration, Unit::StandardGravity) if name == "combined_g" => (0.0, 3.0),
        (Quantity::Acceleration, Unit::StandardGravity) => (-3.0, 3.0),
        (Quantity::Acceleration, Unit::MeterPerSecondSquared) => (-30.0, 30.0),
        (Quantity::Speed, Unit::KilometerPerHour) => (0.0, 250.0),
        (Quantity::Speed, Unit::MilePerHour) => (0.0, 160.0),
        (Quantity::Speed, _) => (0.0, 70.0),
        (Quantity::Rpm, _) => (0.0, 12_000.0),
        (Quantity::AngularVelocity, Unit::RadianPerSecond) => (-5.0, 5.0),
        (Quantity::AngularVelocity, _) => (-300.0, 300.0),
        (Quantity::LapTime, _) => (0.0, 120.0),
        _ => (0.0, 100.0),
    }
}

fn forward_axis_label(axis: usize) -> &'static str {
    match axis {
        0 => "+X points forward",
        1 => "−X points forward",
        2 => "+Y points forward",
        3 => "−Y points forward",
        4 => "+Z points forward",
        5 => "−Z points forward",
        _ => "+X points forward",
    }
}

fn forward_axis_vector(axis: usize) -> [f64; 3] {
    match axis {
        0 => [1.0, 0.0, 0.0],
        1 => [-1.0, 0.0, 0.0],
        2 => [0.0, 1.0, 0.0],
        3 => [0.0, -1.0, 0.0],
        4 => [0.0, 0.0, 1.0],
        5 => [0.0, 0.0, -1.0],
        _ => [1.0, 0.0, 0.0],
    }
}

fn dataset_duration(dataset: &TelemetryDataset) -> Option<f64> {
    let first = dataset
        .channels
        .values()
        .filter_map(|channel| channel.series.samples.first().map(|sample| sample.time))
        .min_by(f64::total_cmp)?;
    let last = dataset
        .channels
        .values()
        .filter_map(|channel| channel.series.samples.last().map(|sample| sample.time))
        .max_by(f64::total_cmp)?;
    Some((last - first).max(0.0))
}

/// Convert a user-facing adjustment window around the target's current
/// alignment into the raw target-minus-reference lag searched by the core.
fn correlation_lag_window(
    target_offset: f64,
    reference_offset: f64,
    first_adjustment: f64,
    second_adjustment: f64,
) -> (f64, f64) {
    let current_lag = target_offset - reference_offset;
    (
        current_lag + first_adjustment.min(second_adjustment),
        current_lag + first_adjustment.max(second_adjustment),
    )
}

/// Translate a raw lag into the adjustment and resulting target offset shown
/// in the estimate UI.
fn correlation_candidate_offsets(
    raw_lag: f64,
    target_offset: f64,
    reference_offset: f64,
) -> (f64, f64) {
    let resulting_target_offset = reference_offset + raw_lag;
    (
        resulting_target_offset - target_offset,
        resulting_target_offset,
    )
}

const SOURCE_LOW_PASS_ENABLED: &str = "low_pass_enabled";
const SOURCE_LOW_PASS_HZ: &str = "low_pass_hz";

fn source_low_pass_settings(settings: &Value) -> (bool, f64) {
    let enabled = settings
        .get(SOURCE_LOW_PASS_ENABLED)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cutoff = settings
        .get(SOURCE_LOW_PASS_HZ)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(8.0)
        .clamp(0.5, 50.0);
    (enabled, cutoff)
}

fn set_source_low_pass_settings(settings: &mut Value, enabled: bool, cutoff_hz: f64) {
    if !settings.is_object() {
        *settings = json!({});
    }
    let object = settings
        .as_object_mut()
        .expect("source settings normalized to an object");
    object.insert(SOURCE_LOW_PASS_ENABLED.into(), json!(enabled));
    object.insert(SOURCE_LOW_PASS_HZ.into(), json!(cutoff_hz.clamp(0.5, 50.0)));
}

fn apply_low_pass_to_dataset(
    dataset: &mut TelemetryDataset,
    cutoff_hz: f64,
) -> Result<usize, String> {
    let mut filtered = 0;
    for channel in dataset.channels.values_mut() {
        if channel.descriptor.interpolation != overlay_core::Interpolation::Linear
            || channel.series.samples.len() < 2
        {
            continue;
        }
        channel.series = channel
            .series
            .low_pass_hz(cutoff_hz)
            .map_err(|error| error.to_string())?;
        filtered += 1;
    }
    Ok(filtered)
}

/// Apply source conditioning before deriving camera-frame channels. Keeping
/// this order in one helper prevents source reload and recalibration actions
/// from producing different filter cascades.
fn prepare_loaded_dataset(
    dataset: &mut TelemetryDataset,
    source_cutoff_hz: Option<f64>,
    camera_calibration: Option<&CameraCalibration>,
) -> Result<usize, String> {
    let filtered_channels = match source_cutoff_hz {
        Some(cutoff_hz) => apply_low_pass_to_dataset(dataset, cutoff_hz)?,
        None => 0,
    };
    if let Some(calibration) = camera_calibration {
        add_derived_inertial_channels(dataset, calibration, INSTA360_IMU_SAMPLE_RATE_HZ);
    }
    Ok(filtered_channels)
}

fn remove_source_bindings(project: &mut ProjectV1, source_id: SourceId) -> usize {
    let mut removed = 0;
    for widget in &mut project.widgets {
        let before = widget.bindings.len();
        widget
            .bindings
            .retain(|binding| binding.channel.source_id != source_id);
        removed += before - widget.bindings.len();
    }
    removed
}

fn remove_source_from_project(project: &mut ProjectV1, source_id: SourceId) -> Option<usize> {
    let index = project
        .sources
        .iter()
        .position(|source| source.id == source_id)?;
    project.sources.remove(index);
    remove_source_bindings(project, source_id);
    Some(index)
}

fn imu_window(channels: &[&overlay_core::TelemetryChannel], start: f64, end: f64) -> Vec<[f64; 3]> {
    let count = channels
        .iter()
        .map(|channel| channel.series.samples.len())
        .min()
        .unwrap_or(0);
    (0..count)
        .filter_map(|index| {
            let time = channels[0].series.samples[index].time;
            (time >= start && time <= end).then(|| {
                [
                    channels[0].series.samples[index].value,
                    channels[1].series.samples[index].value,
                    channels[2].series.samples[index].value,
                ]
            })
        })
        .collect()
}

/// Returns acceleration motion in m/s² and absolute gyro motion in rad/s.
/// Gravity-magnitude error is included so constant cornering/acceleration is
/// not mistaken for a stationary interval merely because it is smooth.
fn imu_motion_metrics(accel: &[[f64; 3]], gyro: &[[f64; 3]]) -> Option<(f64, f64)> {
    let gravity = median_gravity(accel.iter().copied())?;
    let gravity_magnitude = gravity
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    let accel_variance = accel
        .iter()
        .map(|sample| {
            sample
                .iter()
                .zip(gravity)
                .map(|(value, center)| (value - center).powi(2))
                .sum::<f64>()
        })
        .sum::<f64>()
        / accel.len() as f64;
    let accel_motion = (accel_variance + (gravity_magnitude - 9.80665).powi(2)).sqrt();
    let gyro_motion = (gyro
        .iter()
        .flat_map(|sample| sample.iter())
        .map(|value| value * value)
        .sum::<f64>()
        / gyro.len().max(1) as f64)
        .sqrt();
    Some((accel_motion, gyro_motion))
}

fn quietest_imu_interval(
    accel: &[&overlay_core::TelemetryChannel],
    gyro: &[&overlay_core::TelemetryChannel],
    duration: f64,
) -> Option<(f64, f64)> {
    let first = accel
        .iter()
        .chain(gyro.iter())
        .filter_map(|channel| channel.series.samples.first().map(|sample| sample.time))
        .max_by(f64::total_cmp)?;
    let last = accel
        .iter()
        .chain(gyro.iter())
        .filter_map(|channel| channel.series.samples.last().map(|sample| sample.time))
        .min_by(f64::total_cmp)?;
    let duration = duration.clamp(1.0, (last - first).max(1.0));
    let step = (duration * 0.5).max(0.5);
    let mut candidate = first;
    let mut best: Option<(f64, f64)> = None;
    while candidate + duration <= last + f64::EPSILON {
        let accel_window = imu_window(accel, candidate, candidate + duration);
        let gyro_window = imu_window(gyro, candidate, candidate + duration);
        if accel_window.len() >= 20
            && gyro_window.len() >= 20
            && let Some((accel_motion, gyro_motion)) =
                imu_motion_metrics(&accel_window, &gyro_window)
        {
            let score = accel_motion + gyro_motion * (1.0 / 0.15);
            if best.is_none_or(|(_, best_score)| score < best_score) {
                best = Some((candidate, score));
            }
        }
        candidate += step;
    }
    best.map(|(start, _)| (start, start + duration))
}

fn style_string(style: &Value, name: &str) -> Option<String> {
    style.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn parse_unit_system(value: &str) -> Option<UnitSystem> {
    match value {
        "metric" => Some(UnitSystem::Metric),
        "imperial" => Some(UnitSystem::Imperial),
        _ => None,
    }
}

fn unit_system_label(system: UnitSystem) -> &'static str {
    match system {
        UnitSystem::Metric => "Units: Metric",
        UnitSystem::Imperial => "Units: Imperial",
    }
}

fn unit_name(unit: &Unit) -> &str {
    let symbol = unit.symbol();
    if symbol.is_empty() {
        "unitless"
    } else {
        symbol
    }
}

fn set_style(style: &mut Value, name: &str, value: Value) {
    if !style.is_object() {
        *style = json!({});
    }
    style.as_object_mut().unwrap().insert(name.into(), value);
}

fn remove_style(style: &mut Value, name: &str) {
    if let Some(object) = style.as_object_mut() {
        object.remove(name);
    }
}

fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    format!("{:02}:{:05.2}", (seconds / 60.0) as u64, seconds % 60.0)
}

fn codec_display_name(codec: &str) -> &str {
    if codec.eq_ignore_ascii_case("hevc") || codec.eq_ignore_ascii_case("h265") {
        "H.265 / HEVC"
    } else if codec.eq_ignore_ascii_case("h264") || codec.eq_ignore_ascii_case("avc1") {
        "H.264 / AVC"
    } else {
        codec
    }
}

/// Convert normalized project widgets to a small transparent surface for
/// FFmpeg. Rendering only their union avoids allocating a 4K RGBA image for
/// every output frame.
fn export_surface(
    widgets: &[WidgetConfig],
    full: RenderSize,
) -> Option<(WidgetGeometry, Vec<WidgetConfig>)> {
    if widgets.is_empty() || full.width == 0 || full.height == 0 {
        return None;
    }
    let left = widgets
        .iter()
        .map(|w| (w.rect.x * full.width as f32).floor() as i32)
        .min()?;
    let top = widgets
        .iter()
        .map(|w| (w.rect.y * full.height as f32).floor() as i32)
        .min()?;
    let right = widgets
        .iter()
        .map(|w| ((w.rect.x + w.rect.width) * full.width as f32).ceil() as i32)
        .max()?;
    let bottom = widgets
        .iter()
        .map(|w| ((w.rect.y + w.rect.height) * full.height as f32).ceil() as i32)
        .max()?;
    let width = (right - left).max(1) as u32;
    let height = (bottom - top).max(1) as u32;
    let mut remapped = widgets.to_vec();
    for widget in &mut remapped {
        let x = widget.rect.x * full.width as f32 - left as f32;
        let y = widget.rect.y * full.height as f32 - top as f32;
        widget.rect = NormalizedRect::new(
            x / width as f32,
            y / height as f32,
            widget.rect.width * full.width as f32 / width as f32,
            widget.rect.height * full.height as f32 / height as f32,
        );
    }
    Some((WidgetGeometry::new(left, top, width, height), remapped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn imu_test_channel(name: &str, values: impl Fn(f64) -> f64) -> overlay_core::TelemetryChannel {
        let source_id = SourceId::new();
        overlay_core::TelemetryChannel {
            descriptor: overlay_core::ChannelDescriptor {
                id: overlay_core::ChannelId::for_source_name(source_id, name),
                name: name.into(),
                quantity: Quantity::Generic,
                unit: Unit::Unitless,
                interpolation: overlay_core::Interpolation::Linear,
                description: None,
            },
            series: overlay_core::ChannelSeries::new(
                (0..=60)
                    .map(|index| {
                        let time = f64::from(index) * 0.1;
                        overlay_core::TimedSample {
                            time,
                            value: values(time),
                        }
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn default_project_has_all_core_visuals() {
        let widgets = default_widgets_for(UnitSystem::Metric);
        assert!(widgets.iter().any(|w| w.kind == "xy_dot"));
        assert!(widgets.iter().any(|w| w.kind == "radial"));
        assert!(widgets.iter().any(|w| w.kind == "bar"));
        assert!(make_widget("numeric", 3).kind == "numeric");
        let imperial = default_widgets_for(UnitSystem::Imperial);
        let speed = imperial
            .iter()
            .find(|widget| widget.kind == "radial")
            .unwrap();
        assert_eq!(style_string(&speed.style, "unit").as_deref(), Some("mph"));
        assert!((style_number(&speed.style, "max").unwrap() - 136.701_7).abs() < 0.001);
    }

    #[test]
    fn gps_binding_selection_prefers_degree_channels_from_one_source() {
        let source_id = SourceId::new();
        let mut latitude = imu_test_channel("gps_latitude", |_| 40.0);
        latitude.descriptor.unit = Unit::Degree;
        latitude.descriptor.quantity = Quantity::Position;
        latitude.descriptor.id =
            overlay_core::ChannelId::for_source_name(source_id, "gps_latitude");
        let mut longitude = imu_test_channel("gps_longitude", |_| -73.0);
        longitude.descriptor.unit = Unit::Degree;
        longitude.descriptor.quantity = Quantity::Position;
        longitude.descriptor.id =
            overlay_core::ChannelId::for_source_name(source_id, "gps_longitude");
        let mut dataset = TelemetryDataset {
            source_id,
            ..Default::default()
        };
        dataset.insert(latitude);
        dataset.insert(longitude);
        let (lat, lon) = select_gps_coordinate_channels(&[dataset]).expect("GPS pair");
        assert_eq!(lat.source_id, source_id);
        assert_eq!(lon.source_id, source_id);
        assert_eq!(
            lat.channel_id,
            overlay_core::ChannelId::for_source_name(source_id, "gps_latitude")
        );
    }

    #[test]
    fn coordinate_capture_uses_video_time_plus_source_offset() {
        let source_id = SourceId::new();
        let mut latitude = imu_test_channel("gps_latitude", |time| 40.0 + time);
        latitude.descriptor.unit = Unit::Degree;
        latitude.descriptor.id =
            overlay_core::ChannelId::for_source_name(source_id, "gps_latitude");
        let reference = ChannelRef {
            source_id,
            channel_id: latitude.descriptor.id,
        };
        let mut dataset = TelemetryDataset {
            source_id,
            ..Default::default()
        };
        dataset.insert(latitude);
        let offsets = HashMap::from([(source_id, 0.5)]);
        assert!(
            (coordinate_value_at(&[dataset], &offsets, &reference, 1.0).unwrap() - 41.5).abs()
                < 1e-9
        );
    }

    #[test]
    fn source_low_pass_settings_round_trip_without_discarding_adapter_settings() {
        let mut settings = json!({"delimiter": ";"});
        set_source_low_pass_settings(&mut settings, true, 12.5);
        assert_eq!(settings["delimiter"], ";");
        assert_eq!(source_low_pass_settings(&settings), (true, 12.5));
    }

    #[test]
    fn correlation_adjustment_window_is_relative_to_current_alignment() {
        assert_eq!(correlation_lag_window(3.0, 1.25, -2.0, 4.0), (-0.25, 5.75));
        // Reversed values are normalized defensively, even though the UI
        // disables Estimate until the displayed bounds are ordered.
        assert_eq!(correlation_lag_window(3.0, 1.25, 4.0, -2.0), (-0.25, 5.75));
    }

    #[test]
    fn correlation_raw_lag_maps_to_reported_adjustment_and_offset() {
        let (adjustment, resulting_offset) = correlation_candidate_offsets(-0.4, 0.8, 1.1);
        assert!((resulting_offset - 0.7).abs() < 1e-12);
        assert!((adjustment + 0.1).abs() < 1e-12);
    }

    #[test]
    fn source_low_pass_filters_continuous_channels_only() {
        let source_id = SourceId::new();
        let mut continuous = imu_test_channel("continuous", |time| {
            if (time * 10.0) as i64 % 2 == 0 {
                0.0
            } else {
                10.0
            }
        });
        continuous.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, "c");
        let mut discrete = imu_test_channel("discrete", |time| (time * 10.0).round());
        discrete.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, "d");
        discrete.descriptor.interpolation = overlay_core::Interpolation::Hold;
        let discrete_before = discrete.series.clone();
        let mut dataset = TelemetryDataset {
            source_id,
            ..Default::default()
        };
        dataset.insert(continuous);
        dataset.insert(discrete);

        assert_eq!(apply_low_pass_to_dataset(&mut dataset, 1.0).unwrap(), 1);
        assert_eq!(
            dataset.named("discrete").expect("discrete channel").series,
            discrete_before
        );
        let values = &dataset
            .named("continuous")
            .expect("continuous channel")
            .series
            .samples;
        assert!(
            values
                .iter()
                .skip(1)
                .any(|sample| sample.value > 0.0 && sample.value < 10.0)
        );
    }

    #[test]
    fn camera_source_smoothing_has_stable_order_across_recalibration() {
        let source_id = SourceId::new();
        let mut dataset = TelemetryDataset {
            source_id,
            ..Default::default()
        };
        for (name, value) in [
            ("raw_accel_x", 2.0),
            ("raw_accel_y", 0.0),
            ("raw_accel_z", 9.80665),
            ("raw_gyro_x", 0.0),
            ("raw_gyro_y", 0.0),
            ("raw_gyro_z", 0.0),
        ] {
            let mut channel = imu_test_channel(name, |time| {
                value
                    + if name == "raw_accel_x" {
                        (time * 31.0).sin()
                    } else {
                        0.0
                    }
            });
            channel.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, name);
            dataset.insert(channel);
        }
        let calibration = CameraCalibration::default();
        assert_eq!(
            prepare_loaded_dataset(&mut dataset, Some(3.0), Some(&calibration)).unwrap(),
            6
        );
        let first = dataset.named("longitudinal_g").unwrap().series.clone();

        // Re-applying calibration derives from the already source-smoothed raw
        // channels and must reproduce the reload pipeline exactly.
        add_derived_inertial_channels(&mut dataset, &calibration, INSTA360_IMU_SAMPLE_RATE_HZ);
        assert_eq!(dataset.named("longitudinal_g").unwrap().series, first);
    }

    #[test]
    fn removing_source_from_project_removes_only_its_bindings() {
        let removed_id = SourceId::new();
        let kept_id = SourceId::new();
        let mut project = ProjectV1::new("video.mp4");
        project.sources = vec![
            SourceConfig {
                id: removed_id,
                name: "removed".into(),
                adapter: "generic_csv".into(),
                path: "removed.csv".into(),
                alignment: Default::default(),
                settings: Value::Null,
                unknown: BTreeMap::new(),
            },
            SourceConfig {
                id: kept_id,
                name: "kept".into(),
                adapter: "generic_csv".into(),
                path: "kept.csv".into(),
                alignment: Default::default(),
                settings: Value::Null,
                unknown: BTreeMap::new(),
            },
        ];
        let mut removed_binding = ChannelBinding::new(
            "value",
            ChannelRef {
                source_id: removed_id,
                channel_id: overlay_core::ChannelId::for_source_name(removed_id, "speed"),
            },
        );
        removed_binding.scale = 2.0;
        let kept_binding = ChannelBinding::new(
            "value",
            ChannelRef {
                source_id: kept_id,
                channel_id: overlay_core::ChannelId::for_source_name(kept_id, "speed"),
            },
        );
        let mut widget = make_widget("numeric", 0);
        widget.bindings = vec![removed_binding, kept_binding];
        project.widgets = vec![widget];

        assert_eq!(
            remove_source_from_project(&mut project, removed_id),
            Some(0)
        );
        assert_eq!(project.sources.len(), 1);
        assert_eq!(project.sources[0].id, kept_id);
        assert_eq!(project.widgets[0].bindings.len(), 1);
        assert_eq!(project.widgets[0].bindings[0].channel.source_id, kept_id);
    }

    #[test]
    fn data_widget_presets_have_distinct_visual_defaults() {
        for kind in [
            "tachometer",
            "temperature",
            "lap_timer",
            "delta",
            "shift_lights",
            "center_bar",
            "gear",
        ] {
            let widget = make_widget(kind, 0);
            assert_eq!(widget.kind, kind);
            assert!(widget.rect.width > 0.0 && widget.rect.height > 0.0);
            assert!(widget.style.get("label").and_then(Value::as_str).is_some());
            assert!(widget.style.get("format").and_then(Value::as_str).is_some());
        }
        assert_eq!(
            style_number(&make_widget("gear", 0).style, "background_opacity"),
            Some(0.92)
        );
        assert_eq!(
            preferred_channels("temperature"),
            &["water_temperature", "exhaust_temperature"]
        );
        assert_eq!(
            preferred_channels("delta"),
            &["best_today_diff", "predictive_time"]
        );
        assert_eq!(
            style_number(&make_widget("delta", 0).style, "min"),
            Some(-10.0)
        );
        assert_eq!(
            widget_suggested_range(
                "temperature",
                "exhaust_temperature",
                &Quantity::Temperature,
                &Unit::Celsius
            ),
            (0.0, 1000.0)
        );
        assert_eq!(
            widget_suggested_range(
                "center_bar",
                "steering_angle",
                &Quantity::Position,
                &Unit::Degree
            ),
            (-180.0, 180.0)
        );
    }

    #[test]
    fn widget_binding_presents_native_units_in_dashboard_units() {
        let source_id = SourceId::new();
        let reference = ChannelRef {
            source_id,
            channel_id: overlay_core::ChannelId::for_source_name(source_id, "gps_speed"),
        };
        let speed = ChannelDescriptor {
            id: reference.channel_id,
            name: "gps_speed".into(),
            quantity: Quantity::Speed,
            unit: Unit::MeterPerSecond,
            interpolation: overlay_core::Interpolation::Linear,
            description: None,
        };
        let binding = widget_channel_binding(
            "radial",
            "value",
            reference,
            Some(&speed),
            UnitSystem::Metric,
        );
        assert_eq!(binding.scale, 3.6);
        assert_eq!(binding.display_unit, Some(Unit::KilometerPerHour));

        let imperial = widget_channel_binding(
            "radial",
            "value",
            ChannelRef {
                source_id,
                channel_id: speed.id,
            },
            Some(&speed),
            UnitSystem::Imperial,
        );
        assert!((imperial.scale - 2.236_936_292_054_4).abs() < 1e-9);
        assert_eq!(imperial.display_unit, Some(Unit::MilePerHour));

        let reference = ChannelRef {
            source_id,
            channel_id: overlay_core::ChannelId::for_source_name(source_id, "best_today_diff"),
        };
        let delta = ChannelDescriptor {
            id: reference.channel_id,
            name: "best_today_diff".into(),
            quantity: Quantity::LapTime,
            unit: Unit::Millisecond,
            interpolation: overlay_core::Interpolation::Linear,
            description: None,
        };
        let binding = widget_channel_binding(
            "delta",
            "value",
            reference,
            Some(&delta),
            UnitSystem::Metric,
        );
        assert_eq!(binding.scale, 0.001);
        assert_eq!(binding.display_unit, Some(Unit::Second));

        let mut celsius = ChannelBinding::new(
            "value",
            ChannelRef {
                source_id,
                channel_id: speed.id,
            },
        );
        assert!(retarget_binding_unit(
            &mut celsius,
            &Unit::Celsius,
            &Unit::Fahrenheit
        ));
        assert!((celsius.apply(100.0) - 212.0).abs() < 1e-9);
        assert_eq!(
            widget_suggested_range(
                "temperature",
                "water_temperature",
                &Quantity::Temperature,
                &Unit::Fahrenheit,
            ),
            (32.0, 248.0)
        );
        assert_eq!(parse_unit_system("imperial"), Some(UnitSystem::Imperial));
    }

    #[test]
    fn export_surface_is_tight_and_preserves_pixel_geometry() {
        let widgets = vec![make_widget("numeric", 0)];
        let (geometry, mapped) = export_surface(&widgets, RenderSize::new(1000, 500)).unwrap();
        assert_eq!((geometry.x, geometry.y), (40, 25));
        assert_eq!((geometry.width, geometry.height), (160, 60));
        assert_eq!(mapped[0].rect.x, 0.0);
        assert_eq!(mapped[0].rect.y, 0.0);
        assert!((mapped[0].rect.width - 1.0).abs() < f32::EPSILON);
        assert!((mapped[0].rect.height - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn quiet_interval_detection_can_override_a_moving_video_start() {
        let accel_x = imu_test_channel("ax", |time| {
            if time < 3.0 {
                (time * 15.0).sin() * 2.0
            } else {
                0.0
            }
        });
        let accel_y = imu_test_channel("ay", |_| 0.0);
        let accel_z = imu_test_channel("az", |_| 9.80665);
        let gyro_x = imu_test_channel("gx", |time| if time < 3.0 { 0.5 } else { 0.0 });
        let gyro_y = imu_test_channel("gy", |_| 0.0);
        let gyro_z = imu_test_channel("gz", |_| 0.0);
        let interval = quietest_imu_interval(
            &[&accel_x, &accel_y, &accel_z],
            &[&gyro_x, &gyro_y, &gyro_z],
            2.0,
        )
        .unwrap();
        assert!(interval.0 >= 3.0, "selected {interval:?}");
        assert!(interval.1 <= 6.0);
    }

    #[test]
    #[ignore = "parses the supplied large Insta360 recording"]
    fn supplied_recording_has_racing_scale_g_after_alignment() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("LRV_20260830_124108_01_017.lrv");
        if !path.exists() {
            return;
        }
        let source_id = SourceId::new();
        let mut dataset = AdapterRegistry::with_builtins()
            .load("insta360", source_id, &path, &Value::Null)
            .unwrap();
        let (gravity, gyro_bias) = {
            let accel = ["raw_accel_x", "raw_accel_y", "raw_accel_z"]
                .map(|name| dataset.named(name).unwrap());
            let gyro =
                ["raw_gyro_x", "raw_gyro_y", "raw_gyro_z"].map(|name| dataset.named(name).unwrap());
            let (start, end) = quietest_imu_interval(&accel, &gyro, 2.0).unwrap();
            (
                median_gravity(imu_window(&accel, start, end)).unwrap(),
                median_gravity(imu_window(&gyro, start, end)).unwrap(),
            )
        };
        let rotation = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .find_map(|forward| {
                overlay_core::guided_sensor_to_vehicle_from_forward(
                    gravity, forward, 0.0, [false; 3],
                )
            })
            .unwrap();
        let gravity_magnitude = gravity
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        let correction = 1.0 - 9.80665 / gravity_magnitude;
        let calibration = CameraCalibration {
            sensor_to_vehicle: rotation,
            accelerometer_bias: gravity.map(|value| value * correction),
            gyroscope_bias: gyro_bias,
            low_pass_hz: 8.0,
            notes: None,
        };
        assert!(add_derived_inertial_channels(
            &mut dataset,
            &calibration,
            1_000.0
        ));
        // The supplied Studio export begins 79.0385 seconds into the raw file.
        let mut driving_g = dataset
            .named("combined_g")
            .unwrap()
            .series
            .samples
            .iter()
            .filter(|sample| (79.0385..=138.9).contains(&sample.time))
            .map(|sample| sample.value)
            .collect::<Vec<_>>();
        driving_g.sort_by(f64::total_cmp);
        let p99 = driving_g[driving_g.len() * 99 / 100];
        assert!((1.5..4.0).contains(&p99), "unexpected p99 G: {p99}");
    }
}
