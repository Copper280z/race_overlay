//! Best-effort distance and motion alignment strategies.

use super::*;

fn recording_channel<'a>(
    recording: &Recording,
    data: &'a HashMap<SourceId, AnalysisSourceData>,
    names: &[&str],
) -> Option<(&'a TelemetryChannel, f64)> {
    recording
        .sources
        .iter()
        .filter(|source| source.id == recording.primary_source)
        .chain(
            recording
                .sources
                .iter()
                .filter(|source| source.id != recording.primary_source),
        )
        .find_map(|source| {
            let channel = names
                .iter()
                .find_map(|name| data.get(&source.id)?.processed.named(name))?;
            Some((channel, source.alignment.offset_seconds))
        })
}

fn normalized_run_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, AnalysisSourceData>,
    names: &[&str],
    cutoff_hz: f64,
) -> Option<ChannelSeries> {
    let (recording, segment) = segment(workspace, key)?;
    let (channel, source_offset) = recording_channel(recording, data, names)?;
    if channel.descriptor.interpolation != Interpolation::Linear {
        return None;
    }
    let mut series = ChannelSeries::new(
        channel
            .series
            .samples
            .iter()
            .filter_map(|sample| {
                let recording_time = sample.time - source_offset;
                (recording_time >= segment.start_recording_time
                    && recording_time <= segment.end_recording_time)
                    .then_some(TimedSample {
                        time: recording_time - segment.start_recording_time,
                        value: sample.value,
                    })
            })
            .collect(),
    );
    series.gap_seconds = channel.series.gap_seconds;
    if series.samples.len() < 24 {
        return None;
    }
    Some(series.low_pass_hz(cutoff_hz).unwrap_or(series))
}

fn distance_domain_series(
    workspace: &AnalysisWorkspace,
    run: &PreparedComparisonRun,
    data: &HashMap<SourceId, AnalysisSourceData>,
    names: &[&str],
    cutoff_hz: f64,
) -> Option<ChannelSeries> {
    let time_series = normalized_run_series(workspace, &run.key, data, names, cutoff_hz)?;
    let maximum = run.distance.last()?.progress;
    if !maximum.is_finite() || maximum < 20.0 {
        return None;
    }
    let spacing = (maximum / 2_000.0).max(0.5);
    let count = (maximum / spacing).floor() as usize;
    let samples = (0..=count)
        .filter_map(|index| {
            let distance = index as f64 * spacing;
            let recording_time = time_at_progress(&run.distance, distance)?;
            let value =
                time_series.sample_at_default(recording_time - run.start, Interpolation::Linear)?;
            Some(TimedSample {
                time: distance,
                value,
            })
        })
        .collect::<Vec<_>>();
    (samples.len() >= 40).then(|| ChannelSeries::new(samples).with_gap(spacing * 3.0))
}

fn correlate_distance_series(
    reference: &ChannelSeries,
    target: &ChannelSeries,
    use_absolute_correlation: bool,
) -> Option<CorrelationResult> {
    let coverage = reference
        .samples
        .last()?
        .time
        .min(target.samples.last()?.time);
    if coverage < 20.0 {
        return None;
    }
    let reference_window = (coverage * 0.45).clamp(60.0, 250.0).min(coverage);
    let search = (coverage * 0.08).clamp(8.0, 25.0);
    let reference = ChannelSeries::new(
        reference
            .samples
            .iter()
            .take_while(|sample| sample.time <= reference_window)
            .copied()
            .collect(),
    );
    let minimum_overlap = (reference_window * 0.55)
        .clamp(15.0, 80.0)
        .min(reference_window * 0.8);
    correlate_channel_series(
        &reference,
        target,
        &CorrelationConfig {
            min_lag_seconds: -search,
            max_lag_seconds: search,
            lag_resolution_seconds: Some(0.5),
            min_overlap_seconds: minimum_overlap,
            min_samples: 30,
            use_absolute_correlation,
            ..Default::default()
        },
    )
    .ok()
    .filter(|result| result.target_minus_reference_seconds.abs() < search * 0.8)
}

/// Matched runs are timed where each has travelled this far past the matched
/// distance. Traveled distance creeps while a car is staged (GPS and speed
/// noise), so "still at zero" would land anywhere in the staging; passing a
/// metre marks the departure itself.
const DEPARTURE_METERS: f64 = 1.0;

