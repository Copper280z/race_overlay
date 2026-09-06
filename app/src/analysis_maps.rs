use crate::analysis_imagery::{GeoBounds, ImageryConfig, ImageryController};
use egui::{Color32, Pos2};
use overlay_core::{
    GpsPoint, Interpolation, ProgressSample, ReferenceCourse, SegmentRef, TimedSample, Unit,
    progress_at_time, time_at_progress,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct MapTrace {
    pub segment: SegmentRef,
    pub name: String,
    pub color: Color32,
    pub gps: Vec<GpsPoint>,
    pub progress: Vec<ProgressSample>,
    pub values: Vec<TimedSample>,
    pub interpolation: Interpolation,
    pub gap_seconds: Option<f64>,
    pub cursor_time: Option<f64>,
}
pub struct MapSelection {
    pub segment: SegmentRef,
    pub recording_time: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MapSettings {
    pub actual_gps: bool,
    pub small_multiples: bool,
    pub rotation_degrees: f64,
    pub imagery: ImageryConfig,
    pub manual_scale: Option<[f64; 2]>,
    pub low_pass_hz: Option<f64>,
    pub display_unit: Option<Unit>,
    #[serde(flatten)]
    pub unknown: std::collections::BTreeMap<String, serde_json::Value>,
}
impl Default for MapSettings {
    fn default() -> Self {
        Self {
            actual_gps: false,
            small_multiples: true,
            rotation_degrees: 0.0,
            imagery: Default::default(),
            manual_scale: None,
            low_pass_hz: None,
            display_unit: None,
            unknown: Default::default(),
        }
    }
}
pub struct MapPanel {
    imagery: ImageryController,
    zoom: f32,
    pan: egui::Vec2,
}
impl Default for MapPanel {
    fn default() -> Self {
        Self {
            imagery: Default::default(),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
        }
    }
}

fn bounds(traces: &[MapTrace]) -> Option<GeoBounds> {
    let mut b = GeoBounds {
        west: f64::INFINITY,
        east: f64::NEG_INFINITY,
        south: f64::INFINITY,
        north: f64::NEG_INFINITY,
    };
    for p in traces.iter().flat_map(|t| &t.gps) {
        b.west = b.west.min(p.longitude);
        b.east = b.east.max(p.longitude);
        b.south = b.south.min(p.latitude);
        b.north = b.north.max(p.latitude);
    }
    b.valid().then_some(b)
}
fn course_bounds(course: &ReferenceCourse) -> Option<GeoBounds> {
    let mut b = GeoBounds {
        west: f64::INFINITY,
        east: f64::NEG_INFINITY,
        south: f64::INFINITY,
        north: f64::NEG_INFINITY,
    };
    for p in &course.points {
        b.west = b.west.min(p.longitude);
        b.east = b.east.max(p.longitude);
        b.south = b.south.min(p.latitude);
        b.north = b.north.max(p.latitude);
    }
    b.valid().then_some(b)
}
pub fn sample_value(
    values: &[TimedSample],
    t: f64,
    interpolation: Interpolation,
    gap: Option<f64>,
) -> Option<f64> {
    if !t.is_finite() {
        return None;
    }
    let i = values.partition_point(|v| v.time < t);
    if let Some(v) = values.get(i).filter(|v| (v.time - t).abs() < 1e-8) {
        return v.value.is_finite().then_some(v.value);
    }
    if i == 0 || i == values.len() {
        return None;
    }
    let a = values[i - 1];
    let b = values[i];
    let dt = b.time - a.time;
    if dt <= 0.0 || dt > gap.unwrap_or(2.0) || !a.value.is_finite() || !b.value.is_finite() {
        return None;
    }
    match interpolation {
        Interpolation::Hold => Some(a.value),
        Interpolation::Linear => Some(a.value + (b.value - a.value) * (t - a.time) / dt),
    }
}
fn course_point(course: &ReferenceCourse, s: f64) -> Option<(f64, f64)> {
    if course.points.len() < 2 || !s.is_finite() {
        return None;
    }
    let i = course
        .cumulative_meters
        .partition_point(|v| *v < s)
        .clamp(1, course.points.len() - 1);
    if !course.valid_segments.get(i - 1).copied().unwrap_or(false) {
        return None;
    }
    let a = course.cumulative_meters[i - 1];
    let b = course.cumulative_meters[i];
    if b <= a {
        return None;
    }
    let f = ((s - a) / (b - a)).clamp(0.0, 1.0);
    let p = course.points[i - 1];
    let q = course.points[i];
    Some((
        p.latitude + (q.latitude - p.latitude) * f,
        p.longitude + (q.longitude - p.longitude) * f,
    ))
}
fn color(value: f64, range: [f64; 2]) -> Color32 {
    let f = ((value - range[0]) / (range[1] - range[0]).max(1e-9)).clamp(0.0, 1.0) as f32;
    // Blue -> cyan -> yellow -> red: one fixed shared scale, never a
    // per-lap autoscale that would make unequal values look identical.
    let stops = [
        [50.0, 105.0, 235.0],
        [40.0, 215.0, 180.0],
        [245.0, 220.0, 55.0],
        [245.0, 65.0, 45.0],
    ];
    let i = ((f * 3.0).floor() as usize).min(2);
    let u = f * 3.0 - i as f32;
    let c = |axis: usize| (stops[i][axis] + (stops[i + 1][axis] - stops[i][axis]) * u) as u8;
    Color32::from_rgb(c(0), c(1), c(2))
}

fn automatic_color_range(traces: &[MapTrace]) -> Option<[f64; 2]> {
    let mut range = traces
        .iter()
        .flat_map(|trace| &trace.values)
        .filter(|sample| sample.value.is_finite())
        .fold([f64::INFINITY, f64::NEG_INFINITY], |range, sample| {
            [range[0].min(sample.value), range[1].max(sample.value)]
        });
    if !range.iter().all(|value| value.is_finite()) {
        return None;
    }
    if range[0] == range[1] {
        let padding = (range[0].abs() * 0.05).max(1.0);
        range = [range[0] - padding, range[1] + padding];
    }
    Some(range)
}

/// Screen-space normal pointing to the driver's left along the reference path.
/// Screen Y increases downward, hence `(dy, -dx)` rather than `(-dy, dx)`.
fn course_normal(points: &[Pos2], valid: &[bool], index: usize) -> egui::Vec2 {
    let incoming = (index > 0 && valid.get(index - 1) == Some(&true))
        .then(|| points[index] - points[index - 1])
        .filter(|v| v.length_sq() > 1e-6)
        .map(egui::Vec2::normalized);
    let outgoing = (index + 1 < points.len() && valid.get(index) == Some(&true))
        .then(|| points[index + 1] - points[index])
        .filter(|v| v.length_sq() > 1e-6)
        .map(egui::Vec2::normalized);
    let tangent = match (incoming, outgoing) {
        (Some(a), Some(b)) if (a + b).length_sq() > 1e-6 => (a + b).normalized(),
        (Some(a), _) => a,
        (_, Some(b)) => b,
        _ => egui::Vec2::X,
    };
    egui::vec2(tangent.y, -tangent.x)
}

fn lane_offsets(index: usize, count: usize, total_width: f32) -> (f32, f32, f32) {
    let lane_width = total_width / count.max(1) as f32;
    let outer = total_width / 2.0 - lane_width * index as f32;
    let inner = outer - lane_width;
    (outer, inner, (outer + inner) / 2.0)
}
impl MapPanel {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        traces: &[MapTrace],
        reference: Option<&ReferenceCourse>,
        channel_label: &str,
        config: &mut MapSettings,
        asset_dir: &Path,
    ) -> Option<MapSelection> {
        let geographic_bounds = bounds(traces);
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut config.small_multiples,
                if config.actual_gps {
                    "Side by side"
                } else {
                    "Split ribbon"
                },
            )
            .on_hover_text(if config.actual_gps {
                "Give every run its own GPS map"
            } else {
                "Split one course ribbon lengthwise, left to right in the comparison order"
            });
            ui.add(
                egui::DragValue::new(&mut config.rotation_degrees)
                    .speed(0.5)
                    .suffix("° rotation"),
            );
            if ui.button("Fit").clicked() {
                self.zoom = 1.0;
                self.pan = egui::Vec2::ZERO;
            }
        });
        if config.actual_gps {
            self.imagery
                .ui(ui, &mut config.imagery, geographic_bounds, asset_dir);
        }
        if traces.is_empty() {
            ui.label("Select laps or runs to display their GPS paths.");
            return None;
        }
        if !config.actual_gps && reference.is_none() {
            ui.label("Choose a reference with usable GPS for the simplified course map.");
            return None;
        }
        let map_bounds = if config.actual_gps {
            geographic_bounds
        } else {
            reference.and_then(course_bounds)
        };
        let Some(b) = map_bounds else {
            ui.label("No usable GPS for this selection.");
            return None;
        };
        let automatic_range = automatic_color_range(traces).unwrap_or([0.0, 1.0]);
        ui.horizontal_wrapped(|ui| {
            ui.label("Color range");
            let mut automatic = config.manual_scale.is_none();
            if ui
                .checkbox(&mut automatic, "Auto")
                .on_hover_text("Use the minimum and maximum across every displayed run")
                .changed()
            {
                config.manual_scale = (!automatic).then_some(automatic_range);
            }
            if let Some(range) = config.manual_scale.as_mut() {
                let speed = ((range[1] - range[0]).abs() / 200.0).max(0.01);
                ui.label("Min");
                ui.add(egui::DragValue::new(&mut range[0]).speed(speed));
                ui.label("Max");
                ui.add(egui::DragValue::new(&mut range[1]).speed(speed));
                if !range.iter().all(|value| value.is_finite()) || range[1] <= range[0] {
                    ui.colored_label(Color32::LIGHT_RED, "Maximum must exceed minimum");
                }
            }
        });
        let range = config
            .manual_scale
            .filter(|range| range.iter().all(|value| value.is_finite()) && range[1] > range[0])
            .unwrap_or(automatic_range);
        ui.horizontal(|ui| {
            ui.label(channel_label);
            if range.iter().all(|v| v.is_finite()) {
                ui.label(format!("{:.2}", range[0]));
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(100.0, 10.0), egui::Sense::hover());
                for i in 0..50 {
                    let x = i as f32 / 50.0;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(rect.left() + rect.width() * x, rect.top()),
                            egui::pos2(rect.left() + rect.width() * (x + 0.02), rect.bottom()),
                        ),
                        0.0,
                        color(range[0] + (range[1] - range[0]) * x as f64, range),
                    );
                }
                ui.label(format!("{:.2}", range[1]));
            }
        });
        let groups = if config.actual_gps && config.small_multiples {
            traces.len()
        } else {
            1
        };
        let columns = if groups > 1 { 2 } else { 1 };
        let rows = groups.div_ceil(columns);
        let full = ui.available_rect_before_wrap();
        let cell = egui::vec2(
            full.width() / columns as f32,
            (full.height() / rows as f32).max(120.0),
        );
        let mut selection = None;
        let center_lat = (b.north + b.south) / 2.0;
        let center_lon = (b.west + b.east) / 2.0;
        let cos = center_lat.to_radians().cos();
        let theta = config.rotation_degrees.to_radians();
        let rotate = |lat: f64, lon: f64| {
            let x = (lon - center_lon) * cos;
            let y = lat - center_lat;
            (
                (x * theta.cos() - y * theta.sin()) as f32,
                (x * theta.sin() + y * theta.cos()) as f32,
            )
        };
        let mut span = egui::Vec2::splat(1e-6);
        for (lat, lon) in [
            (b.south, b.west),
            (b.north, b.west),
            (b.south, b.east),
            (b.north, b.east),
        ] {
            let (x, y) = rotate(lat, lon);
            span.x = span.x.max(x.abs() * 2.0);
            span.y = span.y.max(y.abs() * 2.0);
        }
        for group in 0..groups {
            let origin = full.min
                + egui::vec2(
                    (group % columns) as f32 * cell.x,
                    (group / columns) as f32 * cell.y,
                );
            let rect = egui::Rect::from_min_size(origin, cell).shrink(4.0);
            let response = ui.interact(
                rect,
                ui.id().with(("map-view", group)),
                egui::Sense::click_and_drag(),
            );
            if response.dragged_by(egui::PointerButton::Secondary) {
                self.pan += response.drag_delta() / rect.size();
            }
            if response.hovered() {
                let wheel = ui.input(|i| i.smooth_scroll_delta.y);
                if wheel != 0.0 {
                    self.zoom = (self.zoom * (wheel * 0.002).exp()).clamp(0.25, 30.0);
                }
            }
            let scale =
                ((rect.width() - 24.0) / span.x).min((rect.height() - 45.0) / span.y) * self.zoom;
            let project = |lat: f64, lon: f64| {
                let (x, y) = rotate(lat, lon);
                rect.center() + self.pan * rect.size() + egui::vec2(x * scale, -y * scale)
            };
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
            if config.actual_gps {
                self.imagery.paint(&painter, &config.imagery, project);
            }
            let selected: &[MapTrace] = if config.actual_gps && config.small_multiples {
                &traces[group..group + 1]
            } else {
                traces
            };
            if !config.actual_gps && config.small_multiples {
                let course = reference.expect("reference checked above");
                let centerline = course
                    .points
                    .iter()
                    .map(|p| project(p.latitude, p.longitude))
                    .collect::<Vec<_>>();
                let normals = (0..centerline.len())
                    .map(|i| course_normal(&centerline, &course.valid_segments, i))
                    .collect::<Vec<_>>();
                let count = selected.len();
                let total_width = if count == 1 {
                    6.0
                } else {
                    (count as f32 * 4.5).clamp(9.0, 18.0)
                };
                let pointer = response.interact_pointer_pos();
                let mut nearest: Option<(f32, usize, f64)> = None;
                for (lane, trace) in selected.iter().enumerate() {
                    let (outer, inner, center) = lane_offsets(lane, count, total_width);
                    let lane_width = (outer - inner).abs();
                    let times = course
                        .cumulative_meters
                        .iter()
                        .map(|s| time_at_progress(&trace.progress, *s))
                        .collect::<Vec<_>>();
                    for i in 1..centerline.len() {
                        if course.valid_segments.get(i - 1) != Some(&true) {
                            continue;
                        }
                        let (Some(a_t), Some(b_t)) = (times[i - 1], times[i]) else {
                            continue;
                        };
                        if b_t <= a_t || b_t - a_t > 2.0 {
                            continue;
                        }
                        let fill = sample_value(
                            &trace.values,
                            (a_t + b_t) / 2.0,
                            trace.interpolation,
                            trace.gap_seconds,
                        )
                        .map(|v| color(v, range))
                        .unwrap_or(ui.visuals().weak_text_color());
                        let delta = centerline[i] - centerline[i - 1];
                        if delta.length_sq() <= 1e-6 {
                            continue;
                        }
                        // Use one normal for both ends of this short segment.
                        // Averaged vertex normals can make a quad self-intersect
                        // at a sharp/noisy GPS turn, causing egui's convex mesh
                        // tessellator to emit triangles across the whole panel.
                        let normal = egui::vec2(delta.y, -delta.x).normalized();
                        painter.line_segment(
                            [
                                centerline[i - 1] + normal * center,
                                centerline[i] + normal * center,
                            ],
                            egui::Stroke::new(lane_width + 0.35, fill),
                        );
                    }
                    if let Some(pointer) = pointer {
                        for ((pos, normal), time) in centerline.iter().zip(&normals).zip(&times) {
                            let Some(time) = time else { continue };
                            let distance = (*pos + *normal * center).distance_sq(pointer);
                            if nearest.is_none_or(|v| distance < v.0) {
                                nearest = Some((distance, lane, *time));
                            }
                        }
                    }
                    if let Some(time) = trace.cursor_time
                        && let Some(progress) = progress_at_time(&trace.progress, time)
                        && let Some((lat, lon)) = course_point(course, progress)
                    {
                        let i = course
                            .cumulative_meters
                            .partition_point(|s| *s < progress)
                            .min(normals.len().saturating_sub(1));
                        let pos = project(lat, lon) + normals[i] * center;
                        let radius = (total_width / count.max(1) as f32 * 0.38).clamp(2.5, 4.5);
                        painter.circle_filled(pos, radius, trace.color);
                        painter.circle_stroke(
                            pos,
                            radius + 1.5,
                            egui::Stroke::new(1.5, Color32::WHITE),
                        );
                    }
                    let side = match (lane, count) {
                        (0, 2) => "left",
                        (1, 2) => "right",
                        _ if count > 1 => "left → right",
                        _ => "full ribbon",
                    };
                    painter.text(
                        rect.left_top() + egui::vec2(6.0, 5.0 + lane as f32 * 17.0),
                        egui::Align2::LEFT_TOP,
                        format!("{} · {side}", trace.name),
                        egui::FontId::proportional(13.0),
                        trace.color,
                    );
                }
                if response.clicked()
                    && let Some((_, lane, time)) = nearest.filter(|v| v.0 < 900.0)
                {
                    selection = Some(MapSelection {
                        segment: selected[lane].segment.clone(),
                        recording_time: time,
                    });
                }
            } else {
                for trace in selected {
                    let mut previous: Option<(Pos2, f64)> = None;
                    let mut nearest: Option<(f32, f64)> = None;
                    for gps in &trace.gps {
                        let t = gps.recording_time;
                        let coordinate = if config.actual_gps {
                            Some((gps.latitude, gps.longitude))
                        } else {
                            progress_at_time(&trace.progress, t)
                                .and_then(|s| reference.and_then(|c| course_point(c, s)))
                        };
                        let Some((lat, lon)) = coordinate else {
                            previous = None;
                            continue;
                        };
                        let pos = project(lat, lon);
                        let value =
                            sample_value(&trace.values, t, trace.interpolation, trace.gap_seconds);
                        if let Some((last, last_t)) =
                            previous.filter(|(_, last_t)| t - *last_t <= 2.0 && t > *last_t)
                        {
                            let _ = last_t;
                            painter.line_segment(
                                [last, pos],
                                egui::Stroke::new(
                                    3.0,
                                    value
                                        .map(|v| color(v, range))
                                        .unwrap_or(ui.visuals().weak_text_color()),
                                ),
                            );
                        }
                        previous = Some((pos, t));
                        if let Some(pointer) = response.interact_pointer_pos() {
                            let d = pos.distance_sq(pointer);
                            if nearest.is_none_or(|v| d < v.0) {
                                nearest = Some((d, t));
                            }
                        }
                    }
                    if response.clicked()
                        && let Some((distance, t)) = nearest.filter(|v| v.0 < 900.0)
                    {
                        let _ = distance;
                        selection = Some(MapSelection {
                            segment: trace.segment.clone(),
                            recording_time: t,
                        });
                    }
                    if let Some(t) = trace.cursor_time {
                        let coordinate = if config.actual_gps {
                            let i = trace.gps.partition_point(|p| p.recording_time < t);
                            trace
                                .gps
                                .get(i)
                                .filter(|p| (p.recording_time - t).abs() < 0.25)
                                .map(|p| (p.latitude, p.longitude))
                        } else {
                            progress_at_time(&trace.progress, t)
                                .and_then(|s| reference.and_then(|c| course_point(c, s)))
                        };
                        if let Some((lat, lon)) = coordinate {
                            let pos = project(lat, lon);
                            painter.circle_filled(pos, 6.0, trace.color);
                            painter.circle_stroke(pos, 7.0, egui::Stroke::new(1.5, Color32::WHITE));
                        }
                    }
                }
            }
            if config.actual_gps && config.small_multiples {
                painter.text(
                    rect.left_top() + egui::vec2(6.0, 5.0),
                    egui::Align2::LEFT_TOP,
                    &traces[group].name,
                    egui::FontId::proportional(13.0),
                    traces[group].color,
                );
            }
        }
        ui.allocate_rect(full, egui::Sense::hover());
        selection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_ribbon_lanes_split_at_center_in_comparison_order() {
        assert_eq!(lane_offsets(0, 2, 10.0), (5.0, 0.0, 2.5));
        assert_eq!(lane_offsets(1, 2, 10.0), (0.0, -5.0, -2.5));
    }

    #[test]
    fn ribbon_left_is_relative_to_direction_of_travel() {
        // A northbound course travels upward on screen; its left is west.
        let points = [egui::pos2(10.0, 20.0), egui::pos2(10.0, 10.0)];
        let normal = course_normal(&points, &[true], 0);
        assert!(normal.x < -0.99);
        assert!(normal.y.abs() < 0.01);
    }

    #[test]
    fn automatic_range_expands_constant_values() {
        let trace = MapTrace {
            segment: SegmentRef {
                recording_id: overlay_core::RecordingId::new(),
                segment_id: overlay_core::SegmentId::new(),
            },
            name: String::new(),
            color: Color32::WHITE,
            gps: vec![],
            progress: vec![],
            values: vec![TimedSample {
                time: 0.0,
                value: 20.0,
            }],
            interpolation: Interpolation::Linear,
            gap_seconds: None,
            cursor_time: None,
        };
        assert_eq!(automatic_color_range(&[trace]), Some([19.0, 21.0]));
    }
}
