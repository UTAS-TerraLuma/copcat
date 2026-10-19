//! Level of detail: choosing which octree nodes to draw for the current camera.
//!
//! Everything here is pure (no IO, no GPU), so it's cheap to run every frame
//! and easy to unit test.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};

use copc_streaming::VoxelKey;
use glam::{Mat4, Vec3, Vec4};

use crate::camera::{OrbitCamera, Projection};
use crate::subcloud::Footprint;

/// The shape of a COPC octree: which nodes exist and how many points each holds.
pub struct Octree {
    /// Half the side of the root cube. The cube is centred on the local origin,
    /// because positions are stored relative to the octree centre.
    halfsize: f32,
    /// Average distance between points in the root node, in metres.
    spacing: f32,
    /// The points' bounding box in local coordinates. Node cubes are clipped
    /// to it, because a flat site leaves most of each cube empty.
    data_bounds: (Vec3, Vec3),
    point_counts: HashMap<VoxelKey, u32>,
}

impl Octree {
    pub fn new(
        halfsize: f32,
        spacing: f32,
        data_bounds: (Vec3, Vec3),
        point_counts: HashMap<VoxelKey, u32>,
    ) -> Self {
        Self {
            halfsize,
            spacing,
            data_bounds,
            point_counts,
        }
    }

    pub fn node_count(&self) -> usize {
        self.point_counts.len()
    }

    /// `None` if the node isn't in the hierarchy.
    pub fn point_count(&self, key: &VoxelKey) -> Option<u32> {
        self.point_counts.get(key).copied()
    }

    /// The part of the node's cube that can hold points, in local coordinates
    /// (relative to the octree centre).
    pub fn node_bounds(&self, key: &VoxelKey) -> (Vec3, Vec3) {
        let side = 2.0 * self.halfsize / 2_f32.powi(key.level);
        let min = Vec3::splat(-self.halfsize)
            + Vec3::new(key.x as f32, key.y as f32, key.z as f32) * side;
        let max = min + Vec3::splat(side);
        // Clamp both corners into the data box. Written as max/min rather than
        // `clamp` so a cube entirely outside it collapses flat instead of panicking.
        let (data_min, data_max) = self.data_bounds;
        let clip = |corner: Vec3| corner.max(data_min).min(data_max);
        (clip(min), clip(max))
    }

    /// Each level adds points between its parent's, halving the spacing.
    fn node_spacing(&self, key: &VoxelKey) -> f32 {
        self.spacing / 2_f32.powi(key.level)
    }
}

/// What node selection needs to know about the camera.
pub struct View {
    frustum: Frustum,
    /// A subcloud's outline that nodes must be above.
    clip: Option<Footprint>,
    eye: Vec3,
    pixel_scale: PixelScale,
}

/// How a length in metres becomes pixels on screen.
#[derive(Clone, Copy, Debug)]
enum PixelScale {
    /// `px = length / distance * scale`: further away looks smaller.
    Perspective(f32),
    /// `px = length * scale` at any distance.
    Orthographic(f32),
}

impl View {
    pub fn new(camera: &OrbitCamera, viewport_px: [u32; 2]) -> Self {
        let [width, height] = viewport_px.map(|v| v.max(1) as f32);
        let pixel_scale = match camera.projection() {
            Projection::Perspective => {
                PixelScale::Perspective(height / (2.0 * (camera.fov_y * 0.5).tan()))
            }
            Projection::Orthographic => {
                PixelScale::Orthographic(height / (2.0 * camera.half_height()))
            }
        };
        Self {
            frustum: Frustum::from_view_projection(camera.view_projection(width / height)),
            clip: None,
            eye: camera.eye(),
            pixel_scale,
        }
    }

    /// Also skips nodes entirely outside `footprint`, so detail is only spent
    /// on what will be drawn.
    pub fn clipped_to(self, footprint: Footprint) -> Self {
        Self {
            clip: Some(footprint),
            ..self
        }
    }

    fn sees_box(&self, min: Vec3, max: Vec3) -> bool {
        box_touches_planes(&self.frustum.planes, min, max)
            && self
                .clip
                .is_none_or(|footprint| footprint.touches_box(min, max))
    }

