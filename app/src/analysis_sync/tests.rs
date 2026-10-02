//! Video synchronization integration tests.

use super::super::*;
use super::*;

fn channel(
    source_id: SourceId,
    name: &str,
    unit: Unit,
    values: impl Fn(f64) -> f64,
) -> TelemetryChannel {
    TelemetryChannel {
        descriptor: ChannelDescriptor {
            id: ChannelId::for_source_name(source_id, name),
            name: name.into(),
            quantity: Quantity::Acceleration,
            unit,
            interpolation: Interpolation::Linear,
            description: None,
        },
        series: ChannelSeries::new(
            (0..=60)
                .map(|index| {
                    let time = f64::from(index) * 0.1;
                    TimedSample {
                        time,
                        value: values(time),
                    }
                })
                .collect(),
        ),
    }
}

#[test]
fn analysis_calibration_creates_and_prefers_vehicle_lateral_acceleration() {
    let logger_id = SourceId::new();
    let camera_id = SourceId::new();
    let recording_id = RecordingId::new();
    let logger = SourceConfig {
        id: logger_id,
        name: "logger".into(),
        adapter: "synthetic".into(),
        path: "logger".into(),
        alignment: Default::default(),
        settings: json!({}),
        unknown: Default::default(),
    };
    let camera = SourceConfig {
        id: camera_id,
        name: "camera".into(),
        adapter: "insta360".into(),
        path: "camera.lrv".into(),
        alignment: Default::default(),
        settings: json!({}),
        unknown: Default::default(),
    };
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![Recording {
        id: recording_id,
        name: "run".into(),
        sources: vec![logger, camera],
        primary_source: logger_id,
        video_path: Some("video.mp4".into()),
        video_processing: None,
        video_offset_seconds: 0.0,
        segments: vec![],
        overlay_snapshot: None,
        unknown: Default::default(),
    }];
    let mut logger_data = TelemetryDataset {
        source_id: logger_id,
        ..Default::default()
    };
    logger_data.insert(channel(
        logger_id,
        "gps_lateral_acceleration",
        Unit::StandardGravity,
        |time| time.sin(),
    ));
    app.data
        .insert(logger_id, SourceData::from_dataset(Arc::new(logger_data)));
    let mut camera_data = TelemetryDataset {
        source_id: camera_id,
        ..Default::default()
    };
    for item in [
        channel(
            camera_id,
            "raw_accel_x",
            Unit::MeterPerSecondSquared,
            |_| 0.0,
        ),
        channel(
            camera_id,
            "raw_accel_y",
            Unit::MeterPerSecondSquared,
            |time| {
                if time <= 2.0 { 0.0 } else { time.sin() }
            },
        ),
        channel(
            camera_id,
            "raw_accel_z",
            Unit::MeterPerSecondSquared,
            |_| 9.80665,
        ),
        channel(camera_id, "raw_gyro_x", Unit::RadianPerSecond, |_| 0.0),
        channel(camera_id, "raw_gyro_y", Unit::RadianPerSecond, |_| 0.0),
        channel(camera_id, "raw_gyro_z", Unit::RadianPerSecond, |_| 0.0),
    ] {
        camera_data.insert(item);
    }
    app.data
        .insert(camera_id, SourceData::from_dataset(Arc::new(camera_data)));
    app.video_sync
        .recording_mut(recording_id)
        .calibration_drafts
        .insert(
            camera_id,
            VehicleCalibrationDraft {
                source_time: true,
                auto_stationary: false,
                ..Default::default()
            },
        );

    app.apply_camera_calibration(recording_id, camera_id, None);

    assert!(app.data[&camera_id].processed.named("lateral_g").is_some());
    assert!(
        app.workspace.recordings[0]
            .overlay_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.camera_calibration.notes.as_ref())
            .is_some()
    );
    let (target, camera) = app
        .default_video_alignment_channels(logger_id, camera_id)
        .unwrap();
    assert_eq!(
        app.data[&logger_id]
            .processed
            .channel(target.channel_id)
            .unwrap()
            .descriptor
            .name,
        "gps_lateral_acceleration"
    );
    assert_eq!(
        app.data[&camera_id]
            .processed
            .channel(camera.channel_id)
            .unwrap()
            .descriptor
            .name,
        "lateral_g"
    );
}

fn source_config(id: SourceId, name: &str, adapter: &str, path: &str) -> SourceConfig {
    SourceConfig {
        id,
        name: name.into(),
        adapter: adapter.into(),
        path: path.into(),
        alignment: Default::default(),
        settings: json!({}),
        unknown: Default::default(),
    }
}

