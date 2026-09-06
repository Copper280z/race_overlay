use super::workflow::replace_segments_preserving_identity;
use super::*;

fn source(name: &str, id: SourceId) -> SourceConfig {
    SourceConfig {
        id,
        name: name.into(),
        adapter: "synthetic".into(),
        path: name.into(),
        alignment: Default::default(),
        settings: Value::Null,
        unknown: BTreeMap::new(),
    }
}

fn recording(name: &str, source: SourceConfig, segment_name: &str) -> (Recording, SegmentRef) {
    let id = RecordingId::new();
    let segment = RunSegment {
        id: SegmentId::new(),
        name: segment_name.into(),
        start_recording_time: 0.,
        end_recording_time: 2.,
        kind: SegmentKind::Lap,
        estimated: false,
        competitive: true,
        unknown: BTreeMap::new(),
    };
    let key = SegmentRef {
        recording_id: id,
        segment_id: segment.id,
    };
    (
        Recording {
            id,
            name: name.into(),
            sources: vec![source.clone()],
            primary_source: source.id,
            video_path: None,
            video_offset_seconds: 0.,
            segments: vec![segment],
            overlay_snapshot: None,
            unknown: BTreeMap::new(),
        },
        key,
    )
}

#[test]
fn comparison_keeps_source_identity_when_channel_names_match() {
    let a = SourceId::new();
    let b = SourceId::new();
    let (ra, ka) = recording("A", source("same", a), "lap");
    let (rb, kb) = recording("B", source("same", b), "lap");
    let workspace = AnalysisWorkspace {
        recordings: vec![ra, rb],
        reference: Some(ka.clone()),
        ..Default::default()
    };
    let mut data = HashMap::new();
    data.insert(
        a,
        SourceData::from_dataset(Arc::new(TelemetryDataset {
            source_id: a,
            ..Default::default()
        })),
    );
    data.insert(
        b,
        SourceData::from_dataset(Arc::new(TelemetryDataset {
            source_id: b,
            ..Default::default()
        })),
    );
    let prepared = prepare_comparison(&workspace, &[ka.clone(), kb.clone()], &data);
    assert_eq!(prepared.runs.len(), 2);
    assert_eq!(prepared.runs[0].key, ka);
    assert_eq!(prepared.runs[1].key, kb);
    assert_eq!(automatic_default_mode(&prepared, Some(&ka)), XMode::Time);
    let mut app = AnalysisApp::new();
    app.workspace = workspace;
    app.state.selection = vec![ka, kb];
    app.state.mode = XMode::Time;
    app.prepared = prepared;
    for (id, value) in [(a, 10.0), (b, 20.0)] {
        let mut dataset = TelemetryDataset {
            source_id: id,
            ..Default::default()
        };
        dataset.insert(TelemetryChannel {
            descriptor: overlay_core::ChannelDescriptor {
                id: overlay_core::ChannelId::new(),
                name: "gps_speed".into(),
                quantity: Quantity::Speed,
                unit: Unit::MeterPerSecond,
                interpolation: Interpolation::Linear,
                description: None,
            },
            series: ChannelSeries::new(
                (0..=20)
                    .map(|i| TimedSample {
                        time: i as f64 / 10.0,
                        value,
                    })
                    .collect(),
            ),
        });
        let dataset = Arc::new(dataset);
        app.data.insert(
            id,
            SourceData {
                raw: dataset.clone(),
                processed: dataset,
            },
        );
    }
    let plots = app.build_plot("gps_speed", &PlotOptions::default(), UnitSystem::Metric);
    assert_eq!(plots.len(), 2);
    assert!((plots[0].points[0][0][1] - 36.0).abs() < 1e-9);
    assert!((plots[1].points[0][0][1] - 72.0).abs() < 1e-9);
}

