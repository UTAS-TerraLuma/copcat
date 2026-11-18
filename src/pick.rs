//! Finding the ground under the pointer in a 3D view: a ray from the camera
//! through the pointer, met with a level plane. Pure maths, so it's easy to
//! test. Subclouds are vertical prisms, so only where on the ground (XY) the
//! pointer is matters, not how high the points there are.

use glam::{Mat4, Vec2, Vec3};

/// Rays closer to level than this (the sine of the angle, about 0.06°) never
/// meet the ground anywhere useful, so count as missing it.
const MIN_RAY_SLOPE: f32 = 1e-3;

/// A half-line in local coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    /// A unit vector.
    pub direction: Vec3,
}

/// The pointer's position in normalised device coordinates (-1..1, Y up),
/// for a view `size` big with the pointer `offset` from its top left, both
/// in the same units.
pub fn to_ndc(offset: Vec2, size: Vec2) -> Vec2 {
    let uv = offset / size.max(Vec2::splat(1e-6));
    Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0)
}

/// The ray through `ndc` of a view drawn with `view_projection`, from the
/// near plane away from the camera. Works for perspective (rays fan out
/// from the eye) and orthographic (parallel rays) alike.
pub fn ray_through(view_projection: Mat4, ndc: Vec2) -> Option<Ray> {
    let inverse = view_projection.inverse();
    // wgpu's depth runs 0 (near) to 1 (far).
    let near = inverse.project_point3(ndc.extend(0.0));
    let far = inverse.project_point3(ndc.extend(1.0));
    let direction = (far - near).try_normalize()?;
    near.is_finite().then_some(Ray {
        origin: near,
        direction,
    })
}

/// Where `ray` meets the level plane at height `z`, or `None` if it runs
/// (nearly) level or away from it.
pub fn hit_level(ray: Ray, z: f32) -> Option<Vec2> {
    if ray.direction.z.abs() < MIN_RAY_SLOPE {
        return None;
    }
    let t = (z - ray.origin.z) / ray.direction.z;
    (t >= 0.0).then(|| (ray.origin + ray.direction * t).truncate())
}

