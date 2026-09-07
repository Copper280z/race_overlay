//! Channel selection and logger-to-camera correlation.
use super::super::*;
use super::*;

fn normalized_alignment_channel(name: &str) -> String {
    name.to_ascii_lowercase().replace([' ', '-'], "_")
}

fn alignment_channel_family(name: &str) -> Option<&'static str> {
    match normalized_alignment_channel(name).as_str() {
        "lateral_g" | "gps_lateral_acceleration" | "lateral_acceleration" => Some("lateral"),
        "gps_speed" | "speed" => Some("speed"),
        "yaw_rate" | "gps_yaw_rate" => Some("yaw"),
        "rpm" => Some("rpm"),
        "combined_g" => Some("combined_g"),
        _ => None,
    }
}

fn alignment_channel_rank(name: &str) -> usize {
    match alignment_channel_family(name) {
        Some("lateral") => 0,
        Some("speed") => 1,
        Some("yaw") => 2,
        Some("rpm") => 3,
        Some("combined_g") => 4,
        _ => 5,
    }
}

fn alignment_channels_match(left: &str, right: &str) -> bool {
    normalized_alignment_channel(left) == normalized_alignment_channel(right)
        || alignment_channel_family(left)
            .zip(alignment_channel_family(right))
            .is_some_and(|(left, right)| left == right)
}

impl AnalysisApp {
    pub(super) fn video_alignment_channel_choices(
        &self,
        source_id: SourceId,
    ) -> Vec<(ChannelRef, String)> {
        let Some(dataset) = self.data.get(&source_id).map(|data| &data.processed) else {
            return vec![];
        };
        let mut channels = dataset
            .channels
            .values()
            .filter(|channel| {
                channel.descriptor.interpolation == Interpolation::Linear
                    && channel.series.samples.len() >= 24
            })
            .collect::<Vec<_>>();
        channels.sort_by(|left, right| {
            alignment_channel_rank(&left.descriptor.name)
                .cmp(&alignment_channel_rank(&right.descriptor.name))
                .then_with(|| right.series.samples.len().cmp(&left.series.samples.len()))
                .then_with(|| left.descriptor.name.cmp(&right.descriptor.name))
        });
        channels
            .into_iter()
            .map(|channel| {
                (
                    ChannelRef {
                        source_id,
                        channel_id: channel.descriptor.id,
                    },
                    format!(
                        "{} ({}) · {} samples",
                        channel.descriptor.name,
                        channel.descriptor.unit.symbol(),
                        channel.series.samples.len()
                    ),
                )
            })
            .collect()
    }

    pub(super) fn default_video_alignment_channels(
        &self,
        target_source: SourceId,
        camera_source: SourceId,
    ) -> Option<(ChannelRef, ChannelRef)> {
        let target = self.data.get(&target_source)?.processed.as_ref();
        let camera = self.data.get(&camera_source)?.processed.as_ref();
        target
            .channels
            .values()
            .filter(|channel| {
                channel.descriptor.interpolation == Interpolation::Linear
                    && channel.series.samples.len() >= 24
            })
            .flat_map(|target_channel| {
                camera
                    .channels
                    .values()
                    .filter(move |camera_channel| {
                        camera_channel.descriptor.interpolation == Interpolation::Linear
                            && camera_channel.series.samples.len() >= 24
                            && target_channel.descriptor.unit.family().is_some()
                            && target_channel.descriptor.unit.family()
                                == camera_channel.descriptor.unit.family()
                    })
                    .map(move |camera_channel| {
                        let channels_match = alignment_channels_match(
                            &target_channel.descriptor.name,
                            &camera_channel.descriptor.name,
                        );
                        let score = (
                            !channels_match,
                            alignment_channel_rank(&target_channel.descriptor.name)
                                .max(alignment_channel_rank(&camera_channel.descriptor.name)),
                            usize::MAX
                                - target_channel
                                    .series
                                    .samples
                                    .len()
                                    .min(camera_channel.series.samples.len()),
                        );
                        (
                            score,
                            ChannelRef {
                                source_id: target_source,
                                channel_id: target_channel.descriptor.id,
                            },
                            ChannelRef {
                                source_id: camera_source,
                                channel_id: camera_channel.descriptor.id,
                            },
                        )
                    })
            })
            .min_by(|left, right| left.0.cmp(&right.0))
            .map(|(_, target, camera)| (target, camera))
    }

