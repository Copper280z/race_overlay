//! Import adapters. Their configuration is JSON-owned by the project, while the
//! runtime API only returns the common [`TelemetryDataset`] representation.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("unsupported source format: {0}")]
    Unsupported(String),
    #[error("could not read telemetry: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid telemetry: {0}")]
    Invalid(String),
    #[error("adapter configuration: {0}")]
    Config(String),
}

pub trait TelemetrySourceAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn load(
        &self,
        source_id: SourceId,
        path: &Path,
        settings: &Value,
    ) -> Result<TelemetryDataset, AdapterError>;
}
#[derive(Default)]
pub struct AdapterRegistry {
    adapters: BTreeMap<String, Arc<dyn TelemetrySourceAdapter>>,
}
impl AdapterRegistry {
    pub fn with_builtins() -> Self {
        let mut r = Self::default();
        r.register(GenericCsvAdapter);
        r.register(SyntheticAdapter);
        r.register(Insta360Adapter);
        r.register(AimXrkAdapter);
        r
    }
    pub fn register<A: TelemetrySourceAdapter + 'static>(&mut self, adapter: A) {
        self.adapters
            .insert(adapter.id().to_owned(), Arc::new(adapter));
    }
    pub fn get(&self, id: &str) -> Option<&Arc<dyn TelemetrySourceAdapter>> {
        self.adapters.get(id)
    }
    pub fn load(
        &self,
        adapter: &str,
        source_id: SourceId,
        path: &Path,
        settings: &Value,
    ) -> Result<TelemetryDataset, AdapterError> {
        self.get(adapter)
            .ok_or_else(|| AdapterError::Unsupported(adapter.into()))?
            .load(source_id, path, settings)
    }
}

/// AiM MyChron XRK recordings, decoded into the common telemetry model.
pub struct AimXrkAdapter;
impl TelemetrySourceAdapter for AimXrkAdapter {
    fn id(&self) -> &'static str {
        "aim_xrk"
    }

    fn load(
        &self,
        source_id: SourceId,
        path: &Path,
        _: &Value,
    ) -> Result<TelemetryDataset, AdapterError> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension != "xrk" {
            return Err(AdapterError::Unsupported(
                "AiM inputs must be original .xrk recordings".into(),
            ));
        }
        let recording = crate::xrk::parse(&fs::read(path)?)
            .map_err(|error| AdapterError::Invalid(format!("XRK: {error}")))?;
        let mut dataset = TelemetryDataset {
            source_id,
            metadata: recording.metadata,
            laps: recording
                .laps
                .iter()
                .map(|lap| TelemetryLap {
                    number: lap.number,
                    start_time: lap.start,
                    end_time: lap.end,
                    lap_type: lap.lap_type.clone(),
                })
                .collect(),
            ..Default::default()
        };
        dataset.metadata.insert(
            "unconfigured_packets".into(),
            (recording.diagnostics.unconfigured_s
                + recording.diagnostics.unconfigured_m
                + recording.diagnostics.unconfigured_g)
                .to_string(),
        );
        dataset.metadata.insert(
            "unrecognized_bytes".into(),
            recording.diagnostics.unrecognized_bytes.to_string(),
        );

        let mut used_names = BTreeMap::<String, usize>::new();
        for channel in recording.channels {
            if channel.samples.is_empty() {
                continue;
            }
            let base = canonical_xrk_name(&channel.long_name);
            let occurrence = used_names.entry(base.clone()).or_default();
            *occurrence += 1;
            let name = if *occurrence == 1 {
                base
            } else {
                format!("{base}_{}", *occurrence)
            };
            let (quantity, unit) = xrk_quantity_unit(&name, &channel.unit);
            let sample_rate = if channel.sample_period_ms > 0 {
                format!("; nominal {} Hz", 1000.0 / channel.sample_period_ms as f64)
            } else {
                String::new()
            };
            let description = format!(
                "AiM XRK '{}' ({}, index {}, source type {}, source channel {}, decoder {}{})",
                channel.long_name,
                channel.short_name,
                channel.index,
                channel.source_type,
                channel.source_channel_id,
                channel.decoder,
                sample_rate,
            );
            dataset.insert(TelemetryChannel {
                descriptor: ChannelDescriptor {
                    id: ChannelId::for_source_name(source_id, &name),
                    name,
                    quantity,
                    unit,
                    interpolation: if channel.interpolate {
                        Interpolation::Linear
                    } else {
                        Interpolation::Hold
                    },
                    description: Some(description),
                },
                series: ChannelSeries::new(
                    channel
                        .samples
                        .into_iter()
                        .map(|(time, value)| TimedSample { time, value })
                        .collect(),
                ),
            });
        }
        add_lap_channels(&mut dataset);
        if dataset.channels.is_empty() {
            return Err(AdapterError::Invalid(
                "XRK contained definitions but no decoded samples".into(),
            ));
        }
        Ok(dataset)
    }
}

