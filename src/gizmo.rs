//! The axis gizmo in the corner of the 3D view. It shows which way X, Y and Z
//! point; clicking an axis end looks along that axis, and dragging it orbits.

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke};
use glam::Vec3;

use crate::camera::{OrbitCamera, ViewPreset};

/// Side of the gizmo's square, in logical pixels.
pub const SIZE: f32 = 96.0;
const HANDLE_RADIUS: f32 = 9.0;
/// Centre to handle, leaving room for the handle inside the square.
const ARM_LENGTH: f32 = SIZE * 0.5 - HANDLE_RADIUS - 2.0;

const X_COLOR: Color32 = Color32::from_rgb(230, 70, 70);
const Y_COLOR: Color32 = Color32::from_rgb(110, 200, 80);
const Z_COLOR: Color32 = Color32::from_rgb(70, 130, 240);

/// One clickable end of an axis.
struct Handle {
    axis: Vec3,
    color: Color32,
    /// Only the positive ends are labelled, like Blender's gizmo.
    label: Option<&'static str>,
    /// The view from this end, looking back at the target.
    preset: ViewPreset,
}

/// A `static` rather than a `const`, so `place_handles` can hand out
/// `&'static Handle`s: a const is copied into each use, a static has one address.
static HANDLES: [Handle; 6] = [
    Handle {
        axis: Vec3::X,
        color: X_COLOR,
        label: Some("X"),
        preset: ViewPreset::Right,
    },
    Handle {
        axis: Vec3::NEG_X,
        color: X_COLOR,
        label: None,
        preset: ViewPreset::Left,
    },
    Handle {
        axis: Vec3::Y,
        color: Y_COLOR,
        label: Some("Y"),
        preset: ViewPreset::Back,
    },
    Handle {
        axis: Vec3::NEG_Y,
        color: Y_COLOR,
        label: None,
        preset: ViewPreset::Front,
    },
    Handle {
        axis: Vec3::Z,
        color: Z_COLOR,
        label: Some("Z"),
        preset: ViewPreset::Top,
    },
    Handle {
        axis: Vec3::NEG_Z,
        color: Z_COLOR,
        label: None,
        preset: ViewPreset::Bottom,
    },
];

/// A handle placed on screen for the current camera.
struct Placed {
    handle: &'static Handle,
    pos: Pos2,
    /// Towards the viewer is positive, so sorting by it paints back to front.
    depth: f32,
}

/// Handles input for the gizmo in `rect` and paints it.
pub fn show(ui: &mut egui::Ui, rect: Rect, camera: &mut OrbitCamera) {
    // Registered after the 3D view, so it's on top and gets the pointer first.
    let response = ui.interact(rect, ui.id().with("axis_gizmo"), Sense::click_and_drag());
    if response.dragged_by(egui::PointerButton::Primary) {
        let drag = response.drag_delta();
        camera.orbit(glam::vec2(drag.x, drag.y));
    }

    let hovered = response
        .hover_pos()
        .and_then(|pointer| handle_at(&place_handles(rect, camera), pointer))
        .map(|handle| handle.preset);
    if let Some(preset) = hovered {
        if response.clicked() {
            camera.set_view(preset);
        }
        response.on_hover_text(preset.description(camera.axes));
    }

    // Placed again so the picture matches the camera after any click or drag.
    let mut handles = place_handles(rect, camera);
    handles.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    paint(ui.painter(), rect, &handles, hovered);
}

fn place_handles(rect: Rect, camera: &OrbitCamera) -> Vec<Placed> {
    // Only the rotation matters, so transform directions, not points.
    let view = camera.view_matrix();
    HANDLES
        .iter()
        .map(|handle| {
            let direction = view.transform_vector3(camera.axes.to_world(handle.axis));
            Placed {
                handle,
                // View-space Y is up, screen Y is down.
                pos: rect.center() + egui::vec2(direction.x, -direction.y) * ARM_LENGTH,
                depth: direction.z,
            }
        })
        .collect()
}

/// The handle under the pointer. Where two overlap, the one in front wins.
fn handle_at(handles: &[Placed], pointer: Pos2) -> Option<&'static Handle> {
    handles
        .iter()
        .filter(|placed| placed.pos.distance(pointer) <= HANDLE_RADIUS)
        .max_by(|a, b| a.depth.total_cmp(&b.depth))
        .map(|placed| placed.handle)
}

fn paint(painter: &egui::Painter, rect: Rect, handles: &[Placed], hovered: Option<ViewPreset>) {
    let centre = rect.center();
    painter.circle_filled(centre, SIZE * 0.5, Color32::from_black_alpha(90));

    for placed in handles {
        let Placed { handle, pos, .. } = placed;
        let is_hovered = hovered == Some(handle.preset);
        match handle.label {
            Some(label) => {
                painter.line_segment([centre, *pos], Stroke::new(2.0, handle.color));
                painter.circle_filled(*pos, HANDLE_RADIUS, handle.color);
                painter.text(
                    *pos,
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(12.0),
                    Color32::BLACK,
                );
            }
            None => {
                painter.circle_filled(*pos, HANDLE_RADIUS - 2.0, handle.color.gamma_multiply(0.35));
                painter.circle_stroke(*pos, HANDLE_RADIUS - 2.0, Stroke::new(1.5, handle.color));
            }
        }
        if is_hovered {
            painter.circle_stroke(*pos, HANDLE_RADIUS + 1.5, Stroke::new(2.0, Color32::WHITE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gizmo_rect() -> Rect {
        Rect::from_min_size(Pos2::ZERO, egui::vec2(SIZE, SIZE))
    }

    #[test]
    fn top_view_should_put_y_handle_above_centre() {
        let mut camera = OrbitCamera::default();
        camera.set_view(ViewPreset::Top);
        let handles = place_handles(gizmo_rect(), &camera);
        let y = handles.iter().find(|p| p.handle.axis == Vec3::Y).unwrap();
        assert!(y.pos.y < gizmo_rect().center().y, "{:?}", y.pos);
    }

    #[test]
    fn transect_front_view_should_put_x_handle_right_of_centre() {
        // A transect heading south-west, far from the world axes.
        let mut camera = OrbitCamera::default();
        camera.axes = crate::camera::Axes::Transect { heading: -2.5 };
        camera.set_view(ViewPreset::Front);
        let handles = place_handles(gizmo_rect(), &camera);
        let x = handles.iter().find(|p| p.handle.axis == Vec3::X).unwrap();
        let centre = gizmo_rect().center();
        assert!(x.pos.x > centre.x + 1.0 && (x.pos.y - centre.y).abs() < 1e-3, "{:?}", x.pos);
    }

    #[test]
    fn handle_at_should_prefer_the_handle_in_front() {
        // Looking straight down, +Z and -Z both sit at the centre.
        let mut camera = OrbitCamera::default();
        camera.set_view(ViewPreset::Top);
        let handles = place_handles(gizmo_rect(), &camera);
        let picked = handle_at(&handles, gizmo_rect().center()).map(|h| h.preset);
        assert_eq!(picked, Some(ViewPreset::Top));
    }
}
