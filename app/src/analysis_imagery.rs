//! Optional, georeferenced track imagery. Fetching and decoding never run in
//! the UI thread; the GPS trace and comparison do not depend on this service.
use crate::ui_kit::{Tone, widgets};
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
const USGS: &str =
    "https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer/export";
const ATTRIBUTION: &str = "USDA, USGS The National Map: Orthoimagery";

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

    fn conus(self) -> bool {
        self.valid()
            && self.west >= -125.0
            && self.east <= -66.0
            && self.south >= 24.0
            && self.north <= 50.0
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

    fn geo(self, u: f64, v: f64) -> [f64; 2] {
        geographic(
            self.x[0] * u + self.x[1] * v + self.x[2],
            self.y[0] * u + self.y[1] * v + self.y[2],
        )
    }
}

struct LoadedImage {
    path: PathBuf,
    image: egui::ColorImage,
    downloaded_bounds: Option<GeoBounds>,
    source_url: Option<String>,
    /// True when a locally cached image satisfied the request.
    from_cache: bool,
}

#[derive(Default)]
pub struct ImageryController {
    texture: Option<TextureHandle>,
    loaded_path: Option<PathBuf>,
    requested_path: Option<PathBuf>,
    pending: Option<Receiver<Result<LoadedImage, String>>>,
    error: Option<String>,
    /// Non-error status, such as "used a cached image".
    notice: Option<String>,
    registration_error: Option<String>,
    editing_bounds: Option<GeoBounds>,
    editing_controls: Option<[ControlPoint; 3]>,
}

