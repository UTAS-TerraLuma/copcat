//! The top-down map in Subcloud mode. It shows the whole cloud with the
//! subcloud's shape drawn over it, and is where the shape is drawn and edited.

use std::f32::consts::{FRAC_PI_2, TAU};

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke};
use glam::{Mat4, Vec2, Vec3};

use crate::camera::{OrbitCamera, Projection, ViewPreset};
use crate::lod::LodSettings;
use crate::subcloud::{self, ShapeKind, Subcloud};

/// The map shows the whole site, so coarse detail is plenty.
pub const LOD: LodSettings = LodSettings {
    point_budget: 1_000_000,
    target_spacing_px: 2.0,
};

/// The colour subclouds are drawn in, on the map and in the density profile.
pub const SUBCLOUD_COLOR: Color32 = Color32::from_rgb(255, 210, 80);

const HANDLE_RADIUS: f32 = 7.0;
/// Size handles sit at least this far (in logical pixels) from where they
/// measure from, so they can be grabbed even when the shape is smaller than
/// a pixel.
const MIN_HANDLE_OFFSET: f32 = 14.0;
/// How far the square's rotate handle sticks out past its side, in logical pixels.
const ROTATE_STEM: f32 = 22.0;
/// With Shift held, rotating snaps to multiples of this.
const ROTATE_SNAP: f32 = 15.0 * TAU / 360.0;
/// Empty space around the cloud when the map is fitted to it.
const FIT_MARGIN: f32 = 1.05;

/// The map's state that lasts between frames.
pub struct SubcloudMap {
    camera: OrbitCamera,
    /// The cloud's local bounds, to fit the map to.
    bounds: (Vec3, Vec3),
    /// Fit the map to the cloud next frame, once its size is known.
    needs_fit: bool,
    drag: Option<Drag>,
}

/// The part of the shape under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// A line's ends.
    Start,
    End,
    /// A line's width handle: +1 on the side `Line::normal` points to, -1 opposite.
    Width { side: f32 },
    /// A circle's radius handle or a square's side handle.
    Size,
    /// The round handle that turns the shape and points forward.
    Rotate,
    Body,
}

/// A left-drag in progress on the map.
#[derive(Clone, Copy, Debug)]
enum Drag {
    Start,
    /// Also used while drawing a new line, which ends at the pointer.
    End,
    /// Drawing a new circle or square, sized so its edge is at the pointer.
    Draw,
    Width { side: f32, last: Vec2 },
    Size { last: Vec2 },
    Rotate,
    Move { last: Vec2 },
}

/// Where the map was laid out this frame, for painting it once it's rendered.
pub struct MapFrame {
    painter: egui::Painter,
    projection: MapProjection,
    /// Physical pixels to render at.
    pub size_px: [u32; 2],
    hovered: Option<Grab>,
}

impl SubcloudMap {
    pub fn new(bounds: (Vec3, Vec3)) -> Self {
        let mut camera = OrbitCamera::default();
        camera.set_projection(Projection::Orthographic);
        camera.set_view(ViewPreset::Top);
        Self {
            camera,
            bounds,
            needs_fit: true,
            drag: None,
        }
    }

    pub fn camera(&self) -> &OrbitCamera {
        &self.camera
    }

