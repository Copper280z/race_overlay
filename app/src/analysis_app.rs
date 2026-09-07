//! Telemetry-first analysis workspace. The existing overlay editor is a separate mode.
use crate::analysis_maps::{MapColorMode, MapPanel, MapSettings, MapTrace};
use crossbeam_channel::{Receiver, Sender, unbounded};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use overlay_core::*;
use overlay_media::{AlignmentResult, AnalysisPreview, FfmpegTools, VideoMetadata};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
#[path = "analysis_recordings.rs"]
mod recordings;
#[path = "analysis_sync/mod.rs"]
mod sync;
#[cfg(test)]
#[path = "analysis_tests.rs"]
mod tests;
use sync::{CameraVideoSyncMetadata, VideoAlignmentCandidate, VideoSyncController};
#[path = "analysis_views.rs"]
mod views;
#[path = "analysis_workflow.rs"]
mod workflow;
const COLORS: [egui::Color32; 8] = [
    egui::Color32::from_rgb(65, 170, 255),
    egui::Color32::from_rgb(255, 156, 66),
    egui::Color32::from_rgb(70, 210, 145),
    egui::Color32::from_rgb(210, 110, 255),
    egui::Color32::from_rgb(250, 215, 60),
    egui::Color32::from_rgb(70, 215, 220),
    egui::Color32::from_rgb(250, 105, 140),
    egui::Color32::from_rgb(180, 190, 255),
];
const DELTA_CHANNEL: &str = "delta_time";
const DELTA_LABEL: &str = "Time delta — positive is slower";
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize, Debug)]
enum XMode {
    Time,
    Distance,
    #[default]
    Course,
}
impl XMode {
    fn label(self) -> &'static str {
        match self {
            Self::Time => "Elapsed time",
            Self::Distance => "Traveled distance",
            Self::Course => "Course position",
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct PlotOptions {
    channels: Vec<String>,
    filter: Option<f64>,
    units: BTreeMap<String, Unit>,
    bindings: BTreeMap<String, ChannelRef>,
    delta: bool,
    show_legend: bool,
    legend_labels: BTreeMap<String, String>,
    #[serde(flatten)]
    unknown: BTreeMap<String, Value>,
}
impl Default for PlotOptions {
    fn default() -> Self {
        Self {
            channels: vec!["gps_speed".into(), "rpm".into()],
            filter: None,
            units: BTreeMap::new(),
            bindings: BTreeMap::new(),
            delta: false,
            show_legend: true,
            legend_labels: Default::default(),
            unknown: Default::default(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct ScatterOptions {
    x_channel: String,
    y_channel: String,
    z_channel: Option<String>,
    filter: Option<f64>,
    units: BTreeMap<String, Unit>,
    bindings: BTreeMap<String, ChannelRef>,
    show_legend: bool,
    legend_labels: BTreeMap<String, String>,
    #[serde(flatten)]
    unknown: BTreeMap<String, Value>,
}
impl Default for ScatterOptions {
    fn default() -> Self {
        Self {
            x_channel: "gps_lateral_acceleration".into(),
            y_channel: "gps_inline_acceleration".into(),
            z_channel: None,
            filter: None,
            units: Default::default(),
            bindings: Default::default(),
            show_legend: true,
            legend_labels: Default::default(),
            unknown: Default::default(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
struct VideoOptions {
    slot: usize,
    segment: Option<SegmentRef>,
    linked: bool,
    time: f64,
    #[serde(default, flatten)]
    unknown: BTreeMap<String, Value>,
}
#[derive(Clone, Serialize, Deserialize)]
enum TabKind {
    Browser,
    Plot(PlotOptions),
    Scatter(ScatterOptions),
    Video(VideoOptions),
    Map {
        channel: String,
        settings: Box<MapSettings>,
    },
    Stats,
    Setup,
}
#[derive(Clone, Serialize, Deserialize)]
struct Tab {
    id: u64,
    kind: TabKind,
    #[serde(default, flatten)]
    unknown: BTreeMap<String, Value>,
}
impl Tab {
    fn title(&self) -> String {
        match &self.kind {
            TabKind::Browser => "Recordings & laps".into(),
            TabKind::Plot(p) => {
                if p.delta {
                    "Time gain / loss".into()
                } else {
                    "Channel plot".into()
                }
            }
            TabKind::Scatter(_) => "X/Y scatter plot".into(),
            TabKind::Video(v) => format!("Video {}", v.slot + 1),
            TabKind::Map { settings, .. } => {
                if settings.actual_gps {
                    "GPS imagery".into()
                } else {
                    "Channel course map".into()
                }
            }
            TabKind::Stats => "Values & statistics".into(),
            TabKind::Setup => "Timing & course setup".into(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
struct UiState {
    selection: Vec<SegmentRef>,
    mode: XMode,
    cursor: f64,
    range: Option<[f64; 2]>,
    dock: Value,
    selected_recording: Option<RecordingId>,
    #[serde(flatten)]
    unknown: BTreeMap<String, Value>,
}
pub enum AnalysisAction {
    OpenOverlay(ProjectV1),
}
#[derive(Clone)]
struct SourceData {
    raw: Arc<TelemetryDataset>,
    processed: Arc<TelemetryDataset>,
}
#[cfg(test)]
impl SourceData {
    fn from_dataset(dataset: Arc<TelemetryDataset>) -> Self {
        Self {
            raw: dataset.clone(),
            processed: dataset,
        }
    }
}
#[derive(Clone, Debug)]
struct BestEffortTimeAlignment {
    /// Added to segment-relative time to place it on the reference time base.
    offset_seconds: f64,
    coefficient: Option<f64>,
    channel: &'static str,
    distance_offset_meters: Option<f64>,
}
#[derive(Clone)]
struct PreparedRun {
    key: SegmentRef,
    name: String,
    color: egui::Color32,
    start: f64,
    end: f64,
    gps: Vec<GpsPoint>,
    distance: Vec<ProgressSample>,
    progress: Vec<ProgressSample>,
    time_alignment: Option<BestEffortTimeAlignment>,
}
struct Prepared {
    runs: Vec<PreparedRun>,
    course: Option<ReferenceCourse>,
    automatic_time_alignment: bool,
}
fn automatic_alignment_complete(prepared: &Prepared, reference: Option<&SegmentRef>) -> bool {
    let candidates = prepared
        .runs
        .iter()
        .filter(|run| Some(&run.key) != reference)
        .collect::<Vec<_>>();
    !candidates.is_empty() && candidates.iter().all(|run| run.time_alignment.is_some())
}

fn automatic_default_mode(prepared: &Prepared, reference: Option<&SegmentRef>) -> XMode {
    if automatic_alignment_complete(prepared, reference) {
        XMode::Time
    } else if prepared.runs.iter().all(|run| run.distance.len() >= 2) {
        XMode::Distance
    } else {
        XMode::Time
    }
}
enum Event {
    Loaded(u64, SourceId, Result<SourceData, String>),
    Prepared(u64, Prepared),
    Probed(PathBuf, Result<VideoMetadata, String>),
    VideoAudioAligned(u64, RecordingId, SourceId, Result<AlignmentResult, String>),
    VideoAlignmentEstimated(u64, RecordingId, Result<VideoAlignmentCandidate, String>),
}
struct VideoRuntime {
    path: PathBuf,
    decoder: AnalysisPreview,
    texture: Option<egui::TextureHandle>,
    requested: Option<(f64, u32, u32)>,
    error: Option<String>,
}
struct PlotTrace {
    segment: SegmentRef,
    name: String,
    color: egui::Color32,
    points: Vec<Vec<[f64; 2]>>,
    unit: Unit,
}
struct ScatterTrace {
    segment: SegmentRef,
    name: String,
    color: egui::Color32,
    points: Vec<[f64; 3]>,
}
pub struct AnalysisApp {
    pub workspace: AnalysisWorkspace,
    state: UiState,
    dock: DockState<Tab>,
    next_tab: u64,
    path: Option<PathBuf>,
    dirty: bool,
    data: HashMap<SourceId, SourceData>,
    loading: HashMap<SourceId, u64>,
    load_serial: u64,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    message: String,
    errors: Vec<String>,
    actions: Vec<AnalysisAction>,
    revision: u64,
    prepared_revision: u64,
    preparing: bool,
    prepared: Prepared,
    plot_cache: HashMap<String, Arc<Vec<PlotTrace>>>,
    scatter_cache: HashMap<String, Arc<Vec<ScatterTrace>>>,
    map_cache: HashMap<String, Vec<MapTrace>>,
    maps: HashMap<u64, MapPanel>,
    videos: HashMap<u64, VideoRuntime>,
    visible_videos: HashSet<u64>,
    metadata: HashMap<PathBuf, Result<VideoMetadata, String>>,
    probing: HashSet<PathBuf>,
    video_sync: VideoSyncController,
    playing: bool,
    last_frame: Instant,
    active_overlay: Option<RecordingId>,
    remove_recording: Option<RecordingId>,
    auto_select_pending: bool,
    select_imports_pending: HashSet<RecordingId>,
    auto_mode_pending: bool,
    automatic_mode: bool,
}
impl AnalysisApp {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        let mut app = Self {
            workspace: Default::default(),
            state: Default::default(),
            dock: DockState::new(vec![]),
            next_tab: 1,
            path: None,
            dirty: false,
            data: HashMap::new(),
            loading: HashMap::new(),
            load_serial: 0,
            tx,
            rx,
            message: "Drop XRK/CSV files to compare. Video and saved tracks are optional.".into(),
            errors: vec![],
            actions: vec![],
            revision: 1,
            prepared_revision: 0,
            preparing: false,
            prepared: Prepared {
                runs: vec![],
                course: None,
                automatic_time_alignment: false,
            },
            plot_cache: HashMap::new(),
            scatter_cache: HashMap::new(),
            map_cache: HashMap::new(),
            maps: HashMap::new(),
            videos: HashMap::new(),
            visible_videos: HashSet::new(),
            metadata: HashMap::new(),
            probing: HashSet::new(),
            video_sync: VideoSyncController::default(),
            playing: false,
            last_frame: Instant::now(),
            active_overlay: None,
            remove_recording: None,
            auto_select_pending: false,
            select_imports_pending: HashSet::new(),
            auto_mode_pending: false,
            automatic_mode: true,
        };
        app.layout(0);
        app.dirty = false;
        app
    }
    fn tab(&mut self, kind: TabKind) -> Tab {
        let id = self.next_tab;
        self.next_tab += 1;
        Tab {
            id,
            kind,
            unknown: Default::default(),
        }
    }
    fn layout(&mut self, preset: usize) {
        let plot = self.tab(TabKind::Plot(Default::default()));
        let browser = self.tab(TabKind::Browser);
        let setup = self.tab(TabKind::Setup);
        self.dock = DockState::new(vec![plot]);
        let [main, _] =
            self.dock
                .main_surface_mut()
                .split_left(NodeIndex::root(), 0.78, vec![browser, setup]);
        let stats = self.tab(TabKind::Stats);
        let map = self.tab(TabKind::Map {
            channel: "gps_speed".into(),
            settings: Default::default(),
        });
        let imagery = self.tab(TabKind::Map {
            channel: "gps_speed".into(),
            settings: Box::new(MapSettings {
                actual_gps: true,
                ..Default::default()
            }),
        });
        let [upper, _] =
            self.dock
                .main_surface_mut()
                .split_below(main, 0.64, vec![map, imagery, stats]);
        if preset != 1 {
            let left = self.tab(TabKind::Video(VideoOptions {
                slot: 0,
                segment: None,
                linked: true,
                time: 0.0,
                unknown: Default::default(),
            }));
            let right = self.tab(TabKind::Video(VideoOptions {
                slot: 1,
                segment: None,
                linked: true,
                time: 0.0,
                unknown: Default::default(),
            }));
            let [_, video] = self.dock.main_surface_mut().split_above(
                upper,
                if preset == 2 { 0.55 } else { 0.38 },
                vec![left],
            );
            self.dock
                .main_surface_mut()
                .split_right(video, 0.5, vec![right]);
        }
        self.videos.clear();
        self.dirty = true;
    }
    fn changed(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
        self.plot_cache.clear();
        self.scatter_cache.clear();
        self.map_cache.clear();
    }
    pub fn pause(&mut self) {
        self.playing = false;
        self.videos.clear();
    }
    pub fn step_reference(&mut self, direction: i32) {
        let Some(run) = self.reference_run() else {
            return;
        };
        let Some(recording) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == run.key.recording_id)
        else {
            return;
        };
        let duration = (run.end - run.start).max(0.0);
        let current_recording_time = run.start + self.state.cursor;
        let video_step = recording
            .video_path
            .as_ref()
            .and_then(|path| self.metadata.get(path))
            .and_then(|metadata| metadata.as_ref().ok())
            .and_then(VideoMetadata::fps)
            .filter(|fps| fps.is_finite() && *fps > 0.0)
            .map(|fps| 1.0 / fps);
        let next_elapsed = video_step
            .map(|step| self.state.cursor + f64::from(direction) * step)
            .or_else(|| {
                let source = recording
                    .sources
                    .iter()
                    .find(|source| source.id == recording.primary_source)?;
                let dataset = self.data.get(&source.id)?;
                let source_time = current_recording_time + source.alignment.offset_seconds;
                let adjacent = dataset.processed.channels.values().filter_map(|channel| {
                    let samples = &channel.series.samples;
                    if direction < 0 {
                        let index =
                            samples.partition_point(|sample| sample.time < source_time - 1e-9);
                        index.checked_sub(1).and_then(|index| samples.get(index))
                    } else {
                        let index =
                            samples.partition_point(|sample| sample.time <= source_time + 1e-9);
                        samples.get(index)
                    }
                });
                let sample = if direction < 0 {
                    adjacent.max_by(|left, right| left.time.total_cmp(&right.time))
                } else {
                    adjacent.min_by(|left, right| left.time.total_cmp(&right.time))
                }?;
                Some(sample.time - source.alignment.offset_seconds - run.start)
            });
        if let Some(elapsed) = next_elapsed {
            self.playing = false;
            self.state.cursor = elapsed.clamp(0.0, duration);
        }
    }
    pub fn take_actions(&mut self) -> Vec<AnalysisAction> {
        std::mem::take(&mut self.actions)
    }
    pub fn report_overlay_open_error(&mut self, error: impl Into<String>) {
        self.message = format!("Could not open Overlay: {}", error.into());
    }
    pub fn clear_overlay_link(&mut self) {
        self.active_overlay = None;
    }
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
    fn enqueue(&mut self, source: SourceConfig) {
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
    fn add_paths(&mut self, paths: Vec<PathBuf>) {
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
    fn attach_video(&mut self, id: RecordingId, path: PathBuf) {
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
    fn poll(&mut self, ctx: &egui::Context) {
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
    fn default_selection(&mut self) {
        self.state
            .selection
            .retain(|key| segment(&self.workspace, key).is_some());
        if self
            .workspace
            .reference
            .as_ref()
            .is_none_or(|r| segment(&self.workspace, r).is_none())
        {
            self.workspace.reference = self.workspace.recordings.iter().find_map(|r| {
                r.segments
                    .iter()
                    .filter(|s| s.competitive)
                    .min_by(|a, b| a.duration().total_cmp(&b.duration()))
                    .map(|s| SegmentRef {
                        recording_id: r.id,
                        segment_id: s.id,
                    })
            });
        }
        if let Some(reference) = &self.workspace.reference {
            if !self.state.selection.contains(reference) {
                self.state.selection.insert(0, reference.clone());
            }
            let latest = self.workspace.recordings.iter().rev().find_map(|r| {
                r.segments
                    .iter()
                    .rev()
                    .find(|s| s.competitive && s.id != reference.segment_id)
                    .map(|s| SegmentRef {
                        recording_id: r.id,
                        segment_id: s.id,
                    })
            });
            if let Some(latest) = latest
                && !self.state.selection.contains(&latest)
            {
                if self.state.selection.len() == 2 {
                    self.state.selection[1] = latest;
                } else {
                    self.state.selection.push(latest);
                }
            }
        }
        self.changed();
    }
    fn retain_valid_selection(&mut self) {
        self.state
            .selection
            .retain(|key| segment(&self.workspace, key).is_some());
        if self
            .workspace
            .reference
            .as_ref()
            .is_some_and(|key| segment(&self.workspace, key).is_none())
        {
            self.workspace.reference = self.state.selection.first().cloned();
        }
        if self
            .workspace
            .selected
            .as_ref()
            .is_some_and(|key| segment(&self.workspace, key).is_none())
        {
            self.workspace.selected = None;
        }
    }
    fn all_segment_keys(&self) -> Vec<SegmentRef> {
        self.workspace
            .recordings
            .iter()
            .flat_map(|recording| {
                recording.segments.iter().map(|segment| SegmentRef {
                    recording_id: recording.id,
                    segment_id: segment.id,
                })
            })
            .collect()
    }
    fn select_segments_for_recordings(&mut self, recording_ids: &HashSet<RecordingId>) {
        for key in self
            .all_segment_keys()
            .into_iter()
            .filter(|key| recording_ids.contains(&key.recording_id))
        {
            if !self.state.selection.contains(&key) {
                self.state.selection.push(key);
            }
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, tools: Option<&FfmpegTools>, units: UnitSystem) {
        if !self.workspace.settings.is_object() {
            self.workspace.settings = json!({});
        }
        self.poll(ui.ctx());
        self.start_pending_video_audio_sync(tools);
        let dt = self.last_frame.elapsed().as_secs_f64().min(0.1);
        self.last_frame = Instant::now();
        if self.playing {
            self.state.cursor += dt;
            if self.state.cursor > self.reference_run().map_or(0.0, |r| r.end - r.start) {
                self.playing = false;
            }
        }
        let drops = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect::<Vec<_>>()
        });
        if !drops.is_empty() {
            self.add_paths(drops);
        }
        self.toolbar(ui);
        if self.preparing || !self.loading.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.small(format!(
                    "Preparing comparison · {} source(s) loading",
                    self.loading.len()
                ));
            });
        }
        if !self.errors.is_empty() {
            ui.collapsing(format!("{} issue(s)", self.errors.len()), |ui| {
                for e in &self.errors {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                }
                if ui.button("Dismiss").clicked() {
                    self.errors.clear();
                }
            });
        }
        ui.small(&self.message);
        self.visible_videos.clear();
        let mut dock = std::mem::replace(&mut self.dock, DockState::new(vec![]));
        struct Viewer<'a> {
            app: &'a mut AnalysisApp,
            tools: Option<&'a FfmpegTools>,
            units: UnitSystem,
        }
        impl TabViewer for Viewer<'_> {
            type Tab = Tab;
            fn id(&mut self, t: &mut Tab) -> egui::Id {
                egui::Id::new(t.id)
            }
            fn title(&mut self, t: &mut Tab) -> egui::WidgetText {
                t.title().into()
            }
            fn ui(&mut self, ui: &mut egui::Ui, t: &mut Tab) {
                match &mut t.kind {
                    TabKind::Browser => self.app.browser(ui, self.tools),
                    TabKind::Setup => self.app.setup(ui),
                    TabKind::Stats => self.app.statistics(ui, self.units),
                    TabKind::Plot(p) => self.app.plot(ui, t.id, p, self.units),
                    TabKind::Scatter(options) => self.app.scatter(ui, t.id, options, self.units),
                    TabKind::Video(v) => self.app.video(ui, t.id, v, self.tools),
                    TabKind::Map { channel, settings } => {
                        self.app.map(ui, t.id, channel, settings, self.units)
                    }
                }
            }
        }
        DockArea::new(&mut dock).show_inside(
            ui,
            &mut Viewer {
                app: self,
                tools,
                units,
            },
        );
        self.dock = dock;
        let open_ids: HashSet<_> = self.dock.iter_all_tabs().map(|(_, t)| t.id).collect();
        self.maps.retain(|id, _| open_ids.contains(id));
        let cache_is_open = |key: &String| {
            key.split(':')
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .is_some_and(|id| open_ids.contains(&id))
        };
        self.plot_cache.retain(|key, _| cache_is_open(key));
        self.scatter_cache.retain(|key, _| cache_is_open(key));
        self.map_cache.retain(|key, _| cache_is_open(key));
        self.videos.retain(|id, _| self.visible_videos.contains(id));
        if self.playing {
            ui.ctx().request_repaint();
        }
        self.remove_dialog(ui.ctx());
    }
    fn reference_run(&self) -> Option<&PreparedRun> {
        self.workspace
            .reference
            .as_ref()
            .and_then(|key| self.prepared.runs.iter().find(|r| &r.key == key))
    }
    fn effective_mode(&self) -> XMode {
        if self.state.mode == XMode::Course && self.prepared.course.is_none() {
            XMode::Time
        } else {
            self.state.mode
        }
    }
    fn x_at_time(&self, run: &PreparedRun, t: f64) -> Option<f64> {
        match self.effective_mode() {
            XMode::Time => Some(
                t - run.start
                    + run
                        .time_alignment
                        .as_ref()
                        .map_or(0.0, |alignment| alignment.offset_seconds),
            ),
            XMode::Distance => progress_at_time(&run.distance, t),
            XMode::Course => progress_at_time(&run.progress, t),
        }
    }
    fn time_at_x(&self, run: &PreparedRun, x: f64) -> Option<f64> {
        match self.effective_mode() {
            XMode::Time => {
                let relative = x - run
                    .time_alignment
                    .as_ref()
                    .map_or(0.0, |alignment| alignment.offset_seconds);
                (relative >= 0.0 && relative <= run.end - run.start).then_some(run.start + relative)
            }
            XMode::Distance => time_at_progress(&run.distance, x),
            XMode::Course => time_at_progress(&run.progress, x),
        }
    }
    fn run_time(&self, run: &PreparedRun) -> Option<f64> {
        let reference = self.reference_run()?;
        if run.key == reference.key {
            let t = reference.start + self.state.cursor;
            return (t >= reference.start && t <= reference.end).then_some(t);
        }
        let x = self.x_at_time(reference, reference.start + self.state.cursor)?;
        self.time_at_x(run, x)
    }
    fn delta_window(&self, run: &PreparedRun) -> (f64, f64) {
        if self.workspace.course.gates.is_empty() {
            return (run.start, run.end);
        }
        let Some(recording) = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == run.key.recording_id)
        else {
            return (run.start, run.end);
        };
        let Some(data) = self.data.get(&recording.primary_source) else {
            return (run.start, run.end);
        };
        let gps = gps_points(&data.raw, recording);
        gate_window(&gps, &self.workspace.course.gates, run.start, run.end)
            .unwrap_or((run.start, run.end))
    }
    fn scrub_x(&mut self, x: f64) {
        if let Some(r) = self.reference_run()
            && let Some(t) = self.time_at_x(r, x)
        {
            self.state.cursor = (t - r.start).clamp(0.0, r.end - r.start);
            self.playing = false;
        }
    }
    fn source_channel<'a>(
        &'a self,
        run: &PreparedRun,
        name: &str,
        options: &PlotOptions,
    ) -> Option<(&'a TelemetryChannel, f64)> {
        self.source_channel_with_bindings(run, name, &options.bindings)
    }
    fn source_channel_with_bindings<'a>(
        &'a self,
        run: &PreparedRun,
        name: &str,
        bindings: &BTreeMap<String, ChannelRef>,
    ) -> Option<(&'a TelemetryChannel, f64)> {
        let recording = self
            .workspace
            .recordings
            .iter()
            .find(|r| r.id == run.key.recording_id)?;
        let override_key = format!("{}:{name}", recording.id.0);
        if let Some(binding) = bindings.get(&override_key) {
            let source = recording
                .sources
                .iter()
                .find(|s| s.id == binding.source_id)?;
            return Some((
                self.data
                    .get(&source.id)?
                    .processed
                    .channel(binding.channel_id)?,
                source.alignment.offset_seconds,
            ));
        }
        let find = |source: &SourceConfig| {
            self.data
                .get(&source.id)
                .and_then(|d| {
                    d.processed.named(name).or_else(|| {
                        if name == "gps_speed" {
                            d.processed.named("speed")
                        } else {
                            None
                        }
                    })
                })
                .map(|c| (c, source.alignment.offset_seconds))
        };
        recording
            .sources
            .iter()
            .find(|s| s.id == recording.primary_source)
            .and_then(find)
            .or_else(|| recording.sources.iter().find_map(find))
    }
    fn channel_names(&self) -> Vec<String> {
        let mut names = self
            .data
            .values()
            .flat_map(|d| {
                d.processed
                    .channels
                    .values()
                    .map(|c| c.descriptor.name.clone())
            })
            .collect::<Vec<_>>();
        names.sort();
        names.dedup();
        names.push(DELTA_CHANNEL.into());
        names
    }
}

fn segment_key(segment: &SegmentRef) -> String {
    format!("{}:{}", segment.recording_id.0, segment.segment_id.0)
}
fn gate_window(
    gps: &[GpsPoint],
    gates: &[Gate],
    interval_start: f64,
    interval_end: f64,
) -> Option<(f64, f64)> {
    let starts = gate_crossings(gps, *gates.first()?);
    let pairs = if gates.len() == 1 {
        starts
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect::<Vec<_>>()
    } else {
        let finishes = gate_crossings(gps, gates[1]);
        starts
            .into_iter()
            .filter_map(|start| {
                finishes
                    .iter()
                    .copied()
                    .find(|finish| *finish > start + 1.0)
                    .map(|finish| (start, finish))
            })
            .collect()
    };
    pairs
        .into_iter()
        .filter_map(|pair| {
            let overlap = (pair.1.min(interval_end) - pair.0.max(interval_start)).max(0.0);
            (overlap > 0.0).then_some((overlap, pair))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, pair)| pair)
}
fn segment<'a>(
    workspace: &'a AnalysisWorkspace,
    key: &SegmentRef,
) -> Option<(&'a Recording, &'a RunSegment)> {
    let r = workspace
        .recordings
        .iter()
        .find(|r| r.id == key.recording_id)?;
    Some((r, r.segments.iter().find(|s| s.id == key.segment_id)?))
}
fn adapter_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "xrk" => "aim_xrk",
        "insv" | "lrv" => "insta360",
        _ => "generic_csv",
    }
}
fn is_video(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "mp4" | "mov" | "mkv"
    )
}
fn is_camera_telemetry_source(source: &SourceConfig) -> bool {
    builtin_adapter_capabilities(&source.adapter).camera_telemetry
}
fn display_unit(unit: Unit, quantity: Quantity, system: UnitSystem) -> Unit {
    system
        .default_unit_for(&quantity)
        .filter(|u| u.family() == unit.family())
        .unwrap_or(unit)
}

fn recording_channel<'a>(
    recording: &Recording,
    data: &'a HashMap<SourceId, SourceData>,
    names: &[&str],
) -> Option<(&'a TelemetryChannel, f64)> {
    recording
        .sources
        .iter()
        .filter(|source| source.id == recording.primary_source)
        .chain(
            recording
                .sources
                .iter()
                .filter(|source| source.id != recording.primary_source),
        )
        .find_map(|source| {
            let channel = names
                .iter()
                .find_map(|name| data.get(&source.id)?.processed.named(name))?;
            Some((channel, source.alignment.offset_seconds))
        })
}

fn normalized_run_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, SourceData>,
    names: &[&str],
    cutoff_hz: f64,
) -> Option<ChannelSeries> {
    let (recording, segment) = segment(workspace, key)?;
    let (channel, source_offset) = recording_channel(recording, data, names)?;
    if channel.descriptor.interpolation != Interpolation::Linear {
        return None;
    }
    let mut series = ChannelSeries::new(
        channel
            .series
            .samples
            .iter()
            .filter_map(|sample| {
                let recording_time = sample.time - source_offset;
                (recording_time >= segment.start_recording_time
                    && recording_time <= segment.end_recording_time)
                    .then_some(TimedSample {
                        time: recording_time - segment.start_recording_time,
                        value: sample.value,
                    })
            })
            .collect(),
    );
    series.gap_seconds = channel.series.gap_seconds;
    if series.samples.len() < 24 {
        return None;
    }
    Some(series.low_pass_hz(cutoff_hz).unwrap_or(series))
}

fn distance_domain_series(
    workspace: &AnalysisWorkspace,
    run: &PreparedRun,
    data: &HashMap<SourceId, SourceData>,
    names: &[&str],
    cutoff_hz: f64,
) -> Option<ChannelSeries> {
    let time_series = normalized_run_series(workspace, &run.key, data, names, cutoff_hz)?;
    let maximum = run.distance.last()?.progress;
    if !maximum.is_finite() || maximum < 20.0 {
        return None;
    }
    let spacing = (maximum / 2_000.0).max(0.5);
    let count = (maximum / spacing).floor() as usize;
    let samples = (0..=count)
        .filter_map(|index| {
            let distance = index as f64 * spacing;
            let recording_time = time_at_progress(&run.distance, distance)?;
            let value =
                time_series.sample_at_default(recording_time - run.start, Interpolation::Linear)?;
            Some(TimedSample {
                time: distance,
                value,
            })
        })
        .collect::<Vec<_>>();
    (samples.len() >= 40).then(|| ChannelSeries::new(samples).with_gap(spacing * 3.0))
}

fn correlate_distance_series(
    reference: &ChannelSeries,
    target: &ChannelSeries,
    use_absolute_correlation: bool,
) -> Option<CorrelationResult> {
    let coverage = reference
        .samples
        .last()?
        .time
        .min(target.samples.last()?.time);
    if coverage < 20.0 {
        return None;
    }
    let reference_window = (coverage * 0.45).clamp(60.0, 250.0).min(coverage);
    let search = (coverage * 0.08).clamp(8.0, 25.0);
    let reference = ChannelSeries::new(
        reference
            .samples
            .iter()
            .take_while(|sample| sample.time <= reference_window)
            .copied()
            .collect(),
    );
    let minimum_overlap = (reference_window * 0.55)
        .clamp(15.0, 80.0)
        .min(reference_window * 0.8);
    correlate_channel_series(
        &reference,
        target,
        &CorrelationConfig {
            min_lag_seconds: -search,
            max_lag_seconds: search,
            lag_resolution_seconds: Some(0.5),
            min_overlap_seconds: minimum_overlap,
            min_samples: 30,
            use_absolute_correlation,
            ..Default::default()
        },
    )
    .ok()
    .filter(|result| result.target_minus_reference_seconds.abs() < search * 0.8)
}

fn distance_anchor_time(samples: &[ProgressSample], distance: f64) -> Option<f64> {
    if distance.abs() > 1e-6 {
        return time_at_progress(samples, distance);
    }
    samples
        .iter()
        .take_while(|sample| sample.progress.abs() <= 1e-6)
        .filter(|sample| sample.confidence > 0.0)
        .map(|sample| sample.recording_time)
        .last()
        .or_else(|| time_at_progress(samples, distance))
}

fn spatial_time_alignment(
    workspace: &AnalysisWorkspace,
    reference: &PreparedRun,
    target: &PreparedRun,
    data: &HashMap<SourceId, SourceData>,
) -> Option<BestEffortTimeAlignment> {
    let candidates = [
        (
            "lateral acceleration by distance",
            &[
                "lateral_g",
                "gps_lateral_acceleration",
                "lateral_acceleration",
            ][..],
            3.0,
            true,
            0.65,
        ),
        (
            "yaw rate by distance",
            &["yaw_rate", "gps_yaw_rate", "raw_gyro_z", "gyro_z"][..],
            3.0,
            true,
            0.65,
        ),
        (
            "speed by distance",
            &["gps_speed", "speed"][..],
            2.0,
            false,
            0.85,
        ),
    ];
    candidates
        .into_iter()
        .find_map(|(label, names, cutoff, absolute, minimum_score)| {
            let reference_series =
                distance_domain_series(workspace, reference, data, names, cutoff)?;
            let target_series = distance_domain_series(workspace, target, data, names, cutoff)?;
            let result = correlate_distance_series(&reference_series, &target_series, absolute)?;
            let score = if absolute {
                result.correlation_coefficient.abs()
            } else {
                result.correlation_coefficient
            };
            (score >= minimum_score).then_some((label, result))
        })
        .and_then(|(channel, result)| {
            let distance_offset = result.target_minus_reference_seconds;
            let reference_distance = (-distance_offset).max(0.0);
            let target_distance = reference_distance + distance_offset;
            let reference_time = distance_anchor_time(&reference.distance, reference_distance)?;
            let target_time = distance_anchor_time(&target.distance, target_distance)?;
            let reference_elapsed = reference_time - reference.start;
            let target_elapsed = target_time - target.start;
            let maximum_anchor_elapsed = 15.0_f64
                .min((reference.end - reference.start) * 0.2)
                .min((target.end - target.start) * 0.2);
            if reference_elapsed > maximum_anchor_elapsed || target_elapsed > maximum_anchor_elapsed
            {
                return None;
            }
            Some(BestEffortTimeAlignment {
                offset_seconds: reference_elapsed - target_elapsed,
                coefficient: Some(result.correlation_coefficient),
                channel,
                distance_offset_meters: Some(distance_offset),
            })
        })
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

fn axis_motion_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, SourceData>,
    names: [&str; 3],
) -> Option<ChannelSeries> {
    let (recording, segment) = segment(workspace, key)?;
    for source in &recording.sources {
        let dataset = &data.get(&source.id)?.processed;
        let axes = names.map(|name| dataset.named(name));
        let [Some(x), Some(y), Some(z)] = axes else {
            continue;
        };
        let baseline_end = segment.start_recording_time + 3.0;
        let baseline = [&x, &y, &z].map(|axis| {
            let mut values = axis
                .series
                .samples
                .iter()
                .filter(|sample| {
                    let time = sample.time - source.alignment.offset_seconds;
                    time >= segment.start_recording_time && time <= baseline_end
                })
                .map(|sample| sample.value)
                .collect::<Vec<_>>();
            median(&mut values)
        });
        let [Some(bx), Some(by), Some(bz)] = baseline else {
            continue;
        };
        let samples = x
            .series
            .samples
            .iter()
            .filter_map(|sample| {
                let recording_time = sample.time - source.alignment.offset_seconds;
                if recording_time < segment.start_recording_time
                    || recording_time > segment.end_recording_time
                {
                    return None;
                }
                let y = y
                    .series
                    .sample_at_default(sample.time, Interpolation::Linear)?;
                let z = z
                    .series
                    .sample_at_default(sample.time, Interpolation::Linear)?;
                Some(TimedSample {
                    time: recording_time - segment.start_recording_time,
                    value: ((sample.value - bx).powi(2) + (y - by).powi(2) + (z - bz).powi(2))
                        .sqrt(),
                })
            })
            .collect::<Vec<_>>();
        if samples.len() >= 24 {
            let series = ChannelSeries::new(samples);
            return Some(series.low_pass_hz(3.0).unwrap_or(series));
        }
    }
    None
}

fn motion_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, SourceData>,
) -> Option<ChannelSeries> {
    axis_motion_series(
        workspace,
        key,
        data,
        ["raw_accel_x", "raw_accel_y", "raw_accel_z"],
    )
    .or_else(|| {
        axis_motion_series(
            workspace,
            key,
            data,
            ["accelerometer_x", "accelerometer_y", "accelerometer_z"],
        )
    })
    .or_else(|| {
        axis_motion_series(
            workspace,
            key,
            data,
            ["raw_gyro_x", "raw_gyro_y", "raw_gyro_z"],
        )
    })
    .or_else(|| axis_motion_series(workspace, key, data, ["gyro_x", "gyro_y", "gyro_z"]))
    .or_else(|| normalized_run_series(workspace, key, data, &["combined_g"], 3.0))
    .or_else(|| {
        normalized_run_series(
            workspace,
            key,
            data,
            &[
                "lateral_g",
                "gps_lateral_acceleration",
                "lateral_acceleration",
            ],
            3.0,
        )
    })
}

