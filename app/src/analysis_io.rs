//! Analysis file import, background work, and media attachment coordination.

use super::{
    AnalysisApp, CameraVideoSyncMetadata, Prepared, SourceData, VideoAlignmentCandidate, XMode,
    adapter_for, automatic_alignment_complete, automatic_default_mode, is_camera_telemetry_source,
    is_video,
};
use eframe::egui;
use overlay_core::{
    AdapterRegistry, ProjectV1, Recording, RecordingId, SourceConfig, SourceId, auto_segments,
    gps_points, prepare_comparison, prepare_loaded_dataset, recording_from_project,
};
use overlay_media::{AlignmentResult, VideoMetadata};
use serde_json::{Value, json};
use std::{collections::HashSet, path::PathBuf, sync::Arc};

pub(super) enum Event {
    Loaded(u64, SourceId, Result<SourceData, String>),
    Prepared(u64, Prepared),
    Probed(PathBuf, Result<VideoMetadata, String>),
    VideoAudioAligned(u64, RecordingId, SourceId, Result<AlignmentResult, String>),
    VideoAlignmentEstimated(u64, RecordingId, Result<VideoAlignmentCandidate, String>),
}

impl AnalysisApp {
    pub fn import_project(&mut self, project: ProjectV1) {
        let ids: Vec<_> = project.sources.iter().map(|s| s.id).collect();
        let existing = self
            .workspace
            .recordings
            .iter()
            .position(|r| Some(r.id) == self.active_overlay)
            .or_else(|| {
                self.workspace
                    .recordings
                    .iter()
                    .position(|r| r.sources.iter().any(|s| ids.contains(&s.id)))
            });
        let video_offset = existing
            .map(|index| {
                let old = &self.workspace.recordings[index];
                let old_primary_offset = old
                    .sources
                    .iter()
                    .find(|source| source.id == old.primary_source)
                    .map(|source| source.alignment.offset_seconds);
                let incoming_primary_offset = project
                    .sources
                    .iter()
                    .find(|source| source.id == old.primary_source)
                    .map(|source| source.alignment.offset_seconds);
                old_primary_offset
                    .zip(incoming_primary_offset)
                    .map_or(old.video_offset_seconds, |(recording, video)| {
                        recording - video
                    })
            })
            .unwrap_or(0.0);
        let mut recording = recording_from_project(project, "Overlay recording", video_offset);
        if let Some(i) = existing {
            let old = &self.workspace.recordings[i];
            recording.id = old.id;
            recording.name = old.name.clone();
            recording.segments = old.segments.clone();
            recording.unknown = old.unknown.clone();
            if recording.sources.iter().any(|s| s.id == old.primary_source) {
                recording.primary_source = old.primary_source;
            }
            self.workspace.recordings[i] = recording.clone();
        } else {
            self.workspace.recordings.push(recording.clone());
            self.auto_select_pending = true;
        }
        self.state.selected_recording = Some(recording.id);
        let ids: HashSet<_> = self
            .workspace
            .recordings
            .iter()
            .flat_map(|r| r.sources.iter().map(|s| s.id))
            .collect();
        self.data.retain(|id, _| ids.contains(id));
        for source in recording.sources {
            self.enqueue(source);
        }
        self.changed();
    }
    pub(super) fn enqueue(&mut self, source: SourceConfig) {
        for recording_id in self
            .workspace
            .recordings
            .iter()
            .filter(|recording| recording.sources.iter().any(|item| item.id == source.id))
            .map(|recording| recording.id)
            .collect::<Vec<_>>()
        {
            self.video_sync.invalidate_estimate(recording_id);
        }
        self.load_serial = self.load_serial.wrapping_add(1);
        let serial = self.load_serial;
        self.loading.insert(source.id, serial);
        let tx = self.tx.clone();
        let calibration = if is_camera_telemetry_source(&source) {
            self.workspace
                .recordings
                .iter()
                .find(|r| r.sources.iter().any(|s| s.id == source.id))
                .and_then(|r| r.overlay_snapshot.as_ref())
                .filter(|p| p.camera_calibration.notes.is_some())
                .map(|p| p.camera_calibration.clone())
        } else {
            None
        };
        std::thread::spawn(move || {
            let result = AdapterRegistry::with_builtins()
                .load(&source.adapter, source.id, &source.path, &source.settings)
                .map_err(|e| e.to_string())
                .and_then(|raw| {
                    let mut processed = raw.clone();
                    let cutoff = source
                        .settings
                        .get("low_pass_enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                        .then(|| {
                            source
                                .settings
                                .get("low_pass_hz")
                                .and_then(Value::as_f64)
                                .unwrap_or(8.0)
                        });
                    prepare_loaded_dataset(&mut processed, cutoff, calibration.as_ref())?;
                    Ok(SourceData {
                        raw: Arc::new(raw),
                        processed: Arc::new(processed),
                    })
                });
            let _ = tx.send(Event::Loaded(serial, source.id, result));
        });
    }
    fn add_telemetry(&mut self, path: PathBuf) -> RecordingId {
        let name = path
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("Recording")
            .to_owned();
        let source = SourceConfig {
            id: SourceId::new(),
            name: name.clone(),
            adapter: adapter_for(&path).into(),
            path,
            alignment: Default::default(),
            settings: json!({}),
            unknown: Default::default(),
        };
        let recording = Recording {
            id: RecordingId::new(),
            name,
            sources: vec![source.clone()],
            primary_source: source.id,
            video_path: None,
            video_offset_seconds: 0.0,
            segments: vec![],
            overlay_snapshot: None,
            unknown: Default::default(),
        };
        let id = recording.id;
        self.workspace.recordings.push(recording);
        self.auto_select_pending = true;
        self.state.selected_recording = Some(id);
        self.enqueue(source);
        self.changed();
        id
    }
    pub(super) fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let (videos, other): (Vec<_>, Vec<_>) = paths.into_iter().partition(|p| is_video(p));
        let mut added = vec![];
        for path in other {
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                self.open_path(path);
                continue;
            }
            if !matches!(
                path.extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .as_str(),
                "xrk" | "csv" | "insv" | "lrv"
            ) {
                self.errors
                    .push(format!("Unsupported file: {}", path.display()));
                continue;
            }
            let id = self.add_telemetry(path);
            added.push(id);
        }
        if added.len() > 1 {
            self.select_imports_pending.extend(added.iter().copied());
        }
        if videos.len() == 1 && (added.len() == 1 || added.is_empty()) {
            if let Some(id) = added.first().copied().or(self.state.selected_recording) {
                self.attach_video(id, videos[0].clone());
            } else {
                self.message = "Import telemetry first, then attach its video.".into();
            }
        } else if !videos.is_empty() {
            if !self.workspace.settings.is_object() {
                self.workspace.settings = json!({});
            }
            self.workspace.settings["unassigned_videos"] = json!(videos);
            self.message = "Videos need pairing: choose one in the recording browser.".into();
        }
    }
    pub(super) fn attach_video(&mut self, id: RecordingId, path: PathBuf) {
        let mut camera_sources = Vec::new();
        if let Some(r) = self.workspace.recordings.iter_mut().find(|r| r.id == id) {
            if r.video_path.as_ref() != Some(&path) {
                for source in r
                    .sources
                    .iter_mut()
                    .filter(|source| is_camera_telemetry_source(source))
                {
                    camera_sources.push(source.id);
                    CameraVideoSyncMetadata::clear(&mut source.settings);
                }
            }
            r.video_path = Some(path);
        }
        for source_id in camera_sources {
            self.video_sync.invalidate_audio(id, source_id);
        }
        self.video_sync.invalidate_estimate(id);
        self.changed();
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Loaded(serial, id, result) => {
                    if self.loading.get(&id) != Some(&serial) {
                        continue;
                    }
                    self.loading.remove(&id);
                    if !self
                        .workspace
                        .recordings
                        .iter()
                        .any(|r| r.sources.iter().any(|s| s.id == id))
                    {
                        continue;
                    }
                    match result {
                        Ok(data) => {
                            if let Some(recording) = self
                                .workspace
                                .recordings
                                .iter_mut()
                                .find(|r| r.primary_source == id)
                                && recording.segments.is_empty()
                            {
                                let gps = gps_points(&data.raw, recording);
                                recording.segments = auto_segments(&data.raw, recording, &gps);
                            }
                            self.data.insert(id, data);
                            self.changed();
                        }
                        Err(e) => self.errors.push(e),
                    }
                }
                Event::Prepared(revision, prepared) => {
                    self.preparing = false;
                    if revision == self.revision {
                        if self.auto_mode_pending && prepared.automatic_time_alignment {
                            let aligned = automatic_alignment_complete(
                                &prepared,
                                self.workspace.reference.as_ref(),
                            );
                            self.state.mode = automatic_default_mode(
                                &prepared,
                                self.workspace.reference.as_ref(),
                            );
                            if aligned {
                                self.message = "Compared recordings using automatic motion-trace time alignment. Inspect the reported coefficients; traveled distance remains available.".into();
                            } else if self.state.mode == XMode::Distance {
                                self.message = "One or more recordings could not be time-aligned confidently; using traveled distance as the safer default.".into();
                            } else {
                                self.message = "One or more recordings could not be time-aligned confidently and do not share a distance basis; using segment-relative time.".into();
                            }
                            self.auto_mode_pending = false;
                        }
                        self.prepared = prepared;
                        self.prepared_revision = revision;
                        self.plot_cache.clear();
                        self.scatter_cache.clear();
                        self.map_cache.clear();
                        ctx.request_repaint();
                    }
                }
                Event::Probed(path, result) => {
                    self.probing.remove(&path);
                    self.metadata.insert(path, result);
                }
                Event::VideoAudioAligned(serial, recording_id, source_id, result) => {
                    self.handle_video_audio_aligned(serial, recording_id, source_id, result);
                }
                Event::VideoAlignmentEstimated(serial, recording_id, result) => {
                    self.handle_video_alignment_estimated(serial, recording_id, result);
                }
            }
        }
        if self.loading.is_empty() && self.auto_select_pending {
            self.auto_select_pending = false;
            self.default_selection();
            let imported = std::mem::take(&mut self.select_imports_pending);
            self.select_segments_for_recordings(&imported);
            self.auto_mode_pending = self.workspace.course.gates.is_empty()
                && self.automatic_mode
                && self
                    .state
                    .selection
                    .iter()
                    .map(|key| key.recording_id)
                    .collect::<HashSet<_>>()
                    .len()
                    > 1;
        }
        if !self.preparing && self.prepared_revision != self.revision {
            let tx = self.tx.clone();
            let revision = self.revision;
            let workspace = self.workspace.clone();
            let selection = self.state.selection.clone();
            let data = self.data.clone();
            self.preparing = true;
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let prepared = prepare_comparison(&workspace, &selection, &data);
                let _ = tx.send(Event::Prepared(revision, prepared));
                ctx.request_repaint();
            });
        }
    }
}