#[test]
fn gate_capture_uses_pinned_reference_even_when_prepared_runs_are_stale() {
    let first_source = SourceId::new();
    let reference_source = SourceId::new();
    let (first, first_key) = recording("first", source("first", first_source), "run");
    let (mut reference, reference_key) = recording(
        "pinned reference",
        source("reference", reference_source),
        "run",
    );
    reference.segments[0].start_recording_time = 100.0;
    reference.segments[0].end_recording_time = 110.0;
    let gps_dataset = |id: SourceId, start: f64, latitude: f64| {
        let mut dataset = TelemetryDataset {
            source_id: id,
            ..Default::default()
        };
        for (name, values) in [
            (
                "gps_latitude",
                (0..=100)
                    .map(|index| TimedSample {
                        time: start + index as f64 * 0.1,
                        value: latitude + index as f64 * 0.000_001,
                    })
                    .collect(),
            ),
            (
                "gps_longitude",
                (0..=100)
                    .map(|index| TimedSample {
                        time: start + index as f64 * 0.1,
                        value: -76.0 + index as f64 * 0.000_001,
                    })
                    .collect(),
            ),
        ] {
            dataset.insert(TelemetryChannel {
                descriptor: ChannelDescriptor {
                    id: ChannelId::new(),
                    name: name.into(),
                    quantity: Quantity::Position,
                    unit: Unit::Degree,
                    interpolation: Interpolation::Linear,
                    description: None,
                },
                series: ChannelSeries::new(values),
            });
        }
        Arc::new(dataset)
    };
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![first, reference];
    app.workspace.reference = Some(reference_key.clone());
    app.state.selection = vec![first_key.clone(), reference_key];
    app.state.cursor = 2.0;
    app.data.insert(
        first_source,
        SourceData::from_dataset(gps_dataset(first_source, 0.0, 10.0)),
    );
    app.data.insert(
        reference_source,
        SourceData::from_dataset(gps_dataset(reference_source, 100.0, 42.0)),
    );
    app.prepared.runs = vec![PreparedRun {
        key: first_key,
        name: "stale first run".into(),
        color: COLORS[0],
        start: 0.0,
        end: 2.0,
        gps: vec![],
        distance: vec![],
        progress: vec![],
        time_alignment: None,
    }];

    let (recording_id, name, time, gate) = app.reference_gate_capture().unwrap();
    assert_eq!(recording_id, app.workspace.recordings[1].id);
    assert!(name.starts_with("pinned reference /"));
    assert!((time - 102.0).abs() < 0.11);
    assert!(gate.latitude > 41.0);
}

#[test]
fn interval_redetection_preserves_the_pinned_segment_identity() {
    let source_id = SourceId::new();
    let (mut recording, reference) = recording("run", source("source", source_id), "old");
    let replacement = RunSegment {
        id: SegmentId::new(),
        name: "redetected".into(),
        start_recording_time: 50.0,
        end_recording_time: 110.0,
        kind: SegmentKind::Autocross,
        estimated: true,
        competitive: true,
        unknown: BTreeMap::new(),
    };
    replace_segments_preserving_identity(&mut recording, vec![replacement]);
    assert_eq!(recording.segments[0].id, reference.segment_id);
    assert_eq!(recording.segments[0].name, "redetected");
}

#[test]
fn stopped_reference_video_keeps_its_actual_elapsed_clock() {
    let mut app = AnalysisApp::new();
    let (r, key) = recording("reference", source("source", SourceId::new()), "lap");
    app.workspace.recordings = vec![r];
    app.workspace.reference = Some(key.clone());
    app.state.cursor = 1.5;
    let progress = (0..=2)
        .map(|i| ProgressSample {
            recording_time: i as f64,
            progress: 0.0,
            confidence: 1.0,
        })
        .collect::<Vec<_>>();
    app.prepared.runs = vec![PreparedRun {
        key,
        name: "stopped".into(),
        color: COLORS[0],
        start: 0.0,
        end: 2.0,
        gps: vec![],
        distance: progress.clone(),
        progress,
        time_alignment: None,
    }];
    for mode in [XMode::Time, XMode::Distance, XMode::Course] {
        app.state.mode = mode;
        assert_eq!(app.run_time(&app.prepared.runs[0]), Some(1.5));
    }
}

