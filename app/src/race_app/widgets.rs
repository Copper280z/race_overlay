//! Widget catalog, bindings, unit policy, and dashboard editing.
use super::super::policy::{
    apply_unbound_widget_unit_default, coordinate_value_at, default_display_unit,
    is_latitude_channel, is_longitude_channel, make_widget, preferred_channels, remove_style,
    retarget_widget_units, select_gps_coordinate_channels, set_style, widget_channel_binding,
    widget_suggested_range,
};
use super::OverlayEditor;
use overlay_core::{ChannelBinding, ChannelRef, NormalizedRect, SourceId, Unit};
use serde_json::json;
use std::collections::HashMap;

impl OverlayEditor {
    pub(super) fn auto_bind_unbound_widgets(&mut self) {
        let channels: Vec<_> = self
            .session
            .datasets()
            .iter()
            .flat_map(|d| {
                d.channels
                    .values()
                    .map(move |c| (d.source_id, c.descriptor.id, c.descriptor.name.clone()))
            })
            .collect();
        let Some(project) = self.project() else {
            return;
        };
        let mut bindings = Vec::new();
        for (widget_index, widget) in project.widgets.iter().enumerate() {
            if !widget.bindings.is_empty() {
                continue;
            }
            if widget.kind == "track_map" {
                if let Some((latitude, longitude)) =
                    select_gps_coordinate_channels(self.session.datasets())
                {
                    bindings.push((widget_index, "latitude", latitude));
                    bindings.push((widget_index, "longitude", longitude));
                }
            } else if widget.kind == "xy_dot" {
                let x = channels
                    .iter()
                    .find(|c| c.2 == "lateral_g")
                    .or_else(|| channels.iter().find(|c| c.2 == "gps_lateral_acceleration"));
                let y = channels
                    .iter()
                    .find(|c| c.2 == "longitudinal_g")
                    .or_else(|| channels.iter().find(|c| c.2 == "gps_inline_acceleration"));
                if let (Some(x), Some(y)) = (x, y) {
                    bindings.push((
                        widget_index,
                        "x",
                        ChannelRef {
                            source_id: x.0,
                            channel_id: x.1,
                        },
                    ));
                    bindings.push((
                        widget_index,
                        "y",
                        ChannelRef {
                            source_id: y.0,
                            channel_id: y.1,
                        },
                    ));
                }
            } else {
                let preferred = preferred_channels(&widget.kind);
                let channel = preferred
                    .iter()
                    .find_map(|name| channels.iter().find(|channel| channel.2 == **name))
                    .or_else(|| {
                        (widget.kind == "numeric")
                            .then(|| channels.first())
                            .flatten()
                    });
                if let Some(channel) = channel {
                    bindings.push((
                        widget_index,
                        "value",
                        ChannelRef {
                            source_id: channel.0,
                            channel_id: channel.1,
                        },
                    ));
                }
            }
        }
        for (widget_index, slot, reference) in bindings {
            self.bind_widget_channel(widget_index, slot, reference);
        }
    }

