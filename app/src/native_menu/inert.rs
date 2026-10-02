//! Stand-in for platforms whose menu lives inside the window.

use super::{MenuCommand, MenuState};

pub struct NativeMenu;

impl NativeMenu {
    pub fn install(_ctx: &eframe::egui::Context) -> Self {
        Self
    }

    pub fn poll(&mut self) -> Vec<MenuCommand> {
        Vec::new()
    }

    pub fn sync(&mut self, _state: MenuState) {}
}
