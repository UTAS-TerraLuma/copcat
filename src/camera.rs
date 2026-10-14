use std::f32::consts::{FRAC_PI_2, PI};

use glam::camera::rh::{proj, view};
use glam::{Mat4, Vec2, Vec3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    Perspective,
    /// No foreshortening: parallel lines stay parallel, which suits measuring
    /// and straight-on side or top views.
    Orthographic,
}

impl Projection {
    pub fn toggled(self) -> Self {
        match self {
            Self::Perspective => Self::Orthographic,
            Self::Orthographic => Self::Perspective,
        }
    }
}

/// Which way a camera's X and Y axes point, for its view presets and the
/// gizmo. Z is always up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axes {
    /// X east, Y north, as in projected coordinate systems.
    World,
    /// X along a transect from start to end, Y across it (to the left).
    /// `heading` is X's angle anticlockwise from east, in radians.
    Transect { heading: f32 },
}

impl Axes {
    /// The angle X is turned from east.
    fn heading(self) -> f32 {
        match self {
            Self::World => 0.0,
            Self::Transect { heading } => heading,
        }
    }

    /// A direction in these axes, as a world direction.
    pub fn to_world(self, local: Vec3) -> Vec3 {
        glam::Quat::from_rotation_z(self.heading()) * local
    }
}

/// Looking straight along one of a camera's axes (see `Axes`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewPreset {
    Top,
    Bottom,
    Front,
    Back,
    Left,
    Right,
}

impl ViewPreset {
    pub const ALL: [Self; 6] = [
        Self::Top,
        Self::Bottom,
        Self::Front,
        Self::Back,
        Self::Left,
        Self::Right,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "Top",
            Self::Bottom => "Bottom",
            Self::Front => "Front",
            Self::Back => "Back",
            Self::Left => "Left",
            Self::Right => "Right",
        }
    }

    pub fn description(self, axes: Axes) -> &'static str {
        match (axes, self) {
            (Axes::World, Self::Top) => "From above, north up",
            (Axes::World, Self::Bottom) => "From below, north up",
            (Axes::World, Self::Front) => "From the south, looking north",
            (Axes::World, Self::Back) => "From the north, looking south",
            (Axes::World, Self::Left) => "From the west, looking east",
            (Axes::World, Self::Right) => "From the east, looking west",
            (Axes::Transect { .. }, Self::Top) => "From above, A on the left",
            (Axes::Transect { .. }, Self::Bottom) => "From below",
            (Axes::Transect { .. }, Self::Front) => "Side-on, A on the left",
            (Axes::Transect { .. }, Self::Back) => "Side-on from the other side, A on the right",
            (Axes::Transect { .. }, Self::Left) => "Along the transect from A, looking towards B",
            (Axes::Transect { .. }, Self::Right) => "Along the transect from B, looking towards A",
        }
    }

    /// The `(yaw, pitch)` that puts the eye on this side of the target, in
    /// world axes.
    fn angles(self) -> (f32, f32) {
        match self {
            // Yaw only sets which way is up on screen when looking straight
            // down or up; these keep +Y at the top.
            Self::Top => (-FRAC_PI_2, FRAC_PI_2),
            Self::Bottom => (FRAC_PI_2, -FRAC_PI_2),
            Self::Front => (-FRAC_PI_2, 0.0),
            Self::Back => (FRAC_PI_2, 0.0),
            Self::Left => (PI, 0.0),
            Self::Right => (0.0, 0.0),
        }
    }
}

