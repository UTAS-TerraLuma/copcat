//! The top-down map in transect mode. It shows the whole cloud with the
//! transect drawn over it, and is where the transect is drawn and edited.

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, Sense, Stroke};
use glam::{Mat4, Vec2, Vec3};

use crate::camera::{OrbitCamera, Projection, ViewPreset};
use crate::lod::LodSettings;
use crate::transect::{self, Transect};

/// The map shows the whole site, so coarse detail is plenty.
pub const LOD: LodSettings = LodSettings {
    point_budget: 1_000_000,
    target_spacing_px: 2.0,
};

const HANDLE_RADIUS: f32 = 7.0;
/// Width handles sit at least this far (in logical pixels) from the line, so
/// they can be grabbed even when the transect is thinner than a pixel.
const MIN_WIDTH_HANDLE_OFFSET: f32 = 14.0;
/// Empty space around the cloud when the map is fitted to it.
const FIT_MARGIN: f32 = 1.05;
const TRANSECT_COLOR: Color32 = Color32::from_rgb(255, 210, 80);

/// The map's state that lasts between frames.
pub struct TransectMap {
    camera: OrbitCamera,
    /// The cloud's local bounds, to fit the map to.
    bounds: (Vec3, Vec3),
    /// Fit the map to the cloud next frame, once its size is known.
    needs_fit: bool,
    drag: Option<Drag>,
}

/// The part of the transect under the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    Start,
    End,
    /// A width handle: +1 on the side `Transect::normal` points to, -1 opposite.
    Width { side: f32 },
    Body,
}

/// A left-drag in progress on the map.
#[derive(Clone, Copy, Debug)]
enum Drag {
    Start,
    End,
    /// Drawing a new transect: the end follows the pointer.
    Draw,
    Width { side: f32, last: Vec2 },
    Move { last: Vec2 },
}

/// What the map did to the transect this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Change {
    None,
    /// The line moved or turned, so the 3D view should re-aim at it.
    Line,
    /// The whole transect slid by this much, without turning.
    Moved(Vec2),
    Width,
}

/// Where the map was laid out this frame, for painting it once it's rendered.
pub struct MapFrame {
    painter: egui::Painter,
    projection: MapProjection,
    /// Physical pixels to render at.
    pub size_px: [u32; 2],
    hovered: Option<Grab>,
}

impl TransectMap {
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
    pub fn show(&mut self, ui: &mut egui::Ui, transect: &mut Transect) -> (MapFrame, Change) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        if self.needs_fit || response.double_clicked() {
            self.fit(rect.aspect_ratio());
        }
        self.pan_and_zoom(ui, &response, rect);

        let projection = MapProjection::new(&self.camera, rect);
        let change = self.edit(ui, &response, &projection, transect);

        let hovered = match self.drag {
            // While dragging, keep showing what's being dragged.
            Some(Drag::Start) => Some(Grab::Start),
            Some(Drag::End | Drag::Draw) => Some(Grab::End),
            Some(Drag::Width { side, .. }) => Some(Grab::Width { side }),
            Some(Drag::Move { .. }) => Some(Grab::Body),
            None => response
                .hover_pos()
                .and_then(|pos| grab_at(&projection, transect, pos)),
        };
        response.on_hover_cursor(match hovered {
            Some(Grab::Start | Grab::End) => CursorIcon::Grab,
            Some(Grab::Width { .. }) => CursorIcon::ResizeColumn,
            Some(Grab::Body) => CursorIcon::Move,
            None => CursorIcon::Crosshair,
        });

