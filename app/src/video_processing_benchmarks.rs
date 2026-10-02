//! Opt-in release benchmarks over local footage; no machine-speed assertions.
use super::*;
use std::{hint::black_box, time::Duration};

fn report(label: &str, size: PreviewSize, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let median = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    eprintln!(
        "{label} {}×{}: median {:.2} ms, p95 {:.2} ms, mean {:.2} ms ({:.1} updates/s)",
        size.width,
        size.height,
        median * 1000.0,
        p95 * 1000.0,
        mean * 1000.0,
        1.0 / mean
    );
}

fn wait(preview: &VideoPreview, target: f64, step: f64) -> PreviewFrame {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(frame) = preview.try_recv() {
            let frame = frame.expect("preview frame");
            if (frame.timestamp - target).abs() < step * 0.6 {
                return frame;
            }
        }
        assert!(
            Instant::now() < deadline,
            "preview stalled: {}",
            preview.status()
        );
        std::thread::sleep(Duration::from_micros(500));
    }
}

#[test]
#[ignore = "release benchmark; needs GPU, FFmpeg, local INSV and its Studio MP4 export"]
fn benchmark_preview_resolutions() {
    if cfg!(debug_assertions) {
        panic!("run this benchmark with --release");
    }
    let tools = overlay_media::discover(&overlay_media::FfmpegConfig::default()).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let raw = root.join("VID_20260830_124108_00_017.insv");
    let flat = root.join("VID_20260830_124108_00_017.mp4");
    let info = overlay_media::probe_dual_video(&tools, &raw).unwrap();
    let step = 1.0 / info.fps.as_f64();
    let mut config = VideoProcessingConfig::default();
    config.stabilization.enabled = true;
    let settings = Arc::new(Mutex::new(Settings {
        config: config.clone(),
        revision: 0,
    }));
    let mut processor = RawProcessor::new(&raw, settings).unwrap();
    assert_eq!(processor.projector.backend(), "wgpu");
    let mut decoder =
        overlay_media::DualVideoDecoder::start(&tools, &raw, &info, 5.0, Some(1024)).unwrap();
    let mut frame = decoder.next(&AtomicBool::new(false)).unwrap().unwrap();
    drop(decoder);
    eprintln!(
        "raw lens proxy: {}×{}, stabilized default projection, {:.3} source fps",
        frame.width,
        frame.height,
        info.fps.as_f64()
    );
    let sizes = [PreviewSize::new(960, 540), PreviewSize::new(1920, 1080)];
    for size in sizes {
        for _ in 0..8 {
            black_box(processor.process(&frame, size).unwrap());
        }
        let mut samples = Vec::new();
        for _ in 0..90 {
            // Upload each frame as playback does; do not measure only a cached
            // paused frame's projection. Pixels stay fixed to isolate size.
            frame.timestamp += step;
            let at = Instant::now();
            black_box(processor.process(&frame, size).unwrap());
            samples.push(at.elapsed().as_secs_f64());
        }
        report("GPU projection + upload/readback", size, samples);
    }
    for (label, path, config) in [
        ("MP4 analysis backend", flat, None),
        ("INSV stabilized backend", raw, Some(config)),
    ] {
        // Reverse the order on the second pass to reduce order/cache bias.
        let mut samples = [Vec::new(), Vec::new()];
        for order in [[0, 1], [1, 0]] {
            for index in order {
                let size = sizes[index];
                let preview = VideoPreview::spawn(
                    tools.clone(),
                    path.clone(),
                    config.clone(),
                    Some(info.fps.as_f64()),
                    false,
                );
                for i in 0..8 {
                    let target = 5.0 + f64::from(i) * step;
                    preview.request(target, size).unwrap();
                    black_box(wait(&preview, target, step));
                }
                for i in 8..38 {
                    let target = 5.0 + f64::from(i) * step;
                    let at = Instant::now();
                    preview.request(target, size).unwrap();
                    black_box(wait(&preview, target, step));
                    samples[index].push(at.elapsed().as_secs_f64());
                }
                drop(preview);
                // Give the canceled decoder time to release its resources.
                std::thread::sleep(Duration::from_millis(150));
            }
        }
        for (size, samples) in sizes.into_iter().zip(samples) {
            report(label, size, samples);
        }
    }
}

#[test]
#[ignore = "release benchmark; needs GPU, FFmpeg and local INSV footage"]
fn benchmark_preview_lens_resolutions() {
    if cfg!(debug_assertions) {
        panic!("run this benchmark with --release");
    }
    let tools = overlay_media::discover(&overlay_media::FfmpegConfig::default()).unwrap();
    let raw = Path::new(env!("CARGO_MANIFEST_DIR")).join("../VID_20260830_124108_00_017.insv");
    let info = overlay_media::probe_dual_video(&tools, &raw).unwrap();
    let mut config = VideoProcessingConfig::default();
    config.stabilization.enabled = true;
    let mut processor = RawProcessor::new(
        &raw,
        Arc::new(Mutex::new(Settings {
            config,
            revision: 0,
        })),
    )
    .unwrap();
    assert_eq!(processor.projector.backend(), "wgpu");
    let output = PreviewSize::new(1920, 1080);
    let lenses = [1024, 1536, 2048, info.width];
    let mut pipeline = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    let mut projection = pipeline.clone();
    let cancel = AtomicBool::new(false);
    let snapshots = std::env::var_os("RACE_OVERLAY_PREVIEW_BENCH_DIR").map(PathBuf::from);
    if let Some(dir) = &snapshots {
        std::fs::create_dir_all(dir).unwrap();
    }
    for (pass, order) in [[0, 1, 2, 3], [3, 2, 1, 0]].into_iter().enumerate() {
        for index in order {
            let lens = lenses[index];
            let mut decoder =
                overlay_media::DualVideoDecoder::start(&tools, &raw, &info, 5., Some(lens))
                    .unwrap();
            let mut previous: Option<f64> = None;
            for i in 0..128 {
                let at = Instant::now();
                let frame = decoder.next(&cancel).unwrap().expect("paired lens frame");
                assert_eq!((frame.width, frame.height), (lens, lens));
                if let Some(previous) = previous {
                    assert!((frame.timestamp - previous - 1. / info.fps.as_f64()).abs() < 1e-6);
                }
                previous = Some(frame.timestamp);
                let process_at = Instant::now();
                let pixels = processor.process(&frame, output).unwrap();
                let process_seconds = process_at.elapsed().as_secs_f64();
                drop(frame);
                let pixels = black_box(pixels);
                let pipeline_seconds = at.elapsed().as_secs_f64();
                if i >= 8 {
                    pipeline[index].push(pipeline_seconds);
                    projection[index].push(process_seconds);
                } else if pass == 0
                    && i == 0
                    && let Some(dir) = &snapshots
                {
                    image::save_buffer(
                        dir.join(format!("lens-{lens}-output-1080.png")),
                        &pixels,
                        output.width,
                        output.height,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                }
            }
            drop(decoder);
        }
    }
    eprintln!(
        "stabilized 1920×1080 output, 240 timed frames per lens size, two reversed-order passes"
    );
    for (index, lens) in lenses.into_iter().enumerate() {
        let size = PreviewSize::new(lens, lens);
        report(
            "Paired decode + projection; lens size",
            size,
            std::mem::take(&mut pipeline[index]),
        );
        report(
            "GPU projection + source upload/readback; lens size",
            size,
            std::mem::take(&mut projection[index]),
        );
    }
}
