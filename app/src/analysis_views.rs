use super::*;
use crate::analysis_maps::{MapColorMode, MapTrace, color_map};
use crate::ui_kit::{
    Tone,
    theme::{self, text},
    widgets,
};
use eframe::egui::RichText;
use egui_plot::{Legend, Line, LineStyle, Plot, Points, VLine};
use overlay_media::PreviewSize;

/// Telemetry often starts a moment before the first video frame. Until the
/// video begins, hold its first frame rather than showing nothing. Returns the
/// video time to show and how long before the video starts the position is.
pub(super) fn hold_first_frame(video_time: f64) -> (f64, f64) {
    (video_time.max(0.0), (-video_time).max(0.0))
}

/// How long a panel keeps polling for a requested video frame before it
/// stops waiting; a dead decoder must not keep the UI repainting.
const FRAME_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

fn legend_editor(
    ui: &mut egui::Ui,
    show: &mut bool,
    labels: &mut BTreeMap<String, String>,
    entries: &[(String, String, egui::Color32)],
) {
    ui.checkbox(show, "Show legend");
    if *show {
        ui.collapsing("Legend text", |ui| {
            for (key, default, color) in entries {
                ui.horizontal(|ui| {
                    widgets::dot(ui, *color);
                    let label = labels.entry(key.clone()).or_insert_with(|| default.clone());
                    ui.add(egui::TextEdit::singleline(label).desired_width(220.0));
                    if ui.small_button("Reset").clicked() {
                        *label = default.clone();
                    }
                });
            }
        });
    }
}

/// "channel (unit)" heading for a plot.
fn plot_title(name: &str, traces: &[PlotTrace]) -> String {
    let display_name = if name == DELTA_CHANNEL {
        DELTA_LABEL
    } else {
        name
    };
    match traces.first().map(|t| t.unit.symbol()) {
        Some(unit) if !unit.is_empty() => format!("{display_name} ({unit})"),
        _ => display_name.to_owned(),
    }
}

fn compact_legend() -> Legend {
    Legend::default()
        .text_style(egui::TextStyle::Small)
        .background_alpha(0.55)
        .grouping(egui_plot::LegendGrouping::ById)
        // A trace dimmed outside the gates draws its measured part first.
        .color_conflict_handling(egui_plot::ColorConflictHandling::PickFirst)
}

fn trace_item_id(key: &str) -> egui::Id {
    egui::Id::new(("analysis-run", key))
}

fn legend_overflows(
    ui: &egui::Ui,
    entries: &[(String, String, egui::Color32)],
    labels: &BTreeMap<String, String>,
    height: f32,
) -> bool {
    let row_height = ui.text_style_height(&egui::TextStyle::Small) + ui.spacing().item_spacing.y;
    entries.len() as f32 * row_height + 16.0 > height * 0.65
        || entries.iter().any(|(key, name, _)| {
            let label = labels
                .get(key)
                .filter(|label| !label.trim().is_empty())
                .unwrap_or(name);
            widgets::text_width(ui, label, egui::TextStyle::Small) + 40.0
                > ui.available_width() * 0.65
        })
}

/// Keep large legends in one compact anchor inside the plot. The list scrolls
/// in a popover; its toggles use the plot's own visibility memory.
fn overflow_legend(
    ui: &mut egui::Ui,
    plot_id: egui::Id,
    frame: egui::Rect,
    entries: &[(String, String, egui::Color32)],
    labels: &BTreeMap<String, String>,
) {
    let label = format!("Legend ({}) ⏷", entries.len());
    let width = widgets::text_width(ui, &label, egui::TextStyle::Button)
        + ui.spacing().button_padding.x * 2.0;
    let rect = egui::Rect::from_min_size(
        frame.right_top() + egui::vec2(-width - 4.0, 4.0),
        egui::vec2(width, ui.spacing().interact_size.y),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((plot_id, "legend"))
            .max_rect(rect),
    );
    child.set_clip_rect(ui.clip_rect().intersect(frame));
    widgets::popover(&mut child, label, |ui| {
        let Some(mut memory) = egui_plot::PlotMemory::load(ui.ctx(), plot_id) else {
            return;
        };
        let mut changed = false;
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(ui, |ui| {
                for (key, name, color) in entries {
                    ui.horizontal(|ui| {
                        widgets::dot(ui, *color);
                        let item_id = trace_item_id(key);
                        let mut visible = !memory.hidden_items.contains(&item_id);
                        let label = labels
                            .get(key)
                            .filter(|label| !label.trim().is_empty())
                            .unwrap_or(name);
                        let fitted = widgets::fit_text(
                            ui,
                            label,
                            (ui.available_width()
                                - ui.spacing().icon_width
                                - ui.spacing().icon_spacing)
                                .max(0.0),
                        );
                        if ui
                            .checkbox(&mut visible, fitted)
                            .on_hover_text(label)
                            .changed()
                        {
                            if visible {
                                memory.hidden_items.remove(&item_id);
                            } else {
                                memory.hidden_items.insert(item_id);
                            }
                            changed = true;
                        }
                    });
                }
            });
        if changed {
            memory.store(ui.ctx(), plot_id);
            ui.ctx().request_repaint();
        }
    });
}

fn channel_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    axis: &str,
    value: &mut String,
    names: &[String],
) {
    ui.label(axis);
    egui::ComboBox::from_id_salt(id)
        .selected_text(value.as_str())
        .show_ui(ui, |ui| {
            for name in names {
                ui.selectable_value(value, name.clone(), name);
            }
        });
}

fn color_bar(ui: &mut egui::Ui, label: &str, mut range: [f64; 2]) {
    range = expanded_range(range);
    ui.horizontal(|ui| {
        ui.label(format!("{label}: {:.2}", range[0]));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 10.0), egui::Sense::hover());
        for i in 0..48 {
            let left = i as f32 / 48.0;
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(rect.left() + rect.width() * left, rect.top()),
                    egui::pos2(
                        rect.left() + rect.width() * (left + 1.0 / 48.0),
                        rect.bottom(),
                    ),
                ),
                0.0,
                color_map(range[0] + (range[1] - range[0]) * left as f64, range),
            );
        }
        ui.label(format!("{:.2}", range[1]));
    });
}

fn expanded_range(range: [f64; 2]) -> [f64; 2] {
    if range[0] == range[1] {
        let padding = range[0].abs().max(1.0) * 0.05;
        [range[0] - padding, range[1] + padding]
    } else {
        range
    }
}

