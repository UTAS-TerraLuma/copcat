//! The View section: projection and view presets.

use eframe::egui;

use crate::camera::{OrbitCamera, Projection, ViewPreset};

pub(crate) fn view_ui(ui: &mut egui::Ui, camera: &mut OrbitCamera) {
    ui.horizontal_wrapped(|ui| {
        ui.label("Projection:");
        // Not `radio_value`: the camera needs to hear about the click, even on
        // the option already selected, to know the user chose it.
        for (projection, label) in [
            (Projection::Perspective, "Perspective"),
            (Projection::Orthographic, "Orthographic"),
        ] {
            if ui.radio(camera.projection() == projection, label).clicked() {
                camera.set_projection(projection);
            }
        }
    });
    ui.checkbox(&mut camera.auto_orthographic, "Orthographic for axis views")
        .on_hover_text(
            "Snapping to top, front, etc. switches to orthographic; \
             orbiting away switches back to perspective.",
        );
    ui.horizontal_wrapped(|ui| {
        ui.label("View:");
        for preset in ViewPreset::ALL {
            if ui
                .button(preset.label())
                .on_hover_text(preset.description(camera.axes))
                .clicked()
            {
                camera.set_view(preset);
            }
        }
    });
}
