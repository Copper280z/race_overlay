//! Telemetry plot preparation and rendering controls.
use super::OverlayEditor;
use crate::ui_kit::{theme::text, widgets};
use eframe::egui::{self, RichText};
use overlay_core::{Unit, zero_phase_low_pass};

impl OverlayEditor {
    pub(in crate::race_app) fn data_plot_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Telemetry graph")
                    .strong()
                    .size(15.0)
                    .color(text::strong()),
            );
            ui.add(
                egui::Slider::new(&mut self.plot_editor.plot_window_seconds, 1.0..=120.0)
                    .logarithmic(true)
                    .text("Window")
                    .suffix(" s"),
            );
            ui.checkbox(&mut self.plot_editor.plot_filter_enabled, "Smooth")
                .on_hover_text(
                    "Zero-phase, non-causal preview smoothing. It does not modify source data or widgets; the cutoff is the final two-pass -3 dB point.",
                );
            if self.plot_editor.plot_filter_enabled {
                ui.add(
                    egui::Slider::new(&mut self.plot_editor.plot_filter_hz, 0.5..=50.0)
                        .logarithmic(true)
                        .suffix(" Hz"),
                );
            }
            ui.checkbox(&mut self.plot_editor.plot_robust_scale, "Ignore extremes")
                .on_hover_text("Ignore the most extreme 1% of values when scaling the vertical axis");
        });
        let mut time = self.session.current_time();
        ui.horizontal(|ui| {
            ui.monospace(format!(
                "{} / {}",
                widgets::clock(time),
                widgets::clock(self.duration())
            ));
            ui.spacing_mut().slider_width = ui.available_width() - 12.0;
            if ui
                .add(egui::Slider::new(&mut time, 0.0..=self.duration()).show_value(false))
                .changed()
            {
                self.session.set_current_time(time);
                self.stop_playback();
                self.request_preview();
                self.refresh_overlay();
            }
        });
        let half = self.plot_editor.plot_window_seconds * 0.5;
        let start = (self.session.current_time() - half).max(0.0);
        let end = (start + self.plot_editor.plot_window_seconds).min(self.duration());
        let offsets = self.source_offsets();
        let mut plotted: Vec<(String, Vec<(f64, f64)>)> = Vec::new();
        for reference in &self.plot_editor.plot_channels {
            let Some(dataset) = self
                .session
                .datasets()
                .iter()
                .find(|dataset| dataset.source_id == reference.source_id)
            else {
                continue;
            };
            let Some(channel) = dataset.channel(reference.channel_id) else {
                continue;
            };
            let offset = offsets.get(&reference.source_id).copied().unwrap_or(0.0);
            let source_start = start + offset;
            let source_end = end + offset;
            let first = channel
                .series
                .samples
                .partition_point(|sample| sample.time < source_start);
            let last = channel
                .series
                .samples
                .partition_point(|sample| sample.time <= source_end);
            let slice = &channel.series.samples[first..last];
            if slice.is_empty() {
                continue;
            }
            let mut values = slice.iter().map(|sample| sample.value).collect::<Vec<_>>();
            if self.plot_editor.plot_filter_enabled && slice.len() > 2 {
                let elapsed = slice.last().unwrap().time - slice.first().unwrap().time;
                let rate = (slice.len() - 1) as f64 / elapsed.max(1e-9);
                values = zero_phase_low_pass(&values, rate, self.plot_editor.plot_filter_hz);
            }
            let display_unit = Unit::default_for(&channel.descriptor.quantity, self.unit_system())
                .filter(|unit| channel.descriptor.unit.is_compatible_with(unit))
                .unwrap_or_else(|| channel.descriptor.unit.clone());
            if display_unit != channel.descriptor.unit {
                values.iter_mut().for_each(|value| {
                    if let Ok(converted) = channel
                        .descriptor
                        .unit
                        .convert_value_to(*value, &display_unit)
                    {
                        *value = converted;
                    }
                });
            }
            let stride = (slice.len() / 2_000).max(1);
            let points = slice
                .iter()
                .zip(values)
                .step_by(stride)
                .map(|(sample, value)| (sample.time - offset, value))
                .collect();
            plotted.push((
                format!("{} ({})", channel.descriptor.name, display_unit.symbol()),
                points,
            ));
        }
        if plotted.is_empty() {
            widgets::empty_state(
                ui,
                "No channels to plot",
                "Tick channels under Data sources, Plot channels on the left.",
                |_| {},
            );
            return;
        }
        let colors = [
            egui::Color32::from_rgb(0, 218, 255),
            egui::Color32::from_rgb(255, 174, 0),
            egui::Color32::from_rgb(255, 80, 135),
            egui::Color32::from_rgb(95, 225, 105),
            egui::Color32::from_rgb(180, 130, 255),
            egui::Color32::from_rgb(255, 245, 100),
        ];
        ui.horizontal_wrapped(|ui| {
            for (index, (name, points)) in plotted.iter().enumerate() {
                let min = points
                    .iter()
                    .map(|point| point.1)
                    .fold(f64::INFINITY, f64::min);
                let max = points
                    .iter()
                    .map(|point| point.1)
                    .fold(f64::NEG_INFINITY, f64::max);
                ui.colored_label(
                    colors[index % colors.len()],
                    format!("{name}: {min:.3}…{max:.3}"),
                );
            }
        });
        let mut range_values = plotted
            .iter()
            .flat_map(|(_, points)| points.iter().map(|point| point.1))
            .filter(|value| value.is_finite())
            .collect::<Vec<_>>();
        range_values.sort_by(f64::total_cmp);
        let (mut y_min, mut y_max) =
            if self.plot_editor.plot_robust_scale && range_values.len() >= 100 {
                (
                    range_values[range_values.len() / 100],
                    range_values[range_values.len() * 99 / 100],
                )
            } else {
                (
                    *range_values.first().unwrap(),
                    *range_values.last().unwrap(),
                )
            };
        if (y_max - y_min).abs() < 1e-12 {
            y_min -= 1.0;
            y_max += 1.0;
        } else {
            let margin = (y_max - y_min) * 0.08;
            y_min -= margin;
            y_max += margin;
        }
        let desired = egui::vec2(ui.available_width(), ui.available_height().max(260.0));
        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 4.0, egui::Color32::from_rgb(8, 12, 18));
        for index in 0..=5 {
            let fraction = index as f32 / 5.0;
            let y = egui::lerp(rect.bottom()..=rect.top(), fraction);
            ui.painter().line_segment(
                [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
                egui::Stroke::new(1.0, egui::Color32::from_gray(45)),
            );
            let value = y_min + (y_max - y_min) * fraction as f64;
            ui.painter().text(
                egui::pos2(rect.left() + 4.0, y - 2.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{value:.2}"),
                egui::FontId::monospace(11.0),
                egui::Color32::GRAY,
            );
        }
        for index in 0..=10 {
            let x = egui::lerp(rect.left()..=rect.right(), index as f32 / 10.0);
            ui.painter().line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.0, egui::Color32::from_gray(32)),
            );
        }
        for (index, (_, points)) in plotted.iter().enumerate() {
            let screen = points
                .iter()
                .filter(|point| point.1.is_finite())
                .map(|point| {
                    let x = egui::remap_clamp(
                        point.0 as f32,
                        start as f32..=end as f32,
                        rect.left()..=rect.right(),
                    );
                    let y = egui::remap_clamp(
                        point.1 as f32,
                        y_min as f32..=y_max as f32,
                        rect.bottom()..=rect.top(),
                    );
                    egui::pos2(x, y)
                })
                .collect::<Vec<_>>();
            ui.painter().add(egui::Shape::line(
                screen,
                egui::Stroke::new(1.6, colors[index % colors.len()]),
            ));
        }
        let playhead = egui::remap_clamp(
            self.session.current_time() as f32,
            start as f32..=end as f32,
            rect.left()..=rect.right(),
        );
        ui.painter().line_segment(
            [
                egui::pos2(playhead, rect.top()),
                egui::pos2(playhead, rect.bottom()),
            ],
            egui::Stroke::new(2.0, text::strong()),
        );
    }
}
