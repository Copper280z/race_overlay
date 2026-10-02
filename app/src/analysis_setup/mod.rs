//! "Timing & course setup" panel. Work is grouped by task so that each tab
//! stays short: interval trimming, per-recording sources and sync, and GPS
//! course gates. Edits are collected in [`SetupOutcome`] and applied after the
//! panel has been drawn, because several need to touch workspace-wide state.

use super::*;
use crate::ui_kit::widgets;

mod gates;
mod intervals;
mod sources;

/// Which group of setup controls is showing. Runtime-only; not persisted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SetupTab {
    #[default]
    Intervals,
    Sources,
    Gates,
}

/// Deferred effects of one frame of setup editing.
#[derive(Default)]
pub(super) struct SetupOutcome {
    pub(super) changed: bool,
    pub(super) reload: Vec<SourceConfig>,
    pub(super) removed_sources: Vec<SourceId>,
    pub(super) add_source_to: Option<RecordingId>,
    pub(super) restore_intervals: bool,
    pub(super) intervals_replaced: bool,
}

impl AnalysisApp {
    pub(super) fn setup(&mut self, ui: &mut egui::Ui) {
        let mut outcome = SetupOutcome::default();
        widgets::segmented(
            ui,
            &mut self.setup_tab,
            &[
                (
                    SetupTab::Intervals,
                    "Intervals",
                    "Trim, add, and detect the run windows to compare",
                ),
                (
                    SetupTab::Sources,
                    "Sources & sync",
                    "Data files, filters, and video offsets for each recording",
                ),
                (
                    SetupTab::Gates,
                    "Course gates",
                    "Timing lines and manual course-position anchors",
                ),
            ],
        );
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match self.setup_tab {
                SetupTab::Intervals => self.setup_intervals(ui, &mut outcome),
                SetupTab::Sources => self.setup_sources(ui, &mut outcome),
                SetupTab::Gates => self.setup_gates(ui, &mut outcome),
            });
        self.apply_setup_outcome(outcome);
    }

    fn apply_setup_outcome(&mut self, outcome: SetupOutcome) {
        let SetupOutcome {
            mut changed,
            reload,
            removed_sources,
            add_source_to,
            restore_intervals,
            intervals_replaced,
        } = outcome;
        if intervals_replaced {
            self.retain_valid_selection();
        }
        if restore_intervals {
            self.restore_detected_intervals();
            changed = true;
        }
        for id in removed_sources {
            self.remove_source(id);
        }
        for source in reload {
            self.enqueue(source);
        }
        if let Some(recording_id) = add_source_to {
            self.attach_data_log_dialog(recording_id);
        }
        if changed {
            self.changed();
        }
    }

    /// Drops a source and every binding that referenced it; files are untouched.
    fn remove_source(&mut self, id: SourceId) {
        self.data.remove(&id);
        self.loading.remove(&id);
        let affected = self
            .workspace
            .recordings
            .iter()
            .filter(|recording| recording.sources.iter().any(|source| source.id == id))
            .map(|recording| recording.id)
            .collect::<Vec<_>>();
        for recording_id in affected {
            self.video_sync.remove_source(recording_id, id);
        }
        for recording in &mut self.workspace.recordings {
            recording.sources.retain(|source| source.id != id);
            if recording.primary_source == id {
                recording.primary_source = recording
                    .sources
                    .first()
                    .map_or_else(SourceId::default, |source| source.id);
            }
            if let Some(project) = &mut recording.overlay_snapshot {
                project.sources.retain(|source| source.id != id);
                for widget in &mut project.widgets {
                    widget
                        .bindings
                        .retain(|binding| binding.channel.source_id != id);
                }
            }
        }
    }
}
