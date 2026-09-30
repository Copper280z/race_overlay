//! Relative gyro stabilization, independent of gravity, vehicle axes, and telemetry filters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quaternion(pub [f64; 4]);
impl Quaternion {
    pub const IDENTITY: Self = Self([1., 0., 0., 0.]);
    pub fn conjugate(self) -> Self {
        let [w, x, y, z] = self.0;
        Self([w, -x, -y, -z])
    }
    pub fn normalized(self) -> Self {
        let n = self.0.iter().map(|v| v * v).sum::<f64>().sqrt();
        if n < 1e-12 {
            Self::IDENTITY
        } else {
            Self(self.0.map(|v| v / n))
        }
    }
    pub fn multiply(self, b: Self) -> Self {
        let [w, x, y, z] = self.0;
        let [a, b, c, d] = b.0;
        Self([
            w * a - x * b - y * c - z * d,
            w * b + x * a + y * d - z * c,
            w * c - x * d + y * a + z * b,
            w * d + x * c - y * b + z * a,
        ])
    }
    pub fn rotate(self, v: [f64; 3]) -> [f64; 3] {
        let q = self
            .multiply(Self([0., v[0], v[1], v[2]]))
            .multiply(self.conjugate());
        [q.0[1], q.0[2], q.0[3]]
    }
    pub fn axis_angle(axis: [f64; 3], angle: f64) -> Self {
        Self::from_rotation_vector(axis.map(|v| v * angle))
    }
    pub fn from_rotation_vector(v: [f64; 3]) -> Self {
        let a = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        if a < 1e-12 {
            return Self::IDENTITY;
        }
        let s = (a * 0.5).sin() / a;
        Self([(a * 0.5).cos(), v[0] * s, v[1] * s, v[2] * s])
    }
    pub fn rotation_vector(self) -> [f64; 3] {
        let mut q = self.normalized().0;
        if q[0] < 0. {
            q = q.map(|v| -v);
        }
        let n = (q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if n < 1e-12 {
            return [0.; 3];
        }
        let a = 2. * n.atan2(q[0]) / n;
        [q[1] * a, q[2] * a, q[3] * a]
    }
    pub fn slerp(self, other: Self, t: f64) -> Self {
        let r = self.conjugate().multiply(other).rotation_vector();
        self.multiply(Self::from_rotation_vector(r.map(|v| v * t)))
            .normalized()
    }
    pub fn matrix(self) -> [[f64; 3]; 3] {
        let x = self.rotate([1., 0., 0.]);
        let y = self.rotate([0., 1., 0.]);
        let z = self.rotate([0., 0., 1.]);
        [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
    }
}
#[derive(Clone, Copy, Debug)]
pub struct GyroSample {
    pub time: f64,
    pub radians_per_second: [f64; 3],
}
#[derive(Clone, Debug, Default)]
pub struct CameraMotion {
    pub samples: Vec<GyroSample>,
}
#[derive(Clone, Debug)]
struct Pose {
    time: f64,
    rig_to_world: Quaternion,
    correction: Quaternion,
    segment: usize,
}
#[derive(Clone, Debug)]
pub struct MotionTrajectory {
    poses: Vec<Pose>,
    raw_poses: Vec<Pose>,
}
impl CameraMotion {
    pub fn prepare(&self, sigma: f64) -> Result<MotionTrajectory, String> {
        if !sigma.is_finite() || !(0.05..=1.).contains(&sigma) {
            return Err("Invalid stabilization smoothing duration".into());
        }
        if self.samples.len() < 2 {
            return Err("Stabilization requires camera gyro samples".into());
        }
        let mut poses = Vec::new();
        let mut raw_poses = Vec::with_capacity(self.samples.len());
        let mut orientation = Quaternion::IDENTITY;
        let mut segment = 0;
        for (i, s) in self.samples.iter().enumerate() {
            if !s.time.is_finite() || !s.radians_per_second.iter().all(|v| v.is_finite()) {
                return Err("Invalid camera gyro sample".into());
            }
            if i > 0 {
                let previous = self.samples[i - 1];
                let dt = s.time - previous.time;
                if dt <= 0. {
                    return Err("Camera gyro timestamps are not strictly increasing".into());
                }
                if dt > 0.05 {
                    segment += 1;
                    orientation = Quaternion::IDENTITY;
                } else {
                    let velocity = std::array::from_fn(|k| {
                        (s.radians_per_second[k] + previous.radians_per_second[k]) * dt * 0.5
                    });
                    orientation = orientation
                        .multiply(Quaternion::from_rotation_vector(velocity))
                        .normalized();
                }
            }
            raw_poses.push(Pose {
                time: s.time,
                rig_to_world: orientation,
                correction: Quaternion::IDENTITY,
                segment,
            });
            // Integrate every IMU sample; retain a 100 Hz trajectory for preparation and sampling.
            if poses
                .last()
                .is_none_or(|p: &Pose| p.segment != segment || s.time - p.time >= 0.0099)
                || i + 1 == self.samples.len()
                || self
                    .samples
                    .get(i + 1)
                    .is_some_and(|n| n.time - s.time > 0.05)
            {
                poses.push(Pose {
                    time: s.time,
                    rig_to_world: orientation,
                    correction: Quaternion::IDENTITY,
                    segment,
                });
            }
        }
        let mut corrections = Vec::with_capacity(poses.len());
        for pose in &poses {
            let start = poses.partition_point(|p| p.time < pose.time - 3. * sigma);
            let end = poses.partition_point(|p| p.time <= pose.time + 3. * sigma);
            let mut sum = [0.; 3];
            let mut weight = 0.;
            for (index, p) in poses[start..end].iter().enumerate() {
                if p.segment != pose.segment {
                    continue;
                }
                let index = start + index;
                let before = poses
                    .get(index.wrapping_sub(1))
                    .filter(|v| v.segment == p.segment)
                    .map_or(0.01, |v| p.time - v.time);
                let after = poses
                    .get(index + 1)
                    .filter(|v| v.segment == p.segment)
                    .map_or(0.01, |v| v.time - p.time);
                let w =
                    (-0.5 * ((p.time - pose.time) / sigma).powi(2)).exp() * (before + after) * 0.5;
                let delta = pose
                    .rig_to_world
                    .conjugate()
                    .multiply(p.rig_to_world)
                    .rotation_vector();
                for k in 0..3 {
                    sum[k] += delta[k] * w;
                }
                weight += w;
            }
            corrections.push(Quaternion::from_rotation_vector(sum.map(|v| v / weight)));
        }
        for (pose, correction) in poses.iter_mut().zip(corrections) {
            pose.correction = correction;
        }
        Ok(MotionTrajectory { poses, raw_poses })
    }
}
impl MotionTrajectory {
    /// Rotation from the smoothed virtual rig into the instantaneous physical rig.
    pub fn correction(&self, time: f64) -> Option<Quaternion> {
        if !time.is_finite() {
            return None;
        }
        let sample = |poses: &[Pose], smoothed: bool| -> Option<Quaternion> {
            let at = |p: &Pose| {
                if smoothed {
                    p.rig_to_world.multiply(p.correction)
                } else {
                    p.rig_to_world
                }
            };
            let index = poses.partition_point(|p| p.time < time);
            if let Some(p) = poses.get(index)
                && (p.time - time).abs() < 1e-9
            {
                return Some(at(p));
            }
            let a = poses.get(index.checked_sub(1)?)?;
            let b = poses.get(index)?;
            if a.segment != b.segment {
                return None;
            }
            Some(at(a).slerp(at(b), (time - a.time) / (b.time - a.time)))
        };
        Some(
            sample(&self.raw_poses, false)?
                .conjugate()
                .multiply(sample(&self.poses, true)?),
        )
    }

    pub fn covers(&self, start: f64, end: f64) -> bool {
        self.correction(start).is_some()
            && self.correction(end).is_some()
            && self
                .poses
                .windows(2)
                .all(|p| p[1].time < start || p[0].time > end || p[0].segment == p[1].segment)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn motion(f: impl Fn(f64) -> [f64; 3]) -> CameraMotion {
        CameraMotion {
            samples: (0..3001)
                .map(|i| {
                    let t = i as f64 / 1000.;
                    GyroSample {
                        time: t,
                        radians_per_second: f(t),
                    }
                })
                .collect(),
        }
    }
    #[test]
    fn rotations_and_wraparound() {
        let q = Quaternion::axis_angle([0., 1., 0.], std::f64::consts::FRAC_PI_2);
        let v = q.rotate([0., 0., 1.]);
        assert!((v[0] - 1.).abs() < 1e-12);
        let a = Quaternion::axis_angle([0., 1., 0.], 179_f64.to_radians());
        let b = Quaternion::axis_angle([0., 1., 0.], 181_f64.to_radians());
        assert!(a.slerp(b, 0.5).rotate([0., 0., 1.])[2] < -0.999);
    }
    #[test]
    fn follows_constant_turn_and_suppresses_vibration() {
        let constant = motion(|_| [0., 1., 0.]).prepare(0.25).unwrap();
        assert!(constant.correction(1.5).unwrap().rotation_vector()[1].abs() < 1e-4);
        let vibration = motion(|t| [0., (t * std::f64::consts::TAU * 10.).cos(), 0.])
            .prepare(0.25)
            .unwrap();
        let correction = vibration.correction(1.025).unwrap().rotation_vector()[1];
        assert!(correction < -0.01);
    }
    #[test]
    fn irregular_samples_follow_banking_and_invalid_clocks_are_rejected() {
        let mut motion = CameraMotion {
            samples: (0..600)
                .map(|i| GyroSample {
                    time: i as f64 * 0.01 + if i % 2 == 0 { 0. } else { 0.002 },
                    radians_per_second: [0., 0., 0.5],
                })
                .collect(),
        };
        let original = motion
            .samples
            .iter()
            .map(|s| s.radians_per_second)
            .collect::<Vec<_>>();
        let prepared = motion.prepare(0.25).unwrap();
        assert!(
            prepared
                .correction(3.)
                .unwrap()
                .rotation_vector()
                .iter()
                .all(|v| v.abs() < 1e-3)
        );
        assert_eq!(
            original,
            motion
                .samples
                .iter()
                .map(|s| s.radians_per_second)
                .collect::<Vec<_>>()
        );
        motion.samples[10].time = motion.samples[9].time;
        assert!(motion.prepare(0.25).is_err());
    }
    #[test]
    fn gaps_are_not_filled_and_seeks_are_deterministic() {
        let mut m = motion(|_| [0.; 3]);
        m.samples.retain(|s| s.time < 1. || s.time > 2.);
        let p = m.prepare(0.25).unwrap();
        assert!(p.correction(1.5).is_none());
        assert!(p.correction(-1.).is_none());
        assert!(!p.covers(0., 3.));
        assert_eq!(p.correction(2.5), p.correction(2.5));
    }
}
