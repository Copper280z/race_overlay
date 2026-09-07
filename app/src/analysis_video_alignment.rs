//! Camera-generic video/audio and cross-source alignment workflow.
use super::*;

fn vehicle_forward_axis_label(axis: usize) -> &'static str {
    match axis {
        0 => "+X points forward",
        1 => "−X points forward",
        2 => "+Y points forward",
        3 => "−Y points forward",
        4 => "+Z points forward",
        5 => "−Z points forward",
        _ => "+X points forward",
    }
}

fn vehicle_forward_axis_vector(axis: usize) -> [f64; 3] {
    match axis {
        0 => [1.0, 0.0, 0.0],
        1 => [-1.0, 0.0, 0.0],
        2 => [0.0, 1.0, 0.0],
        3 => [0.0, -1.0, 0.0],
        4 => [0.0, 0.0, 1.0],
        5 => [0.0, 0.0, -1.0],
        _ => [1.0, 0.0, 0.0],
    }
}

fn normalized_alignment_channel(name: &str) -> String {
    name.to_ascii_lowercase().replace([' ', '-'], "_")
}

fn alignment_channel_family(name: &str) -> Option<&'static str> {
    match normalized_alignment_channel(name).as_str() {
        "lateral_g" | "gps_lateral_acceleration" | "lateral_acceleration" => Some("lateral"),
        "gps_speed" | "speed" => Some("speed"),
        "yaw_rate" | "gps_yaw_rate" => Some("yaw"),
        "rpm" => Some("rpm"),
        "combined_g" => Some("combined_g"),
        _ => None,
    }
}

fn alignment_channel_rank(name: &str) -> usize {
    match alignment_channel_family(name) {
        Some("lateral") => 0,
        Some("speed") => 1,
        Some("yaw") => 2,
        Some("rpm") => 3,
        Some("combined_g") => 4,
        _ => 5,
    }
}

fn alignment_channels_match(left: &str, right: &str) -> bool {
    normalized_alignment_channel(left) == normalized_alignment_channel(right)
        || alignment_channel_family(left)
            .zip(alignment_channel_family(right))
            .is_some_and(|(left, right)| left == right)
}

impl AnalysisApp {
    pub(super) fn start_pending_video_audio_sync(&mut self, tools: Option<&FfmpegTools>) {
        let Some(tools) = tools.cloned() else {
            return;
        };
        let pending = self
            .workspace
            .recordings
            .iter()
            .filter_map(|recording| {
                let video = recording.video_path.clone()?;
                recording.sources.iter().find_map(|source| {
                    (is_camera_telemetry_source(source)
                        && self.data.contains_key(&source.id)
                        && source
                            .settings
                            .get(VIDEO_AUDIO_OFFSET_KEY)
                            .and_then(Value::as_f64)
                            .is_none()
                        && !self.video_audio_jobs.contains_key(&source.id)
                        && !self.video_audio_results.contains_key(&source.id))
                    .then(|| (recording.id, source.clone(), video.clone()))
                })
            })
            .collect::<Vec<_>>();
        for (recording_id, source, video) in pending {
            self.load_serial = self.load_serial.wrapping_add(1);
            let serial = self.load_serial;
            self.video_audio_jobs.insert(source.id, serial);
            let tx = self.tx.clone();
            let tools = tools.clone();
            std::thread::spawn(move || {
                let result = (|| -> Result<AlignmentResult, String> {
                    let source_audio = overlay_media::extract_mono_pcm(&tools, &source.path, 2_000)
                        .map_err(|error| error.to_string())?;
                    let video_audio = overlay_media::extract_mono_pcm(&tools, &video, 2_000)
                        .map_err(|error| error.to_string())?;
                    if source_audio.is_empty() || video_audio.is_empty() {
                        return Err(
                            "Both the camera recording and exported video need audio".into()
                        );
                    }
                    Ok(overlay_media::align_audio(
                        &source_audio,
                        &video_audio,
                        2_000,
                    ))
                })();
                let _ = tx.send(Event::VideoAudioAligned(
                    serial,
                    recording_id,
                    source.id,
                    result,
                ));
            });
        }
    }

