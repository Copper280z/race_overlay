//! Optional, georeferenced track imagery. Fetching and decoding never run in
//! the UI thread; the GPS trace and comparison do not depend on this service.
use crate::imagery_sources::{self, ImagerySource};
use crate::ui_kit::{Tone, theme, widgets};
use egui::{Color32, Pos2, TextureHandle};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

const RADIUS: f64 = 6_378_137.0;
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const TOAST_SECONDS: f64 = 5.0;
const TOAST_FADE_SECONDS: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GeoBounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl GeoBounds {
    pub fn valid(self) -> bool {
        [self.west, self.south, self.east, self.north]
            .iter()
            .all(|v| v.is_finite())
            && self.west >= -180.0
            && self.east <= 180.0
            && self.south > -85.0
            && self.north < 85.0
            && self.west < self.east
            && self.south < self.north
    }

    fn padded(self) -> Self {
        let x = ((self.east - self.west) * 0.1).max(0.0002);
        let y = ((self.north - self.south) * 0.1).max(0.0002);
        Self {
            west: self.west - x,
            east: self.east + x,
            south: self.south - y,
            north: self.north + y,
        }
    }
}

/// Image fractions use a top-left origin. Registration maps these fractions
/// into Web Mercator meters, not into a stretched GPS reference lap.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ControlPoint {
    pub u: f64,
    pub v: f64,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageryConfig {
    pub image_path: Option<PathBuf>,
    pub bounds: Option<GeoBounds>,
    pub controls: Option<[ControlPoint; 3]>,
    pub opacity: f32,
    pub attribution: String,
    pub source_url: Option<String>,
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

impl Default for ImageryConfig {
    fn default() -> Self {
        Self {
            image_path: None,
            bounds: None,
            controls: None,
            opacity: 0.85,
            attribution: String::new(),
            source_url: None,
            unknown: BTreeMap::new(),
        }
    }
}

/// Where downloaded imagery goes and which sources may provide it.
pub struct ImageryContext<'a> {
    pub asset_dir: &'a Path,
    pub sources: &'a [ImagerySource],
}

#[derive(Clone, Copy, Debug)]
struct Registration {
    x: [f64; 3],
    y: [f64; 3],
}

fn mercator(latitude: f64, longitude: f64) -> [f64; 2] {
    [
        RADIUS * longitude.to_radians(),
        RADIUS
            * (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0)
                .tan()
                .ln(),
    ]
}

