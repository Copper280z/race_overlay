//! Source filtering, calibration, correlation, and cleanup policies.
use overlay_core::{CameraCalibration, TelemetryDataset};
#[cfg(test)]
use overlay_core::{ProjectV1, SourceId};
use serde_json::{Value, json};

pub(super) fn forward_axis_label(axis: usize) -> &'static str {
    match axis {
        0 => "+X points forward",
        1 => "−X points forward",
        2 => "+Y points forward",
        3 => "−Y points forward",
        4 => "+Z points forward",
        5 => "−Z points forward",
        _ => "+X points forward",
    }
}

pub(super) fn forward_axis_vector(axis: usize) -> [f64; 3] {
    match axis {
        0 => [1.0, 0.0, 0.0],
        1 => [-1.0, 0.0, 0.0],
        2 => [0.0, 1.0, 0.0],
        3 => [0.0, -1.0, 0.0],
        4 => [0.0, 0.0, 1.0],
        5 => [0.0, 0.0, -1.0],
        _ => [1.0, 0.0, 0.0],
    }
}

pub(super) fn dataset_duration(dataset: &TelemetryDataset) -> Option<f64> {
    let first = dataset
        .channels
        .values()
        .filter_map(|channel| channel.series.samples.first().map(|sample| sample.time))
        .min_by(f64::total_cmp)?;
    let last = dataset
        .channels
        .values()
        .filter_map(|channel| channel.series.samples.last().map(|sample| sample.time))
        .max_by(f64::total_cmp)?;
    Some((last - first).max(0.0))
}

/// Convert a user-facing adjustment window around the target's current
/// alignment into the raw target-minus-reference lag searched by the core.
pub(super) fn correlation_lag_window(
    target_offset: f64,
    reference_offset: f64,
    first_adjustment: f64,
    second_adjustment: f64,
) -> (f64, f64) {
    let current_lag = target_offset - reference_offset;
    (
        current_lag + first_adjustment.min(second_adjustment),
        current_lag + first_adjustment.max(second_adjustment),
    )
}

/// Translate a raw lag into the adjustment and resulting target offset shown
/// in the estimate UI.
pub(super) fn correlation_candidate_offsets(
    raw_lag: f64,
    target_offset: f64,
    reference_offset: f64,
) -> (f64, f64) {
    let resulting_target_offset = reference_offset + raw_lag;
    (
        resulting_target_offset - target_offset,
        resulting_target_offset,
    )
}

const SOURCE_LOW_PASS_ENABLED: &str = "low_pass_enabled";
const SOURCE_LOW_PASS_HZ: &str = "low_pass_hz";

pub(super) fn source_low_pass_settings(settings: &Value) -> (bool, f64) {
    let enabled = settings
        .get(SOURCE_LOW_PASS_ENABLED)
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let cutoff = settings
        .get(SOURCE_LOW_PASS_HZ)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(8.0)
        .clamp(0.5, 50.0);
    (enabled, cutoff)
}

pub(super) fn set_source_low_pass_settings(settings: &mut Value, enabled: bool, cutoff_hz: f64) {
    if !settings.is_object() {
        *settings = json!({});
    }
    let object = settings
        .as_object_mut()
        .expect("source settings normalized to an object");
    object.insert(SOURCE_LOW_PASS_ENABLED.into(), json!(enabled));
    object.insert(SOURCE_LOW_PASS_HZ.into(), json!(cutoff_hz.clamp(0.5, 50.0)));
}

#[cfg(test)]
pub(super) fn apply_low_pass_to_dataset(
    dataset: &mut TelemetryDataset,
    cutoff_hz: f64,
) -> Result<usize, String> {
    overlay_core::apply_low_pass_to_dataset(dataset, cutoff_hz)
}

/// Apply source conditioning before deriving camera-frame channels. Keeping
/// this order in one helper prevents source reload and recalibration actions
/// from producing different filter cascades.
pub(super) fn prepare_loaded_dataset(
    dataset: &mut TelemetryDataset,
    source_cutoff_hz: Option<f64>,
    camera_calibration: Option<&CameraCalibration>,
) -> Result<usize, String> {
    overlay_core::prepare_loaded_dataset(dataset, source_cutoff_hz, camera_calibration)
}

#[cfg(test)]
pub(super) fn remove_source_bindings(project: &mut ProjectV1, source_id: SourceId) -> usize {
    let mut removed = 0;
    for widget in &mut project.widgets {
        let before = widget.bindings.len();
        widget
            .bindings
            .retain(|binding| binding.channel.source_id != source_id);
        removed += before - widget.bindings.len();
    }
    removed
}

#[cfg(test)]
pub(super) fn remove_source_from_project(
    project: &mut ProjectV1,
    source_id: SourceId,
) -> Option<usize> {
    let index = project
        .sources
        .iter()
        .position(|source| source.id == source_id)?;
    project.sources.remove(index);
    remove_source_bindings(project, source_id);
    Some(index)
}