fn spatial_time_alignment(
    workspace: &AnalysisWorkspace,
    reference: &PreparedComparisonRun,
    target: &PreparedComparisonRun,
    data: &HashMap<SourceId, AnalysisSourceData>,
) -> Option<ComparisonTimeAlignment> {
    let candidates = [
        (
            "lateral acceleration by distance",
            &[
                "lateral_g",
                "gps_lateral_acceleration",
                "lateral_acceleration",
            ][..],
            3.0,
            true,
            0.65,
        ),
        (
            "yaw rate by distance",
            &["yaw_rate", "gps_yaw_rate", "raw_gyro_z", "gyro_z"][..],
            3.0,
            true,
            0.65,
        ),
        (
            "speed by distance",
            &["gps_speed", "speed"][..],
            2.0,
            false,
            0.85,
        ),
    ];
    candidates
        .into_iter()
        .find_map(|(label, names, cutoff, absolute, minimum_score)| {
            let reference_series =
                distance_domain_series(workspace, reference, data, names, cutoff)?;
            let target_series = distance_domain_series(workspace, target, data, names, cutoff)?;
            let result = correlate_distance_series(&reference_series, &target_series, absolute)?;
            let score = if absolute {
                result.correlation_coefficient.abs()
            } else {
                result.correlation_coefficient
            };
            (score >= minimum_score).then_some((label, result))
        })
        .and_then(|(channel, result)| {
            let distance_offset = result.target_minus_reference_seconds;
            let reference_distance = (-distance_offset).max(0.0);
            let target_distance = reference_distance + distance_offset;
            let reference_time =
                time_at_progress(&reference.distance, reference_distance + DEPARTURE_METERS)?;
            let target_time =
                time_at_progress(&target.distance, target_distance + DEPARTURE_METERS)?;
            let reference_elapsed = reference_time - reference.start;
            let target_elapsed = target_time - target.start;
            let maximum_anchor_elapsed = 15.0_f64
                .min((reference.end - reference.start) * 0.2)
                .min((target.end - target.start) * 0.2);
            if reference_elapsed > maximum_anchor_elapsed || target_elapsed > maximum_anchor_elapsed
            {
                return None;
            }
            Some(ComparisonTimeAlignment {
                offset_seconds: reference_elapsed - target_elapsed,
                coefficient: Some(result.correlation_coefficient),
                channel,
                distance_offset_meters: Some(distance_offset),
            })
        })
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

fn axis_motion_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, AnalysisSourceData>,
    names: [&str; 3],
) -> Option<ChannelSeries> {
    let (recording, segment) = segment(workspace, key)?;
    for source in &recording.sources {
        let dataset = &data.get(&source.id)?.processed;
        let axes = names.map(|name| dataset.named(name));
        let [Some(x), Some(y), Some(z)] = axes else {
            continue;
        };
        let baseline_end = segment.start_recording_time + 3.0;
        let baseline = [&x, &y, &z].map(|axis| {
            let mut values = axis
                .series
                .samples
                .iter()
                .filter(|sample| {
                    let time = sample.time - source.alignment.offset_seconds;
                    time >= segment.start_recording_time && time <= baseline_end
                })
                .map(|sample| sample.value)
                .collect::<Vec<_>>();
            median(&mut values)
        });
        let [Some(bx), Some(by), Some(bz)] = baseline else {
            continue;
        };
        let samples = x
            .series
            .samples
            .iter()
            .filter_map(|sample| {
                let recording_time = sample.time - source.alignment.offset_seconds;
                if recording_time < segment.start_recording_time
                    || recording_time > segment.end_recording_time
                {
                    return None;
                }
                let y = y
                    .series
                    .sample_at_default(sample.time, Interpolation::Linear)?;
                let z = z
                    .series
                    .sample_at_default(sample.time, Interpolation::Linear)?;
                Some(TimedSample {
                    time: recording_time - segment.start_recording_time,
                    value: ((sample.value - bx).powi(2) + (y - by).powi(2) + (z - bz).powi(2))
                        .sqrt(),
                })
            })
            .collect::<Vec<_>>();
        if samples.len() >= 24 {
            let series = ChannelSeries::new(samples);
            return Some(series.low_pass_hz(3.0).unwrap_or(series));
        }
    }
    None
}

