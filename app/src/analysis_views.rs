use super::*;
use crate::analysis_maps::sample_value;
use egui_plot::{Legend, Line, Plot, VLine};
use overlay_media::{AnalysisPreviewConfig, PreviewSize};

impl AnalysisApp {
    pub(super) fn plot(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        options: &mut PlotOptions,
        units: UnitSystem,
    ) {
        let before = serde_json::to_string(options).unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            if !options.delta {
                ui.menu_button("Channels", |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(250.0)
                        .show(ui, |ui| {
                            for name in self.channel_names() {
                                let mut selected = options.channels.contains(&name);
                                if ui.checkbox(&mut selected, &name).changed() {
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
        if before != serde_json::to_string(options).unwrap_or_default() {
            self.dirty = true;
            self.plot_cache.clear();
        }
        if options.delta && self.prepared.course.is_none() {
            ui.label("Time gain/loss needs a reference course and matching GPS coverage.");
            return;
        }
        let channels = if options.delta {
            vec!["Time delta — positive is slower".to_string()]
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
                ui.label(format!("{name} ({unit})"));
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
                let response = Plot::new((id, &name, self.effective_mode().label()))
                    .height(height)
                    .legend(Legend::default())
                    .link_axis(
                        egui::Id::new(("analysis-x", self.effective_mode().label())),
                        [true, false],
                    )
                    .x_axis_label(x_label)
                    .show(ui, |plot| {
                        for trace in traces.iter() {
                            for piece in &trace.points {
                                if piece.len() > 1 {
                                    plot.line(
                                        Line::new(&trace.name, piece.clone()).color(trace.color),
                                    );
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
                if let Some(x) = response.inner {
                    self.scrub_x(x);
                }
            }
        });
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
            let (samples, unit, gap) = if options.delta {
                let Some(reference) = self.reference_run() else {
                    continue;
                };
                let mut values = reference
                    .progress
                    .iter()
                    .map(|p| {
                        let other = if run.key == reference.key {
                            Some(p.recording_time)
                        } else {
                            time_at_progress(&run.progress, p.progress)
                        };
                        let x = self.x_at_time(reference, p.recording_time);
                        let value = if p.confidence > 0.0 {
                            x.zip(other).map(|(x, other)| {
                                [
                                    x,
                                    (other - run.start) - (p.recording_time - reference.start),
                                ]
                            })
                        } else {
                            None
                        };
                        (p.recording_time, value.unwrap_or([f64::NAN; 2]))
                    })
                    .collect::<Vec<_>>();
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
                let values = series
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
                name: run.name.clone(),
                color: run.color,
                points: pieces,
                unit,
            });
        }
        result
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
        egui::ComboBox::from_id_salt((id, "map-channel"))
            .selected_text(channel.as_str())
            .show_ui(ui, |ui| {
                for name in self.channel_names() {
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
                            ui.selectable_value(&mut target, unit.clone(), unit.symbol());
                        }
                    });
                settings.display_unit = (target != default).then_some(target);
                if ui.small_button("Default unit").clicked() {
                    settings.display_unit = None;
                }
            }
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
                    color: run.color,
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
                ui.colored_label(run.color, &run.name);
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
