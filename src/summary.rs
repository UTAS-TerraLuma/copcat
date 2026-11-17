//! Reading a whole cloud through the viewer's loader, for tests and checks
//! that need to know exactly what the viewer would load.

use std::collections::BTreeMap;

use copc_streaming::VoxelKey;
use eframe::egui;

use crate::heights::HeightSettings;
use crate::loader::{Event, Loader, OpenedCloud};
use crate::lod::Octree;

/// Everything [`read_cloud`] loaded.
#[derive(Debug, Clone, PartialEq)]
pub struct CloudSummary {
    /// The point count in the file's header.
    pub header_points: u64,
    /// The bounds in the file's header, in its coordinates.
    pub bounds_min: [f64; 3],
    pub bounds_max: [f64; 3],
    pub has_rgb: bool,
    /// Octree nodes holding points.
    pub nodes: usize,
    /// Every point in every node, sorted so the order the nodes arrived in
    /// doesn't matter.
    pub points: Vec<SummaryPoint>,
}

/// One point as the viewer decoded it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SummaryPoint {
    /// In the file's coordinates.
    pub position: [f64; 3],
    pub class: u32,
    /// 16-bit RGB, if the file has colour.
    pub rgb: Option<[u16; 3]>,
}

impl CloudSummary {
    /// How many points have each LAS classification.
    pub fn class_counts(&self) -> BTreeMap<u32, usize> {
        let mut counts = BTreeMap::new();
        for point in &self.points {
            *counts.entry(point.class).or_default() += 1;
        }
        counts
    }
}

/// Opens `source` (a path or URL, as the viewer takes) and loads every node,
/// through the same loader thread, byte sources and decoder as the viewer.
pub fn read_cloud(source: &str) -> Result<CloudSummary, String> {
    let loader = Loader::spawn(
        source.to_owned(),
        HeightSettings::default(),
        egui::Context::default(),
    );
    let OpenedCloud { info, octree } = match loader.recv() {
        Some(Event::Opened(opened)) => *opened,
        Some(Event::Failed(error)) => return Err(error.to_string()),
        Some(_) => return Err("the loader sent nodes before opening the file".to_owned()),
        None => return Err("the loader thread crashed".to_owned()),
    };
    let keys = node_keys(&octree);
    loader.want(keys.clone());

    let origin = info.origin;
    let mut points = Vec::new();
    let mut waiting = keys.len();
    while waiting > 0 {
        match loader.recv() {
            Some(Event::Node { key, result }) => {
                waiting -= 1;
                let node = result.map_err(|error| format!("node {key:?}: {error}"))?;
                for (i, local) in node.points.positions.iter().enumerate() {
                    points.push(SummaryPoint {
                        position: std::array::from_fn(|axis| f64::from(local[axis]) + origin[axis]),
                        class: node.classes[i],
                        rgb: node.colors.as_ref().map(|colors| {
                            let [r, g, b, _] = colors[i];
                            [r, g, b]
                        }),
                    });
                }
            }
            Some(Event::Fetching(_) | Event::Opened(_)) => {}
            Some(Event::Failed(error)) => return Err(error.to_string()),
            None => return Err("the loader thread crashed".to_owned()),
        }
    }
    points.sort_by(|a, b| {
        let key = |p: &SummaryPoint| (p.position.map(f64::to_bits), p.class, p.rgb);
        key(a).cmp(&key(b))
    });
    Ok(CloudSummary {
        header_points: info.total_points,
        bounds_min: info.bounds_min,
        bounds_max: info.bounds_max,
        has_rgb: info.has_rgb,
        nodes: keys.len(),
        points,
    })
}

/// Every node in the hierarchy that holds points, parents first.
fn node_keys(octree: &Octree) -> Vec<VoxelKey> {
    let mut keys = Vec::new();
    let mut stack = vec![VoxelKey::ROOT];
    while let Some(key) = stack.pop() {
        let Some(count) = octree.point_count(&key) else {
            continue;
        };
        if count > 0 {
            keys.push(key);
        }
        for child in 0..8 {
            stack.push(VoxelKey {
                level: key.level + 1,
                x: key.x * 2 + (child & 1),
                y: key.y * 2 + ((child >> 1) & 1),
                z: key.z * 2 + ((child >> 2) & 1),
            });
        }
    }
    keys
}
