//! Regression tests for the decomposed overlay application.
use super::INSTA360_IMU_SAMPLE_RATE_HZ;
use super::appearance_policy::parse_rgb;
use super::controllers::WorkerHub;
use super::editor::OverlayEditor;
use super::policy::{
    appearance_for_preset, appearance_number, appearance_preset, apply_low_pass_to_dataset,
    coordinate_value_at, default_widgets_for, export_surface, make_widget, normalize_appearance,
    parse_unit_system, prepare_loaded_dataset, remove_source_from_project, reset_widget_appearance,
    set_source_low_pass_settings, set_style, source_low_pass_settings, style_bool, style_number,
    style_string, widget_channel_binding, widget_suggested_range,
};
use super::session::OverlaySession;
use super::source_policy::{correlation_candidate_offsets, correlation_lag_window};
use super::widget_policy::{
    preferred_channels, retarget_binding_unit, select_gps_coordinate_channels,
};
use crossbeam_channel::unbounded;
use eframe::egui;
use overlay_core::{
    AdapterRegistry, CameraCalibration, ChannelBinding, ChannelDescriptor, ChannelRef,
    ProjectDocument, ProjectV1, Quantity, SourceConfig, SourceId, TelemetryDataset, Unit,
    UnitSystem, VehicleFrameCalibrationRequest, WidgetId, add_derived_inertial_channels,
    fit_vehicle_frame_calibration,
};
use overlay_render::RenderSize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

fn imu_test_channel(name: &str, values: impl Fn(f64) -> f64) -> overlay_core::TelemetryChannel {
    let source_id = SourceId::new();
    overlay_core::TelemetryChannel {
        descriptor: overlay_core::ChannelDescriptor {
            id: overlay_core::ChannelId::for_source_name(source_id, name),
            name: name.into(),
            quantity: Quantity::Generic,
            unit: Unit::Unitless,
            interpolation: overlay_core::Interpolation::Linear,
            description: None,
        },
        series: overlay_core::ChannelSeries::new(
            (0..=60)
                .map(|index| {
                    let time = f64::from(index) * 0.1;
                    overlay_core::TimedSample {
                        time,
                        value: values(time),
                    }
                })
                .collect(),
        ),
    }
}

#[test]
fn headless_overlay_ui_builds_all_primary_panels() {
    let mut editor = OverlayEditor::new_for_test();
    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        events: vec![egui::Event::PointerMoved(egui::pos2(3.0, 4.0))],
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| {
        editor.header_left(ui);
        editor.header_right(ui);
        editor.sources_panel(ui);
        editor.widgets_panel(ui);
        editor.data_plot_ui(ui);
        editor.preview_ui(ui);
    });
    output.textures_delta.clear();
}

#[test]
fn correlation_defaults_to_the_complete_feasible_time_range() {
    let editor = OverlayEditor::new_for_test();

    assert!(editor.correlation_searches_all_time());
}

#[test]
fn default_project_has_all_core_visuals() {
    let widgets = default_widgets_for(UnitSystem::Metric);
    assert!(widgets.iter().any(|w| w.kind == "xy_dot"));
    assert!(widgets.iter().any(|w| w.kind == "radial"));
    assert!(widgets.iter().any(|w| w.kind == "bar"));
    assert!(make_widget("numeric", 3).kind == "numeric");
    let imperial = default_widgets_for(UnitSystem::Imperial);
    let speed = imperial
        .iter()
        .find(|widget| widget.kind == "radial")
        .unwrap();
    assert_eq!(style_string(&speed.style, "unit").as_deref(), Some("mph"));
    assert!((style_number(&speed.style, "max").unwrap() - 136.701_7).abs() < 0.001);
}

#[test]
fn appearance_presets_keep_rgb_and_opacity_separate() {
    let dark = appearance_for_preset("race_dark");
    assert_eq!(appearance_preset(&dark), "race_dark");
    assert_eq!(parse_rgb(&dark["accent"]), Some([0, 218, 255]));
    assert_eq!(dark["accent"].as_array().unwrap().len(), 3);
    let transparent = appearance_for_preset("transparent");
    assert_eq!(transparent["background_opacity"], json!(0.0));
    assert_eq!(parse_rgb(&transparent["background"]), Some([10, 15, 22]));
    assert_eq!(transparent["background"].as_array().unwrap().len(), 3);
}

