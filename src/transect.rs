//! A transect: a vertical slab along a line on the ground, for looking at a
//! thin profile through the cloud. Everything here is pure geometry in local
//! coordinates (relative to the octree centre), so it's easy to test.

use glam::{Vec2, Vec3, Vec4};

use crate::camera::{Axes, OrbitCamera, ViewPreset};

pub const DEFAULT_WIDTH: f32 = 2.0;
/// Narrower than this and the slab holds too few points to be useful.
pub const MIN_WIDTH: f32 = 0.05;
/// Ctrl+scroll slides the transect sideways by this percentage of its width
/// per mouse wheel notch, unless the user changes it. 100% steps to the
/// next slice with no gap or overlap.
pub const DEFAULT_SWEEP_STEP_PERCENT: f32 = 100.0;
/// Extra room around the slab when the camera frames it.
const FRAMING_MARGIN: f32 = 1.1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transect {
    /// Ends of the centre line on the ground (XY).
    pub start: Vec2,
    pub end: Vec2,
    /// Full width across the line, in metres.
    pub width: f32,
}

impl Transect {
    /// West to east across the middle of the data, over 80% of its width.
    pub fn across(min: Vec3, max: Vec3) -> Self {
        let centre = (min + max).truncate() * 0.5;
        let half_length = (max.x - min.x) * 0.4;
        Self {
            start: centre - Vec2::new(half_length, 0.0),
            end: centre + Vec2::new(half_length, 0.0),
            width: DEFAULT_WIDTH,
        }
    }

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

    /// Slides the whole transect sideways by `metres` along `normal`.
    pub fn sweep(&mut self, metres: f32) {
        let offset = self.normal() * metres;
        self.start += offset;
        self.end += offset;
    }

    /// Whether `point` is inside the slab, seen from above.
    pub fn contains(&self, point: Vec2) -> bool {
        self.clip_planes()
            .iter()
            .all(|plane| plane.dot(point.extend(0.0).extend(1.0)) >= 0.0)
    }

    /// The four vertical planes around the slab, as `(normal, d)` with points
    /// inside where `normal·p + d >= 0`. There's no top or bottom: the slab
    /// runs the full height of the cloud.
    pub fn clip_planes(&self) -> [Vec4; 4] {
        let along = self.direction();
        let across = self.normal();
        let half_width = across * self.width * 0.5;
        // The plane through `point` facing `inward`.
        let plane = |inward: Vec2, point: Vec2| inward.extend(0.0).extend(-inward.dot(point));
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

    /// Camera axes with X along the line (start to end) and Y across it.
    pub fn axes(&self) -> Axes {
        let along = self.direction();
        Axes::Transect {
            heading: along.y.atan2(along.x),
        }
    }

    /// Points `camera` straight at the slab from the side, start on the left,
    /// so the whole profile fits a view of `aspect` (width / height).
    /// `z_range` is the cloud's height range.
    pub fn aim(&self, camera: &mut OrbitCamera, (z_min, z_max): (f32, f32), aspect: f32) {
        let half_length = self.length() * 0.5;
        let half_height = (z_max - z_min) * 0.5;
        // A sphere around the whole slab keeps every point inside the clip planes.
        let radius = (half_length.powi(2) + half_height.powi(2) + self.width.powi(2)).sqrt();
        let centre = self.midpoint().extend((z_min + z_max) * 0.5);
        camera.frame_sphere(centre, radius);
        // In transect axes, Front looks along +Y: across the line, A on the left.
        camera.axes = self.axes();
        camera.set_view(ViewPreset::Front);
        camera.set_half_height(half_height.max(half_length / aspect) * FRAMING_MARGIN);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 m long along +X from the origin.
    fn east() -> Transect {
        Transect {
            start: Vec2::ZERO,
            end: Vec2::new(10.0, 0.0),
            width: 2.0,
        }
    }

    #[test]
    fn contains_should_keep_points_inside_the_width() {
        assert!(east().contains(Vec2::new(5.0, 0.9)));
        assert!(east().contains(Vec2::new(5.0, -0.9)));
    }

    #[test]
    fn contains_should_drop_points_beside_or_beyond_the_ends() {
        assert!(!east().contains(Vec2::new(5.0, 1.1)));
        assert!(!east().contains(Vec2::new(-0.1, 0.0)));
        assert!(!east().contains(Vec2::new(10.1, 0.0)));
    }

    #[test]
    fn contains_should_follow_a_diagonal_line() {
        let diagonal = Transect {
            start: Vec2::ZERO,
            end: Vec2::new(10.0, 10.0),
            width: 2.0,
        };
        assert!(diagonal.contains(Vec2::new(5.0, 5.5)));
        assert!(!diagonal.contains(Vec2::new(5.0, 7.0)));
    }

    #[test]
    fn sweep_should_slide_sideways_by_the_given_distance() {
        let mut transect = east();
        transect.sweep(0.2);
        // The normal of an eastward line points north.
        assert_eq!(transect.start, Vec2::new(0.0, 0.2));
        assert_eq!(transect.end, Vec2::new(10.0, 0.2));
    }

    #[test]
    fn aim_should_show_start_on_the_left_and_end_on_the_right() {
        let transect = Transect {
            start: Vec2::new(-30.0, 40.0),
            end: Vec2::new(50.0, -20.0),
            width: 2.0,
        };
        let mut camera = OrbitCamera::default();
        transect.aim(&mut camera, (-5.0, 15.0), 2.0);

        let view_proj = camera.view_projection(2.0);
        let start = view_proj.project_point3(transect.start.extend(5.0));
        let end = view_proj.project_point3(transect.end.extend(5.0));
        assert!(start.x < -0.8 && start.x > -1.0, "start at {start}");
        assert!(end.x > 0.8 && end.x < 1.0, "end at {end}");
        assert!(start.y.abs() < 1e-3 && end.y.abs() < 1e-3);
    }
}
