use crate::{FfmpegTools, MediaError, Rational};
use serde::Deserialize;
use std::{path::Path, process::Command};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioMetadata {
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bit_rate: Option<u64>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoMetadata {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Option<Rational>,
    pub duration: Option<f64>,
    pub codec: Option<String>,
    pub bit_rate: Option<u64>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub color_primaries: Option<String>,
    pub color_range: Option<String>,
    pub has_audio: bool,
    pub audio: Option<AudioMetadata>,
}

impl VideoMetadata {
    pub fn fps(&self) -> Option<f64> {
        self.frame_rate.map(Rational::as_f64)
    }
    pub fn resolution(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

#[derive(Deserialize, Default)]
struct FfprobeJson {
    streams: Vec<Stream>,
    format: Option<Format>,
}
#[derive(Deserialize, Default)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u16>,
}
#[derive(Deserialize, Default)]
struct Format {
    duration: Option<String>,
    bit_rate: Option<String>,
}

fn number<T: std::str::FromStr>(s: Option<&String>) -> Option<T> {
    s.and_then(|v| v.parse().ok())
}
fn duration(stream: &Stream, format: Option<&Format>) -> Option<f64> {
    number(stream.duration.as_ref()).or_else(|| format.and_then(|f| number(f.duration.as_ref())))
}

pub fn probe(tools: &FfmpegTools, path: impl AsRef<Path>) -> Result<VideoMetadata, MediaError> {
    probe_video(tools, path)
}

pub fn probe_video(
    tools: &FfmpegTools,
    path: impl AsRef<Path>,
) -> Result<VideoMetadata, MediaError> {
    let output = Command::new(&tools.ffprobe)
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path.as_ref())
        .output()?;
    if !output.status.success() {
        return Err(MediaError::Process(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let parsed: FfprobeJson = serde_json::from_slice(&output.stdout)
        .map_err(|e| MediaError::Invalid(format!("ffprobe JSON: {e}")))?;
    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| MediaError::Invalid("media has no video stream".into()))?;
    let audio = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"))
        .map(|s| AudioMetadata {
            codec: s.codec_name.clone(),
            sample_rate: number(s.sample_rate.as_ref()),
            channels: s.channels,
            bit_rate: number(s.bit_rate.as_ref()),
            duration: duration(s, parsed.format.as_ref()),
        });
    let bit_rate = number(video.bit_rate.as_ref()).or_else(|| {
        parsed
            .format
            .as_ref()
            .and_then(|f| number(f.bit_rate.as_ref()))
    });
    let frame_rate = video
        .avg_frame_rate
        .as_deref()
        .and_then(Rational::parse)
        .or_else(|| video.r_frame_rate.as_deref().and_then(Rational::parse));
    Ok(VideoMetadata {
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        frame_rate,
        duration: duration(video, parsed.format.as_ref()),
        codec: video.codec_name.clone(),
        bit_rate,
        color_space: video.color_space.clone(),
        color_transfer: video.color_transfer.clone(),
        color_primaries: video.color_primaries.clone(),
        color_range: video.color_range.clone(),
        has_audio: audio.is_some(),
        audio,
    })
}
