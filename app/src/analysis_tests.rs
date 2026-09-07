use super::sync::resolved_video_alignment_offsets;
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
fn arrow_step_uses_video_frame_or_adjacent_primary_sample() {
    let source_id = SourceId::new();
    let (recording, key) = recording("run", source("source", source_id), "lap");
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    dataset.insert(TelemetryChannel {
        descriptor: ChannelDescriptor {
            id: ChannelId::new(),
            name: "speed".into(),
            quantity: Quantity::Speed,
            unit: Unit::MeterPerSecond,
            interpolation: Interpolation::Linear,
            description: None,
        },
        series: ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 0.0,
            },
            TimedSample {
                time: 0.1,
                value: 1.0,
            },
            TimedSample {
                time: 0.3,
                value: 2.0,
            },
        ]),
    });
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![recording];
    app.workspace.reference = Some(key.clone());
    app.state.selection = vec![key];
    app.prepared = prepare_comparison(
        &app.workspace,
        &app.state.selection,
        &HashMap::from([(
            source_id,
            SourceData::from_dataset(Arc::new(dataset.clone())),
        )]),
    );
    app.data
        .insert(source_id, SourceData::from_dataset(Arc::new(dataset)));

    app.step_reference(1);
    assert!((app.state.cursor - 0.1).abs() < 1e-9);
    app.step_reference(1);
    assert!((app.state.cursor - 0.3).abs() < 1e-9);
    app.step_reference(-1);
    assert!((app.state.cursor - 0.1).abs() < 1e-9);

    let video = PathBuf::from("video.mp4");
    app.workspace.recordings[0].video_path = Some(video.clone());
    app.metadata.insert(
        video,
        Ok(VideoMetadata {
            frame_rate: Some(overlay_media::Rational::new(60, 1)),
            ..Default::default()
        }),
    );
    app.state.cursor = 0.0;
    app.step_reference(1);
    assert!((app.state.cursor - 1.0 / 60.0).abs() < 1e-9);
}

#[test]
fn video_alignment_preserves_primary_clock_and_solves_other_offsets() {
    let (camera_recording, video_recording) = resolved_video_alignment_offsets(2.0, 5.0, 3.0);

    assert_eq!(camera_recording, -3.0);
    assert_eq!(video_recording, -6.0);
    // At one physical instant: target=recording+2, camera=target-5,
    // and video=camera-3.
    let recording_time = 10.0;
    assert_eq!(recording_time + camera_recording, 7.0);
    assert_eq!(recording_time + video_recording, 4.0);
}

#[test]
fn imported_recording_selection_includes_every_available_interval() {
    let source_a = SourceId::new();
    let source_b = SourceId::new();
    let (mut recording_a, first_a) = recording("A", source("a", source_a), "run 1");
    let (recording_b, first_b) = recording("B", source("b", source_b), "run 1");
    recording_a.segments.push(RunSegment {
        id: SegmentId::new(),
        name: "run 2".into(),
        start_recording_time: 3.0,
        end_recording_time: 5.0,
        kind: SegmentKind::Autocross,
        estimated: true,
        competitive: false,
        unknown: Default::default(),
    });
    let second_a = SegmentRef {
        recording_id: recording_a.id,
        segment_id: recording_a.segments[1].id,
    };
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![recording_a, recording_b];

    app.select_segments_for_recordings(&HashSet::from([first_a.recording_id]));

    assert!(app.state.selection.contains(&first_a));
    assert!(app.state.selection.contains(&second_a));
    assert!(!app.state.selection.contains(&first_b));
}

