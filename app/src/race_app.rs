//! Thin workspace facade. Analysis remains hosted here for handoff, while all
//! Overlay document, worker, rendering, and panel behavior lives under
//! [`editor::OverlayEditor`].

use crate::ui_kit::{
    Tone,
    theme::{self, surface},
    widgets,
};
use eframe::egui;
use overlay_core::UnitSystem;

const PREVIEW_W: u32 = 960;
const PREVIEW_H: u32 = 540;
const INSTA360_IMU_SAMPLE_RATE_HZ: f64 = 1_000.0;
const UNIT_SYSTEM_STORAGE_KEY: &str = "race-overlay.default-unit-system";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportCodecChoice {
    MatchSource,
    H264,
    H265,
}

impl ExportCodecChoice {
    fn label(self) -> &'static str {
        match self {
            Self::MatchSource => "Match source",
            Self::H264 => "H.264 (most compatible)",
            Self::H265 => "H.265 / HEVC (smaller files)",
        }
    }

    fn codec(self) -> Option<overlay_media::Encoder> {
        match self {
            Self::MatchSource => None,
            Self::H264 => Some(overlay_media::Encoder::H264),
            Self::H265 => Some(overlay_media::Encoder::H265),
        }
    }
}

pub struct RaceOverlayApp {
    analysis: crate::analysis_app::AnalysisApp,
    analysis_mode: bool,
    editor: OverlayEditor,
    /// Last Overlay status already surfaced while Analysis was showing.
    seen_editor_status: String,
    window_title: String,
    /// Absent in headless tests, where there is no main-thread menu bar.
    menu: Option<crate::native_menu::NativeMenu>,
}

