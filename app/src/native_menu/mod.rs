//! Operating-system menu bar.
//!
//! macOS shows an application menu bar at the top of the screen and users
//! expect File/View/Window/Help there, so it is built natively with `muda`.
//! Windows and Linux keep the in-window header as their only menu; there this
//! module is an inert stand-in with the same interface, so the application
//! shell has no platform conditionals.

use crate::analysis_app::PanelChoice;
use overlay_core::UnitSystem;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::NativeMenu;

#[cfg(not(target_os = "macos"))]
mod inert;
#[cfg(not(target_os = "macos"))]
pub use inert::NativeMenu;

/// Something the user chose from the menu bar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuCommand {
    AddFiles,
    OpenWorkspace,
    OpenVideo,
    OpenProject,
    Save,
    SaveAs,
    ShowAnalysis,
    ShowOverlay,
    SetTheme(&'static str),
    SetUnits(UnitSystem),
    AddPanel(PanelChoice),
    LayoutPreset(usize),
    OpenWebsite,
    OpenFfmpegLicensing,
}

/// What the menu needs to show check marks and enable/disable entries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuState {
    pub analysis_mode: bool,
    pub theme_id: &'static str,
    pub units: UnitSystem,
}