#[test]
fn appearance_json_normalization_fills_missing_fields_without_replacing_custom_values() {
    let mut appearance = json!({
        "preset": "custom",
        "accent": [1, 2, 3],
        "background_opacity": 0.4
    });
    normalize_appearance(&mut appearance);
    assert_eq!(parse_rgb(&appearance["accent"]), Some([1, 2, 3]));
    assert_eq!(appearance["background_opacity"], json!(0.4));
    assert!(parse_rgb(&appearance["critical"]).is_some());
    assert_eq!(appearance_number(&appearance, "corner_radius", 0.0), 0.12);
}

#[test]
fn widget_appearance_reset_restores_global_inheritance() {
    let mut widget = make_widget("numeric", 0);
    set_style(&mut widget.style, "inherit_appearance", json!(false));
    set_style(&mut widget.style, "accent", json!([1, 2, 3, 4]));
    set_style(&mut widget.style, "opacity", json!(0.2));
    set_style(&mut widget.style, "foreground_opacity", json!(0.3));
    set_style(&mut widget.style, "corner_radius", json!(0.25));
    reset_widget_appearance(&mut widget);
    assert_eq!(style_bool(&widget.style, "inherit_appearance"), Some(true));
    assert!(widget.style.get("accent").is_none());
    assert!(widget.style.get("opacity").is_none());
    assert!(widget.style.get("foreground_opacity").is_none());
    assert!(widget.style.get("corner_radius").is_none());
}

#[test]
fn gps_binding_selection_prefers_degree_channels_from_one_source() {
    let source_id = SourceId::new();
    let mut latitude = imu_test_channel("gps_latitude", |_| 40.0);
    latitude.descriptor.unit = Unit::Degree;
    latitude.descriptor.quantity = Quantity::Position;
    latitude.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, "gps_latitude");
    let mut longitude = imu_test_channel("gps_longitude", |_| -73.0);
    longitude.descriptor.unit = Unit::Degree;
    longitude.descriptor.quantity = Quantity::Position;
    longitude.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, "gps_longitude");
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    dataset.insert(latitude);
    dataset.insert(longitude);
    let (lat, lon) = select_gps_coordinate_channels(&[dataset]).expect("GPS pair");
    assert_eq!(lat.source_id, source_id);
    assert_eq!(lon.source_id, source_id);
    assert_eq!(
        lat.channel_id,
        overlay_core::ChannelId::for_source_name(source_id, "gps_latitude")
    );
}

#[test]
fn coordinate_capture_uses_video_time_plus_source_offset() {
    let source_id = SourceId::new();
    let mut latitude = imu_test_channel("gps_latitude", |time| 40.0 + time);
    latitude.descriptor.unit = Unit::Degree;
    latitude.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, "gps_latitude");
    let reference = ChannelRef {
        source_id,
        channel_id: latitude.descriptor.id,
    };
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    dataset.insert(latitude);
    let offsets = HashMap::from([(source_id, 0.5)]);
    assert!(
        (coordinate_value_at(&[dataset], &offsets, &reference, 1.0).unwrap() - 41.5).abs() < 1e-9
    );
}

#[test]
fn source_low_pass_settings_round_trip_without_discarding_adapter_settings() {
    let mut settings = json!({"delimiter": ";"});
    set_source_low_pass_settings(&mut settings, true, 12.5);
    assert_eq!(settings["delimiter"], ";");
    assert_eq!(source_low_pass_settings(&settings), (true, 12.5));
}

#[test]
fn correlation_adjustment_window_is_relative_to_current_alignment() {
    assert_eq!(correlation_lag_window(3.0, 1.25, -2.0, 4.0), (-0.25, 5.75));
    // Reversed values are normalized defensively, even though the UI
    // disables Estimate until the displayed bounds are ordered.
    assert_eq!(correlation_lag_window(3.0, 1.25, 4.0, -2.0), (-0.25, 5.75));
}

#[test]
fn correlation_raw_lag_maps_to_reported_adjustment_and_offset() {
    let (adjustment, resulting_offset) = correlation_candidate_offsets(-0.4, 0.8, 1.1);
    assert!((resulting_offset - 0.7).abs() < 1e-12);
    assert!((adjustment + 0.1).abs() < 1e-12);
}