#[test]
fn regular_plot_delta_uses_and_rebases_gate_defined_runs() {
    let source_a = SourceId::new();
    let source_b = SourceId::new();
    let (_, reference_key) = recording("reference", source("a", source_a), "run");
    let (_, candidate_key) = recording("candidate", source("b", source_b), "run");
    let progress = |times: [f64; 3]| {
        [0.0, 50.0, 100.0]
            .into_iter()
            .zip(times)
            .map(|(progress, recording_time)| ProgressSample {
                recording_time,
                progress,
                confidence: 1.0,
            })
            .collect::<Vec<_>>()
    };
    let run = |key: SegmentRef, name: &str, start: f64, end: f64, times| PreparedRun {
        key,
        name: name.into(),
        start,
        end,
        gps: vec![],
        distance: vec![],
        progress: progress(times),
        time_alignment: None,
    };
    let mut app = AnalysisApp::new();
    app.workspace.reference = Some(reference_key.clone());
    app.workspace.course.gates.push(Gate {
        latitude: 0.0,
        longitude: 0.0,
        heading_degrees: 0.0,
        width_meters: 12.0,
    });
    app.state.selection = vec![reference_key.clone(), candidate_key.clone()];
    app.state.mode = XMode::Course;
    app.prepared.runs = vec![
        run(reference_key, "reference", 10.0, 12.0, [10.0, 11.0, 12.0]),
        run(candidate_key, "candidate", 20.0, 23.0, [20.2, 21.5, 23.0]),
    ];

    let traces = app.build_plot(DELTA_CHANNEL, &PlotOptions::default(), UnitSystem::Metric);
    assert_eq!(traces.len(), 2);
    assert_eq!(traces[1].points[0][0], [0.0, 0.0]);
    assert!((traces[1].points[0][2][1] - 0.8).abs() < 1e-9);
}

#[test]
fn scatter_aligns_xy_and_optional_z_on_recording_time() {
    let source_id = SourceId::new();
    let (recording, key) = recording("run", source("source", source_id), "lap");
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    for (name, values) in [
        ("x", [1.0, 2.0, 3.0]),
        ("y", [10.0, 20.0, 30.0]),
        ("z", [100.0, 200.0, 300.0]),
    ] {
        dataset.insert(TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::new(),
                name: name.into(),
                quantity: Quantity::Speed,
                unit: Unit::MeterPerSecond,
                interpolation: Interpolation::Linear,
                description: None,
            },
            series: ChannelSeries::new(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| TimedSample {
                        time: index as f64,
                        value,
                    })
                    .collect(),
            ),
        });
    }
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![recording];
    app.state.selection = vec![key.clone()];
    app.prepared.runs = vec![PreparedRun {
        key,
        name: "run".into(),
        start: 0.0,
        end: 2.0,
        gps: vec![],
        distance: vec![],
        progress: vec![],
        time_alignment: None,
    }];
    app.data
        .insert(source_id, SourceData::from_dataset(Arc::new(dataset)));
    let options = ScatterOptions {
        x_channel: "x".into(),
        y_channel: "y".into(),
        z_channel: Some("z".into()),
        ..Default::default()
    };

    let traces = app.build_scatter(&options, UnitSystem::Metric);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].points.len(), 3);
    assert_eq!(traces[0].points[1], [7.2, 72.0, 720.0]);
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
fn interval_identity_follows_time_overlap_instead_of_list_position() {
    let source_id = SourceId::new();
    let (mut recording, _) = recording("run", source("source", source_id), "late");
    recording.segments[0].start_recording_time = 100.0;
    recording.segments[0].end_recording_time = 110.0;
    let late_id = recording.segments[0].id;
    let early_id = SegmentId::new();
    recording.segments.push(RunSegment {
        id: early_id,
        name: "early".into(),
        start_recording_time: 0.0,
        end_recording_time: 10.0,
        kind: SegmentKind::Autocross,
        estimated: true,
        competitive: true,
        unknown: Default::default(),
    });
    let replacement = |name: &str, start, end| RunSegment {
        id: SegmentId::new(),
        name: name.into(),
        start_recording_time: start,
        end_recording_time: end,
        kind: SegmentKind::Autocross,
        estimated: true,
        competitive: true,
        unknown: Default::default(),
    };

    replace_segments_preserving_identity(
        &mut recording,
        vec![
            replacement("new early", 0.5, 9.5),
            replacement("new late", 100.5, 109.5),
        ],
    );

    assert_eq!(recording.segments[0].id, early_id);
    assert_eq!(recording.segments[1].id, late_id);
}

