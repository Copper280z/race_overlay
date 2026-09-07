//! Guided inertial calibration and derived vehicle-frame channels.
use crate::*;
use thiserror::Error;

const G: f64 = 9.80665;
pub type Vec3 = [f64; 3];

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: Vec3) -> Option<Vec3> {
    let n = norm(a);
    (n > 1e-12).then(|| [a[0] / n, a[1] / n, a[2] / n])
}
fn mul(r: [[f64; 3]; 3], v: Vec3) -> Vec3 {
    [dot(r[0], v), dot(r[1], v), dot(r[2], v)]
}

/// Robust coordinate-wise median of a stationary accelerometer window.
pub fn median_gravity(samples: impl IntoIterator<Item = Vec3>) -> Option<Vec3> {
    let mut v: Vec<Vec3> = samples
        .into_iter()
        .filter(|x| x.iter().all(|n| n.is_finite()))
        .collect();
    if v.is_empty() {
        return None;
    }
    let mid = v.len() / 2;
    let mut out = [0.; 3];
    for axis in 0..3 {
        v.sort_by(|a, b| a[axis].total_cmp(&b[axis]));
        out[axis] = if v.len().is_multiple_of(2) {
            (v[mid - 1][axis] + v[mid][axis]) * 0.5
        } else {
            v[mid][axis]
        };
    }
    Some(out)
}

/// Build a sensor-to-vehicle transform from a stationary gravity vector and a
/// yaw angle around measured sensor-up. Yaw zero is a stable horizontal basis:
/// projected sensor X, or projected sensor Z if X is nearly vertical.
/// Vehicle axes are forward, left, up. `flips` is applied before fitting.
pub fn guided_sensor_to_vehicle(
    gravity: Vec3,
    forward_yaw_radians: f64,
    flips: [bool; 3],
) -> Option<[[f64; 3]; 3]> {
    let g = [
        if flips[0] { -gravity[0] } else { gravity[0] },
        if flips[1] { -gravity[1] } else { gravity[1] },
        if flips[2] { -gravity[2] } else { gravity[2] },
    ];
    // At rest an accelerometer reports up (specific force); use it as vehicle up.
    let up = unit(g)?;
    let project_horizontal = |basis: Vec3| {
        [
            basis[0] - dot(basis, up) * up[0],
            basis[1] - dot(basis, up) * up[1],
            basis[2] - dot(basis, up) * up[2],
        ]
    };
    let zero = unit(project_horizontal([1.0, 0.0, 0.0]))
        .or_else(|| unit(project_horizontal([0.0, 0.0, 1.0])))?;
    let ninety = unit(cross(up, zero))?;
    let forward = unit([
        zero[0] * forward_yaw_radians.cos() + ninety[0] * forward_yaw_radians.sin(),
        zero[1] * forward_yaw_radians.cos() + ninety[1] * forward_yaw_radians.sin(),
        zero[2] * forward_yaw_radians.cos() + ninety[2] * forward_yaw_radians.sin(),
    ])?;
    let left = unit(cross(up, forward))?;
    // The basis above is expressed in the optionally flipped sensor frame.
    // Compose that fit with the flip matrix so the returned transform can be
    // applied directly to the original (unflipped) samples.
    let mut transform = [forward, left, up];
    for row in &mut transform {
        for axis in 0..3 {
            if flips[axis] {
                row[axis] = -row[axis];
            }
        }
    }
    Some(transform)
}

/// Build a sensor-to-vehicle transform from a stationary gravity vector and a
/// concrete sensor-space direction that points toward the vehicle nose. This
/// is the UI-friendly alternative to specifying yaw relative to an implicit
/// sensor basis. `fine_yaw_radians` rotates the selected direction around up.
pub fn guided_sensor_to_vehicle_from_forward(
    gravity: Vec3,
    forward_hint: Vec3,
    fine_yaw_radians: f64,
    flips: [bool; 3],
) -> Option<[[f64; 3]; 3]> {
    guided_sensor_to_vehicle_from_forward_with_trim(
        gravity,
        forward_hint,
        0.0,
        0.0,
        fine_yaw_radians,
        flips,
    )
}

