use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

macro_rules! id {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);
        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}
id!(SourceId);
id!(ChannelId);
id!(WidgetId);

impl ChannelId {
    /// Stable across repeated imports so project bindings survive reopening.
    pub fn for_source_name(source: SourceId, name: &str) -> Self {
        fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash
        }
        let source_bytes = source.0.as_bytes();
        let high = fnv1a(fnv1a(0xcbf2_9ce4_8422_2325, source_bytes), name.as_bytes());
        let low = fnv1a(fnv1a(0x8422_2325_cbf2_9ce4, source_bytes), name.as_bytes());
        Self(Uuid::from_u128((u128::from(high) << 64) | u128::from(low)))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quantity {
    Acceleration,
    AngularVelocity,
    Speed,
    Distance,
    Position,
    Altitude,
    Temperature,
    Pressure,
    Voltage,
    Current,
    Power,
    Rpm,
    LapTime,
    Percent,
    Generic,
    #[serde(untagged)]
    Custom(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    MeterPerSecondSquared,
    FootPerSecondSquared,
    StandardGravity,
    DegreePerSecond,
    RadianPerSecond,
    MeterPerSecond,
    KilometerPerHour,
    MilePerHour,
    Meter,
    Foot,
    Kilometer,
    Mile,
    Degree,
    Radian,
    Second,
    Millisecond,
    Celsius,
    Fahrenheit,
    Pascal,
    Bar,
    Psi,
    Volt,
    Ampere,
    Watt,
    RevolutionsPerMinute,
    Percent,
    Unitless,
    #[serde(untagged)]
    Custom(String),
}

impl Unit {
    pub fn symbol(&self) -> &str {
        match self {
            Self::MeterPerSecondSquared => "m/s²",
            Self::FootPerSecondSquared => "ft/s²",
            Self::StandardGravity => "g",
            Self::DegreePerSecond => "°/s",
            Self::RadianPerSecond => "rad/s",
            Self::MeterPerSecond => "m/s",
            Self::KilometerPerHour => "km/h",
            Self::MilePerHour => "mph",
            Self::Meter => "m",
            Self::Foot => "ft",
            Self::Kilometer => "km",
            Self::Mile => "mi",
            Self::Degree => "°",
            Self::Radian => "rad",
            Self::Second => "s",
            Self::Millisecond => "ms",
            Self::Celsius => "°C",
            Self::Fahrenheit => "°F",
            Self::Pascal => "Pa",
            Self::Bar => "bar",
            Self::Psi => "psi",
            Self::Volt => "V",
            Self::Ampere => "A",
            Self::Watt => "W",
            Self::RevolutionsPerMinute => "rpm",
            Self::Percent => "%",
            Self::Unitless => "",
            Self::Custom(s) => s,
        }
    }

    /// Returns the physical family understood by the built-in conversion
    /// helpers. Custom and unrelated units deliberately return `None`.
    pub fn family(&self) -> Option<UnitFamily> {
        match self {
            Self::MeterPerSecond | Self::KilometerPerHour | Self::MilePerHour => {
                Some(UnitFamily::Speed)
            }
            Self::Celsius | Self::Fahrenheit => Some(UnitFamily::Temperature),
            Self::Meter | Self::Foot | Self::Kilometer | Self::Mile => Some(UnitFamily::Distance),
            Self::MeterPerSecondSquared | Self::FootPerSecondSquared | Self::StandardGravity => {
                Some(UnitFamily::Acceleration)
            }
            Self::Pascal | Self::Bar | Self::Psi => Some(UnitFamily::Pressure),
            Self::Degree | Self::Radian => Some(UnitFamily::Angle),
            Self::DegreePerSecond | Self::RadianPerSecond => Some(UnitFamily::AngularVelocity),
            Self::Second | Self::Millisecond => Some(UnitFamily::Time),
            _ => None,
        }
    }

    /// All known units that can express the same kind of value as `self`.
    /// For units without a conversion family, this returns only `self`, which
    /// lets display controls preserve an unknown custom unit safely.
    pub fn compatible_units(&self) -> Vec<Unit> {
        match self.family() {
            Some(UnitFamily::Speed) => vec![
                Unit::MeterPerSecond,
                Unit::KilometerPerHour,
                Unit::MilePerHour,
            ],
            Some(UnitFamily::Temperature) => vec![Unit::Celsius, Unit::Fahrenheit],
            Some(UnitFamily::Distance) => {
                vec![Unit::Meter, Unit::Kilometer, Unit::Foot, Unit::Mile]
            }
            Some(UnitFamily::Acceleration) => vec![
                Unit::MeterPerSecondSquared,
                Unit::StandardGravity,
                Unit::FootPerSecondSquared,
            ],
            Some(UnitFamily::Pressure) => vec![Unit::Pascal, Unit::Bar, Unit::Psi],
            Some(UnitFamily::Angle) => vec![Unit::Degree, Unit::Radian],
            Some(UnitFamily::AngularVelocity) => {
                vec![Unit::DegreePerSecond, Unit::RadianPerSecond]
            }
            Some(UnitFamily::Time) => vec![Unit::Second, Unit::Millisecond],
            None => vec![self.clone()],
        }
    }

    /// Whether [`Self::convert_value_to`] can convert directly to `other`.
    pub fn is_compatible_with(&self, other: &Unit) -> bool {
        self == other
            || self
                .family()
                .is_some_and(|family| other.family() == Some(family))
    }

    /// Converts a value expressed in `self` to `target`.
    ///
    /// Values with identical units are returned unchanged, including custom
    /// units. Other conversions are intentionally limited to known compatible
    /// physical families rather than guessing from a channel name.
    pub fn convert_value_to(&self, value: f64, target: &Unit) -> Result<f64, UnitConversionError> {
        Unit::convert_value(value, self, target)
    }

    /// Converts `value` from `from` to `to` for the supported physical
    /// families: speed, temperature, distance, acceleration, pressure, angle,
    /// angular velocity, and time.
    pub fn convert_value(value: f64, from: &Unit, to: &Unit) -> Result<f64, UnitConversionError> {
        if from == to {
            return Ok(value);
        }
        let Some(family) = from.family() else {
            return Err(UnitConversionError::Incompatible {
                from: from.clone(),
                to: to.clone(),
            });
        };
        if to.family() != Some(family) {
            return Err(UnitConversionError::Incompatible {
                from: from.clone(),
                to: to.clone(),
            });
        }

        let converted = match family {
            UnitFamily::Speed => {
                speed_in_meters_per_second(value, from).map(|v| speed_from_meters_per_second(v, to))
            }
            UnitFamily::Temperature => {
                temperature_in_celsius(value, from).map(|v| temperature_from_celsius(v, to))
            }
            UnitFamily::Distance => {
                distance_in_meters(value, from).map(|v| distance_from_meters(v, to))
            }
            UnitFamily::Acceleration => acceleration_in_meters_per_second_squared(value, from)
                .map(|v| acceleration_from_meters_per_second_squared(v, to)),
            UnitFamily::Pressure => {
                pressure_in_pascals(value, from).map(|v| pressure_from_pascals(v, to))
            }
            UnitFamily::Angle => angle_in_radians(value, from).map(|v| angle_from_radians(v, to)),
            UnitFamily::AngularVelocity => angular_velocity_in_radians_per_second(value, from)
                .map(|v| angular_velocity_from_radians_per_second(v, to)),
            UnitFamily::Time => time_in_seconds(value, from).map(|v| time_from_seconds(v, to)),
        };

        // Families above are exhaustively matched, but keeping an explicit
        // error here makes this method safe if a future UnitFamily is added.
        converted
            .flatten()
            .ok_or_else(|| UnitConversionError::Incompatible {
                from: from.clone(),
                to: to.clone(),
            })
    }

    /// The customary display unit for a quantity under a chosen measurement
    /// system. `None` means the quantity has no system-specific default.
    pub fn default_for(quantity: &Quantity, system: UnitSystem) -> Option<Self> {
        system.default_unit_for(quantity)
    }
}

/// A physical family supported by [`Unit::convert_value`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitFamily {
    Speed,
    Temperature,
    Distance,
    Acceleration,
    Pressure,
    Angle,
    AngularVelocity,
    Time,
}

/// User-facing measurement-system preference for telemetry display.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitSystem {
    #[default]
    Metric,
    Imperial,
}

