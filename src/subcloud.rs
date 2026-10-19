//! A subcloud: the part of the cloud above a shape on the ground (a line, a
//! circle or a square), for looking at a slice or plot on its own. Everything
//! here is pure geometry in local coordinates (relative to the octree centre),
//! so it's easy to test.

use glam::{Vec2, Vec3, Vec4};

use crate::camera::{Axes, OrbitCamera, ViewPreset};
use crate::lod;

pub const DEFAULT_WIDTH: f32 = 2.0;
pub const DEFAULT_RADIUS: f32 = 10.0;
/// Thinner or smaller than this and a subcloud holds too few points to be useful.
pub const MIN_SIZE: f32 = 0.05;
/// Ctrl+scroll slides the subcloud by this percentage of its thickness per
/// mouse wheel notch, unless the user changes it. 100% steps to the next
/// slice with no gap or overlap.
pub const DEFAULT_SWEEP_STEP_PERCENT: f32 = 100.0;
/// Extra room around the subcloud when the camera frames it.
const FRAMING_MARGIN: f32 = 1.1;

/// Which shape a subcloud is cut out with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    Line,
    Circle,
    Square,
}

impl ShapeKind {
    pub const ALL: [Self; 3] = [Self::Line, Self::Circle, Self::Square];

    pub fn label(self) -> &'static str {
        match self {
            Self::Line => "Line",
            Self::Circle => "Circle",
            Self::Square => "Square",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Line => "A thin slab along a line: a transect",
            Self::Circle => "A round plot: a centre and a radius",
            Self::Square => "A square plot: a centre, a radius and a rotation",
        }
    }
}

/// A vertical slab along a line on the ground: a transect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    /// Ends of the centre line on the ground (XY).
    pub start: Vec2,
    pub end: Vec2,
    /// Full width across the line, in metres.
    pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Circle {
    pub centre: Vec2,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Square {
    pub centre: Vec2,
    /// From the centre to the middle of each side: half the side length.
    pub radius: f32,
    /// Anticlockwise from east, in radians. The square's local X axis (see
    /// [`Square::along`]) points this way.
    pub rotation: f32,
}

/// A subcloud's outline on the ground. Points above or below it at any
/// height are inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Footprint {
    /// Inside all four vertical planes, each `(normal, d)` with points inside
    /// where `normal·p + d >= 0`.
    Planes([Vec4; 4]),
    Circle { centre: Vec2, radius: f32 },
}

impl Footprint {
    pub fn contains(&self, point: Vec2) -> bool {
        match *self {
            Self::Planes(planes) => planes
                .iter()
                .all(|plane| plane.dot(point.extend(0.0).extend(1.0)) >= 0.0),
            Self::Circle { centre, radius } => point.distance_squared(centre) <= radius * radius,
        }
    }

    /// Whether any of a box is above the footprint. Conservative for planes
    /// (see [`lod::box_touches_planes`]), exact for circles.
    pub fn touches_box(&self, min: Vec3, max: Vec3) -> bool {
        match *self {
            Self::Planes(planes) => lod::box_touches_planes(&planes, min, max),
            Self::Circle { centre, radius } => {
                // The point of the box's top-down rectangle nearest the centre.
                let nearest = centre.max(min.truncate()).min(max.truncate());
                nearest.distance_squared(centre) <= radius * radius
            }
        }
    }
}

/// The vertical plane through `point` facing `inward`, as `(normal, d)`.
fn plane(inward: Vec2, point: Vec2) -> Vec4 {
    inward.extend(0.0).extend(-inward.dot(point))
}

impl Line {
    pub fn length(&self) -> f32 {
        self.start.distance(self.end)
    }

    pub fn midpoint(&self) -> Vec2 {
        (self.start + self.end) * 0.5
    }

    /// Unit vector from start to end, or +X while the two coincide.
    pub fn direction(&self) -> Vec2 {
        (self.end - self.start).try_normalize().unwrap_or(Vec2::X)
    }

