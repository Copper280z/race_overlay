//! Recording-list presentation and the actions it emits.

use super::*;

enum RecordingAction {
    AttachVideo(RecordingId),
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
            if ui
                .add_enabled(
                    !all_keys.is_empty()
                        && all_keys
                            .iter()
                            .any(|key| !self.state.selection.contains(key)),
                    egui::Button::new("Select all"),
                )
                .clicked()
            {
                self.state.selection = all_keys.clone();
                changed = true;
            }
            if ui
                .add_enabled(
                    !self.state.selection.is_empty(),
                    egui::Button::new("Deselect all"),
                )
                .clicked()
            {
                self.state.selection.clear();
                changed = true;
            }
            ui.weak(format!(
                "{} of {} selected",
                self.state.selection.len(),
                all_keys.len()
            ));
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.workspace.recordings.is_empty() {
                ui.heading("Start with your data");
                ui.label("Drop multiple XRK files here. Circuit laps appear automatically; autocross gets an estimated driving interval. No track setup required.");
            }
            let recording_ids = self
                .workspace
                .recordings
                .iter()
                .map(|recording| recording.id)
                .collect::<Vec<_>>();
            for recording_id in recording_ids {
                ui.push_id(recording_id.0, |ui| {
                    let Some(recording_index) = self
                        .workspace
                        .recordings
                        .iter()
                        .position(|recording| recording.id == recording_id)
                    else {
                        return;
                    };
                    let name = self.workspace.recordings[recording_index].name.clone();
                    let was_selected = self.state.selected_recording == Some(recording_id);
                    if ui.selectable_label(was_selected, name).clicked() {
                        self.state.selected_recording = Some(recording_id);
                    }
                    let selected = self.state.selected_recording == Some(recording_id);
                    if selected {
                        let recording = &mut self.workspace.recordings[recording_index];
                        changed |= ui.text_edit_singleline(&mut recording.name).changed();
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Attach video…").clicked() {
                                actions.push(RecordingAction::AttachVideo(recording_id));
                            }
                            if ui
                                .add_enabled(
                                    recording.video_path.is_some(),
                                    egui::Button::new("Edit overlay / advanced sync"),
                                )
                                .clicked()
                            {
                                actions.push(RecordingAction::OpenOverlay(recording_id));
                            }
                            if ui.button("Remove…").clicked() {
                                actions.push(RecordingAction::RequestRemoval(recording_id));
                            }
                        });
                        if let Some(path) = &recording.video_path {
                            ui.small(path.file_name().unwrap_or_default().to_string_lossy());
                        }
                    }
                    if selected
                        && let Some(paths) = self
                            .workspace
                            .settings
                            .get("unassigned_videos")
                            .and_then(Value::as_array)
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
                    if selected {
                        self.video_alignment_ui(ui, tools, recording_id);
                    }
                    let recording = &self.workspace.recordings[recording_index];
                    for segment in &recording.segments {
                        let key = SegmentRef {
                            recording_id,
                            segment_id: segment.id,
                        };
                        let mut checked = self.state.selection.contains(&key);
                        ui.horizontal(|ui| {
                            if ui
                                .checkbox(
                                    &mut checked,
                                    format!(
                                        "{} · {:.3}s{}",
                                        segment.name,
                                        segment.duration(),
                                        if segment.estimated { " (estimated)" } else { "" }
                                    ),
                                )
                                .changed()
                            {
                                if checked {
                                    self.state.selection.push(key.clone());
                                } else {
                                    self.state.selection.retain(|item| item != &key);
                                }
                                changed = true;
                            }
                            if ui
                                .selectable_label(
                                    self.workspace.reference.as_ref() == Some(&key),
                                    "Ref",
                                )
                                .on_hover_text("Pin this run as the comparison reference")
                                .clicked()
                            {
                                self.workspace.reference = Some(key.clone());
                                if !self.state.selection.contains(&key) {
                                    self.state.selection.push(key);
                                }
                                self.state.cursor = 0.0;
                                changed = true;
                            }
                        });
                        if checked {
                            self.recording_segment_details(ui, recording_id, segment);
                        }
                    }
                    if recording.segments.is_empty() {
                        ui.weak(if self.loading.contains_key(&recording.primary_source) {
                            "Importing…"
                        } else {
                            "No interval. Check source errors or add a range in setup."
                        });
                    }
                    ui.separator();
                });
            }
        });
        if changed {
            self.changed();
        }
        for action in actions {
            self.apply_recording_action(action);
        }
    }

    fn recording_segment_details(
        &self,
        ui: &mut egui::Ui,
        recording_id: RecordingId,
        segment: &RunSegment,
    ) {
        if !segment.competitive {
            ui.weak("Out/in or noncompetitive — available for inspection");
        }
        let Some(run) =
            self.prepared.runs.iter().find(|run| {
                run.key.recording_id == recording_id && run.key.segment_id == segment.id
            })
        else {
            return;
        };
        if run.progress.is_empty() {
            ui.weak("No GPS course match — use elapsed time or traveled distance.");
        } else {
            let valid = run
                .progress
                .iter()
                .filter(|point| point.confidence > 0.0)
                .count();
            ui.weak(format!(
                "Course match: {:.0}% of GPS samples",
                valid as f64 / run.progress.len() as f64 * 100.0
            ))
            .on_hover_text("Matching coverage, not statistical accuracy. Use actual GPS view to inspect the route; anchors can help ambiguous sections.");
        }
        if !self.prepared.automatic_time_alignment {
            return;
        }
        if self.workspace.reference.as_ref() == Some(&run.key) {
            ui.weak("Time alignment: reference clock");
        } else if let Some(alignment) = &run.time_alignment {
            let details = match (alignment.distance_offset_meters, alignment.coefficient) {
                (Some(distance), Some(coefficient)) => format!(
                    "distance shift {distance:+.1}m · start shift {:+.2}s · r {coefficient:+.3}",
                    alignment.offset_seconds
                ),
                _ => format!("start shift {:+.2}s", alignment.offset_seconds),
            };
            ui.weak(format!("Time alignment: {} · {details}", alignment.channel))
                .on_hover_text("Best-effort comparison shift only; source and video timestamps were not changed.");
        } else if run.distance.len() >= 2 {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "Time alignment unavailable — use traveled distance",
            );
        } else {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "Time alignment unavailable — using segment-relative time",
            );
        }
    }

    fn apply_recording_action(&mut self, action: RecordingAction) {
        match action {
            RecordingAction::AttachVideo(id) => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Exported video", &["mp4", "mov", "mkv"])
                    .pick_file()
                {
                    self.attach_video(id, path);
                }
            }
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
                    if ui.button("Remove").clicked() {
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