fn normalized_speed_series(
    workspace: &AnalysisWorkspace,
    key: &SegmentRef,
    data: &HashMap<SourceId, SourceData>,
) -> Option<ChannelSeries> {
    let (recording, segment) = segment(workspace, key)?;
    let (channel, source_offset) = recording_channel(recording, data, &["gps_speed", "speed"])?;
    if channel.descriptor.interpolation != Interpolation::Linear {
        return None;
    }
    let samples = channel
        .series
        .samples
        .iter()
        .filter_map(|sample| {
            let recording_time = sample.time - source_offset;
            if recording_time < segment.start_recording_time
                || recording_time > segment.end_recording_time
            {
                return None;
            }
            Unit::convert_value(
                sample.value,
                &channel.descriptor.unit,
                &Unit::MeterPerSecond,
            )
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| TimedSample {
                time: recording_time - segment.start_recording_time,
                value,
            })
        })
        .collect::<Vec<_>>();
    let series = ChannelSeries::new(samples);
    (series.samples.len() >= 24).then(|| series.low_pass_hz(2.0).unwrap_or(series))
}

fn speed_onset(series: &ChannelSeries) -> Option<f64> {
    for sample in &series.samples {
        if sample.value < 4.0 {
            continue;
        }
        let window = series
            .samples
            .iter()
            .filter(|candidate| {
                candidate.time >= sample.time && candidate.time <= sample.time + 3.0
            })
            .collect::<Vec<_>>();
        let spans = window
            .first()
            .zip(window.last())
            .is_some_and(|(first, last)| last.time - first.time >= 2.0);
        if spans
            && !window.is_empty()
            && window
                .iter()
                .filter(|candidate| candidate.value >= 4.0)
                .count()
                * 4
                >= window.len() * 3
        {
            return Some(sample.time);
        }
    }
    None
}

