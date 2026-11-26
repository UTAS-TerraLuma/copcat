//! The subcloud tools: in Explore mode, pick a line, circle or square, then
//! drag on the cloud to draw it where you're looking. A line runs from where
//! the drag starts to where it ends; a circle or square is centred where it
//! starts with its edge where it ends (as drawing on the map works).
//!
//! The pointer is put on the ground by meeting its ray with a level plane
//! (see [`crate::pick`]), at the height of the lowest points around where
//! the drag started, and kept at that height for the whole drag.

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{self, CursorIcon, Key, Modifiers, Pos2, Stroke};
use glam::{Mat4, Vec2};

use crate::camera::OrbitCamera;
use crate::lod;
use crate::pick;
use crate::subcloud::{Footprint, ShapeKind, Subcloud};
use crate::theme::Theme;

use super::scene::ViewArea;
use super::session::Session;
use super::tool_strip::STRIP_RESERVED_WIDTH;

/// Points around the circle drawn as a preview.
const CIRCLE_SEGMENTS: usize = 64;

/// The subcloud tool picked in Explore mode, if any, and its drag.
#[derive(Debug, Default)]
pub(crate) struct SubcloudTool {
    picked: Option<ShapeKind>,
    /// A new line's width: the last subcloud's, so redrawing keeps it.
    line_width: f32,
    drag: Option<ToolDrag>,
}

/// A drag drawing a subcloud, on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ToolDrag {
    from: Vec2,
    to: Vec2,
    /// The height of the plane the pointer is met with.
    ground_z: f32,
    /// A circle's or square's turn: forward the way the camera looked when
    /// the drag started.
    rotation: f32,
}

impl SubcloudTool {
    /// Draws `shape` on the next drag, with `line_width` for a line.
    pub(crate) fn pick(&mut self, shape: ShapeKind, line_width: f32) {
        self.picked = Some(shape);
        self.line_width = line_width;
        self.drag = None;
    }

    pub(crate) fn cancel(&mut self) {
        self.picked = None;
        self.drag = None;
    }

    pub(crate) fn picked(&self) -> Option<ShapeKind> {
        self.picked
    }

    /// Whether a left-drag on the 3D view draws rather than orbits.
    pub(crate) fn is_active(&self) -> bool {
        self.picked.is_some()
    }

