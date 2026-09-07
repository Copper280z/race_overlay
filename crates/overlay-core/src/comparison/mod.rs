//! Prepared comparison data and source-agnostic alignment algorithms.

use crate::*;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone)]
pub struct AnalysisSourceData {
    pub raw: Arc<TelemetryDataset>,
    pub processed: Arc<TelemetryDataset>,
}

impl AnalysisSourceData {
    pub fn from_dataset(dataset: Arc<TelemetryDataset>) -> Self {
        Self {
            raw: dataset.clone(),
            processed: dataset,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComparisonTimeAlignment {
    /// Added to segment-relative time to place it on the reference time base.
    pub offset_seconds: f64,
    pub coefficient: Option<f64>,
    pub channel: &'static str,
    pub distance_offset_meters: Option<f64>,
}

#[derive(Clone)]
pub struct PreparedComparisonRun {
    pub key: SegmentRef,
    pub name: String,
    pub start: f64,
    pub end: f64,
    pub gps: Vec<GpsPoint>,
    pub distance: Vec<ProgressSample>,
    pub progress: Vec<ProgressSample>,
    pub time_alignment: Option<ComparisonTimeAlignment>,
}

#[derive(Default)]
pub struct PreparedComparison {
    pub runs: Vec<PreparedComparisonRun>,
    pub course: Option<ReferenceCourse>,
    pub automatic_time_alignment: bool,
}

mod alignment;
mod sampling;

use alignment::best_effort_time_alignment;
use sampling::timed_samples_inside_interval;
pub use sampling::{gate_window, gps_inside_interval, segment};

pub fn prepare_comparison(
    workspace: &AnalysisWorkspace,
    selection: &[SegmentRef],
    data: &HashMap<SourceId, AnalysisSourceData>,
) -> PreparedComparison {
    let gps_for = |key: &SegmentRef| -> Vec<GpsPoint> {
        let Some((r, s)) = segment(workspace, key) else {
            return vec![];
        };
        let gps = data
            .get(&r.primary_source)
            .map(|d| gps_points(&d.raw, r))
            .unwrap_or_default();
        gps_inside_interval(&gps, s.start_recording_time, s.end_recording_time)
    };
    let course = workspace
        .reference
        .as_ref()
        .map(&gps_for)
        .filter(|p| p.len() > 1)
        .map(ReferenceCourse::from_points);
    let mut keys = selection.to_vec();
    if let Some(reference) = &workspace.reference
        && !keys.contains(reference)
    {
        keys.push(reference.clone());
    }
    let automatic_time_alignment = workspace.course.gates.is_empty()
        && keys
            .iter()
            .map(|key| key.recording_id)
            .collect::<HashSet<_>>()
            .len()
            > 1;
    let mut runs = keys
        .iter()
        .filter_map(|key| {
            let (r, s) = segment(workspace, key)?;
            let dataset = &data.get(&r.primary_source)?.raw;
            let gps = gps_for(key);
            let traveled = traveled_distance(dataset, r, &gps);
            let in_range = timed_samples_inside_interval(
                &traveled.samples,
                &traveled.gaps,
                s.start_recording_time,
                s.end_recording_time,
            );
            let first = in_range.first().map_or(0.0, |p| p.value);
            let distance = in_range
                .into_iter()
                .map(|p| ProgressSample {
                    recording_time: p.time,
                    progress: p.value - first,
                    confidence: if traveled
                        .gaps
                        .iter()
                        .any(|(a, b)| p.time > *a && p.time < *b)
                    {
                        0.0
                    } else {
                        1.0
                    },
                })
                .collect();
            let anchors = workspace
                .course
                .manual_anchors
                .iter()
                .filter(|a| a.segment.as_ref() == Some(key))
                .cloned()
                .map(|mut a| {
                    a.segment = None;
                    a
                })
                .collect::<Vec<_>>();
            let progress = course
                .as_ref()
                .map(|c| {
                    if workspace.reference.as_ref() == Some(key) {
                        c.points
                            .iter()
                            .zip(&c.cumulative_meters)
                            .enumerate()
                            .map(|(i, (p, s))| ProgressSample {
                                recording_time: p.recording_time,
                                progress: *s,
                                confidence: if i == 0 || c.valid_segments.get(i - 1) == Some(&true)
                                {
                                    1.0
                                } else {
                                    0.0
                                },
                            })
                            .collect()
                    } else {
                        match_reference_course(c, &gps, &anchors)
                    }
                })
                .unwrap_or_default();
            Some(PreparedComparisonRun {
                key: key.clone(),
                name: format!("{} / {}", r.name, s.name),
                start: s.start_recording_time,
                end: s.end_recording_time,
                gps,
                distance,
                progress,
                time_alignment: None,
            })
        })
        .collect::<Vec<_>>();
    if automatic_time_alignment
        && let Some(reference) = &workspace.reference
        && let Some(reference_run) = runs.iter().find(|run| run.key == *reference).cloned()
    {
        for run in &mut runs {
            if run.key != *reference {
                run.time_alignment =
                    best_effort_time_alignment(workspace, &reference_run, run, data);
            }
        }
    }
    PreparedComparison {
        runs,
        course,
        automatic_time_alignment,
    }
}