fn motion_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, AnalysisSourceData>,
) -> Option<ChannelSeries> {
    axis_motion_series(
        workspace,
        key,
        data,
        ["raw_accel_x", "raw_accel_y", "raw_accel_z"],
    )
    .or_else(|| {
        axis_motion_series(
            workspace,
            key,
            data,
            ["accelerometer_x", "accelerometer_y", "accelerometer_z"],
        )
    })
    .or_else(|| {
        axis_motion_series(
            workspace,
            key,
            data,
            ["raw_gyro_x", "raw_gyro_y", "raw_gyro_z"],
        )
    })
    .or_else(|| axis_motion_series(workspace, key, data, ["gyro_x", "gyro_y", "gyro_z"]))
    .or_else(|| normalized_run_series(workspace, key, data, &["combined_g"], 3.0))
    .or_else(|| {
        normalized_run_series(
            workspace,
            key,
            data,
            &[
                "lateral_g",
                "gps_lateral_acceleration",
                "lateral_acceleration",
            ],
            3.0,
        )
    })
}

fn normalized_speed_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, AnalysisSourceData>,
) -> Option<ChannelSeries> {
    let (recording, segment) = segment(workspace, key)?;
    let (channel, source_offset) = recording_channel(recording, data, &["gps_speed", "speed"])?;
    if channel.descriptor.interpolation != Interpolation::Linear {
        return None;
    }
    let samples = channel
        .series
        .samples
        .iter()
        .filter_map(|sample| {
            let recording_time = sample.time - source_offset;
            if recording_time < segment.start_recording_time
                || recording_time > segment.end_recording_time
            {
                return None;
            }
            Unit::convert_value(
                sample.value,
                &channel.descriptor.unit,
                &Unit::MeterPerSecond,
            )
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| TimedSample {
                time: recording_time - segment.start_recording_time,
                value,
            })
        })
        .collect::<Vec<_>>();
    let series = ChannelSeries::new(samples);
    (series.samples.len() >= 24).then(|| series.low_pass_hz(2.0).unwrap_or(series))
}

fn speed_onset(series: &ChannelSeries) -> Option<f64> {
    for sample in &series.samples {
        if sample.value < 4.0 {
            continue;
        }
        let window = series
            .samples
            .iter()
            .filter(|candidate| {
                candidate.time >= sample.time && candidate.time <= sample.time + 3.0
            })
            .collect::<Vec<_>>();
        let spans = window
            .first()
            .zip(window.last())
            .is_some_and(|(first, last)| last.time - first.time >= 2.0);
        if spans
            && !window.is_empty()
            && window
                .iter()
                .filter(|candidate| candidate.value >= 4.0)
                .count()
                * 4
                >= window.len() * 3
        {
            return Some(sample.time);
        }
    }
    None
}

fn motion_onset(series: &ChannelSeries) -> Option<f64> {
    let end = series.samples.last()?.time;
    if end < 1.0 {
        return None;
    }
    let baseline_end = (end * 0.1).clamp(1.0, 3.0);
    let mut baseline_values = series
        .samples
        .iter()
        .take_while(|sample| sample.time <= baseline_end)
        .map(|sample| sample.value)
        .collect::<Vec<_>>();
    let baseline = median(&mut baseline_values)?;
    let mut baseline_activity = baseline_values
        .iter()
        .map(|value| (value - baseline).abs())
        .collect::<Vec<_>>();
    let noise = median(&mut baseline_activity).unwrap_or(0.0);
    let mut activity = series
        .samples
        .iter()
        .map(|sample| (sample.value - baseline).abs())
        .collect::<Vec<_>>();
    let percentile_index = activity.len() * 4 / 5;
    activity.sort_by(f64::total_cmp);
    let active_level = *activity.get(percentile_index.min(activity.len() - 1))?;
    if active_level <= noise * 2.0 + f64::EPSILON {
        return None;
    }
    let threshold = (noise * 6.0).max(active_level * 0.15).max(1e-6);
    for (index, sample) in series.samples.iter().enumerate() {
        let end_index = series.samples[index..]
            .partition_point(|candidate| candidate.time < sample.time + 0.6)
            + index;
        if end_index <= index + 2 {
            continue;
        }
        let window = &series.samples[index..end_index];
        let active = window
            .iter()
            .filter(|candidate| (candidate.value - baseline).abs() >= threshold)
            .count();
        if active * 3 >= window.len() * 2 {
            return Some(sample.time);
        }
    }
    None
}

