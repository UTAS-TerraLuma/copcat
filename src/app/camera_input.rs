//! Mouse control of a 3D view's camera. Keyboard shortcuts are commands,
//! in [`commands`](super::commands).

use eframe::egui;

use crate::camera::OrbitCamera;

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

/// Mouse control of a 3D view's camera. A left-drag orbits only if
/// `orbit`; otherwise something else (a subcloud tool) has it.
pub(crate) fn camera_controls(
    ui: &egui::Ui,
    response: &egui::Response,
    camera: &mut OrbitCamera,
    orbit: bool,
) -> ViewInput {
    let drag = glam::vec2(response.drag_delta().x, response.drag_delta().y);
    if orbit && response.dragged_by(egui::PointerButton::Primary) {
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
    input
}
