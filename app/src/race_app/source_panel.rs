//! Telemetry source selection, configuration, and channel correlation UI.
use super::super::policy::{
    dataset_duration, forward_axis_label, set_source_low_pass_settings, source_low_pass_settings,
};
use super::{OverlayEditor, PanelAction};
use eframe::egui;
use overlay_core::{ChannelRef, SourceId};

impl OverlayEditor {
    pub(in crate::race_app) fn sources_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Data sources");
        ui.horizontal_wrapped(|ui| {
            if ui.button("+ Camera telemetry").clicked() {
                self.add_file_source("insta360");
            }
            if ui.button("+ CSV").clicked() {
                self.add_file_source("generic_csv");
            }
            if ui.button("+ MyChron XRK").clicked() {
                self.add_file_source("aim_xrk");
            }
            if ui.button("+ Synthetic").clicked() {
                self.add_synthetic();
            }
        });
        let sources = self
            .project()
            .map(|p| p.sources.clone())
            .unwrap_or_default();
        for source in &sources {
            let loaded = self
                .session
                .datasets()
                .iter()
                .any(|d| d.source_id == source.id);
            if ui
                .selectable_label(
                    self.source_editor.selected_source == Some(source.id),
                    format!("{} {}", if loaded { "●" } else { "○" }, source.name),
                )
                .clicked()
            {
                self.apply_panel_action(PanelAction::SelectSource(source.id));
                self.source_editor.remove_source_confirm = None;
                self.invalidate_correlation();
            }
        }
        let mut remove_source = None;
        if let Some(source_id) = self.source_editor.selected_source {
            if let Some(source) = sources.iter().find(|source| source.id == source_id) {
                ui.separator();
                ui.label(format!("Adapter: {}", source.adapter));
                if !source.path.as_os_str().is_empty() {
                    ui.small(source.path.display().to_string());
                }
                if ui
                    .button(format!("Remove source \u{201c}{}\u{201d}", source.name))
                    .clicked()
                {
                    self.source_editor.remove_source_confirm = Some(source.id);
                }
                if self.source_editor.remove_source_confirm == Some(source.id) {
                    ui.group(|ui| {
                        ui.colored_label(
                            egui::Color32::YELLOW,
                            format!(
                                "Remove \u{201c}{}\u{201d}? Its telemetry, plot selections, sync state, and widget bindings will be removed; the video and widgets stay.",
                                source.name
                            ),
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Remove selected source").clicked() {
                                remove_source = Some(source.id);
                            }
                            if ui.button("Keep source").clicked() {
                                self.source_editor.remove_source_confirm = None;
                            }
                        });
                    });
                }
                if source.adapter == "aim_xrk"
                    && let Some(dataset) = self
                        .session
                        .datasets()
                        .iter()
                        .find(|dataset| dataset.source_id == source.id)
                {
                    let venue = dataset
                        .metadata
                        .get("venue")
                        .map(String::as_str)
                        .unwrap_or("unknown venue");
                    ui.small(format!(
                        "{} lap marker(s) · {} · {} channels",
                        dataset.laps.len(),
                        venue,
                        dataset.channels.len()
                    ));
                }
                let syncing = self.source_editor.syncing_sources.contains(&source.id);
                let source_duration = self
                    .session
                    .datasets()
                    .iter()
                    .find(|dataset| dataset.source_id == source.id)
                    .and_then(dataset_duration);
                if source_duration.is_some_and(|duration| {
                    duration > self.duration() + 5.0 && source.alignment.offset_seconds.abs() < 1.0
                }) {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        if syncing {
                            "Aligning the raw recording to the exported video…"
                        } else {
                            "Automatic alignment is unresolved; derived data may not match the video."
                        },
                    );
                }
                if source.adapter == "insta360"
                    && ui
                        .add_enabled(!syncing, egui::Button::new("Re-sync from audio"))
                        .clicked()
                {
                    self.auto_sync();
                }
                ui.collapsing("Advanced timing", |ui| {
                    ui.label("Normal controls always use exported-video time.");
                    let mut offset = source.alignment.offset_seconds;
                    if ui
                        .add(
                            egui::DragValue::new(&mut offset)
                                .speed(0.01)
                                .prefix("Raw source offset ")
                                .suffix(" s"),
                        )
                        .changed()
                    {
                        if let Some(config) = self.session.source_mut(source_id) {
                            config.alignment.offset_seconds = offset;
                        }
                        self.invalidate_correlation();
                        self.refresh_overlay();
                    }
                    ui.small(format!("Video 0.000 s → raw source {offset:.3} s"));
                    ui.checkbox(
                        &mut self.source_editor.calibration_source_time,
                        "Calibration interval uses raw source time",
                    );
                });
                let (mut low_pass_enabled, mut low_pass_hz) =
                    source_low_pass_settings(&source.settings);
                let mut low_pass_changed = false;
                ui.collapsing("Source low-pass (all imported continuous channels)", |ui| {
                    if source.adapter == "insta360" {
                        ui.small("Zero-phase, non-causal smoothing before camera-derived signals are calculated. Discrete channels are unchanged.");
                    } else {
                        ui.small("Zero-phase, non-causal smoothing for every imported continuous channel. Discrete channels such as gear are unchanged.");
                    }
                    ui.small("The displayed cutoff is the final two-pass -3 dB point. Enabled source, derived-G, and widget filters cascade.");
                    low_pass_changed |= ui
                        .checkbox(&mut low_pass_enabled, "Enable source smoothing")
                        .changed();
                    if low_pass_enabled {
                        low_pass_changed |= ui
                            .add(
                                egui::Slider::new(&mut low_pass_hz, 0.5..=50.0)
                                    .logarithmic(true)
                                    .text("Cutoff Hz"),
                            )
                            .changed();
                    }
                });
                if low_pass_changed && let Some(config) = self.session.source_mut(source_id) {
                    set_source_low_pass_settings(
                        &mut config.settings,
                        low_pass_enabled,
                        low_pass_hz,
                    );
                    self.status =
                        "Source smoothing changed; click Reload & apply source smoothing".into();
                }
                if ui.button("Reload & apply source smoothing").clicked() {
                    self.apply_source_low_pass(source.id);
                }
                if source.adapter != "insta360" {
                    self.correlation_ui(ui, source.id);
                }
            }
            if let Some(source) = sources.iter().find(|item| item.id == source_id)
                && source.adapter == "insta360"
            {
                ui.collapsing("Camera calibration", |ui| {
                ui.label("Choose a stationary interval. Then select the camera sensor axis that points toward the kart nose; gravity determines up.");
                ui.small(if self.source_editor.calibration_source_time {
                    "Interval below is raw source time (advanced mode)."
                } else {
                    "Interval below is exported-video time; negative values are allowed."
                });
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.source_editor.calibration_start)
                            .speed(0.1)
                            .prefix("From ")
                            .suffix(" s"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.source_editor.calibration_end)
                            .speed(0.1)
                            .prefix("to ")
                            .suffix(" s"),
                    );
                });
                ui.checkbox(
                    &mut self.source_editor.calibration_auto_stationary,
                    "Auto-find a stationary interval when this range is moving",
                );
                egui::ComboBox::from_label("Camera-forward axis")
                    .selected_text(forward_axis_label(self.source_editor.calibration_forward_axis))
                    .show_ui(ui, |ui| {
                        for axis in 0..6 {
                            ui.selectable_value(
                                &mut self.source_editor.calibration_forward_axis,
                                axis,
                                forward_axis_label(axis),
                            );
                        }
                    });
                ui.add(
                    egui::Slider::new(&mut self.source_editor.calibration_roll_deg, -30.0..=30.0)
                        .step_by(0.1)
                        .text("Fine roll trim°"),
                );
                ui.add(
                    egui::Slider::new(&mut self.source_editor.calibration_pitch_deg, -30.0..=30.0)
                        .step_by(0.1)
                        .text("Fine pitch trim°"),
                );
                ui.add(
                    egui::Slider::new(&mut self.source_editor.calibration_yaw_deg, -90.0..=90.0)
                        .step_by(0.1)
                        .text("Fine yaw trim°"),
                );
                ui.small("Positive pitch raises the nose; positive roll raises the left side; positive yaw turns toward the left.");
                ui.add(
                    egui::Slider::new(&mut self.source_editor.calibration_low_pass_hz, 0.5..=50.0)
                        .logarithmic(true)
                        .text("Derived G smoothing cutoff Hz"),
                );
                ui.small("Zero-phase, non-causal smoothing during calibration for derived longitudinal, lateral, and vertical G; combined G uses those results. The displayed cutoff is the final two-pass -3 dB point. Turn-rate channels are unchanged.");
                if source_low_pass_settings(&source.settings).0 {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        "Source smoothing is also enabled, so the two smoothing stages cascade for derived G.",
                    );
                }
                if ui.button("Apply calibration & recalculate derived G").clicked() {
                    self.calibrate_selected();
                }
                });
            }
            if let Some(source) = sources.iter().find(|item| item.id == source_id) {
                let channels = self
                    .session
                    .datasets()
                    .iter()
                    .find(|dataset| dataset.source_id == source.id)
                    .map(|dataset| {
                        dataset
                            .channels
                            .values()
                            .map(|channel| {
                                (
                                    ChannelRef {
                                        source_id: source.id,
                                        channel_id: channel.descriptor.id,
                                    },
                                    format!(
                                        "{} ({})",
                                        channel.descriptor.name,
                                        channel.descriptor.unit.symbol()
                                    ),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                ui.collapsing("Plot channels", |ui| {
                    for (reference, label) in channels {
                        let mut selected = self.plot_editor.plot_channels.contains(&reference);
                        if ui.checkbox(&mut selected, label).changed() {
                            if selected {
                                self.plot_editor.plot_channels.push(reference);
                            } else {
                                self.plot_editor
                                    .plot_channels
                                    .retain(|item| *item != reference);
                            }
                        }
                    }
                    if ui.button("Open data graph").clicked() {
                        self.plot_editor.show_data_plot = true;
                    }
                });
            }
        }
        if let Some(source_id) = remove_source {
            self.remove_source(source_id);
        }
    }

    pub(super) fn correlation_ui(&mut self, ui: &mut egui::Ui, target_source: SourceId) {
        let target_choices = self.channel_choices_for_source(target_source);
        let reference_sources = self
            .project()
            .map(|project| {
                project
                    .sources
                    .iter()
                    .filter(|source| {
                        source.id != target_source
                            && self
                                .session
                                .datasets()
                                .iter()
                                .any(|dataset| dataset.source_id == source.id)
                    })
                    .map(|source| (source.id, source.name.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if target_choices.is_empty() || reference_sources.is_empty() {
            ui.collapsing("Correlate to another source", |ui| {
                ui.small(
                    "Load a channel from this source and another source to enable correlation.",
                );
            });
            return;
        }

        if self
            .source_editor
            .correlation_target_channel
            .as_ref()
            .is_none_or(|reference| {
                reference.source_id != target_source
                    || !target_choices.iter().any(|(item, _)| item == reference)
            })
        {
            self.source_editor.correlation_target_channel =
                target_choices.first().map(|(item, _)| item.clone());
        }
        if self
            .source_editor
            .correlation_reference_source
            .is_none_or(|source| !reference_sources.iter().any(|(id, _)| *id == source))
        {
            self.source_editor.correlation_reference_source =
                reference_sources.first().map(|(id, _)| *id);
            self.source_editor.correlation_reference_channel = None;
        }
        let reference_source = self.source_editor.correlation_reference_source;
        let reference_choices = reference_source
            .map(|source_id| self.channel_choices_for_source(source_id))
            .unwrap_or_default();
        if self
            .source_editor
            .correlation_reference_channel
            .as_ref()
            .is_none_or(|reference| {
                Some(reference.source_id) != reference_source
                    || !reference_choices.iter().any(|(item, _)| item == reference)
            })
        {
            self.source_editor.correlation_reference_channel =
                reference_choices.first().map(|(item, _)| item.clone());
        }
        let sample_count = |reference: Option<&ChannelRef>| {
            reference.and_then(|reference| {
                self.session
                    .datasets()
                    .iter()
                    .find(|dataset| dataset.source_id == reference.source_id)
                    .and_then(|dataset| dataset.channel(reference.channel_id))
                    .map(|channel| channel.series.samples.len())
            })
        };
        let target_sample_count =
            sample_count(self.source_editor.correlation_target_channel.as_ref());
        let reference_sample_count =
            sample_count(self.source_editor.correlation_reference_channel.as_ref());

        let mut run = false;
        let mut apply = false;
        ui.collapsing("Correlate to another source", |ui| {
            ui.small("Estimate a time adjustment by matching two telemetry channels. Estimating never changes the project; use Apply candidate to persist the selected source offset.");
            let target_label = self.source_editor
                .correlation_target_channel
                .as_ref()
                .and_then(|reference| target_choices.iter().find(|(item, _)| item == reference))
                .map(|(_, label)| label.as_str())
                .unwrap_or("Select target channel");
            ui.label("Target channel");
            egui::ComboBox::from_id_salt(("correlation-target-channel", target_source))
                .selected_text(target_label)
                .show_ui(ui, |ui| {
                    for (reference, label) in &target_choices {
                        if ui
                            .selectable_label(
                                self.source_editor.correlation_target_channel.as_ref() == Some(reference),
                                label,
                            )
                            .clicked()
                        {
                            self.source_editor.correlation_target_channel = Some(reference.clone());
                            self.invalidate_correlation();
                        }
                    }
                });

            let reference_source_label = reference_source
                .and_then(|source| reference_sources.iter().find(|(id, _)| *id == source))
                .map(|(_, name)| name.as_str())
                .unwrap_or("Select reference source");
            ui.label("Reference source");
            egui::ComboBox::from_id_salt(("correlation-reference-source", target_source))
                .selected_text(reference_source_label)
                .show_ui(ui, |ui| {
                    for (source, name) in &reference_sources {
                        if ui
                            .selectable_label(reference_source == Some(*source), name)
                            .clicked()
                        {
                            self.source_editor.correlation_reference_source = Some(*source);
                            self.source_editor.correlation_reference_channel = self
                                .channel_choices_for_source(*source)
                                .first()
                                .map(|(item, _)| item.clone());
                            self.invalidate_correlation();
                        }
                    }
                });
            let reference_label = self.source_editor
                .correlation_reference_channel
                .as_ref()
                .and_then(|reference| reference_choices.iter().find(|(item, _)| item == reference))
                .map(|(_, label)| label.as_str())
                .unwrap_or("Select reference channel");
            ui.label("Reference channel");
            egui::ComboBox::from_id_salt(("correlation-reference-channel", target_source))
                .selected_text(reference_label)
                .show_ui(ui, |ui| {
                    for (reference, label) in &reference_choices {
                        if ui
                            .selectable_label(
                                self.source_editor.correlation_reference_channel.as_ref() == Some(reference),
                                label,
                            )
                            .clicked()
                        {
                            self.source_editor.correlation_reference_channel = Some(reference.clone());
                            self.invalidate_correlation();
                        }
                    }
                });
            if target_sample_count.is_some_and(|count| count < 2) {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "The selected target channel has fewer than two samples; choose a recorded sensor channel.",
                );
            }
            if reference_sample_count.is_some_and(|count| count < 24) {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "The selected reference channel has fewer than 24 samples; choose a denser sensor channel.",
                );
            }
            if ui
                .checkbox(
                    &mut self.source_editor.correlation_all_time,
                    "Search all feasible overlap",
                )
                .changed()
            {
                self.invalidate_correlation();
            }
            ui.add_enabled_ui(!self.source_editor.correlation_all_time, |ui| {
                ui.horizontal(|ui| {
                    let min_changed = ui
                        .add(
                            egui::DragValue::new(
                                &mut self.source_editor.correlation_min_adjustment,
                            )
                            .speed(0.05)
                            .prefix("Adjustment from ")
                            .suffix(" s"),
                        )
                        .changed();
                    let max_changed = ui
                        .add(
                            egui::DragValue::new(
                                &mut self.source_editor.correlation_max_adjustment,
                            )
                            .speed(0.05)
                            .prefix("to ")
                            .suffix(" s"),
                        )
                        .changed();
                    if min_changed || max_changed {
                        self.invalidate_correlation();
                    }
                });
            });
            ui.small(if self.source_editor.correlation_all_time {
                "The estimator searches every lag with sufficient real overlap."
            } else {
                "The search window is relative to the target source's current alignment; negative adjustments are allowed."
            });
            ui.small("If this window extends beyond the two channels' available time spans, it is automatically capped to the portion that can overlap.");
            if ui
                .checkbox(
                &mut self.source_editor.correlation_absolute,
                "Prefer strongest absolute correlation (also allows inverted signals)",
            )
                .changed()
            {
                self.invalidate_correlation();
            }
            ui.horizontal(|ui| {
                let ready = self.source_editor.correlation_target_channel.is_some()
                    && self.source_editor.correlation_reference_channel.is_some()
                    && (self.source_editor.correlation_all_time
                        || self.source_editor.correlation_min_adjustment
                            <= self.source_editor.correlation_max_adjustment)
                    && target_sample_count.is_some_and(|count| count >= 2)
                    && reference_sample_count.is_some_and(|count| count >= 24);
                if ui
                    .add_enabled(
                        ready && !self.source_editor.correlation_running,
                        egui::Button::new("Estimate correlation"),
                    )
                    .clicked()
                {
                    run = true;
                }
                if self.source_editor.correlation_running {
                    ui.spinner();
                    ui.label("Estimating…");
                }
            });
            if let Some(estimate) = &self.source_editor.correlation_result
                && estimate.target_source == target_source
                && self.source_editor.correlation_target_channel.as_ref() == Some(&estimate.target_channel)
                && self.source_editor.correlation_reference_source == Some(estimate.reference_source)
                && self.source_editor.correlation_reference_channel.as_ref() == Some(&estimate.reference_channel)
            {
                ui.group(|ui| {
                    ui.label(format!(
                        "Candidate: current target {:+.3}s → {:+.3}s",
                        estimate.target_current_offset_seconds,
                        estimate.target_offset_seconds
                    ));
                    ui.label(format!(
                        "Adjustment {:+.3}s · raw lag {:+.3}s · reference offset {:+.3}s",
                        estimate.adjustment_seconds,
                        estimate.raw_lag_seconds,
                        estimate.reference_offset_seconds
                    ));
                    ui.label(format!(
                        "Pearson r {:+.3} · {:.2}s overlap · {} samples · {:.3}s resolution",
                        estimate.coefficient,
                        estimate.overlap_seconds,
                        estimate.samples,
                        estimate.resolution_seconds
                    ));
                    if ui.button("Apply candidate offset").clicked() {
                        apply = true;
                    }
                });
            }
        });
        if run {
            self.start_correlation(target_source);
        }
        if apply {
            self.apply_correlation_candidate(target_source);
        }
    }

    pub(super) fn channel_choices_for_source(
        &self,
        source_id: SourceId,
    ) -> Vec<(ChannelRef, String)> {
        self.session
            .datasets()
            .iter()
            .find(|dataset| dataset.source_id == source_id)
            .map(|dataset| {
                let mut channels = dataset.channels.values().collect::<Vec<_>>();
                // Avoid arbitrary UUID-map ordering: dense continuous traces
                // are the safest correlation defaults, while sparse lap
                // metadata remains available when explicitly selected.
                channels.sort_by(|left, right| {
                    let left_continuous =
                        left.descriptor.interpolation == overlay_core::Interpolation::Linear;
                    let right_continuous =
                        right.descriptor.interpolation == overlay_core::Interpolation::Linear;
                    right_continuous
                        .cmp(&left_continuous)
                        .then_with(|| right.series.samples.len().cmp(&left.series.samples.len()))
                        .then_with(|| left.descriptor.name.cmp(&right.descriptor.name))
                });
                channels
                    .into_iter()
                    .map(|channel| {
                        let unit = channel.descriptor.unit.symbol();
                        let name_and_unit = if unit.is_empty() {
                            channel.descriptor.name.clone()
                        } else {
                            format!("{} ({unit})", channel.descriptor.name)
                        };
                        let span = channel
                            .series
                            .samples
                            .first()
                            .zip(channel.series.samples.last())
                            .map_or(0.0, |(first, last)| last.time - first.time);
                        (
                            ChannelRef {
                                source_id,
                                channel_id: channel.descriptor.id,
                            },
                            format!(
                                "{name_and_unit} · {} samples · {span:.1} s",
                                channel.series.samples.len()
                            ),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
