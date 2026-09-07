//! Boundary interpolation and interval clipping for comparison preparation.

use super::*;

pub fn gate_window(
    gps: &[GpsPoint],
    gates: &[Gate],
    interval_start: f64,
    interval_end: f64,
) -> Option<(f64, f64)> {
    let starts = gate_crossings(gps, *gates.first()?);
    let pairs = if gates.len() == 1 {
        starts
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect::<Vec<_>>()
    } else {
        let finishes = gate_crossings(gps, gates[1]);
        starts
            .into_iter()
            .filter_map(|start| {
                finishes
                    .iter()
                    .copied()
                    .find(|finish| *finish > start + 1.0)
                    .map(|finish| (start, finish))
            })
            .collect()
    };
    pairs
        .into_iter()
        .filter_map(|pair| {
            let overlap = (pair.1.min(interval_end) - pair.0.max(interval_start)).max(0.0);
            (overlap > 0.0).then_some((overlap, pair))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, pair)| pair)
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
    fn gate_window_uses_directed_start_and_finish_crossings() {
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

        let window = gate_window(&points, &[gate(0.0), gate(0.002)], 0.0, 3.0).unwrap();

        assert!((window.0 - 0.5).abs() < 1e-9);
        assert!((window.1 - 2.5).abs() < 1e-9);
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
