//! Copcat, a COPC point cloud viewer, as a library shared by the `copcat-viewer`
//! window and the `copcat` command line.

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

/// Opens the viewer window and blocks until it's closed. `initial_source` is a
/// COPC URL or file path to load on startup.
pub fn run_viewer(initial_source: Option<String>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Copcat")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Copcat",
        options,
        Box::new(|cc| Ok(Box::new(app::ViewerApp::new(cc, initial_source)?))),
    )
}