fn geographic(x: f64, y: f64) -> [f64; 2] {
    [
        (2.0 * (y / RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees(),
        (x / RADIUS).to_degrees(),
    ]
}

impl Registration {
    fn from_config(config: &ImageryConfig) -> Result<Self, String> {
        if let Some(points) = config.controls {
            return Self::from_controls(points);
        }
        let b = config
            .bounds
            .filter(|b| b.valid())
            .ok_or("Set valid geographic bounds or three control points")?;
        let [west, north] = mercator(b.north, b.west);
        let [east, south] = mercator(b.south, b.east);
        Ok(Self {
            x: [east - west, 0.0, west],
            y: [0.0, south - north, north],
        })
    }

    fn from_controls(points: [ControlPoint; 3]) -> Result<Self, String> {
        if points.iter().any(|p| {
            ![p.u, p.v, p.latitude, p.longitude]
                .iter()
                .all(|v| v.is_finite())
                || p.latitude.abs() >= 85.0
                || p.longitude.abs() > 180.0
                || !(0.0..=1.0).contains(&p.u)
                || !(0.0..=1.0).contains(&p.v)
        }) {
            return Err(
                "Control points must have valid coordinates and image fractions 0–1".into(),
            );
        }
        let [a, b, c] = points;
        let du = b.u - a.u;
        let dv = b.v - a.v;
        let eu = c.u - a.u;
        let ev = c.v - a.v;
        let determinant = du * ev - eu * dv;
        if determinant.abs() < 1e-7 {
            return Err("Image control points must not be collinear".into());
        }
        let pa = mercator(a.latitude, a.longitude);
        let pb = mercator(b.latitude, b.longitude);
        let pc = mercator(c.latitude, c.longitude);
        let solve = |axis: usize| {
            let db = pb[axis] - pa[axis];
            let dc = pc[axis] - pa[axis];
            let u = (db * ev - dc * dv) / determinant;
            let v = (du * dc - eu * db) / determinant;
            [u, v, pa[axis] - u * a.u - v * a.v]
        };
        let result = Self {
            x: solve(0),
            y: solve(1),
        };
        if (result.x[0] * result.y[1] - result.x[1] * result.y[0]).abs() < 0.01 {
            return Err("Geographic control points must span an area".into());
        }
        Ok(result)
    }

    /// Where a position falls in the image, as fractions from its top left.
    fn fraction(self, latitude: f64, longitude: f64) -> [f64; 2] {
        let [x, y] = mercator(latitude, longitude);
        let (dx, dy) = (x - self.x[2], y - self.y[2]);
        let determinant = self.x[0] * self.y[1] - self.x[1] * self.y[0];
        [
            (dx * self.y[1] - dy * self.x[1]) / determinant,
            (self.x[0] * dy - self.y[0] * dx) / determinant,
        ]
    }

    fn geo(self, u: f64, v: f64) -> [f64; 2] {
        geographic(
            self.x[0] * u + self.x[1] * v + self.x[2],
            self.y[0] * u + self.y[1] * v + self.y[2],
        )
    }
}

/// Where an image belongs on the map and whom to credit for it.
#[derive(Clone, Debug)]
struct Placement {
    bounds: GeoBounds,
    controls: Option<[ControlPoint; 3]>,
    source_url: Option<String>,
    name: String,
    attribution: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Origin {
    Downloaded,
    /// "Get aerial image" found it in the cache.
    Cached,
    /// The user chose it from the cache.
    Picked,
}

struct LoadedImage {
    path: PathBuf,
    image: egui::ColorImage,
    /// Absent when reloading the workspace's own image.
    placement: Option<(Placement, Origin)>,
}

/// What a background load reports; a worker may send several.
enum Update {
    Loaded(Box<LoadedImage>),
    /// A passing remark: the image shown stands, but something failed.
    Toast(String),
    Failed(String),
}

impl From<Result<LoadedImage, String>> for Update {
    fn from(result: Result<LoadedImage, String>) -> Self {
        result.map_or_else(Self::Failed, |loaded| Self::Loaded(Box::new(loaded)))
    }
}

#[derive(Default)]
pub struct ImageryController {
    texture: Option<TextureHandle>,
    loaded_path: Option<PathBuf>,
    requested_path: Option<PathBuf>,
    pending: Option<Receiver<Update>>,
    error: Option<String>,
    /// Non-error status, such as "used a cached image".
    notice: Option<String>,
    registration_error: Option<String>,
    editing_bounds: Option<GeoBounds>,
    editing_controls: Option<[ControlPoint; 3]>,
    /// A brief message over the map and when it appeared, in egui time.
    toast: Option<(String, f64)>,
    /// Cached images of the course, read when the picker opens.
    cache_listing: Option<Vec<CachedImage>>,
}

impl ImageryController {
    pub fn poll(&mut self, ctx: &egui::Context, config: &mut ImageryConfig) {
        let now = ctx.input(|i| i.time);
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, shown)| now - shown >= TOAST_SECONDS)
        {
            self.toast = None;
        }
        while let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(update) => self.apply(ctx, config, update),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => self.pending = None,
            }
        }
        if self.pending.is_none() && self.requested_path != config.image_path {
            self.requested_path = config.image_path.clone();
            self.texture = None;
            self.loaded_path = None;
            if let Some(path) = config.image_path.clone() {
                self.spawn(ctx, move || {
                    Update::from(load_image(&path).map(|image| LoadedImage {
                        path,
                        image,
                        placement: None,
                    }))
                });
            }
        }
    }

    /// Runs `work` off the UI thread, replacing any load in progress.
    fn spawn(&mut self, ctx: &egui::Context, work: impl FnOnce() -> Update + Send + 'static) {
        self.spawn_updates(ctx, move |send| send(work()));
    }

    fn spawn_updates(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(&mut dyn FnMut(Update)) + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            work(&mut |update| {
                let _ = tx.send(update);
                repaint.request_repaint();
            })
        });
        self.pending = Some(rx);
    }

    fn apply(&mut self, ctx: &egui::Context, config: &mut ImageryConfig, update: Update) {
        let loaded = match update {
            Update::Loaded(loaded) => *loaded,
            Update::Toast(text) => return self.toast = Some((text, ctx.input(|i| i.time))),
            Update::Failed(error) => return self.error = Some(error),
        };
        if let Some((placement, origin)) = loaded.placement {
            config.image_path = Some(loaded.path.clone());
            config.bounds = Some(placement.bounds);
            config.controls = placement.controls;
            config.attribution = placement.attribution;
            config.source_url = placement.source_url;
            self.editing_bounds = None;
            self.editing_controls = None;
            self.cache_listing = None;
            let name = placement.name;
            self.notice = match origin {
                Origin::Downloaded => Some(format!("Downloaded from {name}.")),
                Origin::Cached => Some(format!("Cached {name} image; nothing downloaded.")),
                Origin::Picked => None,
            };
        }
        if config.image_path.as_ref() == Some(&loaded.path) {
            self.texture = Some(ctx.load_texture(
                "analysis-track-imagery",
                loaded.image,
                egui::TextureOptions::LINEAR,
            ));
            self.loaded_path = Some(loaded.path.clone());
            self.requested_path = Some(loaded.path);
            self.error = None;
        }
    }

    /// Download / import / clear buttons plus load status. Shared by the
    /// inline "add imagery" prompt and the options menu.
    pub fn quick_actions(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut ImageryConfig,
        course_bounds: Option<GeoBounds>,
        imagery: &ImageryContext<'_>,
        show_clear: bool,
    ) {
        self.poll(ui.ctx(), config);
        ui.horizontal_wrapped(|ui| {
            let padded = course_bounds
                .map(GeoBounds::padded)
                .filter(|bounds| bounds.valid());
            let candidates = padded
                .map(|bounds| imagery_sources::covering(imagery.sources, bounds))
                .unwrap_or_default();
            let hover = candidates.first().map_or_else(
                || "No imagery source covers this course; add one in Settings".to_owned(),
                |source| format!("From {}; cached for reuse", source.name),
            );
            let mut request = None;
            if ui
                .add_enabled(
                    !candidates.is_empty() && self.pending.is_none(),
                    egui::Button::new("Get aerial image"),
                )
                .on_hover_text(&hover)
                .on_disabled_hover_text(&hover)
                .clicked()
            {
                request = Some(false);
            }
            if ui
                .button("Find imagery…")
                .on_hover_text("Search ArcGIS Online for imagery of this course")
                .clicked()
            {
                match padded {
                    Some(bounds) => crate::settings_window::request_find(ui.ctx(), bounds),
                    None => crate::settings_window::request_open(ui.ctx()),
                }
            }
            if config.source_url.is_some()
                && ui
                    .add_enabled(
                        !candidates.is_empty() && self.pending.is_none(),
                        egui::Button::new("Re-download"),
                    )
                    .on_hover_text("Ignore the cache")
                    .clicked()
            {
                request = Some(true);
            }
            if let (Some(refresh), Some(course)) = (request, course_bounds)
                && !candidates.is_empty()
            {
                let dir = imagery.asset_dir.to_owned();
                let cache = crate::app_paths::imagery_cache_dir();
                self.spawn_updates(ui.ctx(), move |send| {
                    get_imagery(&candidates, course, &dir, &cache, refresh, send)
                });
                self.error = None;
                self.notice = None;
            }
            if let Some(course) = course_bounds.filter(|bounds| bounds.valid()) {
                let picked = widgets::popover(ui, "Cached…", |ui| {
                    self.cache_menu(ui, config, course, imagery.sources)
                });
                match picked {
                    Some(Some(entry)) => {
                        widgets::close_popover(ui);
                        let dir = imagery.asset_dir.to_owned();
                        let sources = imagery.sources.to_vec();
                        self.spawn(ui.ctx(), move || {
                            Update::from(open_cached(&entry, &sources, &dir, Origin::Picked))
                        });
                        self.error = None;
                        self.notice = None;
                    }
                    Some(None) => {}
                    None => self.cache_listing = None,
                }
            }
            if ui
                .add_enabled(
                    self.pending.is_none(),
                    egui::Button::new("Import PNG / JPEG…"),
                )
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("Track image", &["png", "jpg", "jpeg"])
                    .pick_file()
            {
                config.image_path = Some(path);
                config.bounds = None;
                config.controls = None;
                config.attribution.clear();
                config.source_url = None;
                self.editing_bounds = course_bounds;
                self.error = None;
            }
            if show_clear && ui.button("Clear image").clicked() {
                // Drop the receiver; an in-flight network request may finish
                // its bounded download but cannot replace this selection.
                self.pending = None;
                self.notice = None;
                *config = ImageryConfig::default();
                self.texture = None;
                self.loaded_path = None;
                self.requested_path = None;
            }
        });
        if self.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading imagery in the background…");
            });
        }
        if let Some(notice) = &self.notice {
            widgets::hint(ui, notice.clone());
        }
        if let Some(error) = &self.error {
            widgets::callout(ui, Tone::Bad, error);
            if ui.button("Retry image load").clicked() {
                self.requested_path = None;
            }
        }
    }

    /// Lists the cached images of `course`; returns the one clicked.
    fn cache_menu(
        &mut self,
        ui: &mut egui::Ui,
        config: &ImageryConfig,
        course: GeoBounds,
        sources: &[ImagerySource],
    ) -> Option<CachedImage> {
        let listing = self.cache_listing.get_or_insert_with(|| {
            cached_for_course(
                read_cache(&crate::app_paths::imagery_cache_dir()),
                course,
                sources,
            )
        });
        if listing.is_empty() {
            widgets::hint(ui, "No cached images of this course");
            return None;
        }
        let current = config.image_path.as_deref().and_then(Path::file_name);
        let mut picked = None;
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for entry in listing.iter() {
                    let selected = current.is_some() && entry.image.file_name() == current;
                    if ui
                        .add(
                            egui::Button::selectable(selected, entry.name(sources))
                                .min_size(egui::vec2(ui.available_width(), 24.0)),
                        )
                        .clicked()
                    {
                        picked = Some(entry.clone());
                    }
                    widgets::hint(ui, entry.caption(course));
                    ui.add_space(2.0);
                }
            });
        picked
    }

    /// Draws the current toast centred at the top of `rect`, fading out over
    /// its last second. Repaints only while one is showing.
    pub fn paint_toast(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some((text, shown)) = &self.toast else {
            return;
        };
        let ctx = painter.ctx();
        let remaining = TOAST_SECONDS - (ctx.input(|i| i.time) - shown);
        if remaining <= 0.0 {
            return;
        }
        if remaining > TOAST_FADE_SECONDS {
            ctx.request_repaint_after(Duration::from_secs_f64(remaining - TOAST_FADE_SECONDS));
        } else {
            ctx.request_repaint();
        }
        let opacity = (remaining / TOAST_FADE_SECONDS).min(1.0) as f32;
        let galley = painter.layout_no_wrap(
            text.clone(),
            egui::FontId::proportional(12.0),
            theme::text::strong().gamma_multiply(opacity),
        );
        let padding = egui::vec2(10.0, 5.0);
        let size = galley.size() + 2.0 * padding;
        let toast = egui::Rect::from_min_size(
            egui::pos2(rect.center().x - size.x / 2.0, rect.top() + 10.0),
            size,
        );
        painter.rect_filled(toast, 4.0, theme::surface::raised().gamma_multiply(opacity));
        painter.galley(toast.min + padding, galley, theme::text::strong());
    }

    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut ImageryConfig,
        course_bounds: Option<GeoBounds>,
        imagery: &ImageryContext<'_>,
    ) {
        widgets::section_label(ui, "Track imagery");
        widgets::hint(
            ui,
            "Optional background. GPS comparison works without imagery or a saved track.",
        );
        self.quick_actions(ui, config, course_bounds, imagery, true);
        let mut registered = false;
        {
            if let Some(path) = &config.image_path {
                ui.small(path.display().to_string());
            }
            ui.add(egui::Slider::new(&mut config.opacity, 0.0..=1.0).text("Imagery opacity"));
            if config.image_path.is_some() {
                ui.collapsing("Register image: geographic bounds", |ui| {
                    let bounds = self.editing_bounds.get_or_insert(config.bounds.or(course_bounds).unwrap_or(GeoBounds {
                        west:-78.0,east:-77.99,south:42.0,north:42.01,
                    }));
                    ui.small("For a north-up image: enter its full west/south/east/north extent. These are image edges, not GPS trace bounds.");
                    ui.horizontal_wrapped(|ui| {
                        for (label,v) in [("West",&mut bounds.west),("South",&mut bounds.south),("East",&mut bounds.east),("North",&mut bounds.north)] {
                            ui.add(egui::DragValue::new(v).speed(0.00001).max_decimals(7).prefix(format!("{label} ")));
                        }
                    });
                    if ui.button("Apply bounds").clicked() {
                        if bounds.valid() { config.bounds=Some(*bounds);config.controls=None;self.registration_error=None;registered=true; }
                        else { self.registration_error=Some("Bounds must be ordered, finite WGS84 coordinates".into()); }
                    }
                });
                ui.collapsing("Register image: three control points", |ui| {
                    ui.small("Choose three identifiable non-collinear points. Click the image to set their image positions, then enter known latitude/longitude. Fractions are measured from its top-left corner.");
                    let bounds = config.bounds.or(course_bounds).unwrap_or(GeoBounds {west:0.0,east:0.01,south:0.0,north:0.01});
                    let points = self.editing_controls.get_or_insert(config.controls.unwrap_or([
                        ControlPoint {u:0.0,v:0.0,latitude:bounds.north,longitude:bounds.west},
                        ControlPoint {u:1.0,v:0.0,latitude:bounds.north,longitude:bounds.east},
                        ControlPoint {u:0.0,v:1.0,latitude:bounds.south,longitude:bounds.west},
                    ]));
                    let point_id = ui.id().with("imagery-point");
                    let active = ui.data_mut(|d| *d.get_temp_mut_or_default::<usize>(point_id)).min(2);
                    if let Some(texture) = &self.texture {
                        let size = texture.size_vec2();
                        let scale = (ui.available_width()/size.x).min(260.0/size.y).min(1.0);
                        let response=ui.add(egui::Image::new(texture).fit_to_exact_size(size*scale).sense(egui::Sense::click()));
                        if response.clicked() && let Some(pos)=response.interact_pointer_pos() {
                            points[active].u=((pos.x-response.rect.left())/response.rect.width()).clamp(0.0,1.0) as f64;
                            points[active].v=((pos.y-response.rect.top())/response.rect.height()).clamp(0.0,1.0) as f64;
                        }
                        for (i,p) in points.iter().enumerate() {
                            let pos=response.rect.min+egui::vec2(p.u as f32*response.rect.width(),p.v as f32*response.rect.height());
                            ui.painter().circle_filled(pos,4.0,Color32::YELLOW);
                            ui.painter().text(pos,egui::Align2::LEFT_BOTTOM,format!("{}",i+1),egui::FontId::monospace(12.0),Color32::YELLOW);
                        }
                    }
                    for (i,p) in points.iter_mut().enumerate() {
                        ui.horizontal_wrapped(|ui| {
                            if ui.selectable_label(active==i,format!("Point {}",i+1)).clicked() {
                                ui.data_mut(|d|d.insert_temp(point_id,i));
                            }
                            ui.add(egui::DragValue::new(&mut p.u).range(0.0..=1.0).speed(0.001).prefix("X "));
                            ui.add(egui::DragValue::new(&mut p.v).range(0.0..=1.0).speed(0.001).prefix("Y "));
                            ui.add(egui::DragValue::new(&mut p.latitude).speed(0.00001).max_decimals(7).prefix("Lat "));
                            ui.add(egui::DragValue::new(&mut p.longitude).speed(0.00001).max_decimals(7).prefix("Lon "));
                        });
                    }
                    if ui.button("Apply control points").clicked() {
                        match Registration::from_controls(*points) {
                            Ok(_) => {config.controls=Some(*points);self.registration_error=None;registered=true;}
                            Err(error) => self.registration_error=Some(error),
                        }
                    }
                });
                if let Err(error) = Registration::from_config(config) {
                    widgets::callout(ui, Tone::Warn, error);
                }
                if let Some(error) = &self.registration_error {
                    widgets::callout(ui, Tone::Bad, error);
                }
            }
            if !config.attribution.is_empty() {
                ui.small(&config.attribution);
            }
        }
        // Downloads are cached already; an import is kept once it is placed.
        if registered
            && config.source_url.is_none()
            && let Some(image) = config.image_path.clone()
            && let Some((bounds, controls)) = registered_extent(config)
        {
            let attribution = config.attribution.clone();
            let cache = crate::app_paths::imagery_cache_dir();
            self.cache_listing = None;
            std::thread::spawn(move || {
                if let Err(error) = remember_import(&image, bounds, controls, &attribution, &cache)
                {
                    log::warn!("Could not cache imported imagery: {error}");
                }
            });
        }
    }

    pub fn paint(
        &self,
        painter: &egui::Painter,
        config: &ImageryConfig,
        geo_to_screen: impl Fn(f64, f64) -> Pos2,
    ) {
        let Some(texture) = &self.texture else { return };
        if self.loaded_path != config.image_path {
            return;
        }
        let Ok(registration) = Registration::from_config(config) else {
            return;
        };
        let mut mesh = egui::Mesh::with_texture(texture.id());
        // A small grid respects projection curvature even if the map's view
        // uses local geographic coordinates rather than Web Mercator.
        const N: u32 = 12;
        let color = Color32::from_white_alpha((config.opacity.clamp(0.0, 1.0) * 255.0) as u8);
        for y in 0..=N {
            for x in 0..=N {
                let u = x as f64 / N as f64;
                let v = y as f64 / N as f64;
                let [lat, lon] = registration.geo(u, v);
                mesh.vertices.push(egui::epaint::Vertex {
                    pos: geo_to_screen(lat, lon),
                    uv: egui::pos2(u as f32, v as f32),
                    color,
                });
            }
        }
        for y in 0..N {
            for x in 0..N {
                let i = y * (N + 1) + x;
                mesh.indices
                    .extend_from_slice(&[i, i + 1, i + N + 1, i + 1, i + N + 2, i + N + 1]);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
        if !config.attribution.is_empty() {
            painter.text(
                painter.clip_rect().left_bottom() + egui::vec2(6.0, -4.0),
                egui::Align2::LEFT_BOTTOM,
                &config.attribution,
                egui::FontId::proportional(11.0),
                Color32::WHITE,
            );
        }
    }
}

fn decode_image(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let (w, h) = reader.into_dimensions().map_err(|e| e.to_string())?;
    if w == 0 || h == 0 || w as u64 * h as u64 > 32_000_000 {
        return Err("Track image must contain at most 32 megapixels".into());
    }
    let image = image::load_from_memory(bytes)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        image.as_raw(),
    ))
}

