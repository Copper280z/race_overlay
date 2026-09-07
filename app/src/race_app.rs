//! Thin workspace facade. Analysis remains hosted here for handoff, while all
//! Overlay document, worker, rendering, and panel behavior lives under
//! [`editor::OverlayEditor`].

use eframe::egui;
use overlay_core::UnitSystem;

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

pub struct RaceOverlayApp {
    analysis: crate::analysis_app::AnalysisApp,
    analysis_mode: bool,
    editor: OverlayEditor,
}

impl RaceOverlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self {
            analysis: crate::analysis_app::AnalysisApp::new(),
            analysis_mode: true,
            editor: OverlayEditor::new(cc),
        }
    }

    fn unit_system(&self) -> UnitSystem {
        self.editor.unit_system()
    }

    fn workspace_mode_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.analysis_mode, "Analysis")
                .clicked()
                && !self.analysis_mode
            {
                self.editor.stop_playback();
                if !self.editor.is_linked_to_analysis() {
                    self.analysis.clear_overlay_link();
                }
                if let Some(project) = self.editor.project().cloned() {
                    self.analysis.import_project(project);
                }
                self.analysis_mode = true;
            }
            if ui
                .selectable_label(!self.analysis_mode, "Overlay")
                .clicked()
            {
                self.analysis.pause();
                self.analysis_mode = false;
            }
            ui.separator();
            ui.weak(if self.analysis_mode {
                "Compare laps and runs · video and imagery optional"
            } else {
                "Edit widgets and export video"
            });
            if self.analysis_mode {
                let mut unit_system = self.unit_system();
                egui::ComboBox::from_id_salt("analysis-unit-system")
                    .selected_text(unit_system_label(unit_system))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut unit_system, UnitSystem::Metric, "Metric");
                        ui.selectable_value(&mut unit_system, UnitSystem::Imperial, "Imperial");
                    });
                if unit_system != self.unit_system() {
                    self.editor.set_unit_system(unit_system);
                }
            }
        });
    }
}

// Workspace application glue remains in the façade. Feature behavior is implemented in private modules above.
impl eframe::App for RaceOverlayApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(
            UNIT_SYSTEM_STORAGE_KEY,
            match self.unit_system() {
                UnitSystem::Metric => "metric",
                UnitSystem::Imperial => "imperial",
            }
            .into(),
        );
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.editor.poll_workers(&ctx);
        let step = if !ctx.text_edit_focused() {
            ctx.input_mut(|input| {
                if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    -1
                } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    1
                } else {
                    0
                }
            })
        } else {
            0
        };
        if step != 0 {
            if self.analysis_mode {
                self.analysis.step_reference(step);
            } else {
                self.editor.step_frame(step);
            }
        }
        egui::Panel::top("workspace-mode").show(ui, |ui| self.workspace_mode_bar(ui));
        if self.analysis_mode {
            let editor_status = self.editor.status().to_owned();
            egui::Panel::bottom("analysis-host-status").show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.small(&editor_status);
                    if self.editor.export_running() {
                        ui.add(
                            egui::ProgressBar::new(self.editor.export_fraction())
                                .desired_width(140.0),
                        );
                        if ui.button("Cancel export").clicked() {
                            self.editor.cancel_export();
                        }
                    }
                });
            });
            self.analysis
                .ui(ui, self.editor.tools(), self.unit_system());
            for action in self.analysis.take_actions() {
                match action {
                    crate::analysis_app::AnalysisAction::OpenOverlay(project) => {
                        if self.editor.open_analysis_overlay(project) {
                            self.analysis_mode = false;
                        } else {
                            self.analysis
                                .report_overlay_open_error(self.editor.status().to_owned());
                        }
                    }
                }
            }
            // An export already in progress continues while data is inspected.
            if self.editor.export_running() {
                self.editor.export_dialog(&ctx);
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(33));
            return;
        }
        self.editor.load_pending_overlay_texture(&ctx);
        if self.editor.playback_finished() {
            self.editor.stop_playback();
        }
        egui::Panel::top("toolbar").show(ui, |ui| self.editor.top_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(self.editor.status());
                if let Some(error) = self.editor.tools_error() {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("FFmpeg: {error}"));
                }
            });
        });
        egui::Panel::left("sources")
            .resizable(true)
            .default_size(270.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.editor.sources_panel(ui));
            });
        egui::Panel::right("widgets")
            .resizable(true)
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.editor.widgets_panel(ui));
            });
        egui::CentralPanel::default().show(ui, |ui| {
            if self.editor.plot_visible() {
                self.editor.data_plot_ui(ui);
            } else {
                self.editor.preview_ui(ui);
            }
        });
        self.editor.export_dialog(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

mod appearance_policy;
mod controllers;
mod editor;
mod export_policy;
mod policy;
mod session;
mod source_policy;
#[cfg(test)]
mod tests;
mod widget_policy;

use editor::OverlayEditor;
use policy::unit_system_label;
