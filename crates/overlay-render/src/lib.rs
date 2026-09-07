//! Deterministic, platform-independent rendering for telemetry overlays.

pub use overlay_core::NormalizedRect;
use overlay_core::{
    ChannelBinding, ChannelSeries, GapPolicy, Interpolation, SourceId, TelemetryDataset,
    WidgetConfig, ZERO_PHASE_CUTOFF_COMPENSATION,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

pub mod bitmap_font;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl PixelRect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    pub fn right(self) -> i32 {
        self.x.saturating_add(self.width as i32)
    }
    pub fn bottom(self) -> i32 {
        self.y.saturating_add(self.height as i32)
    }
    fn union(self, o: Self) -> Self {
        let l = self.x.min(o.x);
        let t = self.y.min(o.y);
        let r = self.right().max(o.right());
        let b = self.bottom().max(o.bottom());
        Self::new(l, t, r.saturating_sub(l) as u32, b.saturating_sub(t) as u32)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderSize {
    pub width: u32,
    pub height: u32,
}
impl RenderSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
impl RgbaImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
    pub fn into_raw(self) -> Vec<u8> {
        self.pixels
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderResult {
    pub image: RgbaImage,
    pub bounds: Option<PixelRect>,
    pub full_size: Option<RgbaImage>,
}
impl RenderResult {
    pub fn cropped(&self) -> &RgbaImage {
        &self.image
    }
    pub fn full_image(&self) -> Option<&RgbaImage> {
        self.full_size.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba(pub u8, pub u8, pub u8, pub u8);
impl Rgba {
    pub const WHITE: Self = Self(255, 255, 255, 255);
    pub const BLACK: Self = Self(0, 0, 0, 255);
    pub const TRANSPARENT: Self = Self(0, 0, 0, 0);
    fn with_alpha(self, a: f32) -> Self {
        Self(
            self.0,
            self.1,
            self.2,
            ((self.3 as f32) * a.clamp(0., 1.)).round() as u8,
        )
    }
    pub(crate) fn sk(self) -> Color {
        Color::from_rgba8(self.0, self.1, self.2, self.3)
    }
}
impl Default for Rgba {
    fn default() -> Self {
        Self::WHITE
    }
}
impl Serialize for Rgba {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        [self.0, self.1, self.2, self.3].serialize(s)
    }
}
impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Vec::<u8>::deserialize(d)?;
        match v.as_slice() {
            [red, green, blue] => Ok(Self(*red, *green, *blue, 255)),
            [red, green, blue, alpha] => Ok(Self(*red, *green, *blue, *alpha)),
            _ => Err(serde::de::Error::custom(
                "an RGBA color must contain three (RGB) or four (RGBA) channels",
            )),
        }
    }
}

/// Fully resolved project appearance.  Values are intentionally public so a
/// host with custom widgets can use the exact same semantic palette as the
/// built-in renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AppearancePalette {
    pub accent: Rgba,
    pub text: Rgba,
    pub background: Rgba,
    pub muted: Rgba,
    pub positive: Rgba,
    pub warning: Rgba,
    pub critical: Rgba,
    pub foreground_opacity: f32,
    pub background_opacity: f32,
    /// Fraction of the smaller widget dimension, in the range 0..=0.5.
    pub corner_radius: f32,
}

impl AppearancePalette {
    pub const fn race_dark() -> Self {
        Self {
            accent: Rgba(0, 218, 255, 255),
            text: Rgba(255, 255, 255, 255),
            background: Rgba(10, 15, 22, 255),
            muted: Rgba(100, 110, 125, 255),
            positive: Rgba(60, 225, 133, 255),
            warning: Rgba(247, 176, 50, 255),
            critical: Rgba(245, 77, 59, 255),
            foreground_opacity: 0.92,
            background_opacity: 0.92,
            corner_radius: 0.12,
        }
    }
    pub const fn light() -> Self {
        Self {
            accent: Rgba(0, 120, 190, 255),
            text: Rgba(24, 32, 44, 255),
            background: Rgba(242, 246, 250, 255),
            muted: Rgba(103, 116, 133, 255),
            positive: Rgba(20, 150, 85, 255),
            warning: Rgba(206, 128, 0, 255),
            critical: Rgba(202, 54, 45, 255),
            foreground_opacity: 0.92,
            background_opacity: 0.92,
            corner_radius: 0.12,
        }
    }
    pub const fn transparent() -> Self {
        Self {
            // Keep RGB meaningful for a user who later raises the opacity;
            // opacity, rather than a hidden per-color alpha, is the style
            // control for newly-authored projects.
            background_opacity: 0.,
            ..Self::race_dark()
        }
    }
}
impl Default for AppearancePalette {
    fn default() -> Self {
        Self::race_dark()
    }
}

/// Resolve a schema-free project `appearance` JSON object into the semantic
/// palette used by the renderer. `preset` may be `race_dark`/`current`,
/// `light`, or `transparent`; explicit values always win over the preset.
/// `panel` is accepted as an alias for `background`.
pub fn resolve_appearance(appearance: &Value) -> AppearancePalette {
    let preset = appearance
        .get("preset")
        .and_then(Value::as_str)
        .unwrap_or("race_dark")
        .trim()
        .to_ascii_lowercase();
    let mut palette = match preset.as_str() {
        "light" => AppearancePalette::light(),
        "transparent" => AppearancePalette::transparent(),
        "current" | "race_dark" | "race-dark" | "dark" => AppearancePalette::race_dark(),
        _ => AppearancePalette::race_dark(),
    };
    macro_rules! color_value {
        ($field:ident, $key:literal) => {
            if let Some(value) = color(appearance, $key) {
                palette.$field = value;
            }
        };
    }
    color_value!(accent, "accent");
    color_value!(text, "text");
    palette.background = color(appearance, "background")
        .or_else(|| color(appearance, "panel"))
        .unwrap_or(palette.background);
    color_value!(muted, "muted");
    color_value!(positive, "positive");
    color_value!(warning, "warning");
    color_value!(critical, "critical");
    if let Some(value) = number(appearance, "foreground_opacity") {
        palette.foreground_opacity = value.clamp(0., 1.) as f32;
    }
    if let Some(value) = number(appearance, "background_opacity") {
        palette.background_opacity = value.clamp(0., 1.) as f32;
    }
    if let Some(value) = number(appearance, "corner_radius") {
        palette.corner_radius = value.clamp(0., 0.5) as f32;
    }
    palette
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Widget {
    #[serde(default)]
    pub id: String,
    #[serde(alias = "type", alias = "widget_type")]
    pub type_id: String,
    pub rect: NormalizedRect,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_opacity")]
    pub background_opacity: f32,
    #[serde(default = "default_background")]
    pub background: Rgba,
    #[serde(default = "default_accent")]
    pub accent: Rgba,
    #[serde(default = "default_text")]
    pub text_color: Rgba,
    #[serde(default = "default_muted")]
    pub muted: Rgba,
    #[serde(default = "default_positive")]
    pub positive: Rgba,
    #[serde(default = "default_warning")]
    pub warning: Rgba,
    #[serde(default = "default_critical")]
    pub critical: Rgba,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default, alias = "offset_seconds", alias = "source_time_offset")]
    pub source_offset: f64,
    #[serde(default)]
    pub value_slot: String,
    #[serde(default)]
    pub x_slot: String,
    #[serde(default)]
    pub y_slot: String,
    /// GPS bindings used by `track_map`. These remain separate from the
    /// general-purpose x/y slots so a project can use both widget kinds.
    #[serde(default)]
    pub latitude_slot: String,
    #[serde(default)]
    pub longitude_slot: String,
    #[serde(skip)]
    track_map: TrackMap,
    #[serde(skip)]
    core_bindings: Option<Vec<ChannelBinding>>,
}
fn default_opacity() -> f32 {
    AppearancePalette::race_dark().foreground_opacity
}
fn default_background() -> Rgba {
    AppearancePalette::race_dark().background
}
fn default_accent() -> Rgba {
    AppearancePalette::race_dark().accent
}
fn default_text() -> Rgba {
    Rgba::WHITE
}
fn default_muted() -> Rgba {
    AppearancePalette::race_dark().muted
}
fn default_positive() -> Rgba {
    AppearancePalette::race_dark().positive
}
fn default_warning() -> Rgba {
    AppearancePalette::race_dark().warning
}
fn default_critical() -> Rgba {
    AppearancePalette::race_dark().critical
}
fn default_corner_radius() -> f32 {
    AppearancePalette::race_dark().corner_radius
}
impl Default for Widget {
    fn default() -> Self {
        Self {
            id: String::new(),
            type_id: "numeric".into(),
            rect: NormalizedRect::default(),
            opacity: AppearancePalette::race_dark().foreground_opacity,
            background_opacity: AppearancePalette::race_dark().background_opacity,
            background: default_background(),
            accent: default_accent(),
            text_color: default_text(),
            muted: default_muted(),
            positive: default_positive(),
            warning: default_warning(),
            critical: default_critical(),
            corner_radius: default_corner_radius(),
            label: String::new(),
            unit: String::new(),
            format: String::new(),
            min: None,
            max: None,
            source_offset: 0.,
            value_slot: String::new(),
            x_slot: String::new(),
            y_slot: String::new(),
            latitude_slot: String::new(),
            longitude_slot: String::new(),
            track_map: TrackMap::default(),
            core_bindings: None,
        }
    }
}