fn recording_of(name: &str, sources: Vec<SourceConfig>, video: Option<&str>) -> Recording {
    Recording {
        id: RecordingId::new(),
        name: name.into(),
        primary_source: sources[0].id,
        sources,
        video_path: video.map(Into::into),
        video_processing: None,
        video_offset_seconds: 0.0,
        segments: vec![],
        overlay_snapshot: None,
        unknown: Default::default(),
    }
}

/// A channel with samples only at `start` and `end`, enough to give a dataset
/// a time span.
fn spanning_dataset(
    source_id: SourceId,
    start: f64,
    end: f64,
    metadata: &[(&str, &str)],
) -> SourceData {
    let mut dataset = TelemetryDataset {
        source_id,
        metadata: metadata
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        ..Default::default()
    };
    let mut item = channel(source_id, "gps_speed", Unit::MeterPerSecond, |_| 1.0);
    item.series = ChannelSeries::new(vec![
        TimedSample {
            time: start,
            value: 1.0,
        },
        TimedSample {
            time: end,
            value: 1.0,
        },
    ]);
    dataset.insert(item);
    SourceData::from_dataset(Arc::new(dataset))
}

/// Camera telemetry with a still start (so it can be calibrated) and a moving
/// remainder.
fn camera_imu(camera_id: SourceId) -> SourceData {
    let mut dataset = TelemetryDataset {
        source_id: camera_id,
        ..Default::default()
    };
    for item in [
        channel(
            camera_id,
            "raw_accel_x",
            Unit::MeterPerSecondSquared,
            |_| 0.0,
        ),
        channel(
            camera_id,
            "raw_accel_y",
            Unit::MeterPerSecondSquared,
            |time| {
                if time <= 2.0 { 0.0 } else { time.sin() }
            },
        ),
        channel(
            camera_id,
            "raw_accel_z",
            Unit::MeterPerSecondSquared,
            |_| 9.80665,
        ),
        channel(camera_id, "raw_gyro_x", Unit::RadianPerSecond, |_| 0.0),
        channel(camera_id, "raw_gyro_y", Unit::RadianPerSecond, |_| 0.0),
        channel(camera_id, "raw_gyro_z", Unit::RadianPerSecond, |_| 0.0),
    ] {
        dataset.insert(item);
    }
    SourceData::from_dataset(Arc::new(dataset))
}

#[test]
fn a_new_camera_is_calibrated_with_the_default_orientation_once() {
    let camera_id = SourceId::new();
    let recording = recording_of(
        "clip",
        vec![source_config(camera_id, "camera", "insta360", "camera.lrv")],
        None,
    );
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![recording];
    app.data.insert(camera_id, camera_imu(camera_id));

    app.apply_default_camera_calibrations();

    assert!(app.data[&camera_id].processed.named("lateral_g").is_some());
    assert!(
        app.workspace.recordings[0]
            .overlay_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.camera_calibration.notes.is_some())
    );
    assert!(app.message().contains("faces the direction of travel"));

    // A later manual change is not overwritten.
    app.message.clear();
    let calibrated = app.data[&camera_id].processed.clone();
    app.apply_default_camera_calibrations();
    assert!(app.message().is_empty());
    assert!(Arc::ptr_eq(&calibrated, &app.data[&camera_id].processed));
}

#[test]
fn default_calibration_waits_for_the_audio_match_when_there_is_a_video() {
    let camera_id = SourceId::new();
    let recording = recording_of(
        "clip",
        vec![source_config(camera_id, "camera", "insta360", "camera.lrv")],
        Some("clip.insv"),
    );
    let recording_id = recording.id;
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![recording];
    app.data.insert(camera_id, camera_imu(camera_id));

    app.apply_default_camera_calibrations();
    assert!(app.data[&camera_id].processed.named("lateral_g").is_none());

    // The audio match arrives; video time can now be used.
    CameraVideoSyncMetadata {
        camera_minus_video_seconds: Some(0.0),
        audio_peak: Some(1.0),
        audio_confidence: Some(10.0),
        applied: false,
    }
    .write(&mut app.workspace.recordings[0].sources[0].settings);
    app.apply_default_camera_calibrations();
    assert!(app.data[&camera_id].processed.named("lateral_g").is_some());
    assert!(
        app.video_sync
            .recording(recording_id)
            .is_some_and(|state| state.auto_calibration_tried.contains(&camera_id))
    );
}

