//! Appearance and style normalization policies.
use super::widget_policy::style_number;
use eframe::egui;
use overlay_core::{Unit, UnitSystem, WidgetConfig};
use overlay_render::resolve_appearance;
use serde_json::{Value, json};

pub(super) fn style_string(style: &Value, name: &str) -> Option<String> {
    style.get(name).and_then(Value::as_str).map(str::to_owned)
}

pub(super) fn parse_unit_system(value: &str) -> Option<UnitSystem> {
    match value {
        "metric" => Some(UnitSystem::Metric),
        "imperial" => Some(UnitSystem::Imperial),
        _ => None,
    }
}

pub(super) fn unit_name(unit: &Unit) -> &str {
    let symbol = unit.symbol();
    if symbol.is_empty() {
        "unitless"
    } else {
        symbol
    }
}

pub(super) fn set_style(style: &mut Value, name: &str, value: Value) {
    if !style.is_object() {
        *style = json!({});
    }
    style.as_object_mut().unwrap().insert(name.into(), value);
}

pub(super) fn remove_style(style: &mut Value, name: &str) {
    if let Some(object) = style.as_object_mut() {
        object.remove(name);
    }
}

const APPEARANCE_COLOR_KEYS: &[&str] = &[
    "accent",
    "text",
    "background",
    "muted",
    "positive",
    "warning",
    "critical",
];

pub(super) fn appearance_for_preset(name: &str) -> Value {
    let preset = match name {
        "light" | "transparent" | "race_dark" => name,
        _ => "race_dark",
    };
    let palette = resolve_appearance(&json!({"preset": preset}));
    json!({
        "preset": preset,
        "accent": [palette.accent.0, palette.accent.1, palette.accent.2],
        "text": [palette.text.0, palette.text.1, palette.text.2],
        "background": [palette.background.0, palette.background.1, palette.background.2],
        "muted": [palette.muted.0, palette.muted.1, palette.muted.2],
        "positive": [palette.positive.0, palette.positive.1, palette.positive.2],
        "warning": [palette.warning.0, palette.warning.1, palette.warning.2],
        "critical": [palette.critical.0, palette.critical.1, palette.critical.2],
        "foreground_opacity": palette.foreground_opacity,
        "background_opacity": palette.background_opacity,
        "corner_radius": palette.corner_radius,
    })
}

pub(super) fn normalize_appearance(appearance: &mut Value) {
    let preset = appearance_preset(appearance).to_owned();
    let defaults = appearance_for_preset(&preset);
    if !appearance.is_object() {
        *appearance = defaults;
        return;
    }
    for key in APPEARANCE_COLOR_KEYS {
        if appearance.get(*key).and_then(parse_rgb).is_none() {
            set_appearance_value(appearance, key, defaults.get(*key).cloned().unwrap());
        }
    }
    for (key, fallback) in [
        ("foreground_opacity", 0.92),
        ("background_opacity", 0.92),
        ("corner_radius", 0.12),
    ] {
        if appearance_number(appearance, key, f64::NAN).is_nan() {
            set_appearance_number(appearance, key, fallback);
        }
    }
}

pub(super) fn appearance_preset(appearance: &Value) -> &str {
    appearance
        .get("preset")
        .and_then(Value::as_str)
        .filter(|name| matches!(*name, "race_dark" | "light" | "transparent" | "custom"))
        .unwrap_or("race_dark")
}

pub(super) fn appearance_preset_label(name: &str) -> &'static str {
    match name {
        "light" => "Light",
        "transparent" => "Transparent",
        "custom" => "Custom",
        _ => "Race dark",
    }
}

pub(super) fn parse_rgb(value: &Value) -> Option<[u8; 3]> {
    let values = value.as_array()?;
    Some([
        u8::try_from(values.first()?.as_u64()?).ok()?,
        u8::try_from(values.get(1)?.as_u64()?).ok()?,
        u8::try_from(values.get(2)?.as_u64()?).ok()?,
    ])
}

