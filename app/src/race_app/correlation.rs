//! Cross-source channel correlation controls.
use super::super::policy::{correlation_candidate_offsets, correlation_lag_window};
use super::OverlayEditor;
use super::{CorrelationEstimate, WorkerEvent};
use overlay_core::{
    CorrelationConfig, SourceId, correlate_channel_series, feasible_correlation_lag_range,
};
use std::thread;

impl OverlayEditor {
    pub(super) fn start_correlation(&mut self, target_source: SourceId) {
        let Some(target_channel) = self.source_editor.correlation_target_channel.clone() else {
            return;
        };
        let Some(reference_channel) = self.source_editor.correlation_reference_channel.clone()
        else {
            return;
        };
        let Some(reference_source) = self.source_editor.correlation_reference_source else {
            return;
        };
        if target_channel.source_id != target_source
            || reference_channel.source_id != reference_source
            || target_source == reference_source
        {
            self.status = "Choose channels from two different loaded sources".into();
            return;
        }
        let Some(target) = self
            .session
            .datasets()
            .iter()
            .find(|dataset| dataset.source_id == target_source)
            .and_then(|dataset| dataset.channel(target_channel.channel_id))
            .cloned()
        else {
            self.status = "Target channel is unavailable".into();
            return;
        };
        let Some(reference) = self
            .session
            .datasets()
            .iter()
            .find(|dataset| dataset.source_id == reference_source)
            .and_then(|dataset| dataset.channel(reference_channel.channel_id))
            .cloned()
        else {
            self.status = "Reference channel is unavailable".into();
            return;
        };
        let offsets = self.source_offsets();
        let target_offset = offsets.get(&target_source).copied().unwrap_or(0.0);
        let reference_offset = offsets.get(&reference_source).copied().unwrap_or(0.0);
        let (min_lag, max_lag) = if self.source_editor.correlation_all_time {
            let Some(bounds) = feasible_correlation_lag_range(
                &reference.series,
                &target.series,
                CorrelationConfig::default().min_overlap_seconds,
            ) else {
                self.status = "The selected channels do not have a searchable time range".into();
                return;
            };
            bounds
        } else {
            correlation_lag_window(
                target_offset,
                reference_offset,
                self.source_editor.correlation_min_adjustment,
                self.source_editor.correlation_max_adjustment,
            )
        };
        let absolute = self.source_editor.correlation_absolute;
        let job = self.worker_hub.next_correlation_generation();
        self.source_editor.correlation_job = job;
        self.source_editor.correlation_running = true;
        self.source_editor.correlation_result = None;
        self.status = "Estimating telemetry correlation…".into();
        let tx = self.worker_hub.sender();
        thread::spawn(move || {
            let result = correlate_channel_series(
                &reference.series,
                &target.series,
                &CorrelationConfig {
                    min_lag_seconds: min_lag,
                    max_lag_seconds: max_lag,
                    use_absolute_correlation: absolute,
                    ..CorrelationConfig::default()
                },
            )
            .map(|result| {
                let (adjustment_seconds, target_offset_seconds) = correlation_candidate_offsets(
                    result.target_minus_reference_seconds,
                    target_offset,
                    reference_offset,
                );
                CorrelationEstimate {
                    target_source,
                    target_channel,
                    reference_source,
                    reference_channel,
                    raw_lag_seconds: result.target_minus_reference_seconds,
                    adjustment_seconds,
                    target_offset_seconds,
                    reference_offset_seconds: reference_offset,
                    target_current_offset_seconds: target_offset,
                    coefficient: result.correlation_coefficient,
                    samples: result.sample_count,
                    overlap_seconds: result.overlap_seconds,
                    resolution_seconds: result.lag_resolution_seconds,
                }
            })
            .map_err(|error| error.to_string());
            let _ = tx.send(WorkerEvent::CorrelationEstimated(job, result));
        });
    }

    pub(super) fn apply_correlation_candidate(&mut self, target_source: SourceId) {
        let Some(estimate) = self.source_editor.correlation_result.clone() else {
            return;
        };
        if estimate.target_source != target_source {
            return;
        }
        let offsets = self.source_offsets();
        let Some(reference_offset) = offsets.get(&estimate.reference_source).copied() else {
            self.status = "The reference source is no longer available; estimate again".into();
            self.invalidate_correlation();
            return;
        };
        let Some(current_target_offset) = offsets.get(&target_source).copied() else {
            self.status = "The target source is no longer available; estimate again".into();
            self.invalidate_correlation();
            return;
        };
        if (reference_offset - estimate.reference_offset_seconds).abs() > 1e-9
            || (current_target_offset - estimate.target_current_offset_seconds).abs() > 1e-9
        {
            self.status = "A source offset changed; estimate correlation again".into();
            self.invalidate_correlation();
            return;
        }
        let target_offset = reference_offset + estimate.raw_lag_seconds;
        if let Some(source) = self.project_mut().and_then(|project| {
            project
                .sources
                .iter_mut()
                .find(|source| source.id == target_source)
        }) {
            source.alignment.offset_seconds = target_offset;
            self.status = format!(
                "Applied correlation offset {target_offset:+.3}s (r {:+.3})",
                estimate.coefficient
            );
            self.source_editor.correlation_result = None;
            self.refresh_overlay();
        }
    }
}