fn paired_fixture() -> (AnalysisApp, RecordingId, RecordingId, SourceId, SourceId) {
    let (camera_id, logger_id) = (SourceId::new(), SourceId::new());
    let video = recording_of(
        "VID_20260830_124108_00_017",
        vec![source_config(
            camera_id,
            "camera",
            "insta360",
            "VID_20260830_124108_00_017.insv",
        )],
        Some("VID_20260830_124108_00_017.insv"),
    );
    let log = recording_of(
        "a_0082",
        vec![source_config(
            logger_id,
            "a_0082",
            "synthetic",
            "a_0082.xrk",
        )],
        None,
    );
    let (video_id, log_id) = (video.id, log.id);
    let mut app = AnalysisApp::new();
    app.workspace.recordings = vec![video, log];
    app.data
        .insert(camera_id, spanning_dataset(camera_id, -0.28, 189.67, &[]));
    app.data.insert(
        logger_id,
        spanning_dataset(
            logger_id,
            0.0,
            171.8,
            &[("date", "08/30/2026"), ("time", "12:42:36")],
        ),
    );
    (app, video_id, log_id, camera_id, logger_id)
}

#[test]
fn a_log_recorded_during_a_video_joins_its_entry_and_becomes_its_clock() {
    let (mut app, video_id, log_id, camera_id, logger_id) = paired_fixture();
    // The camera's own whole-recording interval, accepted as the clock.
    app.workspace.recordings[0].segments = vec![RunSegment {
        id: SegmentId::new(),
        name: "Recording interval (no GPS)".into(),
        start_recording_time: -0.28,
        end_recording_time: 189.67,
        kind: SegmentKind::Unknown,
        estimated: true,
        competitive: true,
        unknown: Default::default(),
    }];
    CameraVideoSyncMetadata {
        camera_minus_video_seconds: Some(0.0),
        audio_peak: Some(1.0),
        audio_confidence: Some(10.0),
        applied: true,
    }
    .write(&mut app.workspace.recordings[0].sources[0].settings);

    app.pair_loggers_with_cameras();

    assert_eq!(app.workspace.recordings.len(), 1);
    let entry = &app.workspace.recordings[0];
    assert_eq!(entry.id, video_id);
    assert!(entry.sources.iter().any(|source| source.id == logger_id));
    assert_eq!(entry.primary_source, logger_id);
    assert!(
        !CameraVideoSyncMetadata::read(&entry.sources[0].settings).applied,
        "the camera must be matched to the log before it counts as synchronized"
    );
    // The camera's whole-video interval gave way to one from the log.
    assert_eq!(entry.segments.len(), 1);
    assert!((entry.segments[0].end_recording_time - 171.8).abs() < 1e-9);
    assert!(
        app.video_sync
            .recording(video_id)
            .unwrap()
            .auto_sync_pending
    );
    assert!(app.video_sync.recording(log_id).is_none());
    assert_eq!(app.state.selected_recording, Some(video_id));
    let _ = camera_id;
}

#[test]
fn logs_that_were_not_recorded_during_the_video_stay_separate() {
    let (mut app, ..) = paired_fixture();
    let logger_id = app.workspace.recordings[1].sources[0].id;
    app.data.insert(
        logger_id,
        spanning_dataset(
            logger_id,
            0.0,
            150.0,
            &[("date", "08/30/2026"), ("time", "12:48:19")],
        ),
    );

    app.pair_loggers_with_cameras();

    assert_eq!(app.workspace.recordings.len(), 2);
}

#[test]
fn a_log_added_to_a_video_is_synced_only_when_the_video_has_none() {
    let (mut app, video_id, log_id, ..) = paired_fixture();
    app.workspace.recordings.remove(1);
    app.attach_source(video_id, "a_0082.xrk".into());
    assert!(
        app.video_sync
            .recording(video_id)
            .unwrap()
            .auto_sync_pending
    );
    assert_eq!(app.workspace.recordings[0].sources.len(), 2);
    let _ = log_id;

    // A second log on an entry that already has one must not redo its sync.
    app.video_sync.recording_mut(video_id).auto_sync_pending = false;
    let first_log = app.workspace.recordings[0].sources[1].id;
    app.workspace.recordings[0].primary_source = first_log;
    app.attach_source(video_id, "a_0083.xrk".into());
    assert!(
        !app.video_sync
            .recording(video_id)
            .unwrap()
            .auto_sync_pending
    );
}

