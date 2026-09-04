//! Guided inertial calibration and derived vehicle-frame channels.
use crate::*;

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