fn read_image_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("Cannot open imagery {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err("Image exceeds the 64 MiB input limit".into());
    }
    Ok(bytes)
}

fn load_image(path: &Path) -> Result<egui::ColorImage, String> {
    decode_image(&read_image_bytes(path)?)
}

fn image_extension(bytes: &[u8]) -> &'static str {
    match image::guess_format(bytes) {
        Ok(image::ImageFormat::Png) => "png",
        _ => "jpg",
    }
}

fn download_image(
    source: &ImagerySource,
    bounds: GeoBounds,
    dir: &Path,
) -> Result<LoadedImage, String> {
    let name = &source.name;
    if !bounds.valid() || !source.covers(bounds) {
        return Err(format!("{name} does not cover this course"));
    }
    let [west, south] = mercator(bounds.south, bounds.west);
    let [east, north] = mercator(bounds.north, bounds.east);
    if east - west > 30_000.0 || north - south > 30_000.0 {
        return Err("Select a course area smaller than 30 km across".into());
    }
    let fetched = imagery_sources::fetch(
        source,
        bounds,
        imagery_sources::MAX_IMAGE_PIXELS,
        Duration::from_secs(60),
    )?;
    let (bytes, actual, url) = (fetched.bytes, fetched.extent, fetched.request);
    let image = decode_image(&bytes)?;
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create imagery folder: {e}"))?;
    let path = dir.join(format!(
        "{}.{}",
        cache_key(&source.cache_tag(), bounds),
        image_extension(&bytes)
    ));
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let metadata = serde_json::json!({
        "bounds": actual,
        "attribution": source.attribution,
        "source_name": source.name,
        "source_url": url,
        "image": path.file_name().map(|name| name.to_string_lossy()),
    });
    fs::write(
        path.with_extension("image.json"),
        serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let attribution = if source.attribution.is_empty() {
        source.name.clone()
    } else {
        source.attribution.clone()
    };
    Ok(LoadedImage {
        path,
        image,
        placement: Some((
            Placement {
                bounds: actual,
                controls: None,
                source_url: Some(url),
                name: source.name.clone(),
                attribution,
            },
            Origin::Downloaded,
        )),
    })
}

/// Requested area rounded outward to a ~100 m grid, so nearby requests for the
/// same venue map to the same cache entry.
fn snapped(bounds: GeoBounds) -> GeoBounds {
    const GRID: f64 = 0.001;
    GeoBounds {
        west: (bounds.west / GRID).floor() * GRID,
        south: (bounds.south / GRID).floor() * GRID,
        east: (bounds.east / GRID).ceil() * GRID,
        north: (bounds.north / GRID).ceil() * GRID,
    }
}

/// `tag` names the source, so each source's images are cached separately.
fn cache_key(tag: &str, bounds: GeoBounds) -> String {
    let milli = |value: f64| (value * 1000.0).round() as i64;
    format!(
        "{tag}-w{}-s{}-e{}-n{}",
        milli(bounds.west),
        milli(bounds.south),
        milli(bounds.east),
        milli(bounds.north)
    )
}

/// Cache-key prefix of images the user imported and registered.
const IMPORT_TAG: &str = "import";

/// An image in the cache, with what its metadata file records.
#[derive(Clone, Debug)]
struct CachedImage {
    image: PathBuf,
    /// The cache-key prefix: a source's [`ImagerySource::cache_tag`] or
    /// [`IMPORT_TAG`].
    tag: String,
    /// The area the image shows; for a rotated import, the area around it.
    bounds: GeoBounds,
    controls: Option<[ControlPoint; 3]>,
    name: Option<String>,
    attribution: String,
    source_url: Option<String>,
    saved: Option<std::time::SystemTime>,
}

impl CachedImage {
    fn is_import(&self) -> bool {
        self.tag == IMPORT_TAG
    }

    /// The source's current name when it is still configured (the user may
    /// have renamed it), else the name recorded with the image.
    fn name(&self, sources: &[ImagerySource]) -> String {
        sources
            .iter()
            .cloned()
            .chain(std::iter::once(ImagerySource::usgs()))
            .find(|source| source.cache_tag() == self.tag)
            .map(|source| source.name)
            .or_else(|| self.name.clone())
            .unwrap_or_else(|| self.tag.clone())
    }

    fn placement(&self, sources: &[ImagerySource]) -> Placement {
        let name = self.name(sources);
        let attribution = if self.attribution.is_empty() && !self.is_import() {
            name.clone()
        } else {
            self.attribution.clone()
        };
        Placement {
            bounds: self.bounds,
            controls: self.controls,
            source_url: self.source_url.clone(),
            name,
            attribution,
        }
    }

    /// True when every point of a course with these bounds is in the image.
    /// A north-up image need only contain the bounds. A rotated import must
    /// contain their corners, which is slightly stricter than the course.
    fn shows_all_of(&self, course: GeoBounds) -> bool {
        let Some(registration) = self
            .controls
            .and_then(|controls| Registration::from_controls(controls).ok())
        else {
            return contains(self.bounds, course);
        };
        [
            (course.north, course.west),
            (course.north, course.east),
            (course.south, course.west),
            (course.south, course.east),
        ]
        .into_iter()
        .all(|(latitude, longitude)| {
            let [u, v] = registration.fraction(latitude, longitude);
            (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)
        })
    }

    fn caption(&self, course: GeoBounds) -> String {
        let date = self.saved.map(|saved| {
            chrono::DateTime::<chrono::Local>::from(saved)
                .format("%b %-d, %Y")
                .to_string()
        });
        let partial = (!self.shows_all_of(course)).then_some("partial");
        [date.as_deref(), partial]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

fn area(bounds: GeoBounds) -> f64 {
    (bounds.east - bounds.west) * (bounds.north - bounds.south)
}

fn contains(outer: GeoBounds, inner: GeoBounds) -> bool {
    outer.west <= inner.west
        && outer.east >= inner.east
        && outer.south <= inner.south
        && outer.north >= inner.north
}

fn overlaps(a: GeoBounds, b: GeoBounds) -> bool {
    a.west < b.east && a.east > b.west && a.south < b.north && a.north > b.south
}

/// Every usable image in the cache: its metadata parses and its image file
/// is present.
fn read_cache(cache_dir: &Path) -> Vec<CachedImage> {
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let key = file_name.strip_suffix(".image.json")?;
            let tag = key.split_once('-').map_or(key, |(tag, _)| tag).to_owned();
            let metadata: serde_json::Value =
                serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let text = |field: &str| {
                metadata
                    .get(field)
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            };
            let bounds: GeoBounds = serde_json::from_value(metadata.get("bounds")?.clone()).ok()?;
            let controls = metadata
                .get("controls")
                .and_then(|controls| serde_json::from_value(controls.clone()).ok());
            // Images cached before sources existed are PNGs named like their metadata.
            let image = entry
                .path()
                .with_file_name(text("image").unwrap_or_else(|| format!("{key}.png")));
            let saved = fs::metadata(&image).ok()?.modified().ok();
            (bounds.valid() && image.is_file()).then(|| CachedImage {
                image,
                tag,
                bounds,
                controls,
                name: text("source_name").filter(|name| !name.is_empty()),
                attribution: text("attribution").unwrap_or_default(),
                source_url: text("source_url"),
                saved,
            })
        })
        .collect()
}

/// The area requested for a course: its GPS bounds with a margin, rounded
/// outward to a ~100 m grid so nearby courses share cache entries.
fn request_area(course: GeoBounds) -> GeoBounds {
    snapped(course.padded())
}

/// The smallest image cached from the source tagged `tag` that shows the
/// whole course (the bounds of its GPS points), without being so much larger
/// than what would be requested for it that its resolution would be poor.
/// The course lies inside the margin around it, well clear of the edges of
/// an image downloaded for it.
fn find_cached<'a>(
    entries: &'a [CachedImage],
    tag: &str,
    course: GeoBounds,
) -> Option<&'a CachedImage> {
    let largest = area(request_area(course)) * 4.0;
    entries
        .iter()
        .filter(|entry| {
            entry.tag == tag && contains(entry.bounds, course) && area(entry.bounds) <= largest
        })
        .min_by(|a, b| area(a.bounds).total_cmp(&area(b.bounds)))
}