    /// Approximate on-screen size in pixels of `length` metres at `distance`.
    fn projected_px(&self, length: f32, distance: f32) -> f32 {
        match self.pixel_scale {
            PixelScale::Perspective(scale) => length * scale / distance.max(1e-6),
            PixelScale::Orthographic(scale) => length * scale,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LodSettings {
    /// Stop adding nodes once this many points are selected.
    pub point_budget: u64,
    /// Refine a node while its points would be further apart than this on screen.
    pub target_spacing_px: f32,
}

impl Default for LodSettings {
    fn default() -> Self {
        Self {
            point_budget: 5_000_000,
            target_spacing_px: 2.0,
        }
    }
}

#[derive(Debug, Default)]
pub struct Selection {
    /// Nodes to draw, most important (coarsest, nearest) first. Every node's
    /// parent comes before it, so a prefix of this list is a hole-free cloud.
    pub keys: Vec<VoxelKey>,
    /// Total points in `keys`.
    pub points: u64,
}

/// Picks the nodes to draw: traverses from the root, skipping nodes outside the
/// view and refining nodes whose points would look too sparse, until the point
/// budget runs out.
///
/// A priority queue ordered by projected point spacing means the budget goes to
/// the nodes that look sparsest on screen first, rather than to whichever branch
/// a depth-first walk happens to reach first.
pub fn select_nodes(octree: &Octree, view: &View, settings: &LodSettings) -> Selection {
    let mut selection = Selection::default();
    let mut queue: BinaryHeap<Candidate> =
        Candidate::new(octree, view, VoxelKey::ROOT).into_iter().collect();

    while let Some(candidate) = queue.pop() {
        let count = octree.point_count(&candidate.key).map_or(0, u64::from);
        // Always allow the first node, so a tiny budget still shows something.
        if selection.points + count > settings.point_budget && !selection.keys.is_empty() {
            break;
        }
        selection.points += count;
        // Empty nodes aren't drawn, but their children may still hold points.
        if count > 0 {
            selection.keys.push(candidate.key);
        }
        if candidate.spacing_px > settings.target_spacing_px {
            queue.extend(
                candidate
                    .key
                    .children()
                    .into_iter()
                    .filter_map(|child| Candidate::new(octree, view, child)),
            );
        }
    }
    selection
}

/// Every node with points above `footprint`, at every level: all the
/// full-resolution points that could be inside it, coarsest first. `None` if
/// they add up to more than `point_limit`.
pub fn nodes_in_footprint(
    octree: &Octree,
    footprint: &Footprint,
    point_limit: u64,
) -> Option<Selection> {
    let mut selection = Selection::default();
    // Breadth first, so coarse nodes (and a rough result) come first.
    let mut queue = VecDeque::from([VoxelKey::ROOT]);
    while let Some(key) = queue.pop_front() {
        let Some(count) = octree.point_count(&key) else {
            continue;
        };
        let (min, max) = octree.node_bounds(&key);
        if !footprint.touches_box(min, max) {
            continue;
        }
        selection.points += u64::from(count);
        if selection.points > point_limit {
            return None;
        }
        if count > 0 {
            selection.keys.push(key);
        }
        queue.extend(key.children());
    }
    Some(selection)
}

/// Roughly how high the points above `footprint` reach: the height ranges in
/// `known` (the actual points' ranges of nodes loaded so far) of the finest
/// nodes there. Finer nodes cover less ground, so fit the points under the
/// footprint more tightly. `None` if no node there has a known range.
pub fn z_range_under(
    octree: &Octree,
    footprint: &Footprint,
    known: &HashMap<VoxelKey, (f32, f32)>,
) -> Option<(f32, f32)> {
    let touches = |key: &VoxelKey| {
        octree.point_count(key).is_some() && {
            let (min, max) = octree.node_bounds(key);
            footprint.touches_box(min, max)
        }
    };
    if !touches(&VoxelKey::ROOT) {
        return None;
    }
    // Depth first, finishing each node's children before the node itself,
    // so a node's range is only used where none of its children has one.
    let mut finest: HashMap<VoxelKey, (f32, f32)> = HashMap::new();
    let mut stack = vec![(VoxelKey::ROOT, false)];
    while let Some((key, children_done)) = stack.pop() {
        if !children_done {
            stack.push((key, true));
            stack.extend(key.children().into_iter().filter(&touches).map(|c| (c, false)));
            continue;
        }
        let from_children = key
            .children()
            .iter()
            .filter_map(|child| finest.remove(child))
            .reduce(union);
        if let Some(range) = from_children.or_else(|| known.get(&key).copied()) {
            finest.insert(key, range);
        }
    }
    finest.remove(&VoxelKey::ROOT)
}

fn union((low_a, high_a): (f32, f32), (low_b, high_b): (f32, f32)) -> (f32, f32) {
    (low_a.min(low_b), high_a.max(high_b))
}

/// A node waiting in the traversal queue.
struct Candidate {
    key: VoxelKey,
    /// How far apart this node's points appear on screen, in pixels.
    spacing_px: f32,
}

impl Candidate {
    /// `None` if the node doesn't exist or is outside the view.
    fn new(octree: &Octree, view: &View, key: VoxelKey) -> Option<Self> {
        octree.point_count(&key)?;
        let (min, max) = octree.node_bounds(&key);
        if !view.sees_box(min, max) {
            return None;
        }
        // Distance to the node's bounding sphere: zero or less when the eye is
        // inside it, which `projected_px` turns into "very detailed".
        let centre = (min + max) * 0.5;
        let radius = (max - min).length() * 0.5;
        let distance = view.eye.distance(centre) - radius;
        Some(Self {
            key,
            spacing_px: view.projected_px(octree.node_spacing(&key), distance),
        })
    }
}

// `BinaryHeap` needs a total order. `f32` only has a partial one (NaN), so
// order by `total_cmp` and define equality to match.
impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.spacing_px.total_cmp(&other.spacing_px)
    }
}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Candidate {}

/// The six planes bounding what the camera can see.
struct Frustum {
    /// Each plane is `(normal, d)`, with points inside where `normal·p + d >= 0`.
    planes: [Vec4; 6],
}

impl Frustum {
    /// Gribb–Hartmann plane extraction, for wgpu's 0..1 clip-space depth.
    fn from_view_projection(m: Mat4) -> Self {
        let [r0, r1, r2, r3] = [0, 1, 2, 3].map(|i| m.row(i));
        Self {
            planes: [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2],
        }
    }
}

/// Whether a box is at least partly on the inside of every plane.
/// Conservative: may say yes for a box just outside where two planes meet,
/// which only costs drawing an extra node.
pub fn box_touches_planes(planes: &[Vec4], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|plane| {
        let normal = plane.truncate();
        // The corner furthest along the normal; if even that is outside,
        // the whole box is.
        let corner = Vec3::select(normal.cmpge(Vec3::ZERO), max, min);
        normal.dot(corner) + plane.w >= 0.0
    })
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;
    use crate::subcloud::{Line, Subcloud};

