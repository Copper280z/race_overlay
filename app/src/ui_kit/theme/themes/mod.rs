//! Built-in color schemes.
//!
//! To add one: copy `race_dark.rs` to a new file, change the values and the
//! `THEME` id/name, declare the module below, and add it to [`ALL`]. It then
//! appears in the ⚙ menu with no other code changes.

use super::palette::Theme;

mod daylight;
mod graphite;
mod race_dark;

/// Every selectable scheme; the first is the default.
pub const ALL: &[Theme] = &[race_dark::THEME, graphite::THEME, daylight::THEME];

/// Palette active before a preference is loaded.
pub(super) const DEFAULT_PALETTE: super::palette::Palette = ALL[0].palette;
