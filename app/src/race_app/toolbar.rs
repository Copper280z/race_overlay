//! Toolbar and export dialog controls.
use super::super::ExportCodecChoice;
use super::super::policy::{codec_display_name, unit_system_label};
use super::{OverlayEditor, PanelAction};
use eframe::egui;
use overlay_core::UnitSystem;
use overlay_media::ExportSettings;

impl OverlayEditor {
    pub(in crate::race_app) fn top_bar(&mut self, ui: &mut egui::Ui) {
        let view = self.toolbar_view();
        ui.horizontal(|ui| {
            if ui.button("Open video").clicked() {
                self.open_video();
            }
            if ui.button("Open project").clicked() {
                self.open_project();
            }
            let previous_units = view.unit_system;
            let mut selected_units = view.unit_system;
            egui::ComboBox::from_id_salt("default-unit-system")
                .selected_text(unit_system_label(view.unit_system))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut selected_units, UnitSystem::Metric, "Metric");
                    ui.selectable_value(&mut selected_units, UnitSystem::Imperial, "Imperial");
                });
            if selected_units != previous_units {
                self.apply_panel_action(PanelAction::SetUnitSystem(selected_units));
            }
            ui.add_enabled_ui(view.has_project, |ui| {
                if ui.selectable_label(!view.show_data_plot, "Video").clicked() {
                    self.apply_panel_action(PanelAction::SetPlotVisible(false));
                }
                if ui
                    .selectable_label(view.show_data_plot, "Data graph")
                    .clicked()
                {
                    self.apply_panel_action(PanelAction::SetPlotVisible(true));
                }
                if ui.button("Save").clicked() {
                    self.save_project(false);
                }
                if ui.button("Save as…").clicked() {
                    self.save_project(true);
                }
                if ui.button("Export MP4…").clicked() {
                    self.apply_panel_action(PanelAction::OpenExportDialog);
                }
            });
            if let Some(cancel) = &self.export_controller.export_cancel {
                ui.add(
                    egui::ProgressBar::new(self.export_controller.export_fraction)
                        .desired_width(120.0),
                );
                if ui.button("Cancel export").clicked() {
                    cancel.cancel();
                }
            }
        });
    }

    pub(in crate::race_app) fn export_dialog(&mut self, ctx: &egui::Context) {
        if !self.export_controller.export_dialog_open {
            return;
        }
        let Some(metadata) = self.session.metadata().cloned() else {
            self.export_controller.export_dialog_open = false;
            return;
        };
        let mut open = self.export_controller.export_dialog_open;
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
                    ui.radio_value(&mut self.export_controller.export_codec, choice, choice.label());
                }
                ui.small(match self.export_controller.export_codec {
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
                ui.checkbox(&mut self.export_controller.export_match_bitrate, "Match source bitrate");
                ui.add_enabled_ui(!self.export_controller.export_match_bitrate, |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.export_controller.export_bitrate_mbps, 1.0..=200.0)
                            .logarithmic(true)
                            .suffix(" Mb/s")
                            .text("Video bitrate"),
                    );
                });
                let selected_rate = if self.export_controller.export_match_bitrate {
                    metadata.bit_rate.map(|rate| rate as f64 / 1_000_000.0)
                } else {
                    Some(self.export_controller.export_bitrate_mbps)
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
                        &mut self.export_controller.export_apple_compatible,
                        "Apple-compatible MP4 codec tags and color defaults",
                    );
                    ui.checkbox(
                        &mut self.export_controller.export_force_yuv420p,
                        "Force widely compatible 8-bit YUV 4:2:0",
                    );
                    ui.checkbox(
                        &mut self.export_controller.export_fast_start,
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
                        self.export_controller.export_dialog_open = false;
                    }
                });
            });
        self.export_controller.export_dialog_open &= open;
        if start_export {
            let settings = ExportSettings {
                codec: self.export_controller.export_codec.codec(),
                bitrate: (!self.export_controller.export_match_bitrate).then_some(
                    (self.export_controller.export_bitrate_mbps * 1_000_000.0).round() as u64,
                ),
                pixel_format: self
                    .export_controller
                    .export_force_yuv420p
                    .then(|| "yuv420p".into()),
                apple_compatible: self.export_controller.export_apple_compatible,
                fast_start: self.export_controller.export_fast_start,
                ..Default::default()
            };
            self.export_controller.export_dialog_open = false;
            self.export_with_settings(settings);
        }
    }
}
