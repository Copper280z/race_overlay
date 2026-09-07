use crate::{FfmpegTools, MediaError};
use std::{path::Path, process::Command};

/// Decode the first audio stream as little-endian mono 32-bit PCM.
pub fn extract_mono_pcm(
    tools: &FfmpegTools,
    path: impl AsRef<Path>,
    sample_rate: u32,
) -> Result<Vec<f32>, MediaError> {
    extract_mono_pcm_with_tools(tools, path, sample_rate)
}

pub fn extract_mono_pcm_with_tools(
    tools: &FfmpegTools,
    path: impl AsRef<Path>,
    sample_rate: u32,
) -> Result<Vec<f32>, MediaError> {
    if sample_rate == 0 {
        return Err(MediaError::Invalid(
            "audio sample rate must be non-zero".into(),
        ));
    }
    let out = Command::new(&tools.ffmpeg)
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(path.as_ref())
        .args(["-vn", "-ac", "1", "-ar"])
        .arg(sample_rate.to_string())
        .args(["-f", "f32le", "pipe:1"])
        .output()?;
    if !out.status.success() {
        let message = String::from_utf8_lossy(&out.stderr);
        let lower = message.to_ascii_lowercase();
        if lower.contains("does not contain any stream")
            || lower.contains("matches no streams")
            || (lower.contains("audio") && lower.contains("not found"))
        {
            return Ok(Vec::new());
        }
        return Err(MediaError::Process(message.into_owned()));
    }
    let mut pcm = Vec::with_capacity(out.stdout.len() / 4);
    for bytes in out.stdout.as_chunks::<4>().0 {
        pcm.push(f32::from_le_bytes(*bytes));
    }
    Ok(pcm)
}