impl UnitSystem {
    /// Returns a readable default while retaining the semantic distinction
    /// between trip distance and altitude (miles versus feet in imperial).
    pub fn default_unit_for(self, quantity: &Quantity) -> Option<Unit> {
        match (self, quantity) {
            (Self::Metric, Quantity::Speed) => Some(Unit::KilometerPerHour),
            (Self::Imperial, Quantity::Speed) => Some(Unit::MilePerHour),
            (Self::Metric, Quantity::Temperature) => Some(Unit::Celsius),
            (Self::Imperial, Quantity::Temperature) => Some(Unit::Fahrenheit),
            (Self::Metric, Quantity::Distance | Quantity::Position) => Some(Unit::Kilometer),
            (Self::Imperial, Quantity::Distance | Quantity::Position) => Some(Unit::Mile),
            (Self::Metric, Quantity::Altitude) => Some(Unit::Meter),
            (Self::Imperial, Quantity::Altitude) => Some(Unit::Foot),
            // G-force is conventional in motorsport independent of regional
            // distance units, and is more readable than either SI base unit.
            (_, Quantity::Acceleration) => Some(Unit::StandardGravity),
            (Self::Metric, Quantity::Pressure) => Some(Unit::Bar),
            (Self::Imperial, Quantity::Pressure) => Some(Unit::Psi),
            _ => None,
        }
    }
}

/// Returned when a conversion is requested for different or unsupported unit
/// families. Values are cloned so callers can report the exact request.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum UnitConversionError {
    #[error("cannot convert incompatible units {from:?} and {to:?}")]
    Incompatible { from: Unit, to: Unit },
}

fn speed_in_meters_per_second(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::MeterPerSecond => Some(value),
        Unit::KilometerPerHour => Some(value / 3.6),
        Unit::MilePerHour => Some(value * 0.447_04),
        _ => None,
    }
}

fn speed_from_meters_per_second(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::MeterPerSecond => Some(value),
        Unit::KilometerPerHour => Some(value * 3.6),
        Unit::MilePerHour => Some(value / 0.447_04),
        _ => None,
    }
}

fn temperature_in_celsius(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Celsius => Some(value),
        Unit::Fahrenheit => Some((value - 32.0) * 5.0 / 9.0),
        _ => None,
    }
}

fn temperature_from_celsius(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Celsius => Some(value),
        Unit::Fahrenheit => Some(value * 9.0 / 5.0 + 32.0),
        _ => None,
    }
}

fn distance_in_meters(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Meter => Some(value),
        Unit::Foot => Some(value * 0.304_8),
        Unit::Kilometer => Some(value * 1_000.0),
        Unit::Mile => Some(value * 1_609.344),
        _ => None,
    }
}

fn distance_from_meters(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Meter => Some(value),
        Unit::Foot => Some(value / 0.304_8),
        Unit::Kilometer => Some(value / 1_000.0),
        Unit::Mile => Some(value / 1_609.344),
        _ => None,
    }
}

fn acceleration_in_meters_per_second_squared(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::MeterPerSecondSquared => Some(value),
        Unit::FootPerSecondSquared => Some(value * 0.304_8),
        Unit::StandardGravity => Some(value * 9.806_65),
        _ => None,
    }
}

fn acceleration_from_meters_per_second_squared(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::MeterPerSecondSquared => Some(value),
        Unit::FootPerSecondSquared => Some(value / 0.304_8),
        Unit::StandardGravity => Some(value / 9.806_65),
        _ => None,
    }
}

fn pressure_in_pascals(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Pascal => Some(value),
        Unit::Bar => Some(value * 100_000.0),
        Unit::Psi => Some(value * 6_894.757_293_168),
        _ => None,
    }
}

fn pressure_from_pascals(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Pascal => Some(value),
        Unit::Bar => Some(value / 100_000.0),
        Unit::Psi => Some(value / 6_894.757_293_168),
        _ => None,
    }
}

fn angle_in_radians(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Degree => Some(value.to_radians()),
        Unit::Radian => Some(value),
        _ => None,
    }
}

fn angle_from_radians(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Degree => Some(value.to_degrees()),
        Unit::Radian => Some(value),
        _ => None,
    }
}

fn angular_velocity_in_radians_per_second(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::DegreePerSecond => Some(value.to_radians()),
        Unit::RadianPerSecond => Some(value),
        _ => None,
    }
}

fn angular_velocity_from_radians_per_second(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::DegreePerSecond => Some(value.to_degrees()),
        Unit::RadianPerSecond => Some(value),
        _ => None,
    }
}

fn time_in_seconds(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Second => Some(value),
        Unit::Millisecond => Some(value / 1_000.0),
        _ => None,
    }
}

fn time_from_seconds(value: f64, unit: &Unit) -> Option<f64> {
    match unit {
        Unit::Second => Some(value),
        Unit::Millisecond => Some(value * 1_000.0),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    #[default]
    Linear,
    Hold,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimedSample {
    pub time: f64,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelDescriptor {
    pub id: ChannelId,
    pub name: String,
    pub quantity: Quantity,
    pub unit: Unit,
    #[serde(default)]
    pub interpolation: Interpolation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GapPolicy {
    pub max_gap_seconds: f64,
}

/// Settings for correlation-based alignment of two continuous telemetry
/// streams.
///
/// The search tests `target(t + lag)` against `reference(t)`. Consequently a
/// positive result means the target logger's timestamps are later than the
/// reference logger's timestamps for the same event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorrelationConfig {
    /// Inclusive lower bound for the target-minus-reference timestamp lag.
    pub min_lag_seconds: f64,
    /// Inclusive upper bound for the target-minus-reference timestamp lag.
    pub max_lag_seconds: f64,
    /// Spacing between tested lags. `None` derives a conservative value from
    /// the sample intervals of both streams.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag_resolution_seconds: Option<f64>,
    /// A candidate must cover at least this much reference-clock time.
    pub min_overlap_seconds: f64,
    /// A candidate must contain at least this many interpolated sample pairs.
    pub min_samples: usize,
    /// Interpolation used when sampling the target at a shifted reference
    /// timestamp.
    #[serde(default)]
    pub interpolation: Interpolation,
    /// When enabled, select the greatest absolute coefficient. This is useful
    /// for inverted sensors, while the returned coefficient retains its sign.
    #[serde(default)]
    pub use_absolute_correlation: bool,
    /// Remove an independent least-squares linear trend from each candidate
    /// before calculating Pearson correlation. This avoids long speed/RPM
    /// ramps dominating a match while retaining braking, cornering, and other
    /// shape information.
    #[serde(default = "default_detrend")]
    pub detrend: bool,
}

const fn default_detrend() -> bool {
    true
}

impl Default for CorrelationConfig {
    fn default() -> Self {
        Self {
            min_lag_seconds: -10.0,
            max_lag_seconds: 10.0,
            lag_resolution_seconds: None,
            min_overlap_seconds: 2.0,
            min_samples: 24,
            interpolation: Interpolation::Linear,
            use_absolute_correlation: false,
            detrend: true,
        }
    }
}

/// Result of a correlation offset search.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CorrelationResult {
    /// The lag for which `target(t + lag)` best matches `reference(t)`.
    ///
    /// This is also `target_source_time - reference_source_time` for the same
    /// physical instant. It is intentionally independent of either source's
    /// current alignment setting.
    pub target_minus_reference_seconds: f64,
    /// Signed Pearson correlation coefficient, in `[-1, 1]`.
    pub correlation_coefficient: f64,
    /// Number of reference/target pairs contributing to the coefficient.
    pub sample_count: usize,
    /// Span, on the reference clock, covered by those pairs.
    pub overlap_seconds: f64,
    /// The candidate-lag resolution actually used by the search.
    pub lag_resolution_seconds: f64,
}

impl CorrelationResult {
    /// Computes the target source alignment which corresponds to a reference
    /// source alignment under `source_time = video_time + offset_seconds`.
    pub fn target_offset_from_reference(&self, reference_offset_seconds: f64) -> f64 {
        reference_offset_seconds + self.target_minus_reference_seconds
    }

    /// Computes the amount to add to the target's current source alignment.
    /// A caller can apply this directly with
    /// `target.alignment.offset_seconds += result.target_offset_adjustment(...)`.
    pub fn target_offset_adjustment(
        &self,
        reference_offset_seconds: f64,
        target_offset_seconds: f64,
    ) -> f64 {
        self.target_offset_from_reference(reference_offset_seconds) - target_offset_seconds
    }
}

/// Failure from a correlation offset search.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum CorrelationError {
    #[error("lag bounds must be finite and ordered, got {min_lag_seconds} to {max_lag_seconds}")]
    InvalidLagRange {
        min_lag_seconds: f64,
        max_lag_seconds: f64,
    },
    #[error("lag_resolution_seconds must be finite and greater than zero, got {0}")]
    InvalidLagResolution(f64),
    #[error("min_overlap_seconds must be finite and non-negative, got {0}")]
    InvalidMinimumOverlap(f64),
    #[error("min_samples must be at least 2, got {0}")]
    InvalidMinimumSamples(usize),
    #[error("lag range and resolution require {count} candidates; limit is {limit}")]
    TooManyLagCandidates { count: usize, limit: usize },
    #[error(
        "requested lag range {requested_min_lag_seconds} to {requested_max_lag_seconds} has no feasible overlap; feasible lags are {feasible_min_lag_seconds} to {feasible_max_lag_seconds}"
    )]
    NoFeasibleLagOverlap {
        requested_min_lag_seconds: f64,
        requested_max_lag_seconds: f64,
        feasible_min_lag_seconds: f64,
        feasible_max_lag_seconds: f64,
    },
    #[error(
        "reference and target require at least two finite samples (got {reference} and {target})"
    )]
    InsufficientInputSamples { reference: usize, target: usize },
    #[error(
        "no tested lag had at least {min_samples} samples across {min_overlap_seconds:.3} s of overlap"
    )]
    InsufficientOverlap {
        min_samples: usize,
        min_overlap_seconds: f64,
    },
    #[error("all overlapping candidates were constant after {detrending} detrending")]
    ConstantSignal { detrending: &'static str },
}

