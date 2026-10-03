//! Small reusable widgets built on the theme. Each one replaces a pattern that
//! was previously hand-rolled in several panels.

use super::theme::{self, Tone, surface, text};
use eframe::egui::{
    self, Color32, CornerRadius, Margin, RichText, Stroke, Ui, containers::menu::MenuButton,
};

/// A pill-shaped group of mutually exclusive choices. Each option carries a
/// tooltip (empty for none).
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut Ui,
    value: &mut T,
    options: &[(T, &str, &str)],
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(surface::canvas())
        .corner_radius(CornerRadius::same(theme::RADIUS + 2))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (option, label, tip) in options {
                    let selected = *value == *option;
                    let label = if selected {
                        RichText::new(*label).color(text::strong())
                    } else {
                        RichText::new(*label).color(text::weak())
                    };
                    let button = egui::Button::new(label)
                        .fill(if selected {
                            theme::accent_dim()
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE);
                    let response = ui.add(button);
                    let response = if tip.is_empty() {
                        response
                    } else {
                        response.on_hover_text(*tip)
                    };
                    if response.clicked() && !selected {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// An on/off switch.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let size = egui::vec2(34.0, 18.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });
    if ui.is_rect_visible(rect) {
        let t = ui.ctx().animate_bool_responsive(response.id, *on);
        let radius = rect.height() / 2.0;
        let track = if *on {
            theme::accent()
        } else {
            surface::control_border()
        };
        ui.painter().rect_filled(rect, radius, track);
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), t);
        let knob = if *on {
            theme::on_accent()
        } else {
            surface::raised()
        };
        ui.painter()
            .circle_filled(egui::pos2(x, rect.center().y), radius - 3.0, knob);
    }
    response
}

/// Upper-case caption that introduces a group of controls.
pub fn section_label(ui: &mut Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(text::weak()),
    );
}

/// Secondary explanatory text that wraps instead of stretching the panel.
pub fn hint(ui: &mut Ui, message: impl Into<String>) {
    ui.add(egui::Label::new(RichText::new(message.into()).small().color(text::weak())).wrap());
}

/// Grouped content on a raised surface; `selected` adds an accent outline.
pub fn card<R>(
    ui: &mut Ui,
    selected: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(surface::raised())
        .stroke(Stroke::new(
            1.0,
            if selected {
                theme::accent()
            } else {
                surface::border()
            },
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
}

/// A dense selectable row for lists of items (widgets, sources).
pub fn list_row<R>(
    ui: &mut Ui,
    selected: bool,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> egui::InnerResponse<R> {
    let inner = egui::Frame::new()
        .fill(if selected {
            theme::accent_dim()
        } else {
            surface::raised()
        })
        .stroke(Stroke::new(
            1.0,
            if selected {
                theme::accent()
            } else {
                Color32::TRANSPARENT
            },
        ))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        });
    let response = inner.response.interact(egui::Sense::click());
    if response.hovered() && !selected {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(theme::RADIUS),
            Stroke::new(1.0, surface::control_border()),
            egui::StrokeKind::Inside,
        );
    }
    egui::InnerResponse::new(inner.inner, response)
}

/// Compact colored label for state ("Synced", "Estimated", …).
pub fn chip(ui: &mut Ui, label: &str, tone: Tone) -> egui::Response {
    egui::Frame::new()
        .fill(tone.fill())
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| {
            ui.label(RichText::new(label).small().color(tone.color()))
        })
        .response
}

/// A full-width message with a tint; used for warnings that need an action.
pub fn callout(ui: &mut Ui, tone: Tone, message: impl Into<String>) {
    egui::Frame::new()
        .fill(tone.fill())
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(message.into()).color(tone.color())).wrap());
        });
}

/// The one obvious action in a region.
pub fn primary_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).strong().color(theme::on_accent()))
            .fill(theme::accent()),
    )
}

/// Destructive action; visually quiet until hovered.
pub fn danger_button(ui: &mut Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(Tone::Bad.color()))
            .fill(Tone::Bad.fill())
            .stroke(Stroke::NONE),
    )
}

