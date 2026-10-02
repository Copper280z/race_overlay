//! The macOS menu bar.

use super::{MenuCommand, MenuState};
use crate::analysis_app::{LAYOUT_PRESETS, PanelChoice};
use crate::ui_kit::theme;
use crossbeam_channel::{Receiver, unbounded};
use eframe::egui;
use muda::{
    AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
    accelerator::{Accelerator, Code, Modifiers},
};
use overlay_core::UnitSystem;
use std::collections::HashMap;

const WEBSITE: &str = "https://github.com/Copper280z/race_overlay";

pub struct NativeMenu {
    // Kept alive: dropping the menu removes it from the menu bar.
    _menu: Menu,
    events: Receiver<String>,
    commands: HashMap<String, MenuCommand>,
    analysis: CheckMenuItem,
    overlay: CheckMenuItem,
    themes: Vec<(&'static str, CheckMenuItem)>,
    metric: CheckMenuItem,
    imperial: CheckMenuItem,
    panels: Submenu,
    last: Option<MenuState>,
}

fn accel(mods: Modifiers, key: Code) -> Option<Accelerator> {
    Some(Accelerator::new(mods, key))
}

/// Builds items and remembers which command each one triggers.
struct Builder {
    commands: HashMap<String, MenuCommand>,
}

impl Builder {
    fn item(
        &mut self,
        id: &str,
        text: &str,
        command: MenuCommand,
        accelerator: Option<Accelerator>,
    ) -> MenuItem {
        self.commands.insert(id.to_owned(), command);
        MenuItem::with_id(id, text, true, accelerator)
    }

