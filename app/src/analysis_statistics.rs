//! Compact statistics tables with panel-scoped, persisted column widths.

use super::*;
use crate::analysis_maps::sample_value;
use crate::ui_kit::{
    theme::{self, text},
    widgets,
};
use eframe::egui::RichText;

const HEADERS: [&str; 5] = ["Channel", "Now", "Min", "Max", "Mean"];

fn default_widths(available: f32) -> [f32; 5] {
    [
        available * 0.32,
        available * 0.17,
        available * 0.17,
        available * 0.17,
        available * 0.17,
    ]
}

fn table_row(
    ui: &mut egui::Ui,
    widths: &[f32; 5],
    values: [String; 5],
    header: bool,
) -> egui::Rect {
    let height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(widths.iter().sum(), height),
        egui::Sense::hover(),
    );
    let mut left = rect.left();
    for (column, (width, value)) in widths.iter().zip(values).enumerate() {
        let cell =
            egui::Rect::from_min_size(egui::pos2(left, rect.top()), egui::vec2(*width, height));
        left += width;
        if *width <= 4.0 {
            continue;
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt((response.id, column))
                .max_rect(cell.shrink2(egui::vec2(2.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.set_clip_rect(ui.clip_rect().intersect(cell));
        let label = if header {
            RichText::new(&value).small().color(text::weak())
        } else if column == 0 {
            RichText::new(&value).color(text::normal())
        } else {
            RichText::new(&value).monospace().color(text::normal())
        };
        child
            .add(egui::Label::new(label).truncate())
            .on_hover_text(value);
    }
    rect
}

fn headers(ui: &mut egui::Ui, widths: &mut [f32; 5], defaults: [f32; 5]) {
    let rect = table_row(ui, widths, HEADERS.map(str::to_owned), true);
    let mut right = rect.left();
    for column in 0..5 {
        right += widths[column];
        let id = ui.id().with(("column-width", column));
        let handle = egui::Rect::from_center_size(
            egui::pos2(right, rect.center().y),
            egui::vec2(6.0, rect.height()),
        );
        let response = ui
            .interact(handle, id, egui::Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text("Drag to resize this column; double-click to reset it");
        ui.painter().line_segment(
            [
                egui::pos2(right, rect.top()),
                egui::pos2(right, rect.bottom()),
            ],
            egui::Stroke::new(
                1.0,
                if response.hovered() || response.dragged() {
                    theme::accent()
                } else {
                    theme::surface::border()
                },
            ),
        );
        if response.drag_started() {
            ui.data_mut(|data| data.insert_temp(id, widths[column]));
        }
        if response.dragged() {
            let start = ui
                .data(|data| data.get_temp::<f32>(id))
                .unwrap_or(widths[column]);
            widths[column] = (start + response.total_drag_delta().unwrap_or_default().x).max(0.0);
            ui.ctx().request_repaint();
        }
        if response.double_clicked() {
            widths[column] = defaults[column];
        }
    }
}

/// Fewer decimals for large magnitudes keeps the statistics grid narrow.
fn stat_number(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude >= 1000.0 {
        format!("{value:.0}")
    } else if magnitude >= 100.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

impl AnalysisApp {
    pub(super) fn statistics(&mut self, ui: &mut egui::Ui, id: u64, units: UnitSystem) {
        widgets::hint(
            ui,
            "Values at the shared playhead; min, max, and mean cover the stats range (or the whole run).",
        );
        let options = PlotOptions::default();
        let channels = [
            "gps_speed",
            "rpm",
            "water_temperature",
            "exhaust_temperature",
            "gps_lateral_acceleration",
            "gps_inline_acceleration",
        ];
        let defaults = default_widths((ui.available_width() - 36.0).max(0.0));
        let mut widths = self
            .state
            .stats_columns
            .get(&id)
            .copied()
            .unwrap_or(defaults);
        for (width, default) in widths.iter_mut().zip(defaults) {
            if !width.is_finite() || *width < 0.0 {
                *width = default;
            }
        }
        let before = widths;
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for run in &self.prepared.runs {
                    if !self.state.selection.contains(&run.key) {
                        continue;
                    }
                    let color = run_color(
                        &self.state.selection,
                        self.workspace.reference.as_ref(),
                        &run.key,
                    );
                    widgets::card(ui, false, |ui| {
                        ui.horizontal(|ui| {
                            widgets::dot(ui, color);
                            ui.label(RichText::new(&run.name).strong().color(text::strong()));
                        });
                        let range = self
                            .state
                            .range
                            .and_then(|range| {
                                let reference = self.reference_run()?;
                                let a = self.x_at_time(reference, reference.start + range[0])?;
                                let b = self.x_at_time(reference, reference.start + range[1])?;
                                Some((self.time_at_x(run, a)?, self.time_at_x(run, b)?))
                            })
                            .or_else(|| self.state.range.is_none().then_some((run.start, run.end)));
                        let Some(range) = range else {
                            widgets::hint(
                                ui,
                                "The selected range is outside this run's matched coverage.",
                            );
                            return;
                        };
                        ui.push_id(("stats", run.key.segment_id.0), |ui| {
                            headers(ui, &mut widths, defaults);
                            for name in channels {
                                let Some((channel, off)) = self.source_channel(run, name, &options)
                                else {
                                    continue;
                                };
                                let target = display_unit(
                                    channel.descriptor.unit.clone(),
                                    channel.descriptor.quantity.clone(),
                                    units,
                                );
                                let convert = |v| {
                                    Unit::convert_value(v, &channel.descriptor.unit, &target)
                                        .unwrap_or(v)
                                };
                                let value = self
                                    .run_time(run)
                                    .and_then(|t| {
                                        sample_value(
                                            &channel.series.samples,
                                            t + off,
                                            channel.descriptor.interpolation,
                                            channel.series.gap_seconds,
                                        )
                                    })
                                    .map(convert);
                                let cache_id = ui.id().with((
                                    self.prepared_revision,
                                    run.key.segment_id.0,
                                    channel.descriptor.id.0,
                                    range.0.to_bits(),
                                    range.1.to_bits(),
                                    format!("{target:?}"),
                                ));
                                let (n, min, max, sum) = ui
                                    .data_mut(|d| d.get_temp::<(u64, f64, f64, f64)>(cache_id))
                                    .unwrap_or_else(|| {
                                        let samples = &channel.series.samples;
                                        let start =
                                            samples.partition_point(|s| s.time - off < range.0);
                                        let end =
                                            samples.partition_point(|s| s.time - off <= range.1);
                                        let stats = samples[start..end.max(start)]
                                            .iter()
                                            .map(|s| convert(s.value))
                                            .filter(|v| v.is_finite())
                                            .fold(
                                                (0u64, f64::INFINITY, f64::NEG_INFINITY, 0.0),
                                                |(n, min, max, sum), v| {
                                                    (n + 1, min.min(v), max.max(v), sum + v)
                                                },
                                            );
                                        ui.data_mut(|d| d.insert_temp(cache_id, stats));
                                        stats
                                    });
                                let values = if n > 0 {
                                    [
                                        stat_number(min),
                                        stat_number(max),
                                        stat_number(sum / n as f64),
                                    ]
                                } else {
                                    ["—".into(), "—".into(), "—".into()]
                                };
                                table_row(
                                    ui,
                                    &widths,
                                    [
                                        format!("{name} ({})", target.symbol()),
                                        value.map_or("—".into(), stat_number),
                                        values[0].clone(),
                                        values[1].clone(),
                                        values[2].clone(),
                                    ],
                                    false,
                                );
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        if before != widths {
            self.state.stats_columns.insert(id, widths);
            self.dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_a_divider_changes_only_its_column_and_can_collapse_it() {
        let ctx = egui::Context::default();
        let defaults = [100.0, 50.0, 50.0, 50.0, 50.0];
        let mut widths = defaults;
        let mut origin = egui::Pos2::ZERO;
        let mut frame = |events| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 90.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(raw, |ui| {
                origin = ui.cursor().min;
                headers(ui, &mut widths, defaults);
            });
            output.textures_delta.clear();
            (origin, widths)
        };
        let (origin, _) = frame(vec![]);
        let start = origin + egui::vec2(100.0, 10.0);
        frame(vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        let (_, grown) = frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(40.0, 0.0),
        )]);
        assert_eq!(grown, [140.0, 50.0, 50.0, 50.0, 50.0]);
        let (_, grown) = frame(vec![egui::Event::PointerMoved(
            start + egui::vec2(80.0, 0.0),
        )]);
        assert_eq!(grown, [180.0, 50.0, 50.0, 50.0, 50.0]);
        frame(vec![egui::Event::PointerMoved(
            start - egui::vec2(110.0, 0.0),
        )]);
        assert_eq!(widths, [0.0, 50.0, 50.0, 50.0, 50.0]);
    }
}