impl ImageryController {
    pub fn poll(&mut self, ctx: &egui::Context, config: &mut ImageryConfig) {
        if let Some(result) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.pending = None;
            match result {
                Ok(loaded) => {
                    if let Some(bounds) = loaded.downloaded_bounds {
                        config.image_path = Some(loaded.path.clone());
                        config.bounds = Some(bounds);
                        config.controls = None;
                        config.attribution = ATTRIBUTION.into();
                        config.source_url = loaded.source_url;
                        self.notice = Some(if loaded.from_cache {
                            "Used the locally cached image; nothing was downloaded.".into()
                        } else {
                            "Downloaded from USGS and cached for reuse.".into()
                        });
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
                Err(error) => self.error = Some(error),
            }
        }
        if self.pending.is_none() && self.requested_path != config.image_path {
            self.requested_path = config.image_path.clone();
            self.texture = None;
            self.loaded_path = None;
            if let Some(path) = config.image_path.clone() {
                let (tx, rx) = mpsc::channel();
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let result = load_image(&path).map(|image| LoadedImage {
                        path,
                        image,
                        downloaded_bounds: None,
                        source_url: None,
                        from_cache: false,
                    });
                    let _ = tx.send(result);
                    repaint.request_repaint();
                });
                self.pending = Some(rx);
            }
        }
    }

    /// Download / import / clear buttons plus load status. Shared by the
    /// inline "add imagery" prompt and the options menu.
    pub fn quick_actions(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut ImageryConfig,
        course_bounds: Option<GeoBounds>,
        asset_dir: &Path,
        show_clear: bool,
    ) {
        self.poll(ui.ctx(), config);
        ui.horizontal_wrapped(|ui| {
            let valid = course_bounds.is_some_and(|b| b.padded().conus());
            let mut request = None;
            if ui.add_enabled(valid && self.pending.is_none(), egui::Button::new("Get US aerial image"))
                .on_hover_text("Uses a locally cached image of this course when one exists; otherwise downloads it once from USGS and caches it. Continental US only; imagery may predate this event.")
                .clicked() {
                request = Some(false);
            }
            if config.source_url.is_some()
                && ui.add_enabled(valid && self.pending.is_none(), egui::Button::new("Re-download"))
                    .on_hover_text("Ignore the cache and fetch a fresh copy from USGS")
                    .clicked() {
                request = Some(true);
            }
            if let Some(refresh) = request {
                let bounds = course_bounds.expect("enabled for bounds").padded();
                let dir = asset_dir.to_owned();
                let cache = crate::app_paths::imagery_cache_dir();
                let repaint = ui.ctx().clone();
                let (tx,rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let result = fetch_imagery(bounds, &dir, &cache, refresh);
                    let _ = tx.send(result);
                    repaint.request_repaint();
                });
                self.error = None;
                self.notice = None;
                self.pending = Some(rx);
            }
            if ui.add_enabled(self.pending.is_none(), egui::Button::new("Import PNG / JPEG…")).clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Track image", &["png","jpg","jpeg"]).pick_file() {
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

    pub fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut ImageryConfig,
        course_bounds: Option<GeoBounds>,
        asset_dir: &Path,
    ) {
        widgets::section_label(ui, "Track imagery");
        widgets::hint(
            ui,
            "Optional background. GPS comparison works without imagery or a saved track.",
        );
        self.quick_actions(ui, config, course_bounds, asset_dir, true);
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
                        if bounds.valid() { config.bounds=Some(*bounds);config.controls=None;self.registration_error=None; }
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
                            Ok(_) => {config.controls=Some(*points);self.registration_error=None;}
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

fn load_image(path: &Path) -> Result<egui::ColorImage, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("Cannot open imagery {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err("Image exceeds the 64 MiB input limit".into());
    }
    decode_image(&bytes)
}

fn download_image(bounds: GeoBounds, dir: &Path) -> Result<LoadedImage, String> {
    if !bounds.conus() {
        return Err(
            "Offline USGS imagery currently supports continental-US course areas only".into(),
        );
    }
    let [west, south] = mercator(bounds.south, bounds.west);
    let [east, north] = mercator(bounds.north, bounds.east);
    if east - west > 30_000.0 || north - south > 30_000.0 {
        return Err("Select a course area smaller than 30 km across".into());
    }
    let longest = (east - west).max(north - south);
    let width = ((east - west) / longest * 2048.0).round().max(64.0) as u32;
    let height = ((north - south) / longest * 2048.0).round().max(64.0) as u32;
    let url = format!(
        "{USGS}?f=json&bbox={west},{south},{east},{north}&bboxSR=3857&imageSR=3857&size={width},{height}&format=png32&transparent=false"
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut response = agent
        .get(&url)
        .call()
        .map_err(|e| format!("USGS imagery request: {e}"))?;
    let json: serde_json::Value = serde_json::from_str(
        &response
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let href = json.get("href").and_then(|v| v.as_str()).ok_or_else(|| {
        format!(
            "USGS returned no image: {}",
            json.get("error").unwrap_or(&json)
        )
    })?;
    // Do not follow arbitrary URLs from an external service response.
    if !href.starts_with("https://basemap.nationalmap.gov/") {
        return Err("USGS returned an unexpected image host".into());
    }
    let extent = json
        .get("extent")
        .ok_or("USGS response omitted the actual image extent")?;
    let coordinate = |key: &str| {
        extent
            .get(key)
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("Invalid USGS extent {key}"))
    };
    let [actual_south, actual_west] = geographic(coordinate("xmin")?, coordinate("ymin")?);
    let [actual_north, actual_east] = geographic(coordinate("xmax")?, coordinate("ymax")?);
    let actual = GeoBounds {
        west: actual_west,
        south: actual_south,
        east: actual_east,
        north: actual_north,
    };
    if !actual.valid() {
        return Err("USGS returned an invalid image extent".into());
    }
    let mut response = agent.get(href).call().map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_IMAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        return Err("USGS image exceeds 64 MiB".into());
    }
    let image = decode_image(&bytes)?;
    fs::create_dir_all(dir).map_err(|e| format!("Cannot create imagery folder: {e}"))?;
    let path = dir.join(format!("{}.png", cache_key(bounds)));
    fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let path = fs::canonicalize(path).map_err(|e| e.to_string())?;
    let metadata = serde_json::json!({"bounds":actual,"attribution":ATTRIBUTION,"source_url":url});
    fs::write(
        path.with_extension("image.json"),
        serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(LoadedImage {
        path,
        image,
        downloaded_bounds: Some(actual),
        source_url: Some(url),
        from_cache: false,
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

fn cache_key(bounds: GeoBounds) -> String {
    let milli = |value: f64| (value * 1000.0).round() as i64;
    format!(
        "usgs-w{}-s{}-e{}-n{}",
        milli(bounds.west),
        milli(bounds.south),
        milli(bounds.east),
        milli(bounds.north)
    )
}

struct CachedImage {
    png: PathBuf,
    bounds: GeoBounds,
    source_url: Option<String>,
}

fn area(bounds: GeoBounds) -> f64 {
    (bounds.east - bounds.west) * (bounds.north - bounds.south)
}

/// The smallest cached image that fully covers `wanted` without being so much
/// larger that its resolution would be poor for the course.
fn find_cached(cache_dir: &Path, wanted: GeoBounds) -> Option<CachedImage> {
    fs::read_dir(cache_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".image.json"))
        .filter_map(|entry| {
            let metadata: serde_json::Value =
                serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let bounds: GeoBounds = serde_json::from_value(metadata.get("bounds")?.clone()).ok()?;
            let png = entry.path().with_file_name(
                entry
                    .file_name()
                    .to_string_lossy()
                    .replace(".image.json", ".png"),
            );
            let covers = bounds.valid()
                && bounds.west <= wanted.west
                && bounds.east >= wanted.east
                && bounds.south <= wanted.south
                && bounds.north >= wanted.north
                && area(bounds) <= area(wanted) * 4.0;
            (covers && png.is_file()).then(|| CachedImage {
                png,
                bounds,
                source_url: metadata
                    .get("source_url")
                    .and_then(|url| url.as_str())
                    .map(str::to_owned),
            })
        })
        .min_by(|a, b| area(a.bounds).total_cmp(&area(b.bounds)))
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

/// Returns imagery for `bounds`: from the local cache when it has a suitable
/// image (unless `refresh`), otherwise downloaded once and cached.
fn fetch_imagery(
    bounds: GeoBounds,
    asset_dir: &Path,
    cache_dir: &Path,
    refresh: bool,
) -> Result<LoadedImage, String> {
    let wanted = snapped(bounds);
    if !refresh && let Some(hit) = find_cached(cache_dir, wanted) {
        let path = install_asset(&hit.png, asset_dir)?;
        let image = load_image(&path)?;
        return Ok(LoadedImage {
            path,
            image,
            downloaded_bounds: Some(hit.bounds),
            source_url: hit.source_url,
            from_cache: true,
        });
    }
    let mut loaded = download_image(wanted, cache_dir)?;
    loaded.path = install_asset(&loaded.path, asset_dir)?;
    Ok(loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires access to the public USGS imagery service"]
    fn downloads_georeferenced_usgs_image() {
        let directory = tempfile::tempdir().unwrap();
        let result = download_image(
            GeoBounds {
                west: -77.707,
                south: 42.891,
                east: -77.704,
                north: 42.894,
            },
            directory.path(),
        )
        .unwrap();
        assert!(result.path.is_file());
        assert!(result.downloaded_bounds.unwrap().valid());
        assert!(!result.image.pixels.is_empty());
    }
    fn bounds(west: f64, south: f64, east: f64, north: f64) -> GeoBounds {
        GeoBounds {
            west,
            south,
            east,
            north,
        }
    }

    fn cache_entry(dir: &Path, key: &str, actual: GeoBounds) {
        fs::write(dir.join(format!("{key}.png")), b"png").unwrap();
        let metadata = serde_json::json!({"bounds": actual, "source_url": "https://example.test"});
        fs::write(
            dir.join(format!("{key}.image.json")),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn snapping_expands_outward_and_is_stable() {
        let s = snapped(bounds(-77.7074, 42.8912, -77.7041, 42.8938));
        assert!(s.west <= -77.7074 && s.east >= -77.7041);
        assert!(s.south <= 42.8912 && s.north >= 42.8938);
        assert_eq!(snapped(s), s);
        assert_eq!(
            cache_key(s),
            cache_key(snapped(bounds(-77.7079, 42.8911, -77.7042, 42.8939)))
        );
    }

    #[test]
    fn cache_reuses_a_covering_image_and_ignores_others() {
        let dir = tempfile::tempdir().unwrap();
        let cached = bounds(-77.71, 42.89, -77.70, 42.90);
        cache_entry(dir.path(), "usgs-a", cached);
        // Inside the cached area: hit.
        let inside = bounds(-77.709, 42.8905, -77.701, 42.8995);
        assert_eq!(find_cached(dir.path(), inside).unwrap().bounds, cached);
        // Partly outside: miss.
        assert!(find_cached(dir.path(), bounds(-77.72, 42.891, -77.704, 42.895)).is_none());
        // A tiny course inside a huge cached image would be low resolution.
        assert!(find_cached(dir.path(), bounds(-77.7050, 42.8950, -77.7040, 42.8960)).is_none());
        // Missing image file: miss.
        fs::remove_file(dir.path().join("usgs-a.png")).unwrap();
        assert!(find_cached(dir.path(), inside).is_none());
    }

    #[test]
    fn cached_image_is_copied_beside_the_workspace() {
        let cache = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        cache_entry(cache.path(), "usgs-a", bounds(-77.71, 42.89, -77.70, 42.90));
        let installed = install_asset(&cache.path().join("usgs-a.png"), assets.path()).unwrap();
        assert!(installed.starts_with(fs::canonicalize(assets.path()).unwrap()));
        assert!(installed.with_extension("image.json").is_file());
        // Installing from its own folder is a no-op rather than a self-copy.
        assert!(install_asset(&installed, assets.path()).is_ok());
    }

    #[test]
    fn geographic_round_trip() {
        for (lat, lon) in [(42.712, -76.88), (0.0, 0.0), (-33.8, 151.2)] {
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
