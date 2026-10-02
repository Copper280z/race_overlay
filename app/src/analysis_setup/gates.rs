//! GPS timing gates and manual course-position anchors.

use super::super::*;
use super::SetupOutcome;
use crate::ui_kit::{theme::text, widgets};
use eframe::egui::RichText;

impl AnalysisApp {
    pub(super) fn setup_gates(&mut self, ui: &mut egui::Ui, outcome: &mut SetupOutcome) {
        widgets::hint(
            ui,
            "Scrub to the timing line and capture it. Runs align at the start gate.",
        );
        ui.add_space(4.0);
        widgets::card(ui, false, |ui| {
            self.gate_capture_controls(ui, outcome);
            let gate_count = self.workspace.course.gates.len();
            for (index, gate) in self.workspace.course.gates.iter_mut().enumerate() {
                ui.separator();
                ui.label(
                    RichText::new(if index == 0 {
                        "Start gate"
                    } else {
                        "Finish gate"
                    })
                    .color(text::strong()),
                );
                ui.horizontal_wrapped(|ui| {
                    outcome.changed |= ui
                        .add(
                            egui::DragValue::new(&mut gate.latitude)
                                .speed(0.00001)
                                .max_decimals(7)
                                .prefix("Lat "),
                        )
                        .changed();
                    outcome.changed |= ui
                        .add(
                            egui::DragValue::new(&mut gate.longitude)
                                .speed(0.00001)
                                .max_decimals(7)
                                .prefix("Lon "),
                        )
                        .changed();
                });
                ui.horizontal_wrapped(|ui| {
                    outcome.changed |= ui
                        .add(
                            egui::DragValue::new(&mut gate.heading_degrees)
                                .speed(0.5)
                                .suffix("° heading"),
                        )
                        .changed();
                    outcome.changed |= ui
                        .add(
                            egui::DragValue::new(&mut gate.width_meters)
                                .range(1.0..=100.0)
                                .suffix(" m wide"),
                        )
                        .changed();
                });
            }
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if gate_count == 1
                    && ui
                        .button("Split laps at gate")
                        .on_hover_text("Replace intervals with laps between crossings")
                        .clicked()
                {
                    self.split_laps_at_gate();
                    outcome.changed = true;
                }
                if ui
                    .button("Restore detected intervals")
                    .on_hover_text("Re-detect laps or runs")
                    .clicked()
                {
                    outcome.restore_intervals = true;
                }
                if ui
                    .add_enabled(
                        !self.workspace.course.gates.is_empty(),
                        egui::Button::new("Clear gates"),
                    )
                    .clicked()
                {
                    self.workspace.course.gates.clear();
                    outcome.changed = true;
                }
            });
        });
        ui.add_space(6.0);
        self.anchor_controls(ui, outcome);
    }

    fn gate_capture_controls(&mut self, ui: &mut egui::Ui, outcome: &mut SetupOutcome) {
        let nudge_id = ui.id().with("gate-capture-nudge");
        let mut nudge = ui.data_mut(|d| d.get_temp::<f64>(nudge_id).unwrap_or(0.0));
        let capture = self.reference_gate_capture(nudge);
        match &capture {
            Some((_, name, time, _)) => {
                ui.label(
                    RichText::new(format!("Capturing from {name} at recording {time:.3} s"))
                        .small()
                        .color(text::weak()),
                );
            }
            None => widgets::hint(
                ui,
                "Pick a reference run with GPS at the playhead to capture a gate.",
            ),
        }
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::DragValue::new(&mut nudge)
                    .speed(0.05)
                    .range(-120.0..=120.0)
                    .prefix("Playhead + ")
                    .suffix(" s"),
            )
            .on_hover_text("Capture this far from the playhead");
            if nudge != 0.0 && ui.small_button("Reset").clicked() {
                nudge = 0.0;
            }
        });
        ui.data_mut(|d| d.insert_temp(nudge_id, nudge));
        ui.horizontal_wrapped(|ui| {
            for (index, label) in ["Capture start gate", "Capture finish gate"]
                .iter()
                .enumerate()
            {
                if ui
                    .add_enabled(capture.is_some(), egui::Button::new(*label))
                    .clicked()
                    && let Some((_, _, _, gate)) = &capture
                {
                    let gates = &mut self.workspace.course.gates;
                    if index == 0 {
                        if gates.is_empty() {
                            gates.push(*gate);
                        } else {
                            gates[0] = *gate;
                        }
                    } else if gates.len() == 1 {
                        gates.push(*gate);
                    } else if gates.len() > 1 {
                        gates[1] = *gate;
                    }
                    outcome.changed = true;
                }
            }
        });
    }

    fn anchor_controls(&mut self, ui: &mut egui::Ui, outcome: &mut SetupOutcome) {
        ui.collapsing("Manual course-position anchors", |ui| {
            widgets::hint(
                ui,
                "Choose a compared run and give a recording time that corresponds to a known reference-course distance. Anchors affect only that run's GPS matching, not its sensor timestamps.",
            );
            let mut selected = self
                .workspace
                .selected
                .clone()
                .or_else(|| self.state.selection.last().cloned());
            egui::ComboBox::from_id_salt("anchor-run")
                .selected_text(
                    selected
                        .as_ref()
                        .and_then(|key| segment(&self.workspace, key))
                        .map_or("Run", |(_, s)| s.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for run in &self.prepared.runs {
                        ui.selectable_value(&mut selected, Some(run.key.clone()), &run.name);
                    }
                });
            self.workspace.selected = selected.clone();
            let id = ui.id().with("anchor-pair");
            let mut pair = ui.data_mut(|d| d.get_temp::<[f64; 2]>(id).unwrap_or([0.0, 0.0]));
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::DragValue::new(&mut pair[0]).speed(0.01).prefix("Recording ").suffix(" s"));
                ui.add(egui::DragValue::new(&mut pair[1]).speed(0.1).prefix("Reference ").suffix(" m"));
            });
            if ui.button("Add anchor").clicked()
                && let Some(key) = selected
            {
                self.workspace.course.manual_anchors.push(ManualAnchor {
                    segment: Some(key),
                    recording_time: pair[0],
                    reference_progress: pair[1],
                    unknown: Default::default(),
                });
                outcome.changed = true;
            }
            ui.data_mut(|d| d.insert_temp(id, pair));
            let mut remove = None;
            for (index, anchor) in self.workspace.course.manual_anchors.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{:.3} s at {:.1} m",
                        anchor.recording_time, anchor.reference_progress
                    ));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = remove {
                self.workspace.course.manual_anchors.remove(index);
                outcome.changed = true;
            }
        });
    }
}