const MAX_CORRELATION_CANDIDATES: usize = 20_001;
const MAX_CORRELATION_SAMPLES_PER_CANDIDATE: usize = 12_000;

/// Searches for the timestamp lag that best aligns two irregular telemetry
/// streams using Pearson correlation.
///
/// For every candidate `lag`, this compares `reference(t)` with
/// `target(t + lag)`. Therefore, if this function returns `+0.250`, an event
/// at reference timestamp `12.000` appears at target timestamp `12.250`.
/// Given the application's convention `source_time = video_time + offset`,
/// use [`CorrelationResult::target_offset_from_reference`] (or
/// [`CorrelationResult::target_offset_adjustment`]) to update the target
/// source alignment.
///
/// The reference samples form the comparison grid and are thinned evenly for
/// bounded runtime. Target values are interpolated at each shifted timestamp;
/// importer-declared gaps are honored, and otherwise the gap tolerance adapts
/// to sparse logs. Each candidate is independently linearly detrended by
/// default before Pearson normalization.
pub fn correlate_channel_series(
    reference: &ChannelSeries,
    target: &ChannelSeries,
    config: &CorrelationConfig,
) -> Result<CorrelationResult, CorrelationError> {
    validate_correlation_config(config)?;
    if reference.samples.len() < 2 || target.samples.len() < 2 {
        return Err(CorrelationError::InsufficientInputSamples {
            reference: reference.samples.len(),
            target: target.samples.len(),
        });
    }

    let mut resolution = match config.lag_resolution_seconds {
        Some(value) => value,
        None => automatic_lag_resolution(reference, target),
    };
    if !resolution.is_finite() || resolution <= 0.0 {
        return Err(CorrelationError::InvalidLagResolution(resolution));
    }
    // A candidate can only have `min_overlap_seconds` of reference-clock
    // overlap when the shifted target interval intersects the reference
    // interval for that long. Restricting the search to this interval is
    // important for callers that use a deliberately broad safety range: it
    // prevents an otherwise irrelevant range from exhausting the candidate
    // limit before the actual data can be searched.
    let (feasible_min_lag, feasible_max_lag) =
        feasible_lag_bounds(reference, target, config.min_overlap_seconds);
    let search_min_lag = config.min_lag_seconds.max(feasible_min_lag);
    let search_max_lag = config.max_lag_seconds.min(feasible_max_lag);
    if search_min_lag > search_max_lag {
        return Err(CorrelationError::NoFeasibleLagOverlap {
            requested_min_lag_seconds: config.min_lag_seconds,
            requested_max_lag_seconds: config.max_lag_seconds,
            feasible_min_lag_seconds: feasible_min_lag,
            feasible_max_lag_seconds: feasible_max_lag,
        });
    }

    // Keep the requested grid's phase when clipping its bounds. The clipped
    // endpoints remain explicit candidates, while interior grid points are
    // still anchored to `config.min_lag_seconds` rather than to the clipped
    // lower bound.
    let mut candidate_lags = correlation_lag_candidates(
        config.min_lag_seconds,
        search_min_lag,
        search_max_lag,
        resolution,
    );
    if config.lag_resolution_seconds.is_none() && candidate_lags.is_err() {
        // Automatic resolution is a runtime safeguard, not a user-requested
        // precision. Coarsen it enough to keep a very long but valid data
        // interval searchable; explicit resolutions retain the hard error.
        let span = search_max_lag - search_min_lag;
        resolution = (span / (MAX_CORRELATION_CANDIDATES - 2) as f64)
            .mul_add(1.0 + 1e-12, 0.0)
            .max(resolution);
        candidate_lags = correlation_lag_candidates(
            config.min_lag_seconds,
            search_min_lag,
            search_max_lag,
            resolution,
        );
    }
    let candidate_lags = match candidate_lags {
        Ok(candidates) => candidates,
        Err(count) => {
            return Err(CorrelationError::TooManyLagCandidates {
                count,
                limit: MAX_CORRELATION_CANDIDATES,
            });
        }
    };

    let stride = reference
        .samples
        .len()
        .div_ceil(MAX_CORRELATION_SAMPLES_PER_CANDIDATE)
        .max(1);
    let target_gap = correlation_gap_policy(target);
    let mut best: Option<CorrelationResult> = None;
    let mut had_sufficient_overlap = false;
    let mut had_non_constant_pair = false;

    for lag in candidate_lags {
        let mut times = Vec::new();
        let mut reference_values = Vec::new();
        let mut target_values = Vec::new();
        for sample in reference.samples.iter().step_by(stride) {
            if let Some(value) =
                target.sample_at(sample.time + lag, target_gap, config.interpolation)
            {
                times.push(sample.time);
                reference_values.push(sample.value);
                target_values.push(value);
            }
        }
        let overlap_seconds = times
            .first()
            .zip(times.last())
            .map_or(0.0, |(first, last)| last - first);
        if times.len() < config.min_samples || overlap_seconds < config.min_overlap_seconds {
            continue;
        }
        had_sufficient_overlap = true;
        let Some(coefficient) =
            pearson_coefficient(&times, &reference_values, &target_values, config.detrend)
        else {
            continue;
        };
        had_non_constant_pair = true;
        let result = CorrelationResult {
            target_minus_reference_seconds: lag,
            correlation_coefficient: coefficient,
            sample_count: times.len(),
            overlap_seconds,
            lag_resolution_seconds: resolution,
        };
        let score = if config.use_absolute_correlation {
            coefficient.abs()
        } else {
            coefficient
        };
        let replace = best.as_ref().is_none_or(|current| {
            let current_score = if config.use_absolute_correlation {
                current.correlation_coefficient.abs()
            } else {
                current.correlation_coefficient
            };
            score > current_score + 1e-12
                || ((score - current_score).abs() <= 1e-12
                    && lag.abs() < current.target_minus_reference_seconds.abs())
        });
        if replace {
            best = Some(result);
        }
    }

    match best {
        Some(result) => Ok(result),
        None if !had_sufficient_overlap => Err(CorrelationError::InsufficientOverlap {
            min_samples: config.min_samples,
            min_overlap_seconds: config.min_overlap_seconds,
        }),
        None if !had_non_constant_pair => Err(CorrelationError::ConstantSignal {
            detrending: if config.detrend { "linear" } else { "no" },
        }),
        None => unreachable!("a non-constant candidate always produces a result"),
    }
}

