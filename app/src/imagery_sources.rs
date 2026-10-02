//! Aerial imagery services the user has added in Settings.
//!
//! State and county GIS offices commonly publish orthoimagery as ArcGIS
//! MapServer or ImageServer services. Most draw an image of any area already
//! projected to Web Mercator; hosted tile layers instead serve fixed Web
//! Mercator tiles, which are stitched together here. A source is remembered
//! with what its service reports about itself, so choosing one for a course
//! needs no network access. USGS is built in as the fallback for the
//! continental US.

use crate::analysis_imagery::GeoBounds;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};

/// Key under which the user's sources are saved in eframe storage.
pub const STORAGE_KEY: &str = "race-overlay.imagery-sources";
/// Largest image requested from any service, whatever it allows: a texture
/// of this size is safe on every supported GPU.
pub const MAX_IMAGE_PIXELS: u32 = 4096;
/// More tiles than this means a mis-sized request rather than a course.
const MAX_TILES: usize = 1024;
const TILE_WORKERS: usize = 8;
/// Finer than any public orthoimagery, so requests never ask for more pixels
/// than a service can fill with detail.
const FINEST_METERS_PER_PIXEL: f64 = 0.15;
const RADIUS: f64 = 6_378_137.0;
const USGS_URL: &str =
    "https://basemap.nationalmap.gov/arcgis/rest/services/USGSImageryOnly/MapServer";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    MapServer,
    ImageServer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImagerySource {
    pub name: String,
    /// The service's REST address, ending in `/MapServer` or `/ImageServer`.
    pub url: String,
    pub kind: ServiceKind,
    /// The area the service reports covering; `None` when it uses a
    /// projection this app does not convert, so coverage is not checked.
    #[serde(default)]
    pub coverage: Option<GeoBounds>,
    #[serde(default)]
    pub attribution: String,
    #[serde(default = "default_max_image")]
    pub max_image_pixels: u32,
    /// Present when the service only serves pre-cut tiles (a hosted tile
    /// layer) instead of drawing images on request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiles: Option<TileScheme>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default, flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

/// A Web Mercator tile cache: square tiles counted from `origin` (top left),
/// with one resolution (metres per pixel) per zoom level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TileScheme {
    pub size: u32,
    pub origin: [f64; 2],
    pub levels: Vec<(u32, f64)>,
}

fn default_max_image() -> u32 {
    2048
}

fn enabled() -> bool {
    true
}

impl ImagerySource {
    /// The built-in fallback: USGS National Map orthoimagery.
    pub fn usgs() -> Self {
        Self {
            name: "USGS National Map".into(),
            url: USGS_URL.into(),
            kind: ServiceKind::MapServer,
            coverage: Some(GeoBounds {
                west: -125.0,
                south: 24.0,
                east: -66.0,
                north: 50.0,
            }),
            attribution: "USDA, USGS The National Map: Orthoimagery".into(),
            max_image_pixels: 2048,
            tiles: None,
            enabled: true,
            unknown: BTreeMap::new(),
        }
    }

    pub fn is_usgs(&self) -> bool {
        self.url == USGS_URL
    }

    pub fn covers(&self, bounds: GeoBounds) -> bool {
        self.coverage.is_none_or(|coverage| {
            coverage.west <= bounds.west
                && coverage.east >= bounds.east
                && coverage.south <= bounds.south
                && coverage.north >= bounds.north
        })
    }

