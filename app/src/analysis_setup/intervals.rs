//! Interval editing: numeric trim fields plus a draggable timeline bar.

use super::super::workflow::replace_segments_preserving_identity;
use super::super::*;
use super::SetupOutcome;
use crate::ui_kit::{theme::text, widgets};
use eframe::egui::RichText;

/// Draggable two-handle timeline. Returns true while an endpoint is dragged.
pub(in crate::analysis_app) fn interval_bar(
    ui: &mut egui::Ui,
    start: &mut f64,
    end: &mut f64,
    bounds: (f64, f64),
) -> bool {
    if !bounds.0.is_finite() || !bounds.1.is_finite() || bounds.1 <= bounds.0 {
        return false;
    }
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(80.0), 26.0),
        egui::Sense::drag(),
    );
    let x = |t: f64| {
        rect.left() + ((t - bounds.0) / (bounds.1 - bounds.0)).clamp(0.0, 1.0) as f32 * rect.width()
    };
    let accent = ui.visuals().selection.stroke.color;
    let painter = ui.painter();
    painter.line_segment(
        [rect.left_center(), rect.right_center()],
        egui::Stroke::new(3.0, ui.visuals().widgets.inactive.bg_fill),
    );
    painter.line_segment(
        [
            egui::pos2(x(*start), rect.center().y),
            egui::pos2(x(*end), rect.center().y),
        ],
        egui::Stroke::new(4.0, accent),
    );
    for t in [*start, *end] {
        painter.circle_filled(egui::pos2(x(t), rect.center().y), 6.5, accent);
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if response.drag_started()
        && let Some(p) = response.interact_pointer_pos()
    {
        ui.data_mut(|d| {
            d.insert_temp(
                response.id,
                (p.x - x(*start)).abs() <= (p.x - x(*end)).abs(),
            )
        });
    }
    if response.dragged()
        && let Some(p) = response.interact_pointer_pos()
    {
        let t = bounds.0
            + ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64 * (bounds.1 - bounds.0);
        if ui
            .data_mut(|d| d.get_temp::<bool>(response.id))
            .unwrap_or(true)
        {
            *start = t.min(*end - 0.001);
        } else {
            *end = t.max(*start + 0.001);
        }
        return true;
    }
    false
}

impl AnalysisApp {
    pub(super) fn setup_intervals(&mut self, ui: &mut egui::Ui, outcome: &mut SetupOutcome) {
        widgets::hint(
            ui,
            "Intervals use the recording clock, not raw camera time. Video panels show exported-video time separately.",
        );
        ui.add_space(4.0);
        for recording in &mut self.workspace.recordings {
            let offset = recording
                .sources
                .iter()
                .find(|source| source.id == recording.primary_source)
                .map_or(0.0, |source| source.alignment.offset_seconds);
            let extent = self.data.get(&recording.primary_source).map(|data| {
                data.raw.channels.values().fold(
                    (f64::INFINITY, f64::NEG_INFINITY),
                    |(low, high), channel| {
                        let samples = &channel.series.samples;
                        (
                            low.min(samples.first().map_or(low, |s| s.time - offset)),
                            high.max(samples.last().map_or(high, |s| s.time - offset)),
                        )
                    },
                )
            });
            ui.push_id(recording.id.0, |ui| {
                widgets::card(ui, false, |ui| {
                    ui.label(
                        RichText::new(recording.name.clone())
                            .strong()
                            .color(text::strong()),
                    );
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button("Detect from data")
                            .on_hover_text(
                                "Replaces this recording's intervals with logger laps or automatic motion detection.",
                            )
                            .clicked()
                            && let Some(data) = self.data.get(&recording.primary_source)
                        {
                            let gps = gps_points(&data.raw, recording);
                            let detected = auto_segments(&data.raw, recording, &gps);
                            replace_segments_preserving_identity(recording, detected);
                            outcome.changed = true;
                            outcome.intervals_replaced = true;
                        }
                        if ui.button("Add manual interval").clicked() {
                            recording.segments.push(RunSegment {
                                id: SegmentId::new(),
                                name: "Manual interval".into(),
                                start_recording_time: 0.0,
                                end_recording_time: 60.0,
                                kind: SegmentKind::Unknown,
                                estimated: true,
                                competitive: false,
                                unknown: Default::default(),
                            });
                            outcome.changed = true;
                        }
                    });
                    for segment in &mut recording.segments {
                        ui.push_id(segment.id.0, |ui| {
                            ui.separator();
                            interval_editor(ui, segment, extent, outcome);
                        });
                    }
                });
            });
            ui.add_space(6.0);
        }
    }
}

fn interval_editor(
    ui: &mut egui::Ui,
    segment: &mut RunSegment,
    extent: Option<(f64, f64)>,
    outcome: &mut SetupOutcome,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(&segment.name).color(text::strong()));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            outcome.changed |= ui
                .checkbox(&mut segment.competitive, "Competitive")
                .on_hover_text(
                    "Competitive intervals can be the reference and are compared by default",
                )
                .changed();
        });
    });
    let mut start = segment.start_recording_time;
    let mut end = segment.end_recording_time;
    let mut edited = false;
    ui.horizontal(|ui| {
        edited |= ui
            .add(
                egui::DragValue::new(&mut start)
                    .speed(0.05)
                    .prefix("Start ")
                    .suffix(" s"),
            )
            .changed();
        edited |= ui
            .add(
                egui::DragValue::new(&mut end)
                    .speed(0.05)
                    .prefix("Finish ")
                    .suffix(" s"),
            )
            .changed();
    });
    if let Some(bounds) = extent {
        edited |= interval_bar(ui, &mut start, &mut end, bounds);
    }
    if edited && start.is_finite() && end.is_finite() && end > start {
        segment.start_recording_time = start;
        segment.end_recording_time = end;
        segment.estimated = true;
        outcome.changed = true;
    }
}
