//! The data that defines a color scheme. Everything visual in the app derives
//! from these fields, so a new scheme is one `Palette` value.

use eframe::egui::Color32;

/// Every color the interface uses. Pick values by role, not by hue:
///
/// * surfaces go from `canvas` (behind plots and video) up through `panel`,
///   `bar`, and `raised` (cards);
/// * `control` is the fill of buttons and fields and must stay distinguishable
///   from every surface it can sit on;
/// * `accent` marks the interactive focus and `accent_dim` its quiet
///   background; `on_accent` is text drawn on a solid `accent` fill.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// True for schemes with light text on dark surfaces.
    pub dark: bool,
    pub canvas: Color32,
    pub panel: Color32,
    pub bar: Color32,
    pub raised: Color32,
    pub hover: Color32,
    pub border: Color32,
    pub control: Color32,
    pub control_border: Color32,
    /// Slider rails and checkbox boxes.
    pub control_rail: Color32,
    pub text_strong: Color32,
    pub text_normal: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub on_accent: Color32,
    pub good: Color32,
    pub warn: Color32,
    pub bad: Color32,
    /// Ordered run/trace colors, visible against this scheme's plot surfaces.
    pub traces: [Color32; 8],
}

pub(super) const DARK_TRACES: [Color32; 8] = [
    Color32::from_rgb(65, 170, 255),
    Color32::from_rgb(255, 156, 66),
    Color32::from_rgb(70, 210, 145),
    Color32::from_rgb(210, 110, 255),
    Color32::from_rgb(250, 215, 60),
    Color32::from_rgb(70, 215, 220),
    Color32::from_rgb(250, 105, 140),
    Color32::from_rgb(180, 190, 255),
];

/// A named palette, as listed in the theme picker.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Stable key stored in user preferences; never change it once shipped.
    pub id: &'static str,
    pub name: &'static str,
    pub palette: Palette,
}