#[test]
fn retaining_gate_selection_does_not_add_the_last_run() {
    let (first, first_key) = recording("first", source("first", SourceId::new()), "run");
    let (last, _) = recording("last", source("last", SourceId::new()), "run");
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![first, last];
    app.workspace.reference = Some(first_key.clone());
    app.state.selection = vec![first_key.clone()];

    app.retain_valid_selection();

    assert_eq!(app.state.selection, vec![first_key]);
}

#[test]
fn applying_gates_preserves_selection_without_adding_an_unselected_run() {
    let first_source = SourceId::new();
    let second_source = SourceId::new();
    let (first, first_key) = recording("first", source("first", first_source), "run");
    let (second, second_key) = recording("second", source("second", second_source), "run");
    let dataset = |source_id| {
        let channel = |name: &str, values: [f64; 4]| TelemetryChannel {
            descriptor: ChannelDescriptor {
                id: ChannelId::new(),
                name: name.into(),
                quantity: Quantity::Position,
                unit: Unit::Degree,
                interpolation: Interpolation::Linear,
                description: None,
            },
            series: ChannelSeries::new(
                values
                    .into_iter()
                    .enumerate()
                    .map(|(index, value)| TimedSample {
                        time: index as f64,
                        value,
                    })
                    .collect(),
            )
            .with_gap(2.0),
        };
        let mut dataset = TelemetryDataset {
            source_id,
            ..Default::default()
        };
        dataset.insert(channel("gps_latitude", [-0.001, 0.001, 0.0015, 0.0025]));
        dataset.insert(channel("gps_longitude", [0.0; 4]));
        SourceData::from_dataset(Arc::new(dataset))
    };
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![first, second];
    app.workspace.reference = Some(first_key.clone());
    app.workspace.course.gates = vec![
        Gate {
            latitude: 0.0,
            longitude: 0.0,
            heading_degrees: 0.0,
            width_meters: 20.0,
        },
        Gate {
            latitude: 0.002,
            longitude: 0.0,
            heading_degrees: 0.0,
            width_meters: 20.0,
        },
    ];
    app.state.selection = vec![first_key.clone()];
    app.data.insert(first_source, dataset(first_source));
    app.data.insert(second_source, dataset(second_source));

    app.apply_gates();

    assert_eq!(app.state.selection, vec![first_key.clone()]);
    assert!(!app.state.selection.contains(&second_key));
    assert_eq!(app.workspace.reference, Some(first_key));
}

#[test]
fn time_plot_interpolates_an_exact_gate_boundary_sample() {
    let source_id = SourceId::new();
    let (mut recording, key) = recording("run", source("source", source_id), "gated");
    recording.segments[0].start_recording_time = 0.5;
    recording.segments[0].end_recording_time = 1.5;
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    dataset.insert(TelemetryChannel {
        descriptor: ChannelDescriptor {
            id: ChannelId::new(),
            name: "gps_speed".into(),
            quantity: Quantity::Speed,
            unit: Unit::MeterPerSecond,
            interpolation: Interpolation::Linear,
            description: None,
        },
        series: ChannelSeries::new(vec![
            TimedSample {
                time: 0.0,
                value: 10.0,
            },
            TimedSample {
                time: 1.0,
                value: 20.0,
            },
            TimedSample {
                time: 2.0,
                value: 30.0,
            },
        ])
        .with_gap(2.0),
    });
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![recording];
    app.workspace.reference = Some(key.clone());
    app.state.selection = vec![key.clone()];
    app.state.mode = XMode::Time;
    app.prepared.runs = vec![PreparedRun {
        key,
        name: "gated".into(),
        start: 0.5,
        end: 1.5,
        gps: vec![],
        distance: vec![],
        progress: vec![],
        time_alignment: None,
    }];
    app.data
        .insert(source_id, SourceData::from_dataset(Arc::new(dataset)));

    let traces = app.build_plot("gps_speed", &PlotOptions::default(), UnitSystem::Metric);

    assert_eq!(traces[0].points[0][0][0], 0.0);
    assert!((traces[0].points[0][0][1] - 54.0).abs() < 1e-9);
}

