//! The default: deep blue-gray surfaces with a blue accent.

use super::super::palette::{Palette, Theme};
use eframe::egui::Color32;

pub const THEME: Theme = Theme {
    id: "race-dark",
    name: "Race Dark",
    palette: Palette {
        dark: true,
        canvas: Color32::from_rgb(14, 16, 20),
        panel: Color32::from_rgb(21, 24, 30),
        bar: Color32::from_rgb(26, 30, 37),
        raised: Color32::from_rgb(27, 31, 39),
        hover: Color32::from_rgb(52, 60, 75),
        border: Color32::from_rgb(45, 51, 62),
        control: Color32::from_rgb(40, 46, 58),
        control_border: Color32::from_rgb(58, 66, 81),
        control_rail: Color32::from_rgb(58, 66, 80),
        text_strong: Color32::from_rgb(240, 243, 248),
        text_normal: Color32::from_rgb(208, 214, 224),
        text_weak: Color32::from_rgb(133, 142, 158),
        accent: Color32::from_rgb(92, 156, 255),
        accent_dim: Color32::from_rgb(38, 70, 122),
        on_accent: Color32::from_rgb(8, 16, 30),
        good: Color32::from_rgb(96, 208, 140),
        warn: Color32::from_rgb(240, 190, 80),
        bad: Color32::from_rgb(255, 121, 121),
        traces: super::super::palette::DARK_TRACES,
    },
};
