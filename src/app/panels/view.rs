//! The View section: the projection. The views from each side, and Reset,
//! are on the 3D view's gizmo, in the command palette and on shortcuts.

use eframe::egui;

use crate::camera::{OrbitCamera, Projection};

use crate::app::commands::Command;

/// The View section, for `camera`, the 3D view's. Picking a projection asks
/// for a command.
pub(crate) fn view_ui(ui: &mut egui::Ui, camera: &mut OrbitCamera, commands: &mut Vec<Command>) {
    ui.horizontal_wrapped(|ui| {
        ui.weak("Projection");
        // Not `radio_value`: the camera needs to hear about the click, even on
        // the option already selected, to know the user chose it.
        for (projection, label) in [
            (Projection::Perspective, "Perspective"),
            (Projection::Orthographic, "Orthographic"),
        ] {
            if ui.radio(camera.projection() == projection, label).clicked() {
                commands.push(Command::SetProjection(projection));
            }
        }
    });
    ui.checkbox(&mut camera.auto_orthographic, "Orthographic for axis views")
        .on_hover_text(
            "Snapping to top, front, etc. switches to orthographic; \
             orbiting away switches back to perspective.",
        );
}