    /// Names this source's entries in the download cache. USGS keeps the
    /// prefix its images were cached under before other sources existed.
    pub fn cache_tag(&self) -> String {
        if self.is_usgs() {
            return "usgs".into();
        }
        // FNV-1a: stable across runs and platforms, unlike `DefaultHasher`.
        let hash = self
            .url
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
            });
        format!("src{:08x}", hash as u32)
    }

    /// The request for a JPEG of `bounds` (Web Mercator metres), at most
    /// `max_pixels` across and within the service's limit, without asking for
    /// detail finer than any orthoimagery.
    fn export_url(&self, bounds: [f64; 4], max_pixels: u32) -> String {
        let [west, south, east, north] = bounds;
        let longest = (east - west).max(north - south);
        let pixels = (longest / finest_mercator_resolution(bounds))
            .min(f64::from(
                max_pixels.min(self.max_image_pixels).min(MAX_IMAGE_PIXELS),
            ))
            .max(16.0);
        let width = ((east - west) / longest * pixels).round().max(16.0) as u32;
        let height = ((north - south) / longest * pixels).round().max(16.0) as u32;
        self.request_url(bounds, width, height)
    }

    fn request_url(&self, [west, south, east, north]: [f64; 4], width: u32, height: u32) -> String {
        let operation = match self.kind {
            ServiceKind::MapServer => "export",
            ServiceKind::ImageServer => "exportImage",
        };
        format!(
            "{}/{operation}?f=json&bbox={west},{south},{east},{north}&bboxSR=3857&imageSR=3857&size={width},{height}&format=jpg",
            self.url
        )
    }

    /// The address to download an image the service named in its response:
    /// only on the service's own host, fetched with the service's scheme.
    pub fn image_url(&self, href: &str) -> Option<String> {
        let (scheme, host) = split_origin(&self.url)?;
        let (_, href_host) = split_origin(href)?;
        if !href_host.eq_ignore_ascii_case(host) {
            return None;
        }
        let path = &href[href.find("://")? + 3 + href_host.len()..];
        Some(format!("{scheme}://{host}{path}"))
    }
}

/// The user's sources in order, then USGS: what "Get aerial image" tries.
pub fn candidates(sources: &[ImagerySource]) -> impl Iterator<Item = ImagerySource> + '_ {
    sources
        .iter()
        .filter(|source| source.enabled)
        .cloned()
        .chain(std::iter::once(ImagerySource::usgs()))
}

/// The first source that covers `bounds`.
pub fn choose(sources: &[ImagerySource], bounds: GeoBounds) -> Option<ImagerySource> {
    candidates(sources).find(|source| source.covers(bounds))
}