#[test]
fn distance_domain_lateral_alignment_zeroes_the_shared_start_not_the_run_middle() {
    let reference_source = SourceId::new();
    let target_source = SourceId::new();
    let (mut reference, reference_key) =
        recording("reference", source("reference", reference_source), "run");
    let (mut target, target_key) = recording("target", source("target", target_source), "run");
    reference.segments[0].end_recording_time = 20.0;
    target.segments[0].end_recording_time = 28.0;
    let profile = |distance: f64| {
        0.4 * (distance * 0.11).sin() + 0.9 * (-((distance - 32.0) / 5.0).powi(2)).exp()
            - 0.7 * (-((distance - 73.0) / 7.0).powi(2)).exp()
    };
    let dataset = |id, delay: f64, speed: f64, duration: f64| {
        let mut dataset = TelemetryDataset {
            source_id: id,
            ..Default::default()
        };
        let timed = (0..=(duration * 20.0) as usize)
            .map(|index| {
                let time = index as f64 * 0.05;
                let distance = ((time - delay) * speed).max(0.0);
                (time, distance)
            })
            .collect::<Vec<_>>();
        for (name, quantity, unit, values) in [
            (
                "distance",
                Quantity::Distance,
                Unit::Meter,
                timed
                    .iter()
                    .map(|(time, distance)| TimedSample {
                        time: *time,
                        value: *distance,
                    })
                    .collect(),
            ),
            (
                "gps_lateral_acceleration",
                Quantity::Acceleration,
                Unit::StandardGravity,
                timed
                    .iter()
                    .map(|(time, distance)| TimedSample {
                        time: *time,
                        value: profile(*distance),
                    })
                    .collect(),
            ),
        ] {
            dataset.insert(TelemetryChannel {
                descriptor: ChannelDescriptor {
                    id: ChannelId::new(),
                    name: name.into(),
                    quantity,
                    unit,
                    interpolation: Interpolation::Linear,
                    description: None,
                },
                series: ChannelSeries::new(values),
            });
        }
        Arc::new(dataset)
    };
    let workspace = AnalysisWorkspace {
        recordings: vec![reference, target],
        reference: Some(reference_key.clone()),
        ..Default::default()
    };
    let data = HashMap::from([
        (
            reference_source,
            SourceData::from_dataset(dataset(reference_source, 0.0, 5.0, 20.0)),
        ),
        (
            target_source,
            SourceData::from_dataset(dataset(target_source, 3.0, 4.0, 28.0)),
        ),
    ]);
    let prepared = prepare_comparison(
        &workspace,
        &[reference_key.clone(), target_key.clone()],
        &data,
    );
    assert_eq!(
        automatic_default_mode(&prepared, Some(&reference_key)),
        XMode::Time
    );
    let alignment = prepared
        .runs
        .iter()
        .find(|run| run.key == target_key)
        .unwrap()
        .time_alignment
        .as_ref()
        .unwrap();
    assert_eq!(alignment.channel, "lateral acceleration by distance");
    assert!(alignment.distance_offset_meters.unwrap().abs() <= 0.51);
    assert!(
        (alignment.offset_seconds + 3.0).abs() <= 0.06,
        "{alignment:?}"
    );
    assert!(alignment.coefficient.is_some_and(|value| value > 0.99));
    assert_eq!(
        workspace.recordings[1].sources[0].alignment.offset_seconds,
        0.0
    );

    let mut app = AnalysisApp::new();
    app.workspace = workspace;
    app.state.mode = XMode::Time;
    app.prepared = prepared;
    let target_run = app
        .prepared
        .runs
        .iter()
        .find(|run| run.key == target_key)
        .unwrap();
    assert!(app.x_at_time(target_run, 3.0).unwrap().abs() <= 0.06);
}

