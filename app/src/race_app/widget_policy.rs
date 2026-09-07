//! Widget catalog, binding, geometry, and unit policies.
use super::appearance_policy::{set_style, style_string};
use eframe::egui;
use overlay_core::{
    ChannelBinding, ChannelDescriptor, ChannelRef, NormalizedRect, Quantity, SourceId,
    TelemetryDataset, Unit, UnitSystem, WidgetConfig, WidgetId,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub(super) fn default_widgets_for(system: UnitSystem) -> Vec<WidgetConfig> {
    let mut widgets = vec![
        make_widget("xy_dot", 0),
        make_widget("radial", 1),
        make_widget("bar", 2),
    ];
    for widget in &mut widgets {
        apply_unbound_widget_unit_default(widget, system);
    }
    widgets
}

pub(super) fn apply_unbound_widget_unit_default(widget: &mut WidgetConfig, system: UnitSystem) {
    let (from, target) = match (widget.kind.as_str(), system) {
        ("radial", UnitSystem::Metric) => (Unit::MilePerHour, Unit::KilometerPerHour),
        ("radial", UnitSystem::Imperial) => (Unit::KilometerPerHour, Unit::MilePerHour),
        ("temperature", UnitSystem::Metric) => (Unit::Fahrenheit, Unit::Celsius),
        ("temperature", UnitSystem::Imperial) => (Unit::Celsius, Unit::Fahrenheit),
        _ => return,
    };
    if style_string(&widget.style, "unit").as_deref() != Some(from.symbol()) {
        return;
    }
    if let Some((scale, offset)) = unit_affine(&from, &target)
        && let (Some(min), Some(max)) = (
            style_number(&widget.style, "min"),
            style_number(&widget.style, "max"),
        )
    {
        let converted_min = min * scale + offset;
        let converted_max = max * scale + offset;
        set_style(
            &mut widget.style,
            "min",
            json!(converted_min.min(converted_max)),
        );
        set_style(
            &mut widget.style,
            "max",
            json!(converted_min.max(converted_max)),
        );
    }
    set_style(&mut widget.style, "unit", json!(target.symbol()));
}

pub(super) fn preferred_channels(kind: &str) -> &'static [&'static str] {
    match kind {
        "radial" => &["speed", "gps_speed"],
        "bar" | "tachometer" | "shift_lights" => &["rpm"],
        "temperature" => &["water_temperature", "exhaust_temperature"],
        "lap_timer" => &["lap_time"],
        "delta" => &["best_today_diff", "predictive_time"],
        "center_bar" => &["steering_angle"],
        "gear" => &["gear"],
        _ => &["gear"],
    }
}

pub(super) fn is_latitude_channel(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "latitude"
        || name == "lat"
        || name == "gps_lat"
        || name == "gps_latitude"
        || name.ends_with("_latitude")
}

pub(super) fn is_longitude_channel(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "longitude"
        || name == "lon"
        || name == "lng"
        || name == "gps_lon"
        || name == "gps_lng"
        || name == "gps_longitude"
        || name.ends_with("_longitude")
}

pub(super) fn select_gps_coordinate_channels(
    datasets: &[TelemetryDataset],
) -> Option<(ChannelRef, ChannelRef)> {
    datasets.iter().find_map(|dataset| {
        let latitude = dataset
            .channels
            .values()
            .filter(|channel| channel.descriptor.unit == Unit::Degree)
            .filter(|channel| is_latitude_channel(&channel.descriptor.name))
            .min_by_key(|channel| {
                if channel.descriptor.name == "gps_latitude" {
                    0
                } else if channel.descriptor.name == "latitude" {
                    1
                } else {
                    2
                }
            })?;
        let longitude = dataset
            .channels
            .values()
            .filter(|channel| channel.descriptor.unit == Unit::Degree)
            .filter(|channel| is_longitude_channel(&channel.descriptor.name))
            .min_by_key(|channel| {
                if channel.descriptor.name == "gps_longitude" {
                    0
                } else if channel.descriptor.name == "longitude" {
                    1
                } else {
                    2
                }
            })?;
        Some((
            ChannelRef {
                source_id: dataset.source_id,
                channel_id: latitude.descriptor.id,
            },
            ChannelRef {
                source_id: dataset.source_id,
                channel_id: longitude.descriptor.id,
            },
        ))
    })
}

pub(super) fn coordinate_value_at(
    datasets: &[TelemetryDataset],
    offsets: &HashMap<SourceId, f64>,
    reference: &ChannelRef,
    video_time: f64,
) -> Option<f64> {
    let dataset = datasets
        .iter()
        .find(|dataset| dataset.source_id == reference.source_id)?;
    let channel = dataset.channel(reference.channel_id)?;
    let name = channel.descriptor.name.as_str();
    if channel.descriptor.unit != Unit::Degree
        || !(is_latitude_channel(name) || is_longitude_channel(name))
    {
        return None;
    }
    let source_time = video_time + offsets.get(&reference.source_id).copied().unwrap_or(0.0);
    channel
        .series
        .sample_at_default(source_time, channel.descriptor.interpolation)
        .filter(|value| value.is_finite())
}