impl Widget {
    pub fn new(type_id: impl Into<String>, rect: NormalizedRect) -> Self {
        Self {
            type_id: type_id.into(),
            rect,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum TrackMode {
    #[default]
    Auto,
    Circuit,
    PointToPoint,
}

#[derive(Clone, Debug)]
struct TrackMap {
    mode: TrackMode,
    /// 0/absent means automatic; positive values select a 1-based full lap.
    lap: Option<usize>,
    start: Option<(f64, f64)>,
    finish: Option<(f64, f64)>,
    line_color: Option<Rgba>,
    position_color: Option<Rgba>,
    start_color: Option<Rgba>,
    finish_color: Option<Rgba>,
    line_width: Option<f32>,
    marker_size: Option<f32>,
    show_markers: bool,
    rotation_degrees: f32,
    padding: Option<f32>,
    /// The selected single traversal, in latitude/longitude coordinates.
    points: Vec<GeoPoint>,
}
impl Default for TrackMap {
    fn default() -> Self {
        Self {
            mode: TrackMode::Auto,
            lap: None,
            start: None,
            finish: None,
            line_color: None,
            position_color: None,
            start_color: None,
            finish_color: None,
            line_width: None,
            marker_size: None,
            show_markers: true,
            rotation_degrees: 0.,
            padding: None,
            points: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct GeoPoint {
    time: f64,
    latitude: f64,
    longitude: f64,
}

pub trait ValueResolver {
    fn resolve(&self, slot: &str, source_offset: f64) -> Option<f64>;

    /// Complete, timestamped samples for a slot. Scalar-only resolvers need
    /// not implement this; the track map simply remains empty for them.
    fn resolve_series(&self, _slot: &str) -> Option<Vec<(f64, f64)>> {
        None
    }
}
impl<F> ValueResolver for F
where
    F: Fn(&str, f64) -> Option<f64>,
{
    fn resolve(&self, slot: &str, source_offset: f64) -> Option<f64> {
        self(slot, source_offset)
    }
}
#[derive(Clone, Debug, Default)]
pub struct DatasetContext {
    pub samples: HashMap<String, Vec<(f64, f64)>>,
}
impl DatasetContext {
    pub fn insert(
        &mut self,
        slot: impl Into<String>,
        samples: impl IntoIterator<Item = (f64, f64)>,
    ) {
        let mut s: Vec<_> = samples.into_iter().collect();
        s.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.samples.insert(slot.into(), s);
    }
    pub fn with_samples(
        mut self,
        slot: impl Into<String>,
        samples: impl IntoIterator<Item = (f64, f64)>,
    ) -> Self {
        self.insert(slot, samples);
        self
    }
}
impl ValueResolver for DatasetContext {
    fn resolve(&self, slot: &str, at: f64) -> Option<f64> {
        let data = self.samples.get(slot)?;
        if data.is_empty() {
            return None;
        }
        match data.binary_search_by(|x| x.0.total_cmp(&at)) {
            Ok(i) => Some(data[i].1),
            Err(0) => None,
            Err(i) if i >= data.len() => None,
            Err(i) => {
                let (a, b) = (data[i - 1], data[i]);
                let t = (at - a.0) / (b.0 - a.0);
                Some(a.1 + (b.1 - a.1) * t)
            }
        }
    }
    fn resolve_series(&self, slot: &str) -> Option<Vec<(f64, f64)>> {
        self.samples.get(slot).cloned()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub crop: bool,
    pub full_size: bool,
}
impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            crop: true,
            full_size: false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct WidgetRegistry {
    ids: Vec<String>,
}
/// Descriptive alias for callers that call this a renderer registry.
pub type RendererRegistry = WidgetRegistry;
impl WidgetRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, type_id: impl Into<String>) {
        let id = type_id.into();
        if !self.ids.iter().any(|x| x == &id) {
            self.ids.push(id)
        }
    }
    pub fn contains(&self, id: &str) -> bool {
        canonical(id).is_some_and(|x| self.ids.iter().any(|v| v == x))
    }
    pub fn type_ids(&self) -> impl Iterator<Item = &str> {
        self.ids.iter().map(String::as_str)
    }
}
impl Default for WidgetRegistry {
    fn default() -> Self {
        Self {
            ids: vec![
                "numeric".into(),
                "bar".into(),
                "radial".into(),
                "xy_dot".into(),
                "tachometer".into(),
                "temperature".into(),
                "lap_timer".into(),
                "delta".into(),
                "shift_lights".into(),
                "center_bar".into(),
                "gear".into(),
                "track_map".into(),
            ],
        }
    }
}

#[derive(Default)]
pub struct Renderer {
    pub registry: WidgetRegistry,
}
impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_registry(registry: WidgetRegistry) -> Self {
        Self { registry }
    }
    pub fn render<R: ValueResolver>(
        &self,
        widgets: &[Widget],
        size: RenderSize,
        resolver: &R,
        source_offset: f64,
        options: RenderOptions,
    ) -> RenderResult {
        let mut pm = Pixmap::new(size.width.max(1), size.height.max(1)).unwrap();
        for w in widgets {
            self.paint_widget(&mut pm, w, size, resolver, source_offset)
        }
        let full = rgba_image(&pm);
        let bounds = alpha_bounds(&full);
        let image = if options.crop {
            crop_image(&full, bounds)
        } else {
            full.clone()
        };
        RenderResult {
            image,
            bounds,
            full_size: options.full_size.then_some(full),
        }
    }
    pub fn render_cropped<R: ValueResolver>(
        &self,
        widgets: &[Widget],
        size: RenderSize,
        resolver: &R,
        source_offset: f64,
    ) -> RenderResult {
        self.render(
            widgets,
            size,
            resolver,
            source_offset,
            RenderOptions::default(),
        )
    }
    pub fn render_full<R: ValueResolver>(
        &self,
        widgets: &[Widget],
        size: RenderSize,
        resolver: &R,
        source_offset: f64,
    ) -> RenderResult {
        self.render(
            widgets,
            size,
            resolver,
            source_offset,
            RenderOptions {
                crop: false,
                full_size: true,
            },
        )
    }
    fn paint_widget<R: ValueResolver>(
        &self,
        pm: &mut Pixmap,
        w: &Widget,
        size: RenderSize,
        resolver: &R,
        source: f64,
    ) {
        let Some(kind) = canonical(&w.type_id) else {
            return;
        };
        if !self.registry.contains(kind) {
            return;
        }
        let r = pixel_rect(w.rect, size);
        if r.width == 0 || r.height == 0 {
            return;
        }
        let op = w.opacity.clamp(0., 1.);
        let bg = w.background.with_alpha(w.background_opacity.clamp(0., 1.));
        let ac = w.accent.with_alpha(op);
        let tx = w.text_color.with_alpha(op);
        let muted = w.muted.with_alpha(op);
        let positive = w.positive.with_alpha(op);
        let warning = w.warning.with_alpha(op);
        let critical = w.critical.with_alpha(op);
        fill_round(
            pm,
            r,
            r.width.min(r.height) as f32 * w.corner_radius.clamp(0., 0.5),
            bg,
        );
        let at = source + w.source_offset;
        match kind {
            "numeric" => paint_numeric(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
            ),
            "bar" => paint_bar(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
                muted,
            ),
            "radial" => paint_radial(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
                muted,
            ),
            "xy_dot" => paint_xy(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.x_slot, "x"), at),
                resolver.resolve(slot_or(&w.y_slot, "y"), at),
                ac,
                tx,
                muted,
            ),
            "tachometer" => paint_tachometer(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
                muted,
                warning,
                critical,
            ),
            "temperature" => paint_temperature(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
                muted,
                warning,
                critical,
            ),
            "lap_timer" => paint_lap_timer(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                tx,
            ),
            "delta" => paint_delta(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                tx,
                muted,
                positive,
                critical,
            ),
            "shift_lights" => paint_shift_lights(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
                muted,
                warning,
                critical,
            ),
            "center_bar" => paint_center_bar(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
                muted,
            ),
            "gear" => paint_gear(
                pm,
                w,
                r,
                resolver.resolve(slot_or(&w.value_slot, "value"), at),
                ac,
                tx,
            ),
            "track_map" => {
                let latitude_slot = slot_or(&w.latitude_slot, "latitude");
                let longitude_slot = slot_or(&w.longitude_slot, "longitude");
                let points = if w.track_map.points.is_empty() {
                    build_track_points(
                        resolver.resolve_series(latitude_slot),
                        resolver.resolve_series(longitude_slot),
                        &w.track_map,
                    )
                } else {
                    w.track_map.points.clone()
                };
                paint_track_map(
                    pm,
                    w,
                    r,
                    &points,
                    resolver.resolve(latitude_slot, at),
                    resolver.resolve(longitude_slot, at),
                    ac,
                    tx,
                    positive,
                    critical,
                    op,
                );
            }
            _ => {}
        }
    }
}

/// Data source used by the project-level adapter. The offset passed to a
/// channel is always source time (`video_time + source_offset`).
pub trait ProjectValueResolver {
    fn resolve_binding(&self, binding: &ChannelBinding, video_time: f64) -> Option<f64>;

    /// Complete source-time samples for geometry widgets. The transform in the
    /// binding is applied, but source alignment is intentionally not: a course
    /// is a static shape while scalar sampling uses video time alignment.
    fn binding_samples(&self, _binding: &ChannelBinding) -> Option<Vec<(f64, f64)>> {
        None
    }

    /// Recorded lap boundaries for the binding's source. A `full` lap is more
    /// authoritative than inferred GPS crossings and omits out/in laps.
    fn lap_ranges(&self, _binding: &ChannelBinding) -> Vec<SourceLap> {
        Vec::new()
    }

    /// Resolve a binding, applying its optional zero-phase low-pass filter.
    ///
    /// The default keeps custom project resolvers backwards compatible. The
    /// built-in telemetry resolvers override it because they can access the
    /// complete channel series needed to evaluate a filter at an arbitrary
    /// render time.
    fn resolve_binding_smoothed(&self, binding: &ChannelBinding, video_time: f64) -> Option<f64> {
        self.resolve_binding(binding, video_time)
    }
}
impl ProjectValueResolver for TelemetryDataset {
    fn resolve_binding(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        if b.channel.source_id != self.source_id {
            return None;
        }
        let channel = self.channel(b.channel.channel_id)?;
        let value = channel.series.sample_at(
            video_time,
            GapPolicy::default(),
            channel.descriptor.interpolation,
        )?;
        Some(b.apply(value))
    }

    fn resolve_binding_smoothed(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        if b.channel.source_id != self.source_id {
            return None;
        }
        let channel = self.channel(b.channel.channel_id)?;
        let value = if let Some(tau) = binding_smoothing_seconds(b) {
            channel
                .series
                .low_pass_with_gap(tau, GapPolicy::default())
                .unwrap_or_else(|_| channel.series.clone())
                .sample_at(
                    video_time,
                    GapPolicy::default(),
                    channel.descriptor.interpolation,
                )?
        } else {
            channel.series.sample_at(
                video_time,
                GapPolicy::default(),
                channel.descriptor.interpolation,
            )?
        };
        Some(b.apply(value))
    }

    fn binding_samples(&self, b: &ChannelBinding) -> Option<Vec<(f64, f64)>> {
        if b.channel.source_id != self.source_id {
            return None;
        }
        let channel = self.channel(b.channel.channel_id)?;
        let filtered;
        let series = if let Some(tau) = binding_smoothing_seconds(b) {
            filtered = channel
                .series
                .low_pass_with_gap(tau, GapPolicy::default())
                .unwrap_or_else(|_| channel.series.clone());
            &filtered
        } else {
            &channel.series
        };
        Some(
            series
                .samples
                .iter()
                .map(|sample| (sample.time, b.apply(sample.value)))
                .collect(),
        )
    }

    fn lap_ranges(&self, b: &ChannelBinding) -> Vec<SourceLap> {
        if b.channel.source_id != self.source_id {
            return Vec::new();
        }
        self.laps
            .iter()
            .map(|lap| SourceLap {
                number: lap.number,
                start_time: lap.start_time,
                end_time: lap.end_time,
                lap_type: lap.lap_type.clone(),
            })
            .collect()
    }
}
impl ProjectValueResolver for &[TelemetryDataset] {
    fn resolve_binding(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)?
            .resolve_binding(b, video_time)
    }

    fn resolve_binding_smoothed(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)?
            .resolve_binding_smoothed(b, video_time)
    }
    fn binding_samples(&self, b: &ChannelBinding) -> Option<Vec<(f64, f64)>> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)?
            .binding_samples(b)
    }
    fn lap_ranges(&self, b: &ChannelBinding) -> Vec<SourceLap> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)
            .map(|dataset| dataset.lap_ranges(b))
            .unwrap_or_default()
    }
}
impl ProjectValueResolver for [TelemetryDataset] {
    fn resolve_binding(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)?
            .resolve_binding(b, video_time)
    }

    fn resolve_binding_smoothed(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)?
            .resolve_binding_smoothed(b, video_time)
    }
    fn binding_samples(&self, b: &ChannelBinding) -> Option<Vec<(f64, f64)>> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)?
            .binding_samples(b)
    }
    fn lap_ranges(&self, b: &ChannelBinding) -> Vec<SourceLap> {
        self.iter()
            .find(|d| d.source_id == b.channel.source_id)
            .map(|dataset| dataset.lap_ranges(b))
            .unwrap_or_default()
    }
}
/// A project resolver that carries per-source alignment offsets. This is the
/// convenient bridge when rendering a complete project with multiple sources.
#[derive(Default)]
pub struct AlignedDatasets<'a> {
    pub datasets: &'a [TelemetryDataset],
    pub source_offsets: HashMap<SourceId, f64>,
    pub gap_policy: GapPolicy,
    /// Precompute and retain complete filtered series. Enable this for export,
    /// where one resolver is reused for every frame. Interactive preview keeps
    /// this false and evaluates a symmetric bounded window around each frame;
    /// that preserves zero phase without filtering the entire source on every
    /// preview refresh.
    pub cache_filtered_series: bool,
    pub smoothing_cache: RefCell<HashMap<SmoothingKey, ChannelSeries>>,
}
impl<'a> ProjectValueResolver for AlignedDatasets<'a> {
    fn resolve_binding(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        let d = self
            .datasets
            .iter()
            .find(|d| d.source_id == b.channel.source_id)?;
        let t = video_time
            + self
                .source_offsets
                .get(&b.channel.source_id)
                .copied()
                .unwrap_or(0.);
        let channel = d.channel(b.channel.channel_id)?;
        let value =
            channel
                .series
                .sample_at(t, self.gap_policy, channel.descriptor.interpolation)?;
        Some(b.apply(value))
    }

    fn resolve_binding_smoothed(&self, b: &ChannelBinding, video_time: f64) -> Option<f64> {
        let d = self
            .datasets
            .iter()
            .find(|d| d.source_id == b.channel.source_id)?;
        let t = video_time
            + self
                .source_offsets
                .get(&b.channel.source_id)
                .copied()
                .unwrap_or(0.);
        let channel = d.channel(b.channel.channel_id)?;
        let tau = binding_smoothing_seconds(b);
        let value = if let Some(tau) = tau
            && self.cache_filtered_series
        {
            let key = SmoothingKey {
                source_id: b.channel.source_id,
                channel_id: b.channel.channel_id,
                smoothing_bits: tau.to_bits(),
            };
            let mut cache = self.smoothing_cache.borrow_mut();
            let filtered = cache.entry(key).or_insert_with(|| {
                channel
                    .series
                    .low_pass_with_gap(tau, self.gap_policy)
                    .unwrap_or_else(|_| channel.series.clone())
            });
            filtered.sample_at(t, self.gap_policy, channel.descriptor.interpolation)?
        } else if let Some(tau) = tau {
            zero_phase_sample_at(
                &channel.series,
                t,
                self.gap_policy,
                channel.descriptor.interpolation,
                tau,
            )?
        } else {
            channel
                .series
                .sample_at(t, self.gap_policy, channel.descriptor.interpolation)?
        };
        Some(b.apply(value))
    }

    fn binding_samples(&self, b: &ChannelBinding) -> Option<Vec<(f64, f64)>> {
        self.datasets
            .iter()
            .find(|d| d.source_id == b.channel.source_id)?
            .binding_samples(b)
    }

