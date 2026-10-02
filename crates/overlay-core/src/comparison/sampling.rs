//! Boundary interpolation and interval clipping for comparison preparation.

use super::*;

/// Laps between consecutive directed crossings of one circuit gate, as
/// (start, finish) recording times.
pub fn gate_laps(gps: &[GpsPoint], gate: Gate) -> Vec<(f64, f64)> {
    gate_crossings(gps, gate)
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .collect()
}

/// When a run first crosses the start gate inside `[start, end]`, and when it
/// next crosses the finish gate (the circuit gate again, with one gate).
pub(super) fn run_gate_times(
    gps: &[GpsPoint],
    gates: &[Gate],
    start: f64,
    end: f64,
) -> (Option<f64>, Option<f64>) {
    let inside = |time: f64| time >= start - 1e-6 && time <= end + 1e-6;
    let Some(start_gate) = gates.first() else {
        return (None, None);
    };
    let Some(started) = gate_crossings(gps, *start_gate)
        .into_iter()
        .find(|time| inside(*time))
    else {
        return (None, None);
    };
    let finished = gate_crossings(gps, *gates.get(1).unwrap_or(start_gate))
        .into_iter()
        .find(|time| *time > started + 1.0 && inside(*time));
    (Some(started), finished)
}
pub fn segment<'a>(
    workspace: &'a AnalysisWorkspace,
    key: &SegmentRef,
) -> Option<(&'a Recording, &'a RunSegment)> {
    let r = workspace
        .recordings
        .iter()
        .find(|r| r.id == key.recording_id)?;
    Some((r, r.segments.iter().find(|s| s.id == key.segment_id)?))
}
fn gps_point_at_time(points: &[GpsPoint], time: f64) -> Option<GpsPoint> {
    if !time.is_finite() || points.is_empty() {
        return None;
    }
    let index = points.partition_point(|point| point.recording_time < time);
    if let Some(point) = points.get(index)
        && (point.recording_time - time).abs() < 1e-9
    {
        return Some(*point);
    }
    let (left, right) = (points.get(index.checked_sub(1)?)?, points.get(index)?);
    let duration = right.recording_time - left.recording_time;
    if !(0.0..=2.0).contains(&duration) || duration <= f64::EPSILON {
        return None;
    }
    let fraction = (time - left.recording_time) / duration;
    let longitude_delta = (right.longitude - left.longitude + 540.0).rem_euclid(360.0) - 180.0;
    Some(GpsPoint {
        recording_time: time,
        latitude: left.latitude + (right.latitude - left.latitude) * fraction,
        longitude: (left.longitude + longitude_delta * fraction + 540.0).rem_euclid(360.0) - 180.0,
        accuracy_meters: left
            .accuracy_meters
            .zip(right.accuracy_meters)
            .map(|(left, right)| left.max(right)),
    })
}

pub fn gps_inside_interval(points: &[GpsPoint], start: f64, end: f64) -> Vec<GpsPoint> {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return vec![];
    }
    let mut clipped = Vec::new();
    if let Some(point) = gps_point_at_time(points, start) {
        clipped.push(point);
    }
    clipped.extend(
        points
            .iter()
            .copied()
            .filter(|point| point.recording_time > start && point.recording_time < end),
    );
    if let Some(point) = gps_point_at_time(points, end)
        && clipped
            .last()
            .is_none_or(|last| (last.recording_time - end).abs() >= 1e-9)
    {
        clipped.push(point);
    }
    clipped
}

fn timed_sample_at_time(
    samples: &[TimedSample],
    gaps: &[(f64, f64)],
    time: f64,
) -> Option<TimedSample> {
    if !time.is_finite() || samples.is_empty() {
        return None;
    }
    let index = samples.partition_point(|sample| sample.time < time);
    if let Some(sample) = samples.get(index)
        && (sample.time - time).abs() < 1e-9
    {
        return Some(*sample);
    }
    if gaps.iter().any(|(start, end)| time > *start && time < *end) {
        return None;
    }
    let (left, right) = (samples.get(index.checked_sub(1)?)?, samples.get(index)?);
    let duration = right.time - left.time;
    if duration <= f64::EPSILON {
        return None;
    }
    let fraction = (time - left.time) / duration;
    Some(TimedSample {
        time,
        value: left.value + (right.value - left.value) * fraction,
    })
}

pub(super) fn timed_samples_inside_interval(
    samples: &[TimedSample],
    gaps: &[(f64, f64)],
    start: f64,
    end: f64,
) -> Vec<TimedSample> {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return vec![];
    }
    let mut clipped = Vec::new();
    if let Some(sample) = timed_sample_at_time(samples, gaps, start) {
        clipped.push(sample);
    }
    clipped.extend(
        samples
            .iter()
            .copied()
            .filter(|sample| sample.time > start && sample.time < end),
    );
    if let Some(sample) = timed_sample_at_time(samples, gaps, end)
        && clipped
            .last()
            .is_none_or(|last| (last.time - end).abs() >= 1e-9)
    {
        clipped.push(sample);
    }
    clipped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_gate_times_use_directed_start_and_finish_crossings_inside_the_run() {
        let points = [-0.001, 0.001, 0.0015, 0.0025]
            .into_iter()
            .enumerate()
            .map(|(index, latitude)| GpsPoint {
                recording_time: index as f64,
                latitude,
                longitude: 0.0,
                accuracy_meters: None,
            })
            .collect::<Vec<_>>();
        let gate = |latitude| Gate {
            latitude,
            longitude: 0.0,
            heading_degrees: 0.0,
            width_meters: 20.0,
        };
        let gates = [gate(0.0), gate(0.002)];

        let (start, finish) = run_gate_times(&points, &gates, 0.0, 3.0);
        assert!((start.unwrap() - 0.5).abs() < 1e-9);
        assert!((finish.unwrap() - 2.5).abs() < 1e-9);

        assert_eq!(run_gate_times(&points, &gates, 1.0, 3.0), (None, None));
        let (start, finish) = run_gate_times(&points, &gates, 0.0, 2.0);
        assert!(start.is_some());
        assert_eq!(finish, None);
    }

    #[test]
    fn boundary_interpolation_does_not_bridge_a_gps_outage() {
        let points = [0.0, 1.0, 4.0]
            .into_iter()
            .map(|recording_time| GpsPoint {
                recording_time,
                latitude: recording_time,
                longitude: recording_time,
                accuracy_meters: None,
            })
            .collect::<Vec<_>>();

        let clipped = gps_inside_interval(&points, 0.5, 2.0);

        assert_eq!(clipped.first().unwrap().recording_time, 0.5);
        assert_eq!(clipped.last().unwrap().recording_time, 1.0);
        assert!(
            clipped
                .iter()
                .all(|point| (point.recording_time - 2.0).abs() > 1e-9)
        );
    }
}
