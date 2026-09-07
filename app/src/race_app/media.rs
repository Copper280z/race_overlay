//! Preview, rendering, export, and background-worker coordination.
use super::super::policy::{dataset_duration, prepare_loaded_dataset, source_low_pass_settings};
use super::super::{PREVIEW_H, PREVIEW_W};
use super::OverlayEditor;
use super::WorkerEvent;
use eframe::egui;
use overlay_core::{SourceId, builtin_adapter_capabilities};
use overlay_media::{PreviewSize, VideoMetadata};
use overlay_render::{
    AlignedDatasets, RenderOptions, RenderSize, render_project_widgets_with_appearance,
};
use std::collections::HashMap;

impl OverlayEditor {
    pub(in crate::race_app) fn step_frame(&mut self, direction: i32) {
        let step = self
            .session
            .metadata()
            .and_then(VideoMetadata::fps)
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .map_or(1.0 / 30.0, |fps| 1.0 / fps);
        self.stop_playback();
        let time =
            (self.session.current_time() + f64::from(direction) * step).clamp(0.0, self.duration());
        self.session.set_current_time(time);
        self.request_preview();
        self.refresh_overlay();
    }

    pub(super) fn request_preview(&mut self) {
        if let Some(preview) = &self.preview_controller.preview
            && let Err(e) = preview.request(
                self.session.current_time().max(0.0),
                PreviewSize::new(PREVIEW_W, PREVIEW_H),
            )
        {
            self.status = e.to_string();
        }
    }

    pub(super) fn start_playback(&mut self) {
        let fps = self
            .session
            .metadata()
            .and_then(VideoMetadata::fps)
            .unwrap_or(30.0)
            .clamp(15.0, 60.0);
        let result = self.preview_controller.preview.as_ref().map(|preview| {
            preview.play(
                self.session.current_time(),
                fps,
                PreviewSize::new(PREVIEW_W, PREVIEW_H),
            )
        });
        match result {
            Some(Ok(playback)) => {
                self.preview_controller.preview_playback = Some(playback);
                self.preview_controller.playing = true;
            }
            Some(Err(error)) => self.status = error.to_string(),
            None => {}
        }
    }

    pub(in crate::race_app) fn stop_playback(&mut self) {
        if let Some(playback) = self.preview_controller.preview_playback.take() {
            playback.pause();
        }
        self.preview_controller.playing = false;
    }

