//! Copcat, a COPC point cloud viewer, as a library shared by the `copcat-viewer`
//! window and the `copcat` command line.

mod app;
mod camera;
mod cloud;
mod export;
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

use std::sync::Arc;

use eframe::{egui, egui_wgpu};

pub use export::{RenderArgs, render};

/// Opens the viewer window and blocks until it's closed. `initial_source` is a
/// COPC URL or file path to load on startup.
pub fn run_viewer(initial_source: Option<String>) -> eframe::Result {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Copcat")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    // egui asks for 8192 pixel textures at most; Save image can draw as big
    // as the GPU allows.
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        let egui_descriptor = setup.device_descriptor.clone();
        setup.device_descriptor = Arc::new(move |adapter| {
            let mut descriptor = egui_descriptor(adapter);
            descriptor.required_limits.max_texture_dimension_2d =
                adapter.limits().max_texture_dimension_2d;
            descriptor
        });
    }

    eframe::run_native(
        "Copcat",
        options,
        Box::new(|cc| Ok(Box::new(app::ViewerApp::new(cc, initial_source)?))),
    )
}
