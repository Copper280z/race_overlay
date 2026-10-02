//! The application's visual identity: type scale, spacing, and the active
//! [`Palette`]. Panels read semantic colors from here (`surface::panel()`,
//! `text::weak()`, [`accent`], [`Tone`]) and never hard-code them, so
//! switching or adding a scheme only touches `themes/`.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme as EguiTheme,
    ThemePreference, Visuals, style::WidgetVisuals,
};
use egui_dock::Style as DockStyle;
use std::sync::RwLock;

mod palette;
mod themes;

pub use palette::{Palette, Theme};
pub use themes::ALL as THEMES;

static ACTIVE: RwLock<Palette> = RwLock::new(themes::DEFAULT_PALETTE);

/// Key under which the chosen scheme is saved in eframe storage.
pub const STORAGE_KEY: &str = "race-overlay.theme";

pub const RADIUS: u8 = 6;

/// The palette currently in effect.
pub fn palette() -> Palette {
    *ACTIVE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Looks up a built-in scheme by its stable id.
pub fn find(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|theme| theme.id == id)
}

/// The scheme whose palette is active, if it is a built-in one.
pub fn active() -> &'static Theme {
    let current = palette();
    THEMES
        .iter()
        .find(|theme| theme.palette == current)
        .unwrap_or(&THEMES[0])
}

/// Neutral surfaces, darkest (or lowest) to highest.
pub mod surface {
    use super::{Color32, palette};
    /// Behind everything, including plots and video letterboxing.
    pub fn canvas() -> Color32 {
        palette().canvas
    }
    /// Side panels, tab bodies.
    pub fn panel() -> Color32 {
        palette().panel
    }
    /// Headers and toolbars.
    pub fn bar() -> Color32 {
        palette().bar
    }
    /// Cards and grouped controls inside a panel.
    pub fn raised() -> Color32 {
        palette().raised
    }
    pub fn border() -> Color32 {
        palette().border
    }
    pub fn control_border() -> Color32 {
        palette().control_border
    }
}

pub mod text {
    use super::{Color32, palette};
    pub fn strong() -> Color32 {
        palette().text_strong
    }
    pub fn normal() -> Color32 {
        palette().text_normal
    }
    pub fn weak() -> Color32 {
        palette().text_weak
    }
}

/// Interactive accent color.
pub fn accent() -> Color32 {
    palette().accent
}

/// Quiet accent-tinted background (selection, active segment).
pub fn accent_dim() -> Color32 {
    palette().accent_dim
}

/// Text drawn on a solid [`accent`] fill.
pub fn on_accent() -> Color32 {
    palette().on_accent
}

/// Meaning carried by status text, chips, and callouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Info,
    Good,
    Warn,
    Bad,
}

impl Tone {
    pub fn color(self) -> Color32 {
        let p = palette();
        match self {
            Self::Neutral => p.text_weak,
            Self::Info => p.accent,
            Self::Good => p.good,
            Self::Warn => p.warn,
            Self::Bad => p.bad,
        }
    }

    /// Low-contrast tint used behind chips and callouts.
    pub fn fill(self) -> Color32 {
        let c = self.color();
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), 32)
    }
}

/// Makes `palette` the active scheme and installs matching egui styles.
/// Safe to call again at runtime to switch schemes.
pub fn apply(ctx: &egui::Context, palette: &Palette) {
    *ACTIVE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = *palette;
    // The palette, not the OS setting, decides light or dark, so both egui
    // themes carry the same visuals.
    ctx.set_theme(if palette.dark {
        ThemePreference::Dark
    } else {
        ThemePreference::Light
    });
    for theme in [EguiTheme::Dark, EguiTheme::Light] {
        ctx.set_visuals_of(theme, visuals(palette));
        ctx.style_mut_of(theme, |style| {
            style.text_styles = [
                (
                    TextStyle::Heading,
                    FontId::new(17.0, FontFamily::Proportional),
                ),
                (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
                (
                    TextStyle::Button,
                    FontId::new(13.5, FontFamily::Proportional),
                ),
                (
                    TextStyle::Small,
                    FontId::new(11.5, FontFamily::Proportional),
                ),
                (
                    TextStyle::Monospace,
                    FontId::new(12.5, FontFamily::Monospace),
                ),
            ]
            .into();
            let spacing = &mut style.spacing;
            spacing.item_spacing = egui::vec2(8.0, 6.0);
            spacing.button_padding = egui::vec2(10.0, 4.0);
            spacing.interact_size = egui::vec2(34.0, 26.0);
            spacing.window_margin = Margin::same(12);
            spacing.menu_margin = Margin::same(8);
            spacing.indent = 14.0;
            spacing.slider_width = 130.0;
            spacing.combo_width = 120.0;
            spacing.scroll.bar_width = 8.0;
            spacing.scroll.floating = true;
            style.interaction.tooltip_delay = 0.4;
        });
    }
}

fn widget(bg: Color32, stroke: Color32, fg: Color32) -> WidgetVisuals {
    WidgetVisuals {
        bg_fill: bg,
        weak_bg_fill: bg,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: CornerRadius::same(RADIUS),
        fg_stroke: Stroke::new(1.0, fg),
        expansion: 0.0,
    }
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.override_text_color = None;
    v.weak_text_color = Some(p.text_weak);
    v.panel_fill = p.panel;
    v.window_fill = p.bar;
    v.extreme_bg_color = p.canvas;
    v.faint_bg_color = p.bar;
    v.code_bg_color = p.canvas;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if p.dark { 110 } else { 50 }),
    };
    v.popup_shadow = v.window_shadow;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.bad;
    v.selection.bg_fill = p.accent_dim;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.widgets.noninteractive = widget(p.panel, p.border, p.text_normal);
    v.widgets.inactive = widget(p.control, p.control_border, p.text_normal);
    // Slider rails and checkbox boxes use `bg_fill`; buttons use `weak_bg_fill`.
    v.widgets.inactive.bg_fill = p.control_rail;
    v.widgets.hovered = widget(p.hover, p.control_border, p.text_strong);
    v.widgets.active = widget(p.accent_dim, p.accent, p.text_strong);
    v.widgets.open = widget(p.hover, p.border, p.text_strong);
    v
}