/// The most preferred of `candidates` with a cached image of `course`: its
/// rank (0 is the top source) and the image.
fn first_cached<'a>(
    entries: &'a [CachedImage],
    candidates: &[ImagerySource],
    course: GeoBounds,
) -> Option<(usize, &'a CachedImage)> {
    candidates.iter().enumerate().find_map(|(rank, source)| {
        find_cached(entries, &source.cache_tag(), course).map(|hit| (rank, hit))
    })
}

/// Cached images showing any of `course`, for the user to choose from:
/// those covering all of it first, then in source order (imports and
/// sources no longer configured last), then the most detailed.
fn cached_for_course(
    entries: Vec<CachedImage>,
    course: GeoBounds,
    sources: &[ImagerySource],
) -> Vec<CachedImage> {
    let tags = imagery_sources::candidates(sources)
        .map(|source| source.cache_tag())
        .collect::<Vec<_>>();
    let rank = |entry: &CachedImage| {
        tags.iter()
            .position(|tag| *tag == entry.tag)
            .unwrap_or(usize::MAX)
    };
    let mut entries = entries
        .into_iter()
        .filter(|entry| overlaps(entry.bounds, course))
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        (!a.shows_all_of(course), rank(a))
            .cmp(&(!b.shows_all_of(course), rank(b)))
            .then(area(a.bounds).total_cmp(&area(b.bounds)))
    });
    entries
}

