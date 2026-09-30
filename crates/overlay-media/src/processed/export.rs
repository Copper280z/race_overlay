use super::*;
use crate::{CancelToken, ExportProgress, ExportSettings, probe_video};
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
/// Encode caller-transformed frames while muxing the original camera audio.
#[allow(clippy::too_many_arguments)]
pub fn export_processed_video(
    tools: &FfmpegTools,
    input: &Path,
    output: &Path,
    settings: &ExportSettings,
    size: PreviewSize,
    processor: &mut dyn VideoFrameProcessor,
    cancel: &CancelToken,
    mut progress: impl FnMut(ExportProgress),
) -> Result<(), MediaError> {
    crate::export::validate_settings(settings)?;
    if output == input
        || output
            .canonicalize()
            .ok()
            .zip(input.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
    {
        return Err(MediaError::Invalid(
            "Output must differ from source video".into(),
        ));
    }
    if output.exists() && !settings.overwrite {
        return Err(MediaError::Invalid("Output already exists".into()));
    }
    let info = probe_dual_video(tools, input)?;
    let metadata = probe_video(tools, input)?;
    let encoder = crate::export::choose_encoder(settings, &metadata, &tools.encoders()?)?;
    let part = crate::export::part_path(output);
    if part.exists() {
        return Err(MediaError::Invalid(format!(
            "Partial output already exists: {}",
            part.display()
        )));
    }
    let mut command = Command::new(&tools.ffmpeg);
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-n",
            "-copyts",
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &format!("{}x{}", size.width, size.height),
            "-framerate",
            &info.fps.to_string(),
            "-i",
            "pipe:0",
            "-itsoffset",
            &(-info.start).to_string(),
            "-i",
        ])
        .arg(input)
        .args([
            "-map",
            "0:v:0",
            "-map",
            "1:a?",
            "-vf",
            &format!(
                "scale=in_range=full:out_range={}:out_color_matrix=bt709",
                if info.full_range { "full" } else { "limited" }
            ),
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            if info.full_range { "pc" } else { "tv" },
            "-c:v",
            &encoder,
            "-c:a",
            "copy",
            "-pix_fmt",
            settings.pixel_format.as_deref().unwrap_or("yuv420p"),
        ]);
    if let Some(quality) = settings.quality {
        command.args(["-crf", &quality.to_string()]);
    } else if let Some(rate) = settings.bitrate.or(metadata.bit_rate) {
        command.args(["-b:v", &rate.to_string()]);
    }
    if let Some(preset) = &settings.preset
        && (encoder == "libx264" || encoder == "libx265")
    {
        command.args(["-preset", preset]);
    }
    if settings.fast_start {
        command.args(["-movflags", "+faststart"]);
    }
    if settings.apple_compatible {
        command.args([
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            if info.full_range { "pc" } else { "tv" },
        ]);
        if encoder.contains("hevc") || encoder.contains("265") {
            command.args(["-tag:v", "hvc1"]);
        }
    }
    let mut child = command
        .arg(&part)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("encoder stdin");
    let mut stderr = child.stderr.take().expect("encoder stderr");
    let diagnostic = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        while let Ok(n) = stderr.read(&mut buffer) {
            if n == 0 {
                break;
            }
            let keep = n.min(8192_usize.saturating_sub(bytes.len()));
            bytes.extend_from_slice(&buffer[..keep]);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let child = Arc::new(Mutex::new(child));
    let finished = Arc::new(AtomicBool::new(false));
    let kill_child = child.clone();
    let done = finished.clone();
    let token = cancel.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let monitor = std::thread::spawn(move || {
        while !done.load(Ordering::Acquire) {
            if token.is_cancelled() {
                flag.store(true, Ordering::Release);
                let _ = kill_child.lock().unwrap().kill();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    let result = (|| {
        let mut decoder = DualVideoDecoder::start(tools, input, &info, 0., None)?;
        let mut count = 0_u64;
        while let Some(frame) = decoder.next(&cancelled)? {
            if cancel.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            let expected = count as f64 / info.fps.as_f64();
            if (frame.timestamp - expected).abs() > 0.0001 {
                return Err(MediaError::Invalid(
                    "Raw export requires continuous constant-rate lens timestamps".into(),
                ));
            }
            let pixels = processor.process(&frame, size)?;
            if pixels.len() != size.width as usize * size.height as usize * 4 {
                return Err(MediaError::Invalid(
                    "Video processor returned an incomplete export frame".into(),
                ));
            }
            stdin.write_all(&pixels)?;
            count += 1;
            progress(ExportProgress {
                encoded_seconds: frame.timestamp,
                duration_seconds: Some(info.duration),
            });
        }
        if count == 0 || (count as f64 - info.duration * info.fps.as_f64()).abs() > 1.0 {
            return Err(MediaError::Invalid(
                "Raw video ended before its declared frame coverage".into(),
            ));
        }
        Ok(())
    })();
    drop(stdin);
    if result.is_err() {
        let _ = child.lock().unwrap().kill();
    }
    let status = loop {
        if let Some(status) = child.lock().unwrap().try_wait()? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    finished.store(true, Ordering::Release);
    let _ = monitor.join();
    let diagnostic = diagnostic.join().unwrap_or_default();
    let result = if cancel.is_cancelled() {
        Err(MediaError::Cancelled)
    } else {
        result.and_then(|()| {
            if status.success() {
                Ok(())
            } else {
                Err(MediaError::Process(diagnostic))
            }
        })
    };
    match result {
        Ok(()) => {
            // Persist uses atomic replacement and preserves the old file if publishing fails.
            let temporary = tempfile::TempPath::try_from_path(&part)?;
            if settings.overwrite {
                temporary.persist(output).map_err(|e| e.error)?;
            } else {
                temporary.persist_noclobber(output).map_err(|e| e.error)?;
            }
            progress(ExportProgress {
                encoded_seconds: info.duration,
                duration_seconds: Some(info.duration),
            });
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(part);
            Err(e)
        }
    }
}