    /// Lays out the map in the rest of `ui` and handles its input.
    pub fn show(&mut self, ui: &mut egui::Ui, subcloud: &mut Subcloud) -> MapFrame {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if self.needs_fit || response.double_clicked() {
            self.fit(rect.aspect_ratio());
        }
        self.pan_and_zoom(ui, &response, rect);

        let projection = MapProjection::new(&self.camera, rect);
        self.edit(ui, &response, &projection, subcloud);

        let hovered = match self.drag {
            // While dragging, keep showing what's being dragged.
            Some(Drag::Start) => Some(Grab::Start),
            Some(Drag::End) => Some(Grab::End),
            Some(Drag::Width { side, .. }) => Some(Grab::Width { side }),
            Some(Drag::Draw | Drag::Size { .. }) => Some(Grab::Size),
            Some(Drag::Rotate) => Some(Grab::Rotate),
            Some(Drag::Move { .. }) => Some(Grab::Body),
            None => response
                .hover_pos()
                .and_then(|pos| grab_at(&projection, subcloud, pos)),
        };
        response.on_hover_cursor(match hovered {
            Some(Grab::Start | Grab::End | Grab::Rotate) => CursorIcon::Grab,
            Some(Grab::Width { .. }) => CursorIcon::ResizeColumn,
            Some(Grab::Size) => CursorIcon::ResizeHorizontal,
            Some(Grab::Body) => CursorIcon::Move,
            None => CursorIcon::Crosshair,
        });

        let pixels_per_point = ui.ctx().pixels_per_point();
        MapFrame {
            painter: ui.painter().with_clip_rect(rect),
            size_px: [rect.width(), rect.height()]
                .map(|side| (side * pixels_per_point).round() as u32),
            projection,
            hovered,
        }
    }

    /// Zooms out to show the whole cloud in a map of `aspect` (width / height).
    fn fit(&mut self, aspect: f32) {
        let (min, max) = self.bounds;
        self.camera.frame_bounds(min, max);
        let extent = (max - min).truncate();
        let half_height = 0.5 * extent.y.max(extent.x / aspect.max(1e-3));
        self.camera.set_half_height(half_height * FIT_MARGIN);
        self.needs_fit = false;
    }

    fn pan_and_zoom(&mut self, ui: &egui::Ui, response: &egui::Response, rect: Rect) {
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            let drag = response.drag_delta();
            self.camera.pan(glam::vec2(drag.x, drag.y), rect.height());
        }
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if let Some(pointer) = response.hover_pos()
            && scroll != 0.0
        {
            // Keep the ground under the pointer still while zooming, as in
            // web maps, rather than zooming into the map's centre.
            let before = MapProjection::new(&self.camera, rect).to_world(pointer);
            self.camera.zoom(scroll);
            let after = MapProjection::new(&self.camera, rect).to_world(pointer);
            self.camera.target += (before - after).extend(0.0);
        }
    }

    fn edit(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        projection: &MapProjection,
        subcloud: &mut Subcloud,
    ) {
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            // egui only calls it a drag once the pointer has moved a little,
            // so pick what to drag from where the button went down.
            let at = projection.to_world(origin);
            self.drag = Some(match grab_at(projection, subcloud, origin) {
                Some(Grab::Start) => Drag::Start,
                Some(Grab::End) => Drag::End,
                Some(Grab::Width { side }) => Drag::Width { side, last: at },
                Some(Grab::Size) => Drag::Size { last: at },
                Some(Grab::Rotate) => Drag::Rotate,
                Some(Grab::Body) => Drag::Move { last: at },
                None => start_drawing(subcloud, at),
            });
        }

        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(pointer), Some(drag)) = (response.interact_pointer_pos(), &mut self.drag)
        {
            let at = projection.to_world(pointer);
            match drag {
                Drag::Start => subcloud.line.start = at,
                Drag::End => subcloud.line.end = at,
                Drag::Draw => draw_to(subcloud, at),
                // The handle drags below are relative, so grabbing a handle
                // drawn away from a tiny shape doesn't jump it to the pointer.
                Drag::Width { side, last } => {
                    let line = &mut subcloud.line;
                    let outward = line.normal() * *side;
                    line.width =
                        (line.width + 2.0 * outward.dot(at - *last)).max(subcloud::MIN_SIZE);
                    *last = at;
                }
                Drag::Size { last } => {
                    resize_by(subcloud, at - *last);
                    *last = at;
                }
                Drag::Rotate => {
                    let snap = ui.input(|i| i.modifiers.shift);
                    rotate_to(subcloud, at, snap);
                }
                Drag::Move { last } => {
                    subcloud.translate(at - *last);
                    *last = at;
                }
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
    }

    /// Paints the rendered map, then the shape on top.
    pub fn paint(&self, frame: &MapFrame, texture_id: egui::TextureId, subcloud: &Subcloud) {
        let painter = &frame.painter;
        let projection = &frame.projection;
        let full_uv = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(texture_id, projection.rect, full_uv, Color32::WHITE);

        let fill = SUBCLOUD_COLOR.gamma_multiply(0.25);
        let outline = Stroke::new(1.5, SUBCLOUD_COLOR);
        let thin = Stroke::new(1.0, SUBCLOUD_COLOR);
        let highlight = |grab: Grab| frame.hovered == Some(grab);
        // Under the other handles: for a line, the stem starts at a width handle.
        let paint_rotate_handle = || {
            let (stem_base, rotate) = rotate_handle(projection, subcloud);
            painter.line_segment([stem_base, rotate], thin);
            paint_round_handle(painter, rotate, highlight(Grab::Rotate));
        };
        match subcloud.shape {
            ShapeKind::Line => {
                let line = &subcloud.line;
                let corners = line.corners().map(|corner| projection.to_screen(corner));
                painter.add(egui::Shape::convex_polygon(corners.to_vec(), fill, outline));
                let start = projection.to_screen(line.start);
                let end = projection.to_screen(line.end);
                painter.line_segment([start, end], thin);
                paint_rotate_handle();
                for side in [1.0, -1.0] {
                    let pos = width_handle(projection, subcloud, side);
                    paint_size_handle(painter, pos, highlight(Grab::Width { side }));
                }
                for (pos, label, grab) in [(start, "A", Grab::Start), (end, "B", Grab::End)] {
                    paint_round_handle(painter, pos, highlight(grab));
                    painter.text(
                        pos,
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(11.0),
                        Color32::BLACK,
                    );
                }
            }
            ShapeKind::Circle => {
                let circle = &subcloud.circle;
                let centre = projection.to_screen(circle.centre);
                let radius_px = projection.to_screen(circle.centre + Vec2::X * circle.radius).x
                    - centre.x;
                painter.circle(centre, radius_px, fill, outline);
                painter.circle_filled(centre, 2.0, SUBCLOUD_COLOR);
                paint_rotate_handle();
                let handle = size_handle(projection, subcloud);
                paint_size_handle(painter, handle, highlight(Grab::Size));
            }
            ShapeKind::Square => {
                let square = &subcloud.square;
                let corners = square.corners().map(|corner| projection.to_screen(corner));
                painter.add(egui::Shape::convex_polygon(corners.to_vec(), fill, outline));
                painter.circle_filled(projection.to_screen(square.centre), 2.0, SUBCLOUD_COLOR);
                paint_rotate_handle();
                let handle = size_handle(projection, subcloud);
                paint_size_handle(painter, handle, highlight(Grab::Size));
            }
        }
    }
}