    pub(super) fn source_offsets(&self) -> HashMap<SourceId, f64> {
        self.project()
            .map(|p| {
                p.sources
                    .iter()
                    .map(|s| (s.id, s.alignment.offset_seconds))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn refresh_overlay(&mut self) {
        let Some(project) = self.project() else {
            self.preview_controller.overlay_texture = None;
            return;
        };
        let aligned = AlignedDatasets {
            datasets: self.session.datasets(),
            source_offsets: self.source_offsets(),
            ..Default::default()
        };
        let appearance = project.appearance.clone();
        let result = render_project_widgets_with_appearance(
            &project.widgets,
            &aligned,
            &appearance,
            RenderSize::new(PREVIEW_W, PREVIEW_H),
            self.session.current_time(),
            RenderOptions {
                crop: false,
                full_size: false,
            },
        );
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [result.image.width as usize, result.image.height as usize],
            &result.image.pixels,
        );
        if let Some(texture) = self.preview_controller.overlay_texture.as_mut() {
            texture.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.preview_controller.pending_overlay_image = Some(image);
        }
    }

    pub(in crate::race_app) fn poll_workers(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self.worker_hub.drain().collect();
        for event in events {
            match event {
                WorkerEvent::SourceLoaded(generation, id, Ok(mut dataset)) => {
                    if !self.worker_hub.source_generation_is_current(id, generation) {
                        continue;
                    }
                    // A source may have been removed while its adapter was
                    // loading. Do not let that late result resurrect it.
                    if !self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        continue;
                    }
                    let saved_calibration = self.project().and_then(|project| {
                        project
                            .sources
                            .iter()
                            .find(|source| {
                                source.id == id
                                    && builtin_adapter_capabilities(&source.adapter)
                                        .vehicle_frame_calibration
                            })
                            .and_then(|_| {
                                project
                                    .camera_calibration
                                    .notes
                                    .as_ref()
                                    .map(|_| project.camera_calibration.clone())
                            })
                    });
                    let source_filter = self.project().and_then(|project| {
                        project
                            .sources
                            .iter()
                            .find(|source| source.id == id)
                            .map(|source| source_low_pass_settings(&source.settings))
                    });
                    let source_cutoff_hz =
                        source_filter.and_then(|(enabled, cutoff_hz)| enabled.then_some(cutoff_hz));
                    let filtered_channels = match prepare_loaded_dataset(
                        &mut dataset,
                        source_cutoff_hz,
                        saved_calibration.as_ref(),
                    ) {
                        Ok(count) => count,
                        Err(error) => {
                            self.status = error;
                            0
                        }
                    };
                    if let Some(old) = self
                        .session
                        .datasets()
                        .iter()
                        .position(|d| d.source_id == id)
                    {
                        self.session.datasets_mut()[old] = dataset;
                    } else {
                        self.session.datasets_mut().push(dataset);
                    }
                    // Reloading a source (including applying its source-level
                    // filter) changes the samples an estimate was based on.
                    self.invalidate_correlation();
                    let count = self
                        .session
                        .datasets()
                        .iter()
                        .find(|d| d.source_id == id)
                        .map(|d| d.channels.len())
                        .unwrap_or(0);
                    self.status = if filtered_channels == 0 {
                        format!("Telemetry loaded: {count} channels")
                    } else {
                        format!(
                            "Telemetry loaded: {count} channels; {filtered_channels} low-pass filtered"
                        )
                    };
                    self.repair_missing_bindings(id);
                    self.auto_bind_unbound_widgets();
                    self.refresh_overlay();
                    let should_auto_sync = self.project().is_some_and(|project| {
                        project.sources.iter().any(|source| {
                            source.id == id
                                && builtin_adapter_capabilities(&source.adapter).embedded_audio_sync
                                && source.alignment.offset_seconds.abs() < 1e-9
                        })
                    }) && self
                        .session
                        .datasets()
                        .iter()
                        .find(|dataset| dataset.source_id == id)
                        .and_then(dataset_duration)
                        .is_some_and(|duration| duration > self.duration() + 1.0);
                    if should_auto_sync {
                        self.sync_source(id);
                    }
                }
                WorkerEvent::SourceLoaded(generation, id, Err(e)) => {
                    if !self.worker_hub.source_generation_is_current(id, generation) {
                        continue;
                    }
                    if self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        self.status = e;
                    }
                }
                WorkerEvent::Synced(generation, id, Ok(result)) => {
                    if !self.worker_hub.sync_generation_is_current(id, generation) {
                        continue;
                    }
                    self.source_editor
                        .syncing_sources
                        .retain(|source_id| *source_id != id);
                    if !self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        continue;
                    }
                    // align_audio reports the exported clip relative to raw audio.
                    // Project offsets map video time back into the raw source timeline.
                    let offset = -result.offset_seconds;
                    if let Some(source) = self
                        .project_mut()
                        .and_then(|p| p.sources.iter_mut().find(|s| s.id == id))
                    {
                        source.alignment.offset_seconds = offset;
                    }
                    self.invalidate_correlation();
                    self.status = format!(
                        "Telemetry synchronized to exported-video time (peak {:.2}, confidence {:.2}){}",
                        result.normalized_peak,
                        result.confidence,
                        if result.auto_acceptable() {
                            ""
                        } else {
                            "; verify manually"
                        }
                    );
                    self.refresh_overlay();
                }
                WorkerEvent::Synced(generation, id, Err(e)) => {
                    if !self.worker_hub.sync_generation_is_current(id, generation) {
                        continue;
                    }
                    self.source_editor
                        .syncing_sources
                        .retain(|source_id| *source_id != id);
                    if !self
                        .project()
                        .is_some_and(|project| project.sources.iter().any(|source| source.id == id))
                    {
                        continue;
                    }
                    self.status = format!("Automatic telemetry alignment failed: {e}");
                }
                WorkerEvent::CorrelationEstimated(job, result) => {
                    let Ok(estimate) = result else {
                        if job == self.source_editor.correlation_job {
                            self.source_editor.correlation_running = false;
                            self.status = format!("Correlation failed: {}", result.unwrap_err());
                        }
                        continue;
                    };
                    if !self.worker_hub.correlation_generation_is_current(job)
                        || job != self.source_editor.correlation_job
                    {
                        continue;
                    }
                    self.source_editor.correlation_running = false;
                    // A source may have been removed, or a newer estimate may
                    // have superseded this one, while the worker was running.
                    // In either case the result is no longer safe to expose.
                    let still_loaded = self.project().is_some_and(|project| {
                        project.sources.iter().any(|source| {
                            source.id == estimate.target_source
                                && project
                                    .sources
                                    .iter()
                                    .any(|other| other.id == estimate.reference_source)
                        })
                    }) && self.session.datasets().iter().any(|dataset| {
                        dataset.source_id == estimate.target_source
                            && dataset
                                .channel(estimate.target_channel.channel_id)
                                .is_some()
                    }) && self.session.datasets().iter().any(|dataset| {
                        dataset.source_id == estimate.reference_source
                            && dataset
                                .channel(estimate.reference_channel.channel_id)
                                .is_some()
                    });
                    if !still_loaded {
                        continue;
                    }
                    self.status = format!(
                        "Correlation: lag {:+.3}s, adjustment {:+.3}s, r {:+.3} · {:.2}s overlap · {} samples",
                        estimate.raw_lag_seconds,
                        estimate.adjustment_seconds,
                        estimate.coefficient,
                        estimate.overlap_seconds,
                        estimate.samples
                    );
                    self.source_editor.correlation_result = Some(estimate);
                }
                WorkerEvent::ExportProgress(p) => {
                    self.export_controller.export_fraction = p
                        .duration_seconds
                        .map(|d| (p.encoded_seconds / d).clamp(0.0, 1.0) as f32)
                        .unwrap_or(0.0);
                }
                WorkerEvent::ExportFinished(result) => {
                    self.export_controller.export_cancel = None;
                    self.export_controller.export_fraction = 0.0;
                    self.status = match result {
                        Ok(path) => format!("Export finished: {}", path.display()),
                        Err(e) => format!("Export failed: {e}"),
                    };
                }
            }
        }
        let mut playback_advanced = false;
        if let Some(preview) = &self.preview_controller.preview {
            while let Some(frame) = preview.try_recv() {
                match frame {
                    Ok(frame) => {
                        if self.preview_controller.playing {
                            self.session
                                .set_current_time(frame.timestamp.min(self.duration()));
                            playback_advanced = true;
                        } else if (frame.timestamp - self.session.current_time()).abs() > 0.75 {
                            continue;
                        }
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [frame.width as usize, frame.height as usize],
                            &frame.rgba,
                        );
                        if let Some(texture) = self.preview_controller.video_texture.as_mut() {
                            texture.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            self.preview_controller.video_texture = Some(ctx.load_texture(
                                "video-preview",
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                    Err(e) => self.status = e.to_string(),
                }
            }
        }
        if playback_advanced {
            self.refresh_overlay();
        }
    }
}
