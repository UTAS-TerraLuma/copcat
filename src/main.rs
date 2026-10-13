mod app;
mod cloud;

use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("COPC Viewer")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "COPC Viewer",
        options,
        Box::new(|_cc| Ok(Box::new(app::ViewerApp::default()))),
    )
}
