//! A light scheme for bright rooms and printing screenshots, with deeper
//! trace colors that retain the run hues used by the dark schemes.

use super::super::palette::{Palette, Theme};
use eframe::egui::Color32;

pub const THEME: Theme = Theme {
    id: "daylight",
    name: "Daylight",
    palette: Palette {
        dark: false,
        canvas: Color32::from_rgb(236, 239, 244),
        panel: Color32::from_rgb(246, 247, 250),
        bar: Color32::from_rgb(232, 235, 241),
        raised: Color32::from_rgb(255, 255, 255),
        hover: Color32::from_rgb(214, 220, 232),
        border: Color32::from_rgb(208, 213, 224),
        control: Color32::from_rgb(255, 255, 255),
        control_border: Color32::from_rgb(190, 197, 212),
        control_rail: Color32::from_rgb(196, 203, 218),
        text_strong: Color32::from_rgb(16, 22, 34),
        text_normal: Color32::from_rgb(44, 52, 68),
        text_weak: Color32::from_rgb(104, 114, 132),
        accent: Color32::from_rgb(30, 100, 220),
        accent_dim: Color32::from_rgb(205, 222, 250),
        on_accent: Color32::from_rgb(255, 255, 255),
        good: Color32::from_rgb(20, 130, 70),
        warn: Color32::from_rgb(170, 100, 0),
        bad: Color32::from_rgb(196, 40, 40),
        traces: [
            Color32::from_rgb(25, 105, 190),
            Color32::from_rgb(180, 85, 15),
            Color32::from_rgb(20, 130, 80),
            Color32::from_rgb(145, 60, 195),
            Color32::from_rgb(145, 115, 0),
            Color32::from_rgb(0, 125, 140),
            Color32::from_rgb(195, 45, 90),
            Color32::from_rgb(95, 100, 185),
        ],
    },
};