/// Build a sensor-to-vehicle transform with intrinsic fine-angle trims.
/// Vehicle axes are forward, left, up. Positive yaw turns forward toward
/// vehicle-left, positive pitch raises the nose, and positive roll raises the
/// vehicle-left side. Trims are applied in yaw, pitch, then roll order.
pub fn guided_sensor_to_vehicle_from_forward_with_trim(
    gravity: Vec3,
    forward_hint: Vec3,
    fine_roll_radians: f64,
    fine_pitch_radians: f64,
    fine_yaw_radians: f64,
    flips: [bool; 3],
) -> Option<[[f64; 3]; 3]> {
    let flip = |vector: Vec3| {
        [
            if flips[0] { -vector[0] } else { vector[0] },
            if flips[1] { -vector[1] } else { vector[1] },
            if flips[2] { -vector[2] } else { vector[2] },
        ]
    };
    let up = unit(flip(gravity))?;
    let hint = flip(forward_hint);
    let base_forward = unit([
        hint[0] - dot(hint, up) * up[0],
        hint[1] - dot(hint, up) * up[1],
        hint[2] - dot(hint, up) * up[2],
    ])?;
    let ninety = unit(cross(up, base_forward))?;
    let yaw_forward = unit([
        base_forward[0] * fine_yaw_radians.cos() + ninety[0] * fine_yaw_radians.sin(),
        base_forward[1] * fine_yaw_radians.cos() + ninety[1] * fine_yaw_radians.sin(),
        base_forward[2] * fine_yaw_radians.cos() + ninety[2] * fine_yaw_radians.sin(),
    ])?;
    let yaw_left = unit(cross(up, yaw_forward))?;
    let pitch_forward = unit([
        yaw_forward[0] * fine_pitch_radians.cos() + up[0] * fine_pitch_radians.sin(),
        yaw_forward[1] * fine_pitch_radians.cos() + up[1] * fine_pitch_radians.sin(),
        yaw_forward[2] * fine_pitch_radians.cos() + up[2] * fine_pitch_radians.sin(),
    ])?;
    let pitch_up = unit([
        up[0] * fine_pitch_radians.cos() - yaw_forward[0] * fine_pitch_radians.sin(),
        up[1] * fine_pitch_radians.cos() - yaw_forward[1] * fine_pitch_radians.sin(),
        up[2] * fine_pitch_radians.cos() - yaw_forward[2] * fine_pitch_radians.sin(),
    ])?;
    let roll_left = unit([
        yaw_left[0] * fine_roll_radians.cos() + pitch_up[0] * fine_roll_radians.sin(),
        yaw_left[1] * fine_roll_radians.cos() + pitch_up[1] * fine_roll_radians.sin(),
        yaw_left[2] * fine_roll_radians.cos() + pitch_up[2] * fine_roll_radians.sin(),
    ])?;
    let roll_up = unit([
        pitch_up[0] * fine_roll_radians.cos() - yaw_left[0] * fine_roll_radians.sin(),
        pitch_up[1] * fine_roll_radians.cos() - yaw_left[1] * fine_roll_radians.sin(),
        pitch_up[2] * fine_roll_radians.cos() - yaw_left[2] * fine_roll_radians.sin(),
    ])?;
    let mut transform = [pitch_forward, roll_left, roll_up];
    for row in &mut transform {
        for axis in 0..3 {
            if flips[axis] {
                row[axis] = -row[axis];
            }
        }
    }
    Some(transform)
}

/// Bias that makes the measured stationary gravity map to exactly +1 g on
/// vehicle-up for the supplied transform. This includes both scale correction
/// and the compensation needed when pitch/roll trims tilt the vehicle frame.
pub fn accelerometer_bias_for_gravity(
    measured_gravity: Vec3,
    sensor_to_vehicle: [[f64; 3]; 3],
) -> Vec3 {
    [
        measured_gravity[0] - G * sensor_to_vehicle[2][0],
        measured_gravity[1] - G * sensor_to_vehicle[2][1],
        measured_gravity[2] - G * sensor_to_vehicle[2][2],
    ]
}