#[test]
fn source_low_pass_filters_continuous_channels_only() {
    let source_id = SourceId::new();
    let mut continuous = imu_test_channel("continuous", |time| {
        if (time * 10.0) as i64 % 2 == 0 {
            0.0
        } else {
            10.0
        }
    });
    continuous.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, "c");
    let mut discrete = imu_test_channel("discrete", |time| (time * 10.0).round());
    discrete.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, "d");
    discrete.descriptor.interpolation = overlay_core::Interpolation::Hold;
    let discrete_before = discrete.series.clone();
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    dataset.insert(continuous);
    dataset.insert(discrete);

    assert_eq!(apply_low_pass_to_dataset(&mut dataset, 1.0).unwrap(), 1);
    assert_eq!(
        dataset.named("discrete").expect("discrete channel").series,
        discrete_before
    );
    let values = &dataset
        .named("continuous")
        .expect("continuous channel")
        .series
        .samples;
    assert!(
        values
            .iter()
            .skip(1)
            .any(|sample| sample.value > 0.0 && sample.value < 10.0)
    );
}

#[test]
fn camera_source_smoothing_has_stable_order_across_recalibration() {
    let source_id = SourceId::new();
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    for (name, value) in [
        ("raw_accel_x", 2.0),
        ("raw_accel_y", 0.0),
        ("raw_accel_z", 9.80665),
        ("raw_gyro_x", 0.0),
        ("raw_gyro_y", 0.0),
        ("raw_gyro_z", 0.0),
    ] {
        let mut channel = imu_test_channel(name, |time| {
            value
                + if name == "raw_accel_x" {
                    (time * 31.0).sin()
                } else {
                    0.0
                }
        });
        channel.descriptor.id = overlay_core::ChannelId::for_source_name(source_id, name);
        dataset.insert(channel);
    }
    let calibration = CameraCalibration::default();
    assert_eq!(
        prepare_loaded_dataset(&mut dataset, Some(3.0), Some(&calibration)).unwrap(),
        6
    );
    let first = dataset.named("longitudinal_g").unwrap().series.clone();

    // Re-applying calibration derives from the already source-smoothed raw
    // channels and must reproduce the reload pipeline exactly.
    add_derived_inertial_channels(&mut dataset, &calibration, INSTA360_IMU_SAMPLE_RATE_HZ);
    assert_eq!(dataset.named("longitudinal_g").unwrap().series, first);
}

#[test]
fn removing_source_from_project_removes_only_its_bindings() {
    let removed_id = SourceId::new();
    let kept_id = SourceId::new();
    let mut project = ProjectV1::new("video.mp4");
    project.sources = vec![
        SourceConfig {
            id: removed_id,
            name: "removed".into(),
            adapter: "generic_csv".into(),
            path: "removed.csv".into(),
            alignment: Default::default(),
            settings: Value::Null,
            unknown: BTreeMap::new(),
        },
        SourceConfig {
            id: kept_id,
            name: "kept".into(),
            adapter: "generic_csv".into(),
            path: "kept.csv".into(),
            alignment: Default::default(),
            settings: Value::Null,
            unknown: BTreeMap::new(),
        },
    ];
    let mut removed_binding = ChannelBinding::new(
        "value",
        ChannelRef {
            source_id: removed_id,
            channel_id: overlay_core::ChannelId::for_source_name(removed_id, "speed"),
        },
    );
    removed_binding.scale = 2.0;
    let kept_binding = ChannelBinding::new(
        "value",
        ChannelRef {
            source_id: kept_id,
            channel_id: overlay_core::ChannelId::for_source_name(kept_id, "speed"),
        },
    );
    let mut widget = make_widget("numeric", 0);
    widget.bindings = vec![removed_binding, kept_binding];
    project.widgets = vec![widget];

    assert_eq!(
        remove_source_from_project(&mut project, removed_id),
        Some(0)
    );
    assert_eq!(project.sources.len(), 1);
    assert_eq!(project.sources[0].id, kept_id);
    assert_eq!(project.widgets[0].bindings.len(), 1);
    assert_eq!(project.widgets[0].bindings[0].channel.source_id, kept_id);
}

