//! An open cloud: the loader thread, what the UI knows about each node, and
//! choosing the nodes for each frame's views.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::TryRecvError;

use copc_streaming::VoxelKey;

use crate::cloud::CloudInfo;
use crate::heights::{HeightKind, HeightSource};
use crate::loader::{Event, Loader, OpenedCloud};
use crate::lod::{self, LodSettings, Octree, Selection};
use crate::renderer::{self, LineVertex, PointColor, PointRenderer, uses_16_bit_color};
use crate::subcloud_map;

use super::settings::ColorMode;

/// The GPU cache may hold this many times the point budget. The headroom keeps
/// recently seen nodes around, so looking back at them doesn't refetch.
const GPU_CACHE_FACTOR: u64 = 3;

// Debug bounds colours, as straight RGBA.
pub(crate) const FETCHING_COLOR: [f32; 4] = [1.0, 0.55, 0.1, 1.0];
pub(crate) const QUEUED_COLOR: [f32; 4] = [0.35, 0.65, 1.0, 0.8];
pub(crate) const LOADED_COLOR: [f32; 4] = [0.4, 0.9, 0.5, 0.3];
pub(crate) const FAILED_COLOR: [f32; 4] = [1.0, 0.2, 0.2, 1.0];

/// An open cloud: the loader thread plus what the UI knows about each node.
pub(crate) struct Session {
    pub(crate) loader: Loader,
    pub(crate) info: CloudInfo,
    pub(crate) octree: Octree,
    /// Nodes chosen for the current camera, most important first.
    pub(crate) selection: Selection,
    /// Nodes for the subcloud map, when it's shown. Mostly the same coarse
    /// nodes as `selection`, plus those for parts of the cloud out of view.
    pub(crate) map_selection: Selection,
    /// Nodes for the image being saved, if any. Chosen for its size, which
    /// can need more detail than the screen.
    pub(crate) export_selection: Option<Selection>,
    /// The height points are drawn at.
    pub(crate) shown: HeightKind,
    /// Frame the camera once the root node arrives: the shown height isn't
    /// in Z, so there's no telling where the cloud is until then.
    pub(crate) frame_on_root: bool,
    /// The last list sent to the loader: selected nodes that aren't loaded yet.
    pub(crate) requested: Vec<VoxelKey>,
    /// Nodes the loader is downloading right now.
    pub(crate) fetching: HashSet<VoxelKey>,
    /// Nodes that failed, with the reason. These aren't retried.
    pub(crate) failed: HashMap<VoxelKey, String>,
    /// Whether any colour value so far needed more than 8 bits. LAS says RGB
    /// is 16-bit, but some writers store 0..255, which would look black.
    pub(crate) rgb_is_16_bit: bool,
    /// Which LAS classes any loaded point has had, for the legend.
    pub(crate) classes_seen: [bool; 256],
}

impl Session {
    pub(crate) fn new(loader: Loader, opened: OpenedCloud) -> Self {
        let shown = opened.info.default_height();
        let mut session = Self {
            loader,
            info: opened.info,
            octree: opened.octree,
            selection: Selection::default(),
            map_selection: Selection::default(),
            export_selection: None,
            shown,
            frame_on_root: false,
            requested: Vec::new(),
            fetching: HashSet::new(),
            failed: HashMap::new(),
            rgb_is_16_bit: false,
            classes_seen: [false; 256],
        };
        session.show_height(shown);
        session.frame_on_root = !session.shown_is_z();
        session
    }

    /// Draws points at `kind` heights from now on.
    pub(crate) fn show_height(&mut self, kind: HeightKind) {
        self.shown = kind;
        self.octree
            .show_heights((!self.shown_is_z()).then_some(kind));
    }

    /// Whether the shown height is the file's Z, which the octree is built on.
    pub(crate) fn shown_is_z(&self) -> bool {
        self.info.heights[self.shown] == Some(HeightSource::Z)
    }