    fn lap_ranges(&self, b: &ChannelBinding) -> Vec<SourceLap> {
        self.datasets
            .iter()
            .find(|d| d.source_id == b.channel.source_id)
            .map(|dataset| dataset.lap_ranges(b))
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug)]
pub struct SourceLap {
    pub number: i32,
    pub start_time: f64,
    pub end_time: f64,
    pub lap_type: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SmoothingKey {
    source_id: SourceId,
    channel_id: overlay_core::ChannelId,
    smoothing_bits: u64,
}

/// Return the per-pass time constant used by the renderer. New bindings use a
/// final two-pass -3 dB cutoff (`low_pass_hz`), so its time constant includes
/// the same forward/backward compensation as `ChannelSeries::low_pass_hz`.
/// The older `smoothing_seconds` field is kept as a compatible per-pass
/// time-constant fallback. If both are present, cutoff Hz wins so projects can
/// migrate without applying two filters.
fn binding_smoothing_seconds(binding: &ChannelBinding) -> Option<f64> {
    if let Some(hz) = binding.low_pass_hz
        && hz.is_finite()
        && hz > 0.
    {
        return Some(1. / (std::f64::consts::TAU * hz * ZERO_PHASE_CUTOFF_COMPENSATION));
    }
    valid_smoothing_seconds(binding.smoothing_seconds)
}

fn valid_smoothing_seconds(value: Option<f64>) -> Option<f64> {
    value.filter(|seconds| seconds.is_finite() && *seconds > 0.)
}

/// Resolve a zero-phase filtered sample without retaining state.
///
/// Interactive preview recreates its [`AlignedDatasets`] resolver as the
/// playhead moves, so retaining a complete filtered series there would make
/// every refresh scan the whole telemetry stream. Instead, filter a symmetric
/// window around the query. Ten time constants on each side leaves less than
/// 0.005% of a first-order pass's impulse response outside the window. The
/// extra sample on each boundary preserves interpolation and gap detection.
fn zero_phase_sample_at(
    series: &ChannelSeries,
    time: f64,
    gap: GapPolicy,
    interpolation: Interpolation,
    smoothing_seconds: f64,
) -> Option<f64> {
    let samples = &series.samples;
    let first = *samples.first()?;
    let last = *samples.last()?;
    if time < first.time || time > last.time {
        return None;
    }
    let radius = smoothing_seconds * 10.0;
    let start_time = time - radius;
    let end_time = time + radius;
    let start = samples
        .partition_point(|sample| sample.time < start_time)
        .saturating_sub(1);
    let end = samples.partition_point(|sample| sample.time <= end_time);
    let window = ChannelSeries {
        samples: samples[start..end.max(start + 1)].to_vec(),
        gap_seconds: series.gap_seconds,
    };
    let filtered = window.low_pass_with_gap(smoothing_seconds, gap).ok()?;
    filtered.sample_at(time, gap, interpolation)
}

/// Project widgets converted to renderer-native form with static geometry
/// (notably a selected GPS route) prepared once for repeated frame rendering.
pub struct PreparedProjectWidgets {
    widgets: Vec<Widget>,
}

pub fn prepare_project_widgets<D: ProjectValueResolver>(
    widgets: &[WidgetConfig],
    data: &D,
) -> PreparedProjectWidgets {
    prepare_project_widgets_with_appearance(widgets, data, &Value::Null)
}

/// Convert project widgets and resolve their inherited project appearance once.
/// The returned value is suitable for repeated export-frame rendering: both
/// static geometry and all palette decisions are retained in it.
pub fn prepare_project_widgets_with_appearance<D: ProjectValueResolver>(
    widgets: &[WidgetConfig],
    data: &D,
    appearance: &Value,
) -> PreparedProjectWidgets {
    let palette = resolve_appearance(appearance);
    let mut converted: Vec<Widget> = widgets
        .iter()
        .map(|widget| widget_from_core_with_appearance(widget, palette))
        .collect();
    // The low-level renderer resolves slots by name. Give each project widget
    // a private key so two widgets both using `value` cannot steal one another's
    // channel binding.
    for (index, widget) in converted.iter_mut().enumerate() {
        widget.value_slot = format!("__render_{index}_value");
        widget.x_slot = format!("__render_{index}_x");
        widget.y_slot = format!("__render_{index}_y");
        widget.latitude_slot = format!("__render_{index}_latitude");
        widget.longitude_slot = format!("__render_{index}_longitude");
        if canonical(&widget.type_id) == Some("track_map") {
            let latitude = widget
                .bindings()
                .iter()
                .find(|binding| binding.slot == "latitude")
                .cloned();
            let longitude = widget
                .bindings()
                .iter()
                .find(|binding| binding.slot == "longitude")
                .cloned();
            if let (Some(latitude), Some(longitude)) = (latitude, longitude)
                && latitude.channel.source_id == longitude.channel.source_id
            {
                let points = build_track_points_with_laps(
                    data.binding_samples(&latitude),
                    data.binding_samples(&longitude),
                    &widget.track_map,
                    &data.lap_ranges(&latitude),
                );
                widget.track_map.points = points;
            }
        }
    }
    PreparedProjectWidgets { widgets: converted }
}

pub fn render_prepared_project_widgets<D: ProjectValueResolver>(
    prepared: &PreparedProjectWidgets,
    data: &D,
    size: RenderSize,
    video_time: f64,
    options: RenderOptions,
) -> RenderResult {
    let resolver = |slot: &str, at: f64| -> Option<f64> {
        // `at` is the project source time. Bindings are selected by slot.
        prepared
            .widgets
            .iter()
            .filter_map(|w| w._binding_value_for_render_slot(slot, data, at))
            .next()
    };
    Renderer::default().render(&prepared.widgets, size, &resolver, video_time, options)
}

/// Render core project widget declarations without making the application
/// maintain a second project schema. Style/settings remain forward-compatible
/// JSON in `overlay-core`; known fields are read here with conservative defaults.
pub fn render_project_widgets<D: ProjectValueResolver>(
    widgets: &[WidgetConfig],
    data: &D,
    size: RenderSize,
    video_time: f64,
    options: RenderOptions,
) -> RenderResult {
    render_project_widgets_with_appearance(widgets, data, &Value::Null, size, video_time, options)
}

/// Render core project widgets with an explicit project-wide appearance JSON.
/// Existing [`render_project_widgets`] callers retain the `race_dark` default.
pub fn render_project_widgets_with_appearance<D: ProjectValueResolver>(
    widgets: &[WidgetConfig],
    data: &D,
    appearance: &Value,
    size: RenderSize,
    video_time: f64,
    options: RenderOptions,
) -> RenderResult {
    let prepared = prepare_project_widgets_with_appearance(widgets, data, appearance);
    render_prepared_project_widgets(&prepared, data, size, video_time, options)
}

impl Widget {
    fn _binding_value_for_render_slot<D: ProjectValueResolver>(
        &self,
        slot: &str,
        data: &D,
        at: f64,
    ) -> Option<f64> {
        let binding_slot = if slot == self.value_slot {
            "value"
        } else if slot == self.x_slot {
            "x"
        } else if slot == self.y_slot {
            "y"
        } else if slot == self.latitude_slot {
            "latitude"
        } else if slot == self.longitude_slot {
            "longitude"
        } else {
            return None;
        };
        self.bindings()
            .iter()
            .find(|b| b.slot == binding_slot)
            .and_then(|b| data.resolve_binding_smoothed(b, at))
    }
    fn bindings(&self) -> &[ChannelBinding] {
        self.core_bindings.as_deref().unwrap_or(&[])
    }
}

/// Resolve a project widget against a semantic palette. This is public for
/// custom project-level renderers; normal callers should use the themed
/// prepare/render helpers so the palette is resolved only once.
pub fn widget_from_core_with_appearance(w: &WidgetConfig, palette: AppearancePalette) -> Widget {
    let mut out = Widget {
        id: w.id.0.to_string(),
        type_id: match w.kind.as_str() {
            "race.speedometer" | "speedometer" => "radial",
            "race.bar" => "bar",
            "race.g_meter" | "g_meter" => "xy_dot",
            "race.tachometer" => "tachometer",
            "race.temperature" => "temperature",
            "race.lap_timer" => "lap_timer",
            "race.delta" => "delta",
            "race.shift_lights" => "shift_lights",
            "race.center_bar" => "center_bar",
            "race.gear" => "gear",
            k => k,
        }
        .into(),
        rect: w.rect,
        core_bindings: Some(w.bindings.clone()),
        value_slot: w
            .bindings
            .iter()
            .find(|b| b.slot == "value")
            .map(|b| b.slot.clone())
            .unwrap_or_else(|| "value".into()),
        x_slot: w
            .bindings
            .iter()
            .find(|b| b.slot == "x")
            .map(|b| b.slot.clone())
            .unwrap_or_else(|| "x".into()),
        y_slot: w
            .bindings
            .iter()
            .find(|b| b.slot == "y")
            .map(|b| b.slot.clone())
            .unwrap_or_else(|| "y".into()),
        latitude_slot: w
            .bindings
            .iter()
            .find(|b| b.slot == "latitude")
            .map(|b| b.slot.clone())
            .unwrap_or_else(|| "latitude".into()),
        longitude_slot: w
            .bindings
            .iter()
            .find(|b| b.slot == "longitude")
            .map(|b| b.slot.clone())
            .unwrap_or_else(|| "longitude".into()),
        ..Default::default()
    };
    let style = &w.style;
    let settings = &w.settings;
    let inherit_appearance = boolean(style, "inherit_appearance")
        .or_else(|| boolean(settings, "inherit_appearance"))
        .unwrap_or(true);
    apply_appearance(&mut out, style, settings, palette, inherit_appearance);
    out.label = string(style, "label")
        .or_else(|| string(settings, "label"))
        .unwrap_or_default();
    out.unit = string(style, "unit")
        .or_else(|| string(settings, "unit"))
        .unwrap_or_default();
    out.format = string(style, "format")
        .or_else(|| string(settings, "format"))
        .unwrap_or_default();
    out.min = number(style, "min").or_else(|| number(settings, "min"));
    out.max = number(style, "max").or_else(|| number(settings, "max"));
    out.track_map = track_map_from_json(style, settings);
    out
}
fn number(v: &Value, key: &str) -> Option<f64> {
    v.get(key).and_then(Value::as_f64)
}
fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}
fn boolean(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

fn style_color(style: &Value, settings: &Value, key: &str, aliases: &[&str]) -> Option<Rgba> {
    color(style, key)
        .or_else(|| aliases.iter().find_map(|alias| color(style, alias)))
        .or_else(|| color(settings, key))
        .or_else(|| aliases.iter().find_map(|alias| color(settings, alias)))
}

fn style_number(style: &Value, settings: &Value, key: &str, aliases: &[&str]) -> Option<f64> {
    number(style, key)
        .or_else(|| aliases.iter().find_map(|alias| number(style, alias)))
        .or_else(|| number(settings, key))
        .or_else(|| aliases.iter().find_map(|alias| number(settings, alias)))
}

fn apply_appearance(
    widget: &mut Widget,
    style: &Value,
    settings: &Value,
    palette: AppearancePalette,
    inherit: bool,
) {
    widget.accent = palette.accent;
    widget.text_color = palette.text;
    widget.background = palette.background;
    widget.muted = palette.muted;
    widget.positive = palette.positive;
    widget.warning = palette.warning;
    widget.critical = palette.critical;
    widget.opacity = palette.foreground_opacity;
    widget.background_opacity = palette.background_opacity;
    widget.corner_radius = palette.corner_radius;
    if inherit {
        return;
    }
    widget.accent =
        style_color(style, settings, "accent", &["accent_color"]).unwrap_or(widget.accent);
    widget.text_color =
        style_color(style, settings, "text", &["text_color"]).unwrap_or(widget.text_color);
    widget.background = style_color(
        style,
        settings,
        "background",
        &["background_color", "panel"],
    )
    .unwrap_or(widget.background);
    widget.muted = style_color(style, settings, "muted", &[]).unwrap_or(widget.muted);
    widget.positive = style_color(style, settings, "positive", &[]).unwrap_or(widget.positive);
    widget.warning = style_color(style, settings, "warning", &[]).unwrap_or(widget.warning);
    widget.critical = style_color(style, settings, "critical", &[]).unwrap_or(widget.critical);
    widget.opacity = style_number(style, settings, "foreground_opacity", &["opacity"])
        .unwrap_or(widget.opacity as f64)
        .clamp(0., 1.) as f32;
    widget.background_opacity = style_number(style, settings, "background_opacity", &[])
        .unwrap_or(widget.background_opacity as f64)
        .clamp(0., 1.) as f32;
    widget.corner_radius = style_number(style, settings, "corner_radius", &[])
        .unwrap_or(widget.corner_radius as f64)
        .clamp(0., 0.5) as f32;
}
fn color(v: &Value, key: &str) -> Option<Rgba> {
    let a = v.get(key)?.as_array()?;
    if a.len() < 3 {
        return None;
    }
    Some(Rgba(
        a[0].as_u64()?.min(255) as u8,
        a[1].as_u64()?.min(255) as u8,
        a[2].as_u64()?.min(255) as u8,
        a.get(3).and_then(Value::as_u64).unwrap_or(255).min(255) as u8,
    ))
}

fn track_map_from_json(style: &Value, settings: &Value) -> TrackMap {
    let get_number = |key| number(style, key).or_else(|| number(settings, key));
    let get_color = |key| color(style, key).or_else(|| color(settings, key));
    let mode = string(style, "track_mode")
        .or_else(|| string(settings, "track_mode"))
        .or_else(|| string(style, "mode"))
        .or_else(|| string(settings, "mode"))
        .unwrap_or_else(|| "auto".into())
        .to_ascii_lowercase();
    let start = match (get_number("start_latitude"), get_number("start_longitude")) {
        (Some(latitude), Some(longitude)) => Some((latitude, longitude)),
        _ => None,
    };
    let finish = match (
        get_number("finish_latitude"),
        get_number("finish_longitude"),
    ) {
        (Some(latitude), Some(longitude)) => Some((latitude, longitude)),
        _ => None,
    };
    TrackMap {
        mode: match mode.as_str() {
            "circuit" | "lap" | "laps" => TrackMode::Circuit,
            "point_to_point" | "point-to-point" | "p2p" | "autocross" => TrackMode::PointToPoint,
            _ => TrackMode::Auto,
        },
        lap: get_number("lap")
            .filter(|value| value.is_finite() && *value >= 1. && value.fract() == 0.)
            .map(|value| value as usize),
        start,
        finish,
        line_color: get_color("line_color").or_else(|| get_color("path_color")),
        position_color: get_color("position_color").or_else(|| get_color("current_color")),
        start_color: get_color("start_color"),
        finish_color: get_color("finish_color"),
        line_width: get_number("line_width")
            .filter(|v| *v > 0.)
            .map(|v| v as f32),
        marker_size: get_number("marker_size")
            .filter(|v| *v > 0.)
            .map(|v| v as f32),
        show_markers: style
            .get("show_markers")
            .or_else(|| settings.get("show_markers"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        rotation_degrees: get_number("rotation_degrees")
            .or_else(|| get_number("rotation"))
            .filter(|v| v.is_finite())
            .unwrap_or(0.) as f32,
        padding: get_number("padding").filter(|v| *v >= 0.).map(|v| v as f32),
        points: Vec::new(),
    }
}

fn pixel_rect(r: NormalizedRect, size: RenderSize) -> PixelRect {
    PixelRect::new(
        (r.x * size.width as f32).round() as i32,
        (r.y * size.height as f32).round() as i32,
        (r.width * size.width as f32).round().max(0.) as u32,
        (r.height * size.height as f32).round().max(0.) as u32,
    )
}

fn canonical(id: &str) -> Option<&'static str> {
    match id.trim().to_ascii_lowercase().as_str() {
        "numeric" | "number" | "value" => Some("numeric"),
        "bar" | "progress" => Some("bar"),
        "radial" | "gauge" => Some("radial"),
        "xy_dot" | "xy-dot" | "g_meter" | "g-meter" | "gmeter" => Some("xy_dot"),
        "tachometer" | "tach" | "rpm" | "race.tachometer" => Some("tachometer"),
        "temperature" | "temp" | "race.temperature" => Some("temperature"),
        "lap_timer" | "lap-timer" | "laptimer" | "race.lap_timer" => Some("lap_timer"),
        "delta" | "lap_delta" | "lap-delta" | "race.delta" => Some("delta"),
        "shift_lights" | "shift-lights" | "shiftlights" | "race.shift_lights" => {
            Some("shift_lights")
        }
        "center_bar" | "center-bar" | "centerbar" | "race.center_bar" => Some("center_bar"),
        "gear" | "gear_indicator" | "gear-indicator" | "race.gear" => Some("gear"),
        "track_map" | "track-map" | "gps_track" | "gps-track" | "race.track_map" => {
            Some("track_map")
        }
        _ => None,
    }
}
fn slot_or<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    if s.is_empty() { fallback } else { s }
}
fn paint_obj(color: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color.sk());
    p.anti_alias = true;
    p
}
fn fill_round(pm: &mut Pixmap, r: PixelRect, rad: f32, color: Rgba) {
    let x = r.x as f32;
    let y = r.y as f32;
    let w = r.width as f32;
    let h = r.height as f32;
    let q = rad.min(w / 2.).min(h / 2.);
    let mut b = PathBuilder::new();
    b.move_to(x + q, y);
    b.line_to(x + w - q, y);
    b.quad_to(x + w, y, x + w, y + q);
    b.line_to(x + w, y + h - q);
    b.quad_to(x + w, y + h, x + w - q, y + h);
    b.line_to(x + q, y + h);
    b.quad_to(x, y + h, x, y + h - q);
    b.line_to(x, y + q);
    b.quad_to(x, y, x + q, y);
    b.close();
    if let Some(path) = b.finish() {
        pm.fill_path(
            &path,
            &paint_obj(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}
fn stroke_line(pm: &mut Pixmap, points: &[(f32, f32)], color: Rgba, width: f32) {
    if points.len() < 2 {
        return;
    }
    let mut b = PathBuilder::new();
    b.move_to(points[0].0, points[0].1);
    for p in &points[1..] {
        b.line_to(p.0, p.1)
    }
    if let Some(path) = b.finish() {
        let s = Stroke {
            width,
            ..Default::default()
        };
        pm.stroke_path(&path, &paint_obj(color), &s, Transform::identity(), None);
    }
}

/// Synchronize irregular GPS channels onto their combined timestamp grid.
/// GPS loggers often emit latitude and longitude in alternating packets; using
/// an intersection would throw away nearly all of those positions.
fn synchronized_geo_points(latitude: Vec<(f64, f64)>, longitude: Vec<(f64, f64)>) -> Vec<GeoPoint> {
    let mut latitude: Vec<_> = latitude
        .into_iter()
        .filter(|(time, value)| time.is_finite() && value.is_finite())
        .collect();
    let mut longitude: Vec<_> = longitude
        .into_iter()
        .filter(|(time, value)| time.is_finite() && value.is_finite())
        .collect();
    latitude.sort_by(|a, b| a.0.total_cmp(&b.0));
    longitude.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Interpolate longitude only after unwrapping it. Otherwise a perfectly
    // ordinary 179.999 -> -179.999 antimeridian crossing is interpolated
    // through Greenwich and turns a kart-sized course into a world-spanning
    // line.
    unwrap_sample_longitudes(&mut longitude);
    let latitude_gap = interpolation_gap_limit(&latitude);
    let longitude_gap = interpolation_gap_limit(&longitude);
    let mut times: Vec<f64> = latitude
        .iter()
        .chain(&longitude)
        .map(|point| point.0)
        .collect();
    times.sort_by(f64::total_cmp);
    times.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut out = Vec::with_capacity(times.len());
    for time in times {
        let (Some(lat), Some(lon)) = (
            interpolate_samples(&latitude, time, latitude_gap),
            interpolate_samples(&longitude, time, longitude_gap),
        ) else {
            continue;
        };
        if (-90.0..=90.0).contains(&lat) && lon.is_finite() {
            out.push(GeoPoint {
                time,
                latitude: lat,
                longitude: lon,
            });
        }
    }
    unwrap_longitudes(&mut out);
    out
}

fn unwrap_sample_longitudes(samples: &mut [(f64, f64)]) {
    let mut previous = None;
    for (_, longitude) in samples {
        *longitude = (*longitude + 180.0).rem_euclid(360.0) - 180.0;
        if let Some(previous) = previous {
            while *longitude - previous > 180.0 {
                *longitude -= 360.0;
            }
            while *longitude - previous < -180.0 {
                *longitude += 360.0;
            }
        }
        previous = Some(*longitude);
    }
}

fn interpolation_gap_limit(samples: &[(f64, f64)]) -> f64 {
    let mut intervals: Vec<_> = samples
        .windows(2)
        .filter_map(|pair| {
            let interval = pair[1].0 - pair[0].0;
            (interval.is_finite() && interval > 0.0).then_some(interval)
        })
        .collect();
    if intervals.is_empty() {
        return 0.5;
    }
    intervals.sort_by(f64::total_cmp);
    (intervals[intervals.len() / 2] * 5.0).max(0.5)
}

fn interpolate_samples(samples: &[(f64, f64)], time: f64, max_gap: f64) -> Option<f64> {
    match samples.binary_search_by(|point| point.0.total_cmp(&time)) {
        Ok(index) => Some(samples[index].1),
        Err(0) => None,
        Err(index) if index >= samples.len() => None,
        Err(index) => {
            let (a, b) = (samples[index - 1], samples[index]);
            if b.0 - a.0 > max_gap {
                return None;
            }
            let fraction = (time - a.0) / (b.0 - a.0);
            Some(a.1 + (b.1 - a.1) * fraction)
        }
    }
}

fn unwrap_longitudes(points: &mut [GeoPoint]) {
    for index in 1..points.len() {
        let previous = points[index - 1].longitude;
        let mut longitude = points[index].longitude;
        while longitude - previous > 180.0 {
            longitude -= 360.0;
        }
        while longitude - previous < -180.0 {
            longitude += 360.0;
        }
        points[index].longitude = longitude;
    }
}

fn build_track_points(
    latitude: Option<Vec<(f64, f64)>>,
    longitude: Option<Vec<(f64, f64)>>,
    settings: &TrackMap,
) -> Vec<GeoPoint> {
    build_track_points_with_laps(latitude, longitude, settings, &[])
}

fn build_track_points_with_laps(
    latitude: Option<Vec<(f64, f64)>>,
    longitude: Option<Vec<(f64, f64)>>,
    settings: &TrackMap,
    laps: &[SourceLap],
) -> Vec<GeoPoint> {
    let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
        return Vec::new();
    };
    let points = synchronized_geo_points(latitude, longitude);
    if points.len() < 2 {
        return points;
    }
    let selected = match settings.mode {
        // Explicit start/finish always wins, even where a logger happens to
        // contain lap records (for example an autocross test day).
        TrackMode::PointToPoint => select_point_to_point(&points, settings).unwrap_or(points),
        TrackMode::Circuit => full_lap_points(&points, laps, settings.lap)
            .or_else(|| select_circuit_lap(&points))
            .unwrap_or(points),
        TrackMode::Auto => full_lap_points(&points, laps, settings.lap)
            .or_else(|| select_point_to_point(&points, settings))
            .or_else(|| select_circuit_lap(&points))
            .unwrap_or(points),
    };
    downsample_geo_points(selected, 1_200)
}

fn full_lap_points(
    points: &[GeoPoint],
    laps: &[SourceLap],
    selected_lap: Option<usize>,
) -> Option<Vec<GeoPoint>> {
    let full: Vec<_> = laps
        .iter()
        .filter(|lap| {
            lap.lap_type.eq_ignore_ascii_case("full")
                && lap.start_time.is_finite()
                && lap.end_time.is_finite()
                && lap.end_time > lap.start_time
        })
        .collect();
    if full.is_empty() {
        return None;
    }
    let lap = if let Some(selected_lap) = selected_lap {
        *full.get(selected_lap.saturating_sub(1))?
    } else {
        // Median duration avoids an anomalously short/long first full lap.
        let mut durations: Vec<_> = full
            .iter()
            .map(|lap| lap.end_time - lap.start_time)
            .collect();
        durations.sort_by(f64::total_cmp);
        let median = durations[(durations.len() - 1) / 2];
        *full.iter().min_by(|a, b| {
            ((a.end_time - a.start_time) - median)
                .abs()
                .total_cmp(&((b.end_time - b.start_time) - median).abs())
                .then_with(|| a.start_time.total_cmp(&b.start_time))
        })?
    };
    let selected: Vec<_> = points
        .iter()
        .copied()
        .filter(|point| point.time >= lap.start_time && point.time <= lap.end_time)
        .collect();
    (selected.len() >= 2).then_some(selected)
}

/// A representative circuit lap is the first return near the recording's
/// opening position. It deliberately selects one interval rather than drawing
/// a dense pile of every lap. The size-relative threshold works from kart
/// tracks through large road courses.
fn select_circuit_lap(points: &[GeoPoint]) -> Option<Vec<GeoPoint>> {
    if points.len() < 8 {
        return None;
    }
    let projected = project_geo(points);
    let (min_x, max_x, min_y, max_y) = projected.iter().fold(
        (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ),
        |(min_x, max_x, min_y, max_y), point| {
            (
                min_x.min(point.0),
                max_x.max(point.0),
                min_y.min(point.1),
                max_y.max(point.1),
            )
        },
    );
    let diagonal = (max_x - min_x).hypot(max_y - min_y);
    if !diagonal.is_finite() || diagonal < 15.0 {
        return None;
    }
    let threshold = (diagonal * 0.045).clamp(5.0, 40.0);
    let start = projected[0];
    let earliest = (points.len() / 8).max(4);
    let index = (earliest..points.len()).find(|&index| {
        let point = projected[index];
        (point.0 - start.0).hypot(point.1 - start.1) <= threshold
    })?;
    let travelled: f64 = projected[..=index]
        .windows(2)
        .map(|segment| (segment[1].0 - segment[0].0).hypot(segment[1].1 - segment[0].1))
        .sum();
    (travelled > diagonal * 1.3).then(|| points[..=index].to_vec())
}

fn select_point_to_point(points: &[GeoPoint], settings: &TrackMap) -> Option<Vec<GeoPoint>> {
    let (start, finish) = (settings.start?, settings.finish?);
    let start_index = closest_geo_index(points, start)?;
    let subsequent_finish = points
        .iter()
        .enumerate()
        .skip(start_index + 1)
        .min_by(|(_, a), (_, b)| {
            geo_distance_m(**a, finish).total_cmp(&geo_distance_m(**b, finish))
        })
        .map(|(index, point)| (index, geo_distance_m(*point, finish)));
    let nearest_finish = points
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            geo_distance_m(**a, finish).total_cmp(&geo_distance_m(**b, finish))
        })
        .map(|(index, point)| (index, geo_distance_m(*point, finish)))?;

    if let Some((finish_index, subsequent_distance)) = subsequent_finish
        && (nearest_finish.0 >= start_index || subsequent_distance <= nearest_finish.1 + 1.0)
    {
        return Some(points[start_index..=finish_index].to_vec());
    }

    // A user may capture the finish first while scrubbing. The geometry is
    // still well-defined; reverse that unique segment so its first/last
    // markers continue to mean start/finish.
    let finish_index = nearest_finish.0;
    if finish_index >= start_index {
        return None;
    }
    let mut selected = points[finish_index..=start_index].to_vec();
    selected.reverse();
    Some(selected)
}