    /// Unit vector across the line, 90° anticlockwise from `direction`.
    pub fn normal(&self) -> Vec2 {
        self.direction().perp()
    }

    /// The four planes around the slab. There's no top or bottom: it runs
    /// the full height of the cloud.
    fn clip_planes(&self) -> [Vec4; 4] {
        let along = self.direction();
        let across = self.normal();
        let half_width = across * self.width * 0.5;
        [
            plane(across, self.start - half_width),
            plane(-across, self.start + half_width),
            plane(along, self.start),
            plane(-along, self.end),
        ]
    }

    /// The slab's outline on the ground, going around it.
    pub fn corners(&self) -> [Vec2; 4] {
        let half_width = self.normal() * self.width * 0.5;
        [
            self.start + half_width,
            self.end + half_width,
            self.end - half_width,
            self.start - half_width,
        ]
    }
}

impl Square {
    /// Unit vector from the centre towards the middle of the first side.
    pub fn along(&self) -> Vec2 {
        Vec2::from_angle(self.rotation)
    }

    /// Unit vector 90° anticlockwise from `along`.
    pub fn across(&self) -> Vec2 {
        self.along().perp()
    }

    fn clip_planes(&self) -> [Vec4; 4] {
        let along = self.along() * self.radius;
        let across = self.across() * self.radius;
        [
            plane(self.along(), self.centre - along),
            plane(-self.along(), self.centre + along),
            plane(self.across(), self.centre - across),
            plane(-self.across(), self.centre + across),
        ]
    }

    /// The square's outline on the ground, going around it.
    pub fn corners(&self) -> [Vec2; 4] {
        let along = self.along() * self.radius;
        let across = self.across() * self.radius;
        [
            self.centre + along + across,
            self.centre - along + across,
            self.centre - along - across,
            self.centre + along - across,
        ]
    }
}

/// The shape the user is working with, plus the other shapes as they were
/// last left, so switching between them doesn't lose anything.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Subcloud {
    pub shape: ShapeKind,
    pub line: Line,
    pub circle: Circle,
    pub square: Square,
}

impl Subcloud {
    /// Each shape in the middle of the data: the line west to east over 80%
    /// of its width, the circle and square at the default radius.
    pub fn new(min: Vec3, max: Vec3) -> Self {
        let centre = (min + max).truncate() * 0.5;
        let half_length = (max.x - min.x) * 0.4;
        Self {
            shape: ShapeKind::Line,
            line: Line {
                start: centre - Vec2::new(half_length, 0.0),
                end: centre + Vec2::new(half_length, 0.0),
                width: DEFAULT_WIDTH,
            },
            circle: Circle {
                centre,
                radius: DEFAULT_RADIUS,
            },
            square: Square {
                centre,
                radius: DEFAULT_RADIUS,
                rotation: 0.0,
            },
        }
    }

    pub fn footprint(&self) -> Footprint {
        match self.shape {
            ShapeKind::Line => Footprint::Planes(self.line.clip_planes()),
            ShapeKind::Circle => Footprint::Circle {
                centre: self.circle.centre,
                radius: self.circle.radius,
            },
            ShapeKind::Square => Footprint::Planes(self.square.clip_planes()),
        }
    }

    /// Moves the current shape without turning it.
    pub fn translate(&mut self, offset: Vec2) {
        match self.shape {
            ShapeKind::Line => {
                self.line.start += offset;
                self.line.end += offset;
            }
            ShapeKind::Circle => self.circle.centre += offset,
            ShapeKind::Square => self.square.centre += offset,
        }
    }

