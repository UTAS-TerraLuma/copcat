use glam::camera::rh::{proj, view};
use glam::{Mat4, Vec2, Vec3};

/// A camera that orbits around a target point, with Z up (the LiDAR convention).
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    /// Angle around the Z axis, in radians. 0 looks along -X.
    pub yaw: f32,
    /// Angle above the XY plane, in radians.
    pub pitch: f32,
    pub fov_y: f32,
    /// Radius of the loaded scene, used to choose clip planes.
    scene_radius: f32,
}

const ORBIT_RADIANS_PER_PIXEL: f32 = 0.005;
const ZOOM_PER_SCROLL_PIXEL: f32 = 0.002;
/// Just short of straight up/down, where `look_at` would become degenerate.
const MAX_PITCH: f32 = 1.55;

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 100.0,
            yaw: -std::f32::consts::FRAC_PI_2,
            pitch: 0.5,
            fov_y: 45_f32.to_radians(),
            scene_radius: 100.0,
        }
    }
}

impl OrbitCamera {
    /// Points the camera at a bounding box so the whole box is in view.
    pub fn frame_bounds(&mut self, min: Vec3, max: Vec3) {
        let radius = ((max - min).length() * 0.5).max(1e-3);
        self.target = (min + max) * 0.5;
        self.scene_radius = radius;
        self.distance = radius / (self.fov_y * 0.5).sin();
    }

    pub fn eye(&self) -> Vec3 {
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let direction = Vec3::new(cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch);
        self.target + direction * self.distance
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        // Clip planes scale with distance so depth precision follows the zoom level.
        let near = (self.distance * 0.01).max(1e-3);
        let far = self.distance + self.scene_radius * 4.0;
        let view = view::look_at_mat4(self.eye(), self.target, Vec3::Z);
        // glam's "directx" convention (depth 0..1, Y up) is also wgpu's.
        let projection = proj::directx::perspective(self.fov_y, aspect, near, far);
        projection * view
    }

    /// Rotates around the target from a mouse drag, in screen pixels.
    pub fn orbit(&mut self, drag: Vec2) {
        self.yaw -= drag.x * ORBIT_RADIANS_PER_PIXEL;
        self.pitch = (self.pitch + drag.y * ORBIT_RADIANS_PER_PIXEL).clamp(-MAX_PITCH, MAX_PITCH);
    }

    /// Slides the target so the scene follows the cursor.
    pub fn pan(&mut self, drag: Vec2, viewport_height: f32) {
        let world_per_pixel = 2.0 * self.distance * (self.fov_y * 0.5).tan() / viewport_height;
        let forward = (self.target - self.eye()).normalize();
        let right = forward.cross(Vec3::Z).normalize();
        let up = right.cross(forward);
        self.target += (-drag.x * right + drag.y * up) * world_per_pixel;
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

    #[test]
    fn frame_bounds_should_keep_every_box_corner_on_screen() {
        // Autzen's bounds relative to its octree centre.
        let min = Vec3::new(-2327.8, -2327.8, -2327.8);
        let max = Vec3::new(1098.2, 2327.8, -2118.6);
        let mut camera = OrbitCamera::default();
        camera.frame_bounds(min, max);

        let view_proj = camera.view_projection(16.0 / 9.0);
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            let ndc = view_proj.project_point3(corner);
            assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0, "{corner} projects to {ndc}");
        }
    }
}