    /// Handles the pointer and Esc over the explore view. Returns the
    /// subcloud once a drag has drawn one, putting the tool down.
    pub(crate) fn interact(
        &mut self,
        ui: &egui::Ui,
        area: &ViewArea,
        camera: &OrbitCamera,
        session: &Session,
    ) -> Option<Subcloud> {
        let shape = self.picked?;
        if !ui.ctx().text_edit_focused()
            && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.cancel();
            return None;
        }
        let response = &area.response;
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        let view_projection = camera.view_projection(area.rect.aspect_ratio());
        let ray_at = |pos: Pos2| {
            let offset = pos - area.rect.min;
            let size = area.rect.size();
            let ndc = pick::to_ndc(Vec2::new(offset.x, offset.y), Vec2::new(size.x, size.y));
            pick::ray_through(view_projection, ndc)
        };

        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            // egui only calls it a drag once the pointer has moved a little,
            // so start from where the button went down. A press on the sky
            // draws nothing.
            self.drag = ray_at(origin)
                .and_then(|ray| ground_under(ray, camera, session))
                .map(|(at, ground_z)| ToolDrag {
                    from: at,
                    to: at,
                    ground_z,
                    rotation: forward_rotation(camera.yaw),
                });
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(pointer), Some(drag)) = (response.interact_pointer_pos(), &mut self.drag)
            // Past the horizon the end stays where it last met the ground.
            && let Some(at) = ray_at(pointer).and_then(|ray| pick::hit_level(ray, drag.ground_z))
        {
            drag.to = at;
        }
        if response.drag_stopped()
            && let Some(drag) = self.drag.take()
            && let Some(subcloud) = drag.subcloud(shape, self.line_width)
        {
            self.picked = None;
            return Some(subcloud);
        }
        None
    }

    /// Paints what the tool is doing over the explore view: a hint while
    /// it's picked, and the shape being drawn on the ground.
    pub(crate) fn paint(
        &self,
        ui: &egui::Ui,
        area: &ViewArea,
        camera: &OrbitCamera,
        theme: &Theme,
    ) {
        let Some(shape) = self.picked else { return };
        let painter = ui.painter().with_clip_rect(area.rect);
        let hint = match shape {
            ShapeKind::Line => "Drag across the cloud to draw a line: from A to B",
            ShapeKind::Circle => "Drag on the cloud to draw a circle: from its centre to its edge",
            ShapeKind::Square => "Drag on the cloud to draw a square: from its centre to its edge",
        };
        let font = egui::TextStyle::Body.resolve(ui.style());
        // Right of the tools' strip.
        let text_pos = area.rect.left_top() + egui::vec2(STRIP_RESERVED_WIDTH + 12.0, 12.0);
        let galley = painter.layout_no_wrap(
            format!("{hint} · Esc: cancel"),
            font.clone(),
            theme.colors.text,
        );
        let backing = egui::Rect::from_min_size(text_pos, galley.size()).expand(6.0);
        painter.rect_filled(backing, 4.0, theme.colors.overlay);
        painter.galley(text_pos, galley, theme.colors.text);

        let Some(drag) = self.drag else { return };
        let Some(subcloud) = drag.subcloud(shape, self.line_width) else {
            return;
        };
        let view_projection = camera.view_projection(area.rect.aspect_ratio());
        let to_screen = |ground: Vec2| to_screen(view_projection, area.rect, ground, drag.ground_z);
        let outline: Vec<Option<Pos2>> = outline(&subcloud).into_iter().map(to_screen).collect();
        let color = theme.colors.subcloud;
        let stroke = Stroke::new(2.0, color);
        if let Some(points) = outline.iter().copied().collect::<Option<Vec<_>>>() {
            painter.add(egui::Shape::convex_polygon(
                points,
                color.gamma_multiply(0.2),
                stroke,
            ));
        } else {
            // Part of it is behind the camera: draw the edges in front.
            for pair in outline.windows(2) {
                if let [Some(a), Some(b)] = pair {
                    painter.line_segment([*a, *b], stroke);
                }
            }
        }
        if let (Some(from), Some(to)) = (to_screen(drag.from), to_screen(drag.to)) {
            painter.line_segment([from, to], Stroke::new(1.0, color));
            painter.circle_filled(from, 3.0, color);
            let size = match shape {
                ShapeKind::Line => format!("{:.1} m", subcloud.line.length()),
                ShapeKind::Circle => format!("radius {:.1} m", subcloud.circle.radius),
                ShapeKind::Square => format!("side {:.1} m", subcloud.square.side),
            };
            let galley = painter.layout_no_wrap(size, font, theme.colors.subcloud_ink);
            let at = to + egui::vec2(10.0, 10.0);
            let backing = egui::Rect::from_min_size(at, galley.size()).expand(3.0);
            painter.rect_filled(backing, 3.0, color);
            painter.galley(at, galley, theme.colors.subcloud_ink);
        }
    }
}

impl ToolDrag {
    fn subcloud(&self, shape: ShapeKind, line_width: f32) -> Option<Subcloud> {
        Subcloud::drawn(shape, self.from, self.to, self.rotation, line_width)
    }
}

/// The turn (see [`Subcloud::rotation`]) that points a circle's or square's
/// forward the way a camera at `yaw` looks, so the subcloud opens looking
/// the same way.
pub(crate) fn forward_rotation(yaw: f32) -> f32 {
    // The eye is at `yaw` from the target, so it looks the opposite way,
    // and forward is 90° anticlockwise from the shape's rotation.
    (yaw + FRAC_PI_2).rem_euclid(TAU)
}