    const HALFSIZE: f32 = 100.0;
    const VIEWPORT: [u32; 2] = [1000, 1000];

    /// Every node down to `depth`, each holding `points_per_node` points.
    fn full_octree(depth: i32, points_per_node: u32) -> Octree {
        let mut counts = HashMap::new();
        let mut level = vec![VoxelKey::ROOT];
        for _ in 0..=depth {
            counts.extend(level.iter().map(|key| (*key, points_per_node)));
            level = level.iter().flat_map(|key| key.children()).collect();
        }
        let cube = (Vec3::splat(-HALFSIZE), Vec3::splat(HALFSIZE));
        Octree::new(HALFSIZE, 10.0, cube, counts)
    }

    fn camera(target: Vec3, distance: f32) -> OrbitCamera {
        let mut camera = OrbitCamera::default();
        camera.frame_bounds(Vec3::splat(-HALFSIZE), Vec3::splat(HALFSIZE));
        camera.target = target;
        camera.distance = distance;
        camera
    }

    fn select(octree: &Octree, camera: &OrbitCamera, settings: &LodSettings) -> Selection {
        select_nodes(octree, &View::new(camera, VIEWPORT), settings)
    }

    #[test]
    fn select_nodes_should_pick_only_root_when_far_away() {
        let octree = full_octree(3, 1000);
        let selection = select(&octree, &camera(Vec3::ZERO, 100_000.0), &LodSettings::default());
        assert_eq!(selection.keys, vec![VoxelKey::ROOT]);
    }