impl RaceOverlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::build(cc, true)
    }

    fn build(cc: &eframe::CreationContext<'_>, native_menu: bool) -> Self {
        let chosen = cc
            .storage
            .and_then(|storage| storage.get_string(theme::STORAGE_KEY))
            .and_then(|id| theme::find(&id))
            .unwrap_or(&theme::THEMES[0]);
        theme::apply(&cc.egui_ctx, &chosen.palette);
        let editor = OverlayEditor::new(cc);
        Self {
            analysis: crate::analysis_app::AnalysisApp::new(),
            analysis_mode: true,
            seen_editor_status: editor.status().to_owned(),
            editor,
            window_title: String::new(),
            menu: native_menu.then(|| crate::native_menu::NativeMenu::install(&cc.egui_ctx)),
        }
    }

    fn unit_system(&self) -> UnitSystem {
        self.editor.unit_system()
    }

    fn set_mode(&mut self, analysis: bool) {
        if analysis == self.analysis_mode {
            return;
        }
        if analysis {
            self.editor.stop_playback();
            if !self.editor.is_linked_to_analysis() {
                self.analysis.clear_overlay_link();
            }
            if let Some(project) = self.editor.project().cloned() {
                self.analysis.import_project(project);
            }
        } else {
            self.analysis.pause();
        }
        self.analysis_mode = analysis;
    }

    fn set_unit_system(&mut self, unit_system: UnitSystem) {
        if self.analysis_mode {
            self.editor.set_unit_system(unit_system);
        } else {
            self.editor
                .apply_panel_action(editor::PanelAction::SetUnitSystem(unit_system));
        }
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        // On macOS the header doubles as the title bar and must clear the
        // window buttons; other platforms keep their native title bar.
        let mac = cfg!(target_os = "macos");
        let margin = egui::Margin::symmetric(12, if mac { 5 } else { 6 });
        egui::Panel::top("workspace-header")
            .frame(theme::bar_frame(surface::bar(), margin))
            .show_separator_line(false)
            .show(ui, |ui| {
                if mac {
                    self.drag_window_from(ui);
                }
                ui.horizontal(|ui| {
                    if mac {
                        ui.add_space(72.0);
                    }
                    let mut mode = self.analysis_mode;
                    if widgets::segmented(
                        ui,
                        &mut mode,
                        &[
                            (
                                true,
                                "Analysis",
                                "Compare laps and runs · video and imagery optional",
                            ),
                            (false, "Overlay", "Edit widgets and export video"),
                        ],
                    ) {
                        self.set_mode(mode);
                    }
                    ui.separator();
                    if self.analysis_mode {
                        self.analysis.header_left(ui);
                    } else {
                        self.editor.header_left(ui);
                    }
                    // What the right-hand group needs, so the title never
                    // squeezes it.
                    let right_need = if self.analysis_mode { 330.0 } else { 160.0 };
                    let spare = ui.available_width() - right_need - 24.0;
                    if spare > 140.0 {
                        let (name, dirty) = self.document();
                        ui.scope(|ui| {
                            ui.set_max_width(spare.min(240.0));
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(name).color(theme::text::weak()),
                                )
                                .truncate(),
                            );
                        });
                        if dirty {
                            ui.label(
                                egui::RichText::new("•")
                                    .size(22.0)
                                    .color(Tone::Warn.color()),
                            )
                            .on_hover_text("Unsaved changes");
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        self.settings_menu(ui);
                        if self.analysis_mode {
                            self.analysis.header_right(ui);
                        } else {
                            self.editor.header_right(ui);
                        }
                    });
                });
            });
    }

    /// Lets the empty parts of the header move (and double-click zoom) the
    /// window when the native title bar is hidden.
    fn drag_window_from(&self, ui: &mut egui::Ui) {
        let rect = ui.max_rect().expand2(egui::vec2(12.0, 6.0));
        let response = ui.interact(
            rect,
            ui.id().with("header-drag"),
            egui::Sense::click_and_drag(),
        );
        if response.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if response.double_clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
    }

    /// Name of the document the current mode is editing, and whether it has
    /// unsaved changes (Overlay does not track dirty state).
    fn document(&self) -> (String, bool) {
        if self.analysis_mode {
            self.analysis.document_title()
        } else {
            (
                self.editor
                    .document_name()
                    .unwrap_or_else(|| "Untitled project".into()),
                false,
            )
        }
    }

    fn settings_menu(&mut self, ui: &mut egui::Ui) {
        widgets::popover(ui, "⚙", |ui| {
            widgets::section_label(ui, "Units");
            let mut units = self.unit_system();
            if widgets::segmented(
                ui,
                &mut units,
                &[
                    (UnitSystem::Metric, "Metric", "km/h, m, °C"),
                    (UnitSystem::Imperial, "Imperial", "mph, ft, °F"),
                ],
            ) {
                self.set_unit_system(units);
            }
            widgets::section_label(ui, "Theme");
            let current = theme::active();
            egui::ComboBox::from_id_salt("theme-picker")
                .selected_text(current.name)
                .show_ui(ui, |ui| {
                    for choice in theme::THEMES {
                        if ui
                            .selectable_label(choice.id == current.id, choice.name)
                            .clicked()
                        {
                            theme::apply(ui.ctx(), &choice.palette);
                        }
                    }
                });
            widgets::section_label(ui, "Keyboard");
            for (keys, action) in [
                ("Space", "Play / pause"),
                ("◀ ▶", "Step one frame or sample"),
                ("⌘/Ctrl S", "Save"),
            ] {
                ui.horizontal(|ui| {
                    ui.monospace(keys);
                    ui.weak(action);
                });
            }
            widgets::section_label(ui, "About");
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.hyperlink_to(
                "Project website",
                "https://github.com/Copper280z/race_overlay",
            );
            widgets::hint(ui, "Race Overlay is licensed under the MIT License.");
            widgets::hint(
                ui,
                "Video features use the separate FFmpeg and FFprobe programs under their applicable LGPL/GPL terms.",
            );
            ui.hyperlink_to("FFmpeg licensing", "https://ffmpeg.org/legal.html");
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .frame(theme::bar_frame(
                surface::canvas(),
                egui::Margin::symmetric(12, 4),
            ))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let busy = self
                        .analysis_mode
                        .then(|| self.analysis.busy_label())
                        .flatten();
                    if let Some(busy) = &busy {
                        ui.spinner();
                        ui.weak(busy);
                    } else {
                        let message = if self.analysis_mode {
                            self.analysis.message().to_owned()
                        } else {
                            self.editor.status().to_owned()
                        };
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(message).color(theme::text::weak()),
                            )
                            .truncate(),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(error) = self.editor.tools_error() {
                            widgets::chip(ui, "FFmpeg unavailable", Tone::Warn)
                                .on_hover_text(error);
                        }
                        if self.editor.export_running() {
                            if ui.button("Cancel").clicked() {
                                self.editor.cancel_export();
                            }
                            ui.add(
                                egui::ProgressBar::new(self.editor.export_fraction())
                                    .desired_width(140.0)
                                    .show_percentage(),
                            );
                            ui.weak("Exporting");
                        }
                        let issues = self.analysis.issues().len();
                        if self.analysis_mode && issues > 0 {
                            let mut clear = false;
                            widgets::popover(
                                ui,
                                egui::RichText::new(format!("⚠ {issues} issue(s)"))
                                    .color(Tone::Bad.color()),
                                |ui| {
                                    for issue in self.analysis.issues() {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(issue).color(Tone::Bad.color()),
                                            )
                                            .wrap(),
                                        );
                                    }
                                    clear = ui.button("Dismiss all").clicked();
                                },
                            );
                            if clear {
                                self.analysis.clear_issues();
                            }
                        }
                    });
                });
            });
    }

    /// Runs commands chosen in the operating-system menu bar (macOS) and keeps
    /// its check marks in step with the application.
    fn handle_menu(&mut self, ctx: &egui::Context) {
        use crate::native_menu::{MenuCommand, MenuState};
        let commands = self
            .menu
            .as_mut()
            .map(|menu| menu.poll())
            .unwrap_or_default();
        for command in commands {
            match command {
                MenuCommand::AddFiles => {
                    self.set_mode(true);
                    self.analysis.add_files_dialog();
                }
                MenuCommand::OpenWorkspace => {
                    self.set_mode(true);
                    self.analysis.open_workspace_dialog();
                }
                MenuCommand::OpenVideo => {
                    self.set_mode(false);
                    self.editor.open_video_dialog();
                }
                MenuCommand::OpenProject => {
                    self.set_mode(false);
                    self.editor.open_project_dialog();
                }
                MenuCommand::Save => self.save_document(false),
                MenuCommand::SaveAs => self.save_document(true),
                MenuCommand::ShowAnalysis => self.set_mode(true),
                MenuCommand::ShowOverlay => self.set_mode(false),
                MenuCommand::SetTheme(id) => {
                    if let Some(scheme) = theme::find(id) {
                        theme::apply(ctx, &scheme.palette);
                    }
                }
                MenuCommand::SetUnits(units) => self.set_unit_system(units),
                MenuCommand::AddPanel(choice) => {
                    self.set_mode(true);
                    self.analysis.add_panel(choice);
                }
                MenuCommand::LayoutPreset(index) => {
                    self.set_mode(true);
                    self.analysis.layout_preset(index);
                }
                MenuCommand::OpenWebsite => {
                    ctx.open_url(egui::OpenUrl::new_tab(
                        "https://github.com/Copper280z/race_overlay",
                    ));
                }
                MenuCommand::OpenFfmpegLicensing => {
                    ctx.open_url(egui::OpenUrl::new_tab("https://ffmpeg.org/legal.html"));
                }
            }
        }
        let state = MenuState {
            analysis_mode: self.analysis_mode,
            theme_id: theme::active().id,
            units: self.unit_system(),
        };
        if let Some(menu) = &mut self.menu {
            menu.sync(state);
        }
    }

    fn save_document(&mut self, as_new: bool) {
        match (self.analysis_mode, as_new) {
            (true, false) => self.analysis.save_workspace(),
            (true, true) => self.analysis.save_workspace_as(),
            (false, false) => self.editor.save_current(),
            (false, true) => self.editor.save_current_as(),
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.text_edit_focused();
        let (step, toggle, save) = ctx.input_mut(|input| {
            let save = input.consume_key(egui::Modifiers::COMMAND, egui::Key::S);
            if typing {
                return (0, false, save);
            }
            let step = if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                -1
            } else if input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                1
            } else {
                0
            };
            let toggle = input.consume_key(egui::Modifiers::NONE, egui::Key::Space);
            (step, toggle, save)
        });
        if step != 0 {
            if self.analysis_mode {
                self.analysis.step_reference(step);
            } else {
                self.editor.step_frame(step);
            }
        }
        if toggle {
            if self.analysis_mode {
                self.analysis.toggle_play();
            } else {
                self.editor.toggle_playback();
            }
        }
        if save {
            self.save_document(false);
        }
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let (name, dirty) = self.document();
        let title = format!("{name}{} — Race Overlay", if dirty { " •" } else { "" });
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    /// Overlay results (export finished, load errors) must stay visible even
    /// when Analysis is the mode being shown.
    fn surface_editor_status(&mut self) {
        let status = self.editor.status();
        if status != self.seen_editor_status {
            self.seen_editor_status = status.to_owned();
            if self.analysis_mode {
                self.analysis.notify(status.to_owned());
            }
        }
    }
}