fn motion_onset(series: &ChannelSeries) -> Option<f64> {
    let end = series.samples.last()?.time;
    if end < 1.0 {
        return None;
    }
    let baseline_end = (end * 0.1).clamp(1.0, 3.0);
    let mut baseline_values = series
        .samples
        .iter()
        .take_while(|sample| sample.time <= baseline_end)
        .map(|sample| sample.value)
        .collect::<Vec<_>>();
    let baseline = median(&mut baseline_values)?;
    let mut baseline_activity = baseline_values
        .iter()
        .map(|value| (value - baseline).abs())
        .collect::<Vec<_>>();
    let noise = median(&mut baseline_activity).unwrap_or(0.0);
    let mut activity = series
        .samples
        .iter()
        .map(|sample| (sample.value - baseline).abs())
        .collect::<Vec<_>>();
    let percentile_index = activity.len() * 4 / 5;
    activity.sort_by(f64::total_cmp);
    let active_level = *activity.get(percentile_index.min(activity.len() - 1))?;
    if active_level <= noise * 2.0 + f64::EPSILON {
        return None;
    }
    let threshold = (noise * 6.0).max(active_level * 0.15).max(1e-6);
    for (index, sample) in series.samples.iter().enumerate() {
        let end_index = series.samples[index..]
            .partition_point(|candidate| candidate.time < sample.time + 0.6)
            + index;
        if end_index <= index + 2 {
            continue;
        }
        let window = &series.samples[index..end_index];
        let active = window
            .iter()
            .filter(|candidate| (candidate.value - baseline).abs() >= threshold)
            .count();
        if active * 3 >= window.len() * 2 {
            return Some(sample.time);
        }
    }
    None
}