fn correlation_lag_candidates(
    requested_min_lag: f64,
    search_min_lag: f64,
    search_max_lag: f64,
    resolution: f64,
) -> Result<Vec<f64>, usize> {
    const ENDPOINT_TOLERANCE_FACTOR: f64 = 1e-9;
    let endpoint_tolerance = resolution * ENDPOINT_TOLERANCE_FACTOR;
    let first_grid_index = ((search_min_lag - requested_min_lag) / resolution).ceil();
    let last_grid_index = ((search_max_lag - requested_min_lag) / resolution).floor();
    if !first_grid_index.is_finite()
        || !last_grid_index.is_finite()
        || last_grid_index - first_grid_index > MAX_CORRELATION_CANDIDATES as f64
    {
        return Err(MAX_CORRELATION_CANDIDATES + 1);
    }
    let first_grid_index = first_grid_index as usize;
    let last_grid_index = last_grid_index as usize;
    let mut candidates = vec![search_min_lag];
    if first_grid_index <= last_grid_index {
        for grid_index in first_grid_index..=last_grid_index {
            let lag = requested_min_lag + grid_index as f64 * resolution;
            if (lag - search_min_lag).abs() > endpoint_tolerance {
                candidates.push(lag);
                if candidates.len() > MAX_CORRELATION_CANDIDATES {
                    return Err(candidates.len());
                }
            }
        }
    }
    if candidates
        .last()
        .is_none_or(|lag| (search_max_lag - *lag).abs() > endpoint_tolerance)
    {
        candidates.push(search_max_lag);
    }
    if candidates.len() > MAX_CORRELATION_CANDIDATES {
        return Err(candidates.len());
    }
    Ok(candidates)
}

/// Returns the inclusive lag interval for which the source time intervals can
/// overlap by at least `min_overlap_seconds`.
///
/// The correlation convention is `target(t + lag)` versus `reference(t)`, so
/// the target interval on the reference clock is
/// `[target_min - lag, target_max - lag]`. Solving for an overlap of at least
/// `minimum` seconds gives
/// `target_min + minimum - reference_max <= lag <=
/// target_max - minimum - reference_min`.
fn feasible_lag_bounds(
    reference: &ChannelSeries,
    target: &ChannelSeries,
    minimum: f64,
) -> (f64, f64) {
    let reference_min = reference.samples[0].time;
    let reference_max = reference.samples[reference.samples.len() - 1].time;
    let target_min = target.samples[0].time;
    let target_max = target.samples[target.samples.len() - 1].time;
    (
        target_min + minimum - reference_max,
        target_max - minimum - reference_min,
    )
}

/// Convenience wrapper for callers holding complete telemetry channels.
pub fn correlate_telemetry_channels(
    reference: &TelemetryChannel,
    target: &TelemetryChannel,
    config: &CorrelationConfig,
) -> Result<CorrelationResult, CorrelationError> {
    correlate_channel_series(&reference.series, &target.series, config)
}

fn validate_correlation_config(config: &CorrelationConfig) -> Result<(), CorrelationError> {
    if !config.min_lag_seconds.is_finite()
        || !config.max_lag_seconds.is_finite()
        || config.min_lag_seconds > config.max_lag_seconds
    {
        return Err(CorrelationError::InvalidLagRange {
            min_lag_seconds: config.min_lag_seconds,
            max_lag_seconds: config.max_lag_seconds,
        });
    }
    if let Some(resolution) = config.lag_resolution_seconds
        && (!resolution.is_finite() || resolution <= 0.0)
    {
        return Err(CorrelationError::InvalidLagResolution(resolution));
    }
    if !config.min_overlap_seconds.is_finite() || config.min_overlap_seconds < 0.0 {
        return Err(CorrelationError::InvalidMinimumOverlap(
            config.min_overlap_seconds,
        ));
    }
    if config.min_samples < 2 {
        return Err(CorrelationError::InvalidMinimumSamples(config.min_samples));
    }
    Ok(())
}

fn automatic_lag_resolution(reference: &ChannelSeries, target: &ChannelSeries) -> f64 {
    let reference_step = median_sample_step(reference).unwrap_or(0.02);
    let target_step = median_sample_step(target).unwrap_or(0.02);
    // Half of the fastest median cadence provides useful sub-sample placement
    // for linear interpolation, while the clamps prevent accidental very large
    // searches on camera-rate data or impractically coarse sparse logs.
    (reference_step.min(target_step) * 0.5).clamp(0.005, 0.1)
}

fn median_sample_step(series: &ChannelSeries) -> Option<f64> {
    let mut steps: Vec<f64> = series
        .samples
        .windows(2)
        .map(|pair| pair[1].time - pair[0].time)
        .filter(|step| step.is_finite() && *step > 0.0)
        .collect();
    if steps.is_empty() {
        return None;
    }
    let middle = steps.len() / 2;
    steps.select_nth_unstable_by(middle, |a, b| a.total_cmp(b));
    Some(steps[middle])
}

fn correlation_gap_policy(series: &ChannelSeries) -> GapPolicy {
    let max_gap_seconds = series.gap_seconds.unwrap_or_else(|| {
        median_sample_step(series)
            .map(|step| (step * 3.0).max(GapPolicy::default().max_gap_seconds))
            .unwrap_or(GapPolicy::default().max_gap_seconds)
    });
    GapPolicy { max_gap_seconds }
}

fn pearson_coefficient(times: &[f64], left: &[f64], right: &[f64], detrend: bool) -> Option<f64> {
    debug_assert_eq!(times.len(), left.len());
    debug_assert_eq!(left.len(), right.len());
    if left.len() < 2 {
        return None;
    }
    let left = detrended_values(times, left, detrend);
    let right = detrended_values(times, right, detrend);
    let left_mean = left.iter().sum::<f64>() / left.len() as f64;
    let right_mean = right.iter().sum::<f64>() / right.len() as f64;
    let (mut covariance, mut left_energy, mut right_energy) = (0.0, 0.0, 0.0);
    for (left, right) in left.iter().zip(right.iter()) {
        let left = left - left_mean;
        let right = right - right_mean;
        covariance += left * right;
        left_energy += left * left;
        right_energy += right * right;
    }
    let denominator = (left_energy * right_energy).sqrt();
    (denominator.is_finite() && denominator > f64::EPSILON)
        .then(|| (covariance / denominator).clamp(-1.0, 1.0))
}

fn detrended_values(times: &[f64], values: &[f64], detrend: bool) -> Vec<f64> {
    if !detrend {
        return values.to_vec();
    }
    let time_mean = times.iter().sum::<f64>() / times.len() as f64;
    let value_mean = values.iter().sum::<f64>() / values.len() as f64;
    let (mut time_energy, mut covariance) = (0.0, 0.0);
    for (&time, &value) in times.iter().zip(values.iter()) {
        let centered_time = time - time_mean;
        time_energy += centered_time * centered_time;
        covariance += centered_time * (value - value_mean);
    }
    if time_energy <= f64::EPSILON {
        return values.iter().map(|value| value - value_mean).collect();
    }
    let slope = covariance / time_energy;
    values
        .iter()
        .zip(times.iter())
        .map(|(&value, &time)| value - (value_mean + slope * (time - time_mean)))
        .collect()
}
impl Default for GapPolicy {
    fn default() -> Self {
        Self {
            max_gap_seconds: 0.5,
        }
    }
}

