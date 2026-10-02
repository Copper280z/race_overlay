//! Per-recording data sources, filtering, and video/log clock offsets.

use super::super::*;
use super::SetupOutcome;
use crate::ui_kit::{
    theme::text,
    widgets::{self, danger_button},
};
use eframe::egui::RichText;

impl AnalysisApp {
    pub(super) fn setup_sources(&mut self, ui: &mut egui::Ui, outcome: &mut SetupOutcome) {
        widgets::hint(
            ui,
            "Timing offsets are signed. video time = recording time + video offset; source time = recording time + source offset.",
        );
        ui.add_space(4.0);
        for recording in &mut self.workspace.recordings {
            ui.push_id(recording.id.0, |ui| {
                widgets::card(ui, false, |ui| {
                    ui.label(
                        RichText::new(recording.name.clone())
                            .strong()
                            .color(text::strong()),
                    );
                    video_offset_controls(ui, recording, outcome);
                    primary_source_picker(ui, recording, outcome);
                    for source in &mut recording.sources {
                        ui.push_id(source.id.0, |ui| {
                            ui.separator();
                            source_editor(ui, source, outcome, &mut self.errors);
                        });
                    }
                    ui.add_space(2.0);
                    if ui.button("Attach another telemetry source…").clicked() {
                        outcome.add_source_to = Some(recording.id);
                    }
                });
            });
            ui.add_space(6.0);
        }
    }
}

fn video_offset_controls(ui: &mut egui::Ui, recording: &mut Recording, outcome: &mut SetupOutcome) {
    ui.horizontal(|ui| {
        ui.label("Video offset");
        outcome.changed |= ui
            .add(
                egui::DragValue::new(&mut recording.video_offset_seconds)
                    .speed(0.01)
                    .suffix(" s"),
            )
            .on_hover_text("video time = recording time + this offset")
            .changed();
    });
    ui.collapsing("Match one visible event", |ui| {
        widgets::hint(
            ui,
            "Unlink a video panel and find a recognizable event. Enter its exported-video time and the same event's recording time. This sets an offset; it does not warp either run.",
        );
        let id = ui.id().with("event-pair");
        let mut pair = ui.data_mut(|d| d.get_temp::<[f64; 2]>(id).unwrap_or([0.0, 0.0]));
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut pair[0]).speed(0.01).prefix("Video ").suffix(" s"));
            ui.add(egui::DragValue::new(&mut pair[1]).speed(0.01).prefix("Recording ").suffix(" s"));
        });
        if ui.button("Apply matching event").clicked() {
            recording.video_offset_seconds = pair[0] - pair[1];
            outcome.changed = true;
        }
        ui.data_mut(|d| d.insert_temp(id, pair));
        widgets::hint(
            ui,
            "For normal synchronization and camera orientation, use the recording's Video alignment section.",
        );
    });
}

fn primary_source_picker(ui: &mut egui::Ui, recording: &mut Recording, outcome: &mut SetupOutcome) {
    let selected = recording
        .sources
        .iter()
        .find(|source| source.id == recording.primary_source)
        .map_or("Primary data", |source| source.name.as_str())
        .to_owned();
    ui.horizontal(|ui| {
        ui.label("Primary data");
        egui::ComboBox::from_id_salt("primary-source")
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for source in &recording.sources {
                    outcome.changed |= ui
                        .selectable_value(&mut recording.primary_source, source.id, &source.name)
                        .changed();
                }
            });
    });
}

fn source_editor(
    ui: &mut egui::Ui,
    source: &mut SourceConfig,
    outcome: &mut SetupOutcome,
    errors: &mut Vec<String>,
) {
    ui.label(RichText::new(&source.name).color(text::strong()));
    ui.add(
        egui::Label::new(
            RichText::new(source.path.display().to_string())
                .small()
                .color(text::weak()),
        )
        .truncate(),
    );
    if source.adapter == "generic_csv" {
        csv_settings(ui, source, outcome, errors);
    }
    outcome.changed |= ui
        .add(
            egui::DragValue::new(&mut source.alignment.offset_seconds)
                .speed(0.01)
                .prefix("Source − recording ")
                .suffix(" s"),
        )
        .on_hover_text("source time = recording time + this offset")
        .changed();
    if !source.settings.is_object() {
        source.settings = json!({});
    }
    // Draft controls must not silently change saved processing until applied.
    let filter_id = ui.id().with("source-filter-draft");
    let (mut enabled, mut hz) = ui
        .data_mut(|d| d.get_temp::<(bool, f64)>(filter_id))
        .unwrap_or((
            source
                .settings
                .get("low_pass_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            source
                .settings
                .get("low_pass_hz")
                .and_then(Value::as_f64)
                .unwrap_or(8.0),
        ));
    ui.horizontal(|ui| {
        ui.checkbox(&mut enabled, "Source low-pass").on_hover_text(
            "Zero-phase, non-causal smoothing of every continuous channel in this source",
        );
        if enabled {
            ui.add(
                egui::DragValue::new(&mut hz)
                    .range(0.1..=100.0)
                    .suffix(" Hz"),
            );
        }
        if ui.button("Reload & apply").clicked() {
            source.settings["low_pass_enabled"] = json!(enabled);
            source.settings["low_pass_hz"] = json!(hz);
            outcome.reload.push(source.clone());
            outcome.changed = true;
        }
    });
    ui.data_mut(|d| d.insert_temp(filter_id, (enabled, hz)));
    ui.horizontal_wrapped(|ui| {
        if ui.button("Locate source file…").clicked()
            && let Some(path) = rfd::FileDialog::new().pick_file()
        {
            source.path = path;
            outcome.reload.push(source.clone());
            outcome.changed = true;
        }
        if danger_button(ui, "Remove source")
            .on_hover_text("Removes its bindings, not files or other sources.")
            .clicked()
        {
            outcome.removed_sources.push(source.id);
            outcome.changed = true;
        }
    });
}

fn csv_settings(
    ui: &mut egui::Ui,
    source: &mut SourceConfig,
    outcome: &mut SetupOutcome,
    errors: &mut Vec<String>,
) {
    ui.collapsing("CSV columns / units (advanced)", |ui| {
        widgets::hint(
            ui,
            "Default: first row is headers, first column is time in seconds. Configure columns with name, quantity and unit for cross-file comparisons and GPS. Explicit null disables a header/time column.",
        );
        let id = ui.id().with("csv-json");
        let mut draft = ui
            .data_mut(|d| d.get_temp::<String>(id))
            .unwrap_or_else(|| serde_json::to_string_pretty(&source.settings).unwrap_or_default());
        ui.add(
            egui::TextEdit::multiline(&mut draft)
                .code_editor()
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );
        if ui.button("Apply CSV settings & reload").clicked() {
            match serde_json::from_str::<Value>(&draft) {
                Ok(value)
                    if value.is_object()
                        && serde_json::from_value::<overlay_core::adapters::CsvConfig>(value.clone())
                            .is_ok() =>
                {
                    source.settings = value;
                    outcome.reload.push(source.clone());
                    outcome.changed = true;
                }
                _ => errors.push(
                    "CSV settings must be a valid configuration object; see docs/usage.md.".into(),
                ),
            }
        }
        ui.data_mut(|d| d.insert_temp(id, draft));
    });
}