fn onset_time_alignment(
    workspace: &AnalysisWorkspace,
    reference: &PreparedRun,
    target: &PreparedRun,
    data: &HashMap<SourceId, SourceData>,
) -> Option<BestEffortTimeAlignment> {
    let onset = |run: &PreparedRun| {
        normalized_speed_series(workspace, &run.key, data)
            .and_then(|series| speed_onset(&series))
            .or_else(|| motion_onset(&motion_series(workspace, &run.key, data)?))
    };
    let reference_onset = onset(reference)?;
    let target_onset = onset(target)?;
    Some(BestEffortTimeAlignment {
        offset_seconds: reference_onset - target_onset,
        coefficient: None,
        channel: "sustained motion onset",
        distance_offset_meters: None,
    })
}

fn best_effort_time_alignment(
    workspace: &AnalysisWorkspace,
    reference: &PreparedRun,
    target: &PreparedRun,
    data: &HashMap<SourceId, SourceData>,
) -> Option<BestEffortTimeAlignment> {
    spatial_time_alignment(workspace, reference, target, data)
        .or_else(|| onset_time_alignment(workspace, reference, target, data))
}

fn gps_point_at_time(points: &[GpsPoint], time: f64) -> Option<GpsPoint> {
    if !time.is_finite() || points.is_empty() {
        return None;
    }
    let index = points.partition_point(|point| point.recording_time < time);
    if let Some(point) = points.get(index)
        && (point.recording_time - time).abs() < 1e-9
    {
        return Some(*point);
    }
    let (left, right) = (points.get(index.checked_sub(1)?)?, points.get(index)?);
    let duration = right.recording_time - left.recording_time;
    if !(0.0..=2.0).contains(&duration) || duration <= f64::EPSILON {
        return None;
    }
    let fraction = (time - left.recording_time) / duration;
    let longitude_delta = (right.longitude - left.longitude + 540.0).rem_euclid(360.0) - 180.0;
    Some(GpsPoint {
        recording_time: time,
        latitude: left.latitude + (right.latitude - left.latitude) * fraction,
        longitude: (left.longitude + longitude_delta * fraction + 540.0).rem_euclid(360.0) - 180.0,
        accuracy_meters: left
            .accuracy_meters
            .zip(right.accuracy_meters)
            .map(|(left, right)| left.max(right)),
    })
}