#[test]
fn imu_only_recordings_align_sustained_motion_onset() {
    let reference_source = SourceId::new();
    let target_source = SourceId::new();
    let (mut reference, reference_key) =
        recording("reference", source("reference", reference_source), "run");
    let (mut target, target_key) = recording("target", source("target", target_source), "run");
    reference.segments[0].end_recording_time = 10.0;
    target.segments[0].end_recording_time = 10.0;
    let dataset = |id, onset: f64| {
        let mut dataset = TelemetryDataset {
            source_id: id,
            ..Default::default()
        };
        for (name, resting) in [
            ("raw_accel_x", 0.0),
            ("raw_accel_y", 0.0),
            ("raw_accel_z", 9.80665),
        ] {
            dataset.insert(TelemetryChannel {
                descriptor: ChannelDescriptor {
                    id: ChannelId::new(),
                    name: name.into(),
                    quantity: Quantity::Acceleration,
                    unit: Unit::MeterPerSecondSquared,
                    interpolation: Interpolation::Linear,
                    description: None,
                },
                series: ChannelSeries::new(
                    (0..=500)
                        .map(|index| {
                            let time = index as f64 * 0.02;
                            TimedSample {
                                time,
                                value: resting
                                    + if name == "raw_accel_x" && time >= onset {
                                        2.0 + (time * 4.0).sin()
                                    } else {
                                        0.0
                                    },
                            }
                        })
                        .collect(),
                ),
            });
        }
        Arc::new(dataset)
    };
    let workspace = AnalysisWorkspace {
        recordings: vec![reference, target],
        reference: Some(reference_key.clone()),
        ..Default::default()
    };
    let data = HashMap::from([
        (
            reference_source,
            SourceData::from_dataset(dataset(reference_source, 2.0)),
        ),
        (
            target_source,
            SourceData::from_dataset(dataset(target_source, 4.0)),
        ),
    ]);
    let prepared = prepare_comparison(&workspace, &[reference_key, target_key.clone()], &data);
    let alignment = prepared
        .runs
        .iter()
        .find(|run| run.key == target_key)
        .unwrap()
        .time_alignment
        .as_ref()
        .unwrap();
    assert_eq!(alignment.channel, "sustained motion onset");
    assert!((alignment.offset_seconds + 2.0).abs() < 0.1);
    assert_eq!(alignment.coefficient, None);
}

#[test]
fn importing_one_overlay_preserves_other_recording_and_offset() {
    let mut app = AnalysisApp::new();
    let first_id = SourceId::new();
    let second_id = SourceId::new();
    let (first, _) = recording("first", source("first", first_id), "lap");
    let (second, _) = recording("second", source("second", second_id), "lap");
    app.workspace.recordings = vec![first.clone(), second.clone()];
    app.workspace.recordings[0].video_offset_seconds = 4.;
    let mut project = project_from_recording(&app.workspace.recordings[0]);
    project.video_path = "updated.mp4".into();
    app.import_project(project);
    assert_eq!(app.workspace.recordings.len(), 2);
    assert_eq!(app.workspace.recordings[0].video_offset_seconds, 4.);
    assert_eq!(app.workspace.recordings[1].name, second.name);
    assert_eq!(
        app.workspace.recordings[1].video_offset_seconds,
        second.video_offset_seconds
    );
}

#[test]
fn headless_ui_builds_panels_from_nonempty_raw_input() {
    let mut app = AnalysisApp::new();
    let ctx = egui::Context::default();
    let raw = egui::RawInput {
        events: vec![egui::Event::PointerMoved(egui::pos2(3., 4.))],
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| app.ui(ui, None, UnitSystem::Metric));
    output.textures_delta.clear();
}

#[test]
fn linked_overlay_removing_all_sources_does_not_create_a_duplicate_recording() {
    let mut app = AnalysisApp::new();
    let (r, _) = recording("recording", source("data", SourceId::new()), "lap");
    let id = r.id;
    let mut project = project_from_recording(&r);
    project.sources.clear();
    app.workspace.recordings.push(r);
    app.active_overlay = Some(id);
    app.import_project(project);
    assert_eq!(app.workspace.recordings.len(), 1);
    assert_eq!(app.workspace.recordings[0].id, id);
    assert!(app.workspace.recordings[0].sources.is_empty());
}

#[test]
fn panel_and_ui_settings_preserve_unknown_fields() {
    let input = json!({"future_ui":42});
    let state: UiState = serde_json::from_value(input).unwrap();
    assert_eq!(serde_json::to_value(state).unwrap()["future_ui"], 42);
    let input = json!({"future_plot":"preserve"});
    let plot: PlotOptions = serde_json::from_value(input).unwrap();
    assert_eq!(
        serde_json::to_value(plot).unwrap()["future_plot"],
        "preserve"
    );
    let input = json!({"future_map":[1,2]});
    let map: MapSettings = serde_json::from_value(input).unwrap();
    assert_eq!(
        serde_json::to_value(map).unwrap()["future_map"],
        json!([1, 2])
    );
}

