//! Optional correction of GPS position drift between runs.
//!
//! Consumer GPS positions drift by metres from one run to the next, while the
//! error within a single run is nearly constant. Each run is therefore shifted
//! by one constant offset that places it on the reference: the shared staging
//! spot when both are standing starts, otherwise the translation that best
//! fits its path onto the reference's path.

use super::*;

/// How a run's drift was estimated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpsDriftMethod {
    /// Both runs sat at the same staging spot before launching.
    Staging,
    /// The run's path was fitted onto the reference's path.
    PathFit,
}

/// The run's GPS position minus the reference's, removed from the prepared run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpsDrift {
    pub east_meters: f64,
    pub north_meters: f64,
    pub method: GpsDriftMethod,
}

impl GpsDrift {
    pub fn meters(&self) -> f64 {
        self.east_meters.hypot(self.north_meters)
    }
}

const METERS_PER_DEGREE: f64 = 111_320.0;
/// Path points farther than this from the reference path are left out of the
/// fit, so detours such as the paddock or a missed gate do not pull it.
const MATCH_RADIUS_METERS: f64 = 10.0;
/// Paths are resampled to points at least this far apart, so time spent
/// stationary does not outweigh the rest of the path.
const PATH_SPACING_METERS: f64 = 1.0;

/// Local east/north metres around a fixed point.
pub(super) struct LocalFrame {
    latitude: f64,
    longitude: f64,
    cos_latitude: f64,
}

impl LocalFrame {
    pub(super) fn new(origin: &GpsPoint) -> Self {
        Self {
            latitude: origin.latitude,
            longitude: origin.longitude,
            cos_latitude: origin.latitude.to_radians().cos(),
        }
    }

    fn xy(&self, point: &GpsPoint) -> [f64; 2] {
        [
            (point.longitude - self.longitude) * METERS_PER_DEGREE * self.cos_latitude,
            (point.latitude - self.latitude) * METERS_PER_DEGREE,
        ]
    }

    /// Removes `drift` from each point.
    pub(super) fn correct(&self, points: &mut [GpsPoint], drift: &GpsDrift) {
        for point in points {
            point.latitude -= drift.north_meters / METERS_PER_DEGREE;
            point.longitude -= drift.east_meters / (METERS_PER_DEGREE * self.cos_latitude);
        }
    }
}

/// One run's GPS inside its interval, with its traveled distance.
pub(super) struct DriftTrack<'a> {
    pub(super) gps: &'a [GpsPoint],
    pub(super) distance: &'a [ProgressSample],
    pub(super) start: f64,
}

/// Whether the run is stationary for its first second: a standing start.
pub(super) fn starts_at_rest(distance: &[ProgressSample], start: f64) -> bool {
    time_at_progress(distance, 0.5).is_some_and(|time| time - start >= 1.0)
}

pub(super) fn estimate_drift(
    frame: &LocalFrame,
    reference: &DriftTrack<'_>,
    run: &DriftTrack<'_>,
) -> Option<GpsDrift> {
    let drift = |[east, north]: [f64; 2], method| GpsDrift {
        east_meters: east,
        north_meters: north,
        method,
    };
    if let (Some(reference), Some(run)) = (
        staging_position(frame, reference),
        staging_position(frame, run),
    ) {
        return Some(drift(
            [run[0] - reference[0], run[1] - reference[1]],
            GpsDriftMethod::Staging,
        ));
    }
    path_fit(frame, reference.gps, run.gps).map(|shift| drift(shift, GpsDriftMethod::PathFit))
}

/// Mean position before the run has moved 0.3 m, when it starts at rest.
fn staging_position(frame: &LocalFrame, track: &DriftTrack<'_>) -> Option<[f64; 2]> {
    if !starts_at_rest(track.distance, track.start) {
        return None;
    }
    let departed = time_at_progress(track.distance, 0.3)?;
    let points = track
        .gps
        .iter()
        .filter(|point| point.recording_time < departed)
        .map(|point| frame.xy(point))
        .collect::<Vec<_>>();
    if points.len() < 5 {
        return None;
    }
    let count = points.len() as f64;
    Some([
        points.iter().map(|point| point[0]).sum::<f64>() / count,
        points.iter().map(|point| point[1]).sum::<f64>() / count,
    ])
}

fn resample(frame: &LocalFrame, gps: &[GpsPoint]) -> Vec<[f64; 2]> {
    let mut points: Vec<[f64; 2]> = Vec::new();
    for point in gps.iter().map(|point| frame.xy(point)) {
        if points.last().is_none_or(|last| {
            (point[0] - last[0]).hypot(point[1] - last[1]) >= PATH_SPACING_METERS
        }) {
            points.push(point);
        }
    }
    points
}

