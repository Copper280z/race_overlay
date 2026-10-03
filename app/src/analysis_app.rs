//! Telemetry-first analysis workspace. The existing overlay editor is a separate mode.
use crate::analysis_maps::{MapPanel, MapSettings, MapTrace};
use crossbeam_channel::{Receiver, Sender, unbounded};
use egui_dock::{DockArea, DockState, NodeIndex, TabViewer};
use overlay_core::*;
use overlay_core::{
    AnalysisSourceData as SourceData, PreparedComparison as Prepared,
    PreparedComparisonRun as PreparedRun,
};
use overlay_media::{AlignmentResult, FfmpegTools, VideoMetadata};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};
#[path = "analysis_chrome.rs"]
mod chrome;
#[path = "analysis_decimate.rs"]
mod decimate;
pub use chrome::{LAYOUT_PRESETS, PanelChoice};
#[path = "analysis_io.rs"]
mod io;
#[path = "analysis_model.rs"]
mod model;
#[path = "analysis_recordings.rs"]
mod recordings;
#[path = "analysis_setup/mod.rs"]
mod setup;
#[path = "analysis_statistics.rs"]
mod statistics;
#[path = "analysis_sync/mod.rs"]
mod sync;
#[cfg(test)]
#[path = "analysis_tests.rs"]
mod tests;
use io::Event;
use model::*;
use sync::{CameraVideoSyncMetadata, VideoAlignmentCandidate, VideoSyncController};
#[path = "analysis_views.rs"]
mod views;
#[path = "analysis_workflow.rs"]
mod workflow;
pub enum AnalysisAction {
    OpenOverlay(Box<ProjectV1>),
    OpenMyChron,
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
    trace_palette: [egui::Color32; 8],
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
    /// Newly added recordings may belong to one another (a log and the video
    /// recorded during it); checked once everything has loaded.
    pair_pending: bool,
    select_imports_pending: HashSet<RecordingId>,
    auto_mode_pending: bool,
    automatic_mode: bool,
    initial_layout_pending: bool,
    setup_tab: setup::SetupTab,
    /// Aerial imagery services added in Settings; owned by the application
    /// shell's preferences and handed in here because maps use them.
    pub(crate) imagery_sources: Vec<crate::imagery_sources::ImagerySource>,
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
            trace_palette: crate::ui_kit::theme::palette().traces,
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
            pair_pending: false,
            select_imports_pending: HashSet::new(),
            auto_mode_pending: false,
            automatic_mode: true,
            initial_layout_pending: true,
            setup_tab: Default::default(),
            imagery_sources: Vec::new(),
        };
        app.layout(1);
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
    /// Builds one of the layout presets: 0 Quick Compare, 1 Data Focus,
    /// 2 Video Compare. `egui_dock` split fractions are the share taken by the
    /// top/left child, whichever of the two is new.
    fn layout(&mut self, preset: usize) {
        let video = |app: &mut Self, slot| {
            app.tab(TabKind::Video(VideoOptions {
                slot,
                segment: None,
                linked: true,
                time: 0.0,
                unknown: Default::default(),
            }))
        };
        let plot = self.tab(TabKind::Plot(Default::default()));
        let delta = self.tab(TabKind::Plot(PlotOptions {
            delta: true,
            ..Default::default()
        }));
        let browser = self.tab(TabKind::Browser);
        let setup = self.tab(TabKind::Setup);
        let map = self.tab(TabKind::Map {
            channel: "gps_speed".into(),
            settings: Default::default(),
        });
        let imagery = self.tab(TabKind::Map {
            channel: "gps_speed".into(),
            settings: Box::new(MapSettings {
                actual_gps: true,
                small_multiples: false,
                ..Default::default()
            }),
        });
        let stats = self.tab(TabKind::Stats);
        let videos = (preset != 1).then(|| [video(self, 0), video(self, 1)]);
        self.dock = DockState::new(vec![plot, delta]);
        let surface = self.dock.main_surface_mut();
        // Recordings on the left, analysis in the middle, maps and values right.
        let [main, _] = surface.split_left(NodeIndex::root(), 0.24, vec![browser, setup]);
        let side_share = if preset == 2 { 0.76 } else { 0.70 };
        let [main, _] = surface.split_right(main, side_share, vec![map, imagery, stats]);
        if let Some([first, second]) = videos {
            let video_share = if preset == 2 { 0.55 } else { 0.40 };
            let [_, top] = surface.split_above(main, video_share, vec![first]);
            surface.split_right(top, 0.5, vec![second]);
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
    fn refresh_trace_palette(&mut self, palette: [egui::Color32; 8]) {
        if self.trace_palette != palette {
            self.trace_palette = palette;
            self.plot_cache.clear();
            self.scatter_cache.clear();
            self.map_cache.clear();
        }
    }
    /// Choose the first import's layout once; later imports preserve the dock.
    fn finish_initial_layout(&mut self) {
        if self.initial_layout_pending && !self.workspace.recordings.is_empty() {
            let has_video = self
                .workspace
                .recordings
                .iter()
                .any(|r| r.video_path.is_some());
            self.layout(if has_video { 0 } else { 1 });
            self.initial_layout_pending = false;
        }
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
    /// Workspace name for the window header and whether it has unsaved edits.
    pub fn document_title(&self) -> (String, bool) {
        let name = self
            .path
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .map(|name| {
                name.strip_suffix(".race-analysis.json")
                    .map(str::to_owned)
                    .unwrap_or(name)
            })
            .unwrap_or_else(|| "Untitled workspace".into());
        (name, self.dirty)
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    /// Background work worth showing in the status bar, if any.
    pub fn busy_label(&self) -> Option<String> {
        (self.preparing || !self.loading.is_empty()).then(|| {
            if self.loading.is_empty() {
                "Preparing comparison…".into()
            } else {
                format!("Loading {} source(s)…", self.loading.len())
            }
        })
    }
    pub fn issues(&self) -> &[String] {
        &self.errors
    }
    pub fn clear_issues(&mut self) {
        self.errors.clear();
    }
    pub fn notify(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }
    /// Imports telemetry, video, or workspace files as if they were dropped.
    #[cfg(test)]
    pub fn add_files(&mut self, paths: Vec<PathBuf>) {
        self.add_paths(paths);
    }
    /// Attaches `path` as the video of the first recording.
    #[cfg(test)]
    pub fn attach_video_to_first(&mut self, path: PathBuf) {
        if let Some(id) = self.workspace.recordings.first().map(|r| r.id) {
            self.attach_video(id, path);
        }
    }
    /// True when no import or comparison work is pending.
    #[cfg(test)]
    pub fn is_settled(&self) -> bool {
        !self.preparing && self.loading.is_empty() && self.prepared_revision == self.revision
    }
    pub fn toggle_play(&mut self) {
        if self.reference_run().is_some() {
            self.playing = !self.playing;
        }
    }
    pub fn save_workspace(&mut self) {
        self.save(false);
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
        self.refresh_trace_palette(crate::ui_kit::theme::palette().traces);
        if !self.workspace.settings.is_object() {
            self.workspace.settings = json!({});
        }
        self.poll(ui.ctx());
        self.start_pending_video_audio_sync(tools);
        self.apply_default_camera_calibrations();
        self.drive_auto_sync();
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
        if !self.workspace.recordings.is_empty() {
            self.transport(ui);
        }
        self.visible_videos.clear();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(crate::ui_kit::theme::surface::canvas()))
            .show(ui, |ui| {
                if self.workspace.recordings.is_empty() {
                    self.welcome(ui);
                } else {
                    self.dock_area(ui, tools, units);
                }
            });
        if ui.input(|i| !i.raw.hovered_files.is_empty()) {
            drop_overlay(ui.ctx());
        }
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
        } else if self.has_background_work() {
            // Worker threads hand results over a channel without waking the
            // UI, so poll while any are running.
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        self.remove_dialog(ui.ctx());
    }
    /// True while a worker may deliver something the next frame must show.
    fn has_background_work(&self) -> bool {
        self.preparing
            || self.prepared_revision != self.revision
            || !self.loading.is_empty()
            || !self.probing.is_empty()
            || self.video_sync.has_jobs()
    }
    fn dock_area(&mut self, ui: &mut egui::Ui, tools: Option<&FfmpegTools>, units: UnitSystem) {
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
                    TabKind::Stats => self.app.statistics(ui, t.id, self.units),
                    TabKind::Plot(p) => self.app.plot(ui, t.id, p, self.units),
                    TabKind::Scatter(options) => self.app.scatter(ui, t.id, options, self.units),
                    TabKind::Video(v) => self.app.video(ui, t.id, v, self.tools),
                    TabKind::Map { channel, settings } => {
                        self.app.map(ui, t.id, channel, settings, self.units)
                    }
                }
            }
        }
        DockArea::new(&mut dock)
            .style(crate::ui_kit::theme::dock_style(ui.style()))
            .show_inside(
                ui,
                &mut Viewer {
                    app: self,
                    tools,
                    units,
                },
            );
        self.dock = dock;
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
    /// Elapsed time is measured from here: the start gate when both this run
    /// and the reference cross it, otherwise the interval start.
    fn elapsed_origin(&self, run: &PreparedRun) -> f64 {
        self.reference_run()
            .and_then(|reference| reference.start_gate)
            .and(run.start_gate)
            .unwrap_or(run.start)
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
    /// Channels a plot can offer: everything loaded, plus any it already
    /// lists that no loaded source has (such as the default speed and RPM
    /// when only a camera is loaded), so those can still be switched off.
    fn channel_choices(&self, selected: &[String]) -> Vec<String> {
        let available = self.channel_names();
        let mut choices = selected
            .iter()
            .filter(|name| !available.contains(name))
            .cloned()
            .collect::<Vec<_>>();
        choices.extend(available);
        choices
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
fn run_color(
    selection: &[SegmentRef],
    reference: Option<&SegmentRef>,
    key: &SegmentRef,
) -> egui::Color32 {
    let index = selection
        .iter()
        .position(|selected| selected == key)
        .or_else(|| (reference == Some(key)).then_some(selection.len()))
        .unwrap_or_default();
    let colors = crate::ui_kit::theme::palette().traces;
    colors[index % colors.len()]
}
fn adapter_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "xrk" | "xrz" | "hrz" => "aim_xrk",
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

#[cfg(test)]
impl AnalysisApp {
    pub fn set_setup_tab_for_test(&mut self, index: usize) {
        self.setup_tab = [
            setup::SetupTab::Intervals,
            setup::SetupTab::Sources,
            setup::SetupTab::Gates,
        ][index];
    }
    /// Brings the first tab whose title contains `title` to the front.
    pub fn focus_tab_for_test(&mut self, title: &str) -> bool {
        let Some(location) = self.dock.find_tab_from(|tab| tab.title().contains(title)) else {
            return false;
        };
        self.dock.set_active_tab(location).is_ok()
    }
}

/// Full-window hint while files are dragged over the application.
fn drop_overlay(ctx: &egui::Context) {
    use crate::ui_kit::theme;
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("file-drop-overlay"),
    ));
    let rect = ctx.content_rect();
    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
    painter.rect_stroke(
        rect.shrink(16.0),
        12.0,
        egui::Stroke::new(2.0, theme::accent()),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "Drop to import",
        egui::FontId::proportional(24.0),
        theme::text::strong(),
    );
}