/// The ground under `ray`, as `(where, ground height)`. Meets the plane at
/// `fallback_z` (say the cloud's lowest point) first, then asks `ground_at`
/// how low the points are around there and meets that height instead, so a
/// slanted ray lands near the ground the user sees rather than far behind
/// it. `None` if the ray misses both.
pub fn ground_under(
    ray: Ray,
    fallback_z: f32,
    ground_at: impl Fn(Vec2) -> Option<f32>,
) -> Option<(Vec2, f32)> {
    let first = hit_level(ray, fallback_z)?;
    match ground_at(first) {
        Some(z) => hit_level(ray, z)
            .map(|at| (at, z))
            .or(Some((first, fallback_z))),
        None => Some((first, fallback_z)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{OrbitCamera, Projection, ViewPreset};

    const ASPECT: f32 = 1.5;

    fn camera(projection: Projection) -> OrbitCamera {
        let mut camera = OrbitCamera::default();
        camera.frame_sphere(Vec3::new(10.0, 20.0, 0.0), 50.0);
        camera.set_projection(projection);
        camera
    }

    /// The ground point (at height `z`) under `ndc` in `camera`'s view.
    fn pick(camera: &OrbitCamera, ndc: Vec2, z: f32) -> Option<Vec2> {
        hit_level(ray_through(camera.view_projection(ASPECT), ndc)?, z)
    }

    fn assert_close(actual: Vec2, expected: Vec2) {
        assert!(actual.abs_diff_eq(expected, 1e-2), "{actual} != {expected}");
    }

    #[test]
    fn to_ndc_should_put_the_top_left_at_minus_one_one() {
        let size = Vec2::new(200.0, 100.0);
        assert_eq!(to_ndc(Vec2::ZERO, size), Vec2::new(-1.0, 1.0));
        assert_eq!(to_ndc(size, size), Vec2::new(1.0, -1.0));
        assert_eq!(to_ndc(size * 0.5, size), Vec2::ZERO);
    }

    #[test]
    fn centre_of_the_view_should_pick_the_target_from_any_angle() {
        for projection in [Projection::Perspective, Projection::Orthographic] {
            for preset in [None, Some(ViewPreset::Top)] {
                let mut camera = camera(projection);
                if let Some(preset) = preset {
                    camera.set_view(preset);
                }
                let at = pick(&camera, Vec2::ZERO, 0.0).expect("looking down at the ground");
                assert_close(at, Vec2::new(10.0, 20.0));
            }
        }
    }

    #[test]
    fn picking_should_undo_projecting_a_ground_point() {
        let ground = Vec3::new(-5.0, 35.0, 0.0);
        for projection in [Projection::Perspective, Projection::Orthographic] {
            let camera = camera(projection);
            let ndc = camera
                .view_projection(ASPECT)
                .project_point3(ground)
                .truncate();
            let at = pick(&camera, ndc, 0.0).expect("on screen");
            assert_close(at, ground.truncate());
        }
    }

    #[test]
    fn top_view_should_pick_north_up_the_screen_and_east_right() {
        let mut camera = camera(Projection::Orthographic);
        camera.set_view(ViewPreset::Top);
        let centre = pick(&camera, Vec2::ZERO, 0.0).unwrap();
        let up = pick(&camera, Vec2::new(0.0, 0.5), 0.0).unwrap();
        let right = pick(&camera, Vec2::new(0.5, 0.0), 0.0).unwrap();
        assert!(up.y > centre.y && (up.x - centre.x).abs() < 1e-3, "{up}");
        assert!(right.x > centre.x && (right.y - centre.y).abs() < 1e-3);
    }

    #[test]
    fn orthographic_top_view_should_pick_the_same_spot_at_any_height() {
        let mut camera = camera(Projection::Orthographic);
        camera.set_view(ViewPreset::Top);
        let ndc = Vec2::new(0.3, -0.4);
        assert_close(
            pick(&camera, ndc, -20.0).unwrap(),
            pick(&camera, ndc, 20.0).unwrap(),
        );
    }

    #[test]
    fn above_the_horizon_should_miss() {
        // From the front, level: the top half of the screen is sky.
        let mut camera = camera(Projection::Perspective);
        camera.pitch = 0.1;
        assert_eq!(pick(&camera, Vec2::new(0.0, 0.9), 0.0), None);
    }

    #[test]
    fn a_level_ray_should_miss() {
        let ray = Ray {
            origin: Vec3::new(0.0, 0.0, 10.0),
            direction: Vec3::X,
        };
        assert_eq!(hit_level(ray, 0.0), None);
    }

    #[test]
    fn a_side_view_should_miss_in_orthographic() {
        let mut camera = camera(Projection::Orthographic);
        camera.set_view(ViewPreset::Front);
        assert_eq!(pick(&camera, Vec2::new(0.2, -0.5), 0.0), None);
    }

    #[test]
    fn ground_under_should_land_on_the_ground_the_points_are_on() {
        // Looking down at 45° from the south: a plane 10 m lower is 10 m
        // further north along the ray.
        let ray = Ray {
            origin: Vec3::new(0.0, -100.0, 100.0),
            direction: Vec3::new(0.0, 1.0, -1.0).normalize(),
        };
        let (at, z) = ground_under(ray, 0.0, |_| Some(10.0)).unwrap();
        assert_eq!(z, 10.0);
        assert_close(at, Vec2::new(0.0, -10.0));
    }

    #[test]
    fn ground_under_should_fall_back_without_points() {
        let ray = Ray {
            origin: Vec3::new(3.0, 4.0, 50.0),
            direction: Vec3::NEG_Z,
        };
        assert_eq!(
            ground_under(ray, 5.0, |_| None),
            Some((Vec2::new(3.0, 4.0), 5.0))
        );
    }
}