fn canonical_xrk_name(original: &str) -> String {
    match original.to_ascii_lowercase().as_str() {
        "exhaust temp" => return "exhaust_temperature".into(),
        "water temp" => return "water_temperature".into(),
        "accelerometerx" => return "accelerometer_x".into(),
        "accelerometery" => return "accelerometer_y".into(),
        "accelerometerz" => return "accelerometer_z".into(),
        "gyrox" => return "gyro_x".into(),
        "gyroy" => return "gyro_y".into(),
        "gyroz" => return "gyro_z".into(),
        "internal batt" => return "internal_battery_voltage".into(),
        "external voltage" => return "external_battery_voltage".into(),
        "logger temperature" => return "logger_temperature".into(),
        "steering angle" => return "steering_angle".into(),
        "calculated_gear" => return "gear".into(),
        "rpm" => return "rpm".into(),
        _ => {}
    }
    let mut result = String::new();
    let mut underscore = false;
    for character in original.chars() {
        if character.is_ascii_alphanumeric() {
            result.push(character.to_ascii_lowercase());
            underscore = false;
        } else if !result.is_empty() && !underscore {
            result.push('_');
            underscore = true;
        }
    }
    while result.ends_with('_') {
        result.pop();
    }
    if result.is_empty() {
        "channel".into()
    } else {
        result
    }
}

fn xrk_quantity_unit(name: &str, raw_unit: &str) -> (Quantity, Unit) {
    let unit = match raw_unit {
        "g" => Unit::StandardGravity,
        "deg" => Unit::Degree,
        "deg/s" => Unit::DegreePerSecond,
        "m/s" => Unit::MeterPerSecond,
        "km/h" => Unit::KilometerPerHour,
        "m" => Unit::Meter,
        "C" => Unit::Celsius,
        "Pa" => Unit::Pascal,
        "bar" => Unit::Bar,
        "psi" => Unit::Psi,
        "V" => Unit::Volt,
        "A" => Unit::Ampere,
        "rpm" => Unit::RevolutionsPerMinute,
        "ms" => Unit::Millisecond,
        "%" => Unit::Percent,
        "" | "gear" => Unit::Unitless,
        other => Unit::Custom(other.into()),
    };
    let quantity = if name.contains("acceler") {
        Quantity::Acceleration
    } else if name.contains("gyro") || name.contains("yaw_rate") {
        Quantity::AngularVelocity
    } else if name.contains("speed") {
        Quantity::Speed
    } else if name.contains("latitude")
        || name.contains("longitude")
        || name.contains("steering_angle")
    {
        Quantity::Position
    } else if name.contains("altitude") {
        Quantity::Altitude
    } else if name.contains("temperature") {
        Quantity::Temperature
    } else if name.contains("pressure") || matches!(&unit, Unit::Pascal | Unit::Bar | Unit::Psi) {
        Quantity::Pressure
    } else if name.contains("voltage") || matches!(&unit, Unit::Volt) {
        Quantity::Voltage
    } else if matches!(&unit, Unit::Ampere) {
        Quantity::Current
    } else if name == "rpm" || matches!(&unit, Unit::RevolutionsPerMinute) {
        Quantity::Rpm
    } else if name.contains("lap")
        || name.contains("predictive_time")
        || name.contains("best_today_diff")
    {
        Quantity::LapTime
    } else if matches!(&unit, Unit::Percent) {
        Quantity::Percent
    } else {
        Quantity::Generic
    };
    (quantity, unit)
}

