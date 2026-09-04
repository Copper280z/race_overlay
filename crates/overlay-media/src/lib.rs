//! Cross-platform media services used by Race Overlay.
//!
//! The crate intentionally talks to the command line FFmpeg tools rather than
//! linking to a platform-specific FFmpeg build.  This keeps the desktop app
//! portable and also makes a missing installation an ordinary, reportable
//! error.

mod audio;
mod export;
mod ffmpeg;
mod preview;
mod probe;
mod sync;

pub use audio::{extract_mono_pcm, extract_mono_pcm_with_tools};
pub use export::{
    CancelToken, ExportCommand, ExportError, ExportProgress, ExportSettings, FrameProvider,
    RgbaFrame, WidgetGeometry, build_export_command, export_video, export_video_with_channel,
};
pub use ffmpeg::{
    Encoder, EncoderCapabilities, Ffmpeg, FfmpegConfig, FfmpegTools, ToolError, discover,
    discover_ffmpeg, parse_rational,
};
pub use preview::{
    PreviewFrame, PreviewHandle, PreviewPlayback, PreviewRequest, PreviewSize, PreviewWorker,
};
pub use probe::{AudioMetadata, VideoMetadata, probe, probe_video};
pub use sync::{AlignmentResult, align_audio};

pub use ffmpeg::Rational;

/// Result type used by the media crate's fallible operations.
pub type Result<T> = std::result::Result<T, MediaError>;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("FFmpeg tools are unavailable: {0}")]
    Tools(#[from] ToolError),
    #[error("FFmpeg operation failed: {0}")]
    Process(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid media data: {0}")]
    Invalid(String),
    #[error("operation cancelled")]
    Cancelled,
}
