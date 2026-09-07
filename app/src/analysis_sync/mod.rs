//! Recording-scoped state and compatibility helpers for video synchronization.
//!
//! The saved keys remain flat for project-file compatibility. Runtime jobs and
//! drafts are grouped by recording so multiple video/log pairs cannot collide.

use overlay_core::{ChannelRef, CorrelationResult, RecordingId, SourceId};
use overlay_media::AlignmentResult;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

mod audio;
mod calibration;
mod correlation;
#[cfg(test)]
mod tests;
mod ui;

const VIDEO_AUDIO_OFFSET_KEY: &str = "analysis_video_audio_offset_seconds";
const VIDEO_AUDIO_PEAK_KEY: &str = "analysis_video_audio_peak";
const VIDEO_AUDIO_CONFIDENCE_KEY: &str = "analysis_video_audio_confidence";
const VIDEO_ALIGNMENT_APPLIED_KEY: &str = "analysis_video_alignment_applied";

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct CameraVideoSyncMetadata {
    pub(super) camera_minus_video_seconds: Option<f64>,
    pub(super) audio_peak: Option<f64>,
    pub(super) audio_confidence: Option<f64>,
    pub(super) applied: bool,
}

impl CameraVideoSyncMetadata {
    pub(super) fn read(settings: &Value) -> Self {
        Self {
            camera_minus_video_seconds: settings
                .get(VIDEO_AUDIO_OFFSET_KEY)
                .and_then(Value::as_f64),
            audio_peak: settings.get(VIDEO_AUDIO_PEAK_KEY).and_then(Value::as_f64),
            audio_confidence: settings
                .get(VIDEO_AUDIO_CONFIDENCE_KEY)
                .and_then(Value::as_f64),
            applied: settings
                .get(VIDEO_ALIGNMENT_APPLIED_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }

    pub(super) fn audio_is_acceptable(self) -> bool {
        self.camera_minus_video_seconds.is_some()
            && self.audio_peak.is_some_and(|peak| peak >= 0.35)
            && self
                .audio_confidence
                .is_some_and(|confidence| confidence >= 2.0)
    }

    pub(super) fn write(self, settings: &mut Value) {
        let object = settings_object(settings);
        write_optional_number(
            object,
            VIDEO_AUDIO_OFFSET_KEY,
            self.camera_minus_video_seconds,
        );
        write_optional_number(object, VIDEO_AUDIO_PEAK_KEY, self.audio_peak);
        write_optional_number(object, VIDEO_AUDIO_CONFIDENCE_KEY, self.audio_confidence);
        object.insert(VIDEO_ALIGNMENT_APPLIED_KEY.into(), json!(self.applied));
    }

    pub(super) fn clear(settings: &mut Value) {
        let Some(object) = settings.as_object_mut() else {
            return;
        };
        object.remove(VIDEO_AUDIO_OFFSET_KEY);
        object.remove(VIDEO_AUDIO_PEAK_KEY);
        object.remove(VIDEO_AUDIO_CONFIDENCE_KEY);
        object.remove(VIDEO_ALIGNMENT_APPLIED_KEY);
    }
}

fn settings_object(settings: &mut Value) -> &mut Map<String, Value> {
    if !settings.is_object() {
        *settings = json!({});
    }
    settings
        .as_object_mut()
        .expect("settings was made an object")
}

fn write_optional_number(object: &mut Map<String, Value>, key: &str, value: Option<f64>) {
    if let Some(value) = value {
        object.insert(key.into(), json!(value));
    } else {
        object.remove(key);
    }
}

#[derive(Clone, Debug)]
pub(super) struct VideoAlignmentCandidate {
    pub(super) recording_id: RecordingId,
    pub(super) target_source: SourceId,
    pub(super) camera_source: SourceId,
    pub(super) target_channel: ChannelRef,
    pub(super) camera_channel: ChannelRef,
    pub(super) camera_video_offset_seconds: f64,
    pub(super) result: CorrelationResult,
}

#[derive(Clone)]
pub(super) struct VehicleCalibrationDraft {
    pub(super) start: f64,
    pub(super) end: f64,
    pub(super) source_time: bool,
    pub(super) auto_stationary: bool,
    pub(super) forward_axis: usize,
    pub(super) roll_degrees: f64,
    pub(super) pitch_degrees: f64,
    pub(super) yaw_degrees: f64,
    pub(super) low_pass_hz: f64,
}

impl Default for VehicleCalibrationDraft {
    fn default() -> Self {
        Self {
            start: 0.0,
            end: 2.0,
            source_time: false,
            auto_stationary: true,
            forward_axis: 0,
            roll_degrees: 0.0,
            pitch_degrees: 0.0,
            yaw_degrees: 0.0,
            low_pass_hz: 8.0,
        }
    }
}

#[derive(Default)]
pub(super) struct RecordingSyncState {
    pub(super) audio_jobs: HashMap<SourceId, u64>,
    pub(super) audio_results: HashMap<SourceId, Result<AlignmentResult, String>>,
    pub(super) alignment_job: Option<u64>,
    pub(super) alignment_result: Option<Result<VideoAlignmentCandidate, String>>,
    pub(super) channels: Option<(ChannelRef, ChannelRef)>,
    pub(super) calibration_drafts: HashMap<SourceId, VehicleCalibrationDraft>,
}

#[derive(Default)]
pub(super) struct VideoSyncController {
    recordings: HashMap<RecordingId, RecordingSyncState>,
}

impl VideoSyncController {
    pub(super) fn recording(&self, id: RecordingId) -> Option<&RecordingSyncState> {
        self.recordings.get(&id)
    }

    pub(super) fn recording_mut(&mut self, id: RecordingId) -> &mut RecordingSyncState {
        self.recordings.entry(id).or_default()
    }

    pub(super) fn invalidate_estimate(&mut self, id: RecordingId) {
        if let Some(state) = self.recordings.get_mut(&id) {
            state.alignment_job = None;
            state.alignment_result = None;
        }
    }

    pub(super) fn invalidate_audio(&mut self, id: RecordingId, source_id: SourceId) {
        if let Some(state) = self.recordings.get_mut(&id) {
            state.audio_jobs.remove(&source_id);
            state.audio_results.remove(&source_id);
        }
    }

    pub(super) fn remove_source(&mut self, id: RecordingId, source_id: SourceId) {
        if let Some(state) = self.recordings.get_mut(&id) {
            state.audio_jobs.remove(&source_id);
            state.audio_results.remove(&source_id);
            state.calibration_drafts.remove(&source_id);
            if state.channels.as_ref().is_some_and(|(target, camera)| {
                target.source_id == source_id || camera.source_id == source_id
            }) {
                state.channels = None;
            }
            state.alignment_job = None;
            state.alignment_result = None;
        }
    }

    pub(super) fn remove_recording(&mut self, id: RecordingId) {
        self.recordings.remove(&id);
    }

    pub(super) fn clear(&mut self) {
        self.recordings.clear();
    }
}

pub(super) fn resolved_video_alignment_offsets(
    target_recording_offset: f64,
    target_minus_camera_seconds: f64,
    camera_minus_video_seconds: f64,
) -> (f64, f64) {
    let camera_recording_offset = target_recording_offset - target_minus_camera_seconds;
    let video_recording_offset = camera_recording_offset - camera_minus_video_seconds;
    (camera_recording_offset, video_recording_offset)
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn sync_metadata_round_trips_through_legacy_project_keys() {
        let expected = CameraVideoSyncMetadata {
            camera_minus_video_seconds: Some(1.25),
            audio_peak: Some(0.8),
            audio_confidence: Some(3.5),
            applied: true,
        };
        let mut settings = json!({ "unknown": 7 });
        expected.write(&mut settings);
        assert_eq!(CameraVideoSyncMetadata::read(&settings), expected);
        assert_eq!(settings["unknown"], 7);

        CameraVideoSyncMetadata::clear(&mut settings);
        assert_eq!(CameraVideoSyncMetadata::read(&settings), Default::default());
        assert_eq!(settings["unknown"], 7);
    }

    #[test]
    fn runtime_state_is_isolated_by_recording() {
        let first = RecordingId::new();
        let second = RecordingId::new();
        let source = SourceId::new();
        let mut controller = VideoSyncController::default();
        controller
            .recording_mut(first)
            .audio_jobs
            .insert(source, 11);
        controller
            .recording_mut(second)
            .audio_jobs
            .insert(source, 22);

        controller.invalidate_audio(first, source);

        assert!(controller.recording(first).unwrap().audio_jobs.is_empty());
        assert_eq!(
            controller.recording(second).unwrap().audio_jobs[&source],
            22
        );
    }
}