/// Error returned for a non-positive or non-finite low-pass time constant.
#[derive(Clone, Copy, Debug, Error, PartialEq)]
pub enum LowPassError {
    #[error("low-pass smoothing_seconds must be finite and greater than zero, got {0}")]
    InvalidSmoothingSeconds(f64),
    #[error("low-pass cutoff_hz must be finite and greater than zero, got {0}")]
    InvalidCutoffHz(f64),
}

/// Per-pass cutoff multiplier that makes two forward/backward first-order
/// passes reach -3 dB at the user-selected final cutoff.
pub const ZERO_PHASE_CUTOFF_COMPENSATION: f64 = 1.553_773_974;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ChannelSeries {
    pub samples: Vec<TimedSample>,
    /// Importer-suggested discontinuity threshold, when the source provides one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap_seconds: Option<f64>,
}
impl ChannelSeries {
    pub fn new(mut samples: Vec<TimedSample>) -> Self {
        samples.retain(|s| s.time.is_finite() && s.value.is_finite());
        samples.sort_by(|a, b| a.time.total_cmp(&b.time));
        Self {
            samples,
            gap_seconds: None,
        }
    }
    pub fn with_gap(mut self, gap_seconds: f64) -> Self {
        self.gap_seconds = (gap_seconds.is_finite() && gap_seconds >= 0.0).then_some(gap_seconds);
        self
    }
    /// Samples at source time. Returns `None` outside the data, or across a gap.
    pub fn sample_at(
        &self,
        time: f64,
        gap: GapPolicy,
        interpolation: Interpolation,
    ) -> Option<f64> {
        let first = *self.samples.first()?;
        let last = *self.samples.last()?;
        if time < first.time || time > last.time {
            return None;
        }
        let index = self.samples.partition_point(|s| s.time <= time);
        if index == 0 {
            return Some(first.value);
        }
        let left = self.samples[index - 1];
        if (left.time - time).abs() < f64::EPSILON {
            return Some(left.value);
        }
        let right = *self.samples.get(index)?;
        if right.time - left.time > gap.max_gap_seconds {
            return None;
        }
        Some(match interpolation {
            Interpolation::Hold => left.value,
            Interpolation::Linear => {
                left.value
                    + (right.value - left.value) * (time - left.time) / (right.time - left.time)
            }
        })
    }
    pub fn sample_at_default(&self, time: f64, interpolation: Interpolation) -> Option<f64> {
        self.sample_at(
            time,
            GapPolicy {
                max_gap_seconds: self
                    .gap_seconds
                    .unwrap_or(GapPolicy::default().max_gap_seconds),
            },
            interpolation,
        )
    }

    /// Produces a zero-phase low-pass-filtered copy of the series.
    ///
    /// `smoothing_seconds` is the exponential time constant used for each pass
    /// of the forward/backward filter. In other words, one pass moves about
    /// 63% toward a step after one time constant; the two passes together have
    /// zero phase shift but a steeper roll-off. Source gaps split the series
    /// into independent segments, so a value cannot bleed across a telemetry
    /// discontinuity. This legacy parameter remains a time constant rather
    /// than a displayed cutoff frequency.
    ///
    /// This is designed to be called when a source or binding changes and then
    /// cached by the caller. The returned series retains source timestamps and
    /// supports the usual `sample_at*` methods in O(log n) per render frame.
    pub fn low_pass(&self, smoothing_seconds: f64) -> Result<Self, LowPassError> {
        self.low_pass_with_gap(smoothing_seconds, self.default_gap_policy())
    }

    /// Like [`Self::low_pass`], with an explicit discontinuity policy.
    pub fn low_pass_with_gap(
        &self,
        smoothing_seconds: f64,
        gap: GapPolicy,
    ) -> Result<Self, LowPassError> {
        if !smoothing_seconds.is_finite() || smoothing_seconds <= 0.0 {
            return Err(LowPassError::InvalidSmoothingSeconds(smoothing_seconds));
        }

        let mut filtered = self.samples.clone();
        let mut segment_start = 0;
        for index in 1..=self.samples.len() {
            let at_segment_end = index == self.samples.len()
                || self.samples[index].time - self.samples[index - 1].time > gap.max_gap_seconds;
            if !at_segment_end {
                continue;
            }

            // A forward pass followed by a reverse-time pass gives a
            // zero-phase response. The elapsed time is taken from the sample
            // timestamps on both passes, which also makes this work for
            // irregularly sampled series.
            let segment = &self.samples[segment_start..index];
            let mut forward = Vec::with_capacity(segment.len());
            if let Some(first) = segment.first() {
                forward.push(first.value);
                for pair in segment.windows(2) {
                    let elapsed = (pair[1].time - pair[0].time).max(0.0);
                    let alpha = -(-elapsed / smoothing_seconds).exp_m1();
                    let previous = *forward.last().expect("first value was pushed");
                    forward.push(previous + alpha * (pair[1].value - previous));
                }
            }

            if let Some(last) = forward.last().copied() {
                filtered[index - 1].value = last;
                for offset in (0..forward.len().saturating_sub(1)).rev() {
                    let elapsed = (segment[offset + 1].time - segment[offset].time).max(0.0);
                    let alpha = -(-elapsed / smoothing_seconds).exp_m1();
                    let next = filtered[segment_start + offset + 1].value;
                    filtered[segment_start + offset].value =
                        next + alpha * (forward[offset] - next);
                }
            }
            segment_start = index;
        }

        Ok(Self {
            samples: filtered,
            gap_seconds: self.gap_seconds,
        })
    }

    /// Applies an optional binding smoothing setting. `None` intentionally
    /// returns an unchanged copy, making it convenient to cache one resolved
    /// series per binding without a renderer-side special case.
    pub fn low_pass_optional(&self, smoothing_seconds: Option<f64>) -> Result<Self, LowPassError> {
        match smoothing_seconds {
            Some(seconds) => self.low_pass(seconds),
            None => Ok(self.clone()),
        }
    }

    /// Produces a zero-phase low-pass-filtered copy from a final two-pass
    /// -3 dB cutoff frequency.
    ///
    /// The underlying first-order filter runs forward and backward. Its
    /// per-pass cutoff is compensated by `1.553773974...`, so the displayed
    /// `cutoff_hz` denotes the -3 dB point of the complete two-pass response.
    pub fn low_pass_hz(&self, cutoff_hz: f64) -> Result<Self, LowPassError> {
        if !cutoff_hz.is_finite() || cutoff_hz <= 0.0 {
            return Err(LowPassError::InvalidCutoffHz(cutoff_hz));
        }
        self.low_pass(1.0 / (std::f64::consts::TAU * cutoff_hz * ZERO_PHASE_CUTOFF_COMPENSATION))
    }

    /// Applies an optional cutoff-frequency setting, returning an unchanged
    /// copy when no cutoff has been configured.
    pub fn low_pass_optional_hz(&self, cutoff_hz: Option<f64>) -> Result<Self, LowPassError> {
        match cutoff_hz {
            Some(hz) => self.low_pass_hz(hz),
            None => Ok(self.clone()),
        }
    }