    pub(super) fn repair_missing_bindings(&mut self, source_id: SourceId) {
        let Some(dataset) = self
            .session
            .datasets()
            .iter()
            .find(|item| item.source_id == source_id)
        else {
            return;
        };
        let named = |name: &str| {
            dataset.named(name).map(|channel| ChannelRef {
                source_id,
                channel_id: channel.descriptor.id,
            })
        };
        let lateral = named("lateral_g").or_else(|| named("gps_lateral_acceleration"));
        let longitudinal = named("longitudinal_g").or_else(|| named("gps_inline_acceleration"));
        let combined = named("combined_g");
        let speed = named("speed").or_else(|| named("gps_speed"));
        let rpm = named("rpm");
        let gear = named("gear");
        let water_temperature = named("water_temperature");
        let exhaust_temperature = named("exhaust_temperature");
        let lap_time = named("lap_time");
        let delta = named("best_today_diff").or_else(|| named("predictive_time"));
        let steering_angle = named("steering_angle");
        let (latitude, longitude) = select_gps_coordinate_channels(std::slice::from_ref(dataset))
            .map_or((None, None), |(latitude, longitude)| {
                (Some(latitude), Some(longitude))
            });
        let widgets = self
            .project()
            .map(|project| project.widgets.clone())
            .unwrap_or_default();
        let mut repairs = Vec::new();
        for (widget_index, widget) in widgets.iter().enumerate() {
            for binding in &widget.bindings {
                if binding.channel.source_id != source_id
                    || dataset.channel(binding.channel.channel_id).is_some()
                {
                    continue;
                }
                let replacement = match (widget.kind.as_str(), binding.slot.as_str()) {
                    ("xy_dot", "x") => lateral.clone(),
                    ("xy_dot", "y") => longitudinal.clone(),
                    ("radial", "value") => speed.clone(),
                    ("bar", "value") => rpm.clone().or_else(|| combined.clone()),
                    ("numeric" | "gear", "value") => gear.clone().or_else(|| combined.clone()),
                    ("tachometer" | "shift_lights", "value") => rpm.clone(),
                    ("temperature", "value") => water_temperature
                        .clone()
                        .or_else(|| exhaust_temperature.clone()),
                    ("lap_timer", "value") => lap_time.clone(),
                    ("delta", "value") => delta.clone(),
                    ("center_bar", "value") => steering_angle.clone(),
                    ("track_map", "latitude") => latitude.clone(),
                    ("track_map", "longitude") => longitude.clone(),
                    _ => None,
                };
                repairs.push((widget_index, binding.slot.clone(), replacement));
            }
        }
        for (widget_index, slot, replacement) in repairs {
            if let Some(reference) = replacement {
                self.bind_widget_channel(widget_index, &slot, reference);
            } else if let Some(widget) = self
                .project_mut()
                .and_then(|project| project.widgets.get_mut(widget_index))
            {
                widget.bindings.retain(|binding| {
                    binding.slot != slot || binding.channel.source_id != source_id
                });
            }
        }
    }

    pub(super) fn add_widget(&mut self, kind: &str) {
        let count = self.project().map(|p| p.widgets.len()).unwrap_or(0);
        let mut widget = make_widget(kind, count);
        apply_unbound_widget_unit_default(&mut widget, self.unit_system());
        if let Some(project) = self.project_mut() {
            project.widgets.push(widget);
            self.widget_editor.selected_widget = project.widgets.last().map(|widget| widget.id);
            self.auto_bind_unbound_widgets();
            self.refresh_overlay();
        }
    }