fn add_lap_channels(dataset: &mut TelemetryDataset) {
    if dataset.laps.is_empty() {
        return;
    }
    let number = dataset
        .laps
        .iter()
        .map(|lap| TimedSample {
            time: lap.start_time.max(0.0),
            value: lap.number as f64,
        })
        .collect();
    let lap_time = dataset
        .laps
        .iter()
        .flat_map(|lap| {
            [
                TimedSample {
                    time: lap.start_time.max(0.0),
                    value: 0.0,
                },
                TimedSample {
                    time: lap.end_time,
                    value: lap.end_time - lap.start_time,
                },
            ]
        })
        .collect();
    for (name, quantity, unit, interpolation, samples) in [
        (
            "lap_number",
            Quantity::Generic,
            Unit::Unitless,
            Interpolation::Hold,
            number,
        ),
        (
            "lap_time",
            Quantity::LapTime,
            Unit::Second,
            Interpolation::Linear,
            lap_time,
        ),
    ] {
        dataset.insert(TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::for_source_name(dataset.source_id, name),
                name: name.into(),
                quantity,
                unit,
                interpolation,
                description: Some("Derived from AiM XRK lap markers".into()),
            },
            series: ChannelSeries::new(samples),
        });
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ColumnSelector {
    Index(usize),
    Name(String),
}
impl ColumnSelector {
    fn index(&self, headers: &[String]) -> Result<usize, AdapterError> {
        match self {
            Self::Index(i) if *i < headers.len() => Ok(*i),
            Self::Index(i) => Err(AdapterError::Config(format!(
                "column index {i} is out of range"
            ))),
            Self::Name(n) => headers
                .iter()
                .position(|x| x == n)
                .ok_or_else(|| AdapterError::Config(format!("column '{n}' was not found"))),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    #[default]
    Seconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}
impl TimeUnit {
    fn seconds(self, n: f64) -> f64 {
        n / match self {
            Self::Seconds => 1.,
            Self::Milliseconds => 1e3,
            Self::Microseconds => 1e6,
            Self::Nanoseconds => 1e9,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CsvColumnConfig {
    pub column: ColumnSelector,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub quantity: Option<Quantity>,
    #[serde(default)]
    pub unit: Option<Unit>,
    #[serde(default)]
    pub interpolation: Interpolation,
    #[serde(default)]
    pub gap_seconds: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CsvConfig {
    /// Zero-based record row containing headers. `None` means generated column names.
    pub header_row: Option<usize>,
    pub time_column: Option<ColumnSelector>,
    #[serde(default)]
    pub time_unit: TimeUnit,
    #[serde(default)]
    pub sample_rate_hz: Option<f64>,
    #[serde(default)]
    pub columns: Vec<CsvColumnConfig>,
    #[serde(default)]
    pub gap_seconds: Option<f64>,
}
impl Default for CsvConfig {
    fn default() -> Self {
        Self {
            header_row: Some(0),
            time_column: Some(ColumnSelector::Index(0)),
            time_unit: TimeUnit::Seconds,
            sample_rate_hz: None,
            columns: vec![],
            gap_seconds: None,
        }
    }
}
pub struct GenericCsvAdapter;
impl GenericCsvAdapter {
    fn delimiter(text: &str) -> u8 {
        let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        [b',', b';', b'\t']
            .into_iter()
            .max_by_key(|d| line.as_bytes().iter().filter(|c| **c == *d).count())
            .unwrap_or(b',')
    }
}
impl TelemetrySourceAdapter for GenericCsvAdapter {
    fn id(&self) -> &'static str {
        "generic_csv"
    }
    fn load(
        &self,
        source_id: SourceId,
        path: &Path,
        settings: &Value,
    ) -> Result<TelemetryDataset, AdapterError> {
        let cfg: CsvConfig = serde_json::from_value(settings.clone()).unwrap_or_default();
        if cfg.time_column.is_none() && !matches!(cfg.sample_rate_hz,Some(n) if n>0.) {
            return Err(AdapterError::Config(
                "choose a time column or a positive sample_rate_hz".into(),
            ));
        }
        let text = fs::read_to_string(path)?;
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .delimiter(Self::delimiter(&text))
            .flexible(true)
            .from_reader(text.as_bytes());
        let records: Vec<Vec<String>> = reader
            .records()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AdapterError::Invalid(e.to_string()))?
            .into_iter()
            .map(|r| r.iter().map(str::to_owned).collect())
            .collect();
        let header_at = cfg.header_row.unwrap_or(usize::MAX);
        let width = records.iter().map(Vec::len).max().unwrap_or(0);
        let headers = if header_at < records.len() {
            records[header_at].clone()
        } else {
            (0..width).map(|i| format!("column_{i}")).collect()
        };
        let time_index = cfg
            .time_column
            .as_ref()
            .map(|c| c.index(&headers))
            .transpose()?;
        let selected = if cfg.columns.is_empty() {
            (0..width)
                .filter(|i| Some(*i) != time_index)
                .map(|i| CsvColumnConfig {
                    column: ColumnSelector::Index(i),
                    name: None,
                    quantity: None,
                    unit: None,
                    interpolation: Interpolation::Linear,
                    gap_seconds: None,
                })
                .collect()
        } else {
            cfg.columns.clone()
        };
        let mut built: Vec<(ChannelDescriptor, Vec<TimedSample>, usize, Option<f64>)> = vec![];
        for c in selected {
            let i = c.column.index(&headers)?;
            let name = c.name.unwrap_or_else(|| {
                headers
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("column_{i}"))
            });
            let gap = c.gap_seconds.or(cfg.gap_seconds);
            built.push((
                ChannelDescriptor {
                    id: ChannelId::for_source_name(source_id, &name),
                    name,
                    quantity: c.quantity.unwrap_or(Quantity::Generic),
                    unit: c.unit.unwrap_or(Unit::Unitless),
                    interpolation: c.interpolation,
                    description: None,
                },
                vec![],
                i,
                gap,
            ));
        }
        let start = if header_at < records.len() {
            header_at + 1
        } else {
            0
        };
        for (row_no, row) in records.into_iter().enumerate().skip(start) {
            let time = match time_index {
                Some(i) => row
                    .get(i)
                    .and_then(|v| v.trim().parse::<f64>().ok())
                    .map(|x| cfg.time_unit.seconds(x)),
                None => cfg.sample_rate_hz.map(|hz| (row_no - start) as f64 / hz),
            };
            let Some(time) = time else { continue };
            for (_, samples, i, _) in &mut built {
                if let Some(Ok(value)) = row.get(*i).map(|v| v.trim().parse::<f64>()) {
                    samples.push(TimedSample { time, value });
                }
            }
        }
        let mut out = TelemetryDataset {
            source_id,
            channels: BTreeMap::new(),
            ..Default::default()
        };
        for (d, s, _, gap) in built {
            out.insert(TelemetryChannel {
                descriptor: d,
                series: match gap {
                    Some(seconds) => ChannelSeries::new(s).with_gap(seconds),
                    None => ChannelSeries::new(s),
                },
            });
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyntheticConfig {
    #[serde(default = "synthetic_duration")]
    pub duration_seconds: f64,
    #[serde(default = "synthetic_rate")]
    pub sample_rate_hz: f64,
}
fn synthetic_duration() -> f64 {
    120.
}
fn synthetic_rate() -> f64 {
    50.
}
impl Default for SyntheticConfig {
    fn default() -> Self {
        Self {
            duration_seconds: 120.,
            sample_rate_hz: 50.,
        }
    }
}
pub struct SyntheticAdapter;
impl TelemetrySourceAdapter for SyntheticAdapter {
    fn id(&self) -> &'static str {
        "synthetic"
    }
    fn load(
        &self,
        source_id: SourceId,
        _: &Path,
        settings: &Value,
    ) -> Result<TelemetryDataset, AdapterError> {
        let c: SyntheticConfig = serde_json::from_value(settings.clone()).unwrap_or_default();
        if c.duration_seconds <= 0. || c.sample_rate_hz <= 0. {
            return Err(AdapterError::Config(
                "duration and sample rate must be positive".into(),
            ));
        };
        let n = (c.duration_seconds * c.sample_rate_hz).round() as usize;
        let mut out = TelemetryDataset {
            source_id,
            channels: BTreeMap::new(),
            ..Default::default()
        };
        let specs = [
            ("speed", Quantity::Speed, Unit::KilometerPerHour),
            (
                "longitudinal_g",
                Quantity::Acceleration,
                Unit::StandardGravity,
            ),
            ("lateral_g", Quantity::Acceleration, Unit::StandardGravity),
            ("rpm", Quantity::Rpm, Unit::RevolutionsPerMinute),
        ];
        for (which, (name, q, u)) in specs.into_iter().enumerate() {
            let s = (0..=n)
                .map(|i| {
                    let t = i as f64 / c.sample_rate_hz;
                    let v = match which {
                        0 => 130.0 + 40.0 * (t * 0.13).sin() + 8.0 * (t * 0.71).sin(),
                        1 => 0.35 * (t * 0.41).sin(),
                        2 => 0.85 * (t * 0.19).sin(),
                        _ => 6200.0 + 1800.0 * (t * 0.13).sin(),
                    };
                    TimedSample { time: t, value: v }
                })
                .collect();
            out.insert(TelemetryChannel {
                descriptor: ChannelDescriptor {
                    id: ChannelId::for_source_name(source_id, name),
                    name: name.into(),
                    quantity: q,
                    unit: u,
                    interpolation: Interpolation::Linear,
                    description: Some("Deterministic synthetic motorsport signal".into()),
                },
                series: ChannelSeries::new(s),
            });
        }
        let gear = (0..=n)
            .map(|i| {
                let t = i as f64 / c.sample_rate_hz;
                // A deterministic stepped shift pattern, deliberately Hold-interpolated.
                let gear = ((t / 7.5).floor() as i32 % 6 + 1) as f64;
                TimedSample {
                    time: t,
                    value: gear,
                }
            })
            .collect();
        out.insert(TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::for_source_name(source_id, "gear"),
                name: "gear".into(),
                quantity: Quantity::Generic,
                unit: Unit::Unitless,
                interpolation: Interpolation::Hold,
                description: Some("Deterministic synthetic discrete gear".into()),
            },
            series: ChannelSeries::new(gear),
        });
        let lap_time = (0..=n)
            .map(|i| {
                let t = i as f64 / c.sample_rate_hz;
                TimedSample {
                    time: t,
                    value: t % 60.0,
                }
            })
            .collect();
        out.insert(TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::for_source_name(source_id, "lap_time"),
                name: "lap_time".into(),
                quantity: Quantity::LapTime,
                unit: Unit::Second,
                interpolation: Interpolation::Linear,
                description: Some("Deterministic synthetic elapsed lap time".into()),
            },
            series: ChannelSeries::new(lap_time),
        });
        Ok(out)
    }
}

/// Binary Insta360 IMU extraction via the pinned `telemetry-parser` revision.
/// Upstream's extension gate excludes `.lrv`, so LRV input is passed to its
/// detector with an `.insv` spelling while retaining the original bytes.
pub struct Insta360Adapter;
impl TelemetrySourceAdapter for Insta360Adapter {
    fn id(&self) -> &'static str {
        "insta360"
    }
    fn load(
        &self,
        source_id: SourceId,
        path: &Path,
        _: &Value,
    ) -> Result<TelemetryDataset, AdapterError> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "insv" && ext != "lrv" {
            return Err(AdapterError::Unsupported(
                "Insta360 inputs must be .insv or .lrv".into(),
            ));
        }
        let mut file = fs::File::open(path)?;
        let size = file.metadata()?.len() as usize;
        let detector_path = if ext == "lrv" {
            path.with_extension("insv")
        } else {
            path.to_owned()
        };
        let input = telemetry_parser::Input::from_stream(
            &mut file,
            size,
            detector_path,
            |_| {},
            Arc::new(AtomicBool::new(false)),
        )
        .map_err(|e| AdapterError::Invalid(e.to_string()))?;
        let map = input
            .samples
            .as_ref()
            .and_then(|s| s.iter().find_map(|sample| sample.tag_map.as_ref()))
            .ok_or_else(|| {
                AdapterError::Invalid("Insta360 file has no telemetry samples".into())
            })?;
        dataset_from_insta360_map(source_id, map)
    }
}
fn dataset_from_insta360_map(
    source_id: SourceId,
    map: &telemetry_parser::tags_impl::GroupedTagMap,
) -> Result<TelemetryDataset, AdapterError> {
    use telemetry_parser::tags_impl::{GroupId, TagId, TagValue};
    let tag = |group, id| {
        map.get(&group)
            .and_then(|tags| tags.get(&id))
            .map(|tag| &tag.value)
    };
    let accel = match tag(GroupId::Accelerometer, TagId::Data) {
        Some(TagValue::Vec_TimeVector3_f64(v)) => v.get(),
        _ => {
            return Err(AdapterError::Invalid(
                "Insta360 file contains no accelerometer samples".into(),
            ));
        }
    };
    let gyro = match tag(GroupId::Gyroscope, TagId::Data) {
        Some(TagValue::Vec_TimeVector3_f64(v)) => v.get(),
        _ => {
            return Err(AdapterError::Invalid(
                "Insta360 file contains no gyroscope samples".into(),
            ));
        }
    };
    let scale = |group| match tag(group, TagId::Scale) {
        Some(TagValue::f64(v)) => *v.get(),
        _ => 1.0,
    };
    let orientation = |group| match tag(group, TagId::Orientation) {
        Some(TagValue::String(v)) => v.get().clone(),
        _ => "XYZ (unmodified source axes)".into(),
    };
    let mut out = TelemetryDataset {
        source_id,
        channels: BTreeMap::new(),
        ..Default::default()
    };
    let add_xyz = |out: &mut TelemetryDataset,
                   prefix: &str,
                   values: &Vec<telemetry_parser::tags_impl::TimeVector3<f64>>,
                   factor: f64,
                   quantity: Quantity,
                   unit: Unit,
                   axes: &str| {
        for (axis, name) in ["x", "y", "z"].into_iter().enumerate() {
            let samples = values
                .iter()
                .map(|s| TimedSample {
                    time: s.t,
                    value: [s.x, s.y, s.z][axis] * factor,
                })
                .collect();
            let channel_name = format!("raw_{prefix}_{name}");
            out.insert(TelemetryChannel { descriptor: ChannelDescriptor { id: ChannelId::for_source_name(source_id, &channel_name), name: channel_name, quantity: quantity.clone(), unit: unit.clone(), interpolation: Interpolation::Linear, description: Some(format!("Insta360 raw {prefix} source XYZ, orientation {axes}; camera calibration transforms this explicitly")) }, series: ChannelSeries::new(samples) });
        }
    };
    add_xyz(
        &mut out,
        "accel",
        accel,
        9.80665 / scale(GroupId::Accelerometer),
        Quantity::Acceleration,
        Unit::MeterPerSecondSquared,
        &orientation(GroupId::Accelerometer),
    );
    add_xyz(
        &mut out,
        "gyro",
        gyro,
        std::f64::consts::PI / 180.0 / scale(GroupId::Gyroscope),
        Quantity::AngularVelocity,
        Unit::RadianPerSecond,
        &orientation(GroupId::Gyroscope),
    );
    if let Some(TagValue::Vec_GpsData(gps)) = tag(GroupId::GPS, TagId::Data) {
        let points: Vec<_> = gps.get().iter().filter(|p| p.is_acquired).collect();
        if let Some(first) = points.first() {
            for (name, quantity, unit, value) in [
                (
                    "gps_latitude",
                    Quantity::Position,
                    Unit::Degree,
                    |p: &telemetry_parser::tags_impl::GpsData| p.lat,
                ),
                (
                    "gps_longitude",
                    Quantity::Position,
                    Unit::Degree,
                    |p: &telemetry_parser::tags_impl::GpsData| p.lon,
                ),
                (
                    "gps_altitude",
                    Quantity::Altitude,
                    Unit::Meter,
                    |p: &telemetry_parser::tags_impl::GpsData| p.altitude,
                ),
                (
                    "gps_speed",
                    Quantity::Speed,
                    Unit::KilometerPerHour,
                    |p: &telemetry_parser::tags_impl::GpsData| p.speed,
                ),
            ]
                as [(
                    &str,
                    Quantity,
                    Unit,
                    fn(&telemetry_parser::tags_impl::GpsData) -> f64,
                ); 4]
            {
                out.insert(TelemetryChannel {
                    descriptor: ChannelDescriptor {
                        id: ChannelId::for_source_name(source_id, name),
                        name: name.into(),
                        quantity,
                        unit,
                        interpolation: Interpolation::Hold,
                        description: Some("Optional Insta360 GPS sample".into()),
                    },
                    series: ChannelSeries::new(
                        points
                            .iter()
                            .map(|p| TimedSample {
                                time: p.unix_timestamp - first.unix_timestamp,
                                value: value(p),
                            })
                            .collect(),
                    ),
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    #[test]
    fn csv_source_filter_settings_do_not_disable_default_time_and_headers() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "t,speed\n0,10\n1,20\n").unwrap();
        let data = GenericCsvAdapter
            .load(
                SourceId::new(),
                f.path(),
                &serde_json::json!({"low_pass_enabled":true,"low_pass_hz":2.0}),
            )
            .unwrap();
        assert_eq!(data.named("speed").unwrap().series.samples.len(), 2);
    }
    #[test]
    fn csv_detects_semicolon_and_interpolates() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "t;speed\n0;10\n0.4;20\n").unwrap();
        let out=GenericCsvAdapter.load(SourceId::new(),f.path(),&serde_json::json!({"header_row":0,"time_column":"t","columns":[{"column":"speed","unit":"kilometer_per_hour","quantity":"speed"}]})).unwrap();
        assert_eq!(
            out.named("speed").unwrap().series.sample_at(
                0.2,
                GapPolicy::default(),
                Interpolation::Linear
            ),
            Some(15.)
        );
    }
    #[test]
    fn synthetic_is_repeatable() {
        let a = SyntheticAdapter
            .load(SourceId::new(), Path::new(""), &Value::Null)
            .unwrap();
        let b = SyntheticAdapter
            .load(SourceId::new(), Path::new(""), &Value::Null)
            .unwrap();
        assert_eq!(
            a.named("speed").unwrap().series.samples[3].value,
            b.named("speed").unwrap().series.samples[3].value
        );
        assert_eq!(
            a.named("gear").unwrap().descriptor.interpolation,
            Interpolation::Hold
        );
        assert!(a.named("lap_time").is_some());
    }
    #[test]
    fn parses_supplied_binary_lrv_when_present() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("LRV_20260830_124108_01_017.lrv");
        if !path.exists() {
            return;
        }
        let dataset = Insta360Adapter
            .load(SourceId::new(), &path, &Value::Null)
            .unwrap();
        assert!(dataset.named("raw_accel_x").is_some());
        assert!(dataset.named("raw_gyro_z").is_some());
        let accel = dataset.named("raw_accel_x").unwrap();
        assert_eq!(accel.series.samples.len(), 189_616);
        let cadence = accel.series.samples[1].time - accel.series.samples[0].time;
        assert!((0.0009..0.0011).contains(&cadence));
        let ay = dataset.named("raw_accel_y").unwrap().series.samples[0].value;
        let az = dataset.named("raw_accel_z").unwrap().series.samples[0].value;
        let magnitude = (accel.series.samples[0].value.powi(2) + ay.powi(2) + az.powi(2)).sqrt();
        assert!(
            (8.0..12.0).contains(&magnitude),
            "unexpected resting acceleration magnitude {magnitude}"
        );
        assert!(dataset.named("gps_speed").is_none());
    }

    #[test]
    fn parses_supplied_multilap_xrk_when_present() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("mychron_data/gvkc/a_0065.xrk");
        if !path.exists() {
            return;
        }
        let dataset = AimXrkAdapter
            .load(SourceId::new(), &path, &Value::Null)
            .unwrap();
        assert_eq!(dataset.laps.len(), 11);
        for name in [
            "rpm",
            "exhaust_temperature",
            "water_temperature",
            "accelerometer_x",
            "accelerometer_y",
            "accelerometer_z",
            "gps_speed",
            "gear",
        ] {
            assert!(dataset.named(name).is_some(), "missing XRK channel {name}");
        }
        let rpm_max = dataset
            .named("rpm")
            .unwrap()
            .series
            .samples
            .iter()
            .map(|sample| sample.value)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((10_000.0..20_000.0).contains(&rpm_max));
        let egt_max = dataset
            .named("exhaust_temperature")
            .unwrap()
            .series
            .samples
            .iter()
            .map(|sample| sample.value)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((500.0..800.0).contains(&egt_max));
    }
}
