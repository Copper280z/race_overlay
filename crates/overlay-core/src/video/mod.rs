//! Camera-relative video geometry. Optical calibration is independent of vehicle telemetry.
mod insta360;
mod motion;
pub use insta360::{RawCameraVideo, read_insta360_video, same_camera_recording};
pub use motion::{CameraMotion, GyroSample, MotionTrajectory, Quaternion};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoProcessingConfig {
    pub input_kind: String,
    pub view: ViewOrientation,
    pub horizontal_fov_degrees: f64,
    /// Stable strings keep unsupported future modes round-trippable.
    pub seam: String,
    pub feather_degrees: f64,
    pub stabilization: StabilizationConfig,
    pub output_height: u32,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}
impl Default for VideoProcessingConfig {
    fn default() -> Self {
        Self {
            input_kind: "insta360_x4_air".into(),
            view: ViewOrientation::default(),
            horizontal_fov_degrees: 90.0,
            seam: "hard_cut".into(),
            feather_degrees: 2.0,
            stabilization: StabilizationConfig::default(),
            output_height: 1080,
            unknown: BTreeMap::new(),
        }
    }
}
impl VideoProcessingConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.input_kind != "insta360_x4_air" {
            return Err(format!("Unsupported raw video input: {}", self.input_kind));
        }
        if !matches!(self.seam.as_str(), "hard_cut" | "feather" | "adaptive") {
            return Err(format!("Unsupported stitching mode: {}", self.seam));
        }
        if !self.horizontal_fov_degrees.is_finite()
            || !(30.0..=150.0).contains(&self.horizontal_fov_degrees)
            || !self.feather_degrees.is_finite()
            || !(0.0..=20.0).contains(&self.feather_degrees)
            || ![self.view.yaw, self.view.pitch, self.view.roll]
                .iter()
                .all(|v| v.is_finite())
            || !self.stabilization.sigma_seconds.is_finite()
            || !(0.05..=1.0).contains(&self.stabilization.sigma_seconds)
            || !matches!(self.output_height, 1080 | 2160)
        {
            return Err("Invalid video view, output size, or smoothing settings".into());
        }
        Ok(())
    }
    pub fn output_size(&self) -> (u32, u32) {
        (self.output_height / 9 * 16, self.output_height)
    }
    pub fn reset_view(&mut self) {
        self.view.yaw = 0.;
        self.view.pitch = 0.;
        self.view.roll = 0.;
        self.horizontal_fov_degrees = 90.0;
    }
    /// Drag moves the image with the pointer, in the current view's local frame.
    pub fn drag(&mut self, dx: f64, dy: f64, width: f64) {
        if width <= 0.0 {
            return;
        }
        let radians = self.horizontal_fov_degrees.to_radians() / width;
        let rotation = self
            .view
            .rotation()
            .multiply(Quaternion::axis_angle([0., 1., 0.], -dx * radians))
            .multiply(Quaternion::axis_angle([1., 0., 0.], dy * radians));
        let matrix = rotation.matrix();
        self.view.pitch = (-matrix[1][2])
            .clamp(-1., 1.)
            .asin()
            .to_degrees()
            .clamp(-89.9, 89.9);
        self.view.yaw = matrix[0][2].atan2(matrix[2][2]).to_degrees();
        self.view.roll = matrix[1][0].atan2(matrix[1][1]).to_degrees();
    }
    pub fn scroll(&mut self, delta: f64) {
        self.horizontal_fov_degrees =
            (self.horizontal_fov_degrees * (-delta * 0.002).exp()).clamp(30.0, 150.0);
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewOrientation {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}
impl ViewOrientation {
    /// Rig axes: X right, Y down, Z toward the front lens. Image Y points down.
    pub fn rotation(&self) -> Quaternion {
        Quaternion::axis_angle([0., 1., 0.], self.yaw.to_radians())
            .multiply(Quaternion::axis_angle(
                [1., 0., 0.],
                self.pitch.to_radians(),
            ))
            .multiply(Quaternion::axis_angle([0., 0., 1.], self.roll.to_radians()))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StabilizationConfig {
    pub enabled: bool,
    pub sigma_seconds: f64,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, Value>,
}
impl Default for StabilizationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sigma_seconds: 0.25,
            unknown: BTreeMap::new(),
        }
    }
}

