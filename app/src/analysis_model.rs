//! Serializable Analysis UI state and view-only runtime models.

use crate::analysis_maps::MapSettings;
use crate::video_processing::VideoPreview;
use eframe::egui;
use overlay_core::{ChannelRef, PreparedComparison as Prepared, RecordingId, SegmentRef, Unit};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};

pub(super) const COLORS: [egui::Color32; 8] = [
    egui::Color32::from_rgb(65, 170, 255),
    egui::Color32::from_rgb(255, 156, 66),
    egui::Color32::from_rgb(70, 210, 145),
    egui::Color32::from_rgb(210, 110, 255),
    egui::Color32::from_rgb(250, 215, 60),
    egui::Color32::from_rgb(70, 215, 220),
    egui::Color32::from_rgb(250, 105, 140),
    egui::Color32::from_rgb(180, 190, 255),
];
pub(super) const DELTA_CHANNEL: &str = "delta_time";
pub(super) const DELTA_LABEL: &str = "Time delta — positive is slower";

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize, Debug)]
pub(super) enum XMode {
    Time,
    Distance,
    #[default]
    Course,
}

impl XMode {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Time => "Elapsed time",
            Self::Distance => "Traveled distance",
            Self::Course => "Course position",
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct PlotOptions {
    pub(super) channels: Vec<String>,
    pub(super) filter: Option<f64>,
    pub(super) units: BTreeMap<String, Unit>,
    pub(super) bindings: BTreeMap<String, ChannelRef>,
    pub(super) delta: bool,
    pub(super) show_legend: bool,
    pub(super) legend_labels: BTreeMap<String, String>,
    #[serde(flatten)]
    pub(super) unknown: BTreeMap<String, Value>,
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
            legend_labels: BTreeMap::new(),
            unknown: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct ScatterOptions {
    pub(super) x_channel: String,
    pub(super) y_channel: String,
    pub(super) z_channel: Option<String>,
    pub(super) filter: Option<f64>,
    pub(super) units: BTreeMap<String, Unit>,
    pub(super) bindings: BTreeMap<String, ChannelRef>,
    pub(super) show_legend: bool,
    pub(super) legend_labels: BTreeMap<String, String>,
    #[serde(flatten)]
    pub(super) unknown: BTreeMap<String, Value>,
}

impl Default for ScatterOptions {
    fn default() -> Self {
        Self {
            x_channel: "gps_lateral_acceleration".into(),
            y_channel: "gps_inline_acceleration".into(),
            z_channel: None,
            filter: None,
            units: BTreeMap::new(),
            bindings: BTreeMap::new(),
            show_legend: true,
            legend_labels: BTreeMap::new(),
            unknown: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct VideoOptions {
    pub(super) slot: usize,
    pub(super) segment: Option<SegmentRef>,
    pub(super) linked: bool,
    pub(super) time: f64,
    #[serde(default, flatten)]
    pub(super) unknown: BTreeMap<String, Value>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) enum TabKind {
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
pub(super) struct Tab {
    pub(super) id: u64,
    pub(super) kind: TabKind,
    #[serde(default, flatten)]
    pub(super) unknown: BTreeMap<String, Value>,
}

impl Tab {
    pub(super) fn title(&self) -> String {
        match &self.kind {
            TabKind::Browser => "Recordings & laps".into(),
            TabKind::Plot(options) => {
                if options.delta {
                    "Time gain / loss".into()
                } else {
                    "Channel plot".into()
                }
            }
            TabKind::Scatter(_) => "X/Y scatter plot".into(),
            TabKind::Video(video) => format!("Video {}", video.slot + 1),
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
pub(super) struct UiState {
    pub(super) selection: Vec<SegmentRef>,
    pub(super) mode: XMode,
    pub(super) cursor: f64,
    pub(super) range: Option<[f64; 2]>,
    pub(super) dock: Value,
    pub(super) selected_recording: Option<RecordingId>,
    #[serde(flatten)]
    pub(super) unknown: BTreeMap<String, Value>,
}

pub(super) fn automatic_alignment_complete(
    prepared: &Prepared,
    reference: Option<&SegmentRef>,
) -> bool {
    let candidates = prepared
        .runs
        .iter()
        .filter(|run| Some(&run.key) != reference)
        .collect::<Vec<_>>();
    !candidates.is_empty() && candidates.iter().all(|run| run.time_alignment.is_some())
}

pub(super) fn automatic_default_mode(prepared: &Prepared, reference: Option<&SegmentRef>) -> XMode {
    if automatic_alignment_complete(prepared, reference) {
        XMode::Time
    } else if prepared.runs.iter().all(|run| run.distance.len() >= 2) {
        XMode::Distance
    } else {
        XMode::Time
    }
}

pub(super) struct VideoRuntime {
    pub(super) path: PathBuf,
    pub(super) decoder: VideoPreview,
    pub(super) texture: Option<egui::TextureHandle>,
    pub(super) requested: Option<(f64, u32, u32)>,
    pub(super) processing: Option<overlay_core::VideoProcessingConfig>,
    pub(super) error: Option<String>,
}

pub(super) struct PlotTrace {
    pub(super) segment: SegmentRef,
    pub(super) name: String,
    pub(super) color: egui::Color32,
    pub(super) points: Vec<Vec<[f64; 2]>>,
    pub(super) unit: Unit,
}

pub(super) struct ScatterTrace {
    pub(super) segment: SegmentRef,
    pub(super) name: String,
    pub(super) color: egui::Color32,
    pub(super) points: Vec<[f64; 3]>,
}
