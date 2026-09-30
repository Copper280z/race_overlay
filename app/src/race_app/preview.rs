//! Video and overlay preview controls.
use super::super::policy::{format_time, normalized_to_screen, point_in};
use super::super::{PREVIEW_H, PREVIEW_W};
use super::{OverlayEditor, PanelAction};
use eframe::egui;

impl OverlayEditor {
    pub(in crate::race_app) fn preview_ui(&mut self, ui: &mut egui::Ui) {
        let mut processing = self.project().and_then(|p| p.video_processing.clone());
        if let Some(config) = &mut processing {
            ui.checkbox(
                &mut self.preview_controller.reframing,
                "Reframe video (drag to aim, scroll for FOV)",
            );
            if crate::video_processing::controls(ui, config) {
                if let Some(overlay_core::ProjectDocument::V1(project)) = self.session.project_mut()
                {
                    project.video_processing = Some(config.clone());
                }
                self.request_preview();
            }
        }
        if let Some(preview) = &self.preview_controller.preview {
            let status = preview.status();
            if !status.is_empty() {
                ui.weak(status);
            }
        }
        let reframing = processing.is_some() && self.preview_controller.reframing;
        let available = ui.available_size();
        let aspect = PREVIEW_W as f32 / PREVIEW_H as f32;
        let mut width = available.x;
        let mut height = width / aspect;
        if height > available.y - 60.0 {
            height = (available.y - 60.0).max(120.0);
            width = height * aspect;
        }
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
        if self.preview_controller.video_texture.is_some()
            && let Some(texture) = &self.preview_controller.video_texture
        {
            ui.painter()
                .image(texture.id(), rect, uv, egui::Color32::WHITE);
        } else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Video preview",
                egui::FontId::proportional(22.0),
                egui::Color32::GRAY,
            );
        }
        if let Some(texture) = &self.preview_controller.overlay_texture {
            ui.painter()
                .image(texture.id(), rect, uv, egui::Color32::WHITE);
        }
        if let Some(widget_id) = self.widget_editor.selected_widget
            && let Some(widget) = self.session.widget(widget_id)
        {
            let wr = normalized_to_screen(widget.rect, rect);
            ui.painter().rect_stroke(
                wr,
                2.0,
                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                egui::StrokeKind::Inside,
            );
            let handle = egui::Rect::from_center_size(wr.right_bottom(), egui::vec2(14.0, 14.0));
            ui.painter().rect_filled(handle, 2.0, egui::Color32::YELLOW);
        }
        if !reframing
            && (response.drag_started() || response.clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let n = egui::pos2(
                (pos.x - rect.left()) / rect.width(),
                (pos.y - rect.top()) / rect.height(),
            );
            let hit = self.project().and_then(|p| {
                p.widgets
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, w)| point_in(n, w.rect))
                    .map(|(_, widget)| widget.id)
            });
            self.apply_panel_action(match hit {
                Some(id) => PanelAction::SelectWidget(id),
                None => PanelAction::ClearWidgetSelection,
            });
            self.widget_editor.resizing_widget = hit
                .and_then(|id| {
                    self.project()
                        .and_then(|p| p.widgets.iter().find(|w| w.id == id))
                })
                .is_some_and(|w| {
                    (n.x - (w.rect.x + w.rect.width)).abs() < 0.03
                        && (n.y - (w.rect.y + w.rect.height)).abs() < 0.05
                });
        }
        if !reframing && response.dragged() {
            let delta = ui.input(|i| i.pointer.delta());
            let nd = egui::vec2(delta.x / rect.width(), delta.y / rect.height());
            let resizing = self.widget_editor.resizing_widget;
            if let Some(widget_id) = self.widget_editor.selected_widget
                && let Some(widget) = self.session.widget_mut(widget_id)
            {
                if resizing {
                    widget.rect.width = (widget.rect.width + nd.x).clamp(0.02, 1.0 - widget.rect.x);
                    widget.rect.height =
                        (widget.rect.height + nd.y).clamp(0.02, 1.0 - widget.rect.y);
                } else {
                    widget.rect.x = (widget.rect.x + nd.x).clamp(0.0, 1.0 - widget.rect.width);
                    widget.rect.y = (widget.rect.y + nd.y).clamp(0.0, 1.0 - widget.rect.height);
                }
                self.refresh_overlay();
            }
        }
        if reframing
            && let Some(config) = &mut processing
            && crate::video_processing::gestures(ui, &response, config)
        {
            if let Some(overlay_core::ProjectDocument::V1(project)) = self.session.project_mut() {
                project.video_processing = Some(config.clone());
            }
            self.request_preview();
        }
        if let Some(preview) = &self.preview_controller.preview {
            let status = preview.status();
            if !status.is_empty() {
                ui.weak(status);
            }
        }
        ui.horizontal(|ui| {
            if ui
                .button(if self.preview_controller.playing {
                    "⏸"
                } else {
                    "▶"
                })
                .clicked()
            {
                if self.preview_controller.playing {
                    self.stop_playback();
                    self.request_preview();
                } else {
                    self.start_playback();
                }
            }
            let mut time = self.session.current_time();
            if ui
                .add(egui::Slider::new(&mut time, 0.0..=self.duration()).show_value(false))
                .changed()
            {
                self.session.set_current_time(time);
                self.stop_playback();
                self.request_preview();
                self.refresh_overlay();
            }
            ui.monospace(format!(
                "{} / {}",
                format_time(self.session.current_time()),
                format_time(self.duration())
            ));
            ui.weak("←/→ frame");
        });
    }
}