// Workspace application glue remains in the façade. Feature behavior is implemented in private modules above.
impl eframe::App for RaceOverlayApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(theme::STORAGE_KEY, theme::active().id.into());
        storage.set_string(
            UNIT_SYSTEM_STORAGE_KEY,
            match self.unit_system() {
                UnitSystem::Metric => "metric",
                UnitSystem::Imperial => "imperial",
            }
            .into(),
        );
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.editor.poll_workers(&ctx);
        self.handle_shortcuts(&ctx);
        self.handle_menu(&ctx);
        self.surface_editor_status();
        self.sync_window_title(&ctx);
        self.header(ui);
        self.status_bar(ui);
        if self.analysis_mode {
            self.analysis
                .ui(ui, self.editor.tools(), self.unit_system());
            for action in self.analysis.take_actions() {
                match action {
                    crate::analysis_app::AnalysisAction::OpenOverlay(project) => {
                        if self.editor.open_analysis_overlay(project) {
                            self.analysis_mode = false;
                        } else {
                            self.analysis
                                .report_overlay_open_error(self.editor.status().to_owned());
                        }
                    }
                }
            }
            // An export already in progress continues while data is inspected.
            // Analysis asks for its own repaints while it has work in flight;
            // an idle window must not repaint at all.
            if self.editor.export_running() {
                self.editor.export_dialog(&ctx);
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
            return;
        }
        self.editor.load_pending_overlay_texture(&ctx);
        if self.editor.playback_finished() {
            self.editor.stop_playback();
        }
        egui::Panel::left("sources")
            .resizable(true)
            .default_size(290.0)
            .frame(theme::bar_frame(surface::panel(), egui::Margin::same(12)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.editor.sources_panel(ui));
            });
        egui::Panel::right("widgets")
            .resizable(true)
            .default_size(320.0)
            .frame(theme::bar_frame(surface::panel(), egui::Margin::same(12)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.editor.widgets_panel(ui));
            });
        egui::CentralPanel::default()
            .frame(theme::bar_frame(surface::canvas(), egui::Margin::same(12)))
            .show(ui, |ui| {
                if self.editor.plot_visible() {
                    self.editor.data_plot_ui(ui);
                } else {
                    self.editor.preview_ui(ui);
                }
            });
        self.editor.export_dialog(&ctx);
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

mod appearance_policy;
mod controllers;
mod editor;
mod export_policy;
mod policy;
mod session;
#[cfg(test)]
mod snapshots;
mod source_policy;
#[cfg(test)]
mod tests;
mod widget_policy;

use editor::OverlayEditor;