/// Inputs for fitting raw inertial samples to the vehicle's forward/left/up
/// coordinate system. Times are expressed on the telemetry source clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleFrameCalibrationRequest {
    pub start_time: f64,
    pub end_time: f64,
    pub auto_find_stationary: bool,
    pub forward_hint: Vec3,
    pub fine_roll_radians: f64,
    pub fine_pitch_radians: f64,
    pub fine_yaw_radians: f64,
    pub low_pass_hz: f64,
}

impl Default for VehicleFrameCalibrationRequest {
    fn default() -> Self {
        Self {
            start_time: 0.0,
            end_time: 2.0,
            auto_find_stationary: true,
            forward_hint: [1.0, 0.0, 0.0],
            fine_roll_radians: 0.0,
            fine_pitch_radians: 0.0,
            fine_yaw_radians: 0.0,
            low_pass_hz: 8.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VehicleFrameCalibrationResult {
    pub calibration: CameraCalibration,
    pub interval_start: f64,
    pub interval_end: f64,
    pub used_automatic_interval: bool,
    pub acceleration_motion_rms: f64,
    pub gyroscope_motion_rms: f64,
}

#[derive(Clone, Debug, Error, PartialEq)]
pub enum VehicleFrameCalibrationError {
    #[error("calibration requires raw accelerometer X/Y/Z channels")]
    MissingAccelerometer,
    #[error("calibration requires raw gyroscope X/Y/Z channels")]
    MissingGyroscope,
    #[error("the stationary interval must contain finite, increasing times")]
    InvalidInterval,
    #[error("the derived-G smoothing cutoff must be finite and greater than zero")]
    InvalidLowPass,
    #[error("no matched IMU samples were found in the stationary interval")]
    NoSamples,
    #[error("the selected forward axis is vertical; choose a different sensor-forward axis")]
    VerticalForwardAxis,
}

/// Fit a reusable calibration from raw sensor channels. This owns stationary
/// interval selection and motion diagnostics so Analysis and Overlay apply
/// identical calibration semantics.
pub fn fit_vehicle_frame_calibration(
    dataset: &TelemetryDataset,
    request: VehicleFrameCalibrationRequest,
) -> Result<VehicleFrameCalibrationResult, VehicleFrameCalibrationError> {
    if !request.start_time.is_finite()
        || !request.end_time.is_finite()
        || request.end_time <= request.start_time
    {
        return Err(VehicleFrameCalibrationError::InvalidInterval);
    }
    if !request.low_pass_hz.is_finite() || request.low_pass_hz <= 0.0 {
        return Err(VehicleFrameCalibrationError::InvalidLowPass);
    }
    let accel = named_inertial_triplet(dataset, "raw_accel_")
        .ok_or(VehicleFrameCalibrationError::MissingAccelerometer)?;
    let gyro = named_inertial_triplet(dataset, "raw_gyro_")
        .ok_or(VehicleFrameCalibrationError::MissingGyroscope)?;
    let mut start = request.start_time;
    let mut end = request.end_time;
    let mut accel_window = inertial_window(accel, start, end);
    let mut gyro_window = inertial_window(gyro, start, end);
    let selected_motion = inertial_motion_metrics(&accel_window, &gyro_window);
    let needs_automatic_interval =
        selected_motion.is_none_or(|(accel_rms, gyro_rms)| accel_rms > 1.0 || gyro_rms > 0.15);
    let mut used_automatic_interval = false;
    if request.auto_find_stationary
        && needs_automatic_interval
        && let Some((quiet_start, quiet_end)) =
            quietest_inertial_interval(accel, gyro, (end - start).max(1.0))
    {
        start = quiet_start;
        end = quiet_end;
        accel_window = inertial_window(accel, start, end);
        gyro_window = inertial_window(gyro, start, end);
        used_automatic_interval = true;
    }
    let gravity = median_gravity(accel_window.iter().copied())
        .ok_or(VehicleFrameCalibrationError::NoSamples)?;
    let gyroscope_bias = median_gravity(gyro_window.iter().copied())
        .ok_or(VehicleFrameCalibrationError::NoSamples)?;
    let sensor_to_vehicle = guided_sensor_to_vehicle_from_forward_with_trim(
        gravity,
        request.forward_hint,
        request.fine_roll_radians,
        request.fine_pitch_radians,
        request.fine_yaw_radians,
        [false; 3],
    )
    .ok_or(VehicleFrameCalibrationError::VerticalForwardAxis)?;
    let (acceleration_motion_rms, gyroscope_motion_rms) =
        inertial_motion_metrics(&accel_window, &gyro_window)
            .ok_or(VehicleFrameCalibrationError::NoSamples)?;
    Ok(VehicleFrameCalibrationResult {
        calibration: CameraCalibration {
            sensor_to_vehicle,
            accelerometer_bias: accelerometer_bias_for_gravity(gravity, sensor_to_vehicle),
            gyroscope_bias,
            low_pass_hz: request.low_pass_hz,
            notes: None,
        },
        interval_start: start,
        interval_end: end,
        used_automatic_interval,
        acceleration_motion_rms,
        gyroscope_motion_rms,
    })
}

fn named_inertial_triplet<'a>(
    dataset: &'a TelemetryDataset,
    prefix: &str,
) -> Option<[&'a TelemetryChannel; 3]> {
    Some([
        dataset.named(&format!("{prefix}x"))?,
        dataset.named(&format!("{prefix}y"))?,
        dataset.named(&format!("{prefix}z"))?,
    ])
}

fn inertial_window(channels: [&TelemetryChannel; 3], start: f64, end: f64) -> Vec<Vec3> {
    let count = channels
        .iter()
        .map(|channel| channel.series.samples.len())
        .min()
        .unwrap_or(0);
    (0..count)
        .filter_map(|index| {
            let time = channels[0].series.samples[index].time;
            (time >= start && time <= end).then(|| {
                [
                    channels[0].series.samples[index].value,
                    channels[1].series.samples[index].value,
                    channels[2].series.samples[index].value,
                ]
            })
        })
        .collect()
}

fn inertial_motion_metrics(accel: &[Vec3], gyro: &[Vec3]) -> Option<(f64, f64)> {
    let gravity = median_gravity(accel.iter().copied())?;
    let gravity_magnitude = norm(gravity);
    let accel_variance = accel
        .iter()
        .map(|sample| {
            sample
                .iter()
                .zip(gravity)
                .map(|(value, center)| (value - center).powi(2))
                .sum::<f64>()
        })
        .sum::<f64>()
        / accel.len() as f64;
    let accel_motion = (accel_variance + (gravity_magnitude - G).powi(2)).sqrt();
    let gyro_motion = (gyro
        .iter()
        .flat_map(|sample| sample.iter())
        .map(|value| value * value)
        .sum::<f64>()
        / gyro.len().max(1) as f64)
        .sqrt();
    Some((accel_motion, gyro_motion))
}

fn quietest_inertial_interval(
    accel: [&TelemetryChannel; 3],
    gyro: [&TelemetryChannel; 3],
    duration: f64,
) -> Option<(f64, f64)> {
    let first = accel
        .iter()
        .chain(gyro.iter())
        .filter_map(|channel| channel.series.samples.first().map(|sample| sample.time))
        .max_by(f64::total_cmp)?;
    let last = accel
        .iter()
        .chain(gyro.iter())
        .filter_map(|channel| channel.series.samples.last().map(|sample| sample.time))
        .min_by(f64::total_cmp)?;
    let duration = duration.clamp(1.0, (last - first).max(1.0));
    let step = (duration * 0.5).max(0.5);
    let mut candidate = first;
    let mut best: Option<(f64, f64)> = None;
    while candidate + duration <= last + f64::EPSILON {
        let accel_window = inertial_window(accel, candidate, candidate + duration);
        let gyro_window = inertial_window(gyro, candidate, candidate + duration);
        if accel_window.len() >= 20
            && gyro_window.len() >= 20
            && let Some((accel_motion, gyro_motion)) =
                inertial_motion_metrics(&accel_window, &gyro_window)
        {
            let score = accel_motion + gyro_motion / 0.15;
            if best.is_none_or(|(_, best_score)| score < best_score) {
                best = Some((candidate, score));
            }
        }
        candidate += step;
    }
    best.map(|(start, _)| (start, start + duration))
}

/// A forward/backward first-order low-pass. Applying it in both directions
/// removes phase delay, which matters for overlays aligned to video.
///
/// `cutoff_hz` describes the final two-pass -3 dB point. Each individual pass
/// therefore uses a cutoff about 1.5538 times higher; without that correction,
/// two identical -3 dB passes would produce -6 dB at the displayed cutoff.
pub fn zero_phase_low_pass(values: &[f64], sample_rate_hz: f64, cutoff_hz: f64) -> Vec<f64> {
    if values.len() < 3 || sample_rate_hz <= 0. || cutoff_hz <= 0. {
        return values.to_vec();
    }
    // For a first-order response, two-pass magnitude is
    // 1 / (1 + (f / f_pass)^2). Solving for -3 dB at `cutoff_hz` gives this
    // per-pass frequency multiplier.
    let pass_cutoff_hz = cutoff_hz * ZERO_PHASE_CUTOFF_COMPENSATION;
    let alpha = 1.0 - (-2.0 * std::f64::consts::PI * pass_cutoff_hz / sample_rate_hz).exp();
    let pass = |input: &[f64]| {
        let mut o = Vec::with_capacity(input.len());
        let mut last = input[0];
        for &x in input {
            last += alpha * (x - last);
            o.push(last)
        }
        o
    };
    let a = pass(values);
    let mut r = a.into_iter().rev().collect::<Vec<_>>();
    r = pass(&r);
    r.into_iter().rev().collect()
}

pub struct DerivedInertialChannels {
    pub longitudinal_g: ChannelSeries,
    pub lateral_g: ChannelSeries,
    pub vertical_g: ChannelSeries,
    pub combined_g: ChannelSeries,
    pub roll_rate: ChannelSeries,
    pub pitch_rate: ChannelSeries,
    pub yaw_rate: ChannelSeries,
}
/// Converts matched XYZ samples to vehicle frame. No speed is inferred from
/// acceleration; integration drifts and is intentionally outside this crate.
pub fn derive_inertial(
    accel: [ChannelSeries; 3],
    gyro: [ChannelSeries; 3],
    calibration: &CameraCalibration,
    _sample_rate_hz: f64,
) -> DerivedInertialChannels {
    let n = accel
        .iter()
        .map(|s| s.samples.len())
        .min()
        .unwrap_or(0)
        .min(gyro.iter().map(|s| s.samples.len()).min().unwrap_or(0));
    let mut a: [Vec<TimedSample>; 3] = std::array::from_fn(|_| Vec::with_capacity(n));
    let mut gr: [Vec<TimedSample>; 3] = std::array::from_fn(|_| Vec::with_capacity(n));
    for i in 0..n {
        let t = accel[0].samples[i].time;
        let av = [
            accel[0].samples[i].value - calibration.accelerometer_bias[0],
            accel[1].samples[i].value - calibration.accelerometer_bias[1],
            accel[2].samples[i].value - calibration.accelerometer_bias[2],
        ];
        let gv = [
            gyro[0].samples[i].value - calibration.gyroscope_bias[0],
            gyro[1].samples[i].value - calibration.gyroscope_bias[1],
            gyro[2].samples[i].value - calibration.gyroscope_bias[2],
        ];
        let av = mul(calibration.sensor_to_vehicle, av);
        let gv = mul(calibration.sensor_to_vehicle, gv);
        for j in 0..3 {
            a[j].push(TimedSample {
                time: t,
                value: av[j] / G - if j == 2 { 1.0 } else { 0.0 },
            });
            gr[j].push(TimedSample {
                time: t,
                value: gv[j].to_degrees(),
            });
        }
    }
    // Use the same timestamp-aware zero-phase filter as source and widget
    // smoothing so cutoff semantics and irregular-sample handling agree.
    let filtered_accel: [ChannelSeries; 3] = std::array::from_fn(|axis| {
        let series = ChannelSeries::new(std::mem::take(&mut a[axis]));
        series
            .low_pass_hz(calibration.low_pass_hz)
            .unwrap_or(series)
    });
    let combined = (0..n)
        .map(|i| TimedSample {
            time: filtered_accel[0].samples[i].time,
            value: (filtered_accel[0].samples[i].value.powi(2)
                + filtered_accel[1].samples[i].value.powi(2))
            .sqrt(),
        })
        .collect();
    let [longitudinal_g, lateral_g, vertical_g] = filtered_accel;
    DerivedInertialChannels {
        longitudinal_g,
        lateral_g,
        vertical_g,
        combined_g: ChannelSeries::new(combined),
        roll_rate: ChannelSeries::new(std::mem::take(&mut gr[0])),
        pitch_rate: ChannelSeries::new(std::mem::take(&mut gr[1])),
        yaw_rate: ChannelSeries::new(std::mem::take(&mut gr[2])),
    }
}

/// Adds standard derived channels when six raw XYZ channels are present.
pub fn add_derived_inertial_channels(
    dataset: &mut TelemetryDataset,
    calibration: &CameraCalibration,
    sample_rate_hz: f64,
) -> bool {
    let get = |n: &str| dataset.named(n).map(|c| c.series.clone());
    let (Some(ax), Some(ay), Some(az), Some(gx), Some(gy), Some(gz)) = (
        get("raw_accel_x"),
        get("raw_accel_y"),
        get("raw_accel_z"),
        get("raw_gyro_x"),
        get("raw_gyro_y"),
        get("raw_gyro_z"),
    ) else {
        return false;
    };
    let d = derive_inertial([ax, ay, az], [gx, gy, gz], calibration, sample_rate_hz);
    for (name, quantity, unit, series) in [
        (
            "longitudinal_g",
            Quantity::Acceleration,
            Unit::StandardGravity,
            d.longitudinal_g,
        ),
        (
            "lateral_g",
            Quantity::Acceleration,
            Unit::StandardGravity,
            d.lateral_g,
        ),
        (
            "vertical_g",
            Quantity::Acceleration,
            Unit::StandardGravity,
            d.vertical_g,
        ),
        (
            "combined_g",
            Quantity::Acceleration,
            Unit::StandardGravity,
            d.combined_g,
        ),
        (
            "roll_rate",
            Quantity::AngularVelocity,
            Unit::DegreePerSecond,
            d.roll_rate,
        ),
        (
            "pitch_rate",
            Quantity::AngularVelocity,
            Unit::DegreePerSecond,
            d.pitch_rate,
        ),
        (
            "yaw_rate",
            Quantity::AngularVelocity,
            Unit::DegreePerSecond,
            d.yaw_rate,
        ),
    ] {
        // Recalibration must update the channel a widget is already bound to,
        // rather than adding another same-named channel with a fresh ID.
        let id = dataset
            .named(name)
            .map(|channel| channel.descriptor.id)
            .unwrap_or_else(|| ChannelId::for_source_name(dataset.source_id, name));
        dataset.insert(TelemetryChannel {
            descriptor: ChannelDescriptor {
                id,
                name: name.into(),
                quantity,
                unit,
                interpolation: Interpolation::Linear,
                description: Some("Camera-calibrated derived inertial channel".into()),
            },
            series,
        });
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn median_rejects_outlier() {
        assert_eq!(
            median_gravity([[0., 0., 9.8], [0., 0., 9.9], [99., 1., -9.]]),
            Some([0., 0., 9.8])
        );
    }
    #[test]
    fn transform_is_orthonormal() {
        let r = guided_sensor_to_vehicle([0., 0., G], 0., [false; 3]).unwrap();
        assert!((dot(r[0], r[1])).abs() < 1e-8);
        assert!((norm(r[2]) - 1.).abs() < 1e-8)
    }
    #[test]
    fn yaw_works_when_gravity_is_along_sensor_y() {
        let r = guided_sensor_to_vehicle([0.0, G, 0.0], std::f64::consts::FRAC_PI_2, [false; 3])
            .unwrap();
        assert!(norm(r[0]) > 0.999);
        assert!(dot(r[0], r[2]).abs() < 1e-12);
        assert!((r[0][2] + 1.0).abs() < 1e-12);
    }
    #[test]
    fn axis_flips_are_composed_into_returned_transform() {
        let r = guided_sensor_to_vehicle([0.0, -G, 0.0], 0.0, [false, true, false]).unwrap();
        let vehicle_gravity = mul(r, [0.0, -G, 0.0]);
        assert!(vehicle_gravity[0].abs() < 1e-12);
        assert!(vehicle_gravity[1].abs() < 1e-12);
        assert!((vehicle_gravity[2] - G).abs() < 1e-12);
    }
    #[test]
    fn explicit_forward_axis_maps_to_vehicle_forward() {
        let r =
            guided_sensor_to_vehicle_from_forward([0.0, -G, 0.0], [0.0, 0.0, 1.0], 0.0, [false; 3])
                .unwrap();
        let vehicle = mul(r, [0.0, 0.0, 2.0]);
        assert!((vehicle[0] - 2.0).abs() < 1e-12);
        assert!(vehicle[1].abs() < 1e-12);
        assert!(vehicle[2].abs() < 1e-12);
    }
    #[test]
    fn roll_and_pitch_trims_have_documented_signs_and_remain_orthonormal() {
        let angle = 20.0_f64.to_radians();
        let r = guided_sensor_to_vehicle_from_forward_with_trim(
            [0.0, 0.0, G],
            [1.0, 0.0, 0.0],
            angle,
            angle,
            10.0_f64.to_radians(),
            [false; 3],
        )
        .unwrap();
        for row in r {
            assert!((norm(row) - 1.0).abs() < 1e-12);
        }
        assert!(dot(r[0], r[1]).abs() < 1e-12);
        assert!(dot(r[0], r[2]).abs() < 1e-12);
        assert!(dot(r[1], r[2]).abs() < 1e-12);

        let pitch_only = guided_sensor_to_vehicle_from_forward_with_trim(
            [0.0, 0.0, G],
            [1.0, 0.0, 0.0],
            0.0,
            angle,
            0.0,
            [false; 3],
        )
        .unwrap();
        assert!(pitch_only[0][2] > 0.0, "positive pitch must raise the nose");
        let roll_only = guided_sensor_to_vehicle_from_forward_with_trim(
            [0.0, 0.0, G],
            [1.0, 0.0, 0.0],
            angle,
            0.0,
            0.0,
            [false; 3],
        )
        .unwrap();
        assert!(
            roll_only[1][2] > 0.0,
            "positive roll must raise the left side"
        );
    }
    #[test]
    fn trimmed_calibration_still_zeroes_stationary_acceleration() {
        let gravity = [0.0, 0.0, 9.7];
        let rotation = guided_sensor_to_vehicle_from_forward_with_trim(
            gravity,
            [1.0, 0.0, 0.0],
            7.0_f64.to_radians(),
            -5.0_f64.to_radians(),
            3.0_f64.to_radians(),
            [false; 3],
        )
        .unwrap();
        let corrected = mul(
            rotation,
            [
                gravity[0] - accelerometer_bias_for_gravity(gravity, rotation)[0],
                gravity[1] - accelerometer_bias_for_gravity(gravity, rotation)[1],
                gravity[2] - accelerometer_bias_for_gravity(gravity, rotation)[2],
            ],
        );
        assert!(corrected[0].abs() < 1e-12);
        assert!(corrected[1].abs() < 1e-12);
        assert!((corrected[2] - G).abs() < 1e-12);
    }
    #[test]
    fn vehicle_frame_fit_finds_quiet_data_and_enables_derived_lateral_g() {
        let raw = |name: &str, values: &dyn Fn(f64) -> f64| TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::new(),
                name: name.into(),
                quantity: Quantity::Generic,
                unit: Unit::Unitless,
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
        };
        let mut dataset = TelemetryDataset::default();
        for channel in [
            raw("raw_accel_x", &|time| {
                if time < 3.0 {
                    (time * 15.0).sin() * 2.0
                } else {
                    0.0
                }
            }),
            raw("raw_accel_y", &|_| 0.0),
            raw("raw_accel_z", &|_| G),
            raw("raw_gyro_x", &|time| if time < 3.0 { 0.5 } else { 0.0 }),
            raw("raw_gyro_y", &|_| 0.0),
            raw("raw_gyro_z", &|_| 0.0),
        ] {
            dataset.insert(channel);
        }

        let result = fit_vehicle_frame_calibration(
            &dataset,
            VehicleFrameCalibrationRequest {
                start_time: 0.0,
                end_time: 2.0,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(result.used_automatic_interval);
        assert!(result.interval_start >= 3.0, "{result:?}");
        assert!(add_derived_inertial_channels(
            &mut dataset,
            &result.calibration,
            1_000.0
        ));
        assert!(dataset.named("lateral_g").is_some());
    }
    #[test]
    fn filter_keeps_constant() {
        let x = zero_phase_low_pass(&[2.; 20], 100., 8.);
        assert!(x.iter().all(|v| (v - 2.).abs() < 1e-9));
    }
    #[test]
    fn zero_phase_cutoff_is_the_final_three_db_point() {
        let sample_rate = 1_000.0;
        let cutoff = 8.0;
        let samples = 20_000;
        let input = (0..samples)
            .map(|index| (std::f64::consts::TAU * cutoff * index as f64 / sample_rate).sin())
            .collect::<Vec<_>>();
        let output = zero_phase_low_pass(&input, sample_rate, cutoff);
        // Ignore both ends, where a finite forward/backward pass has startup
        // transients, and project the middle onto the input sine/cosine.
        let interior = 2_000..18_000;
        let count = interior.len() as f64;
        let (sine, cosine) = interior.fold((0.0, 0.0), |(sine, cosine), index| {
            let phase = std::f64::consts::TAU * cutoff * index as f64 / sample_rate;
            (
                sine + output[index] * phase.sin(),
                cosine + output[index] * phase.cos(),
            )
        });
        let amplitude = 2.0 * sine.hypot(cosine) / count;
        assert!((amplitude - 1.0 / 2.0_f64.sqrt()).abs() < 0.015);
    }
    #[test]
    fn derived_motion_excludes_stationary_gravity_from_vertical_and_combined() {
        let samples = ChannelSeries::new(vec![TimedSample {
            time: 0.0,
            value: 0.0,
        }]);
        let z = ChannelSeries::new(vec![TimedSample {
            time: 0.0,
            value: G,
        }]);
        let d = derive_inertial(
            [samples.clone(), samples.clone(), z],
            [samples.clone(), samples.clone(), samples],
            &CameraCalibration::default(),
            50.0,
        );
        assert!(d.vertical_g.samples[0].value.abs() < 1e-12);
        assert!(d.combined_g.samples[0].value.abs() < 1e-12);
    }

    #[test]
    fn recalibration_replaces_derived_channels_without_breaking_bindings() {
        let raw = |name: &str, value: f64| TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::new(),
                name: name.into(),
                quantity: Quantity::Generic,
                unit: Unit::Unitless,
                interpolation: Interpolation::Linear,
                description: None,
            },
            series: ChannelSeries::new(vec![TimedSample { time: 0.0, value }]),
        };
        let mut dataset = TelemetryDataset::default();
        for (name, value) in [
            ("raw_accel_x", 0.0),
            ("raw_accel_y", 0.0),
            ("raw_accel_z", G),
            ("raw_gyro_x", 0.0),
            ("raw_gyro_y", 0.0),
            ("raw_gyro_z", 0.0),
        ] {
            dataset.insert(raw(name, value));
        }
        assert!(add_derived_inertial_channels(
            &mut dataset,
            &CameraCalibration::default(),
            1_000.0
        ));
        let first_id = dataset.named("combined_g").unwrap().descriptor.id;
        assert!(add_derived_inertial_channels(
            &mut dataset,
            &CameraCalibration::default(),
            1_000.0
        ));
        assert_eq!(dataset.channels.len(), 13);
        assert_eq!(dataset.named("combined_g").unwrap().descriptor.id, first_id);
    }
}