#[test]
fn a_logger_promoted_over_a_camera_keeps_intervals_the_user_made() {
    let (mut app, video_id, _, _, logger_id) = paired_fixture();
    let own = RunSegment {
        id: SegmentId::new(),
        name: "My lap".into(),
        start_recording_time: 10.0,
        end_recording_time: 50.0,
        kind: SegmentKind::Lap,
        estimated: false,
        competitive: true,
        unknown: Default::default(),
    };
    app.workspace.recordings[0].segments = vec![own.clone()];
    let logger_source = app.workspace.recordings[1].sources[0].clone();
    app.workspace.recordings[0].sources.push(logger_source);

    let data = app.data[&logger_id].clone();
    assert!(app.promote_logger_to_primary(logger_id, &data));

    let entry = app
        .workspace
        .recordings
        .iter()
        .find(|recording| recording.id == video_id)
        .unwrap();
    assert_eq!(entry.primary_source, logger_id);
    assert_eq!(entry.segments, vec![own]);
}

fn auto_sync_candidate(
    app: &AnalysisApp,
    recording_id: RecordingId,
    correlation: f64,
    overlap: f64,
) -> VideoAlignmentCandidate {
    let recording = &app.workspace.recordings[0];
    let (target_source, camera_source) = (recording.primary_source, recording.sources[0].id);
    let channel_of = |source: SourceId| ChannelRef {
        source_id: source,
        channel_id: *app.data[&source].processed.channels.keys().next().unwrap(),
    };
    VideoAlignmentCandidate {
        recording_id,
        target_source,
        camera_source,
        target_channel: channel_of(target_source),
        camera_channel: channel_of(camera_source),
        camera_video_offset_seconds: 0.0,
        result: CorrelationResult {
            target_minus_reference_seconds: -2.793,
            correlation_coefficient: correlation,
            sample_count: 10_000,
            overlap_seconds: overlap,
            lag_resolution_seconds: 0.018,
        },
    }
}

fn estimated_fixture(correlation: f64, overlap: f64) -> (AnalysisApp, RecordingId) {
    let (mut app, video_id, _, _, _) = paired_fixture();
    app.pair_loggers_with_cameras();
    CameraVideoSyncMetadata {
        camera_minus_video_seconds: Some(0.0),
        audio_peak: Some(1.0),
        audio_confidence: Some(10.0),
        applied: false,
    }
    .write(&mut app.workspace.recordings[0].sources[0].settings);
    let candidate = auto_sync_candidate(&app, video_id, correlation, overlap);
    let state = app.video_sync.recording_mut(video_id);
    state.channels = Some((
        candidate.target_channel.clone(),
        candidate.camera_channel.clone(),
    ));
    state.alignment_job = Some(7);
    state.auto_sync_apply = true;
    app.handle_video_alignment_estimated(7, video_id, Ok(candidate));
    (app, video_id)
}

#[test]
fn a_strong_automatic_match_is_applied_to_the_video_and_camera() {
    let (app, _) = estimated_fixture(0.64, 171.3);
    let entry = &app.workspace.recordings[0];
    assert!((entry.video_offset_seconds - 2.793).abs() < 1e-9);
    assert!((entry.sources[0].alignment.offset_seconds - 2.793).abs() < 1e-9);
    assert!(CameraVideoSyncMetadata::read(&entry.sources[0].settings).applied);
    assert!(app.message().starts_with("Synchronized automatically."));
}

#[test]
fn a_weak_automatic_match_is_left_for_review() {
    for (correlation, overlap) in [(0.33, 171.3), (0.64, 12.0)] {
        let (app, video_id) = estimated_fixture(correlation, overlap);
        let entry = &app.workspace.recordings[0];
        assert_eq!(entry.video_offset_seconds, 0.0);
        assert!(!CameraVideoSyncMetadata::read(&entry.sources[0].settings).applied);
        assert!(app.message().contains("weak match"));
        assert!(
            app.video_sync
                .recording(video_id)
                .unwrap()
                .alignment_result
                .is_some(),
            "the candidate stays visible for manual review"
        );
    }
}

#[test]
fn the_video_offset_can_be_set_by_hand_and_moves_the_camera_with_it() {
    let (mut app, video_id) = estimated_fixture(0.3, 171.3);
    CameraVideoSyncMetadata {
        camera_minus_video_seconds: Some(0.4),
        audio_peak: Some(1.0),
        audio_confidence: Some(10.0),
        applied: false,
    }
    .write(&mut app.workspace.recordings[0].sources[0].settings);

    app.set_video_offset_by_hand(video_id, 5.0);

    let entry = &app.workspace.recordings[0];
    assert_eq!(entry.video_offset_seconds, 5.0);
    assert!((entry.sources[0].alignment.offset_seconds - 5.4).abs() < 1e-9);
    assert!(CameraVideoSyncMetadata::read(&entry.sources[0].settings).applied);
}