    /// Which way Ctrl+scroll slides the subcloud, as a unit vector, and how
    /// thick the subcloud is that way. `facing` is the horizontal direction
    /// the camera looks (see [`OrbitCamera::facing`]); the slide goes away
    /// from the viewer, along whichever of the shape's axes is closest.
    pub fn sweep_axis(&self, facing: Vec2) -> (Vec2, f32) {
        // Flip `axis` to point away from the viewer.
        let away = |axis: Vec2| if facing.dot(axis) >= 0.0 { axis } else { -axis };
        match self.shape {
            ShapeKind::Line => (away(self.line.normal()), self.line.width),
            ShapeKind::Circle => (facing, 2.0 * self.circle.radius),
            ShapeKind::Square => {
                let (along, across) = (self.square.along(), self.square.across());
                let axis = if facing.dot(along).abs() > facing.dot(across).abs() {
                    along
                } else {
                    across
                };
                (away(axis), 2.0 * self.square.radius)
            }
        }
    }

    /// Camera axes that line up with the shape: for a line, X along it (start
    /// to end) and Y across it.
    pub fn axes(&self) -> Axes {
        match self.shape {
            ShapeKind::Line => {
                let along = self.line.direction();
                Axes::Transect {
                    heading: along.y.atan2(along.x),
                }
            }
            ShapeKind::Circle => Axes::World,
            ShapeKind::Square => Axes::Square {
                heading: self.square.rotation,
            },
        }
    }