#[test]
#[ignore = "requires supplied local XRK recordings"]
fn supplied_xrk_recordings_import_when_available() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mychron_data");
    let paths = [
        "8_30_26_autox/a_0080.xrk",
        "8_30_26_autox/a_0086.xrk",
        "gvkc/a_0065.xrk",
    ]
    .iter()
    .map(|name| root.join(name))
    .collect::<Vec<_>>();
    assert!(
        paths.iter().all(|p| p.exists()),
        "supplied fixtures are required for this explicitly requested test"
    );
    let mut app = AnalysisApp::new();
    let ctx = egui::Context::default();
    let start = Instant::now();
    app.add_paths(paths);
    while !app.loading.is_empty() || app.preparing || app.prepared_revision != app.revision {
        app.poll(&ctx);
        assert!(
            start.elapsed().as_secs() < 30,
            "import did not finish: {:?}",
            app.errors
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    assert_eq!(app.workspace.recordings.len(), 3);
    let stationary = &app.workspace.recordings[0];
    assert!(
        !stationary.segments.iter().any(|s| s.competitive),
        "{:?}",
        stationary.segments
    );
    let autocross = &app.workspace.recordings[1];
    assert!(
        autocross.segments.iter().any(|s| s.competitive),
        "{:?}",
        autocross.segments
    );
    let track = &app.workspace.recordings[2];
    assert_eq!(track.segments.iter().filter(|s| s.competitive).count(), 9);
    assert_eq!(track.segments.len(), 11);
    for r in &app.workspace.recordings {
        assert!(!gps_points(&app.data[&r.primary_source].raw, r).is_empty());
    }
    let selected = track
        .segments
        .iter()
        .filter(|s| s.competitive)
        .take(2)
        .map(|s| SegmentRef {
            recording_id: track.id,
            segment_id: s.id,
        })
        .collect::<Vec<_>>();
    app.workspace.reference = Some(selected[0].clone());
    app.state.selection = selected;
    app.changed();
    while app.preparing || app.prepared_revision != app.revision {
        app.poll(&ctx);
        assert!(start.elapsed().as_secs() < 30);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(app.prepared.runs.len(), 2);
    assert!(
        app.prepared
            .runs
            .iter()
            .all(|r| r.gps.len() > 100 && r.progress.len() > 100)
    );
    let traces = app.build_plot("gps_speed", &PlotOptions::default(), UnitSystem::Metric);
    assert_eq!(traces.len(), 2);
    assert!(
        traces
            .iter()
            .all(|t| t.points.iter().map(Vec::len).sum::<usize>() > 100)
    );
    let raw = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1440.0, 900.0),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(raw, |ui| app.ui(ui, None, UnitSystem::Metric));
    output.textures_delta.clear();
    eprintln!(
        "real XRK import / matching / UI completed in {:?}",
        start.elapsed()
    );
}

#[test]
#[ignore = "requires supplied local autocross XRK recordings"]
fn supplied_autocross_runs_get_best_effort_time_alignment() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mychron_data/8_30_26_autox");
    let paths = ["a_0078.xrk", "a_0091.xrk"].map(|name| root.join(name));
    assert!(paths.iter().all(|path| path.exists()));
    let mut app = AnalysisApp::new();
    let ctx = egui::Context::default();
    app.add_paths(paths.into());
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    while !app.loading.is_empty() || app.preparing || app.prepared_revision != app.revision {
        app.poll(&ctx);
        assert!(Instant::now() < deadline, "{:?}", app.errors);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(app.state.mode, XMode::Time);
    let selected = app
        .workspace
        .recordings
        .iter()
        .filter_map(|recording| {
            recording
                .segments
                .iter()
                .find(|segment| segment.competitive)
                .map(|segment| SegmentRef {
                    recording_id: recording.id,
                    segment_id: segment.id,
                })
        })
        .collect::<Vec<_>>();
    assert_eq!(selected.len(), 2);
    app.workspace.reference = Some(selected[0].clone());
    app.state.selection = selected;
    app.changed();
    while app.preparing || app.prepared_revision != app.revision {
        app.poll(&ctx);
        assert!(Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.prepared.automatic_time_alignment);
    for run in &app.prepared.runs {
        assert!(run.end - run.start < 90.0, "{}", run.name);
    }
    let alignment = app
        .prepared
        .runs
        .iter()
        .find(|run| Some(&run.key) != app.workspace.reference.as_ref())
        .and_then(|run| run.time_alignment.as_ref())
        .unwrap();
    assert_eq!(alignment.channel, "lateral acceleration by distance");
    assert!(alignment.offset_seconds.abs() < 2.0, "{alignment:?}");
    assert!(
        alignment
            .coefficient
            .is_some_and(|value| value.abs() > 0.75)
    );
}