        let pixels_per_point = ui.ctx().pixels_per_point();
        let frame = MapFrame {
            painter: ui.painter().with_clip_rect(rect),
            size_px: [rect.width(), rect.height()]
                .map(|side| (side * pixels_per_point).round() as u32),
            projection,
            hovered,
        };
        (frame, change)
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
        transect: &mut Transect,
    ) -> Change {
        let mut change = Change::None;
        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(origin) = ui.input(|i| i.pointer.press_origin())
        {
            // egui only calls it a drag once the pointer has moved a little,
            // so pick what to drag from where the button went down.
            let at = projection.to_world(origin);
            self.drag = Some(match grab_at(projection, transect, origin) {
                Some(Grab::Start) => Drag::Start,
                Some(Grab::End) => Drag::End,
                Some(Grab::Width { side }) => Drag::Width { side, last: at },
                Some(Grab::Body) => Drag::Move { last: at },
                None => {
                    transect.start = at;
                    transect.end = at;
                    change = Change::Line;
                    Drag::Draw
                }
            });
        }

        if response.dragged_by(egui::PointerButton::Primary)
            && let (Some(pointer), Some(drag)) = (response.interact_pointer_pos(), &mut self.drag)
        {
            let at = projection.to_world(pointer);
            change = match drag {
                Drag::Start => {
                    transect.start = at;
                    Change::Line
                }
                Drag::End | Drag::Draw => {
                    transect.end = at;
                    Change::Line
                }
                Drag::Width { side, last } => {
                    // Relative, so grabbing a handle drawn away from a thin
                    // transect doesn't jump the width to the pointer.
                    let outward = transect.normal() * *side;
                    transect.width = (transect.width + 2.0 * outward.dot(at - *last))
                        .max(transect::MIN_WIDTH);
                    *last = at;
                    Change::Width
                }
                Drag::Move { last } => {
                    let offset = at - *last;
                    transect.start += offset;
                    transect.end += offset;
                    *last = at;
                    Change::Moved(offset)
                }
            };
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        change
    }

    /// Paints the rendered map, then the transect on top.
    pub fn paint(&self, frame: &MapFrame, texture_id: egui::TextureId, transect: &Transect) {
        let painter = &frame.painter;
        let projection = &frame.projection;
        let full_uv = Rect::from_min_max(Pos2::ZERO, egui::pos2(1.0, 1.0));
        painter.image(texture_id, projection.rect, full_uv, Color32::WHITE);

        let outline = transect.corners().map(|corner| projection.to_screen(corner));
        painter.add(egui::Shape::convex_polygon(
            outline.to_vec(),
            TRANSECT_COLOR.gamma_multiply(0.25),
            Stroke::new(1.5, TRANSECT_COLOR),
        ));
        let start = projection.to_screen(transect.start);
        let end = projection.to_screen(transect.end);
        painter.line_segment([start, end], Stroke::new(1.0, TRANSECT_COLOR));

        let highlight = |grab: Grab| frame.hovered == Some(grab);
        for side in [1.0, -1.0] {
            let pos = width_handle(projection, transect, side);
            let color = if highlight(Grab::Width { side }) {
                Color32::WHITE
            } else {
                TRANSECT_COLOR
            };
            painter.rect_filled(Rect::from_center_size(pos, egui::vec2(8.0, 8.0)), 1.0, color);
            painter.rect_stroke(
                Rect::from_center_size(pos, egui::vec2(8.0, 8.0)),
                1.0,
                Stroke::new(1.0, Color32::BLACK),
                egui::StrokeKind::Outside,
            );
        }
        for (pos, label, grab) in [(start, "A", Grab::Start), (end, "B", Grab::End)] {
            let fill = if highlight(grab) {
                Color32::WHITE
            } else {
                TRANSECT_COLOR
            };
            painter.circle(pos, HANDLE_RADIUS, fill, Stroke::new(1.0, Color32::BLACK));
            painter.text(
                pos,
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::proportional(11.0),
                Color32::BLACK,
            );
        }
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
}

/// Where a width handle is drawn: on the slab's edge, or further out when the
/// slab is too thin on screen to grab.
fn width_handle(projection: &MapProjection, transect: &Transect, side: f32) -> Pos2 {
    let middle = projection.to_screen(transect.midpoint());
    let outward = projection.to_screen(transect.midpoint() + transect.normal() * side) - middle;
    // `outward` is one metre on screen, so its length is the map's scale.
    let offset = (outward.length() * transect.width * 0.5).max(MIN_WIDTH_HANDLE_OFFSET);
    middle + outward.normalized() * offset
}

/// What's under the pointer at `pos`, with handles winning over the body.
fn grab_at(projection: &MapProjection, transect: &Transect, pos: Pos2) -> Option<Grab> {
    let near = |handle: Pos2| handle.distance(pos) <= HANDLE_RADIUS;
    let start = projection.to_screen(transect.start);
    let end = projection.to_screen(transect.end);
    if near(start) || near(end) {
        // Prefer the closer end when both are under the pointer.
        return Some(if start.distance(pos) <= end.distance(pos) {
            Grab::Start
        } else {
            Grab::End
        });
    }
    for side in [1.0, -1.0] {
        if near(width_handle(projection, transect, side)) {
            return Some(Grab::Width { side });
        }
    }
    let on_line = distance_to_segment(pos, start, end) <= HANDLE_RADIUS * 0.5;
    (on_line || transect.contains(projection.to_world(pos))).then_some(Grab::Body)
}

fn distance_to_segment(pos: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((pos - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    pos.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 200 × 100 px map showing 400 × 200 m around the origin.
    fn projection() -> MapProjection {
        let mut map = TransectMap::new((Vec3::new(-200.0, -100.0, 0.0), Vec3::new(200.0, 100.0, 10.0)));
        map.fit(2.0);
        map.camera.set_half_height(100.0);
        let rect = Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 100.0));
        MapProjection::new(&map.camera, rect)
    }

    fn transect() -> Transect {
        Transect {
            start: Vec2::new(-100.0, 0.0),
            end: Vec2::new(100.0, 0.0),
            width: 2.0,
        }
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
    fn grab_at_should_find_the_ends() {
        let projection = projection();
        let start = projection.to_screen(transect().start);
        assert_eq!(grab_at(&projection, &transect(), start), Some(Grab::Start));
    }

    #[test]
    fn grab_at_should_find_width_handles_off_a_thin_transect() {
        let projection = projection();
        // 2 m is 1 px wide here, so the handle sits the minimum offset out.
        let handle = width_handle(&projection, &transect(), 1.0);
        let middle = projection.to_screen(Vec2::ZERO);
        assert!((handle.distance(middle) - MIN_WIDTH_HANDLE_OFFSET).abs() < 1e-3);
        assert_eq!(
            grab_at(&projection, &transect(), handle),
            Some(Grab::Width { side: 1.0 })
        );
    }

    #[test]
    fn grab_at_should_miss_open_ground() {
        let projection = projection();
        let away = projection.to_screen(Vec2::new(50.0, 60.0));
        assert_eq!(grab_at(&projection, &transect(), away), None);
    }
}