    /// Points `camera` at the subcloud from the side (for a line, start on
    /// the left), so all of it fits a view of `aspect` (width / height).
    /// `z_range` is the cloud's height range.
    pub fn aim(&self, camera: &mut OrbitCamera, (z_min, z_max): (f32, f32), aspect: f32) {
        // How far the shape reaches across the screen and into it, each way.
        let (centre, half_across, half_depth) = match self.shape {
            ShapeKind::Line => (self.line.midpoint(), self.line.length() * 0.5, self.line.width * 0.5),
            ShapeKind::Circle => (self.circle.centre, self.circle.radius, self.circle.radius),
            ShapeKind::Square => (self.square.centre, self.square.radius, self.square.radius),
        };
        let half_height = (z_max - z_min) * 0.5;
        // A sphere around the whole subcloud keeps every point inside the clip planes.
        let radius = Vec3::new(half_across, half_depth, half_height).length();
        camera.frame_sphere(centre.extend((z_min + z_max) * 0.5), radius);
        // Front looks along +Y in the shape's axes: for a line, across it
        // with A on the left.
        camera.axes = self.axes();
        camera.set_view(ViewPreset::Front);
        camera.set_half_height(half_height.max(half_across / aspect) * FRAMING_MARGIN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 m long along +X from the origin.
    fn east() -> Line {
        Line {
            start: Vec2::ZERO,
            end: Vec2::new(10.0, 0.0),
            width: 2.0,
        }
    }

    fn subcloud(shape: ShapeKind) -> Subcloud {
        Subcloud {
            shape,
            line: east(),
            circle: Circle {
                centre: Vec2::new(5.0, 5.0),
                radius: 2.0,
            },
            square: Square {
                centre: Vec2::ZERO,
                radius: 1.0,
                rotation: std::f32::consts::FRAC_PI_4,
            },
        }
    }

    fn footprint(line: Line) -> Footprint {
        Footprint::Planes(line.clip_planes())
    }

    #[test]
    fn line_should_contain_points_inside_the_width() {
        assert!(footprint(east()).contains(Vec2::new(5.0, 0.9)));
        assert!(footprint(east()).contains(Vec2::new(5.0, -0.9)));
    }

    #[test]
    fn line_should_not_contain_points_beside_or_beyond_the_ends() {
        assert!(!footprint(east()).contains(Vec2::new(5.0, 1.1)));
        assert!(!footprint(east()).contains(Vec2::new(-0.1, 0.0)));
        assert!(!footprint(east()).contains(Vec2::new(10.1, 0.0)));
    }

    #[test]
    fn line_should_follow_a_diagonal() {
        let diagonal = footprint(Line {
            start: Vec2::ZERO,
            end: Vec2::new(10.0, 10.0),
            width: 2.0,
        });
        assert!(diagonal.contains(Vec2::new(5.0, 5.5)));
        assert!(!diagonal.contains(Vec2::new(5.0, 7.0)));
    }

    #[test]
    fn circle_should_contain_points_within_the_radius() {
        let circle = subcloud(ShapeKind::Circle).footprint();
        assert!(circle.contains(Vec2::new(6.9, 5.0)));
        assert!(!circle.contains(Vec2::new(6.5, 6.5)));
    }

    #[test]
    fn turned_square_should_reach_further_to_its_corners() {
        // Turned 45°, the unit square's corners are on the axes, √2 out.
        let square = subcloud(ShapeKind::Square).footprint();
        assert!(square.contains(Vec2::new(1.4, 0.0)));
        assert!(!square.contains(Vec2::new(0.9, 0.9)));
    }

    #[test]
    fn circle_touches_box_should_miss_a_box_only_its_bounding_square_reaches() {
        let circle = subcloud(ShapeKind::Circle).footprint();
        // The corner of this box is 2.12 m from the centre, past the radius.
        let corner_box = (Vec3::new(6.5, 6.5, -1.0), Vec3::new(8.0, 8.0, 1.0));
        assert!(!circle.touches_box(corner_box.0, corner_box.1));
    }

    #[test]
    fn circle_touches_box_should_find_a_box_around_it() {
        let circle = subcloud(ShapeKind::Circle).footprint();
        assert!(circle.touches_box(Vec3::splat(-100.0), Vec3::splat(100.0)));
    }

    #[test]
    fn translate_should_move_only_the_current_shape() {
        let mut moved = subcloud(ShapeKind::Circle);
        moved.translate(Vec2::new(1.0, 0.0));
        assert_eq!(
            (moved.circle.centre, moved.line),
            (Vec2::new(6.0, 5.0), east())
        );
    }

    #[test]
    fn sweep_axis_should_go_away_from_the_viewer_across_a_line() {
        // Looking south, so away is -Y: the opposite of an eastward line's normal.
        let (axis, thickness) = subcloud(ShapeKind::Line).sweep_axis(Vec2::NEG_Y);
        assert_eq!((axis, thickness), (Vec2::NEG_Y, 2.0));
    }

    #[test]
    fn sweep_axis_should_pick_the_square_side_facing_the_viewer() {
        // A square turned 30°, viewed looking north: its `across` axis
        // (pointing at 120°) is closer to north than `along` (30°).
        let mut square = subcloud(ShapeKind::Square);
        square.square.rotation = 30_f32.to_radians();
        let (axis, _) = square.sweep_axis(Vec2::Y);
        assert!(axis.abs_diff_eq(square.square.across(), 1e-6), "{axis}");
    }

    #[test]
    fn aim_should_show_line_start_on_the_left_and_end_on_the_right() {
        let mut subcloud = subcloud(ShapeKind::Line);
        subcloud.line = Line {
            start: Vec2::new(-30.0, 40.0),
            end: Vec2::new(50.0, -20.0),
            width: 2.0,
        };
        let mut camera = OrbitCamera::default();
        subcloud.aim(&mut camera, (-5.0, 15.0), 2.0);

        let view_proj = camera.view_projection(2.0);
        let start = view_proj.project_point3(subcloud.line.start.extend(5.0));
        let end = view_proj.project_point3(subcloud.line.end.extend(5.0));
        assert!(start.x < -0.8 && start.x > -1.0, "start at {start}");
        assert!(end.x > 0.8 && end.x < 1.0, "end at {end}");
        assert!(start.y.abs() < 1e-3 && end.y.abs() < 1e-3);
    }

    #[test]
    fn aim_should_fit_a_circle_side_to_side() {
        let subcloud = subcloud(ShapeKind::Circle);
        let mut camera = OrbitCamera::default();
        subcloud.aim(&mut camera, (0.0, 1.0), 1.0);
        // From the south, the circle's east edge is on the right.
        let east_edge = Vec2::new(7.0, 5.0).extend(0.5);
        let ndc = camera.view_projection(1.0).project_point3(east_edge);
        assert!(ndc.x > 0.8 && ndc.x < 1.0, "{ndc}");
    }
}
