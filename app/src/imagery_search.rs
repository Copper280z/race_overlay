//! Finds public aerial imagery services near a course.
//!
//! Esri's ArcGIS Online catalog lists services that state, county, and city
//! GIS offices have registered, and can be searched anonymously by area. The
//! catalog is only a lead: its extents are often wrong, many entries are
//! dead, need a sign-in, or are not photographs, and publishers rarely
//! register every year they fly. Every candidate is therefore asked to
//! describe itself and to draw a small image of the course, only services
//! that return real imagery there are offered, and the server folders of
//! those that do are listed for unregistered siblings (other years).

use crate::analysis_imagery::GeoBounds;
use crate::imagery_sources::{self, ImagerySource};
use std::{
    collections::HashSet,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

const CATALOG: &str = "https://www.arcgis.com/sharing/rest/search";
const QUERY: &str = "(ortho OR orthoimagery OR orthophoto OR orthophotography OR aerial OR imagery) \
    AND (type:\"Map Service\" OR type:\"Image Service\") \
    -owner:esri -owner:esri_imagery -owner:esri_basemaps";
/// The search area is rounded outward to this many degrees, so the catalog
/// learns roughly where the course is, not exactly.
const AREA_GRID_DEGREES: f64 = 0.1;
const CATALOG_PAGES: usize = 3;
/// Catalog services checked per search. Plausible ones are checked first.
const MAX_CHECKED: usize = 40;
/// Unregistered services checked per server folder that had a match.
const MAX_SIBLINGS: usize = 60;
const WORKERS: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(10);

/// Hosts whose imagery may not be downloaded this way (Esri's commercial
/// basemaps) or that proxy a service behind a sign-in.
const BLOCKED_HOSTS: [&str; 4] = [
    "arcgisonline.com",
    "maptiles.arcgis.com",
    "tiledbasemaps.arcgis.com",
    "utility.arcgis.com",
];
/// Title words of layers that are not natural-colour aerial photographs.
const NOT_PHOTOS: [&str; 16] = [
    "infrared",
    "label",
    "reference",
    "boundar",
    "transportation",
    "topo",
    "hillshade",
    "elevation",
    "parcel",
    "contour",
    "index",
    "footprint",
    "ndvi",
    "satellite",
    "hybrid",
    "avoidance",
];

/// Services verified to draw imagery of the course, most local first.
pub struct Found {
    pub sources: Vec<ImagerySource>,
    /// Candidates checked but not offered.
    pub hidden: usize,
}

/// How far a running search has got, for a progress line.
#[derive(Default)]
pub struct Progress {
    pub checked: AtomicUsize,
    pub total: AtomicUsize,
}

#[derive(Debug, PartialEq)]
struct Candidate {
    url: String,
    kind: imagery_sources::ServiceKind,
    /// How the publisher titled it in the catalog.
    title: String,
    /// The catalog's extent, when it is plausible (not the whole world).
    extent: Option<GeoBounds>,
}

/// Searches the catalog near `course` and verifies the results. Blocking;
/// run off the UI thread.
pub fn find(course: GeoBounds, progress: &Progress) -> Result<Found, String> {
    let area = rounded_out(course);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut items = Vec::new();
    let mut start = 1;
    for _ in 0..CATALOG_PAGES {
        let bbox = format!("{},{},{},{}", area.west, area.south, area.east, area.north);
        let start_text = start.to_string();
        let mut response = agent
            .get(CATALOG)
            .query("f", "json")
            .query("num", "100")
            .query("start", &start_text)
            .query("q", QUERY)
            .query("bbox", &bbox)
            .call()
            .map_err(|error| format!("Could not search the ArcGIS Online catalog: {error}"))?;
        let page: serde_json::Value = serde_json::from_str(
            &response
                .body_mut()
                .with_config()
                .limit(8 * 1024 * 1024)
                .read_to_string()
                .map_err(|error| error.to_string())?,
        )
        .map_err(|_| "The catalog answered with something other than results".to_owned())?;
        if let Some(results) = page.get("results").and_then(|results| results.as_array()) {
            items.extend(results.iter().cloned());
        }
        match page.get("nextStart").and_then(|next| next.as_i64()) {
            Some(next) if next > 0 => start = next,
            _ => break,
        }
    }
    let mut seen = HashSet::new();
    let catalog = candidates(&items, course, &mut seen);
    progress.total.store(catalog.len(), Ordering::Relaxed);
    let mut sources = verify_all(catalog, course, progress);
    let mut folders = sources
        .iter()
        .filter_map(|source| folder_of(&source.url))
        .collect::<Vec<_>>();
    folders.sort();
    folders.dedup();
    let mut siblings = Vec::new();
    for (root, folder) in folders {
        let listing = agent
            .get(&format!("{root}/{folder}"))
            .query("f", "json")
            .call()
            .ok()
            .and_then(|mut response| {
                response
                    .body_mut()
                    .with_config()
                    .limit(4 * 1024 * 1024)
                    .read_to_string()
                    .ok()
            })
            .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok());
        if let Some(listing) = listing {
            siblings.extend(sibling_candidates(&root, &listing, &mut seen));
        }
    }
    progress.total.fetch_add(siblings.len(), Ordering::Relaxed);
    sources.extend(verify_all(siblings, course, progress));
    rank(&mut sources);
    let hidden = progress.total.load(Ordering::Relaxed) - sources.len();
    Ok(Found { sources, hidden })
}

