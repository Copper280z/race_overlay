//! Telemetry source management, camera calibration, and correlation setup.
use super::super::INSTA360_IMU_SAMPLE_RATE_HZ;
use super::super::policy::{forward_axis_label, forward_axis_vector};
use super::OverlayEditor;
use overlay_core::{
    ChannelBinding, ChannelRef, SourceId, VehicleFrameCalibrationRequest,
    add_derived_inertial_channels, fit_vehicle_frame_calibration,
};

impl OverlayEditor {
    pub(super) fn calibrate_selected(&mut self) {
        let Some(source_index) = self.selected_source_index() else {
            self.status = "Select the camera telemetry source".into();
            return;
        };
        let Some(project) = self.project() else {
            return;
        };
        let source = project.sources[source_index].clone();
        let Some(dataset_index) = self
            .session
            .datasets()
            .iter()
            .position(|d| d.source_id == source.id)
        else {
            self.status = "That source is still loading".into();
            return;
        };
        let alignment = if self.source_editor.calibration_source_time {
            0.0
        } else {
            source.alignment.offset_seconds
        };
        let outcome = match fit_vehicle_frame_calibration(
            &self.session.datasets()[dataset_index],
            VehicleFrameCalibrationRequest {
                start_time: self.source_editor.calibration_start + alignment,
                end_time: self.source_editor.calibration_end + alignment,
                auto_find_stationary: self.source_editor.calibration_auto_stationary,
                forward_hint: forward_axis_vector(self.source_editor.calibration_forward_axis),
                fine_roll_radians: self.source_editor.calibration_roll_deg.to_radians(),
                fine_pitch_radians: self.source_editor.calibration_pitch_deg.to_radians(),
                fine_yaw_radians: self.source_editor.calibration_yaw_deg.to_radians(),
                low_pass_hz: self.source_editor.calibration_low_pass_hz,
            },
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.status = error.to_string();
                return;
            }
        };
        let mut calibration = outcome.calibration;
        calibration.notes = Some(format!(
            "Stationary source {:.3}–{:.3}s; forward {}; trim roll {:.1}°, pitch {:.1}°, yaw {:.1}°",
            outcome.interval_start,
            outcome.interval_end,
            forward_axis_label(self.source_editor.calibration_forward_axis),
            self.source_editor.calibration_roll_deg,
            self.source_editor.calibration_pitch_deg,
            self.source_editor.calibration_yaw_deg,
        ));
        self.project_mut().unwrap().camera_calibration = calibration.clone();
        add_derived_inertial_channels(
            &mut self.session.datasets_mut()[dataset_index],
            &calibration,
            INSTA360_IMU_SAMPLE_RATE_HZ,
        );
        self.auto_bind_g_meter(source.id);
        let accel_rms = outcome.acceleration_motion_rms;
        let gyro_rms = outcome.gyroscope_motion_rms;
        self.status = if accel_rms > 1.0 || gyro_rms > 0.15 {
            format!(
                "Calibration applied, but the interval appears to be moving (accel RMS {accel_rms:.2} m/s², gyro RMS {gyro_rms:.2} rad/s). Choose a stationary interval."
            )
        } else if outcome.used_automatic_interval {
            format!(
                "The selected video interval was moving; calibration used the quietest raw-recording interval automatically ({:.1} Hz filter)",
                calibration.low_pass_hz
            )
        } else {
            format!(
                "Calibration applied at {:.1} Hz; derived G and turn-rate channels updated",
                calibration.low_pass_hz
            )
        };
        self.refresh_overlay();
    }

    pub(super) fn auto_bind_g_meter(&mut self, source_id: SourceId) {
        let Some(dataset) = self
            .session
            .datasets()
            .iter()
            .find(|d| d.source_id == source_id)
        else {
            return;
        };
        let x = dataset.named("lateral_g").map(|c| c.descriptor.id);
        let y = dataset.named("longitudinal_g").map(|c| c.descriptor.id);
        let (Some(x), Some(y)) = (x, y) else { return };
        let Some(project) = self.project_mut() else {
            return;
        };
        if let Some(widget) = project.widgets.iter_mut().find(|w| w.kind == "xy_dot") {
            widget.bindings = vec![
                ChannelBinding::new(
                    "x",
                    ChannelRef {
                        source_id,
                        channel_id: x,
                    },
                ),
                ChannelBinding::new(
                    "y",
                    ChannelRef {
                        source_id,
                        channel_id: y,
                    },
                ),
            ];
        }
    }
}