pub(super) fn track_map_mode_label(mode: &str) -> &'static str {
    match mode {
        "circuit" => "Circuit",
        "point_to_point" => "Point-to-point",
        _ => "Auto",
    }
}

pub(super) fn widget_channel_binding(
    kind: &str,
    slot: &str,
    reference: ChannelRef,
    descriptor: Option<&ChannelDescriptor>,
    unit_system: UnitSystem,
) -> ChannelBinding {
    let mut binding = ChannelBinding::new(slot, reference);
    let Some(descriptor) = descriptor else {
        return binding;
    };
    if let Some(target) = default_display_unit(kind, descriptor, unit_system) {
        let _ = retarget_binding_unit(&mut binding, &descriptor.unit, &target);
    } else if kind == "delta"
        && descriptor.unit == Unit::Unitless
        && matches!(
            descriptor.name.as_str(),
            "best_today_diff" | "predictive_time"
        )
    {
        // A few AiM generations omit the unit tag for these millisecond
        // channels. Keep the known format quirk explicit and inspectable.
        binding.scale = 0.001;
        binding.display_unit = Some(Unit::Second);
    }
    binding
}

pub(super) fn default_display_unit(
    kind: &str,
    descriptor: &ChannelDescriptor,
    system: UnitSystem,
) -> Option<Unit> {
    if matches!(kind, "lap_timer" | "delta") && descriptor.unit.is_compatible_with(&Unit::Second) {
        return Some(Unit::Second);
    }
    Unit::default_for(&descriptor.quantity, system)
        .filter(|target| descriptor.unit.is_compatible_with(target))
}

pub(super) fn unit_affine(from: &Unit, to: &Unit) -> Option<(f64, f64)> {
    let zero = from.convert_value_to(0.0, to).ok()?;
    let one = from.convert_value_to(1.0, to).ok()?;
    Some((one - zero, zero))
}

pub(super) fn retarget_binding_unit(
    binding: &mut ChannelBinding,
    source: &Unit,
    target: &Unit,
) -> bool {
    let current = binding.display_unit.as_ref().unwrap_or(source);
    let Some((scale, offset)) = unit_affine(current, target) else {
        return false;
    };
    binding.scale *= scale;
    binding.offset = binding.offset * scale + offset;
    binding.display_unit = Some(target.clone());
    true
}

pub(super) fn retarget_widget_units(
    widget: &mut WidgetConfig,
    descriptors: &HashMap<(SourceId, overlay_core::ChannelId), ChannelDescriptor>,
    target: &Unit,
) {
    let range_transform = widget.bindings.first().and_then(|binding| {
        let descriptor =
            descriptors.get(&(binding.channel.source_id, binding.channel.channel_id))?;
        let current = binding.display_unit.as_ref().unwrap_or(&descriptor.unit);
        unit_affine(current, target)
    });
    for binding in &mut widget.bindings {
        let Some(descriptor) =
            descriptors.get(&(binding.channel.source_id, binding.channel.channel_id))
        else {
            continue;
        };
        retarget_binding_unit(binding, &descriptor.unit, target);
    }
    if let Some((scale, offset)) = range_transform {
        if let (Some(min), Some(max)) = (
            style_number(&widget.style, "min"),
            style_number(&widget.style, "max"),
        ) {
            let converted_min = min * scale + offset;
            let converted_max = max * scale + offset;
            set_style(
                &mut widget.style,
                "min",
                json!(converted_min.min(converted_max)),
            );
            set_style(
                &mut widget.style,
                "max",
                json!(converted_min.max(converted_max)),
            );
        }
        set_style(&mut widget.style, "unit", json!(target.symbol()));
    }
}

pub(super) fn widget_suggested_range(
    kind: &str,
    name: &str,
    quantity: &Quantity,
    unit: &Unit,
) -> (f64, f64) {
    match kind {
        "temperature" if name == "exhaust_temperature" => {
            converted_range(0.0, 1000.0, &Unit::Celsius, unit)
        }
        "temperature" => converted_range(0.0, 120.0, &Unit::Celsius, unit),
        "delta" => converted_range(-10.0, 10.0, &Unit::Second, unit),
        "center_bar" => converted_range(-180.0, 180.0, &Unit::Degree, unit),
        "tachometer" | "shift_lights" => (0.0, 12_000.0),
        "lap_timer" => converted_range(0.0, 120.0, &Unit::Second, unit),
        _ => suggested_range(name, quantity, unit),
    }
}

pub(super) fn converted_range(min: f64, max: f64, from: &Unit, to: &Unit) -> (f64, f64) {
    let Ok(min) = from.convert_value_to(min, to) else {
        return (min, max);
    };
    let Ok(max) = from.convert_value_to(max, to) else {
        return (min, max);
    };
    (min.min(max), min.max(max))
}