fn gps_inside_interval(points: &[GpsPoint], start: f64, end: f64) -> Vec<GpsPoint> {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return vec![];
    }
    let mut clipped = Vec::new();
    if let Some(point) = gps_point_at_time(points, start) {
        clipped.push(point);
    }
    clipped.extend(
        points
            .iter()
            .copied()
            .filter(|point| point.recording_time > start && point.recording_time < end),
    );
    if let Some(point) = gps_point_at_time(points, end)
        && clipped
            .last()
            .is_none_or(|last| (last.recording_time - end).abs() >= 1e-9)
    {
        clipped.push(point);
    }
    clipped
}

fn timed_sample_at_time(
    samples: &[TimedSample],
    gaps: &[(f64, f64)],
    time: f64,
) -> Option<TimedSample> {
    if !time.is_finite() || samples.is_empty() {
        return None;
    }
    let index = samples.partition_point(|sample| sample.time < time);
    if let Some(sample) = samples.get(index)
        && (sample.time - time).abs() < 1e-9
    {
        return Some(*sample);
    }
    if gaps.iter().any(|(start, end)| time > *start && time < *end) {
        return None;
    }
    let (left, right) = (samples.get(index.checked_sub(1)?)?, samples.get(index)?);
    let duration = right.time - left.time;
    if duration <= f64::EPSILON {
        return None;
    }
    let fraction = (time - left.time) / duration;
    Some(TimedSample {
        time,
        value: left.value + (right.value - left.value) * fraction,
    })
}