#[test]
fn data_widget_presets_have_distinct_visual_defaults() {
    for kind in [
        "tachometer",
        "temperature",
        "lap_timer",
        "delta",
        "shift_lights",
        "center_bar",
        "gear",
    ] {
        let widget = make_widget(kind, 0);
        assert_eq!(widget.kind, kind);
        assert!(widget.rect.width > 0.0 && widget.rect.height > 0.0);
        assert!(widget.style.get("label").and_then(Value::as_str).is_some());
        assert!(widget.style.get("format").and_then(Value::as_str).is_some());
    }
    let gear = make_widget("gear", 0);
    assert_eq!(style_bool(&gear.style, "inherit_appearance"), Some(true));
    assert!(gear.style.get("background_opacity").is_none());
    assert_eq!(
        preferred_channels("temperature"),
        &["water_temperature", "exhaust_temperature"]
    );
    assert_eq!(
        preferred_channels("delta"),
        &["best_today_diff", "predictive_time"]
    );
    assert_eq!(
        style_number(&make_widget("delta", 0).style, "min"),
        Some(-10.0)
    );
    assert_eq!(
        widget_suggested_range(
            "temperature",
            "exhaust_temperature",
            &Quantity::Temperature,
            &Unit::Celsius
        ),
        (0.0, 1000.0)
    );
    assert_eq!(
        widget_suggested_range(
            "center_bar",
            "steering_angle",
            &Quantity::Position,
            &Unit::Degree
        ),
        (-180.0, 180.0)
    );
}

#[test]
fn widget_binding_presents_native_units_in_dashboard_units() {
    let source_id = SourceId::new();
    let reference = ChannelRef {
        source_id,
        channel_id: overlay_core::ChannelId::for_source_name(source_id, "gps_speed"),
    };
    let speed = ChannelDescriptor {
        id: reference.channel_id,
        name: "gps_speed".into(),
        quantity: Quantity::Speed,
        unit: Unit::MeterPerSecond,
        interpolation: overlay_core::Interpolation::Linear,
        description: None,
    };
    let binding = widget_channel_binding(
        "radial",
        "value",
        reference,
        Some(&speed),
        UnitSystem::Metric,
    );
    assert_eq!(binding.scale, 3.6);
    assert_eq!(binding.display_unit, Some(Unit::KilometerPerHour));

    let imperial = widget_channel_binding(
        "radial",
        "value",
        ChannelRef {
            source_id,
            channel_id: speed.id,
        },
        Some(&speed),
        UnitSystem::Imperial,
    );
    assert!((imperial.scale - 2.236_936_292_054_4).abs() < 1e-9);
    assert_eq!(imperial.display_unit, Some(Unit::MilePerHour));

    let reference = ChannelRef {
        source_id,
        channel_id: overlay_core::ChannelId::for_source_name(source_id, "best_today_diff"),
    };
    let delta = ChannelDescriptor {
        id: reference.channel_id,
        name: "best_today_diff".into(),
        quantity: Quantity::LapTime,
        unit: Unit::Millisecond,
        interpolation: overlay_core::Interpolation::Linear,
        description: None,
    };
    let binding = widget_channel_binding(
        "delta",
        "value",
        reference,
        Some(&delta),
        UnitSystem::Metric,
    );
    assert_eq!(binding.scale, 0.001);
    assert_eq!(binding.display_unit, Some(Unit::Second));

    let mut celsius = ChannelBinding::new(
        "value",
        ChannelRef {
            source_id,
            channel_id: speed.id,
        },
    );
    assert!(retarget_binding_unit(
        &mut celsius,
        &Unit::Celsius,
        &Unit::Fahrenheit
    ));
    assert!((celsius.apply(100.0) - 212.0).abs() < 1e-9);
    assert_eq!(
        widget_suggested_range(
            "temperature",
            "water_temperature",
            &Quantity::Temperature,
            &Unit::Fahrenheit,
        ),
        (32.0, 248.0)
    );
    assert_eq!(parse_unit_system("imperial"), Some(UnitSystem::Imperial));
}

#[test]
fn export_surface_is_tight_and_preserves_pixel_geometry() {
    let widgets = vec![make_widget("numeric", 0)];
    let (geometry, mapped) = export_surface(&widgets, RenderSize::new(1000, 500)).unwrap();
    assert_eq!((geometry.x, geometry.y), (40, 25));
    assert_eq!((geometry.width, geometry.height), (160, 60));
    assert_eq!(mapped[0].rect.x, 0.0);
    assert_eq!(mapped[0].rect.y, 0.0);
    assert!((mapped[0].rect.width - 1.0).abs() < f32::EPSILON);
    assert!((mapped[0].rect.height - 1.0).abs() < f32::EPSILON);
}