pub fn load(saved: Option<String>) -> Vec<ImagerySource> {
    saved
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save(sources: &[ImagerySource]) -> String {
    serde_json::to_string(sources).unwrap_or_else(|_| "[]".into())
}

fn split_origin(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then_some((scheme, host))
}

/// Reduces whatever link a GIS site offers to the service's REST address:
/// the service page, its export operation, one of its layers, or the WMS
/// endpoint ArcGIS publishes beside it.
pub fn normalize_url(input: &str) -> Result<(String, ServiceKind), String> {
    let trimmed = input.trim();
    let without_query = trimmed.split(['?', '#']).next().unwrap_or_default();
    let (scheme, _) = split_origin(without_query)
        .filter(|(scheme, _)| matches!(*scheme, "http" | "https"))
        .ok_or("Paste a web address starting with https://")?;
    let lower = without_query.to_ascii_lowercase();
    let (marker, kind) = [
        ("/mapserver", ServiceKind::MapServer),
        ("/imageserver", ServiceKind::ImageServer),
    ]
    .into_iter()
    .find_map(|(marker, kind)| lower.find(marker).map(|at| (at + marker.len(), kind)))
    .ok_or("Not an ArcGIS MapServer or ImageServer link")?;
    let mut url = without_query[..marker].to_owned();
    // ArcGIS serves WMS at /arcgis/services/…; the REST API is beside it.
    if !lower.contains("/rest/services/")
        && let Some(at) = lower.find("/services/")
    {
        url.replace_range(at..at + "/services/".len(), "/rest/services/");
    }
    debug_assert!(url.starts_with(scheme));
    Ok((url, kind))
}

/// Builds a source from the description a service returns for `?f=json`.
pub fn parse_service(
    url: &str,
    kind: ServiceKind,
    json: &serde_json::Value,
) -> Result<ImagerySource, String> {
    if let Some(error) = json.get("error") {
        let message = error
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("unknown error");
        return Err(format!("The service refused: {message}"));
    }
    let text = |key: &str| {
        json.get(key)
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let extent_key = match kind {
        ServiceKind::MapServer => "fullExtent",
        ServiceKind::ImageServer => "extent",
    };
    let extent = json
        .get(extent_key)
        .ok_or("This link does not describe an imagery service")?;
    let document = |key: &str| {
        json.get("documentInfo")
            .and_then(|info| info.get(key))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    // Some titles are the publisher's project file path, not a name.
    let title = document("Title").filter(|title| {
        !title.contains(['\\', '/']) && !title.ends_with(".aprx") && !title.ends_with(".mxd")
    });
    let generic = |name: &&str| !matches!(*name, "Layers" | "Map" | "Layer");
    let path_name = url
        .split("/services/")
        .nth(1)
        .and_then(|path| path.rsplit_once('/'))
        .map(|(path, _)| path.to_owned());
    let name = text("mapName")
        .filter(generic)
        .or(title)
        .or(text("name"))
        .map(str::to_owned)
        .or(path_name)
        .unwrap_or_else(|| url.to_owned());
    // Publishers' names often carry doubled spaces.
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let size = |key: &str| {
        json.get(key)
            .and_then(|value| value.as_u64())
            .map(|value| value as u32)
    };
    let max_image_pixels = size("maxImageWidth")
        .zip(size("maxImageHeight"))
        .map(|(width, height)| width.min(height))
        .unwrap_or(default_max_image())
        .max(64);
    let tiles_only =
        text("capabilities").is_some_and(|capabilities| capabilities.contains("TilesOnly"));
    let tiles = if tiles_only {
        Some(
            tile_scheme(json)
                .ok_or("This service only serves tiles, in a layout this app cannot read")?,
        )
    } else {
        None
    };
    let attribution = text("copyrightText")
        .or(document("Credits"))
        .or(document("credits"))
        .or(document("Author"))
        .or(document("author"))
        .unwrap_or_default();
    Ok(ImagerySource {
        name,
        url: url.to_owned(),
        kind,
        coverage: geographic_extent(extent),
        attribution: attribution.to_owned(),
        max_image_pixels,
        tiles,
        enabled: true,
        unknown: BTreeMap::new(),
    })
}

/// A Web Mercator tile cache, from a service description's `tileInfo`.
fn tile_scheme(json: &serde_json::Value) -> Option<TileScheme> {
    let info = json.get("tileInfo")?;
    let number = |value: &serde_json::Value, key: &str| value.get(key)?.as_f64();
    let size = number(info, "rows")?;
    if number(info, "cols")? != size || !(16.0..=2048.0).contains(&size) {
        return None;
    }
    let reference = info.get("spatialReference")?;
    let wkid = reference
        .get("latestWkid")
        .or_else(|| reference.get("wkid"))?
        .as_u64()?;
    if !is_web_mercator(wkid) {
        return None;
    }
    let origin = info.get("origin")?;
    let levels = info
        .get("lods")?
        .as_array()?
        .iter()
        .filter_map(|lod| {
            let resolution = number(lod, "resolution")?;
            (resolution.is_finite() && resolution > 0.0)
                .then_some((number(lod, "level")? as u32, resolution))
        })
        .collect::<Vec<_>>();
    if levels.is_empty() {
        return None;
    }
    Some(TileScheme {
        size: size as u32,
        origin: [number(origin, "x")?, number(origin, "y")?],
        levels,
    })
}

fn is_web_mercator(wkid: u64) -> bool {
    matches!(wkid, 3857 | 102100 | 102113 | 900913)
}

/// An ArcGIS extent in longitude/latitude, when it is Web Mercator or WGS 84.
fn geographic_extent(extent: &serde_json::Value) -> Option<GeoBounds> {
    let value = |key: &str| extent.get(key)?.as_f64().filter(|v| v.is_finite());
    let reference = extent.get("spatialReference")?;
    let wkid = reference
        .get("latestWkid")
        .or_else(|| reference.get("wkid"))?
        .as_u64()?;
    let [xmin, ymin, xmax, ymax] = [
        value("xmin")?,
        value("ymin")?,
        value("xmax")?,
        value("ymax")?,
    ];
    let bounds = match wkid {
        wkid if is_web_mercator(wkid) => from_web_mercator([xmin, ymin, xmax, ymax]),
        4326 => GeoBounds {
            west: xmin,
            south: ymin,
            east: xmax,
            north: ymax,
        },
        _ => return None,
    };
    bounds.valid().then_some(bounds)
}

/// Asks the service at `link` to describe itself. Blocking; run off the UI
/// thread.
pub fn probe(link: &str) -> Result<ImagerySource, String> {
    probe_with(link, Duration::from_secs(20))
}

pub fn probe_with(link: &str, timeout: Duration) -> Result<ImagerySource, String> {
    let (url, kind) = normalize_url(link)?;
    let agent = agent(timeout);
    let mut response = agent
        .get(&format!("{url}?f=json"))
        .call()
        .map_err(|error| format!("Could not reach the service: {error}"))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|error| error.to_string())?;
    let json = serde_json::from_str(&body)
        .map_err(|_| "The service did not answer with a description".to_owned())?;
    parse_service(&url, kind, &json)
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into()
}

fn web_mercator(bounds: GeoBounds) -> [f64; 4] {
    let x = |longitude: f64| RADIUS * longitude.to_radians();
    let y = |latitude: f64| {
        RADIUS
            * (std::f64::consts::FRAC_PI_4 + latitude.to_radians() / 2.0)
                .tan()
                .ln()
    };
    [
        x(bounds.west),
        y(bounds.south),
        x(bounds.east),
        y(bounds.north),
    ]
}

/// [`FINEST_METERS_PER_PIXEL`] in Web Mercator units at this latitude, where
/// map metres are stretched by 1 / cos(latitude).
fn finest_mercator_resolution(mercator: [f64; 4]) -> f64 {
    let middle = (mercator[1] + mercator[3]) / 2.0;
    let latitude = 2.0 * (middle / RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2;
    FINEST_METERS_PER_PIXEL / latitude.cos().max(0.01)
}

fn from_web_mercator([west, south, east, north]: [f64; 4]) -> GeoBounds {
    let longitude = |x: f64| (x / RADIUS).to_degrees();
    let latitude =
        |y: f64| (2.0 * (y / RADIUS).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    GeoBounds {
        west: longitude(west),
        south: latitude(south),
        east: longitude(east),
        north: latitude(north),
    }
}

/// An image of an area as a source delivered it, ready to cache.
pub struct Fetched {
    /// JPEG or PNG.
    pub bytes: Vec<u8>,
    /// The area the image actually shows, which a service may round.
    pub extent: GeoBounds,
    /// What was requested, for the workspace's record.
    pub request: String,
}

/// An image of `bounds` at most about `max_pixels` across. Blocking.
pub fn fetch(
    source: &ImagerySource,
    bounds: GeoBounds,
    max_pixels: u32,
    timeout: Duration,
) -> Result<Fetched, String> {
    let agent = agent(timeout);
    let mercator = web_mercator(bounds);
    match &source.tiles {
        Some(scheme) => fetch_tiles(source, scheme, mercator, max_pixels, &agent),
        None => fetch_export(source, mercator, max_pixels, &agent),
    }
}

fn read_limited(
    response: &mut ureq::http::Response<ureq::Body>,
    limit: u64,
) -> Result<Vec<u8>, String> {
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

fn fetch_export(
    source: &ImagerySource,
    mercator: [f64; 4],
    max_pixels: u32,
    agent: &ureq::Agent,
) -> Result<Fetched, String> {
    let name = &source.name;
    let request = source.export_url(mercator, max_pixels);
    let mut response = agent
        .get(&request)
        .call()
        .map_err(|e| format!("{name} imagery request: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&read_limited(&mut response, 1 << 20)?)
        .map_err(|_| format!("{name} answered with something other than an image"))?;
    let href = json.get("href").and_then(|v| v.as_str()).ok_or_else(|| {
        format!(
            "{name} returned no image: {}",
            json.get("error").unwrap_or(&json)
        )
    })?;
    // Do not follow arbitrary URLs from an external service response.
    let href = source
        .image_url(href)
        .ok_or_else(|| format!("{name} returned an image on another host"))?;
    let extent = json
        .get("extent")
        .ok_or_else(|| format!("{name} omitted the actual image extent"))?;
    let coordinate = |key: &str| {
        extent
            .get(key)
            .and_then(|v| v.as_f64())
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("Invalid {name} extent {key}"))
    };
    let extent = from_web_mercator([
        coordinate("xmin")?,
        coordinate("ymin")?,
        coordinate("xmax")?,
        coordinate("ymax")?,
    ]);
    if !extent.valid() {
        return Err(format!("{name} returned an invalid image extent"));
    }
    let mut response = agent.get(&href).call().map_err(|e| e.to_string())?;
    let bytes = read_limited(&mut response, 64 << 20)?;
    Ok(Fetched {
        bytes,
        extent,
        request,
    })
}

/// The zoom level, tile span (metres), and tile range [first column, first
/// row, last column, last row] covering `mercator` with about `max_pixels`
/// across, never finer than any orthoimagery.
fn tile_range(
    scheme: &TileScheme,
    [west, south, east, north]: [f64; 4],
    max_pixels: u32,
) -> (u32, f64, [i64; 4]) {
    let longest = (east - west).max(north - south);
    let wanted = (longest / f64::from(max_pixels.max(1)))
        .max(finest_mercator_resolution([west, south, east, north]));
    let (level, resolution) = scheme
        .levels
        .iter()
        .copied()
        .filter(|(_, resolution)| *resolution >= wanted * 0.999)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .or_else(|| {
            scheme
                .levels
                .iter()
                .copied()
                .max_by(|a, b| a.1.total_cmp(&b.1))
        })
        .unwrap_or((0, 1.0));
    let span = resolution * f64::from(scheme.size);
    let [x0, y0] = scheme.origin;
    let column = |x: f64| ((x - x0) / span).floor() as i64;
    let row = |y: f64| ((y0 - y) / span).floor() as i64;
    (
        level,
        span,
        [column(west), row(north), column(east), row(south)],
    )
}

fn fetch_tiles(
    source: &ImagerySource,
    scheme: &TileScheme,
    mercator: [f64; 4],
    max_pixels: u32,
    agent: &ureq::Agent,
) -> Result<Fetched, String> {
    let name = &source.name;
    let (level, span, [first_column, first_row, last_column, last_row]) =
        tile_range(scheme, mercator, max_pixels);
    let columns = (last_column - first_column + 1).max(0) as usize;
    let rows = (last_row - first_row + 1).max(0) as usize;
    if columns * rows == 0 || columns * rows > MAX_TILES {
        return Err(format!("{name} would need {} tiles here", columns * rows));
    }
    let size = scheme.size;
    let jobs = std::sync::Mutex::new(
        (first_row..=last_row)
            .flat_map(|row| (first_column..=last_column).map(move |column| (row, column)))
            .collect::<Vec<_>>()
            .into_iter(),
    );
    let tiles = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..TILE_WORKERS {
            scope.spawn(|| {
                while let Some((row, column)) = jobs.lock().ok().and_then(|mut jobs| jobs.next()) {
                    let url = format!("{}/tile/{level}/{row}/{column}", source.url);
                    // A missing tile is left black: services omit tiles
                    // outside their imagery.
                    let tile = agent
                        .get(&url)
                        .call()
                        .ok()
                        .and_then(|mut response| read_limited(&mut response, 4 << 20).ok())
                        .and_then(|bytes| image::load_from_memory(&bytes).ok());
                    if let (Some(tile), Ok(mut tiles)) = (tile, tiles.lock()) {
                        tiles.push((row, column, tile.to_rgb8()));
                    }
                }
            });
        }
    });
    let tiles = tiles.into_inner().map_err(|_| "Tile download failed")?;
    if tiles.is_empty() {
        return Err(format!("{name} has no tiles here"));
    }
    let mut mosaic = image::RgbImage::new(columns as u32 * size, rows as u32 * size);
    for (row, column, tile) in tiles {
        let x = (column - first_column) * i64::from(size);
        let y = (row - first_row) * i64::from(size);
        image::imageops::replace(&mut mosaic, &tile, x, y);
    }
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
        .encode_image(&mosaic)
        .map_err(|e| e.to_string())?;
    let [x0, y0] = scheme.origin;
    let extent = from_web_mercator([
        x0 + first_column as f64 * span,
        y0 - (last_row + 1) as f64 * span,
        x0 + (last_column + 1) as f64 * span,
        y0 - first_row as f64 * span,
    ]);
    Ok(Fetched {
        bytes,
        extent,
        request: format!("{}/tile/{level}/{{row}}/{{column}}", source.url),
    })
}

/// A small image of `bounds` from `source`, to check it has imagery there.
pub fn sample_image(
    source: &ImagerySource,
    bounds: GeoBounds,
    timeout: Duration,
) -> Result<image::RgbImage, String> {
    let fetched = fetch(source, bounds, 64, timeout)?;
    image::load_from_memory(&fetched.bytes)
        .map(|image| image.to_rgb8())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn links_from_gis_sites_reduce_to_the_rest_service() {
        let rest = "https://orthos.its.ny.gov/arcgis/rest/services/wms/Latest/MapServer";
        for link in [
            rest.to_owned(),
            format!("  {rest}?f=pjson  "),
            format!("{rest}/export?bbox=1,2,3,4"),
            format!("{rest}/0"),
            "https://orthos.its.ny.gov/arcgis/services/wms/Latest/MapServer/WMSServer?request=GetCapabilities&service=WMS".into(),
        ] {
            assert_eq!(
                normalize_url(&link),
                Ok((rest.to_owned(), ServiceKind::MapServer)),
                "{link}"
            );
        }
        assert_eq!(
            normalize_url("https://example.gov/arcgis/rest/services/Ortho/ImageServer/exportImage"),
            Ok((
                "https://example.gov/arcgis/rest/services/Ortho/ImageServer".into(),
                ServiceKind::ImageServer
            ))
        );
        assert!(normalize_url("https://example.gov/wms?service=WMS").is_err());
        assert!(normalize_url("ftp://example.gov/arcgis/rest/services/A/MapServer").is_err());
    }

    #[test]
    fn a_map_service_description_becomes_a_source() {
        let url = "https://orthos.its.ny.gov/arcgis/rest/services/wms/Latest/MapServer";
        let source = parse_service(
            url,
            ServiceKind::MapServer,
            &json!({
                "mapName": "Layers",
                "documentInfo": {"Title": ""},
                "copyrightText": "NYS ITS Geospatial Services",
                "maxImageWidth": 4096,
                "maxImageHeight": 4096,
                "fullExtent": {
                    "xmin": -8883203.59, "ymin": 4937225.31,
                    "xmax": -7997063.45, "ymax": 5627154.89,
                    "spatialReference": {"wkid": 102100, "latestWkid": 3857}
                }
            }),
        )
        .unwrap();

        assert_eq!(source.name, "wms/Latest");
        assert_eq!(source.attribution, "NYS ITS Geospatial Services");
        assert_eq!(source.max_image_pixels, 4096);
        let coverage = source.coverage.unwrap();
        assert!((coverage.west + 79.8).abs() < 0.1 && (coverage.north - 45.0).abs() < 0.1);
        assert!(source.covers(GeoBounds {
            west: -77.8,
            south: 42.9,
            east: -77.7,
            north: 43.0
        }));
        assert!(!source.covers(GeoBounds {
            west: -90.0,
            south: 42.9,
            east: -89.9,
            north: 43.0
        }));
    }

    #[test]
    fn service_errors_and_other_projections_are_reported_plainly() {
        let url = "https://example.gov/arcgis/rest/services/A/ImageServer";
        let refused = parse_service(
            url,
            ServiceKind::ImageServer,
            &json!({"error": {"code": 499, "message": "Token Required"}}),
        );
        assert_eq!(refused, Err("The service refused: Token Required".into()));

        let state_plane = parse_service(
            url,
            ServiceKind::ImageServer,
            &json!({
                "name": "Ortho_2023",
                "extent": {"xmin": 1, "ymin": 2, "xmax": 3, "ymax": 4,
                           "spatialReference": {"wkid": 2261}}
            }),
        )
        .unwrap();
        assert_eq!(state_plane.name, "Ortho_2023");
        assert_eq!(state_plane.coverage, None);
        assert_eq!(state_plane.max_image_pixels, 2048);
    }

    fn lorain_style_tile_layer() -> serde_json::Value {
        json!({
            "mapName": "2025 Spring Aerials",
            "capabilities": "Map,TilesOnly",
            "copyrightText": "",
            "documentInfo": {
                "Title": "R:\\GIS\\2025 Spring Aerials.aprx",
                "author": "LorainCoGIS"
            },
            "fullExtent": {
                "xmin": -9167173.07, "ymin": 5021569.78,
                "xmax": -9113578.34, "ymax": 5090229.23,
                "spatialReference": {"wkid": 102100, "latestWkid": 3857}
            },
            "tileInfo": {
                "rows": 256, "cols": 256,
                "origin": {"x": -20037508.342787, "y": 20037508.342787},
                "spatialReference": {"wkid": 102100, "latestWkid": 3857},
                "lods": [
                    {"level": 18, "resolution": 0.597164283559817},
                    {"level": 19, "resolution": 0.298582141647617},
                    {"level": 20, "resolution": 0.14929107082380833},
                    {"level": 21, "resolution": 0.07464553541190416}
                ]
            }
        })
    }

    #[test]
    fn tile_only_layers_are_read_with_readable_names_and_credits() {
        let url = "https://tiles.example/arcgis/rest/services/2025_Spring_Aerial/MapServer";
        let source =
            parse_service(url, ServiceKind::MapServer, &lorain_style_tile_layer()).unwrap();

        assert_eq!(source.name, "2025 Spring Aerials");
        assert_eq!(source.attribution, "LorainCoGIS");
        let tiles = source.tiles.unwrap();
        assert_eq!(tiles.size, 256);
        assert_eq!(tiles.levels.len(), 4);

        // A tile-only layer whose tiles this app cannot use is refused.
        let mut state_plane = lorain_style_tile_layer();
        state_plane["tileInfo"]["spatialReference"] = json!({"wkid": 2261});
        assert!(parse_service(url, ServiceKind::MapServer, &state_plane).is_err());
    }

    #[test]
    fn tiles_are_chosen_at_the_needed_detail() {
        let url = "https://tiles.example/arcgis/rest/services/A/MapServer";
        let tiles = parse_service(url, ServiceKind::MapServer, &lorain_style_tile_layer())
            .unwrap()
            .tiles
            .unwrap();
        let course = web_mercator(GeoBounds {
            west: -82.1395,
            south: 41.4597,
            east: -82.1305,
            north: 41.4647,
        });
        // About 750 m (1000 Web Mercator metres at 41.5°N) across in 4096
        // px needs 0.24 map metres per pixel: level 19.
        let (level, span, [c0, r0, c1, r1]) = tile_range(&tiles, course, MAX_IMAGE_PIXELS);
        assert_eq!(level, 19);
        assert!((span - 0.298582141647617 * 256.0).abs() < 1e-9);
        let (columns, rows) = (c1 - c0 + 1, r1 - r0 + 1);
        assert!(
            (14..=15).contains(&columns) && (10..=12).contains(&rows),
            "{columns}x{rows}"
        );
        // 15 cm on the ground is 0.2 map metres here, so a small course
        // stops at level 19 rather than asking level 20 for finer detail.
        let small = [course[0], course[1], course[0] + 200.0, course[1] + 200.0];
        assert_eq!(tile_range(&tiles, small, MAX_IMAGE_PIXELS).0, 19);
        // The tiles cover the course.
        let [x0, y0] = tiles.origin;
        assert!(x0 + c0 as f64 * span <= course[0] && x0 + (c1 + 1) as f64 * span >= course[2]);
        assert!(y0 - r0 as f64 * span >= course[3] && y0 - (r1 + 1) as f64 * span <= course[1]);
        // A small sample uses the coarsest level on offer.
        assert_eq!(tile_range(&tiles, course, 64).0, 18);
    }

    #[test]
    fn user_sources_come_first_and_usgs_is_the_fallback() {
        let course = GeoBounds {
            west: -76.9,
            south: 42.7,
            east: -76.8,
            north: 42.8,
        };
        let mut state = ImagerySource::usgs();
        state.url = "https://state.example/arcgis/rest/services/Ortho/MapServer".into();
        state.name = "State".into();
        assert_eq!(choose(&[state.clone()], course).unwrap().name, "State");
        state.enabled = false;
        assert!(choose(&[state.clone()], course).unwrap().is_usgs());
        state.enabled = true;
        state.coverage = Some(GeoBounds {
            west: -80.0,
            south: 30.0,
            east: -79.0,
            north: 31.0,
        });
        assert!(choose(&[state], course).unwrap().is_usgs());
        let europe = GeoBounds {
            west: 2.0,
            south: 48.0,
            east: 2.1,
            north: 48.1,
        };
        assert_eq!(choose(&[], europe), None);
    }

    #[test]
    fn requests_follow_the_service_limit_and_stay_on_its_host() {
        let mut source = ImagerySource::usgs();
        source.url = "https://orthos.example/arcgis/rest/services/wms/Latest/MapServer".into();
        source.max_image_pixels = 4096;
        // 1 km wide, 500 m tall: 4096 px is coarser than the finest imagery.
        let request = source.export_url([0.0, 0.0, 1000.0, 500.0], MAX_IMAGE_PIXELS);
        assert!(request.contains("/MapServer/export?"), "{request}");
        assert!(request.contains("size=4096,2048"), "{request}");
        // 150 m: no point asking for more than 1000 px.
        assert!(
            source
                .export_url([0.0, 0.0, 150.0, 150.0], MAX_IMAGE_PIXELS)
                .contains("size=1000,1000")
        );

        assert_eq!(
            source.image_url("http://orthos.example/arcgis/rest/directories/out/a.jpg"),
            Some("https://orthos.example/arcgis/rest/directories/out/a.jpg".into())
        );
        assert_eq!(source.image_url("https://elsewhere.example/a.jpg"), None);
        assert_ne!(source.cache_tag(), ImagerySource::usgs().cache_tag());
        assert_eq!(ImagerySource::usgs().cache_tag(), "usgs");
    }

    #[test]
    fn saved_sources_round_trip_and_keep_unknown_fields() {
        let mut source = ImagerySource::usgs();
        source.url = "https://state.example/arcgis/rest/services/Ortho/MapServer".into();
        source
            .unknown
            .insert("future".into(), serde_json::Value::Bool(true));
        let loaded = load(Some(save(std::slice::from_ref(&source))));
        assert_eq!(loaded, vec![source]);
        assert!(load(Some("not json".into())).is_empty());
        assert!(load(None).is_empty());
    }
}
