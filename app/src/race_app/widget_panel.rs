//! Appearance and widget editor controls.
use super::super::policy::{
    appearance_color_editor, appearance_for_preset, appearance_number, appearance_preset,
    appearance_preset_label, coordinate_pair_label, normalize_appearance, reset_widget_appearance,
    retarget_widget_units, set_appearance_number, set_appearance_string, set_style, style_bool,
    style_number, style_string, track_map_mode_label, unit_name, widget_appearance_ui,
};
use super::{OverlayEditor, PanelAction};
use crate::ui_kit::{theme::text, widgets};
use eframe::egui::{self, RichText};
use overlay_core::ChannelBinding;
use serde_json::json;
use std::collections::HashMap;

fn widget_kind_label(kind: &str) -> &str {
    match kind {
        "numeric" => "Number",
        "bar" => "Bar",
        "radial" => "Gauge",
        "xy_dot" => "G meter",
        "tachometer" => "Tachometer",
        "temperature" => "Temperature",
        "lap_timer" => "Lap timer",
        "delta" => "Delta",
        "shift_lights" => "Shift lights",
        "center_bar" => "Center bar",
        "gear" => "Gear",
        "track_map" => "Track map",
        other => other,
    }
}

fn slot_caption(slot: &str) -> &str {
    match slot {
        "value" => "Channel",
        "x" => "X channel",
        "y" => "Y channel",
        "latitude" => "Latitude",
        "longitude" => "Longitude",
        other => other,
    }
}

impl OverlayEditor {
    fn add_widget_menu(&mut self, ui: &mut egui::Ui) {
        let groups: [(&str, &[(&str, &str)]); 3] = [
            (
                "Instruments",
                &[
                    ("Number", "numeric"),
                    ("Bar", "bar"),
                    ("Gauge", "radial"),
                    ("Tachometer", "tachometer"),
                    ("Temperature", "temperature"),
                    ("G meter", "xy_dot"),
                    ("Gear", "gear"),
                    ("Shift lights", "shift_lights"),
                    ("Center bar", "center_bar"),
                ],
            ),
            ("Timing", &[("Lap timer", "lap_timer"), ("Delta", "delta")]),
            ("Map", &[("Track map", "track_map")]),
        ];
        for (title, kinds) in groups {
            widgets::section_label(ui, title);
            for (label, kind) in kinds {
                if widgets::menu_item(ui, label, None) {
                    self.add_widget(kind);
                    ui.close();
                }
            }
        }
        ui.separator();
        if widgets::menu_item(
            ui,
            "MyChron dashboard",
            Some("A full set of dashboard widgets"),
        ) {
            self.add_mychron_dashboard();
            ui.close();
        }
    }