#[test]
fn prepared_axes_start_at_interpolated_gate_boundaries() {
    let source_id = SourceId::new();
    let (mut recording, key) = recording("run", source("source", source_id), "gated");
    recording.segments[0].start_recording_time = 0.5;
    recording.segments[0].end_recording_time = 1.5;
    let channel = |name: &str, quantity, unit, values: [f64; 3]| TelemetryChannel {
        descriptor: ChannelDescriptor {
            id: ChannelId::new(),
            name: name.into(),
            quantity,
            unit,
            interpolation: Interpolation::Linear,
            description: None,
        },
        series: ChannelSeries::new(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| TimedSample {
                    time: index as f64,
                    value,
                })
                .collect(),
        )
        .with_gap(2.0),
    };
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    dataset.insert(channel(
        "gps_latitude",
        Quantity::Position,
        Unit::Degree,
        [42.0, 42.0001, 42.0002],
    ));
    dataset.insert(channel(
        "gps_longitude",
        Quantity::Position,
        Unit::Degree,
        [-71.0, -71.0001, -71.0002],
    ));
    dataset.insert(channel(
        "gps_speed",
        Quantity::Speed,
        Unit::MeterPerSecond,
        [10.0, 20.0, 30.0],
    ));
    let workspace = AnalysisWorkspace {
        recordings: vec![recording],
        reference: Some(key.clone()),
        ..Default::default()
    };
    let data = HashMap::from([(source_id, SourceData::from_dataset(Arc::new(dataset)))]);

    let prepared = prepare_comparison(&workspace, &[key], &data);
    let run = &prepared.runs[0];
    assert_eq!(run.gps.first().unwrap().recording_time, 0.5);
    assert_eq!(run.gps.last().unwrap().recording_time, 1.5);
    assert_eq!(run.distance.first().unwrap().recording_time, 0.5);
    assert_eq!(run.distance.first().unwrap().progress, 0.0);
    assert_eq!(run.distance.last().unwrap().recording_time, 1.5);
    assert_eq!(run.progress.first().unwrap().recording_time, 0.5);
    assert_eq!(run.progress.first().unwrap().progress, 0.0);
}