/// Dock chrome that matches the rest of the application: flat tabs and quiet
/// separators.
pub fn dock_style(base: &egui::Style) -> DockStyle {
    let p = palette();
    let mut dock = DockStyle::from_egui(base);
    dock.separator.width = 1.0;
    dock.separator.extra_interact_width = 6.0;
    dock.separator.color_idle = p.canvas;
    dock.separator.color_hovered = p.accent_dim;
    dock.separator.color_dragged = p.accent;
    dock.tab_bar.bg_fill = p.bar;
    dock.tab_bar.height = 30.0;
    dock.tab_bar.show_scroll_bar_on_overflow = false;
    dock.tab_bar.hline_color = p.border;
    dock.tab_bar.corner_radius = CornerRadius::ZERO;
    dock.tab.spacing = 2.0;
    dock.tab.hline_below_active_tab_name = false;
    dock.tab.minimum_width = Some(72.0);
    let tab_radius = CornerRadius {
        nw: 6,
        ne: 6,
        sw: 0,
        se: 0,
    };
    let tab = |bg: Color32, fg: Color32| egui_dock::TabInteractionStyle {
        outline_color: Color32::TRANSPARENT,
        corner_radius: tab_radius,
        bg_fill: bg,
        text_color: fg,
    };
    dock.tab.inactive = tab(p.bar, p.text_weak);
    dock.tab.hovered = tab(p.raised, p.text_strong);
    dock.tab.active = tab(p.panel, p.text_strong);
    dock.tab.focused = tab(p.panel, p.text_strong);
    dock.tab.inactive_with_kb_focus = dock.tab.inactive.clone();
    dock.tab.active_with_kb_focus = dock.tab.active.clone();
    dock.tab.focused_with_kb_focus = dock.tab.focused.clone();
    dock.tab.tab_body.bg_fill = p.panel;
    dock.tab.tab_body.stroke = Stroke::NONE;
    dock.tab.tab_body.inner_margin = Margin::same(8);
    dock.tab.tab_body.corner_radius = CornerRadius::ZERO;
    dock.overlay.selection_color = p.accent.gamma_multiply(0.35);
    dock.buttons.add_tab_bg_fill = p.bar;
    dock.buttons.close_all_tabs_bg_fill = p.bar;
    dock.buttons.collapse_tabs_bg_fill = p.bar;
    dock.buttons.minimize_window_bg_fill = p.bar;
    dock.buttons.close_tab_bg_fill = p.bar;
    dock
}

/// Standard frame for side panels and toolbars.
pub fn bar_frame(fill: Color32, margin: Margin) -> egui::Frame {
    egui::Frame::new().fill(fill).inner_margin(margin)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance.
    fn luminance(color: Color32) -> f64 {
        let channel = |value: u8| {
            let v = f64::from(value) / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn scheme_ids_are_unique_and_lookup_round_trips() {
        for (index, theme) in THEMES.iter().enumerate() {
            assert!(THEMES[index + 1..].iter().all(|other| other.id != theme.id));
            assert_eq!(find(theme.id).map(|found| found.name), Some(theme.name));
        }
        assert!(find("no-such-scheme").is_none());
    }

    #[test]
    fn every_scheme_is_readable_on_every_surface() {
        for theme in THEMES {
            let p = theme.palette;
            for (surface, name) in [
                (p.canvas, "canvas"),
                (p.panel, "panel"),
                (p.bar, "bar"),
                (p.raised, "raised"),
            ] {
                assert!(
                    contrast(p.text_normal, surface) >= 7.0,
                    "{}: normal text on {name}",
                    theme.id
                );
                assert!(
                    contrast(p.text_weak, surface) >= 3.5,
                    "{}: weak text on {name}",
                    theme.id
                );
                assert!(
                    contrast(p.accent, surface) >= 3.0,
                    "{}: accent on {name}",
                    theme.id
                );
                for (tone, color) in [("good", p.good), ("warn", p.warn), ("bad", p.bad)] {
                    assert!(
                        contrast(color, surface) >= 3.0,
                        "{}: {tone} on {name}",
                        theme.id
                    );
                }
                for (index, color) in p.traces.iter().enumerate() {
                    assert!(
                        contrast(*color, surface) >= 3.0,
                        "{}: trace {index} on {name}",
                        theme.id
                    );
                }
            }
            assert!(
                contrast(p.on_accent, p.accent) >= 4.5,
                "{}: text on accent buttons",
                theme.id
            );
            assert!(
                contrast(p.text_strong, p.accent_dim) >= 4.5,
                "{}: text on selected fill",
                theme.id
            );
            // Controls must stand out from the containers they sit on.
            for surface in [p.panel, p.bar, p.raised] {
                assert!(
                    contrast(p.control_border, surface) >= 1.25,
                    "{}: control outline",
                    theme.id
                );
            }
            assert_eq!(p.dark, luminance(p.canvas) < luminance(p.text_normal));
        }
    }
}