    fn check(
        &mut self,
        id: &str,
        text: &str,
        command: MenuCommand,
        accelerator: Option<Accelerator>,
    ) -> CheckMenuItem {
        self.commands.insert(id.to_owned(), command);
        CheckMenuItem::with_id(id, text, true, false, accelerator)
    }
}

impl NativeMenu {
    /// Installs the menu bar. Must run on the main thread once the event loop
    /// exists, which is the case inside eframe's creation callback.
    pub fn install(ctx: &egui::Context) -> Self {
        let (tx, events) = unbounded();
        let repaint = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = tx.send(event.id.0);
            repaint.request_repaint();
        }));

        let cmd = Modifiers::META;
        let cmd_shift = Modifiers::META | Modifiers::SHIFT;
        let cmd_alt = Modifiers::META | Modifiers::ALT;
        let mut b = Builder {
            commands: HashMap::new(),
        };

        let app_menu = Submenu::new("Race Overlay", true);
        let about = AboutMetadata {
            name: Some("Race Overlay".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            website: Some(WEBSITE.into()),
            license: Some("MIT".into()),
            ..Default::default()
        };
        app_menu
            .append_items(&[
                &PredefinedMenuItem::about(Some("About Race Overlay"), Some(about)),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(None),
                &PredefinedMenuItem::hide_others(None),
                &PredefinedMenuItem::show_all(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(None),
            ])
            .ok();

        let file = Submenu::new("File", true);
        file.append_items(&[
            &b.item(
                "file.add",
                "Add Files…",
                MenuCommand::AddFiles,
                accel(cmd, Code::KeyO),
            ),
            &b.item(
                "file.workspace",
                "Open Workspace…",
                MenuCommand::OpenWorkspace,
                accel(cmd_shift, Code::KeyO),
            ),
            &PredefinedMenuItem::separator(),
            &b.item(
                "file.video",
                "Open Video…",
                MenuCommand::OpenVideo,
                accel(cmd_alt, Code::KeyO),
            ),
            &b.item(
                "file.project",
                "Open Project…",
                MenuCommand::OpenProject,
                None,
            ),
            &PredefinedMenuItem::separator(),
            &b.item(
                "file.save",
                "Save",
                MenuCommand::Save,
                accel(cmd, Code::KeyS),
            ),
            &b.item(
                "file.save_as",
                "Save As…",
                MenuCommand::SaveAs,
                accel(cmd_shift, Code::KeyS),
            ),
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::close_window(None),
        ])
        .ok();

        let analysis = b.check(
            "view.analysis",
            "Analysis",
            MenuCommand::ShowAnalysis,
            accel(cmd, Code::Digit1),
        );
        let overlay = b.check(
            "view.overlay",
            "Overlay",
            MenuCommand::ShowOverlay,
            accel(cmd, Code::Digit2),
        );
        let theme_menu = Submenu::new("Theme", true);
        let themes = theme::THEMES
            .iter()
            .map(|scheme| {
                let item = b.check(
                    &format!("theme.{}", scheme.id),
                    scheme.name,
                    MenuCommand::SetTheme(scheme.id),
                    None,
                );
                theme_menu.append(&item).ok();
                (scheme.id, item)
            })
            .collect::<Vec<_>>();
        let metric = b.check(
            "units.metric",
            "Metric",
            MenuCommand::SetUnits(UnitSystem::Metric),
            None,
        );
        let imperial = b.check(
            "units.imperial",
            "Imperial",
            MenuCommand::SetUnits(UnitSystem::Imperial),
            None,
        );
        let units_menu = Submenu::new("Units", true);
        units_menu.append_items(&[&metric, &imperial]).ok();
        let view = Submenu::new("View", true);
        view.append_items(&[
            &analysis,
            &overlay,
            &PredefinedMenuItem::separator(),
            &theme_menu,
            &units_menu,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::fullscreen(None),
        ])
        .ok();

        let panels = Submenu::new("Panels", true);
        for (index, choice) in PanelChoice::ALL.iter().enumerate() {
            panels
                .append(&b.item(
                    &format!("panel.{index}"),
                    choice.label(),
                    MenuCommand::AddPanel(*choice),
                    None,
                ))
                .ok();
        }
        panels.append(&PredefinedMenuItem::separator()).ok();
        for (index, (label, _)) in LAYOUT_PRESETS.iter().enumerate() {
            panels
                .append(&b.item(
                    &format!("layout.{index}"),
                    label,
                    MenuCommand::LayoutPreset(index),
                    None,
                ))
                .ok();
        }

        let window = Submenu::new("Window", true);
        window
            .append_items(&[
                &PredefinedMenuItem::minimize(None),
                &PredefinedMenuItem::maximize(None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::bring_all_to_front(None),
            ])
            .ok();

        let help = Submenu::new("Help", true);
        help.append_items(&[
            &b.item(
                "help.website",
                "Race Overlay Website",
                MenuCommand::OpenWebsite,
                None,
            ),
            &b.item(
                "help.ffmpeg",
                "FFmpeg Licensing",
                MenuCommand::OpenFfmpegLicensing,
                None,
            ),
        ])
        .ok();

        let menu = Menu::new();
        menu.append_items(&[&app_menu, &file, &view, &panels, &window, &help])
            .ok();
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        help.set_as_help_menu_for_nsapp();

        Self {
            _menu: menu,
            events,
            commands: b.commands,
            analysis,
            overlay,
            themes,
            metric,
            imperial,
            panels,
            last: None,
        }
    }

    /// Commands chosen since the last call.
    pub fn poll(&mut self) -> Vec<MenuCommand> {
        let mut chosen = Vec::new();
        while let Ok(id) = self.events.try_recv() {
            if let Some(command) = self.commands.get(&id) {
                chosen.push(*command);
            }
            // Check items flip themselves when clicked; force the next sync
            // to restore the true state.
            self.last = None;
        }
        chosen
    }

    /// Updates check marks and enabled state when `state` has changed.
    pub fn sync(&mut self, state: MenuState) {
        if self.last == Some(state) {
            return;
        }
        self.analysis.set_checked(state.analysis_mode);
        self.overlay.set_checked(!state.analysis_mode);
        for (id, item) in &self.themes {
            item.set_checked(*id == state.theme_id);
        }
        self.metric.set_checked(state.units == UnitSystem::Metric);
        self.imperial
            .set_checked(state.units == UnitSystem::Imperial);
        // Panels only exist in Analysis.
        self.panels.set_enabled(state.analysis_mode);
        self.last = Some(state);
    }
}
