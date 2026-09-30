//! Explicit local-footage validation. Never runs as part of routine tests.
//! Usage: inspect_raw_video INPUT OUTPUT_DIRECTORY [START_SECONDS] [FRAME_COUNT] [paced]
use overlay_core::{Quaternion, VideoProcessingConfig, read_insta360_video};
use overlay_media::{DualVideoDecoder, FfmpegConfig, discover, probe_dual_video};
use overlay_render::video::{LensImages, VideoProjector};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().collect::<Vec<_>>();
    let path = PathBuf::from(args.get(1).ok_or("input required")?);
    let out = PathBuf::from(args.get(2).ok_or("output directory required")?);
    let start = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(60.);
    let count = args.get(4).map(|s| s.parse()).transpose()?.unwrap_or(1_u32);
    if count == 0 {
        return Err("frame count must be positive".into());
    }
    let paced = args.get(5).is_some_and(|v| v == "paced");
    let raw = read_insta360_video(&path)?;
    let tools = discover(&FfmpegConfig::default())?;
    let info = probe_dual_video(&tools, &path)?;
    println!(
        "{}; {} gyro samples; gyro coverage {:?}..{:?}; {} fps",
        raw.camera_model,
        raw.motion.samples.len(),
        raw.motion.samples.first().map(|s| s.time),
        raw.motion.samples.last().map(|s| s.time),
        info.fps
    );
    let motion = raw.motion.prepare(0.25)?;
    let mut renderer = VideoProjector::new();
    println!(
        "Projection backend: {}; GPU diagnostic: {:?}",
        renderer.backend(),
        renderer.gpu_error
    );
    let mut decoder = DualVideoDecoder::start(&tools, &path, &info, start, Some(1024))?;
    let stop = AtomicBool::new(false);
    std::fs::create_dir_all(&out)?;
    std::fs::write(
        out.join("optical.json"),
        serde_json::to_vec(
            &serde_json::json!({"lenses":raw.calibration.lenses.iter().map(|l|serde_json::json!({"xi":l.xi,"focal":l.focal,"center":l.center,"distortion":l.distortion,"rotation":l.rig_to_lens.matrix()})).collect::<Vec<_>>(),"gyro":raw.motion.samples.iter().map(|s|[s.time,s.radians_per_second[0],s.radians_per_second[1],s.radians_per_second[2]]).collect::<Vec<_>>()}),
        )?,
    )?;
    let mut config = VideoProcessingConfig::default();
    let mut durations = Vec::new();
    let mut cached_times = Vec::new();
    let mut render_times = Vec::new();
    let mut playback_start = None;
    let mut late = 0;
    let began = Instant::now();
    for index in 0..count {
        let frame_start = Instant::now();
        let frame = decoder.next(&stop)?.ok_or("unexpected EOF")?;
        let images = LensImages {
            width: frame.width,
            height: frame.height,
            pixels: [&frame.pixels[0], &frame.pixels[1]],
            timestamp: frame.timestamp,
        };
        let correction = motion
            .correction(frame.timestamp)
            .ok_or("no gyro coverage")?;
        let render_start = Instant::now();
        let pixels = renderer.render(&images, &raw.calibration, &config, correction, (960, 540))?;
        render_times.push(render_start.elapsed().as_secs_f64());
        durations.push(frame_start.elapsed().as_secs_f64());
        if index == 0 {
            image::save_buffer(
                out.join("stabilized.png"),
                &pixels,
                960,
                540,
                image::ColorType::Rgba8,
            )?;
            for yaw in [0., 90., 180., -90.] {
                config.view.yaw = yaw;
                let pixels = renderer.render(
                    &images,
                    &raw.calibration,
                    &config,
                    Quaternion::IDENTITY,
                    (960, 540),
                )?;
                image::save_buffer(
                    out.join(format!("view-{yaw}.png")),
                    &pixels,
                    960,
                    540,
                    image::ColorType::Rgba8,
                )?;
            }
            image::save_buffer(
                out.join("lens-front.png"),
                &frame.pixels[0],
                frame.width,
                frame.height,
                image::ColorType::Rgba8,
            )?;
            image::save_buffer(
                out.join("lens-rear.png"),
                &frame.pixels[1],
                frame.width,
                frame.height,
                image::ColorType::Rgba8,
            )?;
            for sample in 0..30 {
                config.view.yaw = sample as f64 * 3.;
                config.horizontal_fov_degrees = 60. + sample as f64;
                let now = Instant::now();
                renderer.render(&images, &raw.calibration, &config, correction, (960, 540))?;
                cached_times.push(now.elapsed().as_secs_f64());
            }
            config.view.yaw = 90.;
            config.horizontal_fov_degrees = 90.;
            playback_start = Some(Instant::now());
        }
        if paced && index > 0 {
            let clock = playback_start.unwrap();
            let deadline = std::time::Duration::from_secs_f64(index as f64 / info.fps.as_f64());
            if clock.elapsed() > deadline {
                late += 1;
            } else {
                std::thread::sleep(deadline - clock.elapsed());
            }
        }
    }
    let timed_frames = durations.len().saturating_sub(1);
    let missed = durations
        .iter()
        .skip(1)
        .filter(|t| **t > 1. / info.fps.as_f64())
        .count();
    durations.sort_by(f64::total_cmp);
    println!(
        "Frames: {count}; elapsed: {:.3}s; throughput: {:.2} fps; frame p95: {:.2} ms",
        began.elapsed().as_secs_f64(),
        count as f64 / began.elapsed().as_secs_f64(),
        durations[(durations.len() - 1) * 95 / 100] * 1000.
    );
    cached_times.sort_by(f64::total_cmp);
    render_times.sort_by(f64::total_cmp);
    println!(
        "Projection p95 {:.2} ms; cached view p95 {:.2} ms, max {:.2} ms; frames exceeding budget {missed}/{timed_frames}; paced late frames {late}/{timed_frames}",
        render_times[(render_times.len() - 1) * 95 / 100] * 1000.,
        cached_times[(cached_times.len() - 1) * 95 / 100] * 1000.,
        cached_times.last().unwrap() * 1000.
    );
    Ok(())
}
