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