/// A camera that orbits around a target point, with Z up (the LiDAR convention).
pub struct OrbitCamera {
    pub target: Vec3,
    /// Distance from the eye to the target. In orthographic mode it only
    /// sets the zoom, through `half_height`.
    pub distance: f32,
    /// Angle around the Z axis, in radians. 0 puts the eye on +X, looking along -X.
    pub yaw: f32,
    /// Angle above the XY plane, in radians, from -π/2 (below) to π/2 (above).
    pub pitch: f32,
    pub fov_y: f32,
    /// The axes `set_view` and the gizmo work in.
    pub axes: Axes,
    /// Private so every change goes through `set_projection`, `set_view` or
    /// `orbit`, which keep `projection_is_automatic` in step.
    projection: Projection,
    /// Switch to orthographic when snapping to an axis view, and back to
    /// perspective when orbiting away from it (Blender's "Auto Perspective").
    pub auto_orthographic: bool,
    /// The current orthographic projection came from `auto_orthographic`
    /// rather than the user, so orbiting should undo it.
    projection_is_automatic: bool,
    /// Radius of the loaded scene, used to choose clip planes.
    scene_radius: f32,
}

const ORBIT_RADIANS_PER_PIXEL: f32 = 0.005;
const ZOOM_PER_SCROLL_PIXEL: f32 = 0.002;

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 100.0,
            yaw: -FRAC_PI_2,
            pitch: 0.5,
            fov_y: 45_f32.to_radians(),
            axes: Axes::World,
            projection: Projection::Perspective,
            auto_orthographic: true,
            projection_is_automatic: false,
            scene_radius: 100.0,
        }
    }
}

impl OrbitCamera {
    /// Points the camera at a bounding box so the whole box is in view.
    pub fn frame_bounds(&mut self, min: Vec3, max: Vec3) {
        self.frame_sphere((min + max) * 0.5, (max - min).length() * 0.5);
    }

    /// Points the camera at a sphere so all of it is in view, and sets the
    /// clip planes to cover it.
    pub fn frame_sphere(&mut self, centre: Vec3, radius: f32) {
        let radius = radius.max(1e-3);
        self.target = centre;
        self.scene_radius = radius;
        self.distance = radius / (self.fov_y * 0.5).sin();
    }

    pub fn projection(&self) -> Projection {
        self.projection
    }

    /// A projection the user chose, which orbiting leaves alone.
    pub fn set_projection(&mut self, projection: Projection) {
        self.projection = projection;
        self.projection_is_automatic = false;
    }

    /// Looks along an axis, switching to orthographic if `auto_orthographic`
    /// is on and the view is currently perspective.
    pub fn set_view(&mut self, preset: ViewPreset) {
        let (yaw, pitch) = preset.angles();
        // Turning the axes turns every preset with them.
        (self.yaw, self.pitch) = (yaw + self.axes.heading(), pitch);
        if self.auto_orthographic && self.projection == Projection::Perspective {
            self.projection = Projection::Orthographic;
            self.projection_is_automatic = true;
        }
    }

    /// Half the height of the view at the target, in world units. Orthographic
    /// mode shows this much at every depth, so switching projection keeps
    /// whatever is at the target the same size on screen.
    pub fn half_height(&self) -> f32 {
        self.distance * (self.fov_y * 0.5).tan()
    }

    /// Zooms so the view at the target is `half_height` tall (each way).
    pub fn set_half_height(&mut self, half_height: f32) {
        self.distance = (half_height / (self.fov_y * 0.5).tan()).max(1e-3);
    }

    pub fn eye(&self) -> Vec3 {
        self.target + self.backward() * self.distance
    }

    /// Unit vector the camera looks along.
    pub fn forward(&self) -> Vec3 {
        -self.backward()
    }