/// Floating panel that stays open while its controls are used (sliders,
/// checkboxes, drag values, dropdowns) and closes on an outside click, Escape,
/// or [`close_popover`].
///
/// This is deliberately not an egui menu or popup: egui keeps one "open popup"
/// slot per window, so a dropdown inside a menu would close the menu that
/// contains it. A clicked-in dropdown list counts as inside here.
pub fn popover<R>(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let ctx = ui.ctx().clone();
    let anchor = ui.add(egui::Button::new(label.into()).wrap_mode(egui::TextWrapMode::Extend));
    let id = anchor.id.with("popover");
    let was_open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let mut open = was_open != anchor.clicked();
    let mut result = None;
    if open {
        // Open toward whichever side of the window has more room.
        let below = anchor.rect.center().y < ctx.content_rect().center().y;
        let (pivot, position) = if below {
            (
                egui::Align2::LEFT_TOP,
                anchor.rect.left_bottom() + egui::vec2(0.0, 4.0),
            )
        } else {
            (
                egui::Align2::LEFT_BOTTOM,
                anchor.rect.left_top() - egui::vec2(0.0, 4.0),
            )
        };
        let area = egui::Area::new(id)
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(position)
            .constrain(true)
            .sizing_pass(!was_open)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| {
                        ui.set_min_width(260.0);
                        ui.set_max_width(380.0);
                        add_contents(ui)
                    })
                    .inner
            });
        result = Some(area.inner);
        let close_requested = ctx.data_mut(|d| d.remove_temp::<bool>(close_request_id()).is_some());
        let clicked_outside = !anchor.clicked()
            && ctx.input(|i| i.pointer.any_click())
            && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|pos| {
                ctx.layer_id_at(pos)
                    .is_none_or(|layer| layer.order != egui::Order::Foreground)
            });
        if close_requested || clicked_outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            open = false;
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, open));
    result
}

fn close_request_id() -> egui::Id {
    egui::Id::new("popover-close-request")
}

/// Closes the innermost [`popover`] after the current frame; the popover
/// counterpart of `Ui::close` for menus.
pub fn close_popover(ui: &Ui) {
    ui.ctx()
        .data_mut(|d| d.insert_temp(close_request_id(), true));
}

/// Menu of one-shot actions; closes after a choice.
pub fn action_menu<R>(
    ui: &mut Ui,
    label: impl Into<egui::WidgetText>,
    add_contents: impl FnOnce(&mut Ui) -> R,
) -> Option<R> {
    let (_, inner) = MenuButton::new(label.into()).ui(ui, |ui| {
        ui.set_min_width(200.0);
        ui.set_max_width(280.0);
        add_contents(ui)
    });
    inner.map(|inner| inner.inner)
}

/// A wide menu entry with an optional caption underneath.
pub fn menu_item(ui: &mut Ui, label: &str, caption: Option<&str>) -> bool {
    let clicked = ui
        .add(
            egui::Button::new(label)
                .frame(false)
                .min_size(egui::vec2(ui.available_width(), 24.0)),
        )
        .clicked();
    if let Some(caption) = caption {
        hint(ui, caption);
        ui.add_space(2.0);
    }
    clicked
}

/// Centered placeholder for a region with nothing to show yet.
pub fn empty_state<R>(
    ui: &mut Ui,
    title: &str,
    body: &str,
    actions: impl FnOnce(&mut Ui) -> R,
) -> R {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.25).clamp(8.0, 120.0));
        ui.label(
            RichText::new(title)
                .size(17.0)
                .strong()
                .color(text::strong()),
        );
        ui.add_space(2.0);
        ui.add(egui::Label::new(RichText::new(body).color(text::weak())).wrap());
        ui.add_space(10.0);
        actions(ui)
    })
    .inner
}

/// Builds a percentage-formatted drag value for a `0..=1` fraction.
pub fn percent_widget<'a>(
    value: &'a mut f32,
    min: f32,
    max: f32,
    prefix: &str,
) -> egui::DragValue<'a> {
    egui::DragValue::new(value)
        .speed(0.002)
        .range(min..=max)
        .prefix(prefix.to_owned())
        .custom_formatter(|fraction, _| format!("{:.1}%", fraction * 100.0))
        .custom_parser(|text| {
            text.trim()
                .trim_end_matches('%')
                .parse::<f64>()
                .ok()
                .map(|percent| percent / 100.0)
        })
}

/// `m:ss.mmm` for transport readouts.
pub fn clock(seconds: f64) -> String {
    let seconds = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    format!("{}:{:06.3}", (seconds / 60.0) as u64, seconds % 60.0)
}

/// Small filled circle used as a legend swatch (the bundled fonts have no `●`).
pub fn dot(ui: &mut Ui, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(12.0, 14.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, color);
    response
}

/// `text` shortened with an ellipsis so it fits `max_width` in the body font.
/// Widgets like checkboxes do not shrink their labels, so long names would
/// otherwise run into whatever is to their right.
pub fn fit_text(ui: &Ui, text: &str, max_width: f32) -> String {
    let width = |candidate: &str| text_width(ui, candidate, egui::TextStyle::Body);
    if width(text) <= max_width {
        return text.to_owned();
    }
    let chars = text.chars().collect::<Vec<_>>();
    let (mut low, mut high) = (0, chars.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = format!("{}…", chars[..mid].iter().collect::<String>());
        if width(&candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    format!("{}…", chars[..low].iter().collect::<String>())
}

/// Measure a single line using the same font as its eventual widget.
pub fn text_width(ui: &Ui, text: &str, style: egui::TextStyle) -> f32 {
    let font = style.resolve(ui.style());
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(text.to_owned(), font, Color32::WHITE)
            .size()
            .x
    })
}
