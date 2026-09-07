//! Recording-local video alignment presentation.
use super::super::*;
use super::*;

impl AnalysisApp {
    pub(in crate::analysis_app) fn video_alignment_ui(
        &mut self,
        ui: &mut egui::Ui,
        tools: Option<&FfmpegTools>,
        recording_id: RecordingId,
    ) {
        let Some(recording) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == recording_id)
            .cloned()
        else {
            return;
        };
        let Some(video) = recording.video_path.as_ref() else {
            return;
        };
        let camera_source = recording
            .sources
            .iter()
            .find(|source| is_camera_telemetry_source(source))
            .cloned();
        let alignment_complete = camera_source.as_ref().is_some_and(|source| {
            let metadata = CameraVideoSyncMetadata::read(&source.settings);
            let acceptable_primary_audio =
                recording.primary_source == source.id && metadata.audio_is_acceptable();
            metadata.applied || acceptable_primary_audio
        });
        let mut add_camera = false;
        let mut accept_audio_alignment = false;
        let mut retry_audio = None;
        let mut estimate = false;
        let mut apply = false;
        egui::CollapsingHeader::new(if alignment_complete {
            "Video alignment ✓"
        } else {
            "Video alignment"
        })
        .id_salt((recording.id.0, "video-alignment"))
        .default_open(!alignment_complete)
        .show(ui, |ui| {
            ui.small(format!(
                "Video: {}",
                video.file_name().unwrap_or_default().to_string_lossy()
            ));
            let Some(camera_source) = camera_source.as_ref() else {
                ui.label("Add the matching camera telemetry to align this video to the logger.");
                if ui.button("Add camera telemetry…").clicked() {
                    add_camera = true;
                }
                return;
            };
            ui.small(format!("Camera telemetry: {}", camera_source.name));
            let metadata = CameraVideoSyncMetadata::read(&camera_source.settings);
            let audio_offset = metadata.camera_minus_video_seconds;
            self.camera_calibration_ui(
                ui,
                recording.id,
                camera_source.id,
                audio_offset,
            );
            if self
                .video_sync
                .recording(recording.id)
                .is_some_and(|state| state.audio_jobs.contains_key(&camera_source.id))
            {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Matching camera audio to the exported video…");
                });
                return;
            }
            if let Some(Err(error)) = self
                .video_sync
                .recording(recording.id)
                .and_then(|state| state.audio_results.get(&camera_source.id))
            {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
                if ui
                    .add_enabled(tools.is_some(), egui::Button::new("Retry audio alignment"))
                    .clicked()
                {
                    retry_audio = Some(camera_source.id);
                }
                return;
            }
            let Some(audio_offset) = audio_offset else {
                if tools.is_none() {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        "FFmpeg and FFprobe were not found. Install them, then relaunch Race Overlay.",
                    );
                } else {
                    ui.spinner();
                    ui.label("Waiting to align camera audio…");
                }
                return;
            };
            let peak = metadata.audio_peak.unwrap_or(0.0);
            let confidence = metadata.audio_confidence.unwrap_or(0.0);
            ui.label(format!(
                "Camera ↔ video matched · offset {audio_offset:+.3}s · peak {peak:.2} · confidence {confidence:.2}"
            ));
            if recording.primary_source == camera_source.id {
                if alignment_complete {
                    ui.colored_label(egui::Color32::LIGHT_GREEN, "Video alignment complete.");
                } else {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "The audio match is weak; inspect the timing before accepting it.",
                    );
                    if ui.button("Accept current audio match").clicked() {
                        accept_audio_alignment = true;
                    }
                }
                return;
            }
            let target_choices = self.video_alignment_channel_choices(recording.primary_source);
            let camera_choices = self.video_alignment_channel_choices(camera_source.id);
            let default_pair = self.default_video_alignment_channels(
                recording.primary_source,
                camera_source.id,
            );
            let Some(initial_pair) = default_pair.or_else(|| {
                Some((
                    target_choices.first()?.0.clone(),
                    camera_choices.first()?.0.clone(),
                ))
            }) else {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "No dense continuous channel pair is available for correlation.",
                );
                return;
            };
            let pair = self
                .video_sync
                .recording_mut(recording.id)
                .channels
                .get_or_insert(initial_pair);
            let previous_pair = pair.clone();
            let channel_picker = |ui: &mut egui::Ui,
                                  id,
                                  label: &str,
                                  selected: &mut ChannelRef,
                                  choices: &[(ChannelRef, String)]| {
                let selected_label = choices
                    .iter()
                    .find(|(reference, _)| reference == selected)
                    .map_or("Choose channel", |(_, label)| label.as_str());
                egui::ComboBox::from_id_salt(id)
                    .selected_text(format!("{label}: {selected_label}"))
                    .show_ui(ui, |ui| {
                        for (reference, label) in choices {
                            ui.selectable_value(selected, reference.clone(), label);
                        }
                    });
            };
            channel_picker(
                ui,
                (recording.id.0, "logger-alignment-channel"),
                "Logger",
                &mut pair.0,
                &target_choices,
            );
            channel_picker(
                ui,
                (recording.id.0, "camera-alignment-channel"),
                "Camera",
                &mut pair.1,
                &camera_choices,
            );
            if *pair != previous_pair {
                self.video_sync.recording_mut(recording.id).alignment_result = None;
            }
            let estimate_pending = self
                .video_sync
                .recording(recording.id)
                .and_then(|state| state.alignment_job)
                .is_some();
            if ui
                .add_enabled(
                    !target_choices.is_empty()
                        && !camera_choices.is_empty()
                        && !estimate_pending,
                    egui::Button::new("Estimate logger ↔ camera alignment"),
                )
                .clicked()
            {
                estimate = true;
            }
            if estimate_pending {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Searching the complete feasible overlap…");
                });
            }
            if let Some(result) = self
                .video_sync
                .recording(recording.id)
                .and_then(|state| state.alignment_result.as_ref())
            {
                match result {
                    Ok(candidate) => {
                        let target_offset = recording
                            .sources
                            .iter()
                            .find(|source| source.id == recording.primary_source)
                            .map_or(0.0, |source| source.alignment.offset_seconds);
                        let (_, video_offset) = resolved_video_alignment_offsets(
                            target_offset,
                            candidate.result.target_minus_reference_seconds,
                            candidate.camera_video_offset_seconds,
                        );
                        ui.label(format!(
                            "Candidate: video offset {video_offset:+.3}s · Pearson r {:+.3} · {:.1}s overlap · {} samples",
                            candidate.result.correlation_coefficient,
                            candidate.result.overlap_seconds,
                            candidate.result.sample_count
                        ));
                        if candidate.result.correlation_coefficient.abs() < 0.5 {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                "Weak correlation; choose more comparable channels before applying.",
                            );
                        }
                        if ui.button("Apply video alignment").clicked() {
                            apply = true;
                        }
                    }
                    Err(error) => {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                }
            }
            ui.small("Applying preserves the logger clock, so existing intervals and gates do not need to be rebuilt.");
        });
        if add_camera
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("Camera telemetry", &["lrv", "insv"])
                .pick_file()
        {
            let source = SourceConfig {
                id: SourceId::new(),
                name: path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Camera telemetry")
                    .into(),
                adapter: adapter_for(&path).into(),
                path,
                alignment: Default::default(),
                settings: json!({}),
                unknown: Default::default(),
            };
            if let Some(target) = self
                .workspace
                .recordings
                .iter_mut()
                .find(|target| target.id == recording.id)
            {
                target.sources.push(source.clone());
            }
            self.enqueue(source);
            self.message =
                "Loading camera telemetry; audio alignment will start automatically.".into();
            self.changed();
        }
        if let Some(source_id) = retry_audio {
            self.video_sync
                .recording_mut(recording.id)
                .audio_results
                .remove(&source_id);
        }
        if accept_audio_alignment
            && let Some(camera_source_id) = camera_source.as_ref().map(|source| source.id)
            && let Some(source) = self
                .workspace
                .recordings
                .iter_mut()
                .find(|target| target.id == recording.id)
                .and_then(|target| {
                    target
                        .sources
                        .iter_mut()
                        .find(|source| source.id == camera_source_id)
                })
        {
            let mut metadata = CameraVideoSyncMetadata::read(&source.settings);
            metadata.applied = true;
            metadata.write(&mut source.settings);
            self.message = "Accepted the current camera/video audio match.".into();
            self.changed();
        }
        if estimate {
            self.start_video_alignment_estimate(recording.id);
        }
        if apply {
            self.apply_video_alignment(recording.id);
        }
    }
}