    /// Unit vector from the target towards the eye.
    fn backward(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch)
    }

    /// Screen-up as a world direction. Tilting with the pitch, rather than
    /// always using +Z, keeps it valid when looking straight down.
    fn up(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        Vec3::new(-sin_pitch * cos_yaw, -sin_pitch * sin_yaw, cos_pitch)
    }

    /// Screen-right as a world direction. Always horizontal.
    fn right(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        Vec3::new(-sin_yaw, cos_yaw, 0.0)
    }

    /// Near and far clip distances. In perspective they scale with distance so
    /// depth precision follows the zoom level. Orthographic depth is linear, so
    /// the near plane can sit behind the eye and nothing gets clipped up close.
    pub fn clip_planes(&self) -> (f32, f32) {
        let depth_range = self.scene_radius * 4.0;
        let near = match self.projection {
            Projection::Perspective => (self.distance * 0.01).max(1e-3),
            Projection::Orthographic => self.distance - depth_range,
        };
        (near, self.distance + depth_range)
    }

    /// World to view space: X right, Y up, looking along -Z.
    pub fn view_matrix(&self) -> Mat4 {
        view::look_at_mat4(self.eye(), self.target, self.up())
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let (near, far) = self.clip_planes();
        // glam's "directx" convention (depth 0..1, Y up) is also wgpu's.
        let projection = match self.projection {
            Projection::Perspective => proj::directx::perspective(self.fov_y, aspect, near, far),
            Projection::Orthographic => {
                let half_height = self.half_height();
                let half_width = half_height * aspect;
                proj::directx::orthographic(
                    -half_width,
                    half_width,
                    -half_height,
                    half_height,
                    near,
                    far,
                )
            }
        };
        projection * self.view_matrix()
    }

    /// Rotates around the target from a mouse drag, in screen pixels.
    pub fn orbit(&mut self, drag: Vec2) {
        if drag == Vec2::ZERO {
            return;
        }
        // Rotating away from an axis view undoes its automatic orthographic.
        if self.projection_is_automatic {
            self.set_projection(Projection::Perspective);
        }
        self.yaw -= drag.x * ORBIT_RADIANS_PER_PIXEL;
        self.pitch = (self.pitch + drag.y * ORBIT_RADIANS_PER_PIXEL).clamp(-FRAC_PI_2, FRAC_PI_2);
    }

    /// Slides the target so the scene follows the cursor.
    pub fn pan(&mut self, drag: Vec2, viewport_height: f32) {
        let world_per_pixel = 2.0 * self.half_height() / viewport_height;
        self.target += (-drag.x * self.right() + drag.y * self.up()) * world_per_pixel;
    }

    /// Moves towards or away from the target. Positive `scroll` zooms in.
    pub fn zoom(&mut self, scroll: f32) {
        self.distance *= (-scroll * ZOOM_PER_SCROLL_PIXEL).exp();
        self.distance = self.distance.max(1e-3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT: f32 = 16.0 / 9.0;

    fn assert_close(actual: Vec3, expected: Vec3) {
        assert!(actual.abs_diff_eq(expected, 1e-3), "{actual} != {expected}");
    }

    #[test]
    fn frame_bounds_should_keep_every_box_corner_on_screen() {
        // Autzen's bounds relative to its octree centre.
        let min = Vec3::new(-2327.8, -2327.8, -2327.8);
        let max = Vec3::new(1098.2, 2327.8, -2118.6);
        for projection in [Projection::Perspective, Projection::Orthographic] {
            let mut camera = OrbitCamera {
                projection,
                ..OrbitCamera::default()
            };
            camera.frame_bounds(min, max);

            let view_proj = camera.view_projection(ASPECT);
            for i in 0..8 {
                let corner = Vec3::select(
                    glam::BVec3::new(i & 1 != 0, i & 2 != 0, i & 4 != 0),
                    max,
                    min,
                );
                let ndc = view_proj.project_point3(corner);
                assert!(
                    ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0 && (0.0..=1.0).contains(&ndc.z),
                    "{projection:?}: {corner} projects to {ndc}"
                );
            }
        }
    }

    #[test]
    fn top_view_should_look_straight_down_with_north_up() {
        let mut camera = OrbitCamera::default();
        camera.set_view(ViewPreset::Top);
        assert_close(camera.eye(), Vec3::new(0.0, 0.0, camera.distance));

        let view_proj = camera.view_projection(1.0);
        let north = view_proj.project_point3(Vec3::Y);
        let east = view_proj.project_point3(Vec3::X);
        assert!(north.y > 0.0 && north.x.abs() < 1e-4, "north at {north}");
        assert!(east.x > 0.0 && east.y.abs() < 1e-4, "east at {east}");
    }

    #[test]
    fn side_views_should_put_the_eye_on_their_side() {
        let cases = [
            (ViewPreset::Front, Vec3::NEG_Y),
            (ViewPreset::Back, Vec3::Y),
            (ViewPreset::Left, Vec3::NEG_X),
            (ViewPreset::Right, Vec3::X),
            (ViewPreset::Bottom, Vec3::NEG_Z),
        ];
        for (preset, side) in cases {
            let mut camera = OrbitCamera::default();
            camera.set_view(preset);
            assert_close(camera.eye(), side * camera.distance);
        }
    }

    #[test]
    fn switching_projection_should_keep_the_target_plane_the_same_size() {
        let mut camera = OrbitCamera::default();
        // A point one metre right of the target, in the plane facing the camera.
        let beside_target = camera.target + camera.right();
        let perspective = camera.view_projection(ASPECT).project_point3(beside_target);
        camera.set_projection(Projection::Orthographic);
        let orthographic = camera.view_projection(ASPECT).project_point3(beside_target);
        assert!((perspective.x - orthographic.x).abs() < 1e-4);
    }

    #[test]
    fn axis_view_should_be_orthographic_until_orbited_away() {
        let mut camera = OrbitCamera::default();
        camera.set_view(ViewPreset::Front);
        assert_eq!(camera.projection(), Projection::Orthographic);
        camera.orbit(Vec2::new(10.0, 0.0));
        assert_eq!(camera.projection(), Projection::Perspective);
    }

    #[test]
    fn orbit_should_keep_orthographic_the_user_chose() {
        let mut camera = OrbitCamera::default();
        camera.set_projection(Projection::Orthographic);
        camera.set_view(ViewPreset::Top);
        camera.orbit(Vec2::new(10.0, 0.0));
        assert_eq!(camera.projection(), Projection::Orthographic);
    }

    #[test]
    fn orbit_should_keep_orthographic_chosen_after_an_axis_view() {
        let mut camera = OrbitCamera::default();
        camera.set_view(ViewPreset::Top);
        // Choosing it again makes it the user's choice, not an automatic one.
        camera.set_projection(Projection::Orthographic);
        camera.orbit(Vec2::new(10.0, 0.0));
        assert_eq!(camera.projection(), Projection::Orthographic);
    }

    #[test]
    fn axis_view_should_stay_perspective_with_auto_orthographic_off() {
        let mut camera = OrbitCamera {
            auto_orthographic: false,
            ..OrbitCamera::default()
        };
        camera.set_view(ViewPreset::Top);
        assert_eq!(camera.projection(), Projection::Perspective);
    }

    #[test]
    fn front_view_in_transect_axes_should_look_across_the_transect() {
        // A transect heading north-east.
        let along = Vec2::new(1.0, 1.0).normalize();
        let mut camera = OrbitCamera {
            axes: Axes::Transect {
                heading: along.y.atan2(along.x),
            },
            ..OrbitCamera::default()
        };
        camera.set_view(ViewPreset::Front);

        // The eye is on the right of the line (-Y in transect axes)...
        assert_close(camera.eye(), -along.perp().extend(0.0) * camera.distance);
        // ...so the transect runs left to right across the screen.
        let ahead = camera.view_projection(1.0).project_point3(along.extend(0.0));
        assert!(ahead.x > 0.0 && ahead.y.abs() < 1e-4, "{ahead}");
    }

    #[test]
    fn axes_to_world_should_turn_x_to_the_heading() {
        let axes = Axes::Transect { heading: FRAC_PI_2 };
        assert_close(axes.to_world(Vec3::X), Vec3::Y);
        assert_close(axes.to_world(Vec3::Z), Vec3::Z);
    }
}
