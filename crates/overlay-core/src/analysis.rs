//! Persistent analysis data and deliberately renderer-independent timing/course helpers.
//!
//! Analysis uses a recording clock.  This is intentionally distinct from the
//! editor's video clock: `video_time = recording_time + video_offset_seconds`.
use crate::{ProjectV1, SourceConfig, SourceId, TelemetryDataset, TimedSample, Unit};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum AnalysisError {
    #[error("could not read analysis workspace: {0}")]
    Io(#[from] io::Error),
    #[error("invalid analysis JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported analysis version {0}")]
    UnsupportedVersion(u32),
    #[error("recording was not found")]
    RecordingNotFound,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecordingId(pub Uuid);
impl RecordingId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for RecordingId {
    fn default() -> Self {
        Self::new()
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SegmentId(pub Uuid);
impl SegmentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for SegmentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    #[serde(default)]
    pub id: RecordingId,
    pub name: String,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    pub primary_source: SourceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_processing: Option<crate::VideoProcessingConfig>,
    #[serde(default)]
    pub video_offset_seconds: f64,
    #[serde(default)]
    pub segments: Vec<RunSegment>,
    /// A complete legacy overlay project, including its flattened unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_snapshot: Option<ProjectV1>,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}
impl Recording {
    pub fn source_time(&self, source: SourceId, recording_time: f64) -> Option<f64> {
        self.sources
            .iter()
            .find(|s| s.id == source)
            .map(|s| recording_time + s.alignment.offset_seconds)
    }
    pub fn recording_time(&self, video_time: f64) -> f64 {
        video_time - self.video_offset_seconds
    }
    pub fn video_time(&self, recording_time: f64) -> f64 {
        recording_time + self.video_offset_seconds
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SegmentRef {
    pub recording_id: RecordingId,
    pub segment_id: SegmentId,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    Lap,
    Autocross,
    MotionTrim,
    OutLap,
    InLap,
    #[default]
    Unknown,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunSegment {
    #[serde(default)]
    pub id: SegmentId,
    pub name: String,
    pub start_recording_time: f64,
    pub end_recording_time: f64,
    #[serde(default)]
    pub kind: SegmentKind,
    #[serde(default)]
    pub estimated: bool,
    #[serde(default)]
    pub competitive: bool,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}
impl RunSegment {
    pub fn duration(&self) -> f64 {
        (self.end_recording_time - self.start_recording_time).max(0.0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct AnalysisWorkspace {
    #[serde(default)]
    pub recordings: Vec<Recording>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<SegmentRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<SegmentRef>,
    #[serde(default)]
    pub course: CourseDefinition,
    #[serde(default)]
    pub settings: Value,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
    /// Unknown envelope fields are retained separately so versioned document
    /// extensions survive a load/save cycle.
    #[serde(skip)]
    pub envelope_unknown: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct CourseDefinition {
    #[serde(default)]
    pub gates: Vec<Gate>,
    #[serde(default)]
    pub manual_anchors: Vec<ManualAnchor>,
    /// Opt-in: shift each compared run's GPS onto the reference to remove
    /// position drift between runs (see `comparison::drift`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub correct_gps_drift: bool,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gate {
    pub latitude: f64,
    pub longitude: f64,
    /// direction in degrees clockwise from north
    #[serde(default)]
    pub heading_degrees: f64,
    #[serde(default = "gate_width")]
    pub width_meters: f64,
}
fn gate_width() -> f64 {
    12.0
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManualAnchor {
    /// Optional owner prevents a correction for one run being applied to another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<SegmentRef>,
    pub recording_time: f64,
    pub reference_progress: f64,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AnalysisDocument {
    V1(AnalysisWorkspace),
}
impl Serialize for AnalysisDocument {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct E<'a> {
            version: u32,
            workspace: &'a AnalysisWorkspace,
            #[serde(flatten)]
            unknown: &'a BTreeMap<String, Value>,
        }
        match self {
            Self::V1(w) => E {
                version: 1,
                workspace: w,
                unknown: &w.envelope_unknown,
            }
            .serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for AnalysisDocument {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct E {
            version: u32,
            workspace: AnalysisWorkspace,
            #[serde(default, flatten)]
            unknown: BTreeMap<String, Value>,
        }
        let e = E::deserialize(d)?;
        match e.version {
            1 => {
                let mut workspace = e.workspace;
                workspace.envelope_unknown = e.unknown;
                Ok(Self::V1(workspace))
            }
            n => Err(de::Error::custom(format!(
                "unsupported analysis version {n}"
            ))),
        }
    }
}
impl AnalysisDocument {
    pub fn workspace(&self) -> &AnalysisWorkspace {
        match self {
            Self::V1(w) => w,
        }
    }
    pub fn workspace_mut(&mut self) -> &mut AnalysisWorkspace {
        match self {
            Self::V1(w) => w,
        }
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self, AnalysisError> {
        let path = path.as_ref();
        let v: Value = serde_json::from_slice(&fs::read(path)?)?;
        let n = v
            .get("version")
            .and_then(Value::as_u64)
            .ok_or(AnalysisError::UnsupportedVersion(0))?;
        if n != 1 {
            return Err(AnalysisError::UnsupportedVersion(n as u32));
        };
        let mut doc: Self = serde_json::from_value(v)?;
        let base = fs::canonicalize(path)?
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();
        visit_recording_paths(doc.workspace_mut(), |p| {
            if !p.as_os_str().is_empty() && p.is_relative() {
                *p = base.join(&*p);
            }
        });
        Ok(doc)
    }
    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), AnalysisError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or(Path::new("."));
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("analysis");
        let base = fs::canonicalize(if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        })?;
        let mut portable = self.clone();
        visit_recording_paths(portable.workspace_mut(), |p| {
            if let Ok(relative) = p.strip_prefix(&base) {
                *p = relative.to_path_buf();
            }
        });
        let tmp = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
        fs::write(&tmp, serde_json::to_vec_pretty(&portable)?)?;
        if let Err(error) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(error.into());
        }
        Ok(())
    }
}

fn visit_recording_paths(workspace: &mut AnalysisWorkspace, mut visit: impl FnMut(&mut PathBuf)) {
    // Dock layouts contain known imagery settings; retain all other UI and
    // future settings verbatim while making saved local images portable too.
    fn images(value: &mut Value, visit: &mut impl FnMut(&mut PathBuf)) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if key == "image_path" {
                        if let Some(s) = value.as_str() {
                            let mut path = PathBuf::from(s);
                            visit(&mut path);
                            *value = Value::String(path.to_string_lossy().into_owned());
                        }
                    } else {
                        images(value, visit);
                    }
                }
            }
            Value::Array(values) => {
                for value in values {
                    images(value, visit);
                }
            }
            _ => (),
        }
    }
    if let Some(dock) = workspace
        .settings
        .get_mut("ui")
        .and_then(|ui| ui.get_mut("dock"))
    {
        images(dock, &mut visit);
    }
    for recording in &mut workspace.recordings {
        if let Some(video) = &mut recording.video_path {
            visit(video);
        }
        for source in &mut recording.sources {
            if source.adapter != "synthetic" {
                visit(&mut source.path);
            }
        }
        if let Some(project) = &mut recording.overlay_snapshot {
            visit(&mut project.video_path);
            for source in &mut project.sources {
                if source.adapter != "synthetic" {
                    visit(&mut source.path);
                }
            }
            if let Some(output) = &mut project.export.output_path {
                visit(output);
            }
        }
    }
}

/// Convert a legacy editor project into one recording. Because
/// `video=recording+video_offset`, an old project offset becomes
/// `recording_offset=project_offset+video_offset`.
pub fn recording_from_project(
    project: ProjectV1,
    name: impl Into<String>,
    video_offset_seconds: f64,
) -> Recording {
    let mut sources = project.sources.clone();
    for s in &mut sources {
        s.alignment.offset_seconds += video_offset_seconds;
    }
    let primary_source = sources
        .iter()
        .find(|s| s.adapter == "aim_xrk")
        .or_else(|| sources.first())
        .map(|s| s.id)
        .unwrap_or_default();
    Recording {
        id: RecordingId::new(),
        name: name.into(),
        sources,
        primary_source,
        video_path: Some(project.video_path.clone()),
        video_processing: project.video_processing.clone(),
        video_offset_seconds,
        segments: vec![],
        overlay_snapshot: Some(project),
        unknown: BTreeMap::new(),
    }
}
/// Make an editor project; source offsets retain their exact legacy meaning.
pub fn project_from_recording(recording: &Recording) -> ProjectV1 {
    let mut p = recording
        .overlay_snapshot
        .clone()
        .unwrap_or_else(|| ProjectV1::new(recording.video_path.clone().unwrap_or_default()));
    p.video_path = recording.video_path.clone().unwrap_or(p.video_path);
    p.video_processing = recording.video_processing.clone();
    p.sources = recording.sources.clone();
    for s in &mut p.sources {
        s.alignment.offset_seconds -= recording.video_offset_seconds;
    }
    p
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpsPoint {
    pub recording_time: f64,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_meters: Option<f64>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DistanceSeries {
    pub samples: Vec<TimedSample>,
    pub gaps: Vec<(f64, f64)>,
}
/// Extract paired degree latitude/longitude channels. Pairing is sampled only
/// inside the source's declared gap policy, so a GPS outage is never bridged.
pub fn gps_points(dataset: &TelemetryDataset, recording: &Recording) -> Vec<GpsPoint> {
    let lat = dataset.channels.values().find(|c| {
        matches_name(
            &c.descriptor.name,
            &["latitude", "lat", "gps_lat", "gps_latitude"],
        )
    });
    let lon = dataset.channels.values().find(|c| {
        matches_name(
            &c.descriptor.name,
            &["longitude", "lon", "lng", "gps_lon", "gps_longitude"],
        )
    });
    let (Some(lat), Some(lon)) = (lat, lon) else {
        return vec![];
    };
    if !matches!(lat.descriptor.unit, Unit::Degree) || !matches!(lon.descriptor.unit, Unit::Degree)
    {
        return vec![];
    }
    let acc = dataset
        .channels
        .values()
        .find(|c| {
            matches_name(
                &c.descriptor.name,
                &[
                    "gps_position_accuracy",
                    "position_accuracy",
                    "gps_accuracy",
                    "accuracy",
                ],
            )
        })
        .filter(|c| c.descriptor.unit.family() == Some(crate::UnitFamily::Distance));
    let off = recording
        .sources
        .iter()
        .find(|s| s.id == dataset.source_id)
        .map_or(0., |s| s.alignment.offset_seconds);
    lat.series
        .samples
        .iter()
        .filter_map(|s| {
            let lo = lon
                .series
                .sample_at_default(s.time, crate::Interpolation::Linear)?;
            if !(-90.0..=90.0).contains(&s.value) || !(-180.0..=180.0).contains(&lo) {
                return None;
            };
            Some(GpsPoint {
                recording_time: s.time - off,
                latitude: s.value,
                longitude: lo,
                accuracy_meters: acc.and_then(|a| {
                    a.series
                        .sample_at_default(s.time, crate::Interpolation::Linear)
                        .and_then(|v| Unit::convert_value(v, &a.descriptor.unit, &Unit::Meter).ok())
                        .filter(|v| v.is_finite() && *v >= 0.)
                }),
            })
        })
        .collect()
}
fn matches_name(n: &str, names: &[&str]) -> bool {
    let n = n.to_ascii_lowercase().replace([' ', '-'], "_");
    names.contains(&n.as_str())
}
#[allow(clippy::collapsible_if)]
pub fn traveled_distance(
    dataset: &TelemetryDataset,
    recording: &Recording,
    gps: &[GpsPoint],
) -> DistanceSeries {
    let offset = recording
        .sources
        .iter()
        .find(|s| s.id == dataset.source_id)
        .map_or(0., |s| s.alignment.offset_seconds);
    if let Some(c) = dataset.channels.values().find(|c| {
        matches_name(
            &c.descriptor.name,
            &["distance", "odometer", "gps_distance"],
        ) && c.descriptor.unit.family() == Some(crate::UnitFamily::Distance)
    }) {
        let Ok(first) = c
            .series
            .samples
            .first()
            .map(|x| Unit::convert_value(x.value, &c.descriptor.unit, &Unit::Meter))
            .unwrap_or(Ok(0.))
        else {
            return DistanceSeries {
                samples: vec![],
                gaps: vec![],
            };
        };
        let mut samples = vec![];
        let mut gaps = vec![];
        let mut previous: Option<(f64, f64)> = None;
        let mut reset_bias = 0.0;
        let mut last_distance: f64 = 0.0;
        for sample in &c.series.samples {
            let Ok(value) = Unit::convert_value(sample.value, &c.descriptor.unit, &Unit::Meter)
            else {
                continue;
            };
            if let Some((time, prior)) = previous {
                if sample.time - time > c.series.gap_seconds.unwrap_or(2.) || value + 1. < prior {
                    gaps.push((time - offset, sample.time - offset));
                }
                if value + 1.0 < prior {
                    reset_bias += prior - value;
                }
            }
            last_distance = (value - first + reset_bias).max(last_distance);
            samples.push(TimedSample {
                time: sample.time - offset,
                value: last_distance,
            });
            previous = Some((sample.time, value));
        }
        return DistanceSeries { samples, gaps };
    }
    let speed = dataset.channels.values().find(|c| {
        matches_name(&c.descriptor.name, &["speed", "gps_speed", "velocity"])
            && c.descriptor.unit.family() == Some(crate::UnitFamily::Speed)
    });
    if let Some(c) = speed {
        let mut out = Vec::new();
        let mut gaps = Vec::new();
        let mut d = 0.;
        if let Some(first) = c.series.samples.first() {
            out.push(TimedSample {
                time: first.time - offset,
                value: 0.,
            });
        }
        for pair in c.series.samples.windows(2) {
            let dt = pair[1].time - pair[0].time;
            if dt <= 0. || dt > c.series.gap_seconds.unwrap_or(2.) {
                gaps.push((pair[0].time - offset, pair[1].time - offset));
                continue;
            }
            let (Ok(a), Ok(b)) = (
                Unit::convert_value(pair[0].value, &c.descriptor.unit, &Unit::MeterPerSecond),
                Unit::convert_value(pair[1].value, &c.descriptor.unit, &Unit::MeterPerSecond),
            ) else {
                return DistanceSeries {
                    samples: vec![],
                    gaps: vec![],
                };
            };
            d += (a.max(0.) + b.max(0.)) * dt / 2.;
            out.push(TimedSample {
                time: pair[1].time - offset,
                value: d,
            })
        }
        return DistanceSeries { samples: out, gaps };
    }
    let mut out = Vec::new();
    let mut gaps = Vec::new();
    let mut d = 0.;
    if let Some(first) = gps.first() {
        out.push(TimedSample {
            time: first.recording_time,
            value: 0.,
        });
    }
    for p in gps.windows(2) {
        let dt = p[1].recording_time - p[0].recording_time;
        if dt <= 0. || dt > 2. {
            gaps.push((p[0].recording_time, p[1].recording_time));
            continue;
        }
        d += haversine(&p[0], &p[1]);
        out.push(TimedSample {
            time: p[1].recording_time,
            value: d,
        })
    }
    DistanceSeries { samples: out, gaps }
}
pub fn auto_segments(
    dataset: &TelemetryDataset,
    recording: &Recording,
    gps: &[GpsPoint],
) -> Vec<RunSegment> {
    let offset = recording
        .sources
        .iter()
        .find(|s| s.id == dataset.source_id)
        .map_or(0., |s| s.alignment.offset_seconds);
    let full_laps: Vec<_> = dataset
        .laps
        .iter()
        .filter(|lap| lap.lap_type.eq_ignore_ascii_case("full"))
        .collect();
    if !full_laps.is_empty() {
        return dataset
            .laps
            .iter()
            .map(|l| RunSegment {
                id: SegmentId::new(),
                name: format!("Lap {}", l.number),
                start_recording_time: l.start_time - offset,
                end_recording_time: l.end_time - offset,
                kind: match l.lap_type.to_ascii_lowercase().as_str() {
                    "out" => SegmentKind::OutLap,
                    "in" => SegmentKind::InLap,
                    _ => SegmentKind::Lap,
                },
                estimated: false,
                competitive: !l.lap_type.to_ascii_lowercase().contains("out")
                    && !l.lap_type.to_ascii_lowercase().contains("in"),
                unknown: BTreeMap::new(),
            })
            .collect();
    }
    if let Some(finish) = dataset
        .laps
        .first()
        .filter(|_| dataset.laps.len() > 1)
        .map(|lap| lap.end_time - offset)
        && let Some(start) = autocross_launch_time(dataset, offset, finish)
        && finish - start >= 20.0
    {
        return vec![RunSegment {
            id: SegmentId::new(),
            name: "Autocross run".into(),
            start_recording_time: start,
            end_recording_time: finish,
            kind: SegmentKind::Autocross,
            estimated: true,
            competitive: true,
            unknown: BTreeMap::new(),
        }];
    }
    if gps.is_empty() {
        let range = dataset
            .channels
            .values()
            .flat_map(|c| c.series.samples.iter())
            .filter(|s| s.time.is_finite())
            .fold(None, |range: Option<(f64, f64)>, s| {
                Some(range.map_or((s.time, s.time), |(a, b)| (a.min(s.time), b.max(s.time))))
            });
        return range
            .filter(|(a, b)| b > a)
            .map(|(a, b)| {
                vec![RunSegment {
                    id: SegmentId::new(),
                    name: "Recording interval (no GPS)".into(),
                    start_recording_time: a - offset,
                    end_recording_time: b - offset,
                    kind: SegmentKind::Unknown,
                    estimated: true,
                    competitive: true,
                    unknown: BTreeMap::new(),
                }]
            })
            .unwrap_or_default();
    }
    let mut moving: Vec<&GpsPoint> = gps
        .windows(2)
        .filter_map(|x| {
            ((haversine(&x[0], &x[1]) / (x[1].recording_time - x[0].recording_time).max(0.001)
                > 2.)
                && x[1].recording_time - x[0].recording_time <= 2.)
                .then_some(&x[0])
        })
        .collect();
    let extent = gps.first().map_or(0., |first| {
        gps.iter()
            .map(|point| haversine(first, point))
            .fold(0., f64::max)
    });
    if moving.len() < 3 || extent < 20. {
        return gps
            .first()
            .zip(gps.last())
            .map_or_else(Vec::new, |(first, last)| {
                vec![RunSegment {
                    id: SegmentId::new(),
                    name: "Stationary / noncompetitive".into(),
                    start_recording_time: first.recording_time,
                    end_recording_time: last.recording_time,
                    kind: SegmentKind::MotionTrim,
                    estimated: true,
                    competitive: false,
                    unknown: BTreeMap::new(),
                }]
            });
    };
    let first = moving.remove(0);
    let last = *moving.last().unwrap_or(&first);
    vec![RunSegment {
        id: SegmentId::new(),
        name: "Motion run".into(),
        start_recording_time: first.recording_time,
        end_recording_time: last.recording_time,
        kind: SegmentKind::MotionTrim,
        estimated: true,
        competitive: true,
        unknown: BTreeMap::new(),
    }]
}

/// Seconds kept before a detected autocross launch, so the staging and the
/// start line are inside the interval rather than right at its edge.
const AUTOCROSS_LEAD_IN_SECONDS: f64 = 3.0;

/// The detected launch, moved earlier by [`AUTOCROSS_LEAD_IN_SECONDS`] but not
/// before the first speed sample.
fn autocross_launch_time(
    dataset: &TelemetryDataset,
    source_offset: f64,
    finish_recording_time: f64,
) -> Option<f64> {
    let speed = dataset.channels.values().find(|channel| {
        matches_name(
            &channel.descriptor.name,
            &["speed", "gps_speed", "velocity"],
        ) && channel.descriptor.unit.family() == Some(crate::UnitFamily::Speed)
    })?;
    let samples = speed
        .series
        .samples
        .iter()
        .filter_map(|sample| {
            let time = sample.time - source_offset;
            if time > finish_recording_time || time < finish_recording_time - 240.0 {
                return None;
            }
            Unit::convert_value(sample.value, &speed.descriptor.unit, &Unit::MeterPerSecond)
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| TimedSample { time, value })
        })
        .collect::<Vec<_>>();
    let speed = crate::ChannelSeries::new(samples);
    let speed = speed.low_pass_hz(2.0).unwrap_or(speed);
    let mut candidates = Vec::new();
    for pair in speed.samples.windows(2) {
        let candidate = pair[1].time;
        if pair[0].value >= 4.0 || pair[1].value < 4.0 || finish_recording_time - candidate < 20.0 {
            continue;
        }
        let before = speed
            .samples
            .iter()
            .filter(|sample| sample.time >= candidate - 8.0 && sample.time < candidate - 0.5)
            .collect::<Vec<_>>();
        let after = speed
            .samples
            .iter()
            .filter(|sample| sample.time >= candidate && sample.time <= candidate + 8.0)
            .collect::<Vec<_>>();
        let spans = before
            .first()
            .zip(before.last())
            .is_some_and(|(first, last)| last.time - first.time >= 4.0)
            && after
                .first()
                .zip(after.last())
                .is_some_and(|(first, last)| last.time - first.time >= 5.0);
        if !spans || before.is_empty() || after.is_empty() {
            continue;
        }
        let low_fraction =
            before.iter().filter(|sample| sample.value < 2.0).count() as f64 / before.len() as f64;
        let high_fraction =
            after.iter().filter(|sample| sample.value >= 4.0).count() as f64 / after.len() as f64;
        let peak = after.iter().map(|sample| sample.value).fold(0.0, f64::max);
        if low_fraction >= 0.45 && high_fraction >= 0.65 && peak >= 9.0 {
            candidates.push((low_fraction * 2.0 + high_fraction + peak / 50.0, candidate));
        }
    }
    let first = speed.samples.first()?.time;
    candidates
        .into_iter()
        .max_by(|a, b| a.0.total_cmp(&b.0).then_with(|| b.1.total_cmp(&a.1)))
        .map(|(_, launch)| (launch - AUTOCROSS_LEAD_IN_SECONDS).max(first))
}
pub fn gate_crossings(points: &[GpsPoint], gate: Gate) -> Vec<f64> {
    points
        .windows(2)
        .filter_map(|p| {
            let dt = p[1].recording_time - p[0].recording_time;
            if !dt.is_finite()
                || dt <= 0.0
                || dt > 2.0
                || !gate.width_meters.is_finite()
                || gate.width_meters <= 0.0
            {
                return None;
            }
            let a = local_xy(&p[0], gate.latitude, gate.longitude);
            let b = local_xy(&p[1], gate.latitude, gate.longitude);
            let h = gate.heading_degrees.to_radians();
            let normal = (h.sin(), h.cos());
            let sa = a.0 * normal.0 + a.1 * normal.1;
            let sb = b.0 * normal.0 + b.1 * normal.1;
            if sa <= 0. && sb > 0. {
                let f = (-sa / (sb - sa)).clamp(0., 1.);
                let cross = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
                let along = cross.0 * h.cos() - cross.1 * h.sin();
                (along.abs() <= gate.width_meters / 2.).then_some(
                    p[0].recording_time + (p[1].recording_time - p[0].recording_time) * f,
                )
            } else {
                None
            }
        })
        .collect()
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ReferenceCourse {
    #[serde(default)]
    pub points: Vec<GpsPoint>,
    #[serde(default)]
    pub cumulative_meters: Vec<f64>,
    /// `valid_segments[i]` describes the leg points[i] -> points[i + 1]. GPS
    /// outages are deliberately not converted into imaginary straight lines.
    #[serde(default)]
    pub valid_segments: Vec<bool>,
}
impl ReferenceCourse {
    pub fn from_points(points: Vec<GpsPoint>) -> Self {
        let mut c = vec![0.];
        let mut valid_segments = Vec::new();
        for p in points.windows(2) {
            let valid = p[1].recording_time > p[0].recording_time
                && p[1].recording_time - p[0].recording_time <= 2.
                && p.iter().all(|p| {
                    p.latitude.is_finite()
                        && p.longitude.is_finite()
                        && p.accuracy_meters.is_none_or(|a| a <= 50.0)
                })
                && haversine(&p[0], &p[1]) / (p[1].recording_time - p[0].recording_time) < 120.0;
            valid_segments.push(valid);
            c.push(
                c.last().copied().unwrap_or(0.) + if valid { haversine(&p[0], &p[1]) } else { 0. },
            );
        }
        Self {
            points,
            cumulative_meters: c,
            valid_segments,
        }
    }
    pub fn length_meters(&self) -> f64 {
        self.cumulative_meters.last().copied().unwrap_or(0.)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProgressSample {
    pub recording_time: f64,
    pub progress: f64,
    pub confidence: f64,
}
/// Forward, continuity-constrained GPS projection onto one reference lap.
/// Initial acquisition searches the whole course (supporting partial runs).
/// Failed matches remain explicit confidence-zero samples, so consumers cannot
/// interpolate through them. Anchors narrow acquisition at the specified instant.
pub fn match_reference_course(
    course: &ReferenceCourse,
    points: &[GpsPoint],
    anchors: &[ManualAnchor],
) -> Vec<ProgressSample> {
    if course.points.len() < 2 || course.valid_segments.len() + 1 < course.points.len() {
        return vec![];
    }
    let mut out = Vec::new();
    let mut last: Option<(f64, &GpsPoint)> = None;
    for (index, p) in points.iter().enumerate() {
        if !p.recording_time.is_finite()
            || index > 0 && p.recording_time <= points[index - 1].recording_time
        {
            continue;
        }
        let anchor = anchors
            .iter()
            .filter(|a| {
                a.segment.is_none()
                    && a.reference_progress.is_finite()
                    && (a.recording_time - p.recording_time).abs() <= 0.15
            })
            .min_by(|a, b| {
                (a.recording_time - p.recording_time)
                    .abs()
                    .total_cmp(&(b.recording_time - p.recording_time).abs())
            });
        let movement = last.map_or(0.0, |(_, q)| haversine(q, p));
        let lower = last.map_or(0.0, |(s, _)| s);
        let upper = last.map_or(course.length_meters(), |(s, q)| {
            s + (movement * 2.0 + 10.0)
                .max((p.recording_time - q.recording_time) * 35.0)
                .min(500.0)
        });
        let (lo, hi) = anchor.map_or((lower, upper), |a| {
            (
                (a.reference_progress - 12.0).max(lower),
                a.reference_progress + 12.0,
            )
        });
        let first = course
            .cumulative_meters
            .partition_point(|s| *s < lo)
            .saturating_sub(1);
        let end = course
            .cumulative_meters
            .partition_point(|s| *s <= hi)
            .min(course.points.len() - 1);
        let heading_pair = if index > 0 {
            Some((&points[index.saturating_sub(3)], p))
        } else {
            points
                .get((index + 3).min(points.len() - 1))
                .map(|q| (p, q))
        };
        let mut best: Option<(f64, f64)> = None; // score, progress
        if p.latitude.is_finite()
            && p.longitude.is_finite()
            && p.accuracy_meters.is_none_or(|a| a.is_finite() && a <= 50.0)
        {
            for i in first..end {
                if !course.valid_segments[i] {
                    continue;
                }
                let (fraction, distance) =
                    project_on_leg(p, &course.points[i], &course.points[i + 1]);
                if distance > 35.0 {
                    continue;
                }
                let projected = course.cumulative_meters[i]
                    + (course.cumulative_meters[i + 1] - course.cumulative_meters[i]) * fraction;
                if projected < lo - 2.0 || projected > hi {
                    continue;
                }
                let progress = projected.max(lower);
                let heading = heading_pair.map_or(0.0, |(a, b)| {
                    heading_mismatch(a, b, &course.points[i], &course.points[i + 1])
                });
                let continuity =
                    last.map_or(0.0, |(s, _)| ((progress - s) - movement).abs() * 0.15);
                let anchor_error = anchor.map_or(0.0, |a| (progress - a.reference_progress).abs());
                let score = distance + heading * 15.0 + continuity + anchor_error;
                if best.is_none_or(|(current, _)| score < current) {
                    best = Some((score, progress));
                }
            }
        }
        if let Some((score, progress)) = best.filter(|(score, _)| *score < 45.0) {
            out.push(ProgressSample {
                recording_time: p.recording_time,
                progress,
                confidence: (1.0 - score / 50.0).clamp(0.0, 1.0),
            });
            last = Some((progress, p));
        } else {
            out.push(ProgressSample {
                recording_time: p.recording_time,
                progress: lower,
                confidence: 0.0,
            });
        }
    }
    out
}
fn project_on_leg(point: &GpsPoint, a: &GpsPoint, b: &GpsPoint) -> (f64, f64) {
    let p = local_xy(point, a.latitude, a.longitude);
    let end = local_xy(b, a.latitude, a.longitude);
    let length_squared = end.0 * end.0 + end.1 * end.1;
    if length_squared <= f64::EPSILON {
        return (0., (p.0 * p.0 + p.1 * p.1).sqrt());
    }
    let fraction = ((p.0 * end.0 + p.1 * end.1) / length_squared).clamp(0., 1.);
    let dx = p.0 - end.0 * fraction;
    let dy = p.1 - end.1 * fraction;
    (fraction, (dx * dx + dy * dy).sqrt())
}
fn heading_mismatch(previous: &GpsPoint, current: &GpsPoint, a: &GpsPoint, b: &GpsPoint) -> f64 {
    let motion = local_xy(current, previous.latitude, previous.longitude);
    let leg = local_xy(b, a.latitude, a.longitude);
    let motion_len = (motion.0 * motion.0 + motion.1 * motion.1).sqrt();
    let leg_len = (leg.0 * leg.0 + leg.1 * leg.1).sqrt();
    if motion_len < 0.2 || leg_len < 0.05 {
        return 0.;
    }
    1. - ((motion.0 * leg.0 + motion.1 * leg.1) / (motion_len * leg_len)).clamp(-1., 1.)
}
pub fn progress_at_time(samples: &[ProgressSample], time: f64) -> Option<f64> {
    interpolate_progress(samples, time, true)
}
pub fn time_at_progress(samples: &[ProgressSample], progress: f64) -> Option<f64> {
    interpolate_progress(samples, progress, false)
}
pub fn elapsed_delta(
    reference: &[ProgressSample],
    candidate: &[ProgressSample],
    progress: f64,
) -> Option<f64> {
    let rs = reference.first()?.recording_time;
    let cs = candidate.first()?.recording_time;
    Some(
        (time_at_progress(candidate, progress)? - cs)
            - (time_at_progress(reference, progress)? - rs),
    )
}
fn interpolate_progress(s: &[ProgressSample], x: f64, by_time: bool) -> Option<f64> {
    if !x.is_finite() || s.is_empty() {
        return None;
    }
    let axis = |p: &ProgressSample| {
        if by_time {
            p.recording_time
        } else {
            p.progress
        }
    };
    let value = |p: &ProgressSample| {
        if by_time {
            p.progress
        } else {
            p.recording_time
        }
    };
    let valid = |p: &ProgressSample| {
        p.recording_time.is_finite()
            && p.progress.is_finite()
            && p.confidence.is_finite()
            && p.confidence > 0.0
    };
    // Prepared samples are ordered in time and nondecreasing course progress.
    // Outside coverage is missing, never a held first/last video frame.
    if x < axis(s.first()?) || x > axis(s.last()?) {
        return None;
    }
    let i = s.partition_point(|p| axis(p) < x);
    let right = s.get(i)?;
    if (axis(right) - x).abs() < 1e-9 {
        return valid(right).then(|| value(right));
    }
    let left = s.get(i.checked_sub(1)?)?;
    let dt = right.recording_time - left.recording_time;
    let width = axis(right) - axis(left);
    if !valid(left) || !valid(right) || dt <= 0.0 || dt > 2.0 || width <= 0.0 {
        return None;
    }
    Some(value(left) + (value(right) - value(left)) * (x - axis(left)) / width)
}
pub(crate) fn haversine(a: &GpsPoint, b: &GpsPoint) -> f64 {
    let r = 6_371_000.;
    let dlat = (b.latitude - a.latitude).to_radians();
    let dlon = (b.longitude - a.longitude).to_radians();
    let q = (dlat / 2.).sin().powi(2)
        + a.latitude.to_radians().cos() * b.latitude.to_radians().cos() * (dlon / 2.).sin().powi(2);
    2. * r * q.sqrt().asin()
}
fn local_xy(p: &GpsPoint, lat: f64, lon: f64) -> (f64, f64) {
    let r = 6_371_000.;
    (
        (p.longitude - lon).to_radians() * lat.to_radians().cos() * r,
        (p.latitude - lat).to_radians() * r,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signs_and_unknown_roundtrip() {
        let id = SourceId::new();
        let p = ProjectV1 {
            video_path: "x.mp4".into(),
            video_processing: None,
            appearance: Value::Null,
            sources: vec![SourceConfig {
                id,
                name: "s".into(),
                adapter: "x".into(),
                path: "x".into(),
                alignment: crate::SourceAlignment { offset_seconds: 7. },
                settings: Value::Null,
                unknown: BTreeMap::new(),
            }],
            camera_calibration: Default::default(),
            widgets: vec![],
            export: Default::default(),
            unknown: BTreeMap::new(),
        };
        let r = recording_from_project(p, "r", 3.);
        assert_eq!(r.sources[0].alignment.offset_seconds, 10.);
        assert_eq!(
            project_from_recording(&r).sources[0]
                .alignment
                .offset_seconds,
            7.
        );
        let mut w = AnalysisWorkspace::default();
        w.unknown.insert("future".into(), Value::Bool(true));
        let d = AnalysisDocument::V1(w);
        let v = serde_json::to_string(&d).unwrap();
        assert!(v.contains("future"));
    }
    #[test]
    fn crossing_and_progress() {
        let p = vec![
            GpsPoint {
                recording_time: 0.,
                latitude: -0.0001,
                longitude: 0.,
                accuracy_meters: None,
            },
            GpsPoint {
                recording_time: 1.,
                latitude: 0.0001,
                longitude: 0.,
                accuracy_meters: None,
            },
        ];
        assert_eq!(
            gate_crossings(
                &p,
                Gate {
                    latitude: 0.,
                    longitude: 0.,
                    heading_degrees: 0.,
                    width_meters: 30.
                }
            )
            .len(),
            1
        );
        let s = vec![
            ProgressSample {
                recording_time: 0.,
                progress: 0.,
                confidence: 1.,
            },
            ProgressSample {
                recording_time: 1.,
                progress: 100.,
                confidence: 1.,
            },
        ];
        assert_eq!(progress_at_time(&s, 0.5), Some(50.));
        assert_eq!(time_at_progress(&s, 50.), Some(0.5));
    }

    fn point(t: f64, lat: f64, lon: f64) -> GpsPoint {
        GpsPoint {
            recording_time: t,
            latitude: lat,
            longitude: lon,
            accuracy_meters: None,
        }
    }

    #[test]
    fn partial_run_acquires_beyond_the_first_sixty_four_legs() {
        let gps = (0..500)
            .map(|i| point(i as f64 * 0.1, 0.0, i as f64 * 0.00001))
            .collect::<Vec<_>>();
        let course = ReferenceCourse::from_points(gps.clone());
        let matched = match_reference_course(&course, &gps[300..350], &[]);
        assert!(matched.iter().all(|p| p.confidence > 0.8));
        assert!((matched[0].progress - course.cumulative_meters[300]).abs() < 0.01);
    }

    #[test]
    fn figure_eight_does_not_jump_to_the_other_crossing_branch() {
        let gps = (0..=400)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 400.0;
                point(i as f64 * 0.1, a.sin() * 0.001, (2.0 * a).sin() * 0.0005)
            })
            .collect::<Vec<_>>();
        let course = ReferenceCourse::from_points(gps.clone());
        let matched = match_reference_course(&course, &gps, &[]);
        for (p, s) in matched.iter().zip(&course.cumulative_meters) {
            assert!(
                p.confidence > 0.0 && (p.progress - s).abs() < 3.0,
                "{p:?} expected {s}"
            );
        }
        let mut bad = gps.clone();
        bad[80].latitude += 1.0;
        let matched = match_reference_course(&course, &bad, &[]);
        assert_eq!(matched[80].confidence, 0.0);
        assert_eq!(progress_at_time(&matched, 8.05), None);
    }

    #[test]
    fn workspace_paths_and_unknown_fields_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(dir.path()).unwrap();
        let image = base.join("track.png");
        let mut workspace = AnalysisWorkspace {
            settings: serde_json::json!({"ui":{"dock":{"Map":{"settings":{"imagery":{"image_path":image,"future":123}}}}}}),
            ..Default::default()
        };
        workspace
            .unknown
            .insert("future_workspace".into(), Value::Bool(true));
        workspace
            .envelope_unknown
            .insert("future_envelope".into(), Value::Bool(true));
        let document = AnalysisDocument::V1(workspace);
        let file = base.join("session.race-analysis.json");
        document.save_atomic(&file).unwrap();
        let json: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(
            json["workspace"]["settings"]["ui"]["dock"]["Map"]["settings"]["imagery"]["image_path"],
            "track.png"
        );
        assert_eq!(
            serde_json::to_value(AnalysisDocument::load(file).unwrap()).unwrap(),
            serde_json::to_value(document).unwrap()
        );
    }

    #[test]
    fn interpolation_endpoints_plateau_and_gap_are_deterministic() {
        let samples = vec![
            ProgressSample {
                recording_time: 10.,
                progress: 0.,
                confidence: 1.,
            },
            ProgressSample {
                recording_time: 11.,
                progress: 0.,
                confidence: 1.,
            },
            ProgressSample {
                recording_time: 12.,
                progress: 20.,
                confidence: 1.,
            },
            ProgressSample {
                recording_time: 15.,
                progress: 40.,
                confidence: 1.,
            },
        ];
        assert_eq!(progress_at_time(&samples, 9.), None);
        assert_eq!(progress_at_time(&samples, 10.), Some(0.));
        assert_eq!(time_at_progress(&samples, 41.), None);
        assert_eq!(time_at_progress(&samples, 30.), None);
        assert_eq!(time_at_progress(&samples, 0.), Some(10.));
        assert_eq!(progress_at_time(&samples, 13.), None); // crosses a >2s gap
        assert!(
            progress_at_time(
                &[ProgressSample {
                    recording_time: 0.,
                    progress: 0.,
                    confidence: f64::NAN
                }],
                0.
            )
            .is_none()
        );
    }

    #[test]
    fn elapsed_delta_uses_segment_relative_clocks() {
        let reference = vec![
            ProgressSample {
                recording_time: 100.,
                progress: 0.,
                confidence: 1.,
            },
            ProgressSample {
                recording_time: 110.,
                progress: 100.,
                confidence: 1.,
            },
        ];
        let candidate = vec![
            ProgressSample {
                recording_time: 7.,
                progress: 0.,
                confidence: 1.,
            },
            ProgressSample {
                recording_time: 15.,
                progress: 100.,
                confidence: 1.,
            },
        ];
        // Dense samples represent a real continuous trace; the sparse endpoint
        // fixture above is a dropout and must not be interpolated across.
        assert_eq!(elapsed_delta(&reference, &candidate, 50.), None);
        let dense = |start: f64, duration: f64| {
            (0..=10)
                .map(|i| ProgressSample {
                    recording_time: start + duration * i as f64 / 10.,
                    progress: i as f64 * 10.,
                    confidence: 1.,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            elapsed_delta(&dense(100., 10.), &dense(7., 8.), 50.),
            Some(-1.)
        );
    }

    #[test]
    fn gate_crossing_rejects_dropout_and_respects_direction() {
        let gate = Gate {
            latitude: 0.,
            longitude: 0.,
            heading_degrees: 0.,
            width_meters: 30.,
        };
        assert_eq!(
            gate_crossings(&[point(0., -0.0001, 0.), point(3., 0.0001, 0.)], gate),
            Vec::<f64>::new()
        );
        assert_eq!(
            gate_crossings(&[point(0., -0.0001, 0.), point(1., 0.0001, 0.)], gate).len(),
            1
        );
        assert!(gate_crossings(&[point(0., 0.0001, 0.), point(1., -0.0001, 0.)], gate).is_empty());
    }

    #[test]
    fn matching_empty_and_dropout_courses_is_safe() {
        assert!(match_reference_course(&ReferenceCourse::default(), &[], &[]).is_empty());
        let course = ReferenceCourse::from_points(vec![
            point(0., 0., 0.),
            point(1., 0., 0.0001),
            point(2., 0., 0.0002),
        ]);
        let run = vec![
            point(0., 0., 0.),
            point(1., 0., 0.00005),
            point(5., 0., 0.0001),
            point(6., 0., 0.0002),
        ];
        let matched = match_reference_course(&course, &run, &[]);
        assert!(matched.windows(2).all(|w| w[1].progress >= w[0].progress));
    }

    #[test]
    fn speed_distance_integrates_kmh_in_meters() {
        let source = SourceId::new();
        let mut dataset = TelemetryDataset {
            source_id: source,
            ..Default::default()
        };
        let id = crate::ChannelId::new();
        dataset.insert(crate::TelemetryChannel {
            descriptor: crate::ChannelDescriptor {
                id,
                name: "speed".into(),
                quantity: crate::Quantity::Speed,
                unit: Unit::KilometerPerHour,
                interpolation: crate::Interpolation::Linear,
                description: None,
            },
            series: crate::ChannelSeries::new(vec![
                TimedSample {
                    time: 0.,
                    value: 36.,
                },
                TimedSample {
                    time: 1.,
                    value: 36.,
                },
            ]),
        });
        let recording = Recording {
            id: RecordingId::new(),
            name: "test".into(),
            sources: vec![],
            primary_source: source,
            video_path: None,
            video_processing: None,
            video_offset_seconds: 0.,
            segments: vec![],
            overlay_snapshot: None,
            unknown: BTreeMap::new(),
        };
        let distance = traveled_distance(&dataset, &recording, &[]);
        assert!((distance.samples.last().unwrap().value - 10.).abs() < 1e-9);
        let channel = dataset.channels.get_mut(&id).unwrap();
        channel.descriptor.name = "distance".into();
        channel.descriptor.unit = Unit::Meter;
        channel.series = crate::ChannelSeries::new(
            [0.0, 10.0, 20.0, 0.0, 10.0]
                .into_iter()
                .enumerate()
                .map(|(i, value)| TimedSample {
                    time: i as f64,
                    value,
                })
                .collect(),
        );
        let distance = traveled_distance(&dataset, &recording, &[]);
        assert_eq!(
            distance.samples.iter().map(|p| p.value).collect::<Vec<_>>(),
            vec![0.0, 10.0, 20.0, 20.0, 30.0]
        );
        assert_eq!(distance.gaps, vec![(2.0, 3.0)]);
    }

    #[test]
    fn autocross_segment_uses_sustained_launch_and_first_finish_boundary() {
        let source = SourceId::new();
        let mut dataset = TelemetryDataset {
            source_id: source,
            laps: vec![
                crate::TelemetryLap {
                    number: 0,
                    start_time: 0.0,
                    end_time: 90.0,
                    lap_type: "out".into(),
                },
                crate::TelemetryLap {
                    number: 1,
                    start_time: 90.0,
                    end_time: 105.0,
                    lap_type: "in".into(),
                },
            ],
            ..Default::default()
        };
        dataset.insert(crate::TelemetryChannel {
            descriptor: crate::ChannelDescriptor {
                id: crate::ChannelId::new(),
                name: "gps_speed".into(),
                quantity: crate::Quantity::Speed,
                unit: Unit::MeterPerSecond,
                interpolation: crate::Interpolation::Linear,
                description: None,
            },
            series: crate::ChannelSeries::new(
                (0..=2_100)
                    .map(|index| {
                        let time = index as f64 * 0.05;
                        let value = if (8.0..14.0).contains(&time) {
                            2.5
                        } else if (30.0..90.0).contains(&time) {
                            (4.0 + (time - 30.0) * 2.0).min(24.0)
                        } else {
                            0.0
                        };
                        TimedSample { time, value }
                    })
                    .collect(),
            ),
        });
        let recording = Recording {
            id: RecordingId::new(),
            name: "autocross".into(),
            sources: vec![],
            primary_source: source,
            video_path: None,
            video_processing: None,
            video_offset_seconds: 0.0,
            segments: vec![],
            overlay_snapshot: None,
            unknown: BTreeMap::new(),
        };
        let segments = auto_segments(&dataset, &recording, &[]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].kind, SegmentKind::Autocross);
        let lead_in = 30.0 - AUTOCROSS_LEAD_IN_SECONDS;
        assert!((segments[0].start_recording_time - lead_in).abs() < 0.2);
        assert_eq!(segments[0].end_recording_time, 90.0);
        assert!(segments[0].competitive);
    }
}
