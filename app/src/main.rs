mod race_app;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Race Overlay")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Race Overlay",
        options,
        Box::new(|cc| Ok(Box::new(race_app::RaceOverlayApp::new(cc)))),
    )
}