#[test]
#[ignore = "parses the supplied large Insta360 recording"]
fn supplied_recording_has_racing_scale_g_after_alignment() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("LRV_20260830_124108_01_017.lrv");
    if !path.exists() {
        return;
    }
    let source_id = SourceId::new();
    let mut dataset = AdapterRegistry::with_builtins()
        .load("insta360", source_id, &path, &Value::Null)
        .unwrap();
    let calibration = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
        .into_iter()
        .find_map(|forward| {
            let outcome = fit_vehicle_frame_calibration(
                &dataset,
                VehicleFrameCalibrationRequest {
                    // Exported-video 0–2 s maps to this moving raw-source
                    // interval; automatic selection then finds stationary data.
                    start_time: 79.0,
                    end_time: 81.0,
                    forward_hint: forward,
                    ..Default::default()
                },
            )
            .ok()?;
            Some(outcome.calibration)
        })
        .unwrap();
    assert!(add_derived_inertial_channels(
        &mut dataset,
        &calibration,
        1_000.0
    ));
    // The supplied Studio export begins 79.0385 seconds into the raw file.
    let mut driving_g = dataset
        .named("combined_g")
        .unwrap()
        .series
        .samples
        .iter()
        .filter(|sample| (79.0385..=138.9).contains(&sample.time))
        .map(|sample| sample.value)
        .collect::<Vec<_>>();
    driving_g.sort_by(f64::total_cmp);
    // Use p95 so brief impact spikes do not define the expected driving scale.
    let p95 = driving_g[driving_g.len() * 95 / 100];
    assert!((1.5..4.0).contains(&p95), "unexpected p95 G: {p95}");
}

#[test]
fn session_stable_ids_survive_reordering_and_source_removal() {
    let first_id = SourceId::new();
    let second_id = SourceId::new();
    let widget_id = WidgetId::new();
    let mut project = ProjectV1::new("video.mp4");
    project.sources = vec![
        SourceConfig {
            id: first_id,
            name: "first".into(),
            adapter: "synthetic".into(),
            path: PathBuf::new(),
            alignment: Default::default(),
            settings: Value::Null,
            unknown: BTreeMap::new(),
        },
        SourceConfig {
            id: second_id,
            name: "second".into(),
            adapter: "synthetic".into(),
            path: PathBuf::new(),
            alignment: Default::default(),
            settings: Value::Null,
            unknown: BTreeMap::new(),
        },
    ];
    let mut widget = make_widget("numeric", 0);
    widget.id = widget_id;
    widget.bindings.push(ChannelBinding::new(
        "value",
        ChannelRef {
            source_id: first_id,
            channel_id: overlay_core::ChannelId::for_source_name(first_id, "speed"),
        },
    ));
    project.widgets = vec![widget];
    let mut session = OverlaySession::new();
    session.set_project(project.into());
    assert_eq!(session.source(first_id).unwrap().name, "first");
    assert_eq!(session.widget(widget_id).unwrap().id, widget_id);
    let ProjectDocument::V1(project) = session.project_mut().unwrap();
    project.sources.reverse();
    session.datasets_mut().push(TelemetryDataset {
        source_id: first_id,
        channels: BTreeMap::new(),
        metadata: Default::default(),
        laps: vec![],
    });
    assert_eq!(session.source(first_id).unwrap().name, "first");
    assert_eq!(session.remove_source(first_id), Some(1));
    assert_eq!(session.source(first_id), None);
    assert!(session.datasets().is_empty());
    assert_eq!(session.source(second_id).unwrap().name, "second");
    assert!(session.widget(widget_id).unwrap().bindings.is_empty());
}

#[test]
fn worker_hub_rejects_stale_source_and_sync_generations() {
    let (tx, rx) = unbounded();
    let mut hub = WorkerHub::new(tx, rx);
    let source = SourceId::new();
    let source_generation = hub.next_source_generation(source);
    let newer_source_generation = hub.next_source_generation(source);
    assert!(!hub.source_generation_is_current(source, source_generation));
    assert!(hub.source_generation_is_current(source, newer_source_generation));
    hub.invalidate_source(source);
    assert!(!hub.source_generation_is_current(source, newer_source_generation));
    let sync_generation = hub.next_sync_generation(source);
    let newer_sync_generation = hub.next_sync_generation(source);
    assert!(!hub.sync_generation_is_current(source, sync_generation));
    assert!(hub.sync_generation_is_current(source, newer_sync_generation));
    hub.invalidate_source(source);
    assert!(!hub.sync_generation_is_current(source, newer_sync_generation));
    let correlation_generation = hub.next_correlation_generation();
    let newer_correlation_generation = hub.next_correlation_generation();
    assert!(!hub.correlation_generation_is_current(correlation_generation));
    assert!(hub.correlation_generation_is_current(newer_correlation_generation));
}

