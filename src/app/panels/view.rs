//! The Projection section. The views from each side, and Reset,
//! are on the 3D view's gizmo, in the command palette and on shortcuts.

use eframe::egui;

use crate::camera::{OrbitCamera, Projection};

use super::widgets::{prop_control, segmented};
use crate::app::commands::Command;

/// The Projection section's contents, for `camera`, the 3D view's. Picking a projection asks
/// for a command.
pub(crate) fn view_ui(ui: &mut egui::Ui, camera: &mut OrbitCamera, commands: &mut Vec<Command>) {
    let options = [
        (Projection::Perspective, "Perspective"),
        (Projection::Orthographic, "Orthographic"),
    ];
    // No row label: the section's header already says Projection.
    prop_control(ui, "projection", |ui| {
        let shown = options.map(|(projection, label)| (label, camera.projection() == projection));
        // A click on the option already selected still counts: the camera
        // needs to hear the user chose it.
        if let Some(index) = segmented(ui, &shown) {
            commands.push(Command::SetProjection(options[index].0));
        }
    });
    prop_control(ui, "auto_orthographic", |ui| {
        ui.checkbox(&mut camera.auto_orthographic, "Orthographic for axis views")
            .on_hover_text(
                "Snapping to top, front, etc. switches to orthographic; \
                 orbiting away switches back to perspective.",
            );
    });
}
