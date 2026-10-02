//! Neutral charcoal surfaces with an amber accent; easy on the eyes in a
//! dim trailer or garage.

use super::super::palette::{Palette, Theme};
use eframe::egui::Color32;

pub const THEME: Theme = Theme {
    id: "graphite",
    name: "Graphite",
    palette: Palette {
        dark: true,
        canvas: Color32::from_rgb(16, 16, 17),
        panel: Color32::from_rgb(25, 25, 27),
        bar: Color32::from_rgb(32, 32, 34),
        raised: Color32::from_rgb(33, 33, 36),
        hover: Color32::from_rgb(60, 60, 64),
        border: Color32::from_rgb(52, 52, 56),
        control: Color32::from_rgb(46, 46, 50),
        control_border: Color32::from_rgb(68, 68, 74),
        control_rail: Color32::from_rgb(70, 70, 76),
        text_strong: Color32::from_rgb(245, 245, 246),
        text_normal: Color32::from_rgb(213, 213, 216),
        text_weak: Color32::from_rgb(141, 141, 148),
        accent: Color32::from_rgb(255, 176, 56),
        accent_dim: Color32::from_rgb(96, 66, 20),
        on_accent: Color32::from_rgb(28, 18, 4),
        good: Color32::from_rgb(110, 210, 140),
        warn: Color32::from_rgb(255, 200, 90),
        bad: Color32::from_rgb(255, 122, 118),
        traces: super::super::palette::DARK_TRACES,
    },
};
