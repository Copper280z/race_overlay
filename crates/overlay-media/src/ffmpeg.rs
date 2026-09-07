use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use crate::MediaError;

#[derive(Debug, Clone, Default)]
pub struct FfmpegConfig {
    pub ffmpeg_path: Option<PathBuf>,
    pub ffprobe_path: Option<PathBuf>,
}

impl FfmpegConfig {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_ffmpeg(path: impl Into<PathBuf>) -> Self {
        Self {
            ffmpeg_path: Some(path.into()),
            ..Self::default()
        }
    }
    pub fn with_paths(ffmpeg: impl Into<PathBuf>, ffprobe: impl Into<PathBuf>) -> Self {
        Self {
            ffmpeg_path: Some(ffmpeg.into()),
            ffprobe_path: Some(ffprobe.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FfmpegTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

pub type Ffmpeg = FfmpegTools;

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("could not find {0} on PATH")]
    NotFound(String),
    #[error("configured {0} is not executable: {1}")]
    InvalidConfigured(String, PathBuf),
    #[error("could not start {0}: {1}")]
    Start(String, std::io::Error),
}

fn find_program(name: &str) -> Option<PathBuf> {
    if let Some(path) = env::var_os("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
            #[cfg(windows)]
            for ext in [".exe", ".cmd", ".bat"] {
                let candidate = dir.join(format!("{name}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Finder-launched application bundles receive a minimal PATH which
        // normally excludes both Apple Silicon and Intel Homebrew prefixes.
        for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
            let candidate = Path::new(dir).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        // Also support a future self-contained distribution which places the
        // tools in Race Overlay.app/Contents/Resources.
        if let Ok(executable) = env::current_exe()
            && let Some(contents) = executable.parent().and_then(Path::parent)
        {
            let candidate = contents.join("Resources").join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn validate_configured(
    name: &str,
    path: Option<PathBuf>,
) -> std::result::Result<PathBuf, ToolError> {
    match path {
        Some(p) if p.is_file() => Ok(p),
        Some(p) => Err(ToolError::InvalidConfigured(name.into(), p)),
        None => find_program(name).ok_or_else(|| ToolError::NotFound(name.into())),
    }
}

pub fn discover(config: &FfmpegConfig) -> std::result::Result<FfmpegTools, ToolError> {
    let ffmpeg = validate_configured("ffmpeg", config.ffmpeg_path.clone())?;
    let ffprobe = validate_configured("ffprobe", config.ffprobe_path.clone())?;
    Ok(FfmpegTools { ffmpeg, ffprobe })
}

pub fn discover_ffmpeg(
    config: Option<&FfmpegConfig>,
) -> std::result::Result<FfmpegTools, ToolError> {
    let owned = config.cloned().unwrap_or_default();
    discover(&owned)
}

impl FfmpegTools {
    pub fn discover(config: FfmpegConfig) -> std::result::Result<Self, ToolError> {
        discover(&config)
    }
    pub fn ffmpeg(&self) -> &Path {
        &self.ffmpeg
    }
    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }

    pub fn encoders(&self) -> Result<EncoderCapabilities, MediaError> {
        let out = Command::new(&self.ffmpeg)
            .args(["-hide_banner", "-encoders"])
            .output()
            .map_err(|e| ToolError::Start("ffmpeg".into(), e))?;
        if !out.status.success() {
            return Err(MediaError::Process(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(EncoderCapabilities::from_ffmpeg_output(&text))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoder {
    H264,
    H265,
}

#[derive(Debug, Clone, Default)]
pub struct EncoderCapabilities {
    pub names: Vec<String>,
}

impl EncoderCapabilities {
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            names: names.into_iter().map(Into::into).collect(),
        }
    }
    pub fn from_ffmpeg_output(text: &str) -> Self {
        let names = text
            .lines()
            .filter_map(|line| {
                let line = line.trim_start();
                if line.starts_with("Encoders:") || line.is_empty() || line.starts_with('-') {
                    return None;
                }
                // ffmpeg's encoder rows have flags followed by the codec name.
                let mut words = line.split_whitespace();
                let flags = words.next()?;
                if flags.len() < 4 || !flags.chars().all(|c| c.is_ascii_alphabetic() || c == '.') {
                    return None;
                }
                let name = words.next()?;
                if name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    Some(name.to_owned())
                } else {
                    None
                }
            })
            .collect();
        Self { names }
    }
    pub fn contains(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }
    pub fn select_encoder(&self, codec: Encoder) -> Option<String> {
        self.select(codec)
    }
    /// Select a software encoder suitable for CRF/preset controls.
    pub fn select_software(&self, codec: Encoder) -> Option<String> {
        let name = match codec {
            Encoder::H264 => "libx264",
            Encoder::H265 => "libx265",
        };
        self.contains(name).then(|| name.to_owned())
    }
    pub fn select(&self, codec: Encoder) -> Option<String> {
        let candidates: &[&str] = match codec {
            Encoder::H264 => &[
                "h264_videotoolbox",
                "h264_nvenc",
                "h264_qsv",
                "h264_vaapi",
                "h264_amf",
                "libx264",
            ],
            Encoder::H265 => &[
                "hevc_videotoolbox",
                "hevc_nvenc",
                "hevc_qsv",
                "hevc_vaapi",
                "hevc_amf",
                "libx265",
            ],
        };
        candidates
            .iter()
            .find(|n| self.contains(n))
            .map(|n| (*n).to_owned())
    }
}

impl From<Vec<String>> for EncoderCapabilities {
    fn from(names: Vec<String>) -> Self {
        Self { names }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Rational {
    pub numerator: i64,
    pub denominator: i64,
}

pub fn parse_rational(value: &str) -> Option<Rational> {
    Rational::parse(value)
}

impl Rational {
    pub const fn new(numerator: i64, denominator: i64) -> Self {
        Self {
            numerator,
            denominator,
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("n/a") || value == "0/0" {
            return None;
        }
        let (n, d) = value
            .split_once('/')
            .map(|(n, d)| (n.trim(), d.trim()))
            .unwrap_or((value, "1"));
        let numerator = n.parse().ok()?;
        let denominator = d.parse().ok()?;
        (denominator != 0).then_some(Self {
            numerator,
            denominator,
        })
    }
    pub fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }
}

impl std::fmt::Display for Rational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

impl std::str::FromStr for Rational {
    type Err = &'static str;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or("invalid rational")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rational_parsing() {
        assert_eq!(
            Rational::parse("30000/1001"),
            Some(Rational::new(30000, 1001))
        );
        assert_eq!(Rational::parse("24"), Some(Rational::new(24, 1)));
        assert_eq!(Rational::parse("N/A"), None);
    }
    #[test]
    fn encoder_preference() {
        let c = EncoderCapabilities::new(["libx264", "h264_nvenc", "libx265"]);
        assert_eq!(c.select(Encoder::H264).as_deref(), Some("h264_nvenc"));
        assert_eq!(c.select(Encoder::H265).as_deref(), Some("libx265"));
    }
}
