//! Feature-local editor state. Controllers contain state only; orchestration
//! and persistence remain owned by `RaceOverlayApp` and `OverlaySession`.

use super::ExportCodecChoice;
use super::editor::{CorrelationEstimate, WorkerEvent};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use overlay_core::{ChannelRef, SourceId, WidgetId};
use overlay_media::CancelToken;
use std::collections::HashMap;

pub(super) struct WorkerHub {
    tx: Sender<WorkerEvent>,
    rx: Receiver<WorkerEvent>,
    source_generations: HashMap<SourceId, u64>,
    sync_generations: HashMap<SourceId, u64>,
    correlation_generation: u64,
}

impl WorkerHub {
    pub(super) fn new(tx: Sender<WorkerEvent>, rx: Receiver<WorkerEvent>) -> Self {
        Self {
            tx,
            rx,
            source_generations: HashMap::new(),
            sync_generations: HashMap::new(),
            correlation_generation: 0,
        }
    }

    pub(super) fn next_source_generation(&mut self, source_id: SourceId) -> u64 {
        let generation = self.source_generations.entry(source_id).or_insert(0);
        *generation = generation.wrapping_add(1);
        *generation
    }

    pub(super) fn source_generation_is_current(
        &self,
        source_id: SourceId,
        generation: u64,
    ) -> bool {
        self.source_generations.get(&source_id).copied() == Some(generation)
    }

    /// Retire all work associated with a source before it leaves the session.
    /// Incrementing both counters makes already-queued load and sync events
    /// harmless even if the source ID is later reused by a different session.
    pub(super) fn invalidate_source(&mut self, source_id: SourceId) {
        self.next_source_generation(source_id);
        self.next_sync_generation(source_id);
    }

    pub(super) fn sender(&self) -> Sender<WorkerEvent> {
        self.tx.clone()
    }

    pub(super) fn next_sync_generation(&mut self, source_id: SourceId) -> u64 {
        let generation = self.sync_generations.entry(source_id).or_insert(0);
        *generation = generation.wrapping_add(1);
        *generation
    }

    pub(super) fn sync_generation_is_current(&self, source_id: SourceId, generation: u64) -> bool {
        self.sync_generations.get(&source_id).copied() == Some(generation)
    }

    pub(super) fn next_correlation_generation(&mut self) -> u64 {
        self.correlation_generation = self.correlation_generation.wrapping_add(1);
        self.correlation_generation
    }

    pub(super) fn correlation_generation_is_current(&self, generation: u64) -> bool {
        self.correlation_generation == generation
    }

    pub(super) fn drain(&self) -> impl Iterator<Item = WorkerEvent> + '_ {
        self.rx.try_iter()
    }
}

pub(super) struct SourceEditor {
    pub(super) selected_source: Option<SourceId>,
    pub(super) syncing_sources: Vec<SourceId>,
    pub(super) calibration_start: f64,
    pub(super) calibration_end: f64,
    pub(super) calibration_source_time: bool,
    pub(super) calibration_auto_stationary: bool,
    pub(super) calibration_forward_axis: usize,
    pub(super) calibration_roll_deg: f64,
    pub(super) calibration_pitch_deg: f64,
    pub(super) calibration_yaw_deg: f64,
    pub(super) calibration_low_pass_hz: f64,
    pub(super) remove_source_confirm: Option<SourceId>,
    pub(super) correlation_reference_source: Option<SourceId>,
    pub(super) correlation_target_channel: Option<ChannelRef>,
    pub(super) correlation_reference_channel: Option<ChannelRef>,
    pub(super) correlation_all_time: bool,
    pub(super) correlation_min_adjustment: f64,
    pub(super) correlation_max_adjustment: f64,
    pub(super) correlation_absolute: bool,
    pub(super) correlation_result: Option<CorrelationEstimate>,
    pub(super) correlation_running: bool,
    pub(super) correlation_job: u64,
}

pub(super) struct WidgetEditor {
    pub(super) selected_widget: Option<WidgetId>,
    pub(super) resizing_widget: bool,
}

pub(super) struct PlotEditor {
    pub(super) show_data_plot: bool,
    pub(super) plot_channels: Vec<ChannelRef>,
    pub(super) plot_window_seconds: f64,
    pub(super) plot_filter_enabled: bool,
    pub(super) plot_filter_hz: f64,
    pub(super) plot_robust_scale: bool,
}

pub(super) struct PreviewController {
    pub(super) preview: Option<crate::video_processing::VideoPreview>,
    pub(super) preview_playback: Option<crate::video_processing::VideoPlayback>,
    pub(super) video_texture: Option<egui::TextureHandle>,
    pub(super) overlay_texture: Option<egui::TextureHandle>,
    pub(super) pending_overlay_image: Option<egui::ColorImage>,
    pub(super) playing: bool,
    pub(super) reframing: bool,
}

pub(super) struct ExportController {
    pub(super) export_cancel: Option<CancelToken>,
    pub(super) export_dialog_open: bool,
    pub(super) export_codec: ExportCodecChoice,
    pub(super) export_match_bitrate: bool,
    pub(super) export_bitrate_mbps: f64,
    pub(super) export_apple_compatible: bool,
    pub(super) export_fast_start: bool,
    pub(super) export_force_yuv420p: bool,
    pub(super) export_fraction: f32,
}