/// The reference path as segments, bucketed so a nearest-segment query only
/// looks at the surrounding cells.
struct Polyline {
    points: Vec<[f64; 2]>,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl Polyline {
    fn new(points: Vec<[f64; 2]>) -> Self {
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (index, pair) in points.windows(2).enumerate() {
            // A long jump is a GPS gap, not a path to fit onto.
            if (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]) > 20.0 {
                continue;
            }
            let [xs, ys] = [0, 1].map(|axis| {
                let (a, b) = (pair[0][axis], pair[1][axis]);
                cell(a.min(b))..=cell(a.max(b))
            });
            for x in xs {
                for y in ys.clone() {
                    cells.entry((x, y)).or_default().push(index);
                }
            }
        }
        Self { points, cells }
    }

    /// The closest point on the path within [`MATCH_RADIUS_METERS`], and the
    /// unit direction of the segment it lies on.
    fn nearest(&self, query: [f64; 2]) -> Option<([f64; 2], [f64; 2])> {
        let (cx, cy) = (cell(query[0]), cell(query[1]));
        let mut best: Option<(f64, [f64; 2], [f64; 2])> = None;
        for x in cx - 1..=cx + 1 {
            for y in cy - 1..=cy + 1 {
                for &index in self.cells.get(&(x, y)).into_iter().flatten() {
                    let (a, b) = (self.points[index], self.points[index + 1]);
                    let along = [b[0] - a[0], b[1] - a[1]];
                    let length = along[0].hypot(along[1]);
                    if length <= f64::EPSILON {
                        continue;
                    }
                    let t = (((query[0] - a[0]) * along[0] + (query[1] - a[1]) * along[1])
                        / (length * length))
                        .clamp(0.0, 1.0);
                    let closest = [a[0] + t * along[0], a[1] + t * along[1]];
                    let distance = (query[0] - closest[0]).hypot(query[1] - closest[1]);
                    if distance <= MATCH_RADIUS_METERS
                        && best.is_none_or(|(nearest, _, _)| distance < nearest)
                    {
                        best = Some((distance, closest, [along[0] / length, along[1] / length]));
                    }
                }
            }
        }
        best.map(|(_, closest, direction)| (closest, direction))
    }
}

fn cell(coordinate: f64) -> i64 {
    (coordinate / MATCH_RADIUS_METERS).floor() as i64
}

