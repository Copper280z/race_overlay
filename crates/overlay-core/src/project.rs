use crate::{ChannelBinding, SourceAlignment, SourceId, WidgetId};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("could not read project: {0}")]
    Io(#[from] io::Error),
    #[error("invalid project JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported project version {0}")]
    UnsupportedVersion(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl NormalizedRect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x: x.clamp(0., 1.),
            y: y.clamp(0., 1.),
            width: width.clamp(0., 1.),
            height: height.clamp(0., 1.),
        }
    }
}
impl Default for NormalizedRect {
    fn default() -> Self {
        Self::new(0.05, 0.05, 0.25, 0.12)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraCalibration {
    /// Rotation from source sensor coordinates to vehicle coordinates.
    #[serde(default = "identity_matrix")]
    pub sensor_to_vehicle: [[f64; 3]; 3],
    #[serde(default)]
    pub accelerometer_bias: [f64; 3],
    #[serde(default)]
    pub gyroscope_bias: [f64; 3],
    #[serde(default = "default_low_pass")]
    pub low_pass_hz: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}
fn identity_matrix() -> [[f64; 3]; 3] {
    [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
}
fn default_low_pass() -> f64 {
    8.0
}
impl Default for CameraCalibration {
    fn default() -> Self {
        Self {
            sensor_to_vehicle: identity_matrix(),
            accelerometer_bias: [0.; 3],
            gyroscope_bias: [0.; 3],
            low_pass_hz: 8.,
            notes: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceConfig {
    pub id: SourceId,
    pub name: String,
    /// Adapter identifier, e.g. `generic_csv`.
    pub adapter: String,
    pub path: PathBuf,
    #[serde(default)]
    pub alignment: SourceAlignment,
    /// Adapter-owned settings. Kept losslessly even if this build does not know them.
    #[serde(default)]
    pub settings: Value,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetConfig {
    pub id: WidgetId,
    /// Plugin-friendly stable string such as `race.speedometer`.
    pub kind: String,
    #[serde(default)]
    pub rect: NormalizedRect,
    #[serde(default)]
    pub bindings: Vec<ChannelBinding>,
    /// Widget-owned style and behavior; deliberately schema-free.
    #[serde(default)]
    pub style: Value,
    #[serde(default)]
    pub settings: Value,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ExportConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fps: Option<f64>,
    #[serde(default)]
    pub settings: Value,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectV1 {
    pub video_path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_processing: Option<crate::VideoProcessingConfig>,
    /// Project-wide visual theme. The renderer owns the schema so new palette
    /// fields can be added without changing the stable project envelope.
    #[serde(default)]
    pub appearance: Value,
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    #[serde(default)]
    pub camera_calibration: CameraCalibration,
    #[serde(default)]
    pub widgets: Vec<WidgetConfig>,
    #[serde(default)]
    pub export: ExportConfig,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, Value>,
}
impl ProjectV1 {
    pub fn new(video_path: impl Into<PathBuf>) -> Self {
        Self {
            video_path: video_path.into(),
            video_processing: None,
            // Keep the canonical default explicit in newly-created projects.
            // Individual palette values remain optional and are resolved by
            // the renderer from this preset.
            appearance: serde_json::json!({"preset": "race_dark"}),
            sources: vec![],
            camera_calibration: CameraCalibration::default(),
            widgets: vec![],
            export: ExportConfig::default(),
            unknown: BTreeMap::new(),
        }
    }
}

/// Explicit version envelope. New readers must reject versions they cannot preserve.
#[derive(Clone, Debug, PartialEq)]
pub enum ProjectDocument {
    V1(ProjectV1),
}
impl Serialize for ProjectDocument {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Envelope<'a> {
            version: u32,
            project: &'a ProjectV1,
        }
        match self {
            Self::V1(project) => Envelope {
                version: 1,
                project,
            }
            .serialize(serializer),
        }
    }
}
impl<'de> Deserialize<'de> for ProjectDocument {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Envelope {
            version: u32,
            project: ProjectV1,
        }
        let envelope = Envelope::deserialize(deserializer)?;
        match envelope.version {
            1 => Ok(Self::V1(envelope.project)),
            version => Err(de::Error::custom(format!(
                "unsupported project version {version}"
            ))),
        }
    }
}
impl From<ProjectV1> for ProjectDocument {
    fn from(p: ProjectV1) -> Self {
        Self::V1(p)
    }
}
impl ProjectDocument {
    pub fn v1(&self) -> &ProjectV1 {
        match self {
            Self::V1(v) => v,
        }
    }
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ProjectError> {
        let value: Value = serde_json::from_slice(&fs::read(path)?)?;
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or(ProjectError::UnsupportedVersion(0))?;
        if version != 1 {
            return Err(ProjectError::UnsupportedVersion(version as u32));
        }
        Ok(serde_json::from_value(value)?)
    }
    /// Write to a sibling temporary file and rename it, so an existing project is never half-written.
    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let stem = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project");
        let tmp = parent.join(format!(".{stem}.{}.tmp", std::process::id()));
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        // `rename` replaces atomically on Unix. Windows requires the existing
        // destination to be moved out of the way first; retain a sibling
        // backup until the new file is in place so a failed save is recoverable.
        #[cfg(windows)]
        if path.exists() {
            let backup = parent.join(format!(".{stem}.{}.bak", std::process::id()));
            fs::rename(path, &backup)?;
            if let Err(error) = fs::rename(&tmp, path) {
                let _ = fs::rename(&backup, path);
                let _ = fs::remove_file(&tmp);
                return Err(ProjectError::Io(error));
            }
            fs::remove_file(backup)?;
            return Ok(());
        }
        fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn project_round_trips_unknown_fields_and_atomic_save() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("x.json");
        let mut p = ProjectV1::new("video.mp4");
        p.unknown.insert("future".into(), Value::Bool(true));
        let doc = ProjectDocument::from(p);
        doc.save_atomic(&path).unwrap();
        assert_eq!(
            ProjectDocument::load(&path).unwrap().v1().unknown["future"],
            true
        );
    }
    #[test]
    fn rect_is_normalized() {
        assert_eq!(
            NormalizedRect::new(-1., 2., 3., -1.),
            NormalizedRect {
                x: 0.,
                y: 1.,
                width: 1.,
                height: 0.
            }
        );
    }

    #[test]
    fn appearance_round_trips_as_project_owned_json() {
        let mut project = ProjectV1::new("video.mp4");
        project.appearance = serde_json::json!({
            "preset": "light",
            "accent": [10, 120, 200],
            "future_palette_field": {"kept": true}
        });
        let encoded = serde_json::to_value(ProjectDocument::from(project)).unwrap();
        let decoded: ProjectDocument = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            decoded.v1().appearance["future_palette_field"]["kept"],
            true
        );
    }
}