    fn default_gap_policy(&self) -> GapPolicy {
        GapPolicy {
            max_gap_seconds: self
                .gap_seconds
                .unwrap_or(GapPolicy::default().max_gap_seconds),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryChannel {
    pub descriptor: ChannelDescriptor,
    pub series: ChannelSeries,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct TelemetryDataset {
    pub source_id: SourceId,
    #[serde(default)]
    pub channels: BTreeMap<ChannelId, TelemetryChannel>,
    /// Source-level descriptive fields retained by structured importers.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Lap boundaries in source time, when the logger records them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub laps: Vec<TelemetryLap>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TelemetryLap {
    pub number: i32,
    pub start_time: f64,
    pub end_time: f64,
    pub lap_type: String,
}
impl TelemetryDataset {
    pub fn channel(&self, id: ChannelId) -> Option<&TelemetryChannel> {
        self.channels.get(&id)
    }
    pub fn named(&self, name: &str) -> Option<&TelemetryChannel> {
        self.channels.values().find(|c| c.descriptor.name == name)
    }
    pub fn insert(&mut self, channel: TelemetryChannel) {
        self.channels.insert(channel.descriptor.id, channel);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelRef {
    pub source_id: SourceId,
    pub channel_id: ChannelId,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelBinding {
    pub slot: String,
    pub channel: ChannelRef,
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default)]
    pub offset: f64,
    #[serde(default)]
    pub invert: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smoothing_seconds: Option<f64>,
    /// Optional final two-pass -3 dB cutoff frequency for zero-phase low-pass
    /// filtering. New consumers should prefer this setting;
    /// `smoothing_seconds` remains a legacy per-pass time constant for project
    /// compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low_pass_hz: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_unit: Option<Unit>,
}
fn one() -> f64 {
    1.0
}
impl ChannelBinding {
    pub fn new(slot: impl Into<String>, channel: ChannelRef) -> Self {
        Self {
            slot: slot.into(),
            channel,
            scale: 1.0,
            offset: 0.0,
            invert: false,
            smoothing_seconds: None,
            low_pass_hz: None,
            display_unit: None,
        }
    }
    pub fn apply(&self, value: f64) -> f64 {
        (if self.invert { -value } else { value }) * self.scale + self.offset
    }

    /// Resolves this binding's optional filter into a series suitable for
    /// caching. A frequency-based setting takes precedence over the legacy
    /// time-constant setting when both are present.
    pub fn filtered_series(&self, series: &ChannelSeries) -> Result<ChannelSeries, LowPassError> {
        match self.low_pass_hz {
            Some(hz) => series.low_pass_hz(hz),
            None => series.low_pass_optional(self.smoothing_seconds),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct SourceAlignment {
    /// `source_time = video_time + offset_seconds`.
    #[serde(default)]
    pub offset_seconds: f64,
}
impl SourceAlignment {
    pub fn source_time(self, video_time: f64) -> f64 {
        video_time + self.offset_seconds
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceInstance {
    pub id: SourceId,
    pub name: String,
    pub adapter: String,
    #[serde(default)]
    pub alignment: SourceAlignment,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn interpolation_and_gap() {
        let s = ChannelSeries::new(vec![
            TimedSample {
                time: 1.,
                value: 1.,
            },
            TimedSample {
                time: 0.,
                value: 0.,
            },
            TimedSample {
                time: 3.,
                value: 3.,
            },
        ]);
        assert_eq!(
            s.sample_at(
                0.5,
                GapPolicy {
                    max_gap_seconds: 2.
                },
                Interpolation::Linear
            ),
            Some(0.5)
        );
        assert_eq!(
            s.sample_at(2., GapPolicy::default(), Interpolation::Linear),
            None
        );
    }

    #[test]
    fn channel_ids_are_stable_per_source_and_name() {
        let source = SourceId::new();
        assert_eq!(
            ChannelId::for_source_name(source, "speed"),
            ChannelId::for_source_name(source, "speed")
        );
        assert_ne!(
            ChannelId::for_source_name(source, "speed"),
            ChannelId::for_source_name(source, "rpm")
        );
    }

    #[test]
    fn low_pass_is_zero_phase_and_preserves_sample_lookup() {
        let series = ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 0.0,
            },
            TimedSample {
                time: 1.0,
                value: 10.0,
            },
            TimedSample {
                time: 2.0,
                value: 10.0,
            },
        ])
        .with_gap(2.0);

        let filtered = series.low_pass(1.0).unwrap();
        // Forward/backward filtering has no one-sample causal lag. These are
        // the exact values for the short, uniformly sampled step.
        assert_close(filtered.samples[0].value, 2.6401558741395936);
        assert_close(filtered.samples[1].value, 7.176687736973064);
        assert_close(filtered.samples[2].value, 8.646647167633873);
        assert_close(
            filtered
                .sample_at_default(1.5, Interpolation::Linear)
                .unwrap(),
            (filtered.samples[1].value + filtered.samples[2].value) / 2.0,
        );
    }

    #[test]
    fn low_pass_resets_at_source_gap_and_respects_optional_setting() {
        let series = ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 0.0,
            },
            TimedSample {
                time: 0.1,
                value: 10.0,
            },
            TimedSample {
                time: 2.0,
                value: 20.0,
            },
        ])
        .with_gap(0.5);

        let filtered = series.low_pass(1.0).unwrap();
        assert!(filtered.samples[1].value > 0.0 && filtered.samples[1].value < 10.0);
        assert_eq!(filtered.samples[2].value, 20.0);
        assert_eq!(series.low_pass_optional(None).unwrap(), series);
    }

    #[test]
    fn low_pass_preserves_constants_and_does_not_bleed_across_gaps() {
        let first_segment = ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 1.0,
            },
            TimedSample {
                time: 0.1,
                value: 4.0,
            },
            TimedSample {
                time: 0.2,
                value: 1.0,
            },
        ])
        .with_gap(0.5);
        let with_gap = ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 1.0,
            },
            TimedSample {
                time: 0.1,
                value: 4.0,
            },
            TimedSample {
                time: 0.2,
                value: 1.0,
            },
            TimedSample {
                time: 2.0,
                value: 100.0,
            },
            TimedSample {
                time: 2.1,
                value: 100.0,
            },
        ])
        .with_gap(0.5);

        let constant = ChannelSeries::new(
            (0..20)
                .map(|index| TimedSample {
                    time: index as f64 * 0.1,
                    value: 7.5,
                })
                .collect(),
        )
        .low_pass(0.4)
        .unwrap();
        assert!(
            constant
                .samples
                .iter()
                .all(|sample| (sample.value - 7.5).abs() < 1e-12)
        );

        let filtered_first = first_segment.low_pass(0.4).unwrap();
        let filtered_with_gap = with_gap.low_pass(0.4).unwrap();
        for (left, right) in filtered_first
            .samples
            .iter()
            .zip(filtered_with_gap.samples.iter())
        {
            assert_close(left.value, right.value);
        }
        assert!(
            filtered_with_gap.samples[3..]
                .iter()
                .all(|sample| (sample.value - 100.0).abs() < 1e-12)
        );
    }

    #[test]
    fn low_pass_hz_has_zero_phase_and_final_two_pass_cutoff() {
        let frequency = 2.0;
        let sample_rate = 200.0;
        let samples: Vec<_> = (0..8_000)
            .map(|index| {
                let time = index as f64 / sample_rate;
                TimedSample {
                    time,
                    value: (std::f64::consts::TAU * frequency * time).sin(),
                }
            })
            .collect();
        let series = ChannelSeries::new(samples);
        let filtered = series.low_pass_hz(frequency).unwrap();

        // Ignore startup/end effects and estimate the sinusoid amplitude by
        // projection. At the configured cutoff, the complete forward/backward
        // response should be approximately -3 dB.
        let start = 1_000;
        let end = filtered.samples.len() - start;
        let (input_energy, output_projection, quadrature_projection) =
            (start..end).fold((0.0, 0.0, 0.0), |acc, index| {
                let phase = std::f64::consts::TAU * frequency * filtered.samples[index].time;
                let reference = phase.sin();
                let quadrature = phase.cos();
                (
                    acc.0 + reference * reference,
                    acc.1 + filtered.samples[index].value * reference,
                    acc.2 + filtered.samples[index].value * quadrature,
                )
            });
        let amplitude = output_projection / input_energy;
        assert!((amplitude - 1.0 / 2.0_f64.sqrt()).abs() < 0.01);
        assert!(quadrature_projection.abs() / input_energy < 0.001);

        // A symmetric impulse remains symmetric after zero-phase filtering.
        let impulse = ChannelSeries::new(
            (0..101)
                .map(|index| TimedSample {
                    time: index as f64 * 0.01,
                    value: if index == 50 { 1.0 } else { 0.0 },
                })
                .collect(),
        )
        .low_pass_hz(2.0)
        .unwrap();
        for offset in 1..=20 {
            assert!(
                (impulse.samples[50 - offset].value - impulse.samples[50 + offset].value).abs()
                    < 1e-7
            );
        }
    }

    #[test]
    fn low_pass_rejects_invalid_time_constants() {
        let series = ChannelSeries::new(vec![]);
        assert_eq!(
            series.low_pass(0.0),
            Err(LowPassError::InvalidSmoothingSeconds(0.0))
        );
        assert!(matches!(
            series.low_pass(f64::NAN),
            Err(LowPassError::InvalidSmoothingSeconds(value)) if value.is_nan()
        ));
        assert_eq!(
            series.low_pass_hz(0.0),
            Err(LowPassError::InvalidCutoffHz(0.0))
        );
    }

    #[test]
    fn binding_prefers_frequency_filter_over_legacy_time_constant() {
        let source = SourceId::new();
        let mut binding = ChannelBinding::new(
            "speed",
            ChannelRef {
                source_id: source,
                channel_id: ChannelId::new(),
            },
        );
        binding.smoothing_seconds = Some(1.0);
        binding.low_pass_hz = Some(2.0);
        let series = ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 0.0,
            },
            TimedSample {
                time: 0.1,
                value: 10.0,
            },
        ]);

        assert_eq!(
            binding.filtered_series(&series).unwrap(),
            series.low_pass_hz(2.0).unwrap()
        );
    }

    #[test]
    fn converts_supported_units() {
        assert_close(
            Unit::KilometerPerHour
                .convert_value_to(36.0, &Unit::MeterPerSecond)
                .unwrap(),
            10.0,
        );
        assert_close(
            Unit::Celsius
                .convert_value_to(0.0, &Unit::Fahrenheit)
                .unwrap(),
            32.0,
        );
        assert_close(
            Unit::Mile.convert_value_to(1.0, &Unit::Foot).unwrap(),
            5_280.0,
        );
        assert_close(
            Unit::StandardGravity
                .convert_value_to(1.0, &Unit::FootPerSecondSquared)
                .unwrap(),
            32.174_048_556_430_45,
        );
        assert_close(
            Unit::Bar.convert_value_to(1.0, &Unit::Psi).unwrap(),
            14.503_773_773,
        );
        assert_close(
            Unit::Degree.convert_value_to(180.0, &Unit::Radian).unwrap(),
            std::f64::consts::PI,
        );
        assert_close(
            Unit::DegreePerSecond
                .convert_value_to(180.0, &Unit::RadianPerSecond)
                .unwrap(),
            std::f64::consts::PI,
        );
        assert_close(
            Unit::Second
                .convert_value_to(1.0, &Unit::Millisecond)
                .unwrap(),
            1_000.0,
        );
    }

    #[test]
    fn rejects_incompatible_unit_conversions_but_allows_identical_custom_units() {
        assert_eq!(
            Unit::MeterPerSecond.convert_value_to(1.0, &Unit::Celsius),
            Err(UnitConversionError::Incompatible {
                from: Unit::MeterPerSecond,
                to: Unit::Celsius,
            })
        );
        let custom = Unit::Custom("boost".into());
        assert_eq!(custom.convert_value_to(1.25, &custom), Ok(1.25));
    }

    #[test]
    fn selects_metric_and_imperial_display_defaults() {
        assert_eq!(
            Unit::default_for(&Quantity::Speed, UnitSystem::Metric),
            Some(Unit::KilometerPerHour)
        );
        assert_eq!(
            Unit::default_for(&Quantity::Speed, UnitSystem::Imperial),
            Some(Unit::MilePerHour)
        );
        assert_eq!(
            UnitSystem::Imperial.default_unit_for(&Quantity::Altitude),
            Some(Unit::Foot)
        );
        assert_eq!(
            UnitSystem::Metric.default_unit_for(&Quantity::Acceleration),
            Some(Unit::StandardGravity)
        );
        assert_eq!(
            UnitSystem::Imperial.default_unit_for(&Quantity::Pressure),
            Some(Unit::Psi)
        );
        assert_eq!(UnitSystem::Metric.default_unit_for(&Quantity::Rpm), None);
    }

    #[test]
    fn new_unit_variants_and_optional_binding_fields_are_serde_compatible() {
        assert_eq!(
            serde_json::from_str::<Unit>("\"foot_per_second_squared\"").unwrap(),
            Unit::FootPerSecondSquared
        );

        let source = SourceId::new();
        let binding: ChannelBinding = serde_json::from_value(serde_json::json!({
            "slot": "speed",
            "channel": { "source_id": source, "channel_id": ChannelId::new() }
        }))
        .unwrap();
        assert_eq!(binding.smoothing_seconds, None);
        assert_eq!(binding.low_pass_hz, None);
        assert_eq!(binding.display_unit, None);
        assert_eq!(binding.scale, 1.0);
    }

    #[test]
    fn compatible_units_are_safe_for_display_pickers() {
        assert_eq!(
            Unit::Meter.compatible_units(),
            vec![Unit::Meter, Unit::Kilometer, Unit::Foot, Unit::Mile]
        );
        assert_eq!(
            Unit::Custom("boost".into()).compatible_units(),
            vec![Unit::Custom("boost".into())]
        );
        assert!(Unit::Bar.is_compatible_with(&Unit::Psi));
        assert!(!Unit::Bar.is_compatible_with(&Unit::MilePerHour));
    }

    fn correlation_shape(time: f64) -> f64 {
        let braking_pulse = (-((time - 16.3) / 1.4).powi(2)).exp() * -1.8;
        let cornering_pulse = (-((time - 39.7) / 2.2).powi(2)).exp() * 1.3;
        (time * 0.41).sin()
            + 0.37 * (time * 1.71).cos()
            + 0.16 * (time * 3.13).sin()
            + braking_pulse
            + cornering_pulse
    }

    fn irregular_series(
        start: f64,
        end: f64,
        base_step: f64,
        value: impl Fn(f64) -> f64,
    ) -> ChannelSeries {
        let mut samples = Vec::new();
        let mut time = start;
        let mut index = 0usize;
        while time <= end {
            samples.push(TimedSample {
                time,
                value: value(time),
            });
            time += base_step * (0.72 + (index % 7) as f64 * 0.075);
            index += 1;
        }
        ChannelSeries::new(samples)
    }

    fn regular_series(
        start: f64,
        end: f64,
        step: f64,
        value: impl Fn(f64) -> f64,
    ) -> ChannelSeries {
        let count = ((end - start) / step).round() as usize;
        ChannelSeries::new(
            (0..=count)
                .map(|index| {
                    let time = start + index as f64 * step;
                    TimedSample {
                        time,
                        value: value(time),
                    }
                })
                .collect(),
        )
    }

    fn correlation_config(min_lag_seconds: f64, max_lag_seconds: f64) -> CorrelationConfig {
        CorrelationConfig {
            min_lag_seconds,
            max_lag_seconds,
            lag_resolution_seconds: Some(0.01),
            min_overlap_seconds: 20.0,
            min_samples: 100,
            ..CorrelationConfig::default()
        }
    }

    #[test]
    fn correlation_finds_positive_offset_with_mismatched_irregular_rates() {
        let expected_lag = 0.37;
        let reference = irregular_series(0.0, 65.0, 0.047, correlation_shape);
        // target(t + expected_lag) == reference(t)
        let target = irregular_series(-2.0, 68.0, 0.113, |time| {
            correlation_shape(time - expected_lag)
        });
        let result =
            correlate_channel_series(&reference, &target, &correlation_config(-1.0, 1.0)).unwrap();

        assert_close(result.target_minus_reference_seconds, expected_lag);
        assert!(result.correlation_coefficient > 0.999);
        assert!(result.sample_count > 400);
        assert!(result.overlap_seconds > 60.0);
        assert_close(result.target_offset_from_reference(1.25), 1.62);
        assert_close(result.target_offset_adjustment(1.25, 0.5), 1.12);
    }

    #[test]
    fn correlation_finds_negative_offset() {
        let expected_lag = -0.43;
        let reference = irregular_series(0.0, 65.0, 0.071, correlation_shape);
        let target = irregular_series(-2.0, 68.0, 0.039, |time| {
            correlation_shape(time - expected_lag)
        });
        let result =
            correlate_channel_series(&reference, &target, &correlation_config(-1.0, 1.0)).unwrap();

        assert_close(result.target_minus_reference_seconds, expected_lag);
        assert!(result.correlation_coefficient > 0.999);
    }

    #[test]
    fn absolute_correlation_can_align_an_inverted_sensor() {
        let expected_lag = 0.28;
        let reference = irregular_series(0.0, 65.0, 0.052, correlation_shape);
        let target = irregular_series(-2.0, 68.0, 0.086, |time| {
            -correlation_shape(time - expected_lag)
        });
        let mut config = correlation_config(-1.0, 1.0);
        config.use_absolute_correlation = true;
        let result = correlate_channel_series(&reference, &target, &config).unwrap();

        assert_close(result.target_minus_reference_seconds, expected_lag);
        assert!(result.correlation_coefficient < -0.999);
    }

    #[test]
    fn correlation_rejects_insufficient_overlap() {
        let reference = regular_series(0.0, 10.0, 0.1, correlation_shape);
        let target = regular_series(0.0, 10.0, 0.1, correlation_shape);
        let config = CorrelationConfig {
            min_lag_seconds: -1.0,
            max_lag_seconds: 1.0,
            lag_resolution_seconds: Some(0.1),
            min_overlap_seconds: 2.0,
            min_samples: 10_000,
            ..CorrelationConfig::default()
        };
        assert_eq!(
            correlate_channel_series(&reference, &target, &config),
            Err(CorrelationError::InsufficientOverlap {
                min_samples: 10_000,
                min_overlap_seconds: 2.0,
            })
        );
    }

    #[test]
    fn correlation_caps_huge_requested_range_to_short_series_overlap() {
        let expected_lag = 2.0;
        let reference = regular_series(0.0, 10.0, 0.04, correlation_shape);
        let target = regular_series(2.0, 12.0, 0.04, |time| {
            correlation_shape(time - expected_lag)
        });
        let config = CorrelationConfig {
            min_lag_seconds: -1_000_000.0,
            max_lag_seconds: 1_000_000.0,
            lag_resolution_seconds: Some(0.01),
            min_overlap_seconds: 5.0,
            min_samples: 50,
            ..CorrelationConfig::default()
        };

        let result = correlate_channel_series(&reference, &target, &config).unwrap();
        assert_close(result.target_minus_reference_seconds, expected_lag);
        assert!(result.correlation_coefficient > 0.999);
    }

    #[test]
    fn correlation_caps_partial_requested_range_and_finds_shift() {
        let expected_lag = 5.0;
        let reference = regular_series(0.0, 10.0, 0.04, correlation_shape);
        let target = regular_series(2.0, 12.0, 0.04, |time| {
            correlation_shape(time - expected_lag)
        });
        let config = CorrelationConfig {
            min_lag_seconds: 0.0,
            max_lag_seconds: 100.0,
            lag_resolution_seconds: Some(0.01),
            min_overlap_seconds: 5.0,
            min_samples: 50,
            ..CorrelationConfig::default()
        };

        let result = correlate_channel_series(&reference, &target, &config).unwrap();
        assert_close(result.target_minus_reference_seconds, expected_lag);
        assert!(result.correlation_coefficient > 0.999);
    }

    #[test]
    fn correlation_rejects_requested_range_disjoint_from_feasible_overlap() {
        let reference = regular_series(0.0, 10.0, 0.1, correlation_shape);
        let target = regular_series(2.0, 12.0, 0.1, correlation_shape);
        let config = CorrelationConfig {
            min_lag_seconds: -100.0,
            max_lag_seconds: -10.0,
            lag_resolution_seconds: Some(0.01),
            min_overlap_seconds: 5.0,
            min_samples: 10,
            ..CorrelationConfig::default()
        };

        assert_eq!(
            correlate_channel_series(&reference, &target, &config),
            Err(CorrelationError::NoFeasibleLagOverlap {
                requested_min_lag_seconds: -100.0,
                requested_max_lag_seconds: -10.0,
                feasible_min_lag_seconds: -3.0,
                feasible_max_lag_seconds: 7.0,
            })
        );
    }

    #[test]
    fn correlation_caps_negative_lag_search_to_feasible_overlap() {
        let expected_lag = -2.0;
        let reference = regular_series(0.0, 10.0, 0.04, correlation_shape);
        let target = regular_series(-2.0, 8.0, 0.04, |time| {
            correlation_shape(time - expected_lag)
        });
        let config = CorrelationConfig {
            min_lag_seconds: -1_000_000.0,
            max_lag_seconds: 1_000_000.0,
            lag_resolution_seconds: Some(0.01),
            min_overlap_seconds: 5.0,
            min_samples: 50,
            ..CorrelationConfig::default()
        };

        let result = correlate_channel_series(&reference, &target, &config).unwrap();
        assert_close(result.target_minus_reference_seconds, expected_lag);
        assert!(result.correlation_coefficient > 0.999);
    }

    #[test]
    fn correlation_coarsens_automatic_resolution_for_a_broad_feasible_range() {
        let reference = ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 0.0,
            },
            TimedSample {
                time: 5_000.0,
                value: 1.0,
            },
            TimedSample {
                time: 10_000.0,
                value: 0.0,
            },
        ]);
        let target = reference.clone();
        let config = CorrelationConfig {
            min_lag_seconds: -100_000.0,
            max_lag_seconds: 100_000.0,
            lag_resolution_seconds: None,
            min_overlap_seconds: 1.0,
            min_samples: 2,
            detrend: false,
            ..CorrelationConfig::default()
        };

        let result = correlate_channel_series(&reference, &target, &config).unwrap();
        assert!(result.lag_resolution_seconds > 0.1);
        assert!(result.lag_resolution_seconds < 2.0);
        assert!(result.sample_count >= 2);
    }

    #[test]
    fn correlation_rejects_constant_channels() {
        let reference = irregular_series(0.0, 30.0, 0.08, |_| 12.0);
        let target = irregular_series(-1.0, 31.0, 0.12, |_| 45.0);
        let config = CorrelationConfig {
            min_lag_seconds: -0.5,
            max_lag_seconds: 0.5,
            lag_resolution_seconds: Some(0.05),
            min_overlap_seconds: 10.0,
            min_samples: 50,
            ..CorrelationConfig::default()
        };
        assert_eq!(
            correlate_channel_series(&reference, &target, &config),
            Err(CorrelationError::ConstantSignal {
                detrending: "linear"
            })
        );
    }

    #[test]
    fn correlation_chooses_an_automatic_sensible_resolution() {
        let reference = irregular_series(0.0, 30.0, 0.04, correlation_shape);
        let target = irregular_series(-1.0, 31.0, 0.08, |time| correlation_shape(time - 0.2));
        let config = CorrelationConfig {
            min_lag_seconds: -0.5,
            max_lag_seconds: 0.5,
            lag_resolution_seconds: None,
            min_overlap_seconds: 10.0,
            min_samples: 50,
            ..CorrelationConfig::default()
        };
        let result = correlate_channel_series(&reference, &target, &config).unwrap();
        assert!((0.005..=0.1).contains(&result.lag_resolution_seconds));
        assert!(
            (result.target_minus_reference_seconds - 0.2).abs()
                <= result.lag_resolution_seconds + 1e-9
        );
    }
}