/// Starts a new shape where a drag began on open ground, and returns the
/// drag that sizes it.
fn start_drawing(subcloud: &mut Subcloud, at: Vec2) -> Drag {
    match subcloud.shape {
        ShapeKind::Line => {
            subcloud.line.start = at;
            subcloud.line.end = at;
            Drag::End
        }
        ShapeKind::Circle => {
            subcloud.circle.centre = at;
            subcloud.circle.radius = subcloud::MIN_SIZE;
            Drag::Draw
        }
        ShapeKind::Square => {
            subcloud.square.centre = at;
            subcloud.square.side = subcloud::MIN_SIZE;
            Drag::Draw
        }
    }
}

/// Sizes a new circle or square so its edge is at `at`. A square keeps its
/// rotation.
fn draw_to(subcloud: &mut Subcloud, at: Vec2) {
    match subcloud.shape {
        // Drawing a line moves its end instead.
        ShapeKind::Line => {}
        ShapeKind::Circle => {
            let circle = &mut subcloud.circle;
            circle.radius = circle.centre.distance(at).max(subcloud::MIN_SIZE);
        }
        ShapeKind::Square => {
            let square = &mut subcloud.square;
            square.side = square.side_through(at).max(subcloud::MIN_SIZE);
        }
    }
}

/// Grows a circle or square as its size handle moves by `delta`.
fn resize_by(subcloud: &mut Subcloud, delta: Vec2) {
    match subcloud.shape {
        // Lines have width handles instead.
        ShapeKind::Line => {}
        // The handle is on the right, 90° clockwise from the rotate handle.
        ShapeKind::Circle => {
            let outward = delta.dot(subcloud.along());
            let circle = &mut subcloud.circle;
            circle.radius = (circle.radius + outward).max(subcloud::MIN_SIZE);
        }
        // The handle is on the first side, and the opposite side moves too.
        ShapeKind::Square => {
            let square = &mut subcloud.square;
            square.side = (square.side + 2.0 * delta.dot(square.along())).max(subcloud::MIN_SIZE);
        }
    }
}