pub(super) fn make_widget(kind: &str, index: usize) -> WidgetConfig {
    let (rect, label, unit, min, max) = match kind {
        "xy_dot" => (
            NormalizedRect::new(0.04, 0.63, 0.25, 0.30),
            "G FORCE",
            "g",
            -1.5,
            1.5,
        ),
        "radial" => (
            NormalizedRect::new(0.76, 0.65, 0.20, 0.28),
            "SPEED",
            "km/h",
            0.0,
            220.0,
        ),
        "bar" => (
            NormalizedRect::new(0.32, 0.83, 0.36, 0.10),
            "RPM",
            "rpm",
            0.0,
            10000.0,
        ),
        "tachometer" => (
            NormalizedRect::new(0.30, 0.68, 0.40, 0.22),
            "RPM",
            "rpm",
            0.0,
            12000.0,
        ),
        "temperature" => (
            NormalizedRect::new(0.04, 0.05, 0.20, 0.12),
            "TEMP",
            "°C",
            0.0,
            120.0,
        ),
        "lap_timer" => (
            NormalizedRect::new(0.72, 0.05, 0.24, 0.12),
            "LAP",
            "s",
            0.0,
            120.0,
        ),
        "delta" => (
            NormalizedRect::new(0.72, 0.19, 0.24, 0.12),
            "DELTA",
            "s",
            -10.0,
            10.0,
        ),
        "shift_lights" => (
            NormalizedRect::new(0.30, 0.61, 0.40, 0.08),
            "SHIFT",
            "rpm",
            0.0,
            12000.0,
        ),
        "center_bar" => (
            NormalizedRect::new(0.24, 0.84, 0.52, 0.10),
            "STEERING",
            "°",
            -180.0,
            180.0,
        ),
        "gear" => (
            NormalizedRect::new(0.46, 0.43, 0.08, 0.14),
            "GEAR",
            "",
            0.0,
            8.0,
        ),
        "track_map" => (
            NormalizedRect::new(0.68, 0.04, 0.28, 0.34),
            "TRACK MAP",
            "",
            0.0,
            1.0,
        ),
        _ => (
            NormalizedRect::new(0.04 + (index % 4) as f32 * 0.18, 0.05, 0.16, 0.12),
            "VALUE",
            "",
            0.0,
            100.0,
        ),
    };
    WidgetConfig {
        id: WidgetId::new(),
        kind: kind.into(),
        rect,
        bindings: vec![],
        style: json!({
            "inherit_appearance": true,
            "label": label,
            "unit": unit,
            "min": min,
            "max": max,
            "format": match kind {
                "lap_timer" | "delta" => "{:.2}",
                "temperature" | "center_bar" => "{:.0}",
                _ => "{:.0}"
            },
            "track_mode": if kind == "track_map" { "auto" } else { "" },
            "lap": 0,
            "line_width": if kind == "track_map" { 2.0 } else { 0.0 },
            "rotation_degrees": 0.0,
            "padding": if kind == "track_map" { 0.10 } else { 0.0 },
            "show_markers": kind == "track_map",
        }),
        settings: json!({}),
        unknown: BTreeMap::new(),
    }
}

pub(super) fn normalized_to_screen(r: NormalizedRect, parent: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(
            parent.left() + r.x * parent.width(),
            parent.top() + r.y * parent.height(),
        ),
        egui::vec2(r.width * parent.width(), r.height * parent.height()),
    )
}

pub(super) fn point_in(p: egui::Pos2, r: NormalizedRect) -> bool {
    p.x >= r.x && p.x <= r.x + r.width && p.y >= r.y && p.y <= r.y + r.height
}

pub(super) fn style_number(style: &Value, name: &str) -> Option<f64> {
    style.get(name).and_then(Value::as_f64)
}

pub(super) fn style_bool(style: &Value, name: &str) -> Option<bool> {
    style.get(name).and_then(Value::as_bool)
}

pub(super) fn coordinate_pair_label(style: &Value, which: &str) -> String {
    let lat = style_number(style, &format!("{which}_latitude"));
    let lon = style_number(style, &format!("{which}_longitude"));
    match (lat, lon) {
        (Some(lat), Some(lon)) => format!("{lat:.6}, {lon:.6}"),
        _ => "not set".into(),
    }
}

pub(super) fn suggested_range(name: &str, quantity: &Quantity, unit: &Unit) -> (f64, f64) {
    match (quantity, unit) {
        (Quantity::Acceleration, Unit::StandardGravity) if name == "combined_g" => (0.0, 3.0),
        (Quantity::Acceleration, Unit::StandardGravity) => (-3.0, 3.0),
        (Quantity::Acceleration, Unit::MeterPerSecondSquared) => (-30.0, 30.0),
        (Quantity::Speed, Unit::KilometerPerHour) => (0.0, 250.0),
        (Quantity::Speed, Unit::MilePerHour) => (0.0, 160.0),
        (Quantity::Speed, _) => (0.0, 70.0),
        (Quantity::Rpm, _) => (0.0, 12_000.0),
        (Quantity::AngularVelocity, Unit::RadianPerSecond) => (-5.0, 5.0),
        (Quantity::AngularVelocity, _) => (-300.0, 300.0),
        (Quantity::LapTime, _) => (0.0, 120.0),
        _ => (0.0, 100.0),
    }
}