impl AnalysisApp {
    pub(super) fn plot(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        options: &mut PlotOptions,
        units: UnitSystem,
    ) {
        let before = serde_json::to_string(options).unwrap_or_default();
        let legend_entries = self
            .prepared
            .runs
            .iter()
            .filter(|run| self.state.selection.contains(&run.key))
            .map(|run| {
                (
                    segment_key(&run.key),
                    run.name.clone(),
                    run_color(
                        &self.state.selection,
                        self.workspace.reference.as_ref(),
                        &run.key,
                    ),
                )
            })
            .collect::<Vec<_>>();
        let channels = if options.delta {
            vec![DELTA_CHANNEL.to_string()]
        } else {
            options.channels.clone()
        };
        let course_missing = options.delta && self.prepared.course.is_none();
        // The first plot's title shares the header row with the menus.
        let first_title = match channels.first() {
            Some(name) if !course_missing => {
                let traces = self.plot_traces(id, name, options, units);
                Some(plot_title(name, &traces))
            }
            _ => None,
        };
        self.plot_controls(
            ui,
            id,
            options,
            units,
            &legend_entries,
            first_title.as_deref(),
        );
        if before != serde_json::to_string(options).unwrap_or_default() {
            self.dirty = true;
            self.plot_cache.clear();
        }
        if course_missing {
            ui.label("Time gain/loss needs a reference course and matching GPS coverage.");
            return;
        }
        let height = (ui.available_height() / channels.len().max(1) as f32 - 10.0).max(140.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, name) in channels.into_iter().enumerate() {
                let traces = self.plot_traces(id, &name, options, units);
                if index > 0 {
                    ui.label(
                        RichText::new(plot_title(&name, &traces))
                            .strong()
                            .color(text::normal()),
                    );
                }
                if traces.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.weak("No compatible channel / valid alignment for selected runs.");
                        if !options.delta && ui.small_button("Remove from plot").clicked() {
                            options.channels.retain(|selected| selected != &name);
                        }
                    });
                    continue;
                }
                let cursor = self
                    .reference_run()
                    .and_then(|r| self.x_at_time(r, r.start + self.state.cursor));
                let x_label = match self.effective_mode() {
                    XMode::Time => "s",
                    XMode::Distance | XMode::Course => "m",
                };
                let widget_id = ui.make_persistent_id((id, &name, self.effective_mode().label()));
                let overflow = options.show_legend
                    && index == 0
                    && legend_overflows(ui, &legend_entries, &options.legend_labels, height);
                let mut plot_widget = Plot::new((id, &name, self.effective_mode().label()))
                    .height(height)
                    .allow_zoom([true, false])
                    .allow_scroll([true, false])
                    // Gesture zoom is intentionally X-only, but dragging an
                    // axis should still scale that axis directly. Y remains
                    // independent because only X is linked between plots.
                    .allow_axis_zoom_drag([true, true])
                    .allow_boxed_zoom(false)
                    .link_axis(
                        egui::Id::new(("analysis-x", self.effective_mode().label())),
                        [true, false],
                    )
                    .x_axis_label(x_label);
                // One legend per panel: stacked plots share the same runs.
                if options.show_legend && index == 0 && !overflow {
                    plot_widget = plot_widget.legend(compact_legend());
                }
                // Lines are reduced to what the plot can show at its current
                // zoom; the view comes from the previous frame's bounds.
                let plot_id = egui::Id::new(("analysis-plot-view", id, &name));
                let view = ui.data(|d| d.get_temp::<[f64; 2]>(plot_id));
                let pixels = ui.available_width().max(1.0) as usize;
                for [low, high] in traces.iter().filter_map(|trace| trace.x_range) {
                    plot_widget = plot_widget.include_x(low).include_x(high);
                }
                let response = plot_widget.show(ui, |plot| {
                    for trace in traces.iter() {
                        let label = options
                            .legend_labels
                            .get(&segment_key(&trace.segment))
                            .filter(|label| !label.trim().is_empty())
                            .unwrap_or(&trace.name);
                        // Measured pieces go first: the legend takes the color
                        // of a trace's first line.
                        for outside in [false, true] {
                            let color = if outside {
                                trace.color.gamma_multiply(0.3)
                            } else {
                                trace.color
                            };
                            for (piece, _) in trace
                                .points
                                .iter()
                                .zip(&trace.outside_gates)
                                .filter(|(_, piece_outside)| **piece_outside == outside)
                            {
                                if piece.len() > 1 {
                                    let shown = decimate::view_points(piece, view, pixels);
                                    plot.line(
                                        Line::new(label, shown.into_owned())
                                            .color(color)
                                            .id(trace_item_id(&segment_key(&trace.segment))),
                                    );
                                }
                            }
                        }
                    }
                    if let Some(x) = cursor {
                        plot.vline(VLine::new("", x).color(text::strong()));
                    }
                    if let Some(range) = self.state.range
                        && let Some(r) = self.reference_run()
                    {
                        for t in range {
                            if let Some(x) = self.x_at_time(r, r.start + t) {
                                plot.vline(VLine::new("", x).color(egui::Color32::GRAY));
                            }
                        }
                    }
                    if let Some(r) = self.reference_run() {
                        for t in [r.start_gate, r.finish_gate].into_iter().flatten() {
                            if let Some(x) = self.x_at_time(r, t) {
                                plot.vline(
                                    VLine::new("", x)
                                        .color(theme::accent())
                                        .style(LineStyle::dashed_dense()),
                                );
                            }
                        }
                    }
                    if (plot.response().clicked()
                        || plot.response().dragged_by(egui::PointerButton::Primary))
                        && let Some(p) = plot.pointer_coordinate()
                    {
                        Some(p.x)
                    } else {
                        None
                    }
                });
                if overflow {
                    overflow_legend(
                        ui,
                        widget_id,
                        *response.transform.frame(),
                        &legend_entries,
                        &options.legend_labels,
                    );
                }
                let bounds = response.transform.bounds();
                let shown = [bounds.min()[0], bounds.max()[0]];
                if view != Some(shown) {
                    ui.data_mut(|d| d.insert_temp(plot_id, shown));
                    // The next frame needs the new view to draw the right samples.
                    ui.ctx().request_repaint();
                }
                response.response.context_menu(|ui| {
                    ui.checkbox(&mut options.show_legend, "Show legend");
                    ui.separator();
                    ui.label("Legend text");
                    for (key, default, color) in &legend_entries {
                        ui.horizontal(|ui| {
                            widgets::dot(ui, *color);
                            let label = options
                                .legend_labels
                                .entry(key.clone())
                                .or_insert_with(|| default.clone());
                            ui.add(egui::TextEdit::singleline(label).desired_width(180.0));
                        });
                    }
                    ui.weak("Double-click a plot to reset its view.");
                });
                if let Some(x) = response.inner {
                    self.scrub_x(x);
                }
            }
        });
        if before != serde_json::to_string(options).unwrap_or_default() {
            self.dirty = true;
        }
    }
    /// Always-visible plot header: channel chooser plus an options popover.
    fn plot_controls(
        &self,
        ui: &mut egui::Ui,
        id: u64,
        options: &mut PlotOptions,
        units: UnitSystem,
        legend_entries: &[(String, String, egui::Color32)],
        title: Option<&str>,
    ) {
        ui.horizontal_wrapped(|ui| {
            // The delta panel's title is drawn with its plot below.
            if !options.delta {
                let label = format!("Channels ({}) ⏷", options.channels.len());
                widgets::popover(ui, label, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(280.0)
                        .show(ui, |ui| {
                            let available = self.channel_names();
                            for name in self.channel_choices(&options.channels) {
                                let mut selected = options.channels.contains(&name);
                                let missing = !available.contains(&name);
                                let label = if name == DELTA_CHANNEL {
                                    DELTA_LABEL.to_owned()
                                } else if missing {
                                    format!("{name} (no loaded source has it)")
                                } else {
                                    name.clone()
                                };
                                if ui.checkbox(&mut selected, label).changed() {
                                    if selected {
                                        options.channels.push(name);
                                    } else {
                                        options.channels.retain(|n| n != &name);
                                    }
                                }
                            }
                        });
                });
            }
            widgets::popover(ui, "Options ⏷", |ui| {
                widgets::section_label(ui, "Smoothing");
                ui.horizontal(|ui| {
                    let mut enabled = options.filter.is_some();
                    if ui
                        .checkbox(&mut enabled, "Zero-phase low-pass")
                        .on_hover_text("Non-causal smoothing applied to this panel only")
                        .changed()
                    {
                        options.filter = enabled.then_some(8.0);
                    }
                    if let Some(hz) = options.filter.as_mut() {
                        ui.add(egui::DragValue::new(hz).range(0.1..=100.0).suffix(" Hz"));
                    }
                });
                if !options.delta {
                    widgets::section_label(ui, "Units and channel matching");
                    egui::ScrollArea::vertical()
                        .id_salt("plot-units")
                        .max_height(240.0)
                        .show(ui, |ui| self.plot_unit_rows(ui, id, options, units));
                }
                widgets::section_label(ui, "Legend");
                legend_editor(
                    ui,
                    &mut options.show_legend,
                    &mut options.legend_labels,
                    legend_entries,
                );
            });
            if let Some(title) = title {
                ui.label(RichText::new(title).strong().color(text::normal()));
            }
        });
    }

    /// Prepared traces for one channel, rebuilt only when inputs change.
    fn plot_traces(
        &mut self,
        id: u64,
        name: &str,
        options: &PlotOptions,
        units: UnitSystem,
    ) -> Arc<Vec<PlotTrace>> {
        let cache_key = format!(
            "{id}:{}:{:?}:{units:?}:{name}:{}",
            self.prepared_revision,
            self.effective_mode(),
            serde_json::to_string(options).unwrap_or_default()
        );
        if !self.plot_cache.contains_key(&cache_key) {
            let prefix = format!(
                "{id}:{}:{:?}:{units:?}:{name}:",
                self.prepared_revision,
                self.effective_mode()
            );
            self.plot_cache.retain(|key, _| !key.starts_with(&prefix));
            let traces = self.build_plot(name, options, units);
            self.plot_cache.insert(cache_key.clone(), Arc::new(traces));
        }
        self.plot_cache[&cache_key].clone()
    }

    fn plot_unit_rows(
        &self,
        ui: &mut egui::Ui,
        id: u64,
        options: &mut PlotOptions,
        units: UnitSystem,
    ) {
        for name in options.channels.clone() {
            if name == DELTA_CHANNEL {
                continue;
            }
            let descriptor = self.prepared.runs.iter().find_map(|r| {
                self.source_channel(r, &name, options)
                    .map(|(c, _)| c.descriptor.clone())
            });
            if let Some(d) = descriptor {
                let default = display_unit(d.unit.clone(), d.quantity, units);
                let mut target = options.units.get(&name).cloned().unwrap_or(default.clone());
                ui.horizontal(|ui| {
                    ui.label(&name);
                    egui::ComboBox::from_id_salt((id, &name, "units"))
                        .selected_text(target.symbol())
                        .show_ui(ui, |ui| {
                            for u in d.unit.compatible_units() {
                                ui.selectable_value(&mut target, u.clone(), u.symbol());
                            }
                        });
                    if ui.small_button("Default").clicked() {
                        options.units.remove(&name);
                    } else if target != default || options.units.contains_key(&name) {
                        options.units.insert(name.clone(), target);
                    }
                });
            }
            for recording in &self.workspace.recordings {
                if !self
                    .state
                    .selection
                    .iter()
                    .any(|s| s.recording_id == recording.id)
                {
                    continue;
                }
                let key = format!("{}:{name}", recording.id.0);
                egui::ComboBox::from_id_salt((id, &key))
                    .selected_text(format!(
                        "{}: {}",
                        recording.name,
                        if options.bindings.contains_key(&key) {
                            "manual channel"
                        } else {
                            "automatic"
                        }
                    ))
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                !options.bindings.contains_key(&key),
                                "Automatic name matching",
                            )
                            .clicked()
                        {
                            options.bindings.remove(&key);
                        }
                        for source in &recording.sources {
                            let Some(d) = self.data.get(&source.id) else {
                                continue;
                            };
                            for channel in d.processed.channels.values() {
                                let label = format!(
                                    "{} / {} ({})",
                                    source.name,
                                    channel.descriptor.name,
                                    channel.descriptor.unit.symbol()
                                );
                                if ui.selectable_label(false, label).clicked() {
                                    options.bindings.insert(
                                        key.clone(),
                                        ChannelRef {
                                            source_id: source.id,
                                            channel_id: channel.descriptor.id,
                                        },
                                    );
                                }
                            }
                        }
                    });
            }
        }
    }
    pub(super) fn build_plot(
        &self,
        name: &str,
        options: &PlotOptions,
        units: UnitSystem,
    ) -> Vec<PlotTrace> {
        let mut result = vec![];
        let mut common_unit = None;
        for run in &self.prepared.runs {
            if !self.state.selection.contains(&run.key) {
                continue;
            }
            let delta = options.delta || name == DELTA_CHANNEL;
            // Delta samples are timed on the reference clock.
            let gates = if delta {
                self.reference_run()
            } else {
                Some(run)
            }
            .and_then(|timed| Some((timed.start_gate?, timed.finish_gate)));
            let measured = |t: f64| {
                gates.is_none_or(|(start, finish)| t >= start && finish.is_none_or(|f| t <= f))
            };
            let (samples, unit, gap) = if delta {
                let Some(reference) = self.reference_run() else {
                    continue;
                };
                let reference_origin = self.elapsed_origin(reference);
                let run_origin = self.elapsed_origin(run);
                let mut values = reference
                    .progress
                    .iter()
                    .filter(|point| {
                        point.recording_time >= reference.start
                            && point.recording_time <= reference.end
                    })
                    .map(|p| {
                        let other = if run.key == reference.key {
                            Some(p.recording_time)
                        } else {
                            time_at_progress(&run.progress, p.progress)
                                .filter(|time| *time >= run.start && *time <= run.end)
                        };
                        let x = self.x_at_time(reference, p.recording_time);
                        let value = if p.confidence > 0.0 {
                            x.zip(other).map(|(x, other)| {
                                [
                                    x,
                                    (other - run_origin) - (p.recording_time - reference_origin),
                                ]
                            })
                        } else {
                            None
                        };
                        (p.recording_time, value.unwrap_or([f64::NAN; 2]))
                    })
                    .collect::<Vec<_>>();
                if reference.start_gate.is_some()
                    && run.start_gate.is_some()
                    && let Some(baseline) = values.iter().find_map(|(time, point)| {
                        (*time >= reference_origin && point[1].is_finite()).then_some(point[1])
                    })
                {
                    // Runs that cross the start gate share a meaningful start
                    // line. Rebase at the first jointly covered position from
                    // the gate on, so GPS sample cadence and course projection
                    // cannot introduce a non-zero delta at the gate.
                    for (_, point) in &mut values {
                        if point[1].is_finite() {
                            point[1] -= baseline;
                        }
                    }
                }
                if let Some(hz) = options.filter {
                    // Do not smooth across explicit GPS match failures, even
                    // when their duration is shorter than the usual gap limit.
                    for chunk in values.split_mut(|(_, p)| !p.iter().all(|v| v.is_finite())) {
                        let series = ChannelSeries::new(
                            chunk
                                .iter()
                                .map(|(t, p)| TimedSample {
                                    time: *t,
                                    value: p[1],
                                })
                                .collect(),
                        )
                        .with_gap(2.0);
                        if let Ok(filtered) = series.low_pass_hz(hz) {
                            for (value, filtered) in chunk.iter_mut().zip(filtered.samples) {
                                value.1[1] = filtered.value;
                            }
                        }
                    }
                }
                (values, Unit::Second, 2.0)
            } else {
                let Some((channel, offset)) = self.source_channel(run, name, options) else {
                    continue;
                };
                let target = common_unit.clone().unwrap_or_else(|| {
                    options.units.get(name).cloned().unwrap_or_else(|| {
                        display_unit(
                            channel.descriptor.unit.clone(),
                            channel.descriptor.quantity.clone(),
                            units,
                        )
                    })
                });
                if Unit::convert_value(0.0, &channel.descriptor.unit, &target).is_err() {
                    continue;
                }
                common_unit = Some(target.clone());
                let filtered = if channel.descriptor.interpolation == Interpolation::Linear {
                    options
                        .filter
                        .and_then(|hz| channel.series.low_pass_hz(hz).ok())
                } else {
                    None
                };
                let series = filtered.as_ref().unwrap_or(&channel.series);
                let mut values = series
                    .samples
                    .iter()
                    .filter_map(|p| {
                        let t = p.time - offset;
                        if t < run.start || t > run.end {
                            return None;
                        }
                        // Retain an explicit break when position is unavailable.
                        let x = self.x_at_time(run, t).unwrap_or(f64::NAN);
                        let y =
                            Unit::convert_value(p.value, &channel.descriptor.unit, &target).ok()?;
                        Some((t, [x, y]))
                    })
                    .collect::<Vec<_>>();
                for recording_time in [run.start, run.end] {
                    if values
                        .iter()
                        .any(|(time, _)| (*time - recording_time).abs() < 1e-8)
                    {
                        continue;
                    }
                    let Some(value) = series.sample_at_default(
                        recording_time + offset,
                        channel.descriptor.interpolation,
                    ) else {
                        continue;
                    };
                    let Some(y) =
                        Unit::convert_value(value, &channel.descriptor.unit, &target).ok()
                    else {
                        continue;
                    };
                    values.push((
                        recording_time,
                        [self.x_at_time(run, recording_time).unwrap_or(f64::NAN), y],
                    ));
                }
                values.sort_by(|a, b| a.0.total_cmp(&b.0));
                (values, target, channel.series.gap_seconds.unwrap_or(2.0))
            };
            let mut pieces = vec![];
            let mut outside_gates = vec![];
            let mut piece: Vec<[f64; 2]> = vec![];
            let mut piece_measured = true;
            let mut previous = None;
            for (t, value) in samples {
                if !value.iter().all(|v| v.is_finite()) || previous.is_some_and(|p| t - p > gap) {
                    if piece.len() > 1 {
                        pieces.push(std::mem::take(&mut piece));
                        outside_gates.push(!piece_measured);
                    } else {
                        piece.clear();
                    }
                }
                if !value.iter().all(|v| v.is_finite()) {
                    previous = None;
                    continue;
                }
                let inside = measured(t);
                if let Some(&last) = piece.last()
                    && inside != piece_measured
                {
                    // The new piece starts at the last sample so the line
                    // stays continuous across the gate.
                    if piece.len() > 1 {
                        pieces.push(std::mem::take(&mut piece));
                        outside_gates.push(!piece_measured);
                    }
                    piece = vec![last];
                }
                piece_measured = inside;
                piece.push(value);
                previous = Some(t);
            }
            if piece.len() > 1 {
                pieces.push(piece);
                outside_gates.push(!piece_measured);
            }
            result.push(PlotTrace {
                segment: run.key.clone(),
                name: run.name.clone(),
                color: run_color(
                    &self.state.selection,
                    self.workspace.reference.as_ref(),
                    &run.key,
                ),
                x_range: pieces.iter().flatten().map(|point| point[0]).fold(
                    None,
                    |range: Option<[f64; 2]>, x| {
                        Some(range.map_or([x, x], |[low, high]| [low.min(x), high.max(x)]))
                    },
                ),
                points: pieces,
                outside_gates,
                unit,
            });
        }
        result
    }

    pub(super) fn scatter(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        options: &mut ScatterOptions,
        units: UnitSystem,
    ) {
        let before = serde_json::to_string(options).unwrap_or_default();
        let names = self
            .channel_names()
            .into_iter()
            .filter(|name| name != DELTA_CHANNEL)
            .collect::<Vec<_>>();
        let legend_entries = self
            .prepared
            .runs
            .iter()
            .filter(|run| self.state.selection.contains(&run.key))
            .map(|run| {
                (
                    segment_key(&run.key),
                    run.name.clone(),
                    run_color(
                        &self.state.selection,
                        self.workspace.reference.as_ref(),
                        &run.key,
                    ),
                )
            })
            .collect::<Vec<_>>();
        ui.horizontal_wrapped(|ui| {
            channel_combo(ui, (id, "scatter-x"), "X", &mut options.x_channel, &names);
            channel_combo(ui, (id, "scatter-y"), "Y", &mut options.y_channel, &names);
            let mut use_z = options.z_channel.is_some();
            if ui
                .checkbox(&mut use_z, "Color by")
                .on_hover_text("Color samples by a third channel")
                .changed()
            {
                options.z_channel = use_z.then(|| "gps_speed".into());
            }
            if let Some(z) = options.z_channel.as_mut() {
                channel_combo(ui, (id, "scatter-z"), "Z", z, &names);
            }
            widgets::popover(ui, "Options ⏷", |ui| {
                widgets::section_label(ui, "Smoothing");
                ui.horizontal(|ui| {
                    let mut filter = options.filter.is_some();
                    if ui.checkbox(&mut filter, "Zero-phase low-pass").changed() {
                        options.filter = filter.then_some(8.0);
                    }
                    if let Some(hz) = options.filter.as_mut() {
                        ui.add(egui::DragValue::new(hz).range(0.1..=100.0).suffix(" Hz"));
                    }
                });
                widgets::section_label(ui, "Legend");
                legend_editor(
                    ui,
                    &mut options.show_legend,
                    &mut options.legend_labels,
                    &legend_entries,
                );
            });
        });

        let cache_key = format!(
            "{id}:{}:{units:?}:{}",
            self.prepared_revision,
            serde_json::to_string(options).unwrap_or_default()
        );
        if !self.scatter_cache.contains_key(&cache_key) {
            self.scatter_cache
                .retain(|key, _| !key.starts_with(&format!("{id}:")));
            let traces = self.build_scatter(options, units);
            self.scatter_cache
                .insert(cache_key.clone(), Arc::new(traces));
        }
        let traces = self.scatter_cache.get(&cache_key).unwrap().clone();
        if traces.is_empty() {
            ui.label("No selected run has compatible X and Y channels with overlapping samples.");
            if before != serde_json::to_string(options).unwrap_or_default() {
                self.dirty = true;
            }
            return;
        }
        let x_unit = self.scatter_unit(&options.x_channel, options, units);
        let y_unit = self.scatter_unit(&options.y_channel, options, units);
        let z_range = options.z_channel.as_ref().and_then(|_| {
            traces
                .iter()
                .flat_map(|trace| trace.points.iter().map(|point| point[2]))
                .filter(|value| value.is_finite())
                .fold(None::<[f64; 2]>, |range, value| match range {
                    None => Some([value, value]),
                    Some([low, high]) => Some([low.min(value), high.max(value)]),
                })
                .map(expanded_range)
        });
        if let (Some(z), Some(range)) = (&options.z_channel, z_range) {
            color_bar(ui, z, range);
        }
        let widget_id = ui.make_persistent_id((id, "scatter"));
        let overflow = options.show_legend
            && legend_overflows(
                ui,
                &legend_entries,
                &options.legend_labels,
                ui.available_height(),
            );
        let mut plot_widget = Plot::new((id, "scatter"))
            .x_axis_label(format!(
                "{} ({})",
                options.x_channel,
                x_unit.as_ref().map_or("", Unit::symbol)
            ))
            .y_axis_label(format!(
                "{} ({})",
                options.y_channel,
                y_unit.as_ref().map_or("", Unit::symbol)
            ))
            .allow_boxed_zoom(false);
        if options.show_legend && !overflow {
            plot_widget = plot_widget.legend(compact_legend());
        }
        let response = plot_widget.show(ui, |plot| {
            for trace in traces.iter() {
                let label = options
                    .legend_labels
                    .get(&segment_key(&trace.segment))
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or(&trace.name);
                if let Some(range) = z_range {
                    let mut named = false;
                    for bin in 0..24 {
                        let low = range[0] + (range[1] - range[0]) * bin as f64 / 24.0;
                        let high = range[0] + (range[1] - range[0]) * (bin + 1) as f64 / 24.0;
                        let points = trace
                            .points
                            .iter()
                            .filter(|point| {
                                point[2] >= low
                                    && (point[2] < high || (bin == 23 && point[2] <= high))
                            })
                            .map(|point| [point[0], point[1]])
                            .collect::<Vec<_>>();
                        if !points.is_empty() {
                            let name = if named { "" } else { label };
                            named = true;
                            plot.points(
                                Points::new(name, points)
                                    .radius(2.0)
                                    .id(trace_item_id(&segment_key(&trace.segment)))
                                    .color(color_map((low + high) * 0.5, range)),
                            );
                        }
                    }
                } else {
                    let points = trace
                        .points
                        .iter()
                        .map(|point| [point[0], point[1]])
                        .collect::<Vec<_>>();
                    plot.points(
                        Points::new(label, points)
                            .radius(2.0)
                            .color(trace.color)
                            .id(trace_item_id(&segment_key(&trace.segment))),
                    );
                }
            }
        });
        if overflow {
            overflow_legend(
                ui,
                widget_id,
                *response.transform.frame(),
                &legend_entries,
                &options.legend_labels,
            );
        }
        response.response.context_menu(|ui| {
            ui.checkbox(&mut options.show_legend, "Show legend");
            ui.separator();
            ui.label("Legend text");
            for (key, default, color) in &legend_entries {
                ui.horizontal(|ui| {
                    widgets::dot(ui, *color);
                    let label = options
                        .legend_labels
                        .entry(key.clone())
                        .or_insert_with(|| default.clone());
                    ui.add(egui::TextEdit::singleline(label).desired_width(180.0));
                });
            }
            ui.weak("Double-click the plot to reset its view.");
        });
        if before != serde_json::to_string(options).unwrap_or_default() {
            self.dirty = true;
        }
    }

    fn scatter_unit(
        &self,
        name: &str,
        options: &ScatterOptions,
        units: UnitSystem,
    ) -> Option<Unit> {
        options.units.get(name).cloned().or_else(|| {
            self.prepared.runs.iter().find_map(|run| {
                self.source_channel_with_bindings(run, name, &options.bindings)
                    .map(|(channel, _)| {
                        display_unit(
                            channel.descriptor.unit.clone(),
                            channel.descriptor.quantity.clone(),
                            units,
                        )
                    })
            })
        })
    }

    pub(super) fn build_scatter(
        &self,
        options: &ScatterOptions,
        units: UnitSystem,
    ) -> Vec<ScatterTrace> {
        let x_unit = self.scatter_unit(&options.x_channel, options, units);
        let y_unit = self.scatter_unit(&options.y_channel, options, units);
        let z_unit = options
            .z_channel
            .as_ref()
            .and_then(|name| self.scatter_unit(name, options, units));
        self.prepared
            .runs
            .iter()
            .filter(|run| self.state.selection.contains(&run.key))
            .filter_map(|run| {
                let (x, x_offset) =
                    self.source_channel_with_bindings(run, &options.x_channel, &options.bindings)?;
                let (y, y_offset) =
                    self.source_channel_with_bindings(run, &options.y_channel, &options.bindings)?;
                let z = match options.z_channel.as_ref() {
                    Some(name) => {
                        Some(self.source_channel_with_bindings(run, name, &options.bindings)?)
                    }
                    None => None,
                };
                let filtered = |channel: &TelemetryChannel| {
                    if channel.descriptor.interpolation == Interpolation::Linear {
                        options
                            .filter
                            .and_then(|hz| channel.series.low_pass_hz(hz).ok())
                            .unwrap_or_else(|| channel.series.clone())
                    } else {
                        channel.series.clone()
                    }
                };
                let x_series = filtered(x);
                let y_series = filtered(y);
                let z_series = z.map(|(channel, offset)| (filtered(channel), channel, offset));
                let step = (x_series.samples.len() / 12_000).max(1);
                let points = x_series
                    .samples
                    .iter()
                    .step_by(step)
                    .filter_map(|sample| {
                        let recording_time = sample.time - x_offset;
                        if recording_time < run.start || recording_time > run.end {
                            return None;
                        }
                        let x_value =
                            Unit::convert_value(sample.value, &x.descriptor.unit, x_unit.as_ref()?)
                                .ok()?;
                        let y_value = y_series.sample_at_default(
                            recording_time + y_offset,
                            y.descriptor.interpolation,
                        )?;
                        let y_value =
                            Unit::convert_value(y_value, &y.descriptor.unit, y_unit.as_ref()?)
                                .ok()?;
                        let z_value = if let Some((series, channel, offset)) = &z_series {
                            let value = series.sample_at_default(
                                recording_time + *offset,
                                channel.descriptor.interpolation,
                            )?;
                            Unit::convert_value(value, &channel.descriptor.unit, z_unit.as_ref()?)
                                .ok()?
                        } else {
                            0.0
                        };
                        Some([x_value, y_value, z_value])
                    })
                    .collect::<Vec<_>>();
                (!points.is_empty()).then(|| ScatterTrace {
                    segment: run.key.clone(),
                    name: run.name.clone(),
                    color: run_color(
                        &self.state.selection,
                        self.workspace.reference.as_ref(),
                        &run.key,
                    ),
                    points,
                })
            })
            .collect()
    }

    /// The reframing settings of the recording behind `key`, defaulted for raw
    /// INSV video and absent for ordinary video.
    fn video_processing_for(
        &self,
        key: Option<&SegmentRef>,
    ) -> Option<overlay_core::VideoProcessingConfig> {
        let key = key?;
        let recording = self
            .workspace
            .recordings
            .iter()
            .find(|recording| recording.id == key.recording_id)?;
        let path = recording
            .video_path
            .as_ref()
            .filter(|path| !path.as_os_str().is_empty())?;
        recording
            .video_processing
            .clone()
            .or_else(|| crate::video_processing::default_processing(path))
    }

    pub(super) fn video(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        options: &mut VideoOptions,
        tools: Option<&FfmpegTools>,
    ) {
        self.visible_videos.insert(id);
        let automatic = self.state.selection.get(options.slot).cloned();
        let mut key = options.segment.clone().or(automatic.clone());
        let started_with = key.clone();
        // Reframing controls share the picker's row (or fold into a menu when
        // it is narrow) instead of taking rows of their own.
        let mut processing = self.video_processing_for(key.as_ref());
        ui.horizontal_wrapped(|ui| {
            // Reserve the actual compact controls before sizing the picker.
            // A long lap name must not squeeze the Video button into a sliver.
            let spacing = ui.spacing();
            let linked_width = spacing.icon_width
                + spacing.icon_spacing
                + widgets::text_width(ui, "Linked", egui::TextStyle::Body);
            let controls_width = if processing.is_some() {
                widgets::text_width(ui, "Video ⏷", egui::TextStyle::Button)
                    + spacing.button_padding.x * 2.0
                    + spacing.item_spacing.x
            } else {
                0.0
            };
            let picker_width =
                (ui.available_width() - linked_width - controls_width - spacing.item_spacing.x)
                    .clamp(0.0, 260.0);
            let selected_name = key
                .as_ref()
                .and_then(|key| self.prepared.runs.iter().find(|run| &run.key == key))
                .map_or("Choose lap / run", |run| run.name.as_str());
            // ComboBox::width is a minimum, even in truncate mode. Fit its
            // selected label explicitly so it cannot consume reserved space.
            let picker_text = widgets::fit_text(
                ui,
                selected_name,
                (picker_width
                    - spacing.button_padding.x * 2.0
                    - spacing.icon_width
                    - spacing.icon_spacing)
                    .max(0.0),
            );
            egui::ComboBox::from_id_salt((id, "video-lap"))
                .width(picker_width)
                .truncate()
                .selected_text(picker_text)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(options.segment.is_none(), "Follow comparison selection")
                        .clicked()
                    {
                        options.segment = None;
                        key = automatic;
                    }
                    for r in &self.prepared.runs {
                        if ui
                            .selectable_label(key.as_ref() == Some(&r.key), &r.name)
                            .clicked()
                        {
                            key = Some(r.key.clone());
                            options.segment = key.clone();
                        }
                    }
                })
                .response
                .on_hover_text(selected_name);
            ui.checkbox(&mut options.linked, "Linked").on_hover_text(
                "Follow the shared playhead. Unlink to inspect this video's own timestamp.",
            );
            if !options.linked {
                ui.add(
                    egui::DragValue::new(&mut options.time)
                        .speed(0.02)
                        .prefix("Video ")
                        .suffix(" s"),
                );
            }
            if let Some(config) = &mut processing {
                ui.push_id(id, |ui| {
                    crate::video_processing::controls(ui, config);
                });
            }
        });
        if key != started_with {
            // The picker changed recording this frame; edits belong to the old one.
            processing = self.video_processing_for(key.as_ref());
        }
        let Some(key) = key else {
            widgets::empty_state(
                ui,
                "No run selected",
                "Tick a lap in Recordings & laps, or choose one above.",
                |_| {},
            );
            return;
        };
        let Some(run) = self.prepared.runs.iter().find(|r| r.key == key) else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.weak("Preparing lap…");
            });
            return;
        };
        let Some(recording) = self
            .workspace
            .recordings
            .iter()
            .find(|r| r.id == key.recording_id)
            .cloned()
        else {
            return;
        };
        let Some(path) = recording
            .video_path
            .clone()
            .filter(|p| !p.as_os_str().is_empty())
        else {
            let recording_id = recording.id;
            let mut attach = false;
            widgets::empty_state(
                ui,
                "No video for this run",
                "Telemetry is ready to compare. Attach the matching video to see it here.",
                |ui| attach = ui.button("Attach video…").clicked(),
            );
            if attach {
                self.attach_video_dialog(recording_id);
            }
            return;
        };
        if processing != recording.video_processing {
            if let Some(target) = self
                .workspace
                .recordings
                .iter_mut()
                .find(|r| r.id == recording.id)
            {
                target.video_processing = processing.clone();
            }
            self.dirty = true;
        }
        let Some(tools) = tools else {
            widgets::callout(
                ui,
                Tone::Warn,
                "FFmpeg was not found, so video cannot be shown. Install FFmpeg and relaunch; telemetry analysis is unaffected.",
            );
            return;
        };
        let timestamp = if options.linked {
            self.run_time(run).map(|t| recording.video_time(t))
        } else {
            Some(options.time)
        };
        let Some(timestamp) = timestamp.filter(|t| t.is_finite()) else {
            self.videos.remove(&id);
            widgets::hint(ui, "No video or alignment coverage at this position.");
            return;
        };
        let (timestamp, video_starts_in) = hold_first_frame(timestamp);
        if !self.metadata.contains_key(&path) && self.probing.insert(path.clone()) {
            let tx = self.tx.clone();
            let file = path.clone();
            let tools = tools.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let result = overlay_media::probe_video(&tools, &file).map_err(|e| e.to_string());
                let _ = tx.send(Event::Probed(file, result));
                ctx.request_repaint();
            });
        }
        let metadata = match self.metadata.get(&path) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                widgets::callout(ui, Tone::Bad, e.clone());
                return;
            }
            None => {
                ui.spinner();
                return;
            }
        };
        if metadata.duration.is_some_and(|d| timestamp >= d) {
            self.videos.remove(&id);
            widgets::hint(ui, "The video ends before this position.");
            return;
        }
        if options.linked {
            options.time = timestamp;
        }
        if video_starts_in > 0.0 {
            widgets::hint(
                ui,
                format!(
                    "{} · video starts {:.3} s later; showing its first frame",
                    run.name, video_starts_in
                ),
            );
        } else {
            widgets::hint(ui, format!("{} · video {:.3} s", run.name, timestamp));
        }
        let width = ((ui.available_width().clamp(160.0, 960.0) as u32) / 32 * 32).max(160);
        let height = (if processing.is_some() {
            width as f64 * 9.0 / 16.0
        } else {
            width as f64 * metadata.height as f64 / metadata.width.max(1) as f64
        })
        .round() as u32;
        let height = height.max(2);
        let size = PreviewSize::new(width, height);
        if self.videos.get(&id).is_none_or(|v| v.path != path) {
            self.videos.insert(
                id,
                VideoRuntime {
                    path: path.clone(),
                    decoder: crate::video_processing::VideoPreview::spawn(
                        tools.clone(),
                        path,
                        processing.clone(),
                        metadata.fps(),
                        false,
                    ),
                    processing: processing.clone(),
                    texture: None,
                    requested: None,
                    awaiting_since: None,
                    error: None,
                },
            );
        }
        let runtime = self.videos.get_mut(&id).unwrap();
        let processing_changed = runtime.processing != processing;
        runtime.processing = processing.clone();
        if let Some(config) = &processing {
            runtime.decoder.set_config(config);
        }
        if processing_changed
            || runtime.requested.is_none_or(|(t, w, h)| {
                (t - timestamp).abs() > 1.0 / 60.0 || w != width || h != height
            })
        {
            match runtime.decoder.request(timestamp, size) {
                Ok(()) => runtime.awaiting_since = Some(Instant::now()),
                Err(e) => runtime.error = Some(e.to_string()),
            }
            runtime.requested = Some((timestamp, width, height));
        }
        while let Some(result) = runtime.decoder.try_recv() {
            match result {
                Ok(frame) => {
                    if (frame.timestamp - timestamp).abs() < 0.3 {
                        runtime.awaiting_since = None;
                        runtime.error = None;
                        let image = egui::ColorImage::from_rgba_unmultiplied(
                            [frame.width as usize, frame.height as usize],
                            &frame.rgba,
                        );
                        if let Some(texture) = runtime.texture.as_mut() {
                            texture.set(image, egui::TextureOptions::LINEAR);
                        } else {
                            runtime.texture = Some(ui.ctx().load_texture(
                                format!("analysis-video-{id}"),
                                image,
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                    }
                }
                Err(e) => {
                    runtime.awaiting_since = None;
                    runtime.error = Some(e.to_string());
                    if processing.is_some() {
                        runtime.texture = None;
                    }
                }
            }
        }
        if let Some(error) = &runtime.error {
            widgets::callout(ui, Tone::Bad, error.clone());
        }
        if let Some(texture) = &runtime.texture {
            let response = ui.add(
                egui::Image::new(texture)
                    .max_size(ui.available_size())
                    .maintain_aspect_ratio(true)
                    .sense(if processing.is_some() {
                        egui::Sense::drag()
                    } else {
                        egui::Sense::hover()
                    }),
            );
            if let Some(config) = &mut processing
                && crate::video_processing::gestures(ui, &response, config)
            {
                runtime.decoder.set_config(config);
                runtime.processing = Some(config.clone());
                match runtime.decoder.request(timestamp, size) {
                    Ok(()) => runtime.awaiting_since = Some(Instant::now()),
                    Err(e) => runtime.error = Some(e.to_string()),
                }
            }
        } else {
            ui.spinner();
        }
        let status = runtime.decoder.status();
        if !status.is_empty() {
            widgets::hint(ui, status);
        }
        if processing != recording.video_processing {
            if let Some(target) = self
                .workspace
                .recordings
                .iter_mut()
                .find(|r| r.id == recording.id)
            {
                target.video_processing = processing;
            }
            self.dirty = true;
        }
        // Frames arrive from a worker thread without waking the UI, so poll
        // while one is outstanding. A settled panel must not repaint at all:
        // every repaint re-lays-out every plot.
        if self
            .videos
            .get(&id)
            .and_then(|runtime| runtime.awaiting_since)
            .is_some_and(|since| since.elapsed() < FRAME_WAIT_LIMIT)
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
    }
    pub(super) fn map(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        channel: &mut String,
        settings: &mut MapSettings,
        units: UnitSystem,
    ) {
        let settings_before = serde_json::to_string(&(&*channel, &*settings)).unwrap_or_default();
        let descriptor = self.prepared.runs.iter().find_map(|r| {
            self.source_channel(r, channel, &PlotOptions::default())
                .map(|(c, _)| c.descriptor.clone())
        });
        let target_unit = descriptor.map(|d| {
            settings
                .display_unit
                .clone()
                .unwrap_or_else(|| display_unit(d.unit, d.quantity, units))
        });
        let cache_key = format!(
            "{id}:{}:{units:?}:{channel}:{:?}:{:?}",
            self.prepared_revision, settings.low_pass_hz, target_unit
        );
        if !self.map_cache.contains_key(&cache_key) {
            self.map_cache
                .retain(|key, _| !key.starts_with(&format!("{id}:")));
            let mut traces = vec![];
            let options = PlotOptions::default();
            for run in &self.prepared.runs {
                if !self.state.selection.contains(&run.key) {
                    continue;
                }
                let descriptor = self.source_channel(run, channel, &options);
                let values = descriptor
                    .map(|(c, off)| {
                        let unit = target_unit
                            .clone()
                            .unwrap_or_else(|| c.descriptor.unit.clone());
                        let filtered = if c.descriptor.interpolation == Interpolation::Linear {
                            settings
                                .low_pass_hz
                                .and_then(|hz| c.series.low_pass_hz(hz).ok())
                        } else {
                            None
                        };
                        filtered
                            .as_ref()
                            .unwrap_or(&c.series)
                            .samples
                            .iter()
                            .filter_map(|s| {
                                let time = s.time - off;
                                if time < run.start || time > run.end {
                                    return None;
                                }
                                Some(TimedSample {
                                    time,
                                    value: Unit::convert_value(s.value, &c.descriptor.unit, &unit)
                                        .ok()?,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                traces.push(MapTrace {
                    segment: run.key.clone(),
                    name: run.name.clone(),
                    color: run_color(
                        &self.state.selection,
                        self.workspace.reference.as_ref(),
                        &run.key,
                    ),
                    gps: run.gps.clone(),
                    progress: run.progress.clone(),
                    values,
                    interpolation: descriptor
                        .map_or(Interpolation::Linear, |(c, _)| c.descriptor.interpolation),
                    gap_seconds: descriptor.and_then(|(c, _)| c.series.gap_seconds),
                    cursor_time: None,
                    is_reference: self.workspace.reference.as_ref() == Some(&run.key),
                });
            }
            self.map_cache.insert(cache_key.clone(), traces);
        }
        let mut traces = self.map_cache.remove(&cache_key).unwrap();
        for trace in &mut traces {
            trace.cursor_time = self
                .prepared
                .runs
                .iter()
                .find(|r| r.key == trace.segment)
                .and_then(|r| self.run_time(r));
        }
        let mut panel = self.maps.remove(&id).unwrap_or_default();
        let asset_dir = self
            .path
            .as_ref()
            .map(|p| p.with_extension("assets"))
            .unwrap_or_else(crate::app_paths::unsaved_analysis_imagery_dir);
        self.map_header(
            ui, id, channel, settings, units, &mut panel, &traces, &asset_dir,
        );
        let unit = target_unit;
        let label = format!("{} ({})", channel, unit.as_ref().map_or("", Unit::symbol));
        let selected = panel.ui(
            ui,
            &traces,
            self.prepared.course.as_ref(),
            &label,
            settings,
            &asset_dir,
        );
        if let Some(selected) = selected
            && let Some(run) = self
                .prepared
                .runs
                .iter()
                .find(|r| r.key == selected.segment)
            && let Some(x) = self.x_at_time(run, selected.recording_time)
        {
            self.scrub_x(x);
        }
        self.maps.insert(id, panel);
        self.map_cache.insert(cache_key, traces);
        if settings_before != serde_json::to_string(&(&*channel, &*settings)).unwrap_or_default() {
            self.dirty = true;
        }
    }
    /// Color mode, channel, and an options popover for a map panel.
    #[allow(clippy::too_many_arguments)]
    fn map_header(
        &self,
        ui: &mut egui::Ui,
        id: u64,
        channel: &mut String,
        settings: &mut MapSettings,
        units: UnitSystem,
        panel: &mut crate::analysis_maps::MapPanel,
        traces: &[MapTrace],
        asset_dir: &Path,
    ) {
        // Narrow panels fold everything into one menu (a single short row);
        // wide ones show the mode and channel pickers inline.
        if ui.available_width() < 380.0 {
            widgets::popover(ui, "Map ⏷", |ui| {
                self.map_mode_controls(ui, id, channel, settings);
                if settings.color_mode == MapColorMode::Value {
                    self.map_value_options(ui, id, channel, settings, units);
                }
                panel.options_ui(ui, traces, settings, asset_dir);
            });
            return;
        }
        ui.horizontal_wrapped(|ui| {
            self.map_mode_controls(ui, id, channel, settings);
            widgets::popover(ui, "Options ⏷", |ui| {
                if settings.color_mode == MapColorMode::Value {
                    self.map_value_options(ui, id, channel, settings, units);
                }
                panel.options_ui(ui, traces, settings, asset_dir);
            });
        });
    }

    /// Channel-versus-run coloring and the channel picker.
    fn map_mode_controls(
        &self,
        ui: &mut egui::Ui,
        id: u64,
        channel: &mut String,
        settings: &mut MapSettings,
    ) {
        widgets::segmented(
            ui,
            &mut settings.color_mode,
            &[
                (
                    MapColorMode::Value,
                    "Channel",
                    "Color the course by a channel value",
                ),
                (
                    MapColorMode::Run,
                    "Runs",
                    "Give each run its own solid color",
                ),
            ],
        );
        if settings.color_mode == MapColorMode::Value {
            egui::ComboBox::from_id_salt((id, "map-channel"))
                .selected_text(channel.as_str())
                .show_ui(ui, |ui| {
                    for name in self
                        .channel_names()
                        .into_iter()
                        .filter(|name| name != DELTA_CHANNEL)
                    {
                        ui.selectable_value(channel, name.clone(), name);
                    }
                });
        }
    }

    fn map_value_options(
        &self,
        ui: &mut egui::Ui,
        id: u64,
        channel: &str,
        settings: &mut MapSettings,
        units: UnitSystem,
    ) {
        let descriptor = self.prepared.runs.iter().find_map(|r| {
            self.source_channel(r, channel, &PlotOptions::default())
                .map(|(c, _)| c.descriptor.clone())
        });
        widgets::section_label(ui, "Value");
        ui.horizontal(|ui| {
            let mut enabled = settings.low_pass_hz.is_some();
            if ui.checkbox(&mut enabled, "Zero-phase low-pass").changed() {
                settings.low_pass_hz = enabled.then_some(8.0);
            }
            if let Some(hz) = settings.low_pass_hz.as_mut() {
                ui.add(egui::DragValue::new(hz).range(0.1..=100.0).suffix(" Hz"));
            }
        });
        if let Some(d) = &descriptor {
            let default = display_unit(d.unit.clone(), d.quantity.clone(), units);
            let mut target = settings
                .display_unit
                .clone()
                .filter(|u| d.unit.compatible_units().contains(u))
                .unwrap_or_else(|| default.clone());
            ui.horizontal(|ui| {
                ui.label("Unit");
                egui::ComboBox::from_id_salt((id, "map-unit"))
                    .selected_text(target.symbol())
                    .show_ui(ui, |ui| {
                        for unit in d.unit.compatible_units() {
                            ui.selectable_value(&mut target, unit.clone(), unit.symbol());
                        }
                    });
                if ui.small_button("Default").clicked() {
                    target = default.clone();
                }
            });
            settings.display_unit = (target != default).then_some(target);
        }
    }
}

#[cfg(test)]
mod legend_tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn overflow_legend_toggles_the_plot_without_growing_the_panel() {
        let plot_id = egui::Id::new("overflow-legend-test");
        let entries = (0..13)
            .map(|i| (format!("run-{i}"), format!("Run {i}"), egui::Color32::BLUE))
            .collect::<Vec<_>>();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(320.0, 200.0))
            .build_ui(|ui| {
                assert!(legend_overflows(ui, &entries, &BTreeMap::new(), 160.0));
                let response = Plot::new("test")
                    .id(plot_id)
                    .height(160.0)
                    .show(ui, |plot| {
                        for (key, name, color) in &entries {
                            plot.line(
                                Line::new(name, vec![[0.0, 0.0], [1.0, 1.0]])
                                    .color(*color)
                                    .id(trace_item_id(key)),
                            );
                        }
                    });
                overflow_legend(
                    ui,
                    plot_id,
                    *response.transform.frame(),
                    &entries,
                    &BTreeMap::new(),
                );
            });
        harness.run_steps(3);
        harness.get_by_label("Legend (13) ⏷").click();
        harness.run_steps(3);
        harness.get_by_label("Run 0").click();
        harness.run_steps(3);
        let memory = egui_plot::PlotMemory::load(&harness.ctx, plot_id).unwrap();
        assert!(memory.hidden_items.contains(&trace_item_id("run-0")));
        assert!(!memory.hidden_items.contains(&trace_item_id("run-1")));
        assert!(memory.transform().frame().height() <= 160.0);
    }
}