#[test]
fn session_edits_preserve_unknown_project_source_and_widget_fields() {
    let source_id = SourceId::new();
    let widget_id = WidgetId::new();
    let mut project = ProjectV1::new("video.mp4");
    project
        .unknown
        .insert("future_project_key".into(), Value::String("project".into()));
    project.sources.push(SourceConfig {
        id: source_id,
        name: "telemetry".into(),
        adapter: "generic_csv".into(),
        path: "data.csv".into(),
        alignment: Default::default(),
        settings: Value::Null,
        unknown: [("future_source_key".into(), Value::Bool(true))]
            .into_iter()
            .collect(),
    });
    let mut widget = make_widget("numeric", 0);
    widget.id = widget_id;
    widget.unknown.insert(
        "future_widget_key".into(),
        Value::Number(serde_json::Number::from(42)),
    );
    project.widgets.push(widget);

    let mut session = OverlaySession::new();
    session.set_project(ProjectDocument::V1(project));
    session.source_mut(source_id).unwrap().name = "renamed".into();
    session.widget_mut(widget_id).unwrap().kind = "tachometer".into();

    let encoded = serde_json::to_value(session.project().unwrap()).unwrap();
    assert_eq!(
        encoded["project"]["future_project_key"],
        Value::String("project".into())
    );
    assert_eq!(
        encoded["project"]["sources"][0]["future_source_key"],
        Value::Bool(true)
    );
    assert_eq!(
        encoded["project"]["widgets"][0]["future_widget_key"],
        Value::Number(serde_json::Number::from(42))
    );
}

#[test]
#[ignore = "release benchmark; measures the default three-widget preview at two resolutions"]
fn benchmark_preview_overlay_render() {
    use overlay_render::{AlignedDatasets, RenderOptions, render_project_widgets_with_appearance};
    use std::{hint::black_box, time::Instant};
    if cfg!(debug_assertions) {
        panic!("run this benchmark with --release");
    }
    let source_id = SourceId::new();
    let dataset = AdapterRegistry::with_builtins()
        .load(
            "synthetic",
            source_id,
            &PathBuf::new(),
            &json!({"duration_seconds": 20.0, "sample_rate_hz": 50.0}),
        )
        .unwrap();
    let mut widgets = default_widgets_for(UnitSystem::Metric);
    for (widget, bindings) in widgets.iter_mut().zip([
        vec![("x", "lateral_g"), ("y", "longitudinal_g")],
        vec![("value", "speed")],
        vec![("value", "rpm")],
    ]) {
        for (slot, name) in bindings {
            widget.bindings.push(ChannelBinding::new(
                slot,
                ChannelRef {
                    source_id,
                    channel_id: dataset.named(name).unwrap().descriptor.id,
                },
            ));
        }
    }
    let datasets = vec![dataset];
    let aligned = AlignedDatasets {
        datasets: &datasets,
        ..Default::default()
    };
    let appearance = appearance_for_preset("race_dark");
    let options = RenderOptions {
        crop: false,
        full_size: false,
    };
    for size in [RenderSize::new(960, 540), RenderSize::new(1920, 1080)] {
        let mut samples = Vec::new();
        for i in 0..128 {
            let at = Instant::now();
            let result = render_project_widgets_with_appearance(
                &widgets,
                &aligned,
                &appearance,
                size,
                f64::from(i) / 30.0,
                options,
            );
            black_box(egui::ColorImage::from_rgba_unmultiplied(
                [result.image.width as usize, result.image.height as usize],
                &result.image.pixels,
            ));
            black_box(result);
            if i >= 8 {
                samples.push(at.elapsed().as_secs_f64());
            }
        }
        samples.sort_by(f64::total_cmp);
        let mean = samples.iter().sum::<f64>() / samples.len() as f64;
        eprintln!(
            "Three-widget CPU render + UI color conversion {}×{}: median {:.2} ms, p95 {:.2} ms, mean {:.2} ms",
            size.width,
            size.height,
            samples[samples.len() / 2] * 1000.0,
            samples[samples.len() * 95 / 100] * 1000.0,
            mean * 1000.0
        );
    }
}
