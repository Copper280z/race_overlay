//! Shared, time-domain source conditioning for analysis and overlay export.
use crate::{CameraCalibration, Interpolation, TelemetryDataset, add_derived_inertial_channels};

/// Filter imported continuous channels without modifying held/discrete values.
/// Call on a fresh imported dataset, not on a previously conditioned copy.
pub fn apply_low_pass_to_dataset(
    dataset: &mut TelemetryDataset,
    cutoff_hz: f64,
) -> Result<usize, String> {
    // Prepare all results first so invalid input cannot leave a partially
    // conditioned dataset behind.
    let series = dataset
        .channels
        .iter()
        .filter(|(_, channel)| {
            channel.descriptor.interpolation == Interpolation::Linear
                && channel.series.samples.len() >= 2
        })
        .map(|(id, channel)| {
            channel
                .series
                .low_pass_hz(cutoff_hz)
                .map(|series| (*id, series))
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let count = series.len();
    for (id, series) in series {
        if let Some(channel) = dataset.channels.get_mut(&id) {
            channel.series = series;
        }
    }
    Ok(count)
}

/// Source smoothing precedes camera-derived G smoothing. Both are optional,
/// zero-phase and non-causal. Display filters belong after this preparation,
/// before any resampling from time into distance.
pub fn prepare_loaded_dataset(
    dataset: &mut TelemetryDataset,
    source_cutoff_hz: Option<f64>,
    camera_calibration: Option<&CameraCalibration>,
) -> Result<usize, String> {
    let count = match source_cutoff_hz {
        Some(hz) => apply_low_pass_to_dataset(dataset, hz)?,
        None => 0,
    };
    if let Some(calibration) = camera_calibration {
        add_derived_inertial_channels(dataset, calibration, 1_000.0);
    }
    Ok(count)
}