/// The ground under `ray`: on the lowest points around where it meets the
/// cloud's bottom, or the cloud's bottom where nothing has loaded there.
fn ground_under(ray: pick::Ray, camera: &OrbitCamera, session: &Session) -> Option<(Vec2, f32)> {
    let octree = &session.octree;
    let (min, _) = octree.bounds();
    let known = octree.known_heights(session.shown);
    // A small patch for the view: big enough to have nodes with heights.
    let radius = (camera.half_height() * 0.05).max(0.5);
    pick::ground_under(ray, min.z, |centre| {
        let around = Footprint::Circle { centre, radius };
        lod::z_range_under(octree, &around, known).map(|(low, _)| low)
    })
}

/// The shape's outline on the ground, closed (the first point repeated).
fn outline(subcloud: &Subcloud) -> Vec<Vec2> {
    let mut points = match subcloud.shape {
        ShapeKind::Line => subcloud.line.corners().to_vec(),
        ShapeKind::Square => subcloud.square.corners().to_vec(),
        ShapeKind::Circle => {
            let circle = &subcloud.circle;
            (0..CIRCLE_SEGMENTS)
                .map(|i| {
                    let angle = i as f32 / CIRCLE_SEGMENTS as f32 * TAU;
                    circle.centre + Vec2::from_angle(angle) * circle.radius
                })
                .collect()
        }
    };
    points.push(points[0]);
    points
}

/// Where `ground` at height `z` is in a view of `rect`, or `None` if it's
/// behind the camera.
fn to_screen(view_projection: Mat4, rect: egui::Rect, ground: Vec2, z: f32) -> Option<Pos2> {
    let clip = view_projection * ground.extend(z).extend(1.0);
    if clip.w <= 1e-6 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    // NDC Y is up, screen Y is down.
    Some(rect.min + egui::vec2(ndc.x + 1.0, 1.0 - ndc.y) * 0.5 * rect.size())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::ViewPreset;

    #[test]
    fn forward_rotation_should_face_north_from_the_south() {
        // The default camera is south of its target, looking north.
        let camera = OrbitCamera::default();
        let rotation = forward_rotation(camera.yaw);
        assert!(rotation.abs() < 1e-6, "{rotation}");
    }

    #[test]
    fn drawn_circle_should_look_the_way_the_camera_looked() {
        // From the west, looking east.
        let mut camera = OrbitCamera::default();
        camera.set_view(ViewPreset::Left);
        let drag = ToolDrag {
            from: Vec2::ZERO,
            to: Vec2::new(0.0, 4.0),
            ground_z: 0.0,
            rotation: forward_rotation(camera.yaw),
        };
        let subcloud = drag.subcloud(ShapeKind::Circle, 2.0).unwrap();
        assert!(
            subcloud.across().abs_diff_eq(Vec2::X, 1e-5),
            "{}",
            subcloud.across()
        );
        assert_eq!(subcloud.circle.radius, 4.0);
    }

    #[test]
    fn to_screen_should_put_the_target_in_the_middle() {
        let camera = OrbitCamera::default();
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(200.0, 100.0));
        let at = to_screen(camera.view_projection(2.0), rect, Vec2::ZERO, 0.0).unwrap();
        assert!(at.distance(rect.center()) < 1e-3, "{at:?}");
    }

    #[test]
    fn to_screen_should_skip_points_behind_the_camera() {
        // Looking north from 100 m south, level: 1 km south is behind.
        let mut camera = OrbitCamera::default();
        camera.pitch = 0.0;
        let rect = egui::Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0));
        let behind = to_screen(
            camera.view_projection(1.0),
            rect,
            Vec2::new(0.0, -1000.0),
            0.0,
        );
        assert_eq!(behind, None);
    }

    #[test]
    fn outline_should_close_a_circle() {
        let subcloud =
            Subcloud::drawn(ShapeKind::Circle, Vec2::ZERO, Vec2::new(2.0, 0.0), 0.0, 2.0).unwrap();
        let points = outline(&subcloud);
        assert_eq!(points.len(), CIRCLE_SEGMENTS + 1);
        assert_eq!(points.first(), points.last());
        assert!(points.iter().all(|p| (p.length() - 2.0).abs() < 1e-4));
    }
}