/// Copies a cached image (and its metadata) beside the workspace so saved
/// workspaces keep working without the cache. Returns the workspace copy.
fn install_asset(source: &Path, asset_dir: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(asset_dir).map_err(|e| format!("Cannot create imagery folder: {e}"))?;
    let name = source.file_name().ok_or("Invalid imagery path")?;
    let destination = asset_dir.join(name);
    let same = fs::canonicalize(&destination)
        .ok()
        .zip(fs::canonicalize(source).ok())
        .is_some_and(|(a, b)| a == b);
    if !same {
        fs::copy(source, &destination).map_err(|e| e.to_string())?;
        let _ = fs::copy(
            source.with_extension("image.json"),
            destination.with_extension("image.json"),
        );
    }
    fs::canonicalize(destination).map_err(|e| e.to_string())
}

fn open_cached(
    entry: &CachedImage,
    sources: &[ImagerySource],
    asset_dir: &Path,
    origin: Origin,
) -> Result<LoadedImage, String> {
    let path = install_asset(&entry.image, asset_dir)?;
    Ok(LoadedImage {
        image: load_image(&path)?,
        path,
        placement: Some((entry.placement(sources), origin)),
    })
}

/// Imagery for a course with GPS bounds `course`, sending what it finds as it
/// goes. `candidates` are
/// the sources covering the course, most preferred first. A cached image from
/// the top source is used without contacting the network. One from a lower
/// source is shown at once while the top source is asked for its image,
/// which replaces it when it arrives. Only `refresh` skips the cache.
fn get_imagery(
    candidates: &[ImagerySource],
    course: GeoBounds,
    asset_dir: &Path,
    cache_dir: &Path,
    refresh: bool,
    send: &mut dyn FnMut(Update),
) {
    let Some(top) = candidates.first() else {
        return send(Update::Failed(
            "No imagery source covers this course".into(),
        ));
    };
    let mut shown = None;
    if !refresh {
        let entries = read_cache(cache_dir);
        if let Some((rank, hit)) = first_cached(&entries, candidates, course)
            && let Ok(loaded) = open_cached(hit, candidates, asset_dir, Origin::Cached)
        {
            send(Update::from(Ok(loaded)));
            if rank == 0 {
                return;
            }
            shown = Some(hit.name(candidates));
        }
    }
    let downloaded = download_image(top, request_area(course), cache_dir).and_then(|mut loaded| {
        loaded.path = install_asset(&loaded.path, asset_dir)?;
        Ok(loaded)
    });
    send(match (downloaded, shown) {
        (Ok(loaded), _) => Update::from(Ok(loaded)),
        (Err(_), Some(name)) => Update::Toast(format!(
            "{} unavailable; showing cached {name} image.",
            top.name
        )),
        (Err(error), None) => Update::Failed(error),
    });
}