fn timed_samples_inside_interval(
    samples: &[TimedSample],
    gaps: &[(f64, f64)],
    start: f64,
    end: f64,
) -> Vec<TimedSample> {
    if !start.is_finite() || !end.is_finite() || end <= start {
        return vec![];
    }
    let mut clipped = Vec::new();
    if let Some(sample) = timed_sample_at_time(samples, gaps, start) {
        clipped.push(sample);
    }
    clipped.extend(
        samples
            .iter()
            .copied()
            .filter(|sample| sample.time > start && sample.time < end),
    );
    if let Some(sample) = timed_sample_at_time(samples, gaps, end)
        && clipped
            .last()
            .is_none_or(|last| (last.time - end).abs() >= 1e-9)
    {
        clipped.push(sample);
    }
    clipped
}

fn prepare_comparison(
    workspace: &AnalysisWorkspace,
    selection: &[SegmentRef],
    data: &HashMap<SourceId, SourceData>,
) -> Prepared {
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
        .enumerate()
        .filter_map(|(index, key)| {
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
            Some(PreparedRun {
                key: key.clone(),
                name: format!("{} / {}", r.name, s.name),
                color: COLORS[index % COLORS.len()],
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
    Prepared {
        runs,
        course,
        automatic_time_alignment,
    }
}
