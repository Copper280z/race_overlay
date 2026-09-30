//! Explicit FFmpeg integration checks using generated media, never local recordings.
use super::*;
use crate::{CancelToken, ExportSettings, FfmpegConfig, discover};
use std::{path::Path, process::Command, time::Instant};
fn fixture(tools: &FfmpegTools, path: &Path) {
    let output = Command::new(&tools.ffmpeg)
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=red:size=64x64:rate=30000/1001:duration=1.001",
            "-f",
            "lavfi",
            "-i",
            "color=blue:size=64x64:rate=30000/1001:duration=1.001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=1.001",
            "-map",
            "0:v",
            "-map",
            "1:v",
            "-map",
            "2:a",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
        ])
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn audio_hashes(tools: &FfmpegTools, path: &Path) -> Vec<String> {
    let output = Command::new(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a",
            "-show_packets",
            "-show_data_hash",
            "sha256",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    json["packets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["data_hash"].as_str().unwrap().to_owned())
        .collect()
}
#[test]
#[ignore = "requires FFmpeg with libx264; generates dual-track media"]
fn paired_decode_export_and_audio_preserve_pts_and_rational_rate() {
    let tools = discover(&FfmpegConfig::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("dual.mp4");
    fixture(&tools, &input);
    let info = probe_dual_video(&tools, &input).unwrap();
    assert_eq!(info.fps.to_string(), "30000/1001");
    let mut decoder = DualVideoDecoder::start(&tools, &input, &info, 0.5, Some(32)).unwrap();
    let stop = AtomicBool::new(false);
    let f = decoder.next(&stop).unwrap().unwrap();
    assert!((f.timestamp - 0.5005).abs() < 0.0001);
    assert!(f.pixels[0][0] > 240);
    assert!(f.pixels[1][2] > 240);
    drop(decoder);
    let output = dir.path().join("export.mp4");
    let mut times = vec![];
    let mut processor = |frame: &LensFramePair, size: PreviewSize| {
        times.push(frame.timestamp);
        Ok([180, 60, 20, 255].repeat(size.width as usize * size.height as usize))
    };
    export_processed_video(
        &tools,
        &input,
        &output,
        &ExportSettings {
            encoder: Some("libx264".into()),
            ..Default::default()
        },
        PreviewSize::new(160, 90),
        &mut processor,
        &CancelToken::new(),
        |_| {},
    )
    .unwrap();
    assert_eq!(audio_hashes(&tools, &input), audio_hashes(&tools, &output));
    let metadata = crate::probe_video(&tools, &output).unwrap();
    assert_eq!((metadata.width, metadata.height), (160, 90));
    assert!((metadata.fps().unwrap() - 30000. / 1001.).abs() < 1e-9);
    let old = std::fs::read(&output).unwrap();
    let cancelled = CancelToken::new();
    cancelled.cancel();
    assert!(matches!(
        export_processed_video(
            &tools,
            &input,
            &output,
            &ExportSettings::default(),
            PreviewSize::new(160, 90),
            &mut processor,
            &cancelled,
            |_| {}
        ),
        Err(MediaError::Cancelled)
    ));
    assert_eq!(std::fs::read(&output).unwrap(), old);
    assert_eq!(times.len(), 30);
    assert!(
        times
            .iter()
            .enumerate()
            .all(|(i, t)| (t - i as f64 * 1001. / 30000.).abs() < 0.0001)
    );
}
#[test]
#[ignore = "requires FFmpeg; checks paused reframing and obsolete result rejection"]
fn preview_revision_rerenders_cached_frame_without_redecoding() {
    let tools = discover(&FfmpegConfig::default()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("dual.mp4");
    fixture(&tools, &input);
    let value = Arc::new(AtomicU64::new(10));
    let shared = value.clone();
    let preview = ProcessedPreview::spawn(tools, input, move |_| {
        Ok(Box::new(move |_: &LensFramePair, size: PreviewSize| {
            Ok([shared.load(Ordering::Acquire) as u8, 0, 0, 255]
                .repeat(size.width as usize * size.height as usize))
        }))
    });
    let receive = || {
        let start = Instant::now();
        loop {
            if let Some(result) = preview.try_recv() {
                break result.unwrap();
            }
            assert!(start.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    preview.request(0., PreviewSize::new(160, 90), 0).unwrap();
    let a = receive();
    assert_eq!(a.rgba[0], 10);
    value.store(50, Ordering::Release);
    preview.request(0., PreviewSize::new(160, 90), 1).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    value.store(90, Ordering::Release);
    preview.request(0., PreviewSize::new(160, 90), 2).unwrap();
    let b = receive();
    assert_eq!(b.rgba[0], 90);
    assert_eq!(a.timestamp, b.timestamp);
}
