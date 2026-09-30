//! Application lifecycle and project/workspace transitions.
use super::super::controllers::{
    ExportController, PlotEditor, PreviewController, SourceEditor, WidgetEditor, WorkerHub,
};
use super::super::policy::{appearance_for_preset, default_widgets_for, parse_unit_system};
use super::super::session::OverlaySession;
use super::super::{ExportCodecChoice, UNIT_SYSTEM_STORAGE_KEY};
use super::{OverlayEditor, OverlayOrigin, PanelAction, WorkerEvent};
use crossbeam_channel::unbounded;
use overlay_core::{
    AdapterRegistry, ProjectDocument, ProjectV1, SourceConfig, SourceId, SyntheticConfig,
};
use overlay_media::{
    AlignmentResult, FfmpegConfig, align_audio, discover, extract_mono_pcm, probe_video,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::thread;

impl OverlayEditor {
    pub(in crate::race_app) fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let (tools, tools_error) = match discover(&FfmpegConfig::default()) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e.to_string())),
        };
        let unit_system = cc
            .storage
            .and_then(|storage| storage.get_string(UNIT_SYSTEM_STORAGE_KEY))
            .as_deref()
            .and_then(parse_unit_system)
            .unwrap_or_default();
        Self::with_services(tools, tools_error, unit_system)
    }

    fn with_services(
        tools: Option<overlay_media::FfmpegTools>,
        tools_error: Option<String>,
        unit_system: overlay_core::UnitSystem,
    ) -> Self {
        let (tx, rx) = unbounded();
        Self {
            tools,
            tools_error,
            unit_system,
            origin: OverlayOrigin::Standalone,
            session: OverlaySession::new(),
            status: "Add telemetry in Analysis, or switch to Overlay to open a video.".into(),
            worker_hub: WorkerHub::new(tx, rx),
            source_editor: SourceEditor {
                selected_source: None,
                syncing_sources: vec![],
                calibration_start: 0.0,
                calibration_end: 2.0,
                calibration_source_time: false,
                calibration_auto_stationary: true,
                calibration_forward_axis: 0,
                calibration_roll_deg: 0.0,
                calibration_pitch_deg: 0.0,
                calibration_yaw_deg: 0.0,
                calibration_low_pass_hz: 8.0,
                remove_source_confirm: None,
                correlation_reference_source: None,
                correlation_target_channel: None,
                correlation_reference_channel: None,
                correlation_all_time: true,
                correlation_min_adjustment: -5.0,
                correlation_max_adjustment: 5.0,
                correlation_absolute: false,
                correlation_result: None,
                correlation_running: false,
                correlation_job: 0,
            },
            widget_editor: WidgetEditor {
                selected_widget: None,
                resizing_widget: false,
            },
            plot_editor: PlotEditor {
                show_data_plot: false,
                plot_channels: vec![],
                plot_window_seconds: 10.0,
                plot_filter_enabled: false,
                plot_filter_hz: 8.0,
                plot_robust_scale: true,
            },
            preview_controller: PreviewController {
                preview: None,
                preview_playback: None,
                video_texture: None,
                overlay_texture: None,
                pending_overlay_image: None,
                playing: false,
                reframing: false,
            },
            export_controller: ExportController {
                export_cancel: None,
                export_dialog_open: false,
                export_codec: ExportCodecChoice::MatchSource,
                export_match_bitrate: true,
                export_bitrate_mbps: 20.0,
                export_apple_compatible: true,
                export_fast_start: true,
                export_force_yuv420p: true,
                export_fraction: 0.0,
            },
        }
    }

    #[cfg(test)]
    pub(in crate::race_app) fn new_for_test() -> Self {
        Self::with_services(None, None, overlay_core::UnitSystem::Metric)
    }

    pub(in crate::race_app) fn project(&self) -> Option<&ProjectV1> {
        self.session.project().map(ProjectDocument::v1)
    }

    pub(in crate::race_app) fn open_analysis_overlay(&mut self, project: ProjectV1) -> bool {
        if project.video_path.as_os_str().is_empty() {
            self.status = "Attach a video to this recording before opening its overlay.".into();
            return false;
        }
        if let Err(error) = self.initialize_video(project.video_path.clone()) {
            self.status = error;
            return false;
        }
        self.invalidate_session_sources();
        self.session.clear_datasets();
        self.invalidate_correlation();
        self.session.set_project_path(None);
        let sources = project.sources.clone();
        self.session.set_project(project.into());
        self.request_preview();
        self.origin = OverlayOrigin::Analysis;
        self.apply_panel_action(PanelAction::ClearSourceSelection);
        self.apply_panel_action(PanelAction::ClearWidgetSelection);
        for source in sources {
            self.load_source_async(source);
        }
        self.status =
            "Overlay editor: changes return to this recording when you switch to Analysis.".into();
        true
    }

    pub(super) fn project_mut(&mut self) -> Option<&mut ProjectV1> {
        match self.session.project_mut()? {
            ProjectDocument::V1(p) => Some(p),
        }
    }

    fn invalidate_session_sources(&mut self) {
        let source_ids = self
            .project()
            .map(|project| {
                project
                    .sources
                    .iter()
                    .map(|source| source.id)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for source_id in source_ids {
            self.worker_hub.invalidate_source(source_id);
        }
    }

    /// Resolve UI selection identities only at the collection boundary. The
    /// rest of the editor carries stable IDs, so reordering a project does
    /// not silently retarget an edit to a different source or widget.
    pub(super) fn selected_source_index(&self) -> Option<usize> {
        let id = self.source_editor.selected_source?;
        self.project()?
            .sources
            .iter()
            .position(|source| source.id == id)
    }

    pub(super) fn selected_widget_index(&self) -> Option<usize> {
        let id = self.widget_editor.selected_widget?;
        self.project()?
            .widgets
            .iter()
            .position(|widget| widget.id == id)
    }

    /// Retire any in-flight estimate as well as the visible candidate. A
    /// worker may still finish, but its generation will no longer match.
    pub(super) fn invalidate_correlation(&mut self) {
        self.source_editor.correlation_job = self.worker_hub.next_correlation_generation();
        self.source_editor.correlation_running = false;
        self.source_editor.correlation_result = None;
    }

    pub(super) fn duration(&self) -> f64 {
        self.session
            .metadata()
            .and_then(|m| m.duration)
            .unwrap_or(1.0)
            .max(0.001)
    }

    pub(super) fn initialize_video(&mut self, path: PathBuf) -> Result<(), String> {
        self.stop_playback();
        let tools = self.tools().cloned().ok_or_else(|| {
            self.tools_error()
                .unwrap_or("FFmpeg unavailable")
                .to_owned()
        })?;
        let metadata = probe_video(&tools, &path).map_err(|e| e.to_string())?;
        self.export_controller.export_bitrate_mbps = metadata
            .bit_rate
            .map(|rate| rate as f64 / 1_000_000.0)
            .unwrap_or(20.0)
            .clamp(1.0, 200.0);
        self.export_controller.export_codec = ExportCodecChoice::MatchSource;
        self.export_controller.export_match_bitrate = true;
        self.preview_controller.preview = Some(crate::video_processing::VideoPreview::spawn(
            tools.clone(),
            path.clone(),
            crate::video_processing::default_processing(&path),
            metadata.fps(),
            true,
        ));
        self.session.set_metadata(metadata);
        self.preview_controller.video_texture = None;
        self.preview_controller.overlay_texture = None;
        self.session.set_current_time(0.0);
        self.request_preview();
        Ok(())
    }

    pub(super) fn open_video(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "insv"])
            .pick_file()
        else {
            return;
        };
        match self.initialize_video(path.clone()) {
            Ok(()) => {
                self.invalidate_session_sources();
                let processing = crate::video_processing::default_processing(&path);
                let mut p = ProjectV1::new(path);
                p.video_processing = processing;
                if p.video_processing.is_some() {
                    p.sources.push(SourceConfig {
                        id: SourceId::new(),
                        name: "Camera telemetry".into(),
                        adapter: "insta360".into(),
                        path: p.video_path.clone(),
                        alignment: Default::default(),
                        settings: json!({}),
                        unknown: BTreeMap::new(),
                    });
                }
                p.appearance = appearance_for_preset("race_dark");
                p.widgets = default_widgets_for(self.unit_system());
                self.session.set_project(p.into());
                self.origin = OverlayOrigin::Standalone;
                self.session.set_project_path(None);
                self.session.clear_datasets();
                self.invalidate_correlation();
                self.apply_panel_action(PanelAction::ClearSourceSelection);
                self.widget_editor.selected_widget = self
                    .project()
                    .and_then(|project| project.widgets.first().map(|widget| widget.id));
                if self.project().is_some_and(|p| p.video_processing.is_some()) {
                    for source in self.project().unwrap().sources.clone() {
                        self.load_source_async(source);
                    }
                    self.status = "Raw video opened; drag in Reframe mode to aim.".into();
                } else {
                    self.status =
                        "Video opened. Add the matching LRV/INSV or synthetic data.".into();
                }
                self.refresh_overlay();
            }
            Err(e) => self.status = e,
        }
    }

    pub(super) fn open_project(&mut self) {
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
                self.invalidate_session_sources();
                self.session.clear_datasets();
                self.invalidate_correlation();
                self.session.set_project_path(Some(path));
                self.session.set_project(doc);
                self.request_preview();
                self.origin = OverlayOrigin::Standalone;
                self.source_editor.calibration_low_pass_hz = self
                    .project()
                    .map(|project| project.camera_calibration.low_pass_hz)
                    .unwrap_or(8.0);
                self.apply_panel_action(PanelAction::ClearWidgetSelection);
                self.apply_panel_action(PanelAction::ClearSourceSelection);
                let sources = self.project().unwrap().sources.clone();
                for source in sources {
                    self.load_source_async(source);
                }
                self.status = "Project opened; loading telemetry…".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    pub(super) fn save_project(&mut self, save_as: bool) {
        let path = if !save_as {
            self.session.project_path().cloned()
        } else {
            None
        }
        .or_else(|| {
            rfd::FileDialog::new()
                .set_file_name("race.race-overlay.json")
                .save_file()
        });
        let (Some(path), Some(project)) = (path, self.session.project()) else {
            return;
        };
        match project.save_atomic(&path) {
            Ok(()) => {
                self.session.set_project_path(Some(path));
                self.status = "Project saved".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    pub(super) fn add_file_source(&mut self, adapter: &str) {
        if !self.session.has_project() {
            self.status = "Open a video before adding telemetry".into();
            return;
        }
        let filter = match adapter {
            "insta360" => ("Camera telemetry", vec!["lrv", "insv"]),
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
        self.source_editor.selected_source = Some(source.id);
        self.load_source_async(source);
    }

    pub(super) fn add_synthetic(&mut self) {
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
            self.source_editor.selected_source = Some(source.id);
            self.load_source_async(source);
        } else {
            self.status = "Open a video before adding telemetry".into();
        }
    }

    /// Remove a telemetry source and every reference owned by it. The video,
    /// project, and widgets themselves intentionally remain in place.
    pub(super) fn remove_source(&mut self, source_id: SourceId) {
        let Some(source) = self
            .project()
            .and_then(|project| project.sources.iter().find(|source| source.id == source_id))
            .cloned()
        else {
            self.source_editor.remove_source_confirm = None;
            return;
        };
        self.worker_hub.invalidate_source(source_id);
        let source_name = source.name;
        let source_index = self
            .project()
            .and_then(|project| project.sources.iter().position(|item| item.id == source_id));
        self.invalidate_correlation();
        if self.source_editor.correlation_reference_source == Some(source_id)
            || self
                .source_editor
                .correlation_target_channel
                .as_ref()
                .is_some_and(|channel| channel.source_id == source_id)
            || self
                .source_editor
                .correlation_reference_channel
                .as_ref()
                .is_some_and(|channel| channel.source_id == source_id)
        {
            self.source_editor.correlation_reference_source = None;
            self.source_editor.correlation_target_channel = None;
            self.source_editor.correlation_reference_channel = None;
        }

        debug_assert_eq!(self.session.remove_source(source_id), source_index);
        self.plot_editor
            .plot_channels
            .retain(|reference| reference.source_id != source_id);
        self.source_editor
            .syncing_sources
            .retain(|id| *id != source_id);
        self.source_editor.remove_source_confirm = None;
        if self.source_editor.selected_source == Some(source_id) {
            self.source_editor.selected_source = source_index.and_then(|index| {
                self.project().and_then(|project| {
                    (!project.sources.is_empty())
                        .then(|| project.sources[index.min(project.sources.len() - 1)].id)
                })
            });
        }
        self.status = format!("Removed telemetry source: {source_name}");
        self.refresh_overlay();
    }

    /// Reload from the original source so changing or disabling a filter never
    /// compounds a previously filtered dataset.
    pub(super) fn apply_source_low_pass(&mut self, source_id: SourceId) {
        let source = self.session.source(source_id).cloned();
        if let Some(source) = source {
            self.load_source_async(source);
        }
    }

    pub(super) fn load_source_async(&mut self, source: SourceConfig) {
        self.status = format!("Loading {}…", source.name);
        let generation = self.worker_hub.next_source_generation(source.id);
        let tx = self.worker_hub.sender();
        thread::spawn(move || {
            let registry = AdapterRegistry::with_builtins();
            let result = registry
                .load(&source.adapter, source.id, &source.path, &source.settings)
                .map_err(|e| e.to_string());
            let _ = tx.send(WorkerEvent::SourceLoaded(generation, source.id, result));
        });
    }

    pub(super) fn auto_sync(&mut self) {
        let Some(source_id) = self.source_editor.selected_source else {
            self.status = "Select a camera telemetry source first".into();
            return;
        };
        self.sync_source(source_id);
    }

    pub(super) fn sync_source(&mut self, source_id: SourceId) {
        if self.source_editor.syncing_sources.contains(&source_id) {
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
        let Some(tools) = self.tools().cloned() else {
            self.status = "FFmpeg is required for audio synchronization".into();
            return;
        };
        self.source_editor.syncing_sources.push(source_id);
        let generation = self.worker_hub.next_sync_generation(source_id);
        let tx = self.worker_hub.sender();
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
            let _ = tx.send(WorkerEvent::Synced(generation, source.id, result));
        });
    }
}