pub(super) fn appearance_number(appearance: &Value, key: &str, fallback: f64) -> f64 {
    appearance
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

pub(super) fn set_appearance_value(appearance: &mut Value, key: &str, value: Value) {
    if !appearance.is_object() {
        *appearance = json!({});
    }
    appearance
        .as_object_mut()
        .unwrap()
        .insert(key.into(), value);
}

pub(super) fn set_appearance_string(appearance: &mut Value, key: &str, value: &str) {
    set_appearance_value(appearance, key, json!(value));
}

pub(super) fn set_appearance_number(appearance: &mut Value, key: &str, value: f64) {
    set_appearance_value(appearance, key, json!(value));
}

pub(super) fn appearance_color_editor(
    ui: &mut egui::Ui,
    appearance: &mut Value,
    key: &str,
    label: &str,
) -> bool {
    let mut color =
        parse_rgb(appearance.get(key).unwrap_or(&Value::Null)).unwrap_or([255, 255, 255]);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.color_edit_button_srgb(&mut color).changed() {
            set_appearance_value(appearance, key, json!(color));
            changed = true;
        }
    });
    changed
}

pub(super) fn widget_appearance_ui(
    ui: &mut egui::Ui,
    widget: &mut WidgetConfig,
    global_appearance: &Value,
) -> bool {
    let mut changed = false;
    for (key, label) in [
        ("accent", "Accent"),
        ("text", "Text"),
        ("background", "Panel background"),
        ("muted", "Muted"),
        ("positive", "Positive"),
        ("warning", "Warning"),
        ("critical", "Critical"),
    ] {
        let rgb = style_rgb(&widget.style, key).unwrap_or_else(|| {
            parse_rgb(global_appearance.get(key).unwrap_or(&Value::Null)).unwrap_or([255, 255, 255])
        });
        let mut color = rgb;
        let mut color_changed = false;
        let mut reset = false;
        ui.horizontal(|ui| {
            ui.label(label);
            color_changed = ui.color_edit_button_srgb(&mut color).changed();
            reset = ui.small_button("Reset").clicked();
        });
        if color_changed {
            set_style(&mut widget.style, key, json!(color));
            changed = true;
        }
        if reset {
            remove_style(&mut widget.style, key);
            changed = true;
        }
    }
    for (key, label, range, fallback) in [
        (
            "opacity",
            "Foreground opacity",
            (0.0, 1.0),
            appearance_number(global_appearance, "foreground_opacity", 0.92),
        ),
        (
            "background_opacity",
            "Background opacity",
            (0.0, 1.0),
            appearance_number(global_appearance, "background_opacity", 0.92),
        ),
        (
            "corner_radius",
            "Corner roundness",
            (0.0, 0.30),
            appearance_number(global_appearance, "corner_radius", 0.12),
        ),
    ] {
        let mut value = style_number(&widget.style, key).unwrap_or(fallback);
        if ui
            .add(egui::Slider::new(&mut value, range.0..=range.1).text(label))
            .changed()
        {
            set_style(&mut widget.style, key, json!(value));
            changed = true;
        }
    }
    if ui.button("Reset all widget overrides").clicked() {
        reset_widget_appearance(widget);
        changed = true;
    }
    changed
}

pub(super) fn style_rgb(style: &Value, key: &str) -> Option<[u8; 3]> {
    parse_rgb(style.get(key).unwrap_or(&Value::Null))
}

pub(super) fn reset_widget_appearance(widget: &mut WidgetConfig) {
    for key in APPEARANCE_COLOR_KEYS.iter().copied().chain([
        "opacity",
        "foreground_opacity",
        "background_opacity",
        "corner_radius",
    ]) {
        remove_style(&mut widget.style, key);
    }
    set_style(&mut widget.style, "inherit_appearance", json!(true));
}