fn onset_time_alignment(
    workspace: &AnalysisWorkspace,
    reference: &PreparedComparisonRun,
    target: &PreparedComparisonRun,
    data: &HashMap<SourceId, AnalysisSourceData>,
) -> Option<ComparisonTimeAlignment> {
    let onset = |run: &PreparedComparisonRun| {
        normalized_speed_series(workspace, &run.key, data)
            .and_then(|series| speed_onset(&series))
            .or_else(|| motion_onset(&motion_series(workspace, &run.key, data)?))
    };
    let reference_onset = onset(reference)?;
    let target_onset = onset(target)?;
    Some(ComparisonTimeAlignment {
        offset_seconds: reference_onset - target_onset,
        coefficient: None,
        channel: "sustained motion onset",
        distance_offset_meters: None,
    })
}

/// Standing starts are timed where each run has traveled this far from rest:
/// past the distance that creeps while staged, near where timing begins, and
/// short enough that differing launches still show.
const STANDING_START_METERS: f64 = 3.0;

fn starts_at_rest(run: &PreparedComparisonRun) -> bool {
    super::drift::starts_at_rest(&run.distance, run.start)
}

/// Standing starts from a shared staging spot: time each run where it has
/// traveled the same short distance from rest. Traveled distance comes from
/// speed, so unlike GPS position it does not drift between runs; on the
/// supplied autocross logs, GPS position drifted several metres.
fn launch_time_alignment(
    reference: &PreparedComparisonRun,
    target: &PreparedComparisonRun,
) -> Option<ComparisonTimeAlignment> {
    if !starts_at_rest(reference) || !starts_at_rest(target) {
        return None;
    }
    let launched = |run: &PreparedComparisonRun| {
        Some(time_at_progress(&run.distance, STANDING_START_METERS)? - run.start)
    };
    Some(ComparisonTimeAlignment {
        offset_seconds: launched(reference)? - launched(target)?,
        coefficient: None,
        channel: "standing start",
        distance_offset_meters: None,
    })
}

pub(super) fn best_effort_time_alignment(
    workspace: &AnalysisWorkspace,
    reference: &PreparedComparisonRun,
    target: &PreparedComparisonRun,
    data: &HashMap<SourceId, AnalysisSourceData>,
) -> Option<ComparisonTimeAlignment> {
    launch_time_alignment(reference, target)
        .or_else(|| spatial_time_alignment(workspace, reference, target, data))
        .or_else(|| onset_time_alignment(workspace, reference, target, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run sampled at 10 Hz for 40 s whose traveled distance creeps a
    /// little while staged, then grows at `speed` from `delay`.
    fn run(delay: f64, speed: f64) -> PreparedComparisonRun {
        let distance = (0..=400)
            .map(|index| {
                let time = index as f64 * 0.1;
                ProgressSample {
                    recording_time: time,
                    progress: time.min(delay) * 0.01 + ((time - delay) * speed).max(0.0),
                    confidence: 1.0,
                }
            })
            .collect();
        PreparedComparisonRun {
            key: SegmentRef {
                recording_id: RecordingId::new(),
                segment_id: SegmentId::new(),
            },
            name: "run".into(),
            start: 0.0,
            end: 40.0,
            gps: vec![],
            distance,
            progress: vec![],
            time_alignment: None,
            start_gate: None,
            finish_gate: None,
            gps_drift: None,
        }
    }

    #[test]
    fn standing_starts_are_timed_at_the_same_distance_from_rest() {
        let reference = run(1.5, 5.0);
        let target = run(3.5, 4.0);

        let alignment = launch_time_alignment(&reference, &target).unwrap();

        let launched =
            |delay: f64, speed: f64| delay + (STANDING_START_METERS - delay * 0.01) / speed;
        assert_eq!(alignment.channel, "standing start");
        assert!(
            (alignment.offset_seconds - (launched(1.5, 5.0) - launched(3.5, 4.0))).abs() < 1e-9,
            "{alignment:?}"
        );
    }

    #[test]
    fn a_run_already_moving_is_not_a_standing_start() {
        assert!(launch_time_alignment(&run(1.5, 5.0), &run(0.0, 5.0)).is_none());
    }
}