/// Verifies candidates in parallel, keeping those with imagery of `course`.
fn verify_all(
    candidates: Vec<Candidate>,
    course: GeoBounds,
    progress: &Progress,
) -> Vec<ImagerySource> {
    let queue = Mutex::new(candidates.into_iter());
    let verified = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..WORKERS {
            scope.spawn(|| {
                while let Some(candidate) = queue.lock().ok().and_then(|mut queue| queue.next()) {
                    if let Ok(source) = verify(&candidate, course)
                        && let Ok(mut verified) = verified.lock()
                    {
                        verified.push(source);
                    }
                    progress.checked.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    verified.into_inner().unwrap_or_default()
}

/// The REST root (`…/rest/services`) and folder holding a service, as in
/// `…/rest/services/wms/Latest/MapServer` → (`…/rest/services`, `wms`).
fn folder_of(url: &str) -> Option<(String, String)> {
    let at = url.to_ascii_lowercase().find("/rest/services/")? + "/rest/services".len();
    let (root, path) = url.split_at(at);
    let path = path.trim_start_matches('/');
    // Drop the type and the service name; what remains is the folder.
    let (service, _) = path.rsplit_once('/')?;
    let folder = service.rsplit_once('/').map_or("", |(folder, _)| folder);
    Some((root.to_owned(), folder.to_owned()))
}

/// Services listed in a server folder that the search has not seen yet.
fn sibling_candidates(
    root: &str,
    listing: &serde_json::Value,
    seen: &mut HashSet<String>,
) -> Vec<Candidate> {
    listing
        .get("services")
        .and_then(|services| services.as_array())
        .into_iter()
        .flatten()
        .filter_map(|service| {
            let name = service.get("name")?.as_str()?;
            let kind = service.get("type")?.as_str()?;
            let link = format!("{root}/{name}/{kind}");
            let title = name.rsplit('/').next().unwrap_or(name).replace('_', " ");
            usable(&title, &link, seen)
        })
        .take(MAX_SIBLINGS)
        .collect()
}

/// A candidate for a link and title, unless it is unusable or already seen.
fn usable(title: &str, link: &str, seen: &mut HashSet<String>) -> Option<Candidate> {
    let lower = title.to_ascii_lowercase();
    if NOT_PHOTOS.iter().any(|word| lower.contains(word)) {
        return None;
    }
    let (url, kind) = imagery_sources::normalize_url(link).ok()?;
    let lower_url = url.to_ascii_lowercase();
    let host = lower_url.split("://").nth(1)?.split('/').next()?.to_owned();
    let infrared = lower_url.contains("_cir/") || lower_url.contains("/cir/");
    if infrared
        || BLOCKED_HOSTS.iter().any(|blocked| host.ends_with(blocked))
        || lower_url == ImagerySource::usgs().url.to_ascii_lowercase()
        || !seen.insert(lower_url)
    {
        return None;
    }
    Some(Candidate {
        url,
        kind,
        title: title.trim().to_owned(),
        extent: None,
    })
}

fn rounded_out(bounds: GeoBounds) -> GeoBounds {
    let down = |value: f64| (value / AREA_GRID_DEGREES).floor() * AREA_GRID_DEGREES;
    let up = |value: f64| (value / AREA_GRID_DEGREES).ceil() * AREA_GRID_DEGREES;
    let round = |value: f64| (value * 10.0).round() / 10.0;
    GeoBounds {
        west: round(down(bounds.west)),
        south: round(down(bounds.south)),
        east: round(up(bounds.east)),
        north: round(up(bounds.north)),
    }
}

/// Catalog items worth checking: usable hosts, photo-like titles, one entry
/// per service. Those whose plausible extent covers the course come first,
/// most local first; items with an implausible extent follow in catalog
/// order; items whose plausible extent misses the course are dropped.
fn candidates(
    items: &[serde_json::Value],
    course: GeoBounds,
    seen: &mut HashSet<String>,
) -> Vec<Candidate> {
    let mut local = Vec::new();
    let mut unknown = Vec::new();
    for item in items {
        let text = |key: &str| item.get(key).and_then(|value| value.as_str()).unwrap_or("");
        let extent = item
            .get("extent")
            .and_then(|extent| serde_json::from_value::<[[f64; 2]; 2]>(extent.clone()).ok())
            .map(|[[west, south], [east, north]]| GeoBounds {
                west,
                south,
                east,
                north,
            })
            .filter(|extent| extent.valid() && area(*extent) < 2_000.0);
        // Out-of-area items are skipped before they are marked as seen, so a
        // later entry for the same service with a better extent still counts.
        if extent.is_some_and(|extent| !covers(extent, course)) {
            continue;
        }
        let Some(mut candidate) = usable(text("title"), text("url"), seen) else {
            continue;
        };
        candidate.extent = extent;
        if extent.is_some() {
            local.push(candidate);
        } else {
            unknown.push(candidate);
        }
    }
    local.sort_by(|a, b| {
        let size = |candidate: &Candidate| candidate.extent.map_or(f64::MAX, area);
        size(a).total_cmp(&size(b))
    });
    local.into_iter().chain(unknown).take(MAX_CHECKED).collect()
}

fn area(bounds: GeoBounds) -> f64 {
    (bounds.east - bounds.west) * (bounds.north - bounds.south)
}

fn covers(outer: GeoBounds, inner: GeoBounds) -> bool {
    outer.west <= inner.west
        && outer.east >= inner.east
        && outer.south <= inner.south
        && outer.north >= inner.north
}

/// Describes the service and has it draw a small image of the course: only
/// a service that answers with an image that is not blank is offered.
fn verify(candidate: &Candidate, course: GeoBounds) -> Result<ImagerySource, String> {
    let mut source = imagery_sources::probe_with(&candidate.url, TIMEOUT)?;
    // A name without spaces is a server identifier ("Map5",
    // "2024_Spring_Aerials"); the catalog title was written for people.
    if !source.name.contains(' ') && !candidate.title.is_empty() {
        source.name = candidate.title.clone();
    }
    if source.kind != candidate.kind || !source.covers(course) {
        return Err("does not cover the course".into());
    }
    let sample = imagery_sources::sample_image(&source, course, TIMEOUT)?;
    if !has_detail(&sample) {
        return Err("blank image".into());
    }
    Ok(source)
}

/// Whether an image has any variation: services draw a flat colour, or
/// nothing, where they have no imagery.
fn has_detail(image: &image::RgbImage) -> bool {
    let lumas = image
        .pixels()
        .map(|pixel| {
            f64::from(pixel[0]) * 0.3 + f64::from(pixel[1]) * 0.59 + f64::from(pixel[2]) * 0.11
        })
        .collect::<Vec<_>>();
    let count = lumas.len().max(1) as f64;
    let mean = lumas.iter().sum::<f64>() / count;
    let variance = lumas.iter().map(|luma| (luma - mean).powi(2)).sum::<f64>() / count;
    variance.sqrt() > 4.0
}

/// Most local first (local, county, statewide, national); within a class,
/// the most recent year named. Classes are coarse on purpose: a state that
/// flies one region a year publishes services of very different sizes, and
/// the newest statewide mosaic should not lose to an old partial flight.
fn rank(sources: &mut [ImagerySource]) {
    let class = |source: &ImagerySource| match coverage_label(source) {
        "local" => 0,
        "county" => 1,
        "statewide" => 2,
        "national" => 3,
        _ => 4,
    };
    sources.sort_by(|a, b| {
        class(a)
            .cmp(&class(b))
            .then_with(|| year(b).cmp(&year(a)))
            .then_with(|| a.name.cmp(&b.name))
    });
}

/// The latest year a name mentions; "latest" or "current" counts as newest.
fn year(source: &ImagerySource) -> u32 {
    let name = source.name.to_ascii_lowercase();
    if name.contains("latest") || name.contains("current") {
        return u32::MAX;
    }
    name.split(|c: char| !c.is_ascii_digit())
        .filter_map(|digits| digits.parse::<u32>().ok())
        .filter(|year| (1930..=2100).contains(year))
        .max()
        .unwrap_or(0)
}

/// A rough word for how much ground a source covers.
pub fn coverage_label(source: &ImagerySource) -> &'static str {
    let Some(coverage) = source.coverage else {
        return "coverage unknown";
    };
    // Square kilometres, near enough at mid latitudes.
    let middle = ((coverage.south + coverage.north) / 2.0).to_radians().cos();
    let km2 = area(coverage) * 111.32 * 111.32 * middle;
    match km2 {
        k if k >= 1_000_000.0 => "national",
        k if k >= 20_000.0 => "statewide",
        k if k >= 500.0 => "county",
        _ => "local",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const COURSE: GeoBounds = GeoBounds {
        west: -82.14,
        south: 41.46,
        east: -82.13,
        north: 41.47,
    };

    fn item(title: &str, url: &str, extent: [[f64; 2]; 2]) -> serde_json::Value {
        json!({"title": title, "url": url, "extent": extent})
    }

    #[test]
    fn the_catalog_sees_only_a_rounded_area() {
        let area = rounded_out(COURSE);
        assert_eq!(
            (area.west, area.south, area.east, area.north),
            (-82.2, 41.4, -82.1, 41.5)
        );
    }

    #[test]
    fn candidates_skip_unusable_entries_and_put_local_ones_first() {
        let state = [[-85.0, 38.0], [-80.0, 42.0]];
        let county = [[-82.4, 41.1], [-81.9, 41.6]];
        let elsewhere = [[-90.0, 30.0], [-89.0, 31.0]];
        let world = [[-180.0, -90.0], [180.0, 90.0]];
        let items = [
            item(
                "World Imagery",
                "https://services.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer",
                world,
            ),
            item(
                "Statewide 2023",
                "https://state.example/arcgis/rest/services/Ortho2023/ImageServer",
                state,
            ),
            item(
                "Same service again",
                "https://state.example/arcgis/rest/services/Ortho2023/ImageServer/",
                state,
            ),
            item(
                "County aerials",
                "https://county.example/arcgis/rest/services/Aerial/MapServer",
                county,
            ),
            item(
                "Tile index",
                "https://county.example/arcgis/rest/services/Index/MapServer",
                county,
            ),
            item(
                "Proxied NAIP",
                "https://utility.arcgis.com/usrsvcs/servers/abc/rest/services/NAIP/ImageServer",
                state,
            ),
            item(
                "Elsewhere",
                "https://far.example/arcgis/rest/services/Ortho/MapServer",
                elsewhere,
            ),
            item(
                "Bogus extent",
                "https://odd.example/arcgis/rest/services/Ortho/ImageServer",
                world,
            ),
            item("Not a service", "https://example.com/page.html", county),
        ];

        let urls = candidates(&items, COURSE, &mut HashSet::new())
            .into_iter()
            .map(|candidate| candidate.url)
            .collect::<Vec<_>>();

        assert_eq!(
            urls,
            [
                "https://county.example/arcgis/rest/services/Aerial/MapServer",
                "https://state.example/arcgis/rest/services/Ortho2023/ImageServer",
                "https://odd.example/arcgis/rest/services/Ortho/ImageServer",
            ]
        );
    }

    #[test]
    fn unregistered_siblings_are_found_in_the_same_folder() {
        assert_eq!(
            folder_of("https://orthos.example/arcgis/rest/services/wms/Latest/MapServer"),
            Some((
                "https://orthos.example/arcgis/rest/services".into(),
                "wms".into()
            ))
        );
        assert_eq!(
            folder_of("https://tiles.example/tiles/org/arcgis/rest/services/2025_Aerial/MapServer"),
            Some((
                "https://tiles.example/tiles/org/arcgis/rest/services".into(),
                String::new()
            ))
        );
        let listing = json!({"services": [
            {"name": "wms/2023", "type": "MapServer"},
            {"name": "wms/2023_cir", "type": "MapServer"},
            {"name": "wms/Latest", "type": "MapServer"},
            {"name": "wms/Index_Grid", "type": "MapServer"},
            {"name": "wms/Ortho", "type": "GeometryServer"}
        ]});
        let root = "https://orthos.example/arcgis/rest/services";
        // Already seen through the catalog, under different capitalisation.
        let mut seen = HashSet::from([
            "https://orthos.example/arcgis/rest/services/wms/latest/mapserver".to_owned(),
        ]);

        let siblings = sibling_candidates(root, &listing, &mut seen);

        let urls = siblings
            .iter()
            .map(|candidate| candidate.url.as_str())
            .collect::<Vec<_>>();
        assert_eq!(urls, [format!("{root}/wms/2023/MapServer")]);
        assert_eq!(siblings[0].title, "2023");
    }

    #[test]
    fn blank_samples_are_rejected() {
        let flat = image::RgbImage::from_pixel(64, 64, image::Rgb([255, 255, 255]));
        assert!(!has_detail(&flat));
        let photo = image::RgbImage::from_fn(64, 64, |x, y| {
            image::Rgb([(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8])
        });
        assert!(has_detail(&photo));
    }

    #[test]
    fn local_and_recent_sources_rank_first() {
        let source = |name: &str, coverage: GeoBounds| {
            let mut source = ImagerySource::usgs();
            source.name = name.into();
            source.coverage = Some(coverage);
            source
        };
        let state = GeoBounds {
            west: -85.0,
            south: 38.0,
            east: -80.0,
            north: 42.0,
        };
        let county = GeoBounds {
            west: -82.4,
            south: 41.1,
            east: -81.9,
            north: 41.6,
        };
        // A state that flies one region a year: the old regional flight is
        // smaller than the statewide mosaic but no more local in kind.
        let region = GeoBounds {
            west: -84.0,
            south: 39.5,
            east: -81.0,
            north: 41.5,
        };
        let mut sources = vec![
            source("Ohio 2019", state),
            source("Ohio 2015 central region", region),
            source("Ohio latest", state),
            source("County 2022", county),
            source("Ohio 2023", state),
        ];
        rank(&mut sources);
        let names = sources
            .iter()
            .map(|source| source.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "County 2022",
                "Ohio latest",
                "Ohio 2023",
                "Ohio 2019",
                "Ohio 2015 central region"
            ]
        );
        assert_eq!(coverage_label(&sources[0]), "county");
        assert_eq!(coverage_label(&sources[1]), "statewide");
    }
}