fn closest_geo_index(points: &[GeoPoint], target: (f64, f64)) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            geo_distance_m(**a, target).total_cmp(&geo_distance_m(**b, target))
        })
        .map(|(index, _)| index)
}

fn geo_distance_m(point: GeoPoint, target: (f64, f64)) -> f64 {
    let latitude = (point.latitude + target.0) * 0.5_f64.to_radians();
    let longitude_delta = (point.longitude - target.1 + 180.0).rem_euclid(360.0) - 180.0;
    let x = longitude_delta.to_radians() * latitude.cos();
    let y = (point.latitude - target.0).to_radians();
    6_371_000.0 * x.hypot(y)
}

fn downsample_geo_points(points: Vec<GeoPoint>, limit: usize) -> Vec<GeoPoint> {
    if points.len() <= limit {
        return points;
    }
    let step = (points.len() - 1) as f64 / (limit - 1) as f64;
    (0..limit)
        .map(|index| points[(index as f64 * step).round() as usize])
        .collect()
}

/// Equirectangular projection around the local mean. Longitude has already
/// been unwrapped, and scaling east-west by cos(latitude) keeps the course
/// shape reliable at any practical motorsport latitude.
fn project_geo(points: &[GeoPoint]) -> Vec<(f64, f64)> {
    let mean_latitude =
        points.iter().map(|point| point.latitude).sum::<f64>() / points.len() as f64;
    let mean_longitude =
        points.iter().map(|point| point.longitude).sum::<f64>() / points.len() as f64;
    let scale_x = mean_latitude.to_radians().cos() * 6_371_000.0;
    points
        .iter()
        .map(|point| {
            (
                (point.longitude - mean_longitude).to_radians() * scale_x,
                (point.latitude - mean_latitude).to_radians() * 6_371_000.0,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn paint_track_map(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    points: &[GeoPoint],
    current_latitude: Option<f64>,
    current_longitude: Option<f64>,
    accent: Rgba,
    text: Rgba,
    positive: Rgba,
    critical: Rgba,
    opacity: f32,
) {
    if points.len() < 2 {
        return;
    }
    let map = &w.track_map;
    let local = project_geo(points);
    let rotation = map.rotation_degrees.to_radians() as f64;
    let (sin, cos) = rotation.sin_cos();
    let rotated: Vec<_> = local
        .iter()
        .map(|point| (point.0 * cos - point.1 * sin, point.0 * sin + point.1 * cos))
        .collect();
    let (min_x, max_x, min_y, max_y) = rotated.iter().fold(
        (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ),
        |(min_x, max_x, min_y, max_y), point| {
            (
                min_x.min(point.0),
                max_x.max(point.0),
                min_y.min(point.1),
                max_y.max(point.1),
            )
        },
    );
    let pad = map.padding.unwrap_or(0.10);
    let pad = if pad <= 1. {
        pad * r.width.min(r.height) as f32
    } else {
        pad
    };
    let available_w = (r.width as f32 - pad * 2.).max(1.) as f64;
    let available_h = (r.height as f32 - pad * 2.).max(1.) as f64;
    let scale =
        (available_w / (max_x - min_x).max(1.0)).min(available_h / (max_y - min_y).max(1.0));
    let center_x = r.x as f64 + r.width as f64 * 0.5;
    let center_y = r.y as f64 + r.height as f64 * 0.5;
    let mid_x = (min_x + max_x) * 0.5;
    let mid_y = (min_y + max_y) * 0.5;
    let to_pixel = |point: (f64, f64)| {
        (
            (center_x + (point.0 - mid_x) * scale) as f32,
            (center_y - (point.1 - mid_y) * scale) as f32,
        )
    };
    let pixels: Vec<_> = rotated.into_iter().map(to_pixel).collect();
    let line_color = map
        .line_color
        .map(|color| color.with_alpha(opacity))
        .unwrap_or(accent);
    let line_width = map
        .line_width
        .unwrap_or((r.width.min(r.height) as f32 * 0.025).clamp(1.25, 6.0));
    // Keep telemetry dropouts visible as gaps instead of inventing a long
    // straight section between the samples on either side.
    let gap_limit = interpolation_gap_limit(
        &points
            .iter()
            .map(|point| (point.time, 0.0))
            .collect::<Vec<_>>(),
    );
    let mut segment_start = 0;
    for index in 1..points.len() {
        if points[index].time - points[index - 1].time > gap_limit {
            stroke_line(pm, &pixels[segment_start..index], line_color, line_width);
            segment_start = index;
        }
    }
    stroke_line(pm, &pixels[segment_start..], line_color, line_width);
    let marker = map
        .marker_size
        .unwrap_or((r.width.min(r.height) as f32 * 0.075).clamp(3.0, 12.0));
    let draw_marker = |pm: &mut Pixmap, point: (f32, f32), color: Rgba, size: f32| {
        fill_round(
            pm,
            PixelRect::new(
                (point.0 - size) as i32,
                (point.1 - size) as i32,
                (size * 2.) as u32,
                (size * 2.) as u32,
            ),
            size,
            color,
        );
    };
    if map.show_markers {
        draw_marker(
            pm,
            pixels[0],
            map.start_color
                .map(|color| color.with_alpha(opacity))
                .unwrap_or(positive),
            marker * 0.65,
        );
        draw_marker(
            pm,
            *pixels.last().unwrap(),
            map.finish_color
                .map(|color| color.with_alpha(opacity))
                .unwrap_or(critical),
            marker * 0.65,
        );
    }
    if let (Some(latitude), Some(longitude)) = (current_latitude, current_longitude)
        && latitude.is_finite()
        && longitude.is_finite()
    {
        let mean_latitude =
            points.iter().map(|point| point.latitude).sum::<f64>() / points.len() as f64;
        let mean_longitude =
            points.iter().map(|point| point.longitude).sum::<f64>() / points.len() as f64;
        let mut longitude = longitude;
        while longitude - mean_longitude > 180.0 {
            longitude -= 360.0;
        }
        while longitude - mean_longitude < -180.0 {
            longitude += 360.0;
        }
        let point = (
            (longitude - mean_longitude).to_radians()
                * mean_latitude.to_radians().cos()
                * 6_371_000.0,
            (latitude - mean_latitude).to_radians() * 6_371_000.0,
        );
        let point = (point.0 * cos - point.1 * sin, point.0 * sin + point.1 * cos);
        draw_marker(
            pm,
            to_pixel(point),
            map.position_color
                .map(|color| color.with_alpha(opacity))
                .unwrap_or(text),
            marker,
        );
    }
}
fn paint_numeric(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    _accent: Rgba,
    text: Rgba,
) {
    let value = v
        .map(|x| format_value(x, &w.format))
        .unwrap_or_else(|| "-".into());
    let line = join_label(&w.label, &value, &w.unit);
    draw_text(
        pm,
        r.x as f32 + 8.,
        r.y as f32 + r.height as f32 * 0.42,
        &line,
        text,
        (r.height as f32 * 0.18).max(6.),
    );
}
fn paint_bar(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    accent: Rgba,
    text: Rgba,
    muted: Rgba,
) {
    let pad = (r.width.min(r.height) as f32 * 0.14).max(3.);
    let bx = r.x as f32 + pad;
    let by = r.y as f32 + r.height as f32 * 0.58;
    let bw = (r.width as f32 - pad * 2.).max(1.);
    let bh = (r.height as f32 * 0.16).max(2.);
    if let Some(val) = v {
        let t = norm(val, w.min, w.max);
        fill_round(
            pm,
            PixelRect::new(bx as i32, by as i32, bw as u32, bh as u32),
            bh / 2.,
            muted.with_alpha(0.70),
        );
        fill_round(
            pm,
            PixelRect::new(bx as i32, by as i32, (bw * t) as u32, bh as u32),
            bh / 2.,
            accent,
        )
    } else {
        fill_round(
            pm,
            PixelRect::new(bx as i32, by as i32, bw as u32, bh as u32),
            bh / 2.,
            muted.with_alpha(0.35),
        )
    }
    let label = join_label(
        &w.label,
        &v.map(|x| format_value(x, &w.format))
            .unwrap_or_else(|| "-".into()),
        &w.unit,
    );
    draw_text(
        pm,
        bx,
        r.y as f32 + pad * 1.2,
        &label,
        text,
        (r.height as f32 * 0.14).max(5.),
    );
}
fn paint_radial(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    accent: Rgba,
    text: Rgba,
    muted: Rgba,
) {
    let cx = r.x as f32 + r.width as f32 * 0.5;
    let cy = r.y as f32 + r.height as f32 * 0.56;
    let rad = (r.width.min(r.height) as f32 * 0.37).max(5.);
    let start = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let mut pts = Vec::with_capacity(65);
    for i in 0..=64 {
        let a = start + sweep * i as f32 / 64.;
        pts.push((cx + rad * a.cos(), cy + rad * a.sin()))
    }
    stroke_line(pm, &pts, muted.with_alpha(0.70), (rad * 0.085).max(1.));
    if let Some(value) = v {
        let progress = norm(value, w.min, w.max);
        let n = (progress * 64.).round() as usize;
        stroke_line(pm, &pts[..=n.min(64)], accent, (rad * 0.085).max(1.));
        let angle = start + sweep * progress;
        stroke_line(
            pm,
            &[
                (cx, cy),
                (cx + rad * 0.72 * angle.cos(), cy + rad * 0.72 * angle.sin()),
            ],
            accent,
            (rad * 0.035).max(1.5),
        );
        let hub = (rad * 0.065).max(2.);
        fill_round(
            pm,
            PixelRect::new(
                (cx - hub) as i32,
                (cy - hub) as i32,
                (hub * 2.) as u32,
                (hub * 2.) as u32,
            ),
            hub,
            accent,
        );
    }
    for i in 0..=8 {
        let angle = start + sweep * i as f32 / 8.;
        let inner = if i % 2 == 0 { rad * 0.83 } else { rad * 0.88 };
        let outer = rad * 1.04;
        stroke_line(
            pm,
            &[
                (cx + inner * angle.cos(), cy + inner * angle.sin()),
                (cx + outer * angle.cos(), cy + outer * angle.sin()),
            ],
            muted.with_alpha(0.72),
            (rad * 0.018).max(1.),
        );
    }
    let value = v
        .map(|x| format_value(x, &w.format))
        .unwrap_or_else(|| "-".into());
    draw_centered_text(
        pm,
        cx,
        cy - rad * 0.18,
        &value,
        text,
        (r.height as f32 * 0.20).clamp(8., 58.),
    );
    if !w.unit.is_empty() {
        draw_centered_text(
            pm,
            cx,
            cy + rad * 0.27,
            &w.unit,
            text.with_alpha(0.72),
            (r.height as f32 * 0.075).clamp(5., 14.),
        );
    }
    let label = if w.label.is_empty() {
        "VALUE"
    } else {
        &w.label
    };
    draw_centered_text(
        pm,
        cx,
        r.y as f32 + r.height as f32 * 0.08,
        label,
        text.with_alpha(0.72),
        (r.height as f32 * 0.08).clamp(5., 15.),
    );
}
#[allow(clippy::too_many_arguments)]
fn paint_xy(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    x: Option<f64>,
    y: Option<f64>,
    accent: Rgba,
    text: Rgba,
    muted: Rgba,
) {
    let cx = r.x as f32 + r.width as f32 * 0.5;
    let cy = r.y as f32 + r.height as f32 * 0.55;
    let rad = (r.width.min(r.height) as f32 * 0.33).max(2.);
    let grid = muted.with_alpha(0.57);
    for scale in [0.5_f32, 1.] {
        let mut ring = Vec::with_capacity(49);
        for i in 0..=48 {
            let angle = std::f32::consts::TAU * i as f32 / 48.;
            ring.push((
                cx + rad * scale * angle.cos(),
                cy + rad * scale * angle.sin(),
            ));
        }
        stroke_line(
            pm,
            &ring,
            grid.with_alpha(if scale < 1. { 0.55 } else { 1. }),
            1.,
        );
    }
    stroke_line(pm, &[(cx - rad, cy), (cx + rad, cy)], grid, 1.);
    stroke_line(pm, &[(cx, cy - rad), (cx, cy + rad)], grid, 1.);
    if let (Some(x), Some(y)) = (x, y) {
        let span = w
            .min
            .map(f64::abs)
            .unwrap_or(1.)
            .max(w.max.map(f64::abs).unwrap_or(1.))
            .max(0.001);
        let px = cx + (x / span).clamp(-1., 1.) as f32 * rad;
        let py = cy - (y / span).clamp(-1., 1.) as f32 * rad;
        let d = (r.width.min(r.height) as f32 * 0.07).max(2.);
        fill_round(
            pm,
            PixelRect::new(
                (px - d) as i32,
                (py - d) as i32,
                (d * 2.) as u32,
                (d * 2.) as u32,
            ),
            d,
            accent,
        )
    } else {
        let d = (r.width.min(r.height) as f32 * 0.05).max(2.);
        fill_round(
            pm,
            PixelRect::new(
                (cx - d) as i32,
                (cy - d) as i32,
                (d * 2.) as u32,
                (d * 2.) as u32,
            ),
            d,
            muted.with_alpha(0.35),
        )
    }
    if !w.label.is_empty() {
        draw_text(
            pm,
            r.x as f32 + 4.,
            r.y as f32 + 4.,
            &w.label,
            text,
            (r.height as f32 * 0.12).max(5.),
        );
    }
}

fn paint_gear(pm: &mut Pixmap, w: &Widget, r: PixelRect, v: Option<f64>, accent: Rgba, text: Rgba) {
    let cx = r.x as f32 + r.width as f32 * 0.5;
    let label = if w.label.is_empty() { "GEAR" } else { &w.label };
    draw_centered_text(
        pm,
        cx,
        r.y as f32 + r.height as f32 * 0.09,
        label,
        text.with_alpha(0.72),
        (r.height as f32 * 0.10).clamp(5., 14.),
    );
    let value = match v {
        Some(value) if value.round() <= 0. => "N".into(),
        Some(value) => format!("{:.0}", value),
        None => "-".into(),
    };
    draw_centered_text(
        pm,
        cx,
        r.y as f32 + r.height as f32 * 0.28,
        &value,
        accent,
        (r.height as f32 * 0.53).clamp(12., 72.),
    );
    let line_width = (r.width as f32 * 0.42).max(4.);
    stroke_line(
        pm,
        &[
            (cx - line_width * 0.5, r.y as f32 + r.height as f32 * 0.88),
            (cx + line_width * 0.5, r.y as f32 + r.height as f32 * 0.88),
        ],
        accent.with_alpha(0.75),
        (r.height as f32 * 0.025).max(1.),
    );
}

/// A broad, half-round RPM dial. The last 15% of the scale is deliberately
/// reserved for the shift zone so it remains useful on both a small HUD and a
/// full-width 4K overlay.
#[allow(clippy::too_many_arguments)]
fn paint_tachometer(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    accent: Rgba,
    text: Rgba,
    muted: Rgba,
    warning: Rgba,
    critical: Rgba,
) {
    let cx = r.x as f32 + r.width as f32 * 0.5;
    let cy = r.y as f32 + r.height as f32 * 0.57;
    let radius = (r.width.min(r.height) as f32 * 0.42).max(5.);
    // A 270-degree sweep leaves the lower-center opening clear for the
    // caption and keeps the complete dial inside its widget rectangle.
    let start = std::f32::consts::PI * 0.75;
    let sweep = std::f32::consts::PI * 1.5;
    let lo = w.min.unwrap_or(0.);
    let hi = w.max.unwrap_or(15_000.).max(lo + 1.);
    let value_t = v.map(|value| ((value - lo) / (hi - lo)).clamp(0., 1.) as f32);
    let base = muted.with_alpha(0.67);

    let mut arc = Vec::with_capacity(73);
    for i in 0..=72 {
        let angle = start + sweep * i as f32 / 72.;
        arc.push((cx + radius * angle.cos(), cy + radius * angle.sin()));
    }
    stroke_line(pm, &arc, base, (radius * 0.085).max(1.));
    let shift_start = (72. * 0.85) as usize;
    stroke_line(
        pm,
        &arc[shift_start..],
        critical.with_alpha(0.92),
        (radius * 0.085).max(1.),
    );
    for i in 0..=8 {
        let t = i as f32 / 8.;
        let angle = start + sweep * t;
        let outer = radius * 1.05;
        let inner = radius * if i % 2 == 0 { 0.84 } else { 0.89 };
        let tick = if t >= 0.85 {
            warning.with_alpha(0.94)
        } else {
            muted.with_alpha(0.75)
        };
        stroke_line(
            pm,
            &[
                (cx + inner * angle.cos(), cy + inner * angle.sin()),
                (cx + outer * angle.cos(), cy + outer * angle.sin()),
            ],
            tick,
            (radius * 0.035).max(1.),
        );
    }
    if let Some(t) = value_t {
        let angle = start + sweep * t;
        let tip = radius * 0.79;
        stroke_line(
            pm,
            &[(cx, cy), (cx + tip * angle.cos(), cy + tip * angle.sin())],
            accent,
            (radius * 0.045).max(1.5),
        );
        fill_round(
            pm,
            PixelRect::new(
                (cx - radius * 0.08) as i32,
                (cy - radius * 0.08) as i32,
                (radius * 0.16) as u32,
                (radius * 0.16) as u32,
            ),
            radius * 0.08,
            accent,
        );
    }
    let value = v
        .map(|value| format_value(value, &w.format))
        .unwrap_or_else(|| "-".into());
    let value_size = (r.height as f32 * 0.18).clamp(7., 34.);
    draw_centered_text(pm, cx, cy - radius * 0.22, &value, text, value_size);
    let caption = if w.label.is_empty() { "RPM" } else { &w.label };
    draw_centered_text(
        pm,
        cx,
        r.y as f32 + r.height as f32 * 0.84,
        join_label(caption, "", &w.unit).trim(),
        muted.with_alpha(0.82),
        (r.height as f32 * 0.09).clamp(5., 15.),
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_temperature(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    accent: Rgba,
    text: Rgba,
    muted: Rgba,
    warning: Rgba,
    critical: Rgba,
) {
    let lo = w.min.unwrap_or(40.);
    let hi = w.max.unwrap_or(120.).max(lo + 1.);
    let t = v.map(|value| ((value - lo) / (hi - lo)).clamp(0., 1.) as f32);
    let pad = (r.width.min(r.height) as f32 * 0.12).max(3.);
    let compact = r.width as f32 > r.height as f32 * 1.45;
    let track = muted.with_alpha(0.65);
    let hot = critical.with_alpha(0.96);
    let warm = warning.with_alpha(0.96);
    let value = v
        .map(|value| format_value(value, &w.format))
        .unwrap_or_else(|| "-".into());
    let label = if w.label.is_empty() { "TEMP" } else { &w.label };

    if compact {
        let x = r.x as f32 + pad;
        let y = r.y as f32 + r.height as f32 * 0.62;
        let width = (r.width as f32 - pad * 2.).max(2.);
        let height = (r.height as f32 * 0.16).max(3.);
        fill_round(
            pm,
            PixelRect::new(x as i32, y as i32, width as u32, height as u32),
            height / 2.,
            track,
        );
        if let Some(level) = t {
            let color = if level > 0.86 {
                hot
            } else if level > 0.70 {
                warm
            } else {
                accent
            };
            fill_round(
                pm,
                PixelRect::new(x as i32, y as i32, (width * level) as u32, height as u32),
                height / 2.,
                color,
            );
        }
        draw_text(
            pm,
            x,
            r.y as f32 + pad,
            label,
            text,
            (r.height as f32 * 0.14).clamp(5., 16.),
        );
        let with_unit = join_label("", &value, &w.unit);
        draw_text(
            pm,
            r.x as f32 + r.width as f32 * 0.52,
            r.y as f32 + pad,
            &with_unit,
            text,
            (r.height as f32 * 0.16).clamp(6., 20.),
        );
    } else {
        let x = r.x as f32 + r.width as f32 * 0.24;
        let y = r.y as f32 + pad;
        let width = (r.width as f32 * 0.22).max(5.);
        let height = (r.height as f32 - pad * 2.).max(4.);
        fill_round(
            pm,
            PixelRect::new(x as i32, y as i32, width as u32, height as u32),
            width / 2.,
            track,
        );
        if let Some(level) = t {
            let filled = height * level;
            let color = if level > 0.86 {
                hot
            } else if level > 0.70 {
                warm
            } else {
                accent
            };
            fill_round(
                pm,
                PixelRect::new(
                    x as i32,
                    (y + height - filled) as i32,
                    width as u32,
                    filled as u32,
                ),
                width / 2.,
                color,
            );
        }
        for i in 1..4 {
            let yy = y + height * i as f32 / 4.;
            stroke_line(
                pm,
                &[(x + width * 1.15, yy), (x + width * 1.38, yy)],
                track,
                1.,
            );
        }
        draw_text(
            pm,
            x + width * 1.7,
            y + height * 0.28,
            label,
            text,
            (r.height as f32 * 0.11).clamp(5., 14.),
        );
        draw_text(
            pm,
            x + width * 1.7,
            y + height * 0.48,
            &value,
            text,
            (r.height as f32 * 0.17).clamp(7., 24.),
        );
        if !w.unit.is_empty() {
            draw_text(
                pm,
                x + width * 1.7,
                y + height * 0.69,
                &w.unit,
                muted.with_alpha(0.82),
                (r.height as f32 * 0.09).clamp(5., 12.),
            );
        }
    }
}

fn paint_lap_timer(pm: &mut Pixmap, w: &Widget, r: PixelRect, v: Option<f64>, text: Rgba) {
    let cx = r.x as f32 + r.width as f32 * 0.5;
    let label = if w.label.is_empty() { "LAP" } else { &w.label };
    draw_centered_text(
        pm,
        cx,
        r.y as f32 + r.height as f32 * 0.16,
        label,
        text.with_alpha(0.72),
        (r.height as f32 * 0.12).clamp(5., 16.),
    );
    let value = v.map(format_lap_time).unwrap_or_else(|| "-:--.---".into());
    draw_centered_text(
        pm,
        cx,
        r.y as f32 + r.height as f32 * 0.42,
        &value,
        text,
        (r.height as f32 * 0.30).clamp(8., 40.),
    );
    if !w.unit.is_empty() {
        draw_centered_text(
            pm,
            cx,
            r.y as f32 + r.height as f32 * 0.77,
            &w.unit,
            text.with_alpha(0.65),
            (r.height as f32 * 0.09).clamp(5., 12.),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_delta(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    text: Rgba,
    muted: Rgba,
    positive: Rgba,
    critical: Rgba,
) {
    let cx = r.x as f32 + r.width as f32 * 0.5;
    let label = if w.label.is_empty() {
        "DELTA"
    } else {
        &w.label
    };
    draw_centered_text(
        pm,
        cx,
        r.y as f32 + r.height as f32 * 0.16,
        label,
        text.with_alpha(0.72),
        (r.height as f32 * 0.12).clamp(5., 16.),
    );
    let (sign, body, color) = match v {
        Some(value) if value < 0. => ('-', format!("{:.3}", value.abs()), positive),
        Some(value) => ('+', format!("{:.3}", value), critical),
        None => (' ', "-.---".into(), muted.with_alpha(0.70)),
    };
    let size = (r.height as f32 * 0.29).clamp(8., 38.);
    let body_width = text_width(&body, size);
    let sign_width = size * 0.75;
    let left = cx - (body_width + sign_width) * 0.5;
    if sign == '+' {
        let mid_x = left + size * 0.30;
        let mid_y = r.y as f32 + r.height as f32 * 0.55 + size * 0.42;
        stroke_line(
            pm,
            &[(mid_x - size * 0.20, mid_y), (mid_x + size * 0.20, mid_y)],
            color,
            (size * 0.12).max(1.),
        );
        stroke_line(
            pm,
            &[(mid_x, mid_y - size * 0.20), (mid_x, mid_y + size * 0.20)],
            color,
            (size * 0.12).max(1.),
        );
    } else if sign == '-' {
        draw_text(
            pm,
            left,
            r.y as f32 + r.height as f32 * 0.55,
            "-",
            color,
            size,
        );
    }
    draw_text(
        pm,
        left + sign_width,
        r.y as f32 + r.height as f32 * 0.55,
        &body,
        color,
        size,
    );
    if !w.unit.is_empty() {
        draw_centered_text(
            pm,
            cx,
            r.y as f32 + r.height as f32 * 0.80,
            &w.unit,
            text.with_alpha(0.65),
            (r.height as f32 * 0.09).clamp(5., 12.),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_shift_lights(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    accent: Rgba,
    text: Rgba,
    _muted: Rgba,
    warning: Rgba,
    critical: Rgba,
) {
    const LIGHTS: usize = 10;
    let lo = w.min.unwrap_or(6_000.);
    let hi = w.max.unwrap_or(12_000.).max(lo + 1.);
    let progress = v
        .map(|value| ((value - lo) / (hi - lo)).clamp(0., 1.) as f32)
        .unwrap_or(0.);
    let pad = (r.height as f32 * 0.16).max(2.);
    let gap = (r.width as f32 * 0.018).max(2.);
    let available = (r.width as f32 - pad * 2. - gap * (LIGHTS - 1) as f32).max(1.);
    let width = available / LIGHTS as f32;
    let height = (r.height as f32 * 0.35).max(4.);
    let y = r.y as f32 + r.height as f32 * 0.38;
    let lit = (progress * LIGHTS as f32).ceil() as usize;
    for i in 0..LIGHTS {
        let base = if i >= 8 {
            critical
        } else if i >= 6 {
            warning
        } else {
            accent
        };
        let color = if i < lit { base } else { base.with_alpha(0.18) };
        fill_round(
            pm,
            PixelRect::new(
                (r.x as f32 + pad + i as f32 * (width + gap)) as i32,
                y as i32,
                width as u32,
                height as u32,
            ),
            (height * 0.22).max(1.),
            color,
        );
    }
    let label = if w.label.is_empty() {
        "SHIFT"
    } else {
        &w.label
    };
    draw_text(
        pm,
        r.x as f32 + pad,
        r.y as f32 + pad,
        label,
        text.with_alpha(0.75),
        (r.height as f32 * 0.12).clamp(5., 15.),
    );
    let value = v
        .map(|value| format_value(value, &w.format))
        .unwrap_or_else(|| "-".into());
    draw_text(
        pm,
        r.x as f32 + pad,
        r.y as f32 + r.height as f32 * 0.80,
        &join_label("", &value, &w.unit),
        text,
        (r.height as f32 * 0.12).clamp(5., 15.),
    );
}

fn paint_center_bar(
    pm: &mut Pixmap,
    w: &Widget,
    r: PixelRect,
    v: Option<f64>,
    accent: Rgba,
    text: Rgba,
    muted: Rgba,
) {
    let pad = (r.width.min(r.height) as f32 * 0.12).max(3.);
    let x = r.x as f32 + pad;
    let y = r.y as f32 + r.height as f32 * 0.56;
    let width = (r.width as f32 - pad * 2.).max(2.);
    let height = (r.height as f32 * 0.18).max(3.);
    let center = x + width * 0.5;
    let span = w
        .min
        .map(f64::abs)
        .unwrap_or(1.)
        .max(w.max.map(f64::abs).unwrap_or(1.))
        .max(0.001);
    fill_round(
        pm,
        PixelRect::new(x as i32, y as i32, width as u32, height as u32),
        height / 2.,
        muted.with_alpha(0.67),
    );
    stroke_line(
        pm,
        &[(center, y - height * 0.32), (center, y + height * 1.32)],
        muted.with_alpha(0.79),
        1.,
    );
    if let Some(value) = v {
        let endpoint = center + (value / span).clamp(-1., 1.) as f32 * width * 0.5;
        let left = center.min(endpoint);
        let bar_width = (center - endpoint).abs().max(1.);
        let color = if value < 0. {
            accent.with_alpha(0.82)
        } else {
            accent
        };
        fill_round(
            pm,
            PixelRect::new(left as i32, y as i32, bar_width as u32, height as u32),
            height / 2.,
            color,
        );
    }
    let label = if w.label.is_empty() {
        "INPUT"
    } else {
        &w.label
    };
    draw_text(
        pm,
        x,
        r.y as f32 + pad,
        label,
        text.with_alpha(0.75),
        (r.height as f32 * 0.13).clamp(5., 16.),
    );
    let value = v
        .map(|value| format_value(value, &w.format))
        .unwrap_or_else(|| "-".into());
    let value_with_unit = join_label("", &value, &w.unit);
    draw_text(
        pm,
        x,
        r.y as f32 + r.height as f32 * 0.80,
        &value_with_unit,
        text,
        (r.height as f32 * 0.13).clamp(5., 16.),
    );
}

fn format_lap_time(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "-:--.---".into();
    }
    let millis = (seconds.max(0.) * 1_000.).round() as u64;
    let minutes = millis / 60_000;
    let seconds = (millis / 1_000) % 60;
    format!("{minutes}:{seconds:02}.{:03}", millis % 1_000)
}

fn text_width(s: &str, size: f32) -> f32 {
    s.chars().count() as f32 * 6. * (size / 7.).max(0.5)
}

fn draw_centered_text(pm: &mut Pixmap, center_x: f32, y: f32, s: &str, color: Rgba, size: f32) {
    draw_text(pm, center_x - text_width(s, size) * 0.5, y, s, color, size);
}
fn norm(v: f64, min: Option<f64>, max: Option<f64>) -> f32 {
    let lo = min.unwrap_or(0.);
    let hi = max.unwrap_or(100.);
    if hi <= lo {
        return 0.;
    }
    ((v - lo) / (hi - lo)).clamp(0., 1.) as f32
}
fn join_label(label: &str, value: &str, unit: &str) -> String {
    let mut s = String::new();
    if !label.is_empty() {
        s.push_str(label);
        s.push(' ')
    }
    s.push_str(value);
    if !unit.is_empty() {
        s.push(' ');
        s.push_str(unit)
    }
    s
}
fn format_value(v: f64, fmt: &str) -> String {
    if fmt.is_empty() {
        return if v.fract().abs() < 1e-9 {
            format!("{v:0.0}")
        } else {
            format!("{v:0.2}")
        };
    }
    let precision = fmt.find('.').and_then(|i| {
        fmt[i + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<usize>()
            .ok()
    });
    if let Some(p) = precision {
        return format!("{v:.p$}");
    }
    fmt.replace("{}", &format!("{v}"))
}
fn draw_text(pm: &mut Pixmap, x: f32, y: f32, s: &str, color: Rgba, size: f32) {
    let scale = (size / 7.).max(0.5);
    let mut pen = x;
    for ch in s.chars() {
        if ch == '\n' {
            continue;
        }
        let advance = bitmap_font::draw_char(pm, pen, y, size, ch, color);
        pen += advance * scale;
    }
}
fn rgba_image(pm: &Pixmap) -> RgbaImage {
    let mut pixels = Vec::with_capacity((pm.width() * pm.height() * 4) as usize);
    for p in pm.data().as_chunks::<4>().0 {
        let a = p[3];
        if a == 0 {
            pixels.extend_from_slice(&[0, 0, 0, 0])
        } else {
            pixels.extend_from_slice(&[
                ((p[0] as u16 * 255 / a as u16).min(255)) as u8,
                ((p[1] as u16 * 255 / a as u16).min(255)) as u8,
                ((p[2] as u16 * 255 / a as u16).min(255)) as u8,
                a,
            ])
        }
    }
    RgbaImage::new(pm.width(), pm.height(), pixels)
}
fn alpha_bounds(image: &RgbaImage) -> Option<PixelRect> {
    let mut out = None;
    for y in 0..image.height {
        for x in 0..image.width {
            if image.pixels[((y * image.width + x) * 4 + 3) as usize] > 0 {
                let p = PixelRect::new(x as i32, y as i32, 1, 1);
                out = Some(out.map_or(p, |q: PixelRect| q.union(p)))
            }
        }
    }
    out
}
fn crop_image(image: &RgbaImage, bounds: Option<PixelRect>) -> RgbaImage {
    let Some(b) = bounds else {
        return RgbaImage::new(0, 0, Vec::new());
    };
    let mut out = Vec::with_capacity((b.width * b.height * 4) as usize);
    for y in b.y..b.bottom() {
        let start = ((y as u32 * image.width + b.x as u32) * 4) as usize;
        let end = start + (b.width * 4) as usize;
        out.extend_from_slice(&image.pixels[start..end])
    }
    RgbaImage::new(b.width, b.height, out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitTarget {
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}
pub fn hit_test(rect: NormalizedRect, x: f32, y: f32, tolerance: f32) -> Option<HitTarget> {
    let l = (x - rect.x).abs() <= tolerance;
    let rr = (x - (rect.x + rect.width)).abs() <= tolerance;
    let t = (y - rect.y).abs() <= tolerance;
    let b = (y - (rect.y + rect.height)).abs() <= tolerance;
    if l && t {
        Some(HitTarget::TopLeft)
    } else if rr && t {
        Some(HitTarget::TopRight)
    } else if l && b {
        Some(HitTarget::BottomLeft)
    } else if rr && b {
        Some(HitTarget::BottomRight)
    } else if l {
        Some(HitTarget::Left)
    } else if rr {
        Some(HitTarget::Right)
    } else if t {
        Some(HitTarget::Top)
    } else if b {
        Some(HitTarget::Bottom)
    } else if x >= rect.x && y >= rect.y && x <= rect.x + rect.width && y <= rect.y + rect.height {
        Some(HitTarget::Move)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use overlay_core::{
        ChannelDescriptor, ChannelId, ChannelRef, ChannelSeries, Interpolation, Quantity,
        TelemetryChannel, TimedSample, Unit, WidgetId,
    };
    fn w(kind: &str) -> Widget {
        Widget {
            type_id: kind.into(),
            rect: NormalizedRect::new(0.1, 0.1, 0.5, 0.5),
            label: "T".into(),
            min: Some(0.),
            max: Some(100.),
            ..Default::default()
        }
    }
    #[test]
    fn every_builtin_renders() {
        let mut d = DatasetContext::default();
        d.insert("value", [(0., 50.)]);
        d.insert("x", [(0., 0.2)]);
        d.insert("y", [(0., -0.3)]);
        for k in [
            "numeric",
            "bar",
            "radial",
            "xy_dot",
            "tachometer",
            "temperature",
            "lap_timer",
            "delta",
            "shift_lights",
            "center_bar",
            "gear",
        ] {
            let mut x = w(k);
            if k == "xy_dot" {
                x.x_slot = "x".into();
                x.y_slot = "y".into()
            }
            let out = Renderer::default().render_cropped(&[x], RenderSize::new(100, 80), &d, 0.);
            assert!(out.bounds.is_some(), "{k}");
            assert!(!out.image.pixels.is_empty())
        }
    }
    #[test]
    fn telemetry_widget_aliases_are_canonical() {
        let registry = WidgetRegistry::default();
        for kind in [
            "tach",
            "temp",
            "lap-timer",
            "lap_delta",
            "shift-lights",
            "center-bar",
            "race.tachometer",
            "race.gear",
        ] {
            assert!(registry.contains(kind), "{kind}");
        }
        assert_eq!(format_lap_time(61.2346), "1:01.235");
    }
    #[test]
    fn opacity_and_missing_data() {
        let mut x = w("numeric");
        x.opacity = 0.;
        x.background_opacity = 0.;
        let r = Renderer::default().render_cropped(
            &[x],
            RenderSize::new(40, 40),
            &DatasetContext::default(),
            0.,
        );
        assert!(r.bounds.is_none());
        let out = Renderer::default().render_cropped(
            &[w("radial")],
            RenderSize::new(40, 40),
            &DatasetContext::default(),
            0.,
        );
        assert!(out.bounds.is_some())
    }

    #[test]
    fn background_opacity_is_independent_from_foreground() {
        let d = DatasetContext::default().with_samples("value", [(0., 42.)]);
        let mut foreground_only = w("numeric");
        foreground_only.background_opacity = 0.;
        let visible = Renderer::default().render_cropped(
            &[foreground_only],
            RenderSize::new(100, 80),
            &d,
            0.,
        );
        assert!(visible.bounds.is_some());

        let mut background_only = w("numeric");
        background_only.opacity = 0.;
        background_only.background_opacity = 1.;
        let visible = Renderer::default().render_cropped(
            &[background_only],
            RenderSize::new(100, 80),
            &d,
            0.,
        );
        assert!(visible.bounds.is_some());
    }

    #[test]
    fn widget_overrides_require_opt_out_of_global_appearance() {
        let mut config = WidgetConfig {
            id: WidgetId::new(),
            kind: "numeric".into(),
            rect: NormalizedRect::default(),
            bindings: Vec::new(),
            style: serde_json::json!({"opacity": 0.4}),
            settings: serde_json::json!({}),
            unknown: Default::default(),
        };
        let palette = AppearancePalette::light();
        let widget = widget_from_core_with_appearance(&config, palette);
        assert_eq!(widget.opacity, palette.foreground_opacity);
        assert_eq!(widget.background_opacity, palette.background_opacity);

        config.style["inherit_appearance"] = serde_json::json!(false);
        config.style["background_opacity"] = serde_json::json!(0.8);
        let widget = widget_from_core_with_appearance(&config, palette);
        assert!((widget.opacity - 0.4).abs() < f32::EPSILON);
        assert!((widget.background_opacity - 0.8).abs() < f32::EPSILON);
    }

    #[test]
    fn appearance_presets_and_explicit_palette_values_resolve_predictably() {
        let dark = resolve_appearance(&serde_json::json!({"preset": "race_dark"}));
        assert_eq!(dark, AppearancePalette::race_dark());

        let transparent = resolve_appearance(&serde_json::json!({"preset": "transparent"}));
        assert_eq!(
            transparent.background,
            AppearancePalette::race_dark().background
        );
        assert_eq!(transparent.background_opacity, 0.);

        let custom = resolve_appearance(&serde_json::json!({
            "preset": "light",
            "accent": [1, 2, 3],
            "panel": [4, 5, 6, 7],
            "foreground_opacity": 0.6,
            "background_opacity": 0.4,
            "corner_radius": 0.25,
        }));
        assert_eq!(custom.accent, Rgba(1, 2, 3, 255));
        assert_eq!(custom.background, Rgba(4, 5, 6, 7));
        assert_eq!(custom.foreground_opacity, 0.6);
        assert_eq!(custom.background_opacity, 0.4);
        assert_eq!(custom.corner_radius, 0.25);
    }

    #[test]
    fn project_theme_is_captured_by_prepared_widgets_and_widget_overrides_win() {
        let mut inherited = WidgetConfig {
            id: WidgetId::new(),
            kind: "temperature".into(),
            rect: NormalizedRect::default(),
            bindings: Vec::new(),
            style: serde_json::json!({"accent": [200, 1, 2]}),
            settings: Value::Null,
            unknown: Default::default(),
        };
        let palette_json = serde_json::json!({
            "preset": "race_dark",
            "accent": [3, 4, 5],
            "muted": [6, 7, 8],
            "positive": [9, 10, 11],
            "warning": [12, 13, 14],
            "critical": [15, 16, 17],
        });
        let palette = resolve_appearance(&palette_json);
        let from_global = widget_from_core_with_appearance(&inherited, palette);
        assert_eq!(from_global.accent, palette.accent);
        assert_eq!(from_global.warning, palette.warning);

        inherited.style = serde_json::json!({
            "inherit_appearance": false,
            "accent": [200, 1, 2],
            "warning": [21, 22, 23],
            "foreground_opacity": 0.5,
            "background_opacity": 0.25,
            "corner_radius": 0.05,
        });
        let overridden = widget_from_core_with_appearance(&inherited, palette);
        assert_eq!(overridden.accent, Rgba(200, 1, 2, 255));
        assert_eq!(overridden.warning, Rgba(21, 22, 23, 255));
        assert_eq!(overridden.critical, palette.critical);
        assert_eq!(overridden.opacity, 0.5);
        assert_eq!(overridden.background_opacity, 0.25);
        assert_eq!(overridden.corner_radius, 0.05);

        let datasets: Vec<TelemetryDataset> = Vec::new();
        let prepared = prepare_project_widgets_with_appearance(
            &[inherited],
            &datasets.as_slice(),
            &palette_json,
        );
        assert_eq!(prepared.widgets[0].accent, Rgba(200, 1, 2, 255));
        assert_eq!(prepared.widgets[0].positive, palette.positive);
    }
    #[test]
    fn scale_and_crop() {
        let d = DatasetContext::default().with_samples("value", [(0., 10.)]);
        let x = w("bar");
        let a = Renderer::default().render(
            std::slice::from_ref(&x),
            RenderSize::new(1920, 1080),
            &d,
            0.,
            RenderOptions {
                crop: false,
                full_size: false,
            },
        );
        let b = Renderer::default().render(
            &[x],
            RenderSize::new(3840, 2160),
            &d,
            0.,
            RenderOptions {
                crop: false,
                full_size: false,
            },
        );
        assert_eq!(a.image.width, 1920);
        assert_eq!(b.image.width, 3840);
        assert!(
            b.image.pixels.iter().filter(|&&a| a > 0).count()
                > a.image.pixels.iter().filter(|&&a| a > 0).count()
        )
    }
    #[test]
    fn dataset_interpolates() {
        let d = DatasetContext::default().with_samples("a", [(0., 0.), (1., 10.)]);
        assert_eq!(d.resolve("a", 0.5), Some(5.))
    }

    #[test]
    fn project_widgets_keep_duplicate_value_slots_separate() {
        fn channel(_source: SourceId, id: ChannelId, name: &str, value: f64) -> TelemetryChannel {
            TelemetryChannel {
                descriptor: ChannelDescriptor {
                    id,
                    name: name.into(),
                    quantity: Quantity::Generic,
                    unit: Unit::Unitless,
                    interpolation: Interpolation::Hold,
                    description: None,
                },
                series: ChannelSeries::new(vec![TimedSample { time: 0., value }]),
            }
        }
        let source = SourceId::new();
        let first = ChannelId::new();
        let second = ChannelId::new();
        let mut dataset = TelemetryDataset {
            source_id: source,
            channels: Default::default(),
            ..Default::default()
        };
        dataset.insert(channel(source, first, "first", 10.));
        dataset.insert(channel(source, second, "second", 90.));
        let widgets = [
            WidgetConfig {
                id: WidgetId::new(),
                kind: "numeric".into(),
                rect: NormalizedRect::new(0., 0., 0.45, 0.8),
                bindings: vec![ChannelBinding {
                    slot: "value".into(),
                    channel: ChannelRef {
                        source_id: source,
                        channel_id: first,
                    },
                    scale: 1.,
                    offset: 0.,
                    invert: false,
                    smoothing_seconds: None,
                    low_pass_hz: None,
                    display_unit: None,
                }],
                style: Value::Null,
                settings: Value::Null,
                unknown: Default::default(),
            },
            WidgetConfig {
                id: WidgetId::new(),
                kind: "numeric".into(),
                rect: NormalizedRect::new(0.5, 0., 0.45, 0.8),
                bindings: vec![ChannelBinding {
                    slot: "value".into(),
                    channel: ChannelRef {
                        source_id: source,
                        channel_id: second,
                    },
                    scale: 1.,
                    offset: 0.,
                    invert: false,
                    smoothing_seconds: None,
                    low_pass_hz: None,
                    display_unit: None,
                }],
                style: Value::Null,
                settings: Value::Null,
                unknown: Default::default(),
            },
        ];
        let datasets = [dataset];
        let data = &datasets[..];
        let output = render_project_widgets(
            &widgets,
            &data,
            RenderSize::new(200, 100),
            0.,
            RenderOptions {
                crop: false,
                full_size: false,
            },
        );
        let left: Vec<u8> = output
            .image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 200 < 100)
            .flat_map(|(_, pixel)| pixel.iter().copied())
            .collect();
        let right: Vec<u8> = output
            .image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(i, _)| i % 200 >= 100)
            .flat_map(|(_, pixel)| pixel.iter().copied())
            .collect();
        assert_ne!(left, right);
    }

    #[test]
    fn binding_smoothing_is_optional_and_attenuates_noise() {
        let source = SourceId::new();
        let channel_id = ChannelId::new();
        let dataset = TelemetryDataset {
            source_id: source,
            channels: [(
                channel_id,
                TelemetryChannel {
                    descriptor: ChannelDescriptor {
                        id: channel_id,
                        name: "noise".into(),
                        quantity: Quantity::Generic,
                        unit: Unit::Unitless,
                        interpolation: Interpolation::Linear,
                        description: None,
                    },
                    series: ChannelSeries::new(vec![
                        TimedSample {
                            time: 0.,
                            value: 0.,
                        },
                        TimedSample {
                            time: 0.1,
                            value: 1.,
                        },
                        TimedSample {
                            time: 0.2,
                            value: 0.,
                        },
                    ]),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let mut binding = ChannelBinding::new(
            "value",
            ChannelRef {
                source_id: source,
                channel_id,
            },
        );

        // No smoothing retains the existing interpolated value exactly.
        assert_eq!(dataset.resolve_binding_smoothed(&binding, 0.05), Some(0.5));

        // A short, high-frequency spike is attenuated by the zero-phase filter.
        binding.low_pass_hz = Some(2.0);
        let filtered = dataset
            .resolve_binding_smoothed(&binding, 0.1)
            .expect("sample is in range");
        assert!(filtered.is_finite());
        assert!(filtered > 0. && filtered < 1.);
    }

    #[test]
    fn preview_zero_phase_filter_has_no_lag_and_matches_cached_series() {
        let series = ChannelSeries::new(
            (0..101)
                .map(|index| TimedSample {
                    time: index as f64 * 0.1,
                    value: (index == 50) as u8 as f64,
                })
                .collect(),
        );
        let tau = 0.2;
        let gap = GapPolicy::default();

        // The impulse response is symmetric around the impulse rather than
        // delayed by one pass of the exponential filter.
        let left = zero_phase_sample_at(&series, 4.8, gap, Interpolation::Linear, tau).unwrap();
        let center = zero_phase_sample_at(&series, 5.0, gap, Interpolation::Linear, tau).unwrap();
        let right = zero_phase_sample_at(&series, 5.2, gap, Interpolation::Linear, tau).unwrap();
        assert!((left - right).abs() < 1e-7, "{left} != {right}");
        assert!(center > left);

        // Export's complete-series cache and preview's bounded symmetric
        // window agree away from the window edges.
        let complete = series.low_pass_with_gap(tau, gap).unwrap();
        let expected = complete.sample_at(5.0, gap, Interpolation::Linear).unwrap();
        // The bounded window truncates only the tiny residual tail (well below
        // 1e-7 here); the complete series remains the export reference.
        assert!((center - expected).abs() < 1e-7, "{center} != {expected}");

        let source = SourceId::new();
        let channel_id = ChannelId::new();
        let dataset = TelemetryDataset {
            source_id: source,
            channels: [(
                channel_id,
                TelemetryChannel {
                    descriptor: ChannelDescriptor {
                        id: channel_id,
                        name: "impulse".into(),
                        quantity: Quantity::Generic,
                        unit: Unit::Unitless,
                        interpolation: Interpolation::Linear,
                        description: None,
                    },
                    series: series.clone(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let mut binding = ChannelBinding::new(
            "value",
            ChannelRef {
                source_id: source,
                channel_id,
            },
        );
        binding.smoothing_seconds = Some(tau);
        let datasets = [dataset];
        let preview = AlignedDatasets {
            datasets: &datasets,
            ..Default::default()
        };
        let export = AlignedDatasets {
            datasets: &datasets,
            cache_filtered_series: true,
            ..Default::default()
        };
        let preview_value = preview.resolve_binding_smoothed(&binding, 5.0).unwrap();
        let export_value = export.resolve_binding_smoothed(&binding, 5.0).unwrap();
        assert!((preview_value - export_value).abs() < 1e-7);
    }

    #[test]
    fn widget_hz_cutoff_matches_core_two_pass_filter() {
        let source = SourceId::new();
        let channel_id = ChannelId::new();
        let cutoff = 2.0;
        let series = ChannelSeries::new(
            (0..4_001)
                .map(|index| {
                    let time = index as f64 * 0.01;
                    TimedSample {
                        time,
                        value: (std::f64::consts::TAU * cutoff * time).sin(),
                    }
                })
                .collect(),
        );
        let expected_series = series.low_pass_hz(cutoff).unwrap();
        let dataset = TelemetryDataset {
            source_id: source,
            channels: [(
                channel_id,
                TelemetryChannel {
                    descriptor: ChannelDescriptor {
                        id: channel_id,
                        name: "sine".into(),
                        quantity: Quantity::Generic,
                        unit: Unit::Unitless,
                        interpolation: Interpolation::Linear,
                        description: None,
                    },
                    series,
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let mut binding = ChannelBinding::new(
            "value",
            ChannelRef {
                source_id: source,
                channel_id,
            },
        );
        binding.low_pass_hz = Some(cutoff);
        let datasets = [dataset];
        let resolver = AlignedDatasets {
            datasets: &datasets,
            cache_filtered_series: true,
            ..Default::default()
        };
        let preview = AlignedDatasets {
            datasets: &datasets,
            ..Default::default()
        };
        let time = 20.125;
        let actual = resolver.resolve_binding_smoothed(&binding, time).unwrap();
        let preview_actual = preview.resolve_binding_smoothed(&binding, time).unwrap();
        let expected = expected_series
            .sample_at(time, GapPolicy::default(), Interpolation::Linear)
            .unwrap();
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
        assert!(
            (preview_actual - expected).abs() < 1e-4,
            "{preview_actual} != {expected}"
        );
    }

    #[test]
    fn track_map_uses_one_representative_circuit_lap() {
        let corners = [(0.0, 0.0), (0.0, 0.002), (0.001, 0.002), (0.001, 0.0)];
        let mut points = Vec::new();
        for lap in 0..3 {
            for (index, (latitude, longitude)) in corners.iter().enumerate() {
                points.push(GeoPoint {
                    time: (lap * 4 + index) as f64,
                    latitude: *latitude,
                    longitude: *longitude,
                });
            }
        }
        points.push(GeoPoint {
            time: 12.,
            latitude: 0.,
            longitude: 0.,
        });
        let representative = select_circuit_lap(&points).expect("repeated circuit is detected");
        assert_eq!(representative.len(), 5);
        assert_eq!(representative.first().unwrap().time, 0.);
        assert_eq!(representative.last().unwrap().time, 4.);

        let inferred = build_track_points(
            Some(points.iter().map(|p| (p.time, p.latitude)).collect()),
            Some(points.iter().map(|p| (p.time, p.longitude)).collect()),
            &TrackMap::default(),
        );
        assert_eq!(inferred.first().unwrap().time, 0.);
        assert_eq!(inferred.last().unwrap().time, 4.);

        let full = build_track_points_with_laps(
            Some(points.iter().map(|p| (p.time, p.latitude)).collect()),
            Some(points.iter().map(|p| (p.time, p.longitude)).collect()),
            &TrackMap::default(),
            &[SourceLap {
                number: 2,
                start_time: 4.,
                end_time: 8.,
                lap_type: "full".into(),
            }],
        );
        assert_eq!(full.first().unwrap().time, 4.);
        assert_eq!(full.last().unwrap().time, 8.);
        assert_eq!(full.len(), 5, "out/in laps are excluded");
    }

    #[test]
    fn point_to_point_track_map_crops_to_configured_markers() {
        let points: Vec<_> = (0..=10)
            .map(|index| GeoPoint {
                time: index as f64,
                latitude: 42.0,
                longitude: -71.0 + index as f64 * 0.001,
            })
            .collect();
        let settings = TrackMap {
            mode: TrackMode::PointToPoint,
            start: Some((42.0, -70.997)),
            finish: Some((42.0, -70.993)),
            ..Default::default()
        };
        let cropped = select_point_to_point(&points, &settings).expect("valid traversal");
        assert_eq!(cropped.first().unwrap().time, 3.);
        assert_eq!(cropped.last().unwrap().time, 7.);
        assert_eq!(cropped.len(), 5);
    }

    #[test]
    fn gps_synchronization_interpolates_across_antimeridian_locally() {
        let points = synchronized_geo_points(
            vec![(0.0, 1.0), (0.5, 1.0), (1.0, 1.0)],
            vec![(0.0, 179.0), (1.0, -179.0)],
        );
        let midpoint = points
            .iter()
            .find(|point| (point.time - 0.5).abs() < f64::EPSILON)
            .expect("latitude timestamp should create an interpolated GPS point");
        assert!((midpoint.longitude.abs() - 180.0).abs() < 1e-9);
    }

    #[test]
    fn gps_synchronization_does_not_interpolate_across_dropouts() {
        let points = synchronized_geo_points(
            vec![(0.0, 1.0), (1.0, 1.0), (5.0, 1.0), (10.0, 1.0), (11.0, 1.0)],
            vec![(0.0, 2.0), (1.0, 2.1), (10.0, 3.0), (11.0, 3.1)],
        );
        assert!(points.iter().all(|point| point.time != 5.0));
    }

    #[test]
    fn reversed_point_to_point_capture_keeps_start_then_finish() {
        let points: Vec<_> = (0..=10)
            .map(|index| GeoPoint {
                time: index as f64,
                latitude: 42.0,
                longitude: -71.0 + index as f64 * 0.001,
            })
            .collect();
        let settings = TrackMap {
            mode: TrackMode::PointToPoint,
            start: Some((42.0, -70.993)),
            finish: Some((42.0, -70.997)),
            ..Default::default()
        };
        let cropped = select_point_to_point(&points, &settings).expect("valid reverse traversal");
        assert_eq!(cropped.first().unwrap().time, 7.0);
        assert_eq!(cropped.last().unwrap().time, 3.0);
    }

    #[test]
    fn track_map_marker_visibility_is_read_from_project_style() {
        let config = WidgetConfig {
            id: WidgetId::new(),
            kind: "track_map".into(),
            rect: NormalizedRect::default(),
            bindings: Vec::new(),
            style: serde_json::json!({"show_markers": false}),
            settings: serde_json::json!({}),
            unknown: Default::default(),
        };
        assert!(
            !widget_from_core_with_appearance(&config, AppearancePalette::default())
                .track_map
                .show_markers
        );
    }

    #[test]
    fn explicit_point_to_point_ignores_recorded_laps() {
        let points: Vec<_> = (0..=10)
            .map(|index| GeoPoint {
                time: index as f64,
                latitude: 42.0,
                longitude: -71.0 + index as f64 * 0.001,
            })
            .collect();
        let settings = TrackMap {
            mode: TrackMode::PointToPoint,
            start: Some((42.0, -70.998)),
            finish: Some((42.0, -70.994)),
            ..Default::default()
        };
        let selected = build_track_points_with_laps(
            Some(
                points
                    .iter()
                    .map(|point| (point.time, point.latitude))
                    .collect(),
            ),
            Some(
                points
                    .iter()
                    .map(|point| (point.time, point.longitude))
                    .collect(),
            ),
            &settings,
            &[SourceLap {
                number: 1,
                start_time: 0.,
                end_time: 10.,
                lap_type: "full".into(),
            }],
        );
        assert_eq!(selected.first().unwrap().time, 2.);
        assert_eq!(selected.last().unwrap().time, 6.);
    }

    #[test]
    fn automatic_full_lap_uses_median_duration() {
        let points: Vec<_> = (0..=30)
            .map(|time| GeoPoint {
                time: time as f64,
                latitude: 42.0,
                longitude: -71.0 + time as f64 * 0.0001,
            })
            .collect();
        let laps = [
            SourceLap {
                number: 1,
                start_time: 0.,
                end_time: 4.,
                lap_type: "full".into(),
            },
            SourceLap {
                number: 2,
                start_time: 5.,
                end_time: 15.,
                lap_type: "full".into(),
            },
            SourceLap {
                number: 3,
                start_time: 16.,
                end_time: 20.,
                lap_type: "full".into(),
            },
        ];
        let selected = full_lap_points(&points, &laps, None).unwrap();
        assert_eq!(
            selected.first().unwrap().time,
            0.,
            "lower median is deterministic"
        );
        assert_eq!(selected.last().unwrap().time, 4.);
        let manual = full_lap_points(&points, &laps, Some(2)).unwrap();
        assert_eq!(manual.first().unwrap().time, 5.);
        assert_eq!(manual.last().unwrap().time, 15.);
    }
}