/// Turns the shape so its rotate handle (on its `across` side) points at
/// `at`, to the nearest [`ROTATE_SNAP`] if `snap`.
fn rotate_to(subcloud: &mut Subcloud, at: Vec2, snap: bool) {
    let outward = at - subcloud.centre();
    // Right on the centre the direction means nothing, so keep the last one.
    if outward.length() <= subcloud::MIN_SIZE {
        return;
    }
    let rotation = outward.to_angle() - FRAC_PI_2;
    let rotation = if snap {
        (rotation / ROTATE_SNAP).round() * ROTATE_SNAP
    } else {
        rotation
    };
    // 0..360° reads better in the rotation field than -180..180°.
    subcloud.set_rotation(rotation.rem_euclid(TAU));
}

fn paint_size_handle(painter: &egui::Painter, pos: Pos2, highlighted: bool) {
    let rect = Rect::from_center_size(pos, egui::vec2(8.0, 8.0));
    painter.rect_filled(rect, 1.0, handle_color(highlighted));
    painter.rect_stroke(
        rect,
        1.0,
        Stroke::new(1.0, Color32::BLACK),
        egui::StrokeKind::Outside,
    );
}

fn paint_round_handle(painter: &egui::Painter, pos: Pos2, highlighted: bool) {
    let stroke = Stroke::new(1.0, Color32::BLACK);
    painter.circle(pos, HANDLE_RADIUS, handle_color(highlighted), stroke);
}

fn handle_color(highlighted: bool) -> Color32 {
    if highlighted {
        Color32::WHITE
    } else {
        SUBCLOUD_COLOR
    }
}

/// Converts between map pixels and ground coordinates for one frame.
struct MapProjection {
    rect: Rect,
    world_to_clip: Mat4,
    clip_to_world: Mat4,
}

impl MapProjection {
    fn new(camera: &OrbitCamera, rect: Rect) -> Self {
        let world_to_clip = camera.view_projection(rect.aspect_ratio());
        Self {
            rect,
            world_to_clip,
            clip_to_world: world_to_clip.inverse(),
        }
    }

    fn to_screen(&self, ground: Vec2) -> Pos2 {
        let ndc = self.world_to_clip.project_point3(ground.extend(0.0));
        // NDC Y is up, screen Y is down.
        self.rect.min + egui::vec2(ndc.x + 1.0, 1.0 - ndc.y) * 0.5 * self.rect.size()
    }

    fn to_world(&self, screen: Pos2) -> Vec2 {
        let uv = (screen - self.rect.min) / self.rect.size();
        let ndc = Vec3::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.5);
        self.clip_to_world.project_point3(ndc).truncate()
    }

    /// The screen point `metres` from `origin` towards `direction` (a unit
    /// vector), or further out when that's too close to `origin` to grab.
    fn handle(&self, origin: Vec2, direction: Vec2, metres: f32) -> Pos2 {
        let from = self.to_screen(origin);
        // One metre on screen, so its length is the map's scale.
        let outward = self.to_screen(origin + direction) - from;
        let offset = (outward.length() * metres).max(MIN_HANDLE_OFFSET);
        from + outward.normalized() * offset
    }
}

/// Where a line's width handle is drawn: on the slab's edge, or further out
/// when the slab is too thin on screen to grab.
fn width_handle(projection: &MapProjection, subcloud: &Subcloud, side: f32) -> Pos2 {
    let line = &subcloud.line;
    projection.handle(line.midpoint(), line.normal() * side, line.width * 0.5)
}

