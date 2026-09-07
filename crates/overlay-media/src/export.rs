use crate::{Encoder, EncoderCapabilities, FfmpegTools, MediaError, VideoMetadata, probe_video};
use crossbeam_channel::Receiver;
use std::{
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub type ExportError = MediaError;

fn part_path(output: &Path) -> PathBuf {
    let parent = output.parent().unwrap_or_else(|| Path::new(""));
    match (output.file_stem(), output.extension()) {
        (Some(stem), Some(ext)) => parent.join(format!(
            "{}.part.{}",
            stem.to_string_lossy(),
            ext.to_string_lossy()
        )),
        _ => PathBuf::from(format!("{}.part", output.to_string_lossy())),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl WidgetGeometry {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportSettings {
    /// An encoder name passed directly to FFmpeg. This is an advanced escape
    /// hatch; leaving it unset selects an encoder for `codec` automatically.
    pub encoder: Option<String>,
    /// Output codec. `None` means match the source codec (when supported).
    pub codec: Option<Encoder>,
    /// Target video bitrate in bits per second. When unset, the source bitrate
    /// is retained unless CRF `quality` is selected.
    pub bitrate: Option<u64>,
    /// Constant-rate-factor quality for software H.264/H.265 encoders. The
    /// usual useful range is 0 (lossless) through 51 (worst quality).
    pub quality: Option<u8>,
    /// Encoder preset (for example, `fast`, `medium`, or `slow`). Presets are
    /// passed to libx264/libx265 only; hardware encoders have their own knobs.
    pub preset: Option<String>,
    /// Pixel format for the composited video. `yuv420p` is the default because
    /// it is accepted by QuickTime, iOS, and common web players.
    pub pixel_format: Option<String>,
    /// Add MP4 codec tags and BT.709/YUV420 defaults that make output broadly
    /// playable by Apple media frameworks.
    pub apple_compatible: bool,
    /// Place the MP4 index at the beginning so playback can start while the
    /// file is still being read or streamed.
    pub fast_start: bool,
    pub overwrite: bool,
}
impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            encoder: None,
            codec: None,
            bitrate: None,
            quality: None,
            preset: None,
            pixel_format: Some("yuv420p".into()),
            apple_compatible: true,
            fast_start: true,
            overwrite: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RgbaFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
impl RgbaFrame {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, MediaError> {
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(MediaError::Invalid(
                "RGBA frame is not tightly packed".into(),
            ));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

pub trait FrameProvider {
    fn next_frame(&mut self) -> Result<Option<RgbaFrame>, MediaError>;
}
impl<F> FrameProvider for F
where
    F: FnMut() -> Result<Option<RgbaFrame>, MediaError>,
{
    fn next_frame(&mut self) -> Result<Option<RgbaFrame>, MediaError> {
        self()
    }
}
impl FrameProvider for Receiver<RgbaFrame> {
    fn next_frame(&mut self) -> Result<Option<RgbaFrame>, MediaError> {
        self.recv()
            .map(Some)
            .map_err(|_| MediaError::Process("frame channel closed".into()))
    }
}

struct CancellableChannelProvider {
    frames: Receiver<RgbaFrame>,
    cancel: CancelToken,
}
impl FrameProvider for CancellableChannelProvider {
    fn next_frame(&mut self) -> Result<Option<RgbaFrame>, MediaError> {
        loop {
            if self.cancel.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            match self.frames.recv_timeout(Duration::from_millis(25)) {
                Ok(frame) => return Ok(Some(frame)),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return Ok(None),
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExportProgress {
    pub encoded_seconds: f64,
    pub duration_seconds: Option<f64>,
}
#[derive(Debug, Clone)]
pub struct ExportCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}
impl ExportCommand {
    pub fn args(&self) -> &[String] {
        &self.args
    }
}

pub fn build_export_command(
    video: impl AsRef<Path>,
    output: impl AsRef<Path>,
    settings: &ExportSettings,
    geometry: WidgetGeometry,
    metadata: &VideoMetadata,
    encoder: &str,
) -> ExportCommand {
    let fps = metadata
        .frame_rate
        .map(|r| r.to_string())
        .unwrap_or_else(|| "30/1".into());
    let size = format!("{}x{}", geometry.width, geometry.height);
    let mut args = vec!["-hide_banner".into()];
    args.push(if settings.overwrite {
        "-y".into()
    } else {
        "-n".into()
    });
    args.extend([
        "-i".into(),
        video.as_ref().to_string_lossy().into_owned(),
        "-f".into(),
        "rawvideo".into(),
        "-pix_fmt".into(),
        "rgba".into(),
        "-s".into(),
        size,
        "-framerate".into(),
        fps,
        "-i".into(),
        "pipe:0".into(),
        "-filter_complex".into(),
        {
            let overlay = format!(
                "[0:v][1:v]overlay={}:{}:format=auto",
                geometry.x, geometry.y
            );
            match settings.pixel_format.as_deref() {
                Some(pixel_format) => format!("{overlay},format={pixel_format}[v]"),
                None => format!("{overlay}[v]"),
            }
        },
        "-map".into(),
        "[v]".into(),
        "-map".into(),
        "0:a?".into(),
        "-c:v".into(),
        encoder.into(),
        "-c:a".into(),
        "copy".into(),
        "-map_metadata".into(),
        "0".into(),
    ]);
    if let Some(pixel_format) = &settings.pixel_format {
        args.extend(["-pix_fmt".into(), pixel_format.clone()]);
    }
    // CRF is a quality target and should not be combined with the source or
    // user bitrate. It is meaningful for the software x264/x265 encoders.
    if let Some(quality) = settings.quality
        && (encoder.eq_ignore_ascii_case("libx264") || encoder.eq_ignore_ascii_case("libx265"))
    {
        args.extend(["-crf".into(), quality.to_string()]);
    } else if let Some(rate) = settings.bitrate.or(metadata.bit_rate) {
        args.extend(["-b:v".into(), format!("{}", rate)]);
    }
    if let Some(preset) = &settings.preset
        && (encoder.eq_ignore_ascii_case("libx264") || encoder.eq_ignore_ascii_case("libx265"))
    {
        args.extend(["-preset".into(), preset.clone()]);
    }
    if encoder.ends_with("_videotoolbox") {
        // VideoToolbox can reject otherwise valid dimensions or be unavailable
        // when its hardware session is busy. Let macOS fall back to its
        // software implementation instead of failing the entire export.
        args.extend(["-allow_sw".into(), "1".into()]);
    }
    if settings.apple_compatible
        && let Some(tag) = apple_codec_tag(encoder)
    {
        args.extend(["-tag:v".into(), tag.into()]);
    }
    if let Some(v) = metadata
        .color_space
        .as_deref()
        .or(settings.apple_compatible.then_some("bt709"))
    {
        args.extend(["-colorspace".into(), v.to_owned()]);
    }
    if let Some(v) = metadata
        .color_transfer
        .as_deref()
        .or(settings.apple_compatible.then_some("bt709"))
    {
        args.extend(["-color_trc".into(), v.to_owned()]);
    }
    if let Some(v) = metadata
        .color_primaries
        .as_deref()
        .or(settings.apple_compatible.then_some("bt709"))
    {
        args.extend(["-color_primaries".into(), v.to_owned()]);
    }
    if let Some(v) = &metadata.color_range {
        args.extend(["-color_range".into(), v.clone()]);
    }
    args.extend([
        "-fps_mode".into(),
        "passthrough".into(),
        "-progress".into(),
        "pipe:2".into(),
        "-nostats".into(),
        output.as_ref().to_string_lossy().into_owned(),
    ]);
    if settings.fast_start {
        // Insert this immediately before the output path. FFmpeg accepts the
        // option in either position, but keeping output options together makes
        // the generated command easier to inspect and test.
        let output_path = args.pop().expect("output path");
        args.extend(["-movflags".into(), "+faststart".into(), output_path]);
    }
    ExportCommand {
        program: PathBuf::from("ffmpeg"),
        args,
    }
}

fn apple_codec_tag(encoder: &str) -> Option<&'static str> {
    let encoder = encoder.to_ascii_lowercase();
    if encoder.contains("hevc") || encoder.contains("265") {
        Some("hvc1")
    } else if encoder.contains("h264") || encoder.contains("264") {
        Some("avc1")
    } else {
        None
    }
}

fn validate_settings(settings: &ExportSettings) -> Result<(), MediaError> {
    if settings.bitrate == Some(0) {
        return Err(MediaError::Invalid(
            "video bitrate must be greater than zero".into(),
        ));
    }
    if settings.quality.is_some_and(|quality| quality > 51) {
        return Err(MediaError::Invalid(
            "video quality (CRF) must be between 0 and 51".into(),
        ));
    }
    if settings
        .pixel_format
        .as_deref()
        .is_some_and(|format| format.trim().is_empty())
    {
        return Err(MediaError::Invalid("pixel format cannot be empty".into()));
    }
    Ok(())
}

fn command_for(
    tools: &FfmpegTools,
    video: &Path,
    output: &Path,
    settings: &ExportSettings,
    geometry: WidgetGeometry,
    metadata: &VideoMetadata,
    encoder: &str,
) -> ExportCommand {
    let mut c = build_export_command(video, output, settings, geometry, metadata, encoder);
    c.program = tools.ffmpeg.clone();
    c
}

fn choose_encoder(
    settings: &ExportSettings,
    metadata: &VideoMetadata,
    capabilities: &EncoderCapabilities,
) -> Result<String, MediaError> {
    if let Some(name) = &settings.encoder {
        if capabilities.contains(name) {
            return Ok(name.clone());
        }
        return Err(MediaError::Invalid(format!(
            "requested encoder is unavailable: {name}"
        )));
    }
    let codec = settings
        .codec
        .or_else(|| {
            metadata.codec.as_deref().and_then(|c| {
                if c.eq_ignore_ascii_case("h264") || c.eq_ignore_ascii_case("avc1") {
                    Some(Encoder::H264)
                } else if c.eq_ignore_ascii_case("hevc") || c.eq_ignore_ascii_case("h265") {
                    Some(Encoder::H265)
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            MediaError::Invalid(
                "source video is not H.264 or H.265; select an output codec explicitly".into(),
            )
        })?;
    // CRF is only supported by the software x264/x265 encoders. Selecting one
    // automatically keeps the quality control meaningful while preserving the
    // existing hardware-first automatic choice for bitrate exports.
    if settings.quality.is_some()
        && settings.encoder.is_none()
        && let Some(encoder) = capabilities.select_software(codec)
    {
        return Ok(encoder);
    }
    capabilities
        .select(codec)
        .ok_or_else(|| MediaError::Invalid(format!("no encoder available for {:?}", codec)))
}

#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[allow(clippy::too_many_arguments)]
pub fn export_video<P: FrameProvider>(
    tools: &FfmpegTools,
    video: impl AsRef<Path>,
    output: impl AsRef<Path>,
    settings: &ExportSettings,
    geometry: WidgetGeometry,
    mut provider: P,
    cancel: &CancelToken,
    mut progress: impl FnMut(ExportProgress),
) -> Result<(), MediaError> {
    validate_settings(settings)?;
    let video = video.as_ref();
    let output = output.as_ref();
    if geometry.width == 0 || geometry.height == 0 {
        return Err(MediaError::Invalid("widget geometry has zero size".into()));
    }
    let metadata = probe_video(tools, video)?;
    let encoders = tools.encoders()?;
    let encoder = choose_encoder(settings, &metadata, &encoders)?;
    export_inner(
        tools,
        video,
        output,
        settings,
        geometry,
        &metadata,
        &encoder,
        &mut provider,
        cancel,
        &mut progress,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn export_video_with_channel(
    tools: &FfmpegTools,
    video: impl AsRef<Path>,
    output: impl AsRef<Path>,
    settings: &ExportSettings,
    geometry: WidgetGeometry,
    frames: Receiver<RgbaFrame>,
    cancel: &CancelToken,
    progress: impl FnMut(ExportProgress),
) -> Result<(), MediaError> {
    let provider = CancellableChannelProvider {
        frames,
        cancel: cancel.clone(),
    };
    export_video(
        tools, video, output, settings, geometry, provider, cancel, progress,
    )
}

#[allow(clippy::too_many_arguments)]
fn export_inner<P: FrameProvider>(
    tools: &FfmpegTools,
    video: &Path,
    output: &Path,
    settings: &ExportSettings,
    geometry: WidgetGeometry,
    metadata: &VideoMetadata,
    encoder: &str,
    provider: &mut P,
    cancel: &CancelToken,
    progress: &mut impl FnMut(ExportProgress),
) -> Result<(), MediaError> {
    if output.exists() && !settings.overwrite {
        return Err(MediaError::Invalid(format!(
            "output already exists: {}",
            output.display()
        )));
    }
    let part = part_path(output);
    if part.exists() {
        fs::remove_file(&part)?;
    }
    let command = command_for(tools, video, &part, settings, geometry, metadata, encoder);
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stderr = child.stderr.take().expect("piped stderr");
    let (log_tx, log_rx) = crossbeam_channel::unbounded::<String>();
    thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        for line in reader.lines().map_while(|line| line.ok()) {
            let _ = log_tx.send(line);
        }
    });
    let mut stdin = child.stdin.take().expect("piped stdin");
    let result = (|| -> Result<(), MediaError> {
        let mut log = String::new();
        let mut read_progress = || {
            while let Ok(line) = log_rx.try_recv() {
                if let Some(seconds) = parse_ffmpeg_time(&line) {
                    progress(ExportProgress {
                        encoded_seconds: seconds,
                        duration_seconds: metadata.duration,
                    });
                }
                log.push_str(&line);
                log.push('\n');
            }
        };
        loop {
            if cancel.is_cancelled() {
                return Err(MediaError::Cancelled);
            }
            read_progress();
            match provider.next_frame()? {
                Some(frame) => {
                    if frame.width != geometry.width
                        || frame.height != geometry.height
                        || frame.rgba.len()
                            != geometry.width as usize * geometry.height as usize * 4
                    {
                        return Err(MediaError::Invalid(
                            "frame dimensions do not match widget geometry".into(),
                        ));
                    }
                    stdin.write_all(&frame.rgba).map_err(|e| {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            MediaError::Process("FFmpeg closed the frame pipe".into())
                        } else {
                            MediaError::Io(e)
                        }
                    })?;
                }
                None => break,
            }
        }
        drop(stdin);
        let status = child.wait()?;
        while let Ok(line) = log_rx.recv_timeout(Duration::from_millis(100)) {
            if let Some(seconds) = parse_ffmpeg_time(&line) {
                progress(ExportProgress {
                    encoded_seconds: seconds,
                    duration_seconds: metadata.duration,
                });
            }
            log.push_str(&line);
            log.push('\n');
        }
        if !status.success() {
            return Err(MediaError::Process(log));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_file(&part);
        return result;
    }
    // Unix replaces a destination atomically. Windows does not allow rename
    // over an existing file, so remove only the explicitly requested target.
    #[cfg(windows)]
    if settings.overwrite && output.exists() {
        fs::remove_file(output)?;
    }
    if let Err(e) = fs::rename(&part, output) {
        let _ = fs::remove_file(&part);
        return Err(MediaError::Io(e));
    }
    Ok(())
}

fn parse_ffmpeg_time(line: &str) -> Option<f64> {
    if let Some(value) = line.strip_prefix("out_time_us=") {
        return value.trim().parse::<f64>().ok().map(|v| v / 1_000_000.0);
    }
    if let Some(value) = line.strip_prefix("out_time_ms=") {
        return value.trim().parse::<f64>().ok().map(|v| v / 1_000_000.0);
    }
    let pos = line.find("time=")?;
    let value = line[pos + 5..].split_whitespace().next()?;
    let mut p = value.split(':');
    Some(
        p.next()?.parse::<f64>().ok()? * 3600.0
            + p.next()?.parse::<f64>().ok()? * 60.0
            + p.next()?.parse::<f64>().ok()?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rational;
    #[test]
    fn command_contains_overlay_and_audio_copy() {
        let m = VideoMetadata {
            width: 1920,
            height: 1080,
            frame_rate: Some(Rational::new(30000, 1001)),
            codec: Some("h264".into()),
            bit_rate: Some(8_000_000),
            ..Default::default()
        };
        let c = build_export_command(
            "in.mp4",
            "out.mp4",
            &Default::default(),
            WidgetGeometry::new(12, 34, 100, 50),
            &m,
            "libx264",
        );
        assert!(c.args.windows(2).any(|x| x == ["-c:a", "copy"]));
        assert!(c.args.iter().any(|x| x.contains("overlay=12:34")));
        assert!(c.args.iter().any(|x| x == "30000/1001"));
    }

    #[test]
    fn default_hevc_command_is_apple_compatible() {
        let metadata = VideoMetadata {
            width: 3840,
            height: 2160,
            frame_rate: Some(Rational::new(30000, 1001)),
            codec: Some("hevc".into()),
            color_range: Some("pc".into()),
            ..Default::default()
        };
        let command = build_export_command(
            "input.mp4",
            "output.mp4",
            &ExportSettings::default(),
            WidgetGeometry::new(8, 12, 320, 180),
            &metadata,
            "hevc_videotoolbox",
        );
        assert!(command.args.windows(2).any(|w| w == ["-tag:v", "hvc1"]));
        assert!(command.args.windows(2).any(|w| w == ["-allow_sw", "1"]));
        assert!(
            command
                .args
                .windows(2)
                .any(|w| w == ["-pix_fmt", "yuv420p"])
        );
        assert!(
            command
                .args
                .iter()
                .any(|arg| arg.contains("format=yuv420p"))
        );
        assert!(
            command
                .args
                .windows(2)
                .any(|w| w == ["-movflags", "+faststart"])
        );
        assert!(
            command
                .args
                .windows(2)
                .any(|w| w == ["-colorspace", "bt709"])
        );
    }

    #[test]
    fn quality_and_preset_select_crf_without_source_bitrate() {
        let metadata = VideoMetadata {
            codec: Some("h264".into()),
            bit_rate: Some(50_000_000),
            ..Default::default()
        };
        let settings = ExportSettings {
            bitrate: Some(8_000_000),
            quality: Some(20),
            preset: Some("slow".into()),
            apple_compatible: false,
            fast_start: false,
            ..Default::default()
        };
        let command = build_export_command(
            "input.mp4",
            "output.mp4",
            &settings,
            WidgetGeometry::new(0, 0, 32, 24),
            &metadata,
            "libx264",
        );
        assert!(command.args.windows(2).any(|w| w == ["-crf", "20"]));
        assert!(command.args.windows(2).any(|w| w == ["-preset", "slow"]));
        assert!(!command.args.iter().any(|arg| arg == "-b:v"));
        assert!(!command.args.iter().any(|arg| arg == "-tag:v"));
        assert!(!command.args.iter().any(|arg| arg == "-movflags"));
    }

    #[test]
    fn invalid_export_settings_are_rejected_before_ffmpeg() {
        let err = validate_settings(&ExportSettings {
            quality: Some(52),
            ..Default::default()
        })
        .unwrap_err();
        assert!(err.to_string().contains("between 0 and 51"));
    }

    #[test]
    fn partial_file_preserves_the_container_extension() {
        assert_eq!(
            part_path(Path::new("movie.mp4")),
            PathBuf::from("movie.part.mp4")
        );
    }

    #[test]
    #[ignore = "runs a real FFmpeg encode"]
    fn real_export_burns_overlay_and_preserves_audio() {
        use crate::{Encoder, FfmpegConfig, discover};
        use tempfile::tempdir;

        let tools = discover(&FfmpegConfig::default()).unwrap();
        let dir = tempdir().unwrap();
        let input = dir.path().join("input.mp4");
        let output = dir.path().join("output.mp4");
        let status = Command::new(&tools.ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=320x180:r=30:d=1",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1",
                "-c:v",
                "mpeg4",
                "-c:a",
                "aac",
                "-shortest",
            ])
            .arg(&input)
            .status()
            .unwrap();
        assert!(status.success());

        let pixels = vec![255u8; 32 * 24 * 4];
        let mut n = 0;
        let provider = move || {
            if n == 30 {
                Ok(None)
            } else {
                n += 1;
                RgbaFrame::new(32, 24, pixels.clone()).map(Some)
            }
        };
        let settings = ExportSettings {
            encoder: tools
                .encoders()
                .unwrap()
                .contains("libx264")
                .then(|| "libx264".into()),
            codec: Some(Encoder::H264),
            ..Default::default()
        };
        export_video(
            &tools,
            &input,
            &output,
            &settings,
            WidgetGeometry::new(20, 30, 32, 24),
            provider,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert!(!dir.path().join("output.part.mp4").exists());
        let metadata = probe_video(&tools, &output).unwrap();
        assert_eq!(metadata.resolution(), (320, 180));
        assert!(metadata.has_audio);

        let hevc_output = dir.path().join("output-hevc.mp4");
        let pixels = vec![255u8; 32 * 24 * 4];
        let mut n = 0;
        let provider = move || {
            if n == 30 {
                Ok(None)
            } else {
                n += 1;
                RgbaFrame::new(32, 24, pixels.clone()).map(Some)
            }
        };
        export_video(
            &tools,
            &input,
            &hevc_output,
            &ExportSettings {
                encoder: tools
                    .encoders()
                    .unwrap()
                    .contains("libx265")
                    .then(|| "libx265".into()),
                codec: Some(Encoder::H265),
                ..Default::default()
            },
            WidgetGeometry::new(20, 30, 32, 24),
            provider,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let stream = Command::new(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,codec_tag_string,pix_fmt,color_space",
                "-of",
                "default=nw=1",
            ])
            .arg(&hevc_output)
            .output()
            .unwrap();
        assert!(stream.status.success());
        let stream = String::from_utf8_lossy(&stream.stdout);
        assert!(stream.contains("codec_name=hevc"), "{stream}");
        assert!(stream.contains("codec_tag_string=hvc1"), "{stream}");
        assert!(stream.contains("pix_fmt=yuv420p"), "{stream}");
        assert!(stream.contains("color_space=bt709"), "{stream}");
        assert!(probe_video(&tools, &hevc_output).unwrap().has_audio);
        let decoded = Command::new(&tools.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&hevc_output)
            .args(["-frames:v", "3", "-f", "null", "-"])
            .status()
            .unwrap();
        assert!(decoded.success());
    }
}