#[test]
fn complete_lap_matching_disambiguates_the_shared_start_finish_boundary() {
    let source_id = SourceId::new();
    let (mut recording, lap_six) = recording("track", source("logger", source_id), "Lap 6");
    recording.segments[0].end_recording_time = 4.0;
    let lap_seven_segment = RunSegment {
        id: SegmentId::new(),
        name: "Lap 7".into(),
        start_recording_time: 4.0,
        end_recording_time: 8.0,
        kind: SegmentKind::Lap,
        estimated: false,
        competitive: true,
        unknown: Default::default(),
    };
    let lap_seven = SegmentRef {
        recording_id: recording.id,
        segment_id: lap_seven_segment.id,
    };
    recording.segments.push(lap_seven_segment);

    // The two laps share one GPS sample at the timing boundary. Its location is
    // an exact match for the end of the reference course but is also close to
    // its start, reproducing the ambiguity of a closed circuit.
    let latitudes = [
        42.0, 42.0, 42.0002, 42.0002, 42.00001, 42.0, 42.0002, 42.0002, 42.00001,
    ];
    let longitudes = [
        -77.0, -76.9998, -76.9998, -77.0002, -76.99999, -76.9998, -76.9998, -77.0002, -76.99999,
    ];
    let position_channel = |name: &str, values: [f64; 9]| TelemetryChannel {
        descriptor: ChannelDescriptor {
            id: ChannelId::new(),
            name: name.into(),
            quantity: Quantity::Position,
            unit: Unit::Degree,
            interpolation: Interpolation::Linear,
            description: None,
        },
        series: ChannelSeries::new(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| TimedSample {
                    time: index as f64,
                    value,
                })
                .collect(),
        ),
    };
    let mut dataset = TelemetryDataset {
        source_id,
        ..Default::default()
    };
    dataset.insert(position_channel("gps_latitude", latitudes));
    dataset.insert(position_channel("gps_longitude", longitudes));
    let workspace = AnalysisWorkspace {
        recordings: vec![recording],
        reference: Some(lap_six.clone()),
        ..Default::default()
    };
    let prepared = prepare_comparison(
        &workspace,
        &[lap_six, lap_seven.clone()],
        &HashMap::from([(source_id, SourceData::from_dataset(Arc::new(dataset)))]),
    );
    let course_length = prepared.course.as_ref().unwrap().length_meters();
    let candidate = prepared
        .runs
        .iter()
        .find(|run| run.key == lap_seven)
        .unwrap();

    assert!(
        candidate
            .progress
            .iter()
            .all(|sample| sample.confidence > 0.0)
    );
    assert!(candidate.progress.first().unwrap().progress < 2.0);
    assert!(candidate.progress.last().unwrap().progress > course_length - 2.0);
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
fn returning_from_advanced_sync_preserves_the_analysis_recording_clock() {
    let mut app = AnalysisApp::new();
    let source_id = SourceId::new();
    let (mut recording, key) = recording("run", source("data", source_id), "lap");
    recording.video_path = Some("video.mp4".into());
    recording.video_offset_seconds = 4.0;
    let recording_id = recording.id;
    let mut project = project_from_recording(&recording);
    project.sources[0].alignment.offset_seconds = 10.0;
    app.workspace.recordings.push(recording);
    app.workspace.reference = Some(key.clone());
    app.state.selection = vec![key.clone()];
    app.active_overlay = Some(recording_id);

    app.import_project(project);

    let updated = &app.workspace.recordings[0];
    assert_eq!(updated.sources[0].alignment.offset_seconds, 0.0);
    assert_eq!(updated.video_offset_seconds, -10.0);
    assert_eq!(updated.segments[0].id, key.segment_id);
    assert_eq!(app.workspace.reference, Some(key));
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
    assert!(plot.show_legend);
    assert!(plot.legend_labels.is_empty());
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
    let input = json!({"future_scatter":true});
    let scatter: ScatterOptions = serde_json::from_value(input).unwrap();
    assert!(scatter.show_legend);
    assert_eq!(
        serde_json::to_value(scatter).unwrap()["future_scatter"],
        true
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
#[ignore = "requires the supplied local GVKC XRK recording"]
fn supplied_gvkc_lap_seven_matches_lap_six_course_position() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../mychron_data/gvkc/a_0065.xrk");
    assert!(path.exists(), "supplied GVKC fixture is required");
    let mut app = AnalysisApp::new();
    let ctx = egui::Context::default();
    app.add_paths(vec![path]);
    let deadline = Instant::now() + std::time::Duration::from_secs(30);
    while !app.loading.is_empty() || app.preparing || app.prepared_revision != app.revision {
        app.poll(&ctx);
        assert!(Instant::now() < deadline, "{:?}", app.errors);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(app.errors.is_empty(), "{:?}", app.errors);
    let recording = &app.workspace.recordings[0];
    let key_for = |name: &str| {
        let segment = recording
            .segments
            .iter()
            .find(|segment| segment.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        SegmentRef {
            recording_id: recording.id,
            segment_id: segment.id,
        }
    };
    let lap_six = key_for("Lap 6");
    let lap_seven = key_for("Lap 7");
    app.workspace.reference = Some(lap_six.clone());
    app.state.selection = vec![lap_six, lap_seven.clone()];
    app.changed();
    while app.preparing || app.prepared_revision != app.revision {
        app.poll(&ctx);
        assert!(Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let candidate = app
        .prepared
        .runs
        .iter()
        .find(|run| run.key == lap_seven)
        .unwrap();
    let valid_samples = candidate
        .progress
        .iter()
        .filter(|sample| sample.confidence > 0.0)
        .count();
    let course_length = app.prepared.course.as_ref().unwrap().length_meters();
    assert!(candidate.progress.len() > 800);
    assert_eq!(valid_samples, candidate.progress.len());
    assert!(candidate.progress.first().unwrap().progress < 1.0);
    assert!(candidate.progress.last().unwrap().progress > course_length - 1.0);
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