/// Where a circle's or square's size handle is drawn: on its right-hand
/// edge, 90° clockwise from the rotate handle.
fn size_handle(projection: &MapProjection, subcloud: &Subcloud) -> Pos2 {
    projection.handle(subcloud.centre(), subcloud.along(), reach(subcloud))
}

/// The rotate handle: a stem out from the shape's edge on its `across`
/// (forward) side, as `(stem base, handle)`.
fn rotate_handle(projection: &MapProjection, subcloud: &Subcloud) -> (Pos2, Pos2) {
    let centre = subcloud.centre();
    let base = projection.handle(centre, subcloud.across(), reach(subcloud));
    let outward = (base - projection.to_screen(centre)).normalized();
    (base, base + outward * ROTATE_STEM)
}

/// How far the shape reaches from its centre to its edge, both forward and
/// to the side for circles and squares, and forward for lines.
fn reach(subcloud: &Subcloud) -> f32 {
    let (depth, _) = subcloud.depth_and_length();
    depth * 0.5
}

/// What's under the pointer at `pos`, with handles winning over the body.
fn grab_at(projection: &MapProjection, subcloud: &Subcloud, pos: Pos2) -> Option<Grab> {
    let near = |handle: Pos2| handle.distance(pos) <= HANDLE_RADIUS;
    let inside = subcloud.footprint().contains(projection.to_world(pos));
    match subcloud.shape {
        ShapeKind::Line => {
            let start = projection.to_screen(subcloud.line.start);
            let end = projection.to_screen(subcloud.line.end);
            if near(start) || near(end) {
                // Prefer the closer end when both are under the pointer.
                return Some(if start.distance(pos) <= end.distance(pos) {
                    Grab::Start
                } else {
                    Grab::End
                });
            }
            if near(rotate_handle(projection, subcloud).1) {
                return Some(Grab::Rotate);
            }
            for side in [1.0, -1.0] {
                if near(width_handle(projection, subcloud, side)) {
                    return Some(Grab::Width { side });
                }
            }
            let on_line = distance_to_segment(pos, start, end) <= HANDLE_RADIUS * 0.5;
            (on_line || inside).then_some(Grab::Body)
        }
        ShapeKind::Circle | ShapeKind::Square => {
            if near(size_handle(projection, subcloud)) {
                return Some(Grab::Size);
            }
            if near(rotate_handle(projection, subcloud).1) {
                return Some(Grab::Rotate);
            }
            // The centre stays grabbable when the shape is tiny on screen.
            let centre = projection.to_screen(subcloud.centre());
            (inside || near(centre)).then_some(Grab::Body)
        }
    }
}