    #[test]
    fn select_nodes_should_reach_deepest_level_when_camera_is_inside() {
        let octree = full_octree(3, 1000);
        let selection = select(&octree, &camera(Vec3::ZERO, 1.0), &LodSettings::default());
        let deepest = selection.keys.iter().map(|key| key.level).max();
        assert_eq!(deepest, Some(3));
    }

    #[test]
    fn select_nodes_should_skip_everything_behind_the_camera() {
        let octree = full_octree(3, 1000);
        // Yaw +90° puts the eye on +Y of the target, looking towards -Y,
        // away from the cloud.
        let mut camera = camera(Vec3::new(0.0, -1000.0, 0.0), 10.0);
        camera.yaw = std::f32::consts::FRAC_PI_2;
        let selection = select(&octree, &camera, &LodSettings::default());
        assert!(selection.keys.is_empty(), "{:?}", selection.keys);
    }

    #[test]
    fn select_nodes_should_cull_far_corner_when_zoomed_into_near_corner() {
        let octree = full_octree(3, 1000);
        // The default camera looks along +Y and down, so the -X-Y-Z corner
        // is behind it.
        let camera = camera(Vec3::splat(50.0), 20.0);
        let far_corner = VoxelKey {
            level: 3,
            x: 0,
            y: 0,
            z: 0,
        };
        let selection = select(&octree, &camera, &LodSettings::default());
        assert!(!selection.keys.contains(&far_corner));
    }

    #[test]
    fn select_nodes_should_stop_at_point_budget() {
        let octree = full_octree(3, 1000);
        let settings = LodSettings {
            point_budget: 10_500,
            ..LodSettings::default()
        };
        let selection = select(&octree, &camera(Vec3::ZERO, 1.0), &settings);
        assert_eq!(selection.points, 10_000);
    }

    #[test]
    fn select_nodes_should_list_parents_before_children() {
        let octree = full_octree(3, 1000);
        let selection = select(&octree, &camera(Vec3::splat(50.0), 20.0), &LodSettings::default());
        let position = |key: &VoxelKey| selection.keys.iter().position(|k| k == key);
        let out_of_order: Vec<_> = selection
            .keys
            .iter()
            .filter(|key| {
                key.parent()
                    .is_some_and(|parent| position(&parent) > position(key))
            })
            .collect();
        assert!(out_of_order.is_empty(), "{out_of_order:?}");
    }

    #[test]
    fn select_nodes_should_traverse_through_empty_nodes() {
        let mut octree = full_octree(2, 1000);
        octree.point_counts.insert(VoxelKey::ROOT, 0);
        // Far enough to see the whole cube, close enough to want every level.
        let selection = select(&octree, &camera(Vec3::ZERO, 500.0), &LodSettings::default());
        assert_eq!(selection.keys.len(), 8 + 64);
    }

    #[test]
    fn select_nodes_should_refine_evenly_in_orthographic() {
        let octree = full_octree(5, 1);
        let mut camera = OrbitCamera::default();
        camera.set_projection(Projection::Orthographic);
        camera.frame_bounds(Vec3::splat(-HALFSIZE), Vec3::splat(HALFSIZE));
        // Framed, the root's 10 m spacing is about 27 px, so levels 0..=3 are
        // still sparser than 2 px and get refined. Distance doesn't matter, so
        // that holds for every node: all of levels 0..=4 and none of level 5.
        let selection = select(&octree, &camera, &LodSettings::default());
        assert_eq!(selection.keys.len(), 1 + 8 + 64 + 512 + 4096);
    }

