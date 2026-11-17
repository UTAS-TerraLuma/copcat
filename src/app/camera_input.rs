//! Mouse and keyboard control of a 3D view's camera.

use eframe::egui;

use crate::camera::{OrbitCamera, ViewPreset};

/// Numpad-style view keys, as in Blender: `(key, view, view with Ctrl)`.
const VIEW_SHORTCUTS: [(egui::Key, ViewPreset, ViewPreset); 3] = [
    (egui::Key::Num7, ViewPreset::Top, ViewPreset::Bottom),
    (egui::Key::Num1, ViewPreset::Front, ViewPreset::Back),
    (egui::Key::Num3, ViewPreset::Right, ViewPreset::Left),
];
const PROJECTION_SHORTCUT: egui::Key = egui::Key::Num5;

/// Input over a 3D view that the caller acts on, beyond moving the camera.
pub(crate) struct ViewInput {
    /// Double-clicked: go back to the default view.
    pub(crate) reset: bool,
    /// A Ctrl+scroll or touchpad pinch, as the scroll distance in logical
    /// pixels that would make the same zoom. egui reports Ctrl+scroll as a
    /// zoom gesture rather than a scroll. Positive is scrolling up.
    pub(crate) pinch_px: f32,
    /// Shift was held, as in Ctrl+Shift+scroll. egui still reports that
    /// as the same zoom, just with Shift down.
    pub(crate) sideways: bool,
}

/// Mouse and keyboard control of a 3D view's camera.
pub(crate) fn camera_controls(
    ui: &egui::Ui,
    response: &egui::Response,
    camera: &mut OrbitCamera,
) -> ViewInput {
    let drag = glam::vec2(response.drag_delta().x, response.drag_delta().y);
    if response.dragged_by(egui::PointerButton::Primary) {
        camera.orbit(drag);
    }
    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        camera.pan(drag, response.rect.height());
    }
    let mut input = ViewInput {
        reset: response.double_clicked(),
        pinch_px: 0.0,
        sideways: ui.input(|i| i.modifiers.shift),
    };
    if response.hovered() {
        let (scroll, zoom_delta) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        if scroll != 0.0 {
            camera.zoom(scroll);
        }
        // egui turned these pixels into a zoom factor of exp(px × speed).
        let zoom_speed = ui.ctx().options(|o| o.input_options.scroll_zoom_speed);
        input.pinch_px = zoom_delta.ln() / zoom_speed;
    }

    // Digits typed into text fields aren't shortcuts.
    if !ui.ctx().text_edit_focused() {
        ui.input_mut(|input| {
            for (key, preset, opposite) in VIEW_SHORTCUTS {
                // Ctrl first: a plain-key check would also match Ctrl+key.
                if input.consume_key(egui::Modifiers::COMMAND, key) {
                    camera.set_view(opposite);
                } else if input.consume_key(egui::Modifiers::NONE, key) {
                    camera.set_view(preset);
                }
            }
            if input.consume_key(egui::Modifiers::NONE, PROJECTION_SHORTCUT) {
                camera.set_projection(camera.projection().toggled());
            }
        });
    }
    input
}