fn distance_to_segment(pos: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((pos - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    pos.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subcloud::Line;

    /// A 200 × 100 px map showing 400 × 200 m around the origin.
    fn projection() -> MapProjection {
        let mut map =
            SubcloudMap::new((Vec3::new(-200.0, -100.0, 0.0), Vec3::new(200.0, 100.0, 10.0)));
        map.fit(2.0);
        map.camera.set_half_height(100.0);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 100.0));
        MapProjection::new(&map.camera, rect)
    }

    fn subcloud(shape: ShapeKind) -> Subcloud {
        let mut subcloud = Subcloud::new(Vec3::splat(-100.0), Vec3::splat(100.0));
        subcloud.shape = shape;
        subcloud.line = Line {
            start: Vec2::new(-100.0, 0.0),
            end: Vec2::new(100.0, 0.0),
            width: 2.0,
        };
        subcloud.circle.radius = 20.0;
        subcloud.square.side = 80.0;
        subcloud
    }

    #[test]
    fn to_world_should_undo_to_screen() {
        let projection = projection();
        let ground = Vec2::new(-123.0, 45.0);
        let back = projection.to_world(projection.to_screen(ground));
        assert!(back.abs_diff_eq(ground, 1e-2), "{back}");
    }

    #[test]
    fn to_screen_should_put_north_up_and_east_right() {
        let projection = projection();
        let centre = projection.to_screen(Vec2::ZERO);
        assert!(projection.to_screen(Vec2::new(0.0, 50.0)).y < centre.y);
        assert!(projection.to_screen(Vec2::new(50.0, 0.0)).x > centre.x);
    }

    #[test]
    fn grab_at_should_find_the_line_ends() {
        let projection = projection();
        let line = subcloud(ShapeKind::Line);
        let start = projection.to_screen(line.line.start);
        assert_eq!(grab_at(&projection, &line, start), Some(Grab::Start));
    }

    #[test]
    fn grab_at_should_find_width_handles_off_a_thin_line() {
        let projection = projection();
        let line = subcloud(ShapeKind::Line);
        // 2 m is 1 px wide here, so the handle sits the minimum offset out.
        let handle = width_handle(&projection, &line, 1.0);
        let middle = projection.to_screen(Vec2::ZERO);
        assert!((handle.distance(middle) - MIN_HANDLE_OFFSET).abs() < 1e-3);
        assert_eq!(
            grab_at(&projection, &line, handle),
            Some(Grab::Width { side: 1.0 })
        );
    }

    #[test]
    fn grab_at_should_miss_open_ground() {
        let projection = projection();
        let away = projection.to_screen(Vec2::new(50.0, 60.0));
        assert_eq!(grab_at(&projection, &subcloud(ShapeKind::Line), away), None);
    }

    #[test]
    fn grab_at_should_find_the_circle_size_handle_on_its_east_edge() {
        let projection = projection();
        let east_edge = projection.to_screen(Vec2::new(20.0, 0.0));
        assert_eq!(
            grab_at(&projection, &subcloud(ShapeKind::Circle), east_edge),
            Some(Grab::Size)
        );
    }

    #[test]
    fn grab_at_should_grab_inside_a_circle_as_its_body() {
        let projection = projection();
        let inside = projection.to_screen(Vec2::new(0.0, 15.0));
        assert_eq!(
            grab_at(&projection, &subcloud(ShapeKind::Circle), inside),
            Some(Grab::Body)
        );
    }

    #[test]
    fn grab_at_should_find_the_square_rotate_handle_beyond_its_north_side() {
        let projection = projection();
        let square = subcloud(ShapeKind::Square);
        // Unturned, `across` is north: the side is 40 m (20 px) up, and the
        // handle a stem further.
        let handle = projection.to_screen(Vec2::ZERO) - egui::vec2(0.0, 20.0 + ROTATE_STEM);
        assert_eq!(grab_at(&projection, &square, handle), Some(Grab::Rotate));
    }

    #[test]
    fn resize_by_should_grow_a_square_without_turning_it() {
        let mut square = subcloud(ShapeKind::Square);
        resize_by(&mut square, Vec2::new(5.0, 3.0));
        assert_eq!((square.square.side, square.square.rotation), (90.0, 0.0));
    }

    #[test]
    fn grab_at_should_find_the_line_rotate_handle_on_the_a_left_side() {
        let projection = projection();
        let line = subcloud(ShapeKind::Line);
        // A thin eastward line: the stem starts at the minimum offset north.
        let handle =
            projection.to_screen(Vec2::ZERO) - egui::vec2(0.0, MIN_HANDLE_OFFSET + ROTATE_STEM);
        assert_eq!(grab_at(&projection, &line, handle), Some(Grab::Rotate));
    }

    #[test]
    fn rotate_to_should_point_the_rotate_handle_at_the_pointer() {
        let mut square = subcloud(ShapeKind::Square);
        // The handle starts north; pointing it west is a quarter turn.
        rotate_to(&mut square, Vec2::new(-10.0, 0.0), false);
        let rotation = square.square.rotation;
        assert!((rotation - FRAC_PI_2).abs() < 1e-6, "{rotation}");
    }

    #[test]
    fn rotate_to_should_snap_with_shift() {
        let mut circle = subcloud(ShapeKind::Circle);
        // 50° from north-ish: snaps to the nearest 15°.
        let at = Vec2::from_angle(FRAC_PI_2 + 50_f32.to_radians());
        rotate_to(&mut circle, at, true);
        let degrees = circle.circle.rotation.to_degrees();
        assert!((degrees - 45.0).abs() < 1e-3, "{degrees}");
    }
}