    /// Add a useful MyChron layout without disturbing widgets the user has
    /// already placed. When a source is loaded, bind each dashboard tile to
    /// its intended channel immediately (especially water vs. exhaust temp).
    pub(super) fn add_mychron_dashboard(&mut self) {
        let base = self.project().map(|project| project.widgets.len());
        let Some(base) = base else { return };
        let mut water = make_widget("temperature", base);
        water.rect = NormalizedRect::new(0.04, 0.05, 0.22, 0.12);
        water.style["label"] = json!("WATER");
        let mut egt = make_widget("temperature", base + 1);
        egt.rect = NormalizedRect::new(0.28, 0.05, 0.22, 0.12);
        egt.style["label"] = json!("EGT");
        let mut lap = make_widget("lap_timer", base + 2);
        lap.rect = NormalizedRect::new(0.72, 0.05, 0.24, 0.12);
        let mut delta = make_widget("delta", base + 3);
        delta.rect = NormalizedRect::new(0.72, 0.19, 0.24, 0.12);
        let mut shift = make_widget("shift_lights", base + 4);
        shift.rect = NormalizedRect::new(0.30, 0.61, 0.40, 0.08);
        let mut tach = make_widget("tachometer", base + 5);
        tach.rect = NormalizedRect::new(0.30, 0.70, 0.40, 0.20);
        let mut steering = make_widget("center_bar", base + 6);
        steering.rect = NormalizedRect::new(0.24, 0.90, 0.52, 0.07);
        let mut speed = make_widget("radial", base + 7);
        speed.rect = NormalizedRect::new(0.76, 0.63, 0.20, 0.34);
        let mut gear = make_widget("gear", base + 8);
        gear.rect = NormalizedRect::new(0.46, 0.43, 0.08, 0.14);
        let mut g_meter = make_widget("xy_dot", base + 9);
        g_meter.rect = NormalizedRect::new(0.04, 0.61, 0.20, 0.36);
        g_meter.style["min"] = json!(-2.5);
        g_meter.style["max"] = json!(2.5);
        for widget in [&mut water, &mut egt, &mut speed] {
            apply_unbound_widget_unit_default(widget, self.unit_system());
        }
        let selected = {
            let Some(project) = self.project_mut() else {
                return;
            };
            project.widgets.extend([
                water, egt, lap, delta, shift, tach, steering, speed, gear, g_meter,
            ]);
            project.widgets.last().map(|widget| widget.id)
        };
        self.widget_editor.selected_widget = selected;

        let mychron_source_id = self.project().and_then(|project| {
            self.source_editor
                .selected_source
                .and_then(|id| project.sources.iter().find(|source| source.id == id))
                .filter(|source| source.adapter == "aim_xrk")
                .or_else(|| {
                    project
                        .sources
                        .iter()
                        .rev()
                        .find(|source| source.adapter == "aim_xrk")
                })
                .map(|source| source.id)
        });
        let source = self
            .session
            .datasets()
            .iter()
            .find(|dataset| Some(dataset.source_id) == mychron_source_id)
            .or_else(|| {
                self.session.datasets().iter().find(|dataset| {
                    dataset.named("rpm").is_some()
                        && (dataset.named("water_temperature").is_some()
                            || dataset.named("exhaust_temperature").is_some())
                })
            })
            .map(|dataset| {
                let named = |name: &str| {
                    dataset.named(name).map(|channel| ChannelRef {
                        source_id: dataset.source_id,
                        channel_id: channel.descriptor.id,
                    })
                };
                [
                    (base, "value", named("water_temperature")),
                    (base + 1, "value", named("exhaust_temperature")),
                    (base + 2, "value", named("lap_time")),
                    (
                        base + 3,
                        "value",
                        named("best_today_diff").or_else(|| named("predictive_time")),
                    ),
                    (base + 4, "value", named("rpm")),
                    (base + 5, "value", named("rpm")),
                    (base + 6, "value", named("steering_angle")),
                    (
                        base + 7,
                        "value",
                        named("speed").or_else(|| named("gps_speed")),
                    ),
                    (base + 8, "value", named("gear")),
                    (
                        base + 9,
                        "x",
                        named("lateral_g").or_else(|| named("gps_lateral_acceleration")),
                    ),
                    (
                        base + 9,
                        "y",
                        named("longitudinal_g").or_else(|| named("gps_inline_acceleration")),
                    ),
                ]
            });
        if let Some(bindings) = source {
            for (widget_index, slot, reference) in
                bindings
                    .into_iter()
                    .filter_map(|(widget_index, slot, reference)| {
                        reference.map(|reference| (widget_index, slot, reference))
                    })
            {
                self.bind_widget_channel(widget_index, slot, reference);
            }
        }
        // Keep the dashboard's concise labels after bind_widget_channel has
        // populated descriptive channel labels for ordinary widgets.
        if let Some(project) = self.project_mut() {
            if let Some(widget) = project.widgets.get_mut(base) {
                set_style(&mut widget.style, "label", json!("WATER"));
            }
            if let Some(widget) = project.widgets.get_mut(base + 1) {
                set_style(&mut widget.style, "label", json!("EGT"));
            }
        }
        self.refresh_overlay();
    }

