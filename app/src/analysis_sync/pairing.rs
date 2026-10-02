//! Putting a data log and a camera recording into one entry: finding which
//! files belong together by when they were recorded, attaching a log to an
//! existing video entry, and making the log the clock the entry is read in.
use super::super::*;
use super::*;
use overlay_core::{
    WallClockSpan, camera_start_from_file_name, dataset_span, logger_start_from_metadata,
};

/// Logger entries that each coincide with exactly one camera entry, and the
/// reverse. Anything less certain stays separate; the user can attach a log
/// to a video by hand.
fn unique_matches(
    cameras: &[(RecordingId, WallClockSpan)],
    loggers: &[(RecordingId, WallClockSpan)],
) -> Vec<(RecordingId, RecordingId)> {
    loggers
        .iter()
        .filter_map(|(logger, logger_span)| {
            let mut candidates = cameras
                .iter()
                .filter(|(_, camera_span)| camera_span.coincides_with(logger_span));
            let (camera, camera_span) = candidates.next()?;
            if candidates.next().is_some() {
                return None;
            }
            let rivals = loggers
                .iter()
                .filter(|(_, other)| other.coincides_with(camera_span))
                .count();
            (rivals == 1).then_some((*logger, *camera))
        })
        .collect()
}

impl AnalysisApp {
    /// Attaches a data log or camera file to `recording_id` as another source.
    /// A log added to a video that has none becomes the entry's clock and is
    /// synchronized to the video automatically once both are loaded.
    pub(in crate::analysis_app) fn attach_source(
        &mut self,
        recording_id: RecordingId,
        path: PathBuf,
    ) {
        let source = SourceConfig {
            id: SourceId::new(),
            name: path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("Data")
                .into(),
            adapter: adapter_for(&path).into(),
            path,
            alignment: Default::default(),
            settings: json!({}),
            unknown: Default::default(),
        };
        let is_camera = is_camera_telemetry_source(&source);
        let Some(recording) = self
            .workspace
            .recordings
            .iter_mut()
            .find(|recording| recording.id == recording_id)
        else {
            return;
        };
        let needs_sync = !is_camera
            && recording.video_path.is_some()
            && recording.sources.iter().all(is_camera_telemetry_source);
        recording.sources.push(source.clone());
        self.message = if is_camera {
            "Loading camera telemetry; audio alignment will start automatically.".into()
        } else if needs_sync {
            format!(
                "Loading {}; it will be synchronized to the video automatically.",
                source.name
            )
        } else {
            format!("Loading {}.", source.name)
        };
        if needs_sync {
            self.video_sync
                .recording_mut(recording_id)
                .auto_sync_pending = true;
        }
        self.enqueue(source);
        self.changed();
    }