/// The translation that best places `run` on `reference`, by iterating
/// nearest-segment matches. `None` when too little of the path matches, or
/// when the path runs mostly one way so movement along it is undetermined.
fn path_fit(frame: &LocalFrame, reference: &[GpsPoint], run: &[GpsPoint]) -> Option<[f64; 2]> {
    let polyline = Polyline::new(resample(frame, reference));
    let points = resample(frame, run);
    if points.len() < 20 {
        return None;
    }
    let mut shift = [0.0, 0.0];
    for _ in 0..50 {
        let mut sum = [0.0, 0.0];
        // Second moment of the matched segment normals: how well each
        // direction is pinned down.
        let mut moment = [0.0, 0.0, 0.0];
        let mut matched = 0;
        for point in &points {
            let moved = [point[0] - shift[0], point[1] - shift[1]];
            let Some((closest, direction)) = polyline.nearest(moved) else {
                continue;
            };
            sum[0] += moved[0] - closest[0];
            sum[1] += moved[1] - closest[1];
            let normal = [-direction[1], direction[0]];
            moment[0] += normal[0] * normal[0];
            moment[1] += normal[0] * normal[1];
            moment[2] += normal[1] * normal[1];
            matched += 1;
        }
        if matched * 2 < points.len() {
            return None;
        }
        let count = matched as f64;
        let step = [sum[0] / count, sum[1] / count];
        shift = [shift[0] + step[0], shift[1] + step[1]];
        if step[0].hypot(step[1]) < 0.005 {
            let [xx, xy, yy] = moment.map(|value| value / count);
            let weakest = (xx + yy) / 2.0 - (((xx - yy) / 2.0).powi(2) + xy * xy).sqrt();
            return (weakest >= 0.1).then_some(shift);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: GpsPoint = GpsPoint {
        recording_time: 0.0,
        latitude: 42.0,
        longitude: -76.0,
        accuracy_meters: None,
    };

    /// GPS at 10 Hz along `path` (east/north metres, one per sample), shifted
    /// by `drift`, with traveled distance measured along the unshifted path.
    fn track(path: &[[f64; 2]], drift: [f64; 2]) -> (Vec<GpsPoint>, Vec<ProgressSample>) {
        let cos = ORIGIN.latitude.to_radians().cos();
        let mut traveled = 0.0;
        path.iter()
            .enumerate()
            .map(|(index, point)| {
                if index > 0 {
                    let last = path[index - 1];
                    traveled += (point[0] - last[0]).hypot(point[1] - last[1]);
                }
                let time = index as f64 * 0.1;
                (
                    GpsPoint {
                        recording_time: time,
                        latitude: ORIGIN.latitude + (point[1] + drift[1]) / METERS_PER_DEGREE,
                        longitude: ORIGIN.longitude
                            + (point[0] + drift[0]) / (METERS_PER_DEGREE * cos),
                        accuracy_meters: None,
                    },
                    ProgressSample {
                        recording_time: time,
                        progress: traveled,
                        confidence: 1.0,
                    },
                )
            })
            .unzip()
    }

    /// Two seconds staged at the origin, then north 60 m and east 60 m.
    fn standing_start() -> Vec<[f64; 2]> {
        let staged = (0..20).map(|_| [0.0, 0.0]);
        let north = (1..=60).map(|step| [0.0, step as f64]);
        let east = (1..=60).map(|step| [step as f64, 60.0]);
        staged.chain(north).chain(east).collect()
    }

    /// A lap of a 40 m radius circle.
    fn flying_lap() -> Vec<[f64; 2]> {
        (0..250)
            .map(|step| {
                let angle = step as f64 / 250.0 * std::f64::consts::TAU;
                [40.0 * angle.cos(), 40.0 * angle.sin()]
            })
            .collect()
    }

    fn estimate(reference: &[[f64; 2]], run: &[[f64; 2]], drift: [f64; 2]) -> Option<GpsDrift> {
        let (reference_gps, reference_distance) = track(reference, [0.0, 0.0]);
        let (run_gps, run_distance) = track(run, drift);
        estimate_drift(
            &LocalFrame::new(&ORIGIN),
            &DriftTrack {
                gps: &reference_gps,
                distance: &reference_distance,
                start: 0.0,
            },
            &DriftTrack {
                gps: &run_gps,
                distance: &run_distance,
                start: 0.0,
            },
        )
    }

    #[test]
    fn standing_starts_are_corrected_by_their_shared_staging_spot() {
        let drift = estimate(&standing_start(), &standing_start(), [2.5, -1.5]).unwrap();

        assert_eq!(drift.method, GpsDriftMethod::Staging);
        assert!((drift.east_meters - 2.5).abs() < 1e-6, "{drift:?}");
        assert!((drift.north_meters + 1.5).abs() < 1e-6, "{drift:?}");
    }

    #[test]
    fn flying_laps_are_corrected_by_fitting_the_path() {
        // A slightly different line: 1 m wider, which a fit should not
        // mistake for drift because it is not a translation.
        let wider = flying_lap()
            .into_iter()
            .map(|[x, y]| [x * 41.0 / 40.0, y * 41.0 / 40.0])
            .collect::<Vec<_>>();

        let drift = estimate(&flying_lap(), &wider, [3.0, 2.0]).unwrap();

        assert_eq!(drift.method, GpsDriftMethod::PathFit);
        assert!((drift.east_meters - 3.0).abs() < 0.2, "{drift:?}");
        assert!((drift.north_meters - 2.0).abs() < 0.2, "{drift:?}");
    }

    #[test]
    fn a_straight_path_does_not_pin_down_movement_along_it() {
        let straight = (0..200).map(|step| [0.0, step as f64]).collect::<Vec<_>>();

        assert_eq!(estimate(&straight, &straight, [1.0, 0.0]), None);
    }

    #[test]
    fn correcting_removes_the_estimated_drift() {
        let frame = LocalFrame::new(&ORIGIN);
        let (mut gps, _) = track(&[[5.0, 7.0]], [2.0, -3.0]);

        frame.correct(
            &mut gps,
            &GpsDrift {
                east_meters: 2.0,
                north_meters: -3.0,
                method: GpsDriftMethod::Staging,
            },
        );

        let [east, north] = frame.xy(&gps[0]);
        assert!((east - 5.0).abs() < 1e-6 && (north - 7.0).abs() < 1e-6);
    }
}
