//! Camera IMU calibration into the vehicle coordinate frame.
use super::super::*;
use crate::ui_kit::{Tone, widgets};

const RAW_IMU_CHANNELS: [&str; 6] = [
    "raw_accel_x",
    "raw_accel_y",
    "raw_accel_z",
    "raw_gyro_x",
    "raw_gyro_y",
    "raw_gyro_z",
];

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

impl AnalysisApp {
    pub(super) fn camera_calibration_ui(
        &mut self,
        ui: &mut egui::Ui,
        recording_id: RecordingId,
        camera_source: SourceId,
        camera_video_offset: Option<f64>,
    ) {
        let has_raw_imu = self.data.get(&camera_source).is_some_and(|data| {
            RAW_IMU_CHANNELS
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
        ui.collapsing("Camera orientation", |ui| {
            if let Some(notes) = &calibration_notes {
                widgets::callout(ui, Tone::Good, "Vehicle-frame acceleration and rotation channels are available.",
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
                .video_sync
                .recording_mut(recording_id)
                .calibration_drafts
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

    /// Calibrates each newly loaded camera with the default orientation: the
    /// camera faces the direction of travel and is levelled from the quietest
    /// stationary stretch. Most mounts are like this, and the vehicle-frame
    /// channels are what logger correlation needs. A wrong guess is corrected
    /// by editing the orientation and applying again; this runs once per
    /// source and never overrides a saved or manual calibration.
    pub(in crate::analysis_app) fn apply_default_camera_calibrations(&mut self) {
        let pending = self
            .workspace
            .recordings
            .iter()
            .flat_map(|recording| {
                let calibrated = recording
                    .overlay_snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.camera_calibration.notes.is_some());
                recording
                    .sources
                    .iter()
                    .filter(move |source| {
                        !calibrated
                            && builtin_adapter_capabilities(&source.adapter)
                                .vehicle_frame_calibration
                    })
                    .map(move |source| (recording, source))
            })
            .filter(|(recording, source)| {
                let sync = self.video_sync.recording(recording.id);
                let has_imu = self.data.get(&source.id).is_some_and(|data| {
                    RAW_IMU_CHANNELS
                        .iter()
                        .all(|name| data.raw.named(name).is_some())
                });
                // With a video, the stationary range is in video time, which
                // needs the audio match first (or its failure, which leaves
                // raw camera time).
                let time_settled = recording.video_path.is_none()
                    || CameraVideoSyncMetadata::read(&source.settings)
                        .camera_minus_video_seconds
                        .is_some()
                    || sync.is_some_and(|state| state.audio_results.contains_key(&source.id));
                has_imu
                    && time_settled
                    && sync.is_none_or(|state| {
                        !state.auto_calibration_tried.contains(&source.id)
                            && !state.audio_jobs.contains_key(&source.id)
                    })
            })
            .map(|(recording, source)| {
                (
                    recording.id,
                    source.id,
                    CameraVideoSyncMetadata::read(&source.settings).camera_minus_video_seconds,
                )
            })
            .collect::<Vec<_>>();
        for (recording_id, source_id, camera_video_offset) in pending {
            let state = self.video_sync.recording_mut(recording_id);
            state.auto_calibration_tried.insert(source_id);
            state
                .calibration_drafts
                .entry(source_id)
                .or_default()
                .source_time = camera_video_offset.is_none();
            if self.apply_camera_calibration(recording_id, source_id, camera_video_offset) {
                self.message = "Assumed the camera faces the direction of travel and levelled it from a stationary moment. If that is wrong, change it under Camera orientation in Video alignment and apply again.".into();
            }
        }
    }

    /// Fits and applies the draft calibration; false when it could not be.
    pub(super) fn apply_camera_calibration(
        &mut self,
        recording_id: RecordingId,
        camera_source_id: SourceId,
        camera_video_offset: Option<f64>,
    ) -> bool {
        let Some(draft) = self
            .video_sync
            .recording(recording_id)
            .and_then(|state| state.calibration_drafts.get(&camera_source_id))
            .cloned()
        else {
            return false;
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
            return false;
        };
        let Some(raw) = self
            .data
            .get(&camera_source_id)
            .map(|data| data.raw.clone())
        else {
            self.message = "Camera telemetry is still loading.".into();
            return false;
        };
        let source_time_offset = if draft.source_time {
            0.0
        } else if let Some(offset) = camera_video_offset {
            offset
        } else {
            self.message = "Wait for camera/video audio alignment, or use raw source time.".into();
            return false;
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
            return false;
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
                return false;
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
            return false;
        }
        let Some(fallback_snapshot) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == recording_id)
            .map(project_from_recording)
        else {
            return false;
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
        let sync_state = self.video_sync.recording_mut(recording_id);
        sync_state.channels = None;
        sync_state.alignment_result = None;
        sync_state.alignment_job = None;
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
        true
    }
}