/// The registered area of the configured image and its control points, if
/// it is placed by them.
fn registered_extent(config: &ImageryConfig) -> Option<(GeoBounds, Option<[ControlPoint; 3]>)> {
    let Some(controls) = config.controls else {
        return config
            .bounds
            .filter(|bounds| bounds.valid())
            .map(|b| (b, None));
    };
    let registration = Registration::from_controls(controls).ok()?;
    let corners =
        [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)].map(|(u, v)| registration.geo(u, v));
    let latitudes = corners.map(|[latitude, _]| latitude);
    let longitudes = corners.map(|[_, longitude]| longitude);
    let bounds = GeoBounds {
        west: longitudes.into_iter().fold(f64::INFINITY, f64::min),
        south: latitudes.into_iter().fold(f64::INFINITY, f64::min),
        east: longitudes.into_iter().fold(f64::NEG_INFINITY, f64::max),
        north: latitudes.into_iter().fold(f64::NEG_INFINITY, f64::max),
    };
    bounds.valid().then_some((bounds, Some(controls)))
}

/// Keeps a registered imported image in the cache, so any workspace covering
/// the same place can choose it. The same file imported again replaces its
/// entry, so the latest registration wins.
fn remember_import(
    image: &Path,
    bounds: GeoBounds,
    controls: Option<[ControlPoint; 3]>,
    attribution: &str,
    cache_dir: &Path,
) -> Result<PathBuf, String> {
    let bytes = read_image_bytes(image)?;
    fs::create_dir_all(cache_dir).map_err(|e| format!("Cannot create imagery folder: {e}"))?;
    let key = format!("{IMPORT_TAG}-{:016x}", imagery_sources::stable_hash(&bytes));
    let path = cache_dir.join(format!("{key}.{}", image_extension(&bytes)));
    if !path.is_file() {
        fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    }
    let file_name = image
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let metadata = serde_json::json!({
        "bounds": bounds,
        "controls": controls,
        "attribution": attribution,
        "source_name": format!("Imported {file_name}"),
        "image": path.file_name().map(|name| name.to_string_lossy()),
    });
    fs::write(
        path.with_extension("image.json"),
        serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    // All coordinates here are synthetic: no recorded positions belong in
    // the repository.
    fn bounds(west: f64, south: f64, east: f64, north: f64) -> GeoBounds {
        GeoBounds {
            west,
            south,
            east,
            north,
        }
    }

    /// A made-up course a few hundred metres across.
    fn course() -> GeoBounds {
        bounds(-100.0046, 40.0013, -100.0021, 40.0037)
    }

    #[test]
    #[ignore = "requires access to the public USGS imagery service"]
    fn downloads_georeferenced_usgs_image() {
        let directory = tempfile::tempdir().unwrap();
        let result = download_image(
            &ImagerySource::usgs(),
            request_area(course()),
            directory.path(),
        )
        .unwrap();
        assert!(result.path.is_file());
        assert!(result.placement.unwrap().0.bounds.valid());
        assert!(!result.image.pixels.is_empty());
    }

    #[test]
    fn snapping_expands_outward_and_is_stable() {
        let s = snapped(bounds(-100.0074, 40.0012, -100.0041, 40.0038));
        assert!(s.west <= -100.0074 && s.east >= -100.0041);
        assert!(s.south <= 40.0012 && s.north >= 40.0038);
        assert_eq!(
            cache_key("usgs", s),
            cache_key(
                "usgs",
                snapped(bounds(-100.0079, 40.0011, -100.0042, 40.0039))
            )
        );
    }

    fn cache_entry(dir: &Path, key: &str, actual: GeoBounds) {
        image::RgbImage::new(4, 4)
            .save(dir.join(format!("{key}.png")))
            .unwrap();
        let metadata = serde_json::json!({"bounds": actual, "source_url": "https://example.test"});
        fs::write(
            dir.join(format!("{key}.image.json")),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn cache_reuses_an_image_showing_the_course_and_ignores_others() {
        let dir = tempfile::tempdir().unwrap();
        let cached = bounds(-100.01, 40.00, -100.00, 40.01);
        cache_entry(dir.path(), "usgs-a", cached);
        let entries = read_cache(dir.path());
        // Inside the cached area: hit.
        let inside = bounds(-100.008, 40.002, -100.002, 40.008);
        assert_eq!(
            find_cached(&entries, "usgs", inside).unwrap().bounds,
            cached
        );
        // Another source's cache does not satisfy this one.
        assert!(find_cached(&entries, "src0123abcd", inside).is_none());
        // Partly outside: miss.
        let outside = bounds(-100.012, 40.002, -100.002, 40.008);
        assert!(find_cached(&entries, "usgs", outside).is_none());
        // A tiny course inside a huge cached image would be low resolution.
        let tiny = bounds(-100.0052, 40.0050, -100.0048, 40.0054);
        assert!(find_cached(&entries, "usgs", tiny).is_none());
        // Missing image file: not listed.
        fs::remove_file(dir.path().join("usgs-a.png")).unwrap();
        assert!(read_cache(dir.path()).is_empty());
    }

    #[test]
    fn an_image_downloaded_for_a_course_is_found_for_it_again() {
        let dir = tempfile::tempdir().unwrap();
        // A service may return the requested area a hair smaller after the
        // round trip through Web Mercator; the course is far inside it.
        let shrink = 1e-9;
        let request = request_area(course());
        let returned = bounds(
            request.west + shrink,
            request.south + shrink,
            request.east - shrink,
            request.north - shrink,
        );
        cache_entry(dir.path(), &cache_key("usgs", request), returned);
        let entries = read_cache(dir.path());
        assert!(find_cached(&entries, "usgs", course()).is_some());
        // Another recording of the same course, a little offset, too.
        let offset = bounds(-100.0047, 40.0012, -100.0022, 40.0036);
        assert!(find_cached(&entries, "usgs", offset).is_some());
    }

    fn source(name: &str) -> ImagerySource {
        ImagerySource {
            name: name.into(),
            // Nothing listens here: a request fails at once, without leaving
            // this machine.
            url: format!("http://127.0.0.1:9/arcgis/rest/services/{name}/MapServer"),
            ..ImagerySource::usgs()
        }
    }

    fn cache_for(dir: &Path, source: &ImagerySource) {
        let request = request_area(course());
        cache_entry(dir, &cache_key(&source.cache_tag(), request), request);
    }

    fn run(candidates: &[ImagerySource], cache: &Path, assets: &Path) -> Vec<Update> {
        let mut updates = Vec::new();
        get_imagery(candidates, course(), assets, cache, false, &mut |update| {
            updates.push(update)
        });
        updates
    }

    #[test]
    fn the_top_source_cached_needs_no_request() {
        let cache = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let (state, county) = (source("State"), source("County"));
        cache_for(cache.path(), &state);
        cache_for(cache.path(), &county);

        let updates = run(&[state, county], cache.path(), assets.path());
        let [Update::Loaded(loaded)] = updates.as_slice() else {
            panic!("expected only the cached image");
        };
        let (placement, origin) = loaded.placement.as_ref().unwrap();
        assert_eq!(
            (placement.name.as_str(), *origin),
            ("State", Origin::Cached)
        );
        assert!(
            loaded
                .path
                .starts_with(fs::canonicalize(assets.path()).unwrap())
        );
    }

    #[test]
    fn a_lower_source_cached_is_shown_while_the_top_one_is_tried() {
        let cache = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let (state, county) = (source("State"), source("County"));
        cache_for(cache.path(), &county);

        let updates = run(&[state.clone(), county], cache.path(), assets.path());
        let [Update::Loaded(loaded), Update::Toast(toast)] = updates.as_slice() else {
            panic!("expected the cached image, then a toast");
        };
        assert_eq!(loaded.placement.as_ref().unwrap().0.name, "County");
        assert!(toast.starts_with("State unavailable"), "{toast}");

        // Nothing cached at all: the failure is an error.
        let empty = tempfile::tempdir().unwrap();
        let updates = run(&[state], empty.path(), assets.path());
        assert!(matches!(updates.as_slice(), [Update::Failed(_)]));
    }

    #[test]
    fn the_toast_lasts_five_seconds() {
        let ctx = egui::Context::default();
        let mut controller = ImageryController::default();
        let mut config = ImageryConfig::default();
        controller.apply(&ctx, &mut config, Update::Toast("Gone".into()));
        assert!(controller.toast.is_some());
        let at = |seconds: f64| egui::RawInput {
            time: Some(seconds),
            ..Default::default()
        };
        let frame = |controller: &mut ImageryController, seconds: f64| {
            let mut config = ImageryConfig::default();
            ctx.run_ui(at(seconds), |ui| controller.poll(ui.ctx(), &mut config))
                .textures_delta
                .clear()
        };
        frame(&mut controller, 4.9);
        assert!(controller.toast.is_some());
        frame(&mut controller, 5.1);
        assert!(controller.toast.is_none());
    }

    #[test]
    fn a_picked_image_is_kept_in_the_workspace_settings() {
        let cache = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let county = source("County");
        cache_for(cache.path(), &county);
        let entries = read_cache(cache.path());
        let picked = open_cached(&entries[0], &[county], assets.path(), Origin::Picked).unwrap();

        let mut controller = ImageryController::default();
        let mut config = ImageryConfig::default();
        controller.apply(
            &egui::Context::default(),
            &mut config,
            Update::from(Ok(picked)),
        );
        let saved: ImageryConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert!(
            saved
                .image_path
                .unwrap()
                .starts_with(fs::canonicalize(assets.path()).unwrap())
        );
        assert_eq!(saved.bounds, Some(request_area(course())));
        assert_eq!(saved.attribution, "County");
        assert_eq!(saved.source_url.as_deref(), Some("https://example.test"));
    }

    #[test]
    fn the_picker_lists_every_cached_image_of_the_course_best_first() {
        let cache = tempfile::tempdir().unwrap();
        let (state, county) = (source("State"), source("County"));
        let course = bounds(-100.0065, 40.0015, -100.0045, 40.0035);
        let covering = bounds(-100.007, 40.001, -100.004, 40.004);
        let wider = bounds(-100.008, 40.000, -100.003, 40.005);
        let partial = bounds(-100.006, 40.001, -100.004, 40.004);
        cache_entry(
            cache.path(),
            &cache_key(&county.cache_tag(), covering),
            covering,
        );
        cache_entry(cache.path(), &cache_key(&state.cache_tag(), wider), wider);
        cache_entry(
            cache.path(),
            &cache_key(&state.cache_tag(), partial),
            partial,
        );
        cache_entry(cache.path(), "src00000000-gone", covering);
        let elsewhere = bounds(-90.0, 35.0, -89.9, 35.1);
        cache_entry(cache.path(), &cache_key("usgs", elsewhere), elsewhere);

        let sources = [state, county];
        let listed = cached_for_course(read_cache(cache.path()), course, &sources);
        let names = listed
            .iter()
            .map(|entry| (entry.name(&sources), entry.shows_all_of(course)))
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                ("State".to_owned(), true),
                ("County".to_owned(), true),
                ("src00000000".to_owned(), true),
                ("State".to_owned(), false),
            ]
        );
        assert!(listed[3].caption(course).ends_with("partial"));
    }

    #[test]
    fn a_registered_import_is_cached_for_the_picker() {
        let cache = tempfile::tempdir().unwrap();
        let originals = tempfile::tempdir().unwrap();
        let photo = originals.path().join("track.png");
        image::RgbImage::new(8, 8).save(&photo).unwrap();
        let points = [
            ControlPoint {
                u: 0.0,
                v: 0.0,
                latitude: 40.004,
                longitude: -100.007,
            },
            ControlPoint {
                u: 1.0,
                v: 0.0,
                latitude: 40.004,
                longitude: -100.004,
            },
            ControlPoint {
                u: 0.0,
                v: 1.0,
                latitude: 40.001,
                longitude: -100.007,
            },
        ];
        let config = ImageryConfig {
            image_path: Some(photo.clone()),
            controls: Some(points),
            ..Default::default()
        };
        let (extent, controls) = registered_extent(&config).unwrap();
        let course = bounds(-100.0065, 40.0015, -100.0045, 40.0035);
        assert!(contains(extent, course));
        remember_import(&photo, extent, controls, "", cache.path()).unwrap();
        // Registering the same image again updates its one entry.
        remember_import(&photo, extent, controls, "Survey", cache.path()).unwrap();

        let entries = read_cache(cache.path());
        let [entry] = entries.as_slice() else {
            panic!("expected one cached import");
        };
        assert!(entry.is_import());
        assert!(entry.shows_all_of(course));
        assert!(!entry.shows_all_of(bounds(-100.0075, 40.0015, -100.0045, 40.0035)));
        assert_eq!(entry.name(&[]), "Imported track.png");
        let placement = entry.placement(&[]);
        assert_eq!(placement.attribution, "Survey");
        assert!(placement.controls.is_some() && placement.source_url.is_none());
        // An import is offered, never chosen by "Get aerial image".
        assert!(first_cached(&entries, &[ImagerySource::usgs()], course).is_none());
    }

    #[test]
    fn cached_image_is_copied_beside_the_workspace() {
        let cache = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        cache_entry(
            cache.path(),
            "usgs-a",
            bounds(-100.01, 40.00, -100.00, 40.01),
        );
        let installed = install_asset(&cache.path().join("usgs-a.png"), assets.path()).unwrap();
        assert!(installed.starts_with(fs::canonicalize(assets.path()).unwrap()));
        assert!(installed.with_extension("image.json").is_file());
        // Installing from its own folder is a no-op rather than a self-copy.
        assert!(install_asset(&installed, assets.path()).is_ok());
    }

    #[test]
    fn geographic_round_trip() {
        for (lat, lon) in [(40.0, -100.0), (0.0, 0.0), (-30.0, 150.0)] {
            let [x, y] = mercator(lat, lon);
            let actual = geographic(x, y);
            assert!((actual[0] - lat).abs() < 1e-9);
            assert!((actual[1] - lon).abs() < 1e-9);
        }
    }
    #[test]
    fn bounds_preserve_image_orientation() {
        let b = GeoBounds {
            west: -77.0,
            east: -76.0,
            south: 42.0,
            north: 43.0,
        };
        let cfg = ImageryConfig {
            bounds: Some(b),
            ..Default::default()
        };
        let r = Registration::from_config(&cfg).unwrap();
        assert!((r.geo(0.0, 0.0)[0] - 43.0).abs() < 1e-9);
        assert!((r.geo(1.0, 1.0)[1] + 76.0).abs() < 1e-9);
    }
    #[test]
    fn rotated_registration_and_degenerate_rejection() {
        let p = [
            ControlPoint {
                u: 0.0,
                v: 0.0,
                latitude: 42.0,
                longitude: -77.0,
            },
            ControlPoint {
                u: 1.0,
                v: 0.0,
                latitude: 42.001,
                longitude: -77.001,
            },
            ControlPoint {
                u: 0.0,
                v: 1.0,
                latitude: 42.001,
                longitude: -76.999,
            },
        ];
        let r = Registration::from_controls(p).unwrap();
        for point in p {
            let [lat, lon] = r.geo(point.u, point.v);
            assert!((lat - point.latitude).abs() < 1e-9);
            assert!((lon - point.longitude).abs() < 1e-9);
        }
        let [lat, lon] = r.geo(0.3, 0.7);
        let [u, v] = r.fraction(lat, lon);
        assert!((u - 0.3).abs() < 1e-6 && (v - 0.7).abs() < 1e-6);
        assert!(Registration::from_controls([p[0], p[0], p[2]]).is_err());
        let same = ControlPoint {
            u: 1.0,
            v: 0.0,
            ..p[0]
        };
        assert!(Registration::from_controls([p[0], same, p[2]]).is_err());
    }
    #[test]
    fn invalid_bounds_and_unknown_fields() {
        let b = GeoBounds {
            west: 1.0,
            east: 0.0,
            south: 0.0,
            north: 1.0,
        };
        assert!(!b.valid());
        let value = serde_json::json!({"future":{"test":true}});
        let cfg: ImageryConfig = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(cfg).unwrap()["future"],
            value["future"]
        );
        assert!(decode_image(b"not an image").is_err());
    }
}