    pub(super) fn appearance_panel(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut make_all_inherit = false;
        {
            let Some(project) = self.project_mut() else {
                return;
            };
            let mut appearance = project.appearance.clone();
            normalize_appearance(&mut appearance);
            ui.collapsing("Appearance", |ui| {
                let old_preset = appearance_preset(&appearance);
                let mut preset = old_preset.to_owned();
                egui::ComboBox::from_id_salt("appearance-preset")
                    .selected_text(appearance_preset_label(&preset))
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("race_dark", "Race dark"),
                            ("light", "Light"),
                            ("transparent", "Transparent"),
                            ("custom", "Custom"),
                        ] {
                            ui.selectable_value(&mut preset, value.to_owned(), label);
                        }
                    });
                if preset != old_preset {
                    if preset != "custom" {
                        appearance = appearance_for_preset(&preset);
                    } else {
                        set_appearance_string(&mut appearance, "preset", "custom");
                    }
                    changed = true;
                }
                ui.horizontal(|ui| {
                    if ui.button("Reset to preset").clicked() {
                        let reset_preset = if preset == "custom" {
                            "race_dark"
                        } else {
                            &preset
                        };
                        appearance = appearance_for_preset(reset_preset);
                        changed = true;
                    }
                    if ui.button("Make all widgets use global").clicked() {
                        make_all_inherit = true;
                    }
                });
                ui.separator();
                for (key, label) in [
                    ("accent", "Accent"),
                    ("text", "Text"),
                    ("background", "Panel background"),
                    ("muted", "Muted"),
                    ("positive", "Positive"),
                    ("warning", "Warning"),
                    ("critical", "Critical"),
                ] {
                    if appearance_color_editor(ui, &mut appearance, key, label) {
                        set_appearance_string(&mut appearance, "preset", "custom");
                        changed = true;
                    }
                }
                let mut foreground = appearance_number(&appearance, "foreground_opacity", 0.92);
                if ui
                    .add(egui::Slider::new(&mut foreground, 0.0..=1.0).text("Foreground opacity"))
                    .changed()
                {
                    set_appearance_number(&mut appearance, "foreground_opacity", foreground);
                    set_appearance_string(&mut appearance, "preset", "custom");
                    changed = true;
                }
                let mut background = appearance_number(&appearance, "background_opacity", 0.92);
                if ui
                    .add(egui::Slider::new(&mut background, 0.0..=1.0).text("Background opacity"))
                    .changed()
                {
                    set_appearance_number(&mut appearance, "background_opacity", background);
                    set_appearance_string(&mut appearance, "preset", "custom");
                    changed = true;
                }
                let mut radius = appearance_number(&appearance, "corner_radius", 0.12);
                if ui
                    .add(egui::Slider::new(&mut radius, 0.0..=0.30).text("Corner roundness"))
                    .changed()
                {
                    set_appearance_number(&mut appearance, "corner_radius", radius);
                    set_appearance_string(&mut appearance, "preset", "custom");
                    changed = true;
                }
                ui.small("Widget-specific overrides are available in each widget's settings.");
            });
            if make_all_inherit {
                for widget in &mut project.widgets {
                    reset_widget_appearance(widget);
                }
                changed = true;
            }
            project.appearance = appearance;
        }
        if changed {
            self.refresh_overlay();
        }
    }

    pub(in crate::race_app) fn widgets_panel(&mut self, ui: &mut egui::Ui) {
        if !self.session.has_project() {
            widgets::empty_state(
                ui,
                "Nothing to edit",
                "Open a video to place gauges, bars, and maps over it.",
                |_| {},
            );
            return;
        }
        self.appearance_panel(ui);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Widgets")
                    .strong()
                    .size(15.0)
                    .color(text::strong()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                widgets::action_menu(ui, "Add ⏷", |ui| self.add_widget_menu(ui));
            });
        });
        let widgets_snapshot = self
            .project()
            .map(|p| p.widgets.clone())
            .unwrap_or_default();
        let global_appearance = self
            .project()
            .map(|p| p.appearance.clone())
            .unwrap_or_else(|| appearance_for_preset("race_dark"));
        if widgets_snapshot.is_empty() {
            widgets::hint(ui, "No widgets yet. Use Add to place one on the video.");
        }
        for widget in &widgets_snapshot {
            let label = style_string(&widget.style, "label").unwrap_or_else(|| widget.kind.clone());
            let selected = self.widget_editor.selected_widget == Some(widget.id);
            let row = widgets::list_row(ui, selected, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(label).color(text::strong()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(widget_kind_label(&widget.kind))
                                .small()
                                .color(text::weak()),
                        );
                    });
                });
            });
            if row.response.clicked() {
                self.apply_panel_action(PanelAction::SelectWidget(widget.id));
            }
        }
        let widgets = widgets_snapshot;
        let Some(index) = self.selected_widget_index() else {
            return;
        };
        if index >= widgets.len() {
            return;
        }
        let widget_id = widgets[index].id;
        ui.add_space(8.0);
        widgets::section_label(ui, "Data");
        let slot_names: &[&str] = if widgets[index].kind == "xy_dot" {
            &["x", "y"]
        } else if widgets[index].kind == "track_map" {
            &["latitude", "longitude"]
        } else {
            &["value"]
        };
        let mut refresh = false;
        for slot in slot_names {
            let current = widgets[index].bindings.iter().find(|b| b.slot == *slot);
            let current_label = self.selected_channel_label(current);
            let choices = if widgets[index].kind == "track_map" {
                self.gps_channel_choices(slot)
            } else {
                self.channel_choices()
            };
            ui.horizontal(|ui| {
                ui.label(slot_caption(slot));
                egui::ComboBox::from_id_salt(("widget-slot", widget_id, *slot))
                    .selected_text(current_label)
                    .width(ui.available_width() - 8.0)
                    .truncate()
                    .show_ui(ui, |ui| {
                        for (reference, label) in &choices {
                            let selected = current.is_some_and(|b| b.channel == *reference);
                            if ui.selectable_label(selected, label).clicked() {
                                if widgets[index].kind == "track_map" {
                                    self.bind_track_map_coordinate(index, slot, reference.clone());
                                } else {
                                    self.bind_widget_channel(index, slot, reference.clone());
                                }
                                refresh = true;
                            }
                        }
                    });
            });
        }
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
        let unit_context = (widgets[index].kind != "track_map")
            .then(|| {
                widgets[index].bindings.first().and_then(|binding| {
                    let descriptor = descriptors
                        .get(&(binding.channel.source_id, binding.channel.channel_id))?;
                    let current = binding
                        .display_unit
                        .clone()
                        .unwrap_or_else(|| descriptor.unit.clone());
                    Some((current, descriptor.unit.compatible_units()))
                })
            })
            .flatten();
        let mut requested_unit = None;
        if let Some((current, units)) = &unit_context {
            ui.horizontal(|ui| {
                ui.label("Unit");
                egui::ComboBox::from_id_salt(("widget-display-unit", widgets[index].id))
                    .selected_text(unit_name(current))
                    .show_ui(ui, |ui| {
                        for unit in units {
                            if ui
                                .selectable_label(unit == current, unit_name(unit))
                                .clicked()
                            {
                                requested_unit = Some(unit.clone());
                            }
                        }
                    });
            });
        }
        let mut capture_start = false;
        let mut capture_finish = false;
        let mut clear_start = false;
        let mut clear_finish = false;
        if let Some(widget) = self.session.widget_mut(widget_id) {
            if let Some(unit) = requested_unit {
                retarget_widget_units(widget, &descriptors, &unit);
                refresh = true;
            }
            let filterable = |binding: &ChannelBinding| {
                descriptors
                    .get(&(binding.channel.source_id, binding.channel.channel_id))
                    .is_some_and(|descriptor| {
                        descriptor.interpolation == overlay_core::Interpolation::Linear
                    })
            };
            if widget.bindings.iter().any(filterable) {
                widgets::hint(
                    ui,
                    "Smoothing applies only to this widget's continuous inputs (zero-phase, non-causal). The cutoff shown is the final two-pass -3 dB point; upstream filters cascade.",
                );
                let mut enabled = widget
                    .bindings
                    .iter()
                    .filter(|binding| filterable(binding))
                    .any(|binding| binding.low_pass_hz.is_some());
                let mut cutoff = widget
                    .bindings
                    .iter()
                    .filter(|binding| filterable(binding))
                    .find_map(|binding| binding.low_pass_hz)
                    .unwrap_or(8.0);
                if ui.checkbox(&mut enabled, "Widget low-pass").changed() {
                    for binding in widget
                        .bindings
                        .iter_mut()
                        .filter(|binding| filterable(binding))
                    {
                        binding.low_pass_hz = enabled.then_some(cutoff);
                    }
                    refresh = true;
                }
                if enabled
                    && ui
                        .add(
                            egui::Slider::new(&mut cutoff, 0.5..=50.0)
                                .logarithmic(true)
                                .text("Widget cutoff Hz"),
                        )
                        .changed()
                {
                    for binding in widget
                        .bindings
                        .iter_mut()
                        .filter(|binding| filterable(binding))
                    {
                        binding.low_pass_hz = Some(cutoff);
                    }
                    refresh = true;
                }
            } else if widget.kind != "track_map" && !widget.bindings.is_empty() {
                widgets::hint(
                    ui,
                    "This widget uses discrete data; low-pass does not apply.",
                );
            }
            if widget.kind != "track_map" {
                for binding in &mut widget.bindings {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} transform", binding.slot));
                        refresh |= ui
                            .add(
                                egui::DragValue::new(&mut binding.scale)
                                    .speed(0.01)
                                    .prefix("× "),
                            )
                            .changed();
                        refresh |= ui.checkbox(&mut binding.invert, "Invert").changed();
                    });
                }
            }
            if widget.kind == "track_map" {
                widgets::section_label(ui, "Track map");
                let mut mode = style_string(&widget.style, "track_mode")
                    .or_else(|| style_string(&widget.style, "mode"))
                    .unwrap_or_else(|| "auto".into());
                egui::ComboBox::from_id_salt(("track-map-mode", widget.id))
                    .selected_text(track_map_mode_label(&mode))
                    .show_ui(ui, |ui| {
                        for (value, label) in [
                            ("auto", "Auto"),
                            ("circuit", "Circuit"),
                            ("point_to_point", "Point-to-point"),
                        ] {
                            if ui.selectable_label(mode == value, label).clicked() {
                                mode = value.into();
                            }
                        }
                    });
                if style_string(&widget.style, "track_mode").as_deref() != Some(mode.as_str()) {
                    set_style(&mut widget.style, "track_mode", json!(mode));
                    refresh = true;
                }
                let mut lap = style_number(&widget.style, "lap").unwrap_or(0.0);
                ui.horizontal(|ui| {
                    ui.label("Lap (0 = automatic)");
                    if ui
                        .add(egui::DragValue::new(&mut lap).range(0.0..=999.0).speed(1.0))
                        .changed()
                    {
                        set_style(&mut widget.style, "lap", json!(lap.round()));
                        refresh = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "Start: {}",
                        coordinate_pair_label(&widget.style, "start")
                    ));
                    let capture = ui.button("Capture");
                    capture_start = capture.clicked();
                    capture.on_hover_text(
                        "Capture GPS at the current video playhead (including this source's timing offset)",
                    );
                    clear_start = ui.button("Clear").clicked();
                });
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "Finish: {}",
                        coordinate_pair_label(&widget.style, "finish")
                    ));
                    let capture = ui.button("Capture");
                    capture_finish = capture.clicked();
                    capture.on_hover_text(
                        "Capture GPS at the current video playhead (including this source's timing offset)",
                    );
                    clear_finish = ui.button("Clear").clicked();
                });
                let mut line_width = style_number(&widget.style, "line_width").unwrap_or(2.0);
                if ui
                    .add(egui::Slider::new(&mut line_width, 0.5..=8.0).text("Track line width"))
                    .changed()
                {
                    set_style(&mut widget.style, "line_width", json!(line_width));
                    refresh = true;
                }
                let mut rotation = style_number(&widget.style, "rotation_degrees").unwrap_or(0.0);
                if ui
                    .add(egui::Slider::new(&mut rotation, -180.0..=180.0).text("Map rotation °"))
                    .changed()
                {
                    set_style(&mut widget.style, "rotation_degrees", json!(rotation));
                    refresh = true;
                }
                let mut padding = style_number(&widget.style, "padding").unwrap_or(0.10);
                if ui
                    .add(egui::Slider::new(&mut padding, 0.0..=0.30).text("Map padding"))
                    .changed()
                {
                    set_style(&mut widget.style, "padding", json!(padding));
                    refresh = true;
                }
                let mut show_markers = style_bool(&widget.style, "show_markers").unwrap_or(true);
                if ui
                    .checkbox(&mut show_markers, "Show start/finish markers")
                    .changed()
                {
                    set_style(&mut widget.style, "show_markers", json!(show_markers));
                    refresh = true;
                }
                widgets::hint(
                    ui,
                    "Circuit mode keeps one representative lap; point-to-point uses the start and finish markers.",
                );
            }
            widgets::section_label(ui, "Look");
            let mut inherit_appearance =
                style_bool(&widget.style, "inherit_appearance").unwrap_or(true);
            if ui
                .checkbox(&mut inherit_appearance, "Use global appearance")
                .changed()
            {
                set_style(
                    &mut widget.style,
                    "inherit_appearance",
                    json!(inherit_appearance),
                );
                refresh = true;
            }
            if !inherit_appearance {
                ui.collapsing("Appearance overrides", |ui| {
                    if widget_appearance_ui(ui, widget, &global_appearance) {
                        refresh = true;
                    }
                });
            }
            let mut label = style_string(&widget.style, "label").unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label("Label");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut label)
                            .desired_width(ui.available_width() - 4.0),
                    )
                    .changed()
                {
                    set_style(&mut widget.style, "label", json!(label));
                    refresh = true;
                }
            });
            if unit_context.is_none() && widget.kind != "track_map" {
                let mut unit = style_string(&widget.style, "unit").unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Unit text");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut unit)
                                .desired_width(ui.available_width() - 4.0),
                        )
                        .changed()
                    {
                        set_style(&mut widget.style, "unit", json!(unit));
                        refresh = true;
                    }
                });
            }
            if widget.kind != "track_map" {
                widgets::section_label(ui, "Scale");
                let mut min = style_number(&widget.style, "min").unwrap_or(0.0);
                let mut max = style_number(&widget.style, "max").unwrap_or(100.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::DragValue::new(&mut min).speed(0.1).prefix("Min "))
                        .changed()
                    {
                        set_style(&mut widget.style, "min", json!(min));
                        refresh = true;
                    }
                    if ui
                        .add(egui::DragValue::new(&mut max).speed(0.1).prefix("Max "))
                        .changed()
                    {
                        set_style(&mut widget.style, "max", json!(max));
                        refresh = true;
                    }
                });
            }
            widgets::section_label(ui, "Position & size");
            widgets::hint(
                ui,
                "Fractions of the video frame. Drag on the preview to move; drag the corner to resize.",
            );
            ui.horizontal(|ui| {
                refresh |= ui
                    .add(widgets::percent_widget(&mut widget.rect.x, 0.0, 1.0, "X "))
                    .changed();
                refresh |= ui
                    .add(widgets::percent_widget(&mut widget.rect.y, 0.0, 1.0, "Y "))
                    .changed();
            });
            ui.horizontal(|ui| {
                refresh |= ui
                    .add(widgets::percent_widget(
                        &mut widget.rect.width,
                        0.02,
                        1.0,
                        "W ",
                    ))
                    .changed();
                refresh |= ui
                    .add(widgets::percent_widget(
                        &mut widget.rect.height,
                        0.02,
                        1.0,
                        "H ",
                    ))
                    .changed();
            });
        }
        if widgets[index].kind == "track_map" {
            if capture_start {
                refresh |= self.capture_track_map_point(index, "start");
            }
            if capture_finish {
                refresh |= self.capture_track_map_point(index, "finish");
            }
            if clear_start {
                refresh |= self.clear_track_map_point(index, "start");
            }
            if clear_finish {
                refresh |= self.clear_track_map_point(index, "finish");
            }
        }
        ui.add_space(10.0);
        if widgets::danger_button(ui, "Delete widget").clicked() {
            if let Some(project) = self.project_mut() {
                project.widgets.remove(index);
            }
            self.apply_panel_action(PanelAction::ClearWidgetSelection);
            refresh = true;
        }
        if refresh {
            self.refresh_overlay();
        }
    }
}
