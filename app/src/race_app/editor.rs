//! Overlay editor orchestration and owned feature state.
//!
//! `RaceOverlayApp` owns the workspace switch and delegates Overlay behavior
//! here. This editor privately owns the session, controllers, worker hub, and
//! shared media/unit preferences. Its child modules are responsibility-focused
//! implementations; they communicate across panel boundaries with
//! `PanelAction` rather than taking the application shell as a dependency.

use super::controllers::{
    ExportController, PlotEditor, PreviewController, SourceEditor, WidgetEditor, WorkerHub,
};
use super::session::OverlaySession;
use eframe::egui;
use overlay_core::{ChannelRef, SourceId, TelemetryDataset, UnitSystem, WidgetId};
use overlay_media::{AlignmentResult, ExportProgress, FfmpegTools};
use std::path::PathBuf;

pub(crate) enum WorkerEvent {
    SourceLoaded(u64, SourceId, Result<TelemetryDataset, String>),
    Synced(u64, SourceId, Result<AlignmentResult, String>),
    CorrelationEstimated(u64, Result<CorrelationEstimate, String>),
    ExportProgress(ExportProgress),
    ExportFinished(Result<PathBuf, String>),
}

#[derive(Clone, Debug)]
pub(crate) struct CorrelationEstimate {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum OverlayOrigin {
    #[default]
    Standalone,
    Analysis,
}

/// The Overlay workspace shell. It owns all overlay-only state and exposes
/// the small rendering/event boundary used by the top-level application.
pub(super) struct OverlayEditor {
    tools: Option<FfmpegTools>,
    tools_error: Option<String>,
    unit_system: UnitSystem,
    origin: OverlayOrigin,
    session: OverlaySession,
    status: String,
    worker_hub: WorkerHub,
    source_editor: SourceEditor,
    widget_editor: WidgetEditor,
    plot_editor: PlotEditor,
    preview_controller: PreviewController,
    export_controller: ExportController,
}

/// Narrow read-only toolbar state. Other panels use typed `PanelAction`
/// responses directly because their rendering state is already local to the
/// corresponding controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ToolbarView {
    pub(super) has_project: bool,
    pub(super) unit_system: UnitSystem,
    pub(super) show_data_plot: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PanelAction {
    SelectSource(SourceId),
    SelectWidget(WidgetId),
    ClearSourceSelection,
    ClearWidgetSelection,
    SetUnitSystem(UnitSystem),
    SetPlotVisible(bool),
    OpenExportDialog,
}

#[path = "correlation.rs"]
mod correlation;
#[path = "export.rs"]
mod export;
#[path = "lifecycle.rs"]
mod lifecycle;
#[path = "media.rs"]
mod media;
#[path = "plot.rs"]
mod plot;
#[path = "preview.rs"]
mod preview;
#[path = "source_panel.rs"]
mod source_panel;
#[path = "sources.rs"]
mod sources;
#[path = "toolbar.rs"]
mod toolbar;
#[path = "widget_panel.rs"]
mod widget_panel;
#[path = "widgets.rs"]
mod widgets;

impl OverlayEditor {
    pub(super) fn tools(&self) -> Option<&FfmpegTools> {
        self.tools.as_ref()
    }

    pub(super) fn tools_error(&self) -> Option<&str> {
        self.tools_error.as_deref()
    }

    pub(super) fn unit_system(&self) -> UnitSystem {
        self.unit_system
    }

    pub(super) fn set_unit_system(&mut self, unit_system: UnitSystem) {
        self.unit_system = unit_system;
    }

    pub(super) fn is_linked_to_analysis(&self) -> bool {
        self.origin == OverlayOrigin::Analysis
    }

    pub(super) fn status(&self) -> &str {
        &self.status
    }

    pub(super) fn export_running(&self) -> bool {
        self.export_controller.export_cancel.is_some()
    }

    pub(super) fn export_fraction(&self) -> f32 {
        self.export_controller.export_fraction
    }

    pub(super) fn cancel_export(&self) {
        if let Some(cancel) = &self.export_controller.export_cancel {
            cancel.cancel();
        }
    }

    pub(super) fn load_pending_overlay_texture(&mut self, ctx: &egui::Context) {
        if let Some(image) = self.preview_controller.pending_overlay_image.take() {
            self.preview_controller.overlay_texture =
                Some(ctx.load_texture("telemetry-overlay", image, egui::TextureOptions::LINEAR));
        }
    }

    pub(super) fn playback_finished(&self) -> bool {
        self.preview_controller.playing
            && (self.session.current_time() >= self.duration()
                || self
                    .preview_controller
                    .preview
                    .as_ref()
                    .is_some_and(|p| p.playback_finished()))
    }

    pub(super) fn plot_visible(&self) -> bool {
        self.plot_editor.show_data_plot
    }

    pub(super) fn toolbar_view(&self) -> ToolbarView {
        ToolbarView {
            has_project: self.session.has_project(),
            unit_system: self.unit_system(),
            show_data_plot: self.plot_editor.show_data_plot,
        }
    }

    #[cfg(test)]
    pub(super) fn correlation_searches_all_time(&self) -> bool {
        self.source_editor.correlation_all_time
    }

    pub(super) fn apply_panel_action(&mut self, action: PanelAction) {
        match action {
            PanelAction::SelectSource(id) => self.source_editor.selected_source = Some(id),
            PanelAction::SelectWidget(id) => self.widget_editor.selected_widget = Some(id),
            PanelAction::ClearSourceSelection => self.source_editor.selected_source = None,
            PanelAction::ClearWidgetSelection => self.widget_editor.selected_widget = None,
            PanelAction::SetUnitSystem(system) => {
                self.set_unit_system(system);
                self.apply_default_unit_system();
            }
            PanelAction::SetPlotVisible(visible) => self.plot_editor.show_data_plot = visible,
            PanelAction::OpenExportDialog => self.export_controller.export_dialog_open = true,
        }
    }
}
