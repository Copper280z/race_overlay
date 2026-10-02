#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod analysis_app;
mod analysis_imagery;
mod analysis_maps;
mod app_paths;
mod native_menu;
mod race_app;
mod ui_kit;
mod video_processing;

fn main() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Race Overlay")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([980.0, 640.0]);
    if cfg!(target_os = "macos") {
        // Draw the header into the title bar. Windows and Linux keep their
        // native decorations, so these options are macOS-only.
        viewport = viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Race Overlay",
        options,
        Box::new(|cc| Ok(Box::new(race_app::RaceOverlayApp::new(cc)))),
    )
}
