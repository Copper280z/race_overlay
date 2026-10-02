//! Recording-list presentation and the actions it emits.

use super::*;
use crate::ui_kit::{Tone, theme::text, widgets};
use eframe::egui::RichText;

enum RecordingAction {
    AttachVideo(RecordingId),
    AddDataLog(RecordingId),
    PairImportedVideo(RecordingId, PathBuf),
    OpenOverlay(RecordingId),
    RequestRemoval(RecordingId),
}

impl AnalysisApp {
    pub(super) fn browser(&mut self, ui: &mut egui::Ui, tools: Option<&FfmpegTools>) {
        let mut changed = false;
        let mut actions = Vec::new();
        let all_keys = self.all_segment_keys();
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.state.selection.is_empty(), egui::Button::new("Clear"))
                    .clicked()
                {
                    self.state.selection.clear();
                    changed = true;
                }
                if ui
                    .add_enabled(
                        all_keys
                            .iter()
                            .any(|key| !self.state.selection.contains(key)),
                        egui::Button::new("Select all"),
                    )
                    .clicked()
                {
                    self.state.selection = all_keys.clone();
                    changed = true;
                }
                let full_count = format!(
                    "{} of {} laps selected",
                    self.state.selection.len(),
                    all_keys.len()
                );
                let count = if widgets::text_width(ui, &full_count, egui::TextStyle::Body)
                    <= ui.available_width()
                {
                    full_count.clone()
                } else {
                    format!("{}/{}", self.state.selection.len(), all_keys.len())
                };
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(RichText::new(count).color(text::weak())).truncate())
                        .on_hover_text(full_count);
                });
            });
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.workspace.recordings.is_empty() {
                    widgets::empty_state(
                        ui,
                        "No recordings yet",
                        "Drop XRK or CSV logs here, or use Add files… in the toolbar.",
                        |_| {},
                    );
                }
                let recording_ids = self
                    .workspace
                    .recordings
                    .iter()
                    .map(|recording| recording.id)
                    .collect::<Vec<_>>();
                for recording_id in recording_ids {
                    ui.push_id(recording_id.0, |ui| {
                        self.recording_card(ui, tools, recording_id, &mut changed, &mut actions);
                    });
                    ui.add_space(4.0);
                }
            });
        if changed {
            self.changed();
        }
        for action in actions {
            self.apply_recording_action(action);
        }
    }

    fn recording_card(
        &mut self,
        ui: &mut egui::Ui,
        tools: Option<&FfmpegTools>,
        recording_id: RecordingId,
        changed: &mut bool,
        actions: &mut Vec<RecordingAction>,
    ) {
        let Some(index) = self
            .workspace
            .recordings
            .iter()
            .position(|recording| recording.id == recording_id)
        else {
            return;
        };
        let selected = self.state.selected_recording == Some(recording_id);
        // The whole card selects the recording; widgets inside it sit on top
        // and keep their own clicks.
        let card = ui.scope_builder(egui::UiBuilder::new().sense(egui::Sense::click()), |ui| {
            // Selectable text would take the click for itself.
            ui.style_mut().interaction.selectable_labels = false;
            widgets::card(ui, selected, |ui| {
            let state_id = ui.make_persistent_id(("recording-card", recording_id.0));
            let state = egui::collapsing_header::CollapsingState::load_with_default_open(
                ui.ctx(),
                state_id,
                true,
            );
            state
                .show_header(ui, |ui| {
                    let recording = &self.workspace.recordings[index];
                    let fitted = widgets::fit_text(
                        ui,
                        &recording.name,
                        (ui.available_width() - 150.0).max(60.0),
                    );
                    let name = RichText::new(fitted).strong().color(text::strong());
                    ui.add(egui::Label::new(name).selectable(false).truncate());
                    if recording.video_path.is_some() {
                        widgets::chip(ui, "Video", Tone::Info);
                    }
                    if self.loading.contains_key(&recording.primary_source) {
                        ui.spinner();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.recording_menu(ui, recording_id, index, changed, actions);
                    });
                })
                .body(|ui| {
                    if selected {
                        self.recording_video_section(ui, tools, recording_id, index, actions);
                    }
                    let segments = self.workspace.recordings[index].segments.clone();
                    for segment in &segments {
                        self.lap_row(ui, recording_id, segment, changed);
                    }
                    if segments.is_empty() {
                        let importing = self
                            .loading
                            .contains_key(&self.workspace.recordings[index].primary_source);
                        widgets::hint(
                            ui,
                            if importing {
                                "Importing…"
                            } else {
                                "No interval found. Check source errors or add a range under Timing & course setup."
                            },
                        );
                    }
                });
            })
        });
        if card.response.clicked() {
            self.state.selected_recording = Some(recording_id);
        }
    }

    fn recording_menu(
        &mut self,
        ui: &mut egui::Ui,
        recording_id: RecordingId,
        index: usize,
        changed: &mut bool,
        actions: &mut Vec<RecordingAction>,
    ) {
        widgets::popover(ui, "More ⏷", |ui| {
            let has_video = self.workspace.recordings[index].video_path.is_some();
            widgets::section_label(ui, "Name");
            *changed |= ui
                .text_edit_singleline(&mut self.workspace.recordings[index].name)
                .changed();
            widgets::section_label(ui, "Video");
            if ui.button("Attach video…").clicked() {
                actions.push(RecordingAction::AttachVideo(recording_id));
                widgets::close_popover(ui);
            }
            if ui
                .add_enabled(has_video, egui::Button::new("Edit overlay / advanced sync"))
                .clicked()
            {
                actions.push(RecordingAction::OpenOverlay(recording_id));
                widgets::close_popover(ui);
            }
            widgets::section_label(ui, "Data");
            if ui.button("Add data log…").clicked() {
                actions.push(RecordingAction::AddDataLog(recording_id));
                widgets::close_popover(ui);
            }
            widgets::section_label(ui, "Recording");
            if widgets::danger_button(ui, "Remove…").clicked() {
                actions.push(RecordingAction::RequestRemoval(recording_id));
                widgets::close_popover(ui);
            }
        });
    }

    /// Video status, pairing, and alignment for the selected recording.
    fn recording_video_section(
        &mut self,
        ui: &mut egui::Ui,
        tools: Option<&FfmpegTools>,
        recording_id: RecordingId,
        index: usize,
        actions: &mut Vec<RecordingAction>,
    ) {
        let video_name = self.workspace.recordings[index]
            .video_path
            .as_ref()
            .map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            });
        match &video_name {
            Some(name) => {
                ui.horizontal(|ui| {
                    let fitted =
                        widgets::fit_text(ui, name, (ui.available_width() - 110.0).max(60.0));
                    ui.label(RichText::new(fitted).small().color(text::weak()))
                        .on_hover_text(name);
                    if ui.small_button("Edit overlay").clicked() {
                        actions.push(RecordingAction::OpenOverlay(recording_id));
                    }
                });
            }
            None => {
                if ui.button("Attach video…").clicked() {
                    actions.push(RecordingAction::AttachVideo(recording_id));
                }
            }
        }
        if let Some(paths) = self
            .workspace
            .settings
            .get("unassigned_videos")
            .and_then(Value::as_array)
            && !paths.is_empty()
        {
            egui::ComboBox::from_id_salt("unpaired")
                .selected_text("Pair an imported video…")
                .show_ui(ui, |ui| {
                    for value in paths {
                        if let Some(path) = value.as_str()
                            && ui
                                .selectable_label(
                                    false,
                                    Path::new(path)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy(),
                                )
                                .clicked()
                        {
                            actions.push(RecordingAction::PairImportedVideo(
                                recording_id,
                                PathBuf::from(path),
                            ));
                        }
                    }
                });
        }
        self.video_alignment_ui(ui, tools, recording_id);
        ui.add_space(2.0);
    }

    fn lap_row(
        &mut self,
        ui: &mut egui::Ui,
        recording_id: RecordingId,
        segment: &RunSegment,
        changed: &mut bool,
    ) {
        let key = SegmentRef {
            recording_id,
            segment_id: segment.id,
        };
        let selected = self.state.selection.contains(&key);
        let is_reference = self.workspace.reference.as_ref() == Some(&key);
        let notes = self.segment_notes(recording_id, segment);
        let color = run_color(
            &self.state.selection,
            self.workspace.reference.as_ref(),
            &key,
        );
        let row = ui.horizontal(|ui| {
            if selected {
                widgets::dot(ui, color);
            } else {
                ui.add_space(20.0);
            }
            let mut checked = selected;
            // Chips are optional; drop them when the panel is narrow. The
            // duration, star, dot, and checkbox always keep their room.
            let show_chips = ui.available_width() > 300.0;
            let duration = format!("{:.3} s", segment.duration());
            let star_width = widgets::text_width(ui, "★", egui::TextStyle::Button)
                .max(widgets::text_width(ui, "☆", egui::TextStyle::Button));
            let spacing = ui.spacing();
            let mut reserved = widgets::text_width(ui, &duration, egui::TextStyle::Monospace)
                + star_width
                + spacing.item_spacing.x * 2.0
                + spacing.icon_width
                + spacing.icon_spacing;
            if show_chips {
                for label in [
                    (selected
                        && notes
                            .iter()
                            .any(|(tone, _)| matches!(tone, Tone::Warn | Tone::Bad)))
                    .then_some("check"),
                    segment.estimated.then_some("est."),
                ]
                .into_iter()
                .flatten()
                {
                    reserved += widgets::text_width(ui, label, egui::TextStyle::Small)
                        + 14.0
                        + spacing.item_spacing.x;
                }
            }
            let name = widgets::fit_text(
                ui,
                &segment.name,
                (ui.available_width() - reserved).max(0.0),
            );
            let label = RichText::new(name).color(if selected {
                text::strong()
            } else {
                text::normal()
            });
            if ui
                .checkbox(&mut checked, label)
                .on_hover_text(&segment.name)
                .changed()
            {
                if checked {
                    self.state.selection.push(key.clone());
                } else {
                    self.state.selection.retain(|item| item != &key);
                }
                *changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let star =
                    RichText::new(if is_reference { "★" } else { "☆" }).color(if is_reference {
                        Tone::Warn.color()
                    } else {
                        text::weak()
                    });
                if ui
                    .add(egui::Button::new(star).frame(false))
                    .on_hover_text(if is_reference {
                        "This run is the comparison reference"
                    } else {
                        "Make this run the comparison reference"
                    })
                    .clicked()
                {
                    self.workspace.reference = Some(key.clone());
                    if !self.state.selection.contains(&key) {
                        self.state.selection.push(key.clone());
                    }
                    self.state.cursor = 0.0;
                    *changed = true;
                }
                ui.monospace(duration);
                if selected
                    && show_chips
                    && let Some((tone, _)) = notes
                        .iter()
                        .find(|(tone, _)| matches!(tone, Tone::Warn | Tone::Bad))
                {
                    widgets::chip(ui, "check", *tone);
                }
                if segment.estimated && show_chips {
                    widgets::chip(ui, "est.", Tone::Neutral);
                }
            });
        });
        if selected && !notes.is_empty() {
            row.response.on_hover_ui(|ui| {
                for (tone, note) in &notes {
                    ui.label(RichText::new(note).color(tone.color()));
                }
            });
        }
    }

    /// Match and alignment diagnostics for one interval, most important first.
    fn segment_notes(
        &self,
        recording_id: RecordingId,
        segment: &RunSegment,
    ) -> Vec<(Tone, String)> {
        let mut notes = Vec::new();
        if !segment.competitive {
            notes.push((
                Tone::Neutral,
                "Out/in or noncompetitive — available for inspection".to_owned(),
            ));
        }
        if segment.estimated {
            notes.push((
                Tone::Neutral,
                "Estimated interval; not an official timing-system result".to_owned(),
            ));
        }
        let Some(run) =
            self.prepared.runs.iter().find(|run| {
                run.key.recording_id == recording_id && run.key.segment_id == segment.id
            })
        else {
            return notes;
        };
        if run.progress.is_empty() {
            notes.push((
                Tone::Warn,
                "No GPS course match — use elapsed time or traveled distance.".into(),
            ));
        } else {
            let valid = run
                .progress
                .iter()
                .filter(|point| point.confidence > 0.0)
                .count();
            notes.push((
                Tone::Neutral,
                format!(
                    "Course match: {:.0}% of GPS samples (coverage, not statistical accuracy)",
                    valid as f64 / run.progress.len() as f64 * 100.0
                ),
            ));
        }
        if !self.workspace.course.gates.is_empty() && run.start_gate.is_none() {
            notes.push((Tone::Warn, "Does not cross the start gate".into()));
        }
        if let Some(drift) = &run.gps_drift {
            let method = match drift.method {
                GpsDriftMethod::Staging => "staging",
                GpsDriftMethod::PathFit => "path fit",
            };
            notes.push((
                Tone::Neutral,
                format!("GPS drift: {:.1} m ({method})", drift.meters()),
            ));
        } else if self.workspace.course.correct_gps_drift
            && self.workspace.reference.as_ref() != Some(&run.key)
        {
            notes.push((Tone::Warn, "GPS drift not found".into()));
        }
        if !self.prepared.automatic_time_alignment {
            return notes;
        }
        if self.workspace.reference.as_ref() == Some(&run.key) {
            notes.push((Tone::Neutral, "Time alignment: reference clock".into()));
        } else if let Some(alignment) = &run.time_alignment
            && alignment.channel == START_GATE_ALIGNMENT
        {
            notes.push((
                Tone::Neutral,
                format!(
                    "Time alignment: start gate · shift {:+.2}s",
                    alignment.offset_seconds
                ),
            ));
        } else if let Some(alignment) = &run.time_alignment {
            let details = match (alignment.distance_offset_meters, alignment.coefficient) {
                (Some(distance), Some(coefficient)) => format!(
                    "distance shift {distance:+.1}m · start shift {:+.2}s · r {coefficient:+.3}",
                    alignment.offset_seconds
                ),
                _ => format!("start shift {:+.2}s", alignment.offset_seconds),
            };
            notes.push((
                Tone::Neutral,
                format!(
                    "Time alignment: {} · {details}. Best-effort shift only; source and video timestamps are unchanged.",
                    alignment.channel
                ),
            ));
        } else if run.distance.len() >= 2 {
            notes.push((
                Tone::Warn,
                "Time alignment unavailable — use traveled distance".into(),
            ));
        } else {
            notes.push((
                Tone::Warn,
                "Time alignment unavailable — using segment-relative time".into(),
            ));
        }
        notes
    }

    /// Asks for a video file and attaches it to the recording.
    pub(super) fn attach_video_dialog(&mut self, id: RecordingId) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Video", &["mp4", "mov", "mkv", "insv"])
            .pick_file()
        {
            self.attach_video(id, path);
        }
    }

    fn apply_recording_action(&mut self, action: RecordingAction) {
        match action {
            RecordingAction::AttachVideo(id) => self.attach_video_dialog(id),
            RecordingAction::AddDataLog(id) => self.attach_data_log_dialog(id),
            RecordingAction::PairImportedVideo(id, path) => {
                self.attach_video(id, path.clone());
                if let Some(paths) = self
                    .workspace
                    .settings
                    .get_mut("unassigned_videos")
                    .and_then(Value::as_array_mut)
                {
                    paths.retain(|value| value.as_str() != path.to_str());
                }
            }
            RecordingAction::OpenOverlay(id) => {
                if let Some(project) = self
                    .workspace
                    .recordings
                    .iter()
                    .find(|recording| recording.id == id)
                    .map(project_from_recording)
                {
                    self.active_overlay = Some(id);
                    self.pause();
                    self.actions.push(AnalysisAction::OpenOverlay(project));
                }
            }
            RecordingAction::RequestRemoval(id) => self.remove_recording = Some(id),
        }
    }

    pub(super) fn remove_dialog(&mut self, ctx: &egui::Context) {
        let Some(id) = self.remove_recording else {
            return;
        };
        egui::Window::new("Remove recording?")
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label("Remove its comparison selections and video attachment? Files on disk will not be deleted.");
                ui.horizontal(|ui| {
                    if widgets::danger_button(ui, "Remove").clicked() {
                        if let Some(recording) = self
                            .workspace
                            .recordings
                            .iter()
                            .find(|recording| recording.id == id)
                        {
                            for source in &recording.sources {
                                self.data.remove(&source.id);
                                self.loading.remove(&source.id);
                            }
                        }
                        self.workspace.recordings.retain(|recording| recording.id != id);
                        self.state.selection.retain(|segment| segment.recording_id != id);
                        if self
                            .workspace
                            .reference
                            .as_ref()
                            .is_some_and(|reference| reference.recording_id == id)
                        {
                            self.workspace.reference = None;
                        }
                        self.video_sync.remove_recording(id);
                        self.remove_recording = None;
                        self.default_selection();
                    }
                    if ui.button("Cancel").clicked() {
                        self.remove_recording = None;
                    }
                });
            });
    }
}