    fn camera_calibration_ui(
        &mut self,
        ui: &mut egui::Ui,
        recording_id: RecordingId,
        camera_source: SourceId,
        camera_video_offset: Option<f64>,
    ) {
        let has_raw_imu = self.data.get(&camera_source).is_some_and(|data| {
            [
                "raw_accel_x",
                "raw_accel_y",
                "raw_accel_z",
                "raw_gyro_x",
                "raw_gyro_y",
                "raw_gyro_z",
            ]
            .iter()
            .all(|name| data.raw.named(name).is_some())
        });
        let calibration_notes = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == recording_id)
            .and_then(|recording| recording.overlay_snapshot.as_ref())
            .and_then(|snapshot| snapshot.camera_calibration.notes.as_deref())
            .map(str::to_owned);
        let mut apply = false;
        ui.collapsing("Camera orientation / vehicle axes", |ui| {
            if let Some(notes) = &calibration_notes {
                ui.colored_label(
                    egui::Color32::LIGHT_GREEN,
                    "Vehicle-frame acceleration and rotation channels are available.",
                );
                ui.small(notes);
            } else {
                ui.label("Calibrate the camera IMU before correlating vehicle-frame lateral acceleration.");
            }
            if !has_raw_imu {
                ui.weak("Waiting for raw accelerometer and gyroscope channels…");
                return;
            }
            let draft = self
                .vehicle_calibration_drafts
                .entry(camera_source)
                .or_default();
            ui.small(if draft.source_time {
                "The stationary interval uses raw camera-source time."
            } else {
                "The stationary interval uses exported-video time."
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut draft.start)
                        .speed(0.1)
                        .prefix("From ")
                        .suffix(" s"),
                );
                ui.add(
                    egui::DragValue::new(&mut draft.end)
                        .speed(0.1)
                        .prefix("to ")
                        .suffix(" s"),
                );
            });
            ui.checkbox(
                &mut draft.auto_stationary,
                "Auto-find a stationary interval when this range is moving",
            );
            ui.checkbox(
                &mut draft.source_time,
                "Use raw camera-source time (advanced)",
            );
            egui::ComboBox::from_label("Camera-forward axis")
                .selected_text(vehicle_forward_axis_label(draft.forward_axis))
                .show_ui(ui, |ui| {
                    for axis in 0..6 {
                        ui.selectable_value(
                            &mut draft.forward_axis,
                            axis,
                            vehicle_forward_axis_label(axis),
                        );
                    }
                });
            ui.add(
                egui::Slider::new(&mut draft.roll_degrees, -30.0..=30.0)
                    .step_by(0.1)
                    .text("Fine roll trim°"),
            );
            ui.add(
                egui::Slider::new(&mut draft.pitch_degrees, -30.0..=30.0)
                    .step_by(0.1)
                    .text("Fine pitch trim°"),
            );
            ui.add(
                egui::Slider::new(&mut draft.yaw_degrees, -90.0..=90.0)
                    .step_by(0.1)
                    .text("Fine yaw trim°"),
            );
            ui.small("Positive pitch raises the nose; positive roll raises the left side; positive yaw turns left.");
            ui.add(
                egui::Slider::new(&mut draft.low_pass_hz, 0.5..=50.0)
                    .logarithmic(true)
                    .text("Derived G smoothing cutoff Hz"),
            );
            let time_ready = draft.source_time || camera_video_offset.is_some();
            if ui
                .add_enabled(
                    time_ready,
                    egui::Button::new("Apply calibration & calculate vehicle channels"),
                )
                .clicked()
            {
                apply = true;
            }
            if !time_ready {
                ui.weak("Camera/video audio alignment must finish before using video time.");
            }
        });
        if apply {
            self.apply_camera_calibration(recording_id, camera_source, camera_video_offset);
        }
    }

    fn apply_camera_calibration(
        &mut self,
        recording_id: RecordingId,
        camera_source_id: SourceId,
        camera_video_offset: Option<f64>,
    ) {
        let Some(draft) = self
            .vehicle_calibration_drafts
            .get(&camera_source_id)
            .cloned()
        else {
            return;
        };
        let Some(source) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == recording_id)
            .and_then(|recording| {
                recording
                    .sources
                    .iter()
                    .find(|source| source.id == camera_source_id)
            })
            .cloned()
        else {
            return;
        };
        let Some(raw) = self
            .data
            .get(&camera_source_id)
            .map(|data| data.raw.clone())
        else {
            self.message = "Camera telemetry is still loading.".into();
            return;
        };
        let source_time_offset = if draft.source_time {
            0.0
        } else if let Some(offset) = camera_video_offset {
            offset
        } else {
            self.message = "Wait for camera/video audio alignment, or use raw source time.".into();
            return;
        };
        let mut processed = raw.as_ref().clone();
        let source_cutoff = source
            .settings
            .get("low_pass_enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            .then(|| {
                source
                    .settings
                    .get("low_pass_hz")
                    .and_then(Value::as_f64)
                    .unwrap_or(8.0)
            });
        if let Err(error) = prepare_loaded_dataset(&mut processed, source_cutoff, None) {
            self.message = error;
            return;
        }
        let outcome = match fit_vehicle_frame_calibration(
            &processed,
            VehicleFrameCalibrationRequest {
                start_time: draft.start + source_time_offset,
                end_time: draft.end + source_time_offset,
                auto_find_stationary: draft.auto_stationary,
                forward_hint: vehicle_forward_axis_vector(draft.forward_axis),
                fine_roll_radians: draft.roll_degrees.to_radians(),
                fine_pitch_radians: draft.pitch_degrees.to_radians(),
                fine_yaw_radians: draft.yaw_degrees.to_radians(),
                low_pass_hz: draft.low_pass_hz,
            },
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.message = format!("Camera calibration failed: {error}");
                return;
            }
        };
        let mut calibration = outcome.calibration;
        calibration.notes = Some(format!(
            "Stationary source {:.3}–{:.3}s; forward {}; trim roll {:.1}°, pitch {:.1}°, yaw {:.1}°",
            outcome.interval_start,
            outcome.interval_end,
            vehicle_forward_axis_label(draft.forward_axis),
            draft.roll_degrees,
            draft.pitch_degrees,
            draft.yaw_degrees,
        ));
        if !add_derived_inertial_channels(&mut processed, &calibration, 1_000.0) {
            self.message = "Camera calibration could not create vehicle-frame channels.".into();
            return;
        }
        let Some(fallback_snapshot) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == recording_id)
            .map(project_from_recording)
        else {
            return;
        };
        if let Some(recording) = self
            .workspace
            .recordings
            .iter_mut()
            .find(|recording| recording.id == recording_id)
        {
            let snapshot = recording.overlay_snapshot.get_or_insert(fallback_snapshot);
            snapshot.camera_calibration = calibration;
        }
        if let Some(data) = self.data.get_mut(&camera_source_id) {
            data.processed = Arc::new(processed);
        }
        self.video_alignment_channels.remove(&recording_id);
        self.video_alignment_results.remove(&recording_id);
        self.video_alignment_jobs.remove(&recording_id);
        self.message = if outcome.acceleration_motion_rms > 1.0
            || outcome.gyroscope_motion_rms > 0.15
        {
            format!(
                "Calibration applied, but the interval still appears to be moving (accel RMS {:.2} m/s², gyro RMS {:.2} rad/s).",
                outcome.acceleration_motion_rms, outcome.gyroscope_motion_rms
            )
        } else if outcome.used_automatic_interval {
            "Calibration applied using the quietest stationary interval; lateral acceleration is now available for alignment.".into()
        } else {
            "Calibration applied; lateral acceleration is now available for alignment.".into()
        };
        self.changed();
    }

    fn video_alignment_channel_choices(&self, source_id: SourceId) -> Vec<(ChannelRef, String)> {
        let Some(dataset) = self.data.get(&source_id).map(|data| &data.processed) else {
            return vec![];
        };
        let mut channels = dataset
            .channels
            .values()
            .filter(|channel| {
                channel.descriptor.interpolation == Interpolation::Linear
                    && channel.series.samples.len() >= 24
            })
            .collect::<Vec<_>>();
        channels.sort_by(|left, right| {
            alignment_channel_rank(&left.descriptor.name)
                .cmp(&alignment_channel_rank(&right.descriptor.name))
                .then_with(|| right.series.samples.len().cmp(&left.series.samples.len()))
                .then_with(|| left.descriptor.name.cmp(&right.descriptor.name))
        });
        channels
            .into_iter()
            .map(|channel| {
                (
                    ChannelRef {
                        source_id,
                        channel_id: channel.descriptor.id,
                    },
                    format!(
                        "{} ({}) · {} samples",
                        channel.descriptor.name,
                        channel.descriptor.unit.symbol(),
                        channel.series.samples.len()
                    ),
                )
            })
            .collect()
    }

    fn default_video_alignment_channels(
        &self,
        target_source: SourceId,
        camera_source: SourceId,
    ) -> Option<(ChannelRef, ChannelRef)> {
        let target = self.data.get(&target_source)?.processed.as_ref();
        let camera = self.data.get(&camera_source)?.processed.as_ref();
        target
            .channels
            .values()
            .filter(|channel| {
                channel.descriptor.interpolation == Interpolation::Linear
                    && channel.series.samples.len() >= 24
            })
            .flat_map(|target_channel| {
                camera
                    .channels
                    .values()
                    .filter(move |camera_channel| {
                        camera_channel.descriptor.interpolation == Interpolation::Linear
                            && camera_channel.series.samples.len() >= 24
                            && target_channel.descriptor.unit.family().is_some()
                            && target_channel.descriptor.unit.family()
                                == camera_channel.descriptor.unit.family()
                    })
                    .map(move |camera_channel| {
                        let channels_match = alignment_channels_match(
                            &target_channel.descriptor.name,
                            &camera_channel.descriptor.name,
                        );
                        let score = (
                            !channels_match,
                            alignment_channel_rank(&target_channel.descriptor.name)
                                .max(alignment_channel_rank(&camera_channel.descriptor.name)),
                            usize::MAX
                                - target_channel
                                    .series
                                    .samples
                                    .len()
                                    .min(camera_channel.series.samples.len()),
                        );
                        (
                            score,
                            ChannelRef {
                                source_id: target_source,
                                channel_id: target_channel.descriptor.id,
                            },
                            ChannelRef {
                                source_id: camera_source,
                                channel_id: camera_channel.descriptor.id,
                            },
                        )
                    })
            })
            .min_by(|left, right| left.0.cmp(&right.0))
            .map(|(_, target, camera)| (target, camera))
    }

    fn start_video_alignment_estimate(&mut self, recording_id: RecordingId) {
        let Some(recording) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == recording_id)
        else {
            return;
        };
        let Some((target_channel_ref, camera_channel_ref)) =
            self.video_alignment_channels.get(&recording_id).cloned()
        else {
            return;
        };
        let Some(camera_source) = recording
            .sources
            .iter()
            .find(|source| source.id == camera_channel_ref.source_id)
        else {
            return;
        };
        let Some(camera_video_offset_seconds) = camera_source
            .settings
            .get(VIDEO_AUDIO_OFFSET_KEY)
            .and_then(Value::as_f64)
        else {
            return;
        };
        let Some(target) = self
            .data
            .get(&target_channel_ref.source_id)
            .and_then(|data| data.processed.channel(target_channel_ref.channel_id))
            .cloned()
        else {
            return;
        };
        let Some(camera) = self
            .data
            .get(&camera_channel_ref.source_id)
            .and_then(|data| data.processed.channel(camera_channel_ref.channel_id))
            .cloned()
        else {
            return;
        };
        let config = CorrelationConfig::default();
        let Some((min_lag_seconds, max_lag_seconds)) = feasible_correlation_lag_range(
            &camera.series,
            &target.series,
            config.min_overlap_seconds,
        ) else {
            return;
        };
        self.load_serial = self.load_serial.wrapping_add(1);
        let serial = self.load_serial;
        self.video_alignment_jobs.insert(recording_id, serial);
        self.video_alignment_results.remove(&recording_id);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = correlate_channel_series(
                &camera.series,
                &target.series,
                &CorrelationConfig {
                    min_lag_seconds,
                    max_lag_seconds,
                    ..config
                },
            )
            .map(|result| VideoAlignmentCandidate {
                recording_id,
                target_source: target_channel_ref.source_id,
                camera_source: camera_channel_ref.source_id,
                target_channel: target_channel_ref,
                camera_channel: camera_channel_ref,
                camera_video_offset_seconds,
                result,
            })
            .map_err(|error| error.to_string());
            let _ = tx.send(Event::VideoAlignmentEstimated(serial, recording_id, result));
        });
    }

    fn apply_video_alignment(&mut self, recording_id: RecordingId) {
        let Some(Ok(candidate)) = self.video_alignment_results.get(&recording_id).cloned() else {
            return;
        };
        let Some(recording) = self
            .workspace
            .recordings
            .iter_mut()
            .find(|recording| recording.id == recording_id)
        else {
            return;
        };
        if candidate.recording_id != recording.id
            || candidate.target_source != recording.primary_source
            || self.video_alignment_channels.get(&recording_id)
                != Some(&(
                    candidate.target_channel.clone(),
                    candidate.camera_channel.clone(),
                ))
        {
            return;
        }
        let Some(target_offset) = recording
            .sources
            .iter()
            .find(|source| source.id == candidate.target_source)
            .map(|source| source.alignment.offset_seconds)
        else {
            return;
        };
        let lag = candidate.result.target_minus_reference_seconds;
        let (camera_offset, video_offset) = resolved_video_alignment_offsets(
            target_offset,
            lag,
            candidate.camera_video_offset_seconds,
        );
        let Some(camera_source) = recording
            .sources
            .iter_mut()
            .find(|source| source.id == candidate.camera_source)
        else {
            return;
        };
        let current_audio_offset = camera_source
            .settings
            .get(VIDEO_AUDIO_OFFSET_KEY)
            .and_then(Value::as_f64);
        if current_audio_offset != Some(candidate.camera_video_offset_seconds) {
            self.message = "Camera/video audio alignment changed; estimate again.".into();
            self.video_alignment_results.remove(&recording_id);
            return;
        }
        camera_source.alignment.offset_seconds = camera_offset;
        if !camera_source.settings.is_object() {
            camera_source.settings = json!({});
        }
        camera_source.settings[VIDEO_ALIGNMENT_APPLIED_KEY] = json!(true);
        recording.video_offset_seconds = video_offset;
        self.video_alignment_results.remove(&recording_id);
        self.message = format!(
            "Video aligned without changing the logger clock (video offset {:+.3}s, Pearson r {:+.3}). Existing gates and intervals remain valid.",
            recording.video_offset_seconds, candidate.result.correlation_coefficient
        );
        self.changed();
    }

    pub(super) fn video_alignment_ui(
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
            let explicitly_applied = source
                .settings
                .get(VIDEO_ALIGNMENT_APPLIED_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let acceptable_primary_audio = recording.primary_source == source.id
                && source
                    .settings
                    .get(VIDEO_AUDIO_OFFSET_KEY)
                    .and_then(Value::as_f64)
                    .is_some()
                && source
                    .settings
                    .get(VIDEO_AUDIO_PEAK_KEY)
                    .and_then(Value::as_f64)
                    .is_some_and(|peak| peak >= 0.35)
                && source
                    .settings
                    .get(VIDEO_AUDIO_CONFIDENCE_KEY)
                    .and_then(Value::as_f64)
                    .is_some_and(|confidence| confidence >= 2.0);
            explicitly_applied || acceptable_primary_audio
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
            let audio_offset = camera_source
                .settings
                .get(VIDEO_AUDIO_OFFSET_KEY)
                .and_then(Value::as_f64);
            self.camera_calibration_ui(
                ui,
                recording.id,
                camera_source.id,
                audio_offset,
            );
            if self.video_audio_jobs.contains_key(&camera_source.id) {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Matching camera audio to the exported video…");
                });
                return;
            }
            if let Some(Err(error)) = self.video_audio_results.get(&camera_source.id) {
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
            let peak = camera_source
                .settings
                .get(VIDEO_AUDIO_PEAK_KEY)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let confidence = camera_source
                .settings
                .get(VIDEO_AUDIO_CONFIDENCE_KEY)
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
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
                .video_alignment_channels
                .entry(recording.id)
                .or_insert(initial_pair);
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
                self.video_alignment_results.remove(&recording.id);
            }
            if ui
                .add_enabled(
                    !target_choices.is_empty()
                        && !camera_choices.is_empty()
                        && !self.video_alignment_jobs.contains_key(&recording.id),
                    egui::Button::new("Estimate logger ↔ camera alignment"),
                )
                .clicked()
            {
                estimate = true;
            }
            if self.video_alignment_jobs.contains_key(&recording.id) {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Searching the complete feasible overlap…");
                });
            }
            if let Some(result) = self.video_alignment_results.get(&recording.id) {
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
            self.video_audio_results.remove(&source_id);
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
            if !source.settings.is_object() {
                source.settings = json!({});
            }
            source.settings[VIDEO_ALIGNMENT_APPLIED_KEY] = json!(true);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(
        source_id: SourceId,
        name: &str,
        unit: Unit,
        values: impl Fn(f64) -> f64,
    ) -> TelemetryChannel {
        TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::for_source_name(source_id, name),
                name: name.into(),
                quantity: Quantity::Acceleration,
                unit,
                interpolation: Interpolation::Linear,
                description: None,
            },
            series: ChannelSeries::new(
                (0..=60)
                    .map(|index| {
                        let time = f64::from(index) * 0.1;
                        TimedSample {
                            time,
                            value: values(time),
                        }
                    })
                    .collect(),
            ),
        }
    }

    #[test]
    fn analysis_calibration_creates_and_prefers_vehicle_lateral_acceleration() {
        let logger_id = SourceId::new();
        let camera_id = SourceId::new();
        let recording_id = RecordingId::new();
        let logger = SourceConfig {
            id: logger_id,
            name: "logger".into(),
            adapter: "synthetic".into(),
            path: "logger".into(),
            alignment: Default::default(),
            settings: json!({}),
            unknown: Default::default(),
        };
        let camera = SourceConfig {
            id: camera_id,
            name: "camera".into(),
            adapter: "insta360".into(),
            path: "camera.lrv".into(),
            alignment: Default::default(),
            settings: json!({}),
            unknown: Default::default(),
        };
        let mut app = AnalysisApp::new();
        app.workspace.recordings = vec![Recording {
            id: recording_id,
            name: "run".into(),
            sources: vec![logger, camera],
            primary_source: logger_id,
            video_path: Some("video.mp4".into()),
            video_offset_seconds: 0.0,
            segments: vec![],
            overlay_snapshot: None,
            unknown: Default::default(),
        }];
        let mut logger_data = TelemetryDataset {
            source_id: logger_id,
            ..Default::default()
        };
        logger_data.insert(channel(
            logger_id,
            "gps_lateral_acceleration",
            Unit::StandardGravity,
            |time| time.sin(),
        ));
        app.data
            .insert(logger_id, SourceData::from_dataset(Arc::new(logger_data)));
        let mut camera_data = TelemetryDataset {
            source_id: camera_id,
            ..Default::default()
        };
        for item in [
            channel(
                camera_id,
                "raw_accel_x",
                Unit::MeterPerSecondSquared,
                |_| 0.0,
            ),
            channel(
                camera_id,
                "raw_accel_y",
                Unit::MeterPerSecondSquared,
                |time| {
                    if time <= 2.0 { 0.0 } else { time.sin() }
                },
            ),
            channel(
                camera_id,
                "raw_accel_z",
                Unit::MeterPerSecondSquared,
                |_| 9.80665,
            ),
            channel(camera_id, "raw_gyro_x", Unit::RadianPerSecond, |_| 0.0),
            channel(camera_id, "raw_gyro_y", Unit::RadianPerSecond, |_| 0.0),
            channel(camera_id, "raw_gyro_z", Unit::RadianPerSecond, |_| 0.0),
        ] {
            camera_data.insert(item);
        }
        app.data
            .insert(camera_id, SourceData::from_dataset(Arc::new(camera_data)));
        app.vehicle_calibration_drafts.insert(
            camera_id,
            VehicleCalibrationDraft {
                source_time: true,
                auto_stationary: false,
                ..Default::default()
            },
        );

        app.apply_camera_calibration(recording_id, camera_id, None);

        assert!(app.data[&camera_id].processed.named("lateral_g").is_some());
        assert!(
            app.workspace.recordings[0]
                .overlay_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.camera_calibration.notes.as_ref())
                .is_some()
        );
        let (target, camera) = app
            .default_video_alignment_channels(logger_id, camera_id)
            .unwrap();
        assert_eq!(
            app.data[&logger_id]
                .processed
                .channel(target.channel_id)
                .unwrap()
                .descriptor
                .name,
            "gps_lateral_acceleration"
        );
        assert_eq!(
            app.data[&camera_id]
                .processed
                .channel(camera.channel_id)
                .unwrap()
                .descriptor
                .name,
            "lateral_g"
        );
    }
}
