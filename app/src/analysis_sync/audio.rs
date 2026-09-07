//! Audio alignment between camera telemetry media and exported video.
use super::super::*;
use super::*;

impl AnalysisApp {
    pub(in crate::analysis_app) fn start_pending_video_audio_sync(
        &mut self,
        tools: Option<&FfmpegTools>,
    ) {
        let Some(tools) = tools.cloned() else {
            return;
        };
        let pending = self
            .workspace
            .recordings
            .iter()
            .filter_map(|recording| {
                let video = recording.video_path.clone()?;
                recording.sources.iter().find_map(|source| {
                    let sync_state = self.video_sync.recording(recording.id);
                    (is_camera_telemetry_source(source)
                        && self.data.contains_key(&source.id)
                        && CameraVideoSyncMetadata::read(&source.settings)
                            .camera_minus_video_seconds
                            .is_none()
                        && sync_state
                            .is_none_or(|state| !state.audio_jobs.contains_key(&source.id))
                        && sync_state
                            .is_none_or(|state| !state.audio_results.contains_key(&source.id)))
                    .then(|| (recording.id, source.clone(), video.clone()))
                })
            })
            .collect::<Vec<_>>();
        for (recording_id, source, video) in pending {
            self.load_serial = self.load_serial.wrapping_add(1);
            let serial = self.load_serial;
            self.video_sync
                .recording_mut(recording_id)
                .audio_jobs
                .insert(source.id, serial);
            let tx = self.tx.clone();
            let tools = tools.clone();
            std::thread::spawn(move || {
                let result = (|| -> Result<AlignmentResult, String> {
                    let source_audio = overlay_media::extract_mono_pcm(&tools, &source.path, 2_000)
                        .map_err(|error| error.to_string())?;
                    let video_audio = overlay_media::extract_mono_pcm(&tools, &video, 2_000)
                        .map_err(|error| error.to_string())?;
                    if source_audio.is_empty() || video_audio.is_empty() {
                        return Err(
                            "Both the camera recording and exported video need audio".into()
                        );
                    }
                    Ok(overlay_media::align_audio(
                        &source_audio,
                        &video_audio,
                        2_000,
                    ))
                })();
                let _ = tx.send(Event::VideoAudioAligned(
                    serial,
                    recording_id,
                    source.id,
                    result,
                ));
            });
        }
    }

    pub(in crate::analysis_app) fn handle_video_audio_aligned(
        &mut self,
        serial: u64,
        recording_id: RecordingId,
        source_id: SourceId,
        result: Result<AlignmentResult, String>,
    ) {
        if self
            .video_sync
            .recording(recording_id)
            .and_then(|state| state.audio_jobs.get(&source_id))
            != Some(&serial)
        {
            return;
        }
        self.video_sync
            .recording_mut(recording_id)
            .audio_jobs
            .remove(&source_id);
        if !self.workspace.recordings.iter().any(|recording| {
            recording.id == recording_id
                && recording
                    .sources
                    .iter()
                    .any(|source| source.id == source_id)
        }) {
            return;
        }
        self.video_sync
            .recording_mut(recording_id)
            .audio_results
            .insert(source_id, result.clone());
        match result {
            Ok(alignment) => {
                let camera_video_offset = -alignment.offset_seconds;
                if let Some(recording) = self
                    .workspace
                    .recordings
                    .iter_mut()
                    .find(|recording| recording.id == recording_id)
                    && let Some(source) = recording
                        .sources
                        .iter_mut()
                        .find(|source| source.id == source_id)
                {
                    let mut metadata = CameraVideoSyncMetadata::read(&source.settings);
                    metadata.camera_minus_video_seconds = Some(camera_video_offset);
                    metadata.audio_peak = Some(alignment.normalized_peak);
                    metadata.audio_confidence = Some(alignment.confidence.min(999.0));
                    if recording.primary_source == source_id {
                        recording.video_offset_seconds =
                            source.alignment.offset_seconds - camera_video_offset;
                        metadata.applied = alignment.auto_acceptable();
                    }
                    metadata.write(&mut source.settings);
                }
                self.message = format!(
                    "Camera audio aligned to the exported video (peak {:.2}, confidence {:.2}){}",
                    alignment.normalized_peak,
                    alignment.confidence,
                    if alignment.auto_acceptable() {
                        ""
                    } else {
                        "; inspect before applying telemetry correlation"
                    }
                );
                self.changed();
            }
            Err(error) => {
                self.message = format!("Camera/video audio alignment failed: {error}");
            }
        }
    }
}
