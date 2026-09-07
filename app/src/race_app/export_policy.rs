//! Export geometry and display policies.
use overlay_core::{NormalizedRect, WidgetConfig};
use overlay_media::WidgetGeometry;
use overlay_render::RenderSize;

pub(super) fn format_time(seconds: f64) -> String {
    let seconds = seconds.max(0.0);
    format!("{:02}:{:05.2}", (seconds / 60.0) as u64, seconds % 60.0)
}

pub(super) fn codec_display_name(codec: &str) -> &str {
    if codec.eq_ignore_ascii_case("hevc") || codec.eq_ignore_ascii_case("h265") {
        "H.265 / HEVC"
    } else if codec.eq_ignore_ascii_case("h264") || codec.eq_ignore_ascii_case("avc1") {
        "H.264 / AVC"
    } else {
        codec
    }
}

/// Convert normalized project widgets to a small transparent surface for
/// FFmpeg. Rendering only their union avoids allocating a 4K RGBA image for
/// every output frame.
pub(super) fn export_surface(
    widgets: &[WidgetConfig],
    full: RenderSize,
) -> Option<(WidgetGeometry, Vec<WidgetConfig>)> {
    if widgets.is_empty() || full.width == 0 || full.height == 0 {
        return None;
    }
    let left = widgets
        .iter()
        .map(|w| (w.rect.x * full.width as f32).floor() as i32)
        .min()?;
    let top = widgets
        .iter()
        .map(|w| (w.rect.y * full.height as f32).floor() as i32)
        .min()?;
    let right = widgets
        .iter()
        .map(|w| ((w.rect.x + w.rect.width) * full.width as f32).ceil() as i32)
        .max()?;
    let bottom = widgets
        .iter()
        .map(|w| ((w.rect.y + w.rect.height) * full.height as f32).ceil() as i32)
        .max()?;
    let width = (right - left).max(1) as u32;
    let height = (bottom - top).max(1) as u32;
    let mut remapped = widgets.to_vec();
    for widget in &mut remapped {
        let x = widget.rect.x * full.width as f32 - left as f32;
        let y = widget.rect.y * full.height as f32 - top as f32;
        widget.rect = NormalizedRect::new(
            x / width as f32,
            y / height as f32,
            widget.rect.width * full.width as f32 / width as f32,
            widget.rect.height * full.height as f32 / height as f32,
        );
    }
    Some((WidgetGeometry::new(left, top, width, height), remapped))
}
