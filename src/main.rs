mod app;
mod camera;
mod cloud;
mod gizmo;
mod http_source;
mod loader;
mod lod;
mod profile;
mod renderer;
mod subcloud;
mod subcloud_map;
#[cfg(windows)]
mod windows_dpi;

use eframe::egui;

fn main() -> eframe::Result {
    // `cargo run -- <url-or-path>` loads a cloud on startup.
    let initial_source = std::env::args().nth(1);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("COPC Viewer")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "COPC Viewer",
        options,
        Box::new(|cc| Ok(Box::new(app::ViewerApp::new(cc, initial_source)?))),
    )
}