    /// A 2 m slab running east–west at y = 60, across the whole cube.
    fn slab() -> Footprint {
        let mut subcloud = Subcloud::new(Vec3::splat(-HALFSIZE), Vec3::splat(HALFSIZE));
        subcloud.line = Line {
            start: Vec2::new(-100.0, 60.0),
            end: Vec2::new(100.0, 60.0),
            width: 2.0,
        };
        subcloud.footprint()
    }

    #[test]
    fn select_nodes_should_skip_nodes_outside_the_footprint() {
        let octree = full_octree(3, 1000);
        let view = View::new(&camera(Vec3::ZERO, 500.0), VIEWPORT).clipped_to(slab());
        let selection = select_nodes(&octree, &view, &LodSettings::default());

        let outside: Vec<_> = selection
            .keys
            .iter()
            .filter(|key| {
                let (min, max) = octree.node_bounds(key);
                min.y > 61.0 || max.y < 59.0
            })
            .collect();
        assert!(!selection.keys.is_empty() && outside.is_empty(), "{outside:?}");
    }

    #[test]
    fn nodes_in_footprint_should_take_every_level_under_a_circle() {
        let octree = full_octree(3, 1000);
        // Inside one level 3 node (25 m wide), away from its edges, so one
        // node per level, and every z layer of each: 4 levels × (1, 2, 4, 8).
        let circle = Footprint::Circle {
            centre: Vec2::new(12.5, 12.5),
            radius: 1.0,
        };
        let selection = nodes_in_footprint(&octree, &circle, u64::MAX);
        assert_eq!(selection.map(|s| s.keys.len()), Some(1 + 2 + 4 + 8));
    }

    #[test]
    fn nodes_in_footprint_should_give_up_past_the_point_limit() {
        let octree = full_octree(3, 1000);
        assert!(nodes_in_footprint(&octree, &slab(), 10_000).is_none());
    }

    fn small_circle() -> Footprint {
        Footprint::Circle {
            centre: Vec2::new(12.5, 12.5),
            radius: 1.0,
        }
    }

    fn level_3_under_small_circle(z: i32) -> VoxelKey {
        VoxelKey {
            level: 3,
            x: 4,
            y: 4,
            z,
        }
    }

    #[test]
    fn z_range_under_should_prefer_the_finest_known_nodes() {
        let octree = full_octree(3, 1000);
        let known = HashMap::from([
            // The whole site, including a tall building far away.
            (VoxelKey::ROOT, (-90.0, 80.0)),
            (level_3_under_small_circle(1), (-70.0, -60.0)),
            (level_3_under_small_circle(2), (-55.0, -40.0)),
        ]);
        let range = z_range_under(&octree, &small_circle(), &known);
        assert_eq!(range, Some((-70.0, -40.0)));
    }

    #[test]
    fn z_range_under_should_ignore_known_nodes_away_from_the_footprint() {
        let octree = full_octree(3, 1000);
        let elsewhere = VoxelKey {
            level: 3,
            x: 0,
            y: 0,
            z: 0,
        };
        let known = HashMap::from([(elsewhere, (-100.0, -90.0))]);
        assert_eq!(z_range_under(&octree, &small_circle(), &known), None);
    }

    #[test]
    fn node_bounds_should_clip_cube_to_data_bounds() {
        // A flat site: 2 m of data inside a 200 m cube.
        let flat = (Vec3::new(-100.0, -100.0, -1.0), Vec3::new(100.0, 100.0, 1.0));
        let octree = Octree::new(HALFSIZE, 10.0, flat, HashMap::new());
        let (min, max) = octree.node_bounds(&VoxelKey::ROOT);
        assert_eq!((min.z, max.z), (-1.0, 1.0));
    }

    #[test]
    fn node_bounds_should_split_root_into_octants() {
        let octree = full_octree(1, 1);
        let key = VoxelKey {
            level: 1,
            x: 1,
            y: 0,
            z: 1,
        };
        assert_eq!(
            octree.node_bounds(&key),
            (Vec3::new(0.0, -100.0, 0.0), Vec3::new(100.0, 0.0, 100.0))
        );
    }
}