    /// Asks for a data log and attaches it to the recording.
    pub(in crate::analysis_app) fn attach_data_log_dialog(&mut self, recording_id: RecordingId) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Telemetry", &["xrk", "csv", "insv", "lrv"])
            .pick_file()
        {
            self.attach_source(recording_id, path);
        }
    }

    /// Makes a freshly loaded data log the clock of a recording that so far
    /// only has camera telemetry, so the camera is synchronized *to* the log
    /// and the log's laps and gates stay valid. Returns whether it did.
    pub(in crate::analysis_app) fn promote_logger_to_primary(
        &mut self,
        source_id: SourceId,
        data: &SourceData,
    ) -> bool {
        let Some(recording) = self.workspace.recordings.iter_mut().find(|recording| {
            recording
                .sources
                .iter()
                .any(|source| source.id == source_id)
        }) else {
            return false;
        };
        let is_logger = recording
            .sources
            .iter()
            .any(|source| source.id == source_id && !is_camera_telemetry_source(source));
        let camera_is_clock = recording.sources.iter().any(|source| {
            source.id == recording.primary_source && is_camera_telemetry_source(source)
        });
        if !is_logger || !camera_is_clock {
            return false;
        }
        recording.primary_source = source_id;
        // The camera was accepted as the clock; against a log it is not
        // synchronized until it has been matched to it.
        for source in recording
            .sources
            .iter_mut()
            .filter(|source| is_camera_telemetry_source(source))
        {
            let mut metadata = CameraVideoSyncMetadata::read(&source.settings);
            metadata.applied = false;
            if metadata.camera_minus_video_seconds.is_some() {
                metadata.write(&mut source.settings);
            }
        }
        // Only the camera's own whole-recording interval is replaced; intervals
        // the user made or edited are theirs.
        let camera_interval_only = recording
            .segments
            .iter()
            .all(|segment| segment.estimated && segment.kind == SegmentKind::Unknown);
        if camera_interval_only {
            let gps = gps_points(&data.raw, recording);
            let detected = auto_segments(&data.raw, recording, &gps);
            workflow::replace_segments_preserving_identity(recording, detected);
        }
        self.auto_select_pending = true;
        true
    }

    /// Pairs data logs with camera recordings that were recorded at the same
    /// time and merges each pair into one entry.
    pub(in crate::analysis_app) fn pair_loggers_with_cameras(&mut self) {
        let mut cameras = Vec::new();
        let mut loggers = Vec::new();
        for recording in &self.workspace.recordings {
            if recording.sources.is_empty() {
                continue;
            }
            if recording.sources.iter().all(is_camera_telemetry_source) {
                let span = recording.sources.first().and_then(|source| {
                    let seconds = dataset_span(&self.data.get(&source.id)?.raw)?;
                    WallClockSpan::new(
                        camera_start_from_file_name(&source.path)?,
                        seconds.1 - seconds.0,
                    )
                });
                cameras.extend(span.map(|span| (recording.id, span)));
            } else if recording.video_path.is_none()
                && !recording.sources.iter().any(is_camera_telemetry_source)
            {
                let span = self.data.get(&recording.primary_source).and_then(|data| {
                    let seconds = dataset_span(&data.raw)?;
                    WallClockSpan::new(
                        logger_start_from_metadata(&data.raw.metadata)?,
                        seconds.1 - seconds.0,
                    )
                });
                loggers.extend(span.map(|span| (recording.id, span)));
            }
        }
        for (logger, camera) in unique_matches(&cameras, &loggers) {
            self.merge_logger_into_camera(logger, camera);
        }
    }

    fn merge_logger_into_camera(&mut self, logger_id: RecordingId, camera_id: RecordingId) {
        let Some(logger_index) = self
            .workspace
            .recordings
            .iter()
            .position(|recording| recording.id == logger_id)
        else {
            return;
        };
        let logger = self.workspace.recordings.remove(logger_index);
        let Some(camera) = self
            .workspace
            .recordings
            .iter_mut()
            .find(|recording| recording.id == camera_id)
        else {
            self.workspace.recordings.insert(logger_index, logger);
            return;
        };
        let video_name = camera.name.clone();
        camera.sources.extend(logger.sources);
        self.video_sync.remove_recording(logger_id);
        self.state.selected_recording = Some(camera_id);
        self.retain_valid_selection();
        if let Some(data) = self.data.get(&logger.primary_source).cloned() {
            self.promote_logger_to_primary(logger.primary_source, &data);
        }
        self.video_sync.recording_mut(camera_id).auto_sync_pending = true;
        self.message = format!(
            "{} was recorded during {video_name}; added it to that entry and synchronizing automatically.",
            logger.name
        );
        self.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, seconds: f64) -> WallClockSpan {
        WallClockSpan::new(start, seconds).unwrap()
    }

    #[test]
    fn a_log_pairs_with_the_one_video_recorded_alongside_it() {
        let (video, other_video) = (RecordingId::new(), RecordingId::new());
        let (run, earlier_run, unrelated_run) =
            (RecordingId::new(), RecordingId::new(), RecordingId::new());
        let cameras = [
            (video, span(1_000.0, 190.0)),
            (other_video, span(9_000.0, 190.0)),
        ];
        let loggers = [
            (earlier_run, span(100.0, 130.0)),
            (run, span(1_088.0, 172.0)),
            (unrelated_run, span(5_000.0, 150.0)),
        ];
        assert_eq!(unique_matches(&cameras, &loggers), vec![(run, video)]);
    }

    #[test]
    fn ambiguous_matches_are_left_for_the_user() {
        let video = RecordingId::new();
        let (first, second) = (RecordingId::new(), RecordingId::new());
        // The video straddles two runs.
        assert!(
            unique_matches(
                &[(video, span(1_000.0, 190.0))],
                &[(first, span(900.0, 150.0)), (second, span(1_100.0, 150.0))],
            )
            .is_empty()
        );
        // One run overlaps two videos.
        let (a, b) = (RecordingId::new(), RecordingId::new());
        assert!(
            unique_matches(
                &[(a, span(1_000.0, 100.0)), (b, span(1_110.0, 100.0))],
                &[(first, span(1_050.0, 100.0))],
            )
            .is_empty()
        );
    }
}