    /// Uploads newly loaded nodes. Errors if the loader can't continue.
    pub(crate) fn receive(&mut self, renderer: &mut PointRenderer) -> Result<(), String> {
        loop {
            match self.loader.try_recv() {
                Ok(Event::Fetching(keys)) => self.fetching.extend(keys),
                Ok(Event::Node { key, result }) => {
                    self.fetching.remove(&key);
                    match result {
                        Ok(node) => {
                            let colors = node.colors.as_deref();
                            self.rgb_is_16_bit |= colors.is_some_and(uses_16_bit_color);
                            renderer.insert_node(key, &node.points, colors, &node.classes);
                            for &class in &node.classes {
                                self.classes_seen[class as usize] = true;
                            }
                            self.octree.record_node_heights(key, &node.points);
                        }
                        Err(error) => {
                            self.failed.insert(key, error);
                        }
                    }
                }
                Ok(Event::Failed(error)) => return Err(error.to_string()),
                // Only sent once, before the session existed.
                Ok(Event::Opened(_)) => {}
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    return Err("the loader thread crashed".to_owned());
                }
            }
        }
    }

    /// Picks the nodes for this view (and the map's and image being saved,
    /// if given), asks the loader for missing ones and
    /// frees GPU memory if the cache is over budget.
    pub(crate) fn update(
        &mut self,
        renderer: &mut PointRenderer,
        views: Views<'_>,
        settings: &LodSettings,
    ) {
        self.selection = lod::select_nodes(&self.octree, views.main, settings);
        self.map_selection = views
            .map
            .map(|view| lod::select_nodes(&self.octree, view, &subcloud_map::LOD))
            .unwrap_or_default();
        self.export_selection = views
            .export
            .map(|view| lod::select_nodes(&self.octree, view, settings));
        let export_keys = self.export_selection.iter().flat_map(|s| &s.keys);

        // The 3D view's nodes go first, so the map never delays
        // them, except for an image being saved, which the user is waiting for.
        let not_loaded =
            |key: &&VoxelKey| !renderer.has_node(key) && !self.failed.contains_key(key);
        // `insert` returns false for a key already seen, which drops duplicates.
        let mut seen = HashSet::new();
        let wanted: Vec<VoxelKey> = export_keys
            .clone()
            .chain(&self.selection.keys)
            .filter(&not_loaded)
            .chain(self.map_selection.keys.iter().filter(&not_loaded))
            .filter(|key| seen.insert(**key))
            .copied()
            .collect();
        // Only message the loader when something changed, not every frame.
        if wanted != self.requested {
            self.loader.want(wanted.clone());
            self.requested = wanted;
        }

        let keep: HashSet<VoxelKey> = self
            .selection
            .keys
            .iter()
            .chain(&self.map_selection.keys)
            .chain(export_keys)
            .copied()
            .collect();
        let cache_budget = settings.point_budget * GPU_CACHE_FACTOR;
        for key in renderer.evict_to_budget(cache_budget, &keep) {
            self.loader.evicted(key);
        }
    }

    /// The colouring to draw with: RGB only if the file has it.
    pub(crate) fn point_color(&self, mode: ColorMode) -> PointColor {
        match mode {
            ColorMode::Rgb if self.info.has_rgb => PointColor::rgb(self.rgb_is_16_bit),
            ColorMode::Classification => PointColor::Classification,
            _ => PointColor::Height,
        }
    }

    /// How many of the image's nodes have loaded (or failed), of how many.
    /// `None` until nodes have been chosen for it.
    pub(crate) fn export_progress(&self, renderer: &PointRenderer) -> Option<(usize, usize)> {
        let keys = &self.export_selection.as_ref()?.keys;
        let done = keys
            .iter()
            .filter(|key| renderer.has_node(key) || self.failed.contains_key(key))
            .count();
        Some((done, keys.len()))
    }

    /// Points in selected nodes that are loaded, i.e. actually drawn.
    pub(crate) fn drawn_points(&self, renderer: &PointRenderer) -> u64 {
        self.selection
            .keys
            .iter()
            .filter(|key| renderer.has_node(key))
            .filter_map(|key| self.octree.point_count(key))
            .map(u64::from)
            .sum()
    }

    /// Wireframe cubes for the selected nodes (and any still downloading),
    /// coloured by load status.
    pub(crate) fn bounds_lines(
        &self,
        renderer: &PointRenderer,
        include_loaded: bool,
    ) -> Vec<LineVertex> {
        let color = |key: &VoxelKey| {
            if self.fetching.contains(key) {
                Some(FETCHING_COLOR)
            } else if self.failed.contains_key(key) {
                Some(FAILED_COLOR)
            } else if renderer.has_node(key) {
                include_loaded.then_some(LOADED_COLOR)
            } else {
                Some(QUEUED_COLOR)
            }
        };
        // A download that started before the camera moved can finish after its
        // node left the selection; it's still in flight, so show it.
        let unselected_fetches = self
            .fetching
            .iter()
            .filter(|key| !self.selection.keys.contains(key));
        self.selection
            .keys
            .iter()
            .chain(unselected_fetches)
            .filter_map(|key| {
                let (min, max) = self.octree.node_bounds(key);
                Some(renderer::box_lines(min, max, color(key)?))
            })
            .flatten()
            .collect()
    }
}

/// The views `Session::update` chooses nodes for.
pub(crate) struct Views<'a> {
    pub(crate) main: &'a lod::View,
    pub(crate) map: Option<&'a lod::View>,
    pub(crate) export: Option<&'a lod::View>,
}