    pub(super) fn start_video_alignment_estimate(&mut self, recording_id: RecordingId) {
        let Some(recording) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == recording_id)
        else {
            return;
        };
        let Some((target_channel_ref, camera_channel_ref)) = self
            .video_sync
            .recording(recording_id)
            .and_then(|state| state.channels.clone())
        else {
            return;
        };
        let Some(camera_source) = recording
            .sources
            .iter()
            .find(|source| source.id == camera_channel_ref.source_id)
        else {
            return;
        };
        let Some(camera_video_offset_seconds) =
            CameraVideoSyncMetadata::read(&camera_source.settings).camera_minus_video_seconds
        else {
            return;
        };
        let Some(target) = self
            .data
            .get(&target_channel_ref.source_id)
            .and_then(|data| data.processed.channel(target_channel_ref.channel_id))
            .cloned()
        else {
            return;
        };
        let Some(camera) = self
            .data
            .get(&camera_channel_ref.source_id)
            .and_then(|data| data.processed.channel(camera_channel_ref.channel_id))
            .cloned()
        else {
            return;
        };
        let config = CorrelationConfig::default();
        let Some((min_lag_seconds, max_lag_seconds)) = feasible_correlation_lag_range(
            &camera.series,
            &target.series,
            config.min_overlap_seconds,
        ) else {
            return;
        };
        self.load_serial = self.load_serial.wrapping_add(1);
        let serial = self.load_serial;
        let sync_state = self.video_sync.recording_mut(recording_id);
        sync_state.alignment_job = Some(serial);
        sync_state.alignment_result = None;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = correlate_channel_series(
                &camera.series,
                &target.series,
                &CorrelationConfig {
                    min_lag_seconds,
                    max_lag_seconds,
                    ..config
                },
            )
            .map(|result| VideoAlignmentCandidate {
                recording_id,
                target_source: target_channel_ref.source_id,
                camera_source: camera_channel_ref.source_id,
                target_channel: target_channel_ref,
                camera_channel: camera_channel_ref,
                camera_video_offset_seconds,
                result,
            })
            .map_err(|error| error.to_string());
            let _ = tx.send(Event::VideoAlignmentEstimated(serial, recording_id, result));
        });
    }

    pub(super) fn apply_video_alignment(&mut self, recording_id: RecordingId) {
        let Some(Ok(candidate)) = self
            .video_sync
            .recording(recording_id)
            .and_then(|state| state.alignment_result.clone())
        else {
            return;
        };
        let selected_channels = self
            .video_sync
            .recording(recording_id)
            .and_then(|state| state.channels.as_ref());
        let Some(recording) = self
            .workspace
            .recordings
            .iter_mut()
            .find(|recording| recording.id == recording_id)
        else {
            return;
        };
        if candidate.recording_id != recording.id
            || candidate.target_source != recording.primary_source
            || selected_channels
                != Some(&(
                    candidate.target_channel.clone(),
                    candidate.camera_channel.clone(),
                ))
        {
            return;
        }
        let Some(target_offset) = recording
            .sources
            .iter()
            .find(|source| source.id == candidate.target_source)
            .map(|source| source.alignment.offset_seconds)
        else {
            return;
        };
        let lag = candidate.result.target_minus_reference_seconds;
        let (camera_offset, video_offset) = resolved_video_alignment_offsets(
            target_offset,
            lag,
            candidate.camera_video_offset_seconds,
        );
        let Some(camera_source) = recording
            .sources
            .iter_mut()
            .find(|source| source.id == candidate.camera_source)
        else {
            return;
        };
        let current_audio_offset =
            CameraVideoSyncMetadata::read(&camera_source.settings).camera_minus_video_seconds;
        if current_audio_offset != Some(candidate.camera_video_offset_seconds) {
            self.message = "Camera/video audio alignment changed; estimate again.".into();
            self.video_sync.recording_mut(recording_id).alignment_result = None;
            return;
        }
        camera_source.alignment.offset_seconds = camera_offset;
        let mut metadata = CameraVideoSyncMetadata::read(&camera_source.settings);
        metadata.applied = true;
        metadata.write(&mut camera_source.settings);
        recording.video_offset_seconds = video_offset;
        self.video_sync.recording_mut(recording_id).alignment_result = None;
        self.message = format!(
            "Video aligned without changing the logger clock (video offset {:+.3}s, Pearson r {:+.3}). Existing gates and intervals remain valid.",
            recording.video_offset_seconds, candidate.result.correlation_coefficient
        );
        self.changed();
    }

    pub(in crate::analysis_app) fn handle_video_alignment_estimated(
        &mut self,
        serial: u64,
        recording_id: RecordingId,
        result: Result<VideoAlignmentCandidate, String>,
    ) {
        if self
            .video_sync
            .recording(recording_id)
            .and_then(|state| state.alignment_job)
            != Some(serial)
        {
            return;
        }
        self.video_sync.recording_mut(recording_id).alignment_job = None;
        if !self
            .workspace
            .recordings
            .iter()
            .any(|recording| recording.id == recording_id)
        {
            return;
        }
        self.video_sync.recording_mut(recording_id).alignment_result = Some(result.clone());
        self.message = match result {
            Ok(candidate) => format!(
                "Video alignment estimate: {:+.3}s logger-minus-camera lag, Pearson r {:+.3}; review and apply it in Recordings & laps.",
                candidate.result.target_minus_reference_seconds,
                candidate.result.correlation_coefficient
            ),
            Err(error) => format!("Video alignment estimate failed: {error}"),
        };
    }
}