    pub(super) fn channel_choices(&self) -> Vec<(ChannelRef, String)> {
        self.session
            .datasets()
            .iter()
            .flat_map(|dataset| {
                let source_name = self
                    .project()
                    .and_then(|p| p.sources.iter().find(|s| s.id == dataset.source_id))
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "source".into());
                dataset.channels.values().map(move |channel| {
                    (
                        ChannelRef {
                            source_id: dataset.source_id,
                            channel_id: channel.descriptor.id,
                        },
                        format!(
                            "{} / {} ({})",
                            source_name,
                            channel.descriptor.name,
                            channel.descriptor.unit.symbol()
                        ),
                    )
                })
            })
            .collect()
    }

    pub(super) fn gps_channel_choices(&self, slot: &str) -> Vec<(ChannelRef, String)> {
        self.session
            .datasets()
            .iter()
            .flat_map(|dataset| {
                let source_name = self
                    .project()
                    .and_then(|p| p.sources.iter().find(|s| s.id == dataset.source_id))
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| "source".into());
                dataset.channels.values().filter_map(move |channel| {
                    let name = channel.descriptor.name.as_str();
                    let matching = match slot {
                        "latitude" => is_latitude_channel(name),
                        "longitude" => is_longitude_channel(name),
                        _ => false,
                    };
                    (matching && channel.descriptor.unit == Unit::Degree).then(|| {
                        (
                            ChannelRef {
                                source_id: dataset.source_id,
                                channel_id: channel.descriptor.id,
                            },
                            format!(
                                "{} / {} ({})",
                                source_name,
                                channel.descriptor.name,
                                channel.descriptor.unit.symbol()
                            ),
                        )
                    })
                })
            })
            .collect()
    }

    pub(super) fn selected_channel_label(&self, binding: Option<&ChannelBinding>) -> String {
        let Some(binding) = binding else {
            return "Unbound".into();
        };
        self.channel_choices()
            .into_iter()
            .find(|(r, _)| *r == binding.channel)
            .map(|(_, label)| label)
            .unwrap_or_else(|| "Missing channel".into())
    }

    pub(super) fn bind_widget_channel(
        &mut self,
        widget_index: usize,
        slot: &str,
        reference: ChannelRef,
    ) {
        let unit_system = self.unit_system();
        let descriptor = self
            .session
            .datasets()
            .iter()
            .find(|dataset| dataset.source_id == reference.source_id)
            .and_then(|dataset| dataset.channel(reference.channel_id))
            .map(|channel| channel.descriptor.clone());
        let Some(widget) = self
            .project_mut()
            .and_then(|project| project.widgets.get_mut(widget_index))
        else {
            return;
        };
        let binding = widget_channel_binding(
            &widget.kind,
            slot,
            reference,
            descriptor.as_ref(),
            unit_system,
        );
        let display_unit = binding
            .display_unit
            .clone()
            .or_else(|| descriptor.as_ref().map(|item| item.unit.clone()));
        if let Some(existing) = widget.bindings.iter_mut().find(|item| item.slot == slot) {
            *existing = binding;
        } else {
            widget.bindings.push(binding);
        }
        if let Some(descriptor) = descriptor {
            if !matches!(widget.kind.as_str(), "xy_dot" | "track_map") {
                let label = descriptor.name.replace('_', " ").to_ascii_uppercase();
                set_style(&mut widget.style, "label", json!(label));
            }
            let display_unit = display_unit.as_ref().unwrap_or(&descriptor.unit);
            set_style(&mut widget.style, "unit", json!(display_unit.symbol()));
            let (min, max) = widget_suggested_range(
                &widget.kind,
                &descriptor.name,
                &descriptor.quantity,
                display_unit,
            );
            set_style(&mut widget.style, "min", json!(min));
            set_style(&mut widget.style, "max", json!(max));
        }
    }

    pub(super) fn bind_track_map_coordinate(
        &mut self,
        widget_index: usize,
        slot: &str,
        reference: ChannelRef,
    ) {
        let source_id = reference.source_id;
        self.bind_widget_channel(widget_index, slot, reference);
        let Some(dataset) = self
            .session
            .datasets()
            .iter()
            .find(|dataset| dataset.source_id == source_id)
        else {
            return;
        };
        let Some((latitude, longitude)) =
            select_gps_coordinate_channels(std::slice::from_ref(dataset))
        else {
            return;
        };
        match slot {
            "latitude" => self.bind_widget_channel(widget_index, "longitude", longitude),
            "longitude" => self.bind_widget_channel(widget_index, "latitude", latitude),
            _ => {}
        }
    }

    pub(super) fn capture_track_map_point(&mut self, widget_index: usize, which: &str) -> bool {
        let Some(widget) = self
            .project()
            .and_then(|project| project.widgets.get(widget_index))
        else {
            return false;
        };
        let latitude = widget
            .bindings
            .iter()
            .find(|binding| binding.slot == "latitude")
            .map(|binding| binding.channel.clone());
        let longitude = widget
            .bindings
            .iter()
            .find(|binding| binding.slot == "longitude")
            .map(|binding| binding.channel.clone());
        let (Some(latitude), Some(longitude)) = (latitude, longitude) else {
            self.status = "Bind GPS latitude and longitude before capturing a map point".into();
            return false;
        };
        if latitude.source_id != longitude.source_id {
            self.status = "GPS latitude and longitude must come from the same source".into();
            return false;
        }
        let offsets = self.source_offsets();
        let Some(lat) = coordinate_value_at(
            self.session.datasets(),
            &offsets,
            &latitude,
            self.session.current_time(),
        ) else {
            self.status = "No GPS latitude sample at the current playhead".into();
            return false;
        };
        let Some(lon) = coordinate_value_at(
            self.session.datasets(),
            &offsets,
            &longitude,
            self.session.current_time(),
        ) else {
            self.status = "No GPS longitude sample at the current playhead".into();
            return false;
        };
        if !(lat.abs() <= 90.0 && lon.abs() <= 180.0) {
            self.status = "The captured GPS coordinates are outside valid bounds".into();
            return false;
        }
        if let Some(widget) = self
            .project_mut()
            .and_then(|project| project.widgets.get_mut(widget_index))
        {
            set_style(&mut widget.style, &format!("{which}_latitude"), json!(lat));
            set_style(&mut widget.style, &format!("{which}_longitude"), json!(lon));
            self.status = format!("Captured {which} GPS location ({lat:.6}, {lon:.6})");
            true
        } else {
            false
        }
    }

    pub(super) fn clear_track_map_point(&mut self, widget_index: usize, which: &str) -> bool {
        let Some(widget) = self
            .project_mut()
            .and_then(|project| project.widgets.get_mut(widget_index))
        else {
            return false;
        };
        remove_style(&mut widget.style, &format!("{which}_latitude"));
        remove_style(&mut widget.style, &format!("{which}_longitude"));
        true
    }

    pub(super) fn apply_default_unit_system(&mut self) {
        let descriptors: HashMap<_, _> = self
            .session
            .datasets()
            .iter()
            .flat_map(|dataset| {
                dataset.channels.values().map(move |channel| {
                    (
                        (dataset.source_id, channel.descriptor.id),
                        channel.descriptor.clone(),
                    )
                })
            })
            .collect();
        let system = self.unit_system();
        let Some(project) = self.project_mut() else {
            return;
        };
        for widget in &mut project.widgets {
            let Some(primary) = widget.bindings.first() else {
                apply_unbound_widget_unit_default(widget, system);
                continue;
            };
            let Some(descriptor) =
                descriptors.get(&(primary.channel.source_id, primary.channel.channel_id))
            else {
                continue;
            };
            let Some(target) = default_display_unit(&widget.kind, descriptor, system) else {
                continue;
            };
            retarget_widget_units(widget, &descriptors, &target);
        }
        self.refresh_overlay();
    }
}
