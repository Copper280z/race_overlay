use super::*;
use crate::analysis_maps::{color_map, sample_value};
use egui_plot::{Legend, Line, Plot, Points, VLine};
use overlay_media::{AnalysisPreviewConfig, PreviewSize};

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
                    ui.colored_label(*color, "●");
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
        ui.collapsing("Plot controls", |ui| {
            ui.horizontal_wrapped(|ui| {
                if !options.delta {
                    ui.menu_button("Channels", |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(250.0)
                            .show(ui, |ui| {
                                for name in self.channel_names() {
                                    let mut selected = options.channels.contains(&name);
                                    let label = if name == DELTA_CHANNEL {
                                        DELTA_LABEL
                                    } else {
                                        &name
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
                let mut enabled = options.filter.is_some();
                if ui.checkbox(&mut enabled, "Panel zero-phase LPF").changed() {
                    options.filter = enabled.then_some(8.0);
                }
                if let Some(hz) = options.filter.as_mut() {
                    ui.add(egui::DragValue::new(hz).range(0.1..=100.0).suffix(" Hz"));
                }
            });
            if !options.delta {
                ui.collapsing("Units and channel matching", |ui| {
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
                            let mut target =
                                options.units.get(&name).cloned().unwrap_or(default.clone());
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
                                        if let Some(d) = self.data.get(&source.id) {
                                            for channel in d.processed.channels.values() {
                                                if ui
                                                    .selectable_label(
                                                        false,
                                                        format!(
                                                            "{} / {} ({})",
                                                            source.name,
                                                            channel.descriptor.name,
                                                            channel.descriptor.unit.symbol()
                                                        ),
                                                    )
                                                    .clicked()
                                                {
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
                                    }
                                });
                        }
                    }
                });
            }
            legend_editor(
                ui,
                &mut options.show_legend,
                &mut options.legend_labels,
                &legend_entries,
            );
        });
        if before != serde_json::to_string(options).unwrap_or_default() {
            self.dirty = true;
            self.plot_cache.clear();
        }
        if options.delta && self.prepared.course.is_none() {
            ui.label("Time gain/loss needs a reference course and matching GPS coverage.");
            return;
        }
        let channels = if options.delta {
            vec![DELTA_CHANNEL.to_string()]
        } else {
            options.channels.clone()
        };
        let height = (ui.available_height() / channels.len().max(1) as f32 - 10.0).max(140.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            for name in channels {
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
                    let traces = self.build_plot(&name, options, units);
                    self.plot_cache.insert(cache_key.clone(), Arc::new(traces));
                }
                let traces = self.plot_cache.get(&cache_key).unwrap().clone();
                let unit = traces.first().map_or("", |t| t.unit.symbol());
                let display_name = if name == DELTA_CHANNEL {
                    DELTA_LABEL
                } else {
                    &name
                };
                ui.label(format!("{display_name} ({unit})"));
                if traces.is_empty() {
                    ui.weak("No compatible channel / valid alignment for selected runs.");
                    continue;
                }
                let cursor = self
                    .reference_run()
                    .and_then(|r| self.x_at_time(r, r.start + self.state.cursor));
                let x_label = match self.effective_mode() {
                    XMode::Time
                        if automatic_alignment_complete(
                            &self.prepared,
                            self.workspace.reference.as_ref(),
                        ) =>
                    {
                        "Correlation-aligned seconds"
                    }
                    XMode::Time => "Elapsed seconds",
                    XMode::Distance => "Each run's traveled meters",
                    XMode::Course => "Reference-course meters",
                };
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
                if options.show_legend {
                    plot_widget = plot_widget.legend(Legend::default());
                }
                let response = plot_widget.show(ui, |plot| {
                    for trace in traces.iter() {
                        let label = options
                            .legend_labels
                            .get(&segment_key(&trace.segment))
                            .filter(|label| !label.trim().is_empty())
                            .unwrap_or(&trace.name);
                        for piece in &trace.points {
                            if piece.len() > 1 {
                                plot.line(Line::new(label, piece.clone()).color(trace.color));
                            }
                        }
                    }
                    if let Some(x) = cursor {
                        plot.vline(VLine::new("playhead", x).color(egui::Color32::WHITE));
                    }
                    if let Some(range) = self.state.range
                        && let Some(r) = self.reference_run()
                    {
                        for t in range {
                            if let Some(x) = self.x_at_time(r, r.start + t) {
                                plot.vline(VLine::new("range", x).color(egui::Color32::GRAY));
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
                response.response.context_menu(|ui| {
                    ui.checkbox(&mut options.show_legend, "Show legend");
                    ui.separator();
                    ui.label("Legend text");
                    for (key, default, color) in &legend_entries {
                        ui.horizontal(|ui| {
                            ui.colored_label(*color, "●");
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
            let (samples, unit, gap) = if options.delta || name == DELTA_CHANNEL {
                let Some(reference) = self.reference_run() else {
                    continue;
                };
                let reference_window = self.delta_window(reference);
                let run_window = self.delta_window(run);
                let mut values = reference
                    .progress
                    .iter()
                    .filter(|point| {
                        point.recording_time >= reference_window.0
                            && point.recording_time <= reference_window.1
                    })
                    .map(|p| {
                        let other = if run.key == reference.key {
                            Some(p.recording_time)
                        } else {
                            time_at_progress(&run.progress, p.progress)
                                .filter(|time| *time >= run_window.0 && *time <= run_window.1)
                        };
                        let x = self.x_at_time(reference, p.recording_time);
                        let value = if p.confidence > 0.0 {
                            x.zip(other).map(|(x, other)| {
                                [
                                    x,
                                    (other - run_window.0)
                                        - (p.recording_time - reference_window.0),
                                ]
                            })
                        } else {
                            None
                        };
                        (p.recording_time, value.unwrap_or([f64::NAN; 2]))
                    })
                    .collect::<Vec<_>>();
                if !self.workspace.course.gates.is_empty()
                    && let Some(baseline) = values
                        .iter()
                        .find_map(|(_, point)| point[1].is_finite().then_some(point[1]))
                {
                    // Gate-defined runs share a meaningful start line. Rebase
                    // the comparison at the first jointly covered course
                    // position so GPS sample cadence cannot introduce a
                    // non-zero delta at the start gate.
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
            let mut piece = vec![];
            let mut previous = None;
            for (t, value) in samples {
                if !value.iter().all(|v| v.is_finite()) || previous.is_some_and(|p| t - p > gap) {
                    if piece.len() > 1 {
                        pieces.push(std::mem::take(&mut piece));
                    } else {
                        piece.clear();
                    }
                }
                if !value.iter().all(|v| v.is_finite()) {
                    previous = None;
                    continue;
                }
                piece.push(value);
                previous = Some(t);
            }
            if piece.len() > 1 {
                pieces.push(piece);
            }
            result.push(PlotTrace {
                segment: run.key.clone(),
                name: run.name.clone(),
                color: run_color(
                    &self.state.selection,
                    self.workspace.reference.as_ref(),
                    &run.key,
                ),
                points: pieces,
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
        ui.collapsing("Scatter controls", |ui| {
            ui.horizontal_wrapped(|ui| {
                channel_combo(ui, (id, "scatter-x"), "X", &mut options.x_channel, &names);
                channel_combo(ui, (id, "scatter-y"), "Y", &mut options.y_channel, &names);
                let mut use_z = options.z_channel.is_some();
                if ui.checkbox(&mut use_z, "Color by Z").changed() {
                    options.z_channel = use_z.then(|| "gps_speed".into());
                }
                if let Some(z) = options.z_channel.as_mut() {
                    channel_combo(ui, (id, "scatter-z"), "Z", z, &names);
                }
                let mut filter = options.filter.is_some();
                if ui.checkbox(&mut filter, "Zero-phase LPF").changed() {
                    options.filter = filter.then_some(8.0);
                }
                if let Some(hz) = options.filter.as_mut() {
                    ui.add(egui::DragValue::new(hz).range(0.1..=100.0).suffix(" Hz"));
                }
            });
            legend_editor(
                ui,
                &mut options.show_legend,
                &mut options.legend_labels,
                &legend_entries,
            );
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
        if options.show_legend {
            plot_widget = plot_widget.legend(Legend::default());
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
                    plot.points(Points::new(label, points).radius(2.0).color(trace.color));
                }
            }
        });
        response.response.context_menu(|ui| {
            ui.checkbox(&mut options.show_legend, "Show legend");
            ui.separator();
            ui.label("Legend text");
            for (key, default, color) in &legend_entries {
                ui.horizontal(|ui| {
                    ui.colored_label(*color, "●");
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
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt((id, "video-lap"))
                .selected_text(
                    key.as_ref()
                        .and_then(|k| self.prepared.runs.iter().find(|r| &r.key == k))
                        .map_or("Choose lap / run", |r| r.name.as_str()),
                )
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
                });
            ui.checkbox(&mut options.linked, "Linked");
            if !options.linked {
                ui.add(
                    egui::DragValue::new(&mut options.time)
                        .speed(0.02)
                        .prefix("Video s "),
                );
            }
        });
        let Some(key) = key else {
            ui.label("Select a lap/run in the browser.");
            return;
        };
        let Some(run) = self.prepared.runs.iter().find(|r| r.key == key) else {
            ui.label("Preparing lap…");
            return;
        };
        let Some(recording) = self
            .workspace
            .recordings
            .iter()
            .find(|r| r.id == key.recording_id)
        else {
            return;
        };
        let Some(path) = recording
            .video_path
            .clone()
            .filter(|p| !p.as_os_str().is_empty())
        else {
            ui.label("No video attached. Telemetry is ready to compare.");
            return;
        };
        let Some(tools) = tools else {
            ui.label("FFmpeg unavailable. Telemetry analysis is unaffected.");
            return;
        };
        let timestamp = if options.linked {
            self.run_time(run).map(|t| recording.video_time(t))
        } else {
            Some(options.time)
        };
        let Some(timestamp) = timestamp.filter(|t| t.is_finite() && *t >= 0.0) else {
            self.videos.remove(&id);
            ui.label("No video / alignment coverage at this position.");
            return;
        };
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
                ui.colored_label(egui::Color32::LIGHT_RED, e);
                return;
            }
            None => {
                ui.spinner();
                return;
            }
        };
        if metadata.duration.is_some_and(|d| timestamp >= d) {
            self.videos.remove(&id);
            ui.label("Video ends before this position.");
            return;
        }
        if options.linked {
            options.time = timestamp;
        }
        ui.small(format!("{} · exported video {:.3}s", run.name, timestamp));
        let width = ((ui.available_width().clamp(160.0, 960.0) as u32) / 32 * 32).max(160);
        let height =
            (width as f64 * metadata.height as f64 / metadata.width.max(1) as f64).round() as u32;
        let height = height.max(2);
        let size = PreviewSize::new(width, height);
        if self.videos.get(&id).is_none_or(|v| v.path != path) {
            self.videos.insert(
                id,
                VideoRuntime {
                    path: path.clone(),
                    decoder: AnalysisPreview::spawn_with_config(
                        tools.clone(),
                        path,
                        AnalysisPreviewConfig {
                            output_fps: 30.0,
                            source_fps: metadata.fps(),
                        },
                    ),
                    texture: None,
                    requested: None,
                    error: None,
                },
            );
        }
        let runtime = self.videos.get_mut(&id).unwrap();
        if runtime
            .requested
            .is_none_or(|(t, w, h)| (t - timestamp).abs() > 1.0 / 60.0 || w != width || h != height)
        {
            if let Err(e) = runtime.decoder.request(timestamp, size) {
                runtime.error = Some(e.to_string());
            }
            runtime.requested = Some((timestamp, width, height));
        }
        while let Some(result) = runtime.decoder.try_recv() {
            match result {
                Ok(frame) => {
                    if (frame.timestamp - timestamp).abs() < 0.3 {
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
                Err(e) => runtime.error = Some(e.to_string()),
            }
        }
        if let Some(error) = &runtime.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        if let Some(texture) = &runtime.texture {
            ui.add(
                egui::Image::new(texture)
                    .max_size(ui.available_size())
                    .maintain_aspect_ratio(true),
            );
        } else {
            ui.spinner();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
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
        ui.horizontal(|ui| {
            let label = if settings.controls_expanded {
                "▼ Map controls"
            } else {
                "▶ Map controls"
            };
            if ui
                .selectable_label(settings.controls_expanded, label)
                .clicked()
            {
                settings.controls_expanded = !settings.controls_expanded;
            }
            ui.weak(match settings.color_mode {
                MapColorMode::Value => format!("Channel colormap · {channel}"),
                MapColorMode::Run => "Solid color by run".into(),
            });
        });
        if settings.controls_expanded {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt((id, "map-color-mode"))
                    .selected_text(match settings.color_mode {
                        MapColorMode::Value => "Channel colormap",
                        MapColorMode::Run => "Solid color by run",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut settings.color_mode,
                            MapColorMode::Value,
                            "Channel colormap",
                        );
                        ui.selectable_value(
                            &mut settings.color_mode,
                            MapColorMode::Run,
                            "Solid color by run",
                        );
                    });
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
                    let descriptor = self.prepared.runs.iter().find_map(|r| {
                        self.source_channel(r, channel, &PlotOptions::default())
                            .map(|(c, _)| c.descriptor.clone())
                    });
                    ui.horizontal_wrapped(|ui| {
                        let mut enabled = settings.low_pass_hz.is_some();
                        if ui
                            .checkbox(&mut enabled, "Map value zero-phase LPF")
                            .changed()
                        {
                            settings.low_pass_hz = enabled.then_some(8.0);
                        }
                        if let Some(hz) = settings.low_pass_hz.as_mut() {
                            ui.add(egui::DragValue::new(hz).range(0.1..=100.0).suffix(" Hz"));
                        }
                        if let Some(d) = &descriptor {
                            let default = display_unit(d.unit.clone(), d.quantity.clone(), units);
                            let mut target = settings
                                .display_unit
                                .clone()
                                .filter(|u| d.unit.compatible_units().contains(u))
                                .unwrap_or_else(|| default.clone());
                            egui::ComboBox::from_id_salt((id, "map-unit"))
                                .selected_text(target.symbol())
                                .show_ui(ui, |ui| {
                                    for unit in d.unit.compatible_units() {
                                        ui.selectable_value(
                                            &mut target,
                                            unit.clone(),
                                            unit.symbol(),
                                        );
                                    }
                                });
                            settings.display_unit = (target != default).then_some(target);
                            if ui.small_button("Default unit").clicked() {
                                settings.display_unit = None;
                            }
                        }
                    });
                }
            });
        }
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
            .unwrap_or_else(|| PathBuf::from("analysis.race-analysis.assets"));
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
    pub(super) fn statistics(&mut self, ui: &mut egui::Ui, units: UnitSystem) {
        ui.label("Values at the linked playhead; min/max/mean over the selected reference range (or the full run).");
        let options = PlotOptions::default();
        let channels = [
            "gps_speed",
            "rpm",
            "water_temperature",
            "exhaust_temperature",
            "gps_lateral_acceleration",
            "gps_inline_acceleration",
        ];
        egui::ScrollArea::both().show(ui, |ui| {
            for run in &self.prepared.runs {
                if !self.state.selection.contains(&run.key) {
                    continue;
                }
                ui.colored_label(
                    run_color(
                        &self.state.selection,
                        self.workspace.reference.as_ref(),
                        &run.key,
                    ),
                    &run.name,
                );
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
                    ui.weak("Selected range is outside this run's matched coverage.");
                    continue;
                };
                for name in channels {
                    let Some((channel, off)) = self.source_channel(run, name, &options) else {
                        continue;
                    };
                    let target = display_unit(
                        channel.descriptor.unit.clone(),
                        channel.descriptor.quantity.clone(),
                        units,
                    );
                    let convert =
                        |v| Unit::convert_value(v, &channel.descriptor.unit, &target).unwrap_or(v);
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
                            let start = samples.partition_point(|s| s.time - off < range.0);
                            let end = samples.partition_point(|s| s.time - off <= range.1);
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
                    ui.label(format!(
                        "{name}: {} {}",
                        value.map_or("—".into(), |v| format!("{v:.2}")),
                        target.symbol()
                    ));
                    if n > 0 {
                        ui.small(format!(
                            "min {min:.2} · max {max:.2} · mean {:.2}",
                            sum / n as f64
                        ));
                    }
                }
                ui.separator();
            }
        });
    }
}