/// Unified omnidirectional (Mei) projection plus radial/tangential distortion.
#[derive(Clone, Debug)]
pub struct LensCalibration {
    pub xi: f64,
    /// Intrinsics in normalized decoded-image coordinates.
    pub focal: [f64; 2],
    pub center: [f64; 2],
    pub distortion: [f64; 5],
    pub rig_to_lens: Quaternion,
    pub translation: [f64; 3],
    pub max_angle: f64,
}
impl LensCalibration {
    pub fn project(&self, rig_ray: [f64; 3]) -> Option<([f64; 2], f64)> {
        let [x, y, z] = self.rig_to_lens.rotate(rig_ray);
        let len = (x * x + y * y + z * z).sqrt();
        if len <= 1e-12 {
            return None;
        }
        let angle = (z / len).clamp(-1., 1.).acos();
        if angle > self.max_angle {
            return None;
        }
        let denom = z + self.xi * len;
        if denom <= 1e-12 {
            return None;
        }
        let (x, y) = (x / denom, y / denom);
        let r2 = x * x + y * y;
        let [k1, k2, k3, p1, p2] = self.distortion;
        let radial = 1. + r2 * (k1 + r2 * (k2 + r2 * k3));
        let u = self.center[0]
            + self.focal[0] * (x * radial + 2. * p1 * x * y + p2 * (r2 + 2. * x * x));
        let v = self.center[1]
            + self.focal[1] * (y * radial + 2. * p2 * x * y + p1 * (r2 + 2. * y * y));
        (u.is_finite() && v.is_finite() && (0.0..=1.).contains(&u) && (0.0..=1.).contains(&v))
            .then_some(([u, v], angle))
    }
}
#[derive(Clone, Debug)]
pub struct DualLensCalibration {
    pub lenses: [LensCalibration; 2],
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_preserve_unknown_fields_and_bound_gestures() {
        let mut s: VideoProcessingConfig =
            serde_json::from_value(serde_json::json!({"future":7,"view":{"future_axis":8}}))
                .unwrap();
        s.scroll(10000.);
        assert_eq!(s.horizontal_fov_degrees, 30.);
        s.scroll(-10000.);
        assert_eq!(s.horizontal_fov_degrees, 150.);
        s.drag(10000., 10000., 960.);
        assert!(s.view.yaw >= -180. && s.view.yaw < 180.);
        assert!(s.view.pitch.abs() <= 89.9);
        s.reset_view();
        let value = serde_json::to_value(s).unwrap();
        assert_eq!(value["future"], 7);
        assert_eq!(value["view"]["future_axis"], 8);
    }
    #[test]
    fn drag_uses_the_rolled_view_axes() {
        let mut config = VideoProcessingConfig::default();
        config.view.roll = 90.;
        config.drag(10., 0., 100.);
        assert!((config.view.pitch - 9.).abs() < 1e-9);
        assert!(config.view.yaw.abs() < 1e-9);
    }
    #[test]
    fn optical_axis_projects_to_center_and_rear_is_invalid() {
        let l = LensCalibration {
            xi: 1.,
            focal: [0.5; 2],
            center: [0.5; 2],
            distortion: [0.; 5],
            rig_to_lens: Quaternion::IDENTITY,
            translation: [0.; 3],
            max_angle: 100_f64.to_radians(),
        };
        assert_eq!(l.project([0., 0., 1.]).unwrap().0, [0.5, 0.5]);
        assert!(l.project([0., 0., -1.]).is_none());
    }
}
