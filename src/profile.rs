//! The vertical density profile: how many of a subcloud's points are in each
//! height band. It counts every point, so it loads the subcloud at full
//! resolution (every octree level under it, not just the ones being drawn),
//! up to a point limit.

use std::collections::{HashMap, HashSet};

use copc_streaming::VoxelKey;
use eframe::egui::{self, CornerRadius, Rect, Sense, Stroke};

use crate::app::format_count;
use crate::lod::{self, Octree, Selection};
use crate::subcloud::Footprint;
use crate::subcloud_map::SUBCLOUD_COLOR;

pub const DEFAULT_BIN_SIZE: f32 = 0.5;
pub const MIN_BIN_SIZE: f32 = 0.05;
/// Most points the profile loads, unless the user changes it.
pub const DEFAULT_POINT_LIMIT: u64 = 2_000_000;

/// What the user asked the profile to cover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfileRequest {
    pub footprint: Footprint,
    /// Give up if the nodes under the footprint hold more points than this.
    pub point_limit: u64,
}

/// The full-resolution points under a footprint, as they arrive from the
/// loader, and the profile made from them.
#[derive(Default)]
pub struct ProfilePoints {
    /// What `nodes` was chosen for.
    request: Option<ProfileRequest>,
    /// The request changed this frame. The heights are only refiltered once
    /// it holds still, so dragging the shape stays smooth.
    moving: bool,
    /// The nodes under the footprint, or `None` if they hold too many points.
    nodes: Option<Selection>,
    /// The same keys as `nodes`, for quick lookups.
    wanted: HashSet<VoxelKey>,
    /// Copies of the wanted nodes' positions, kept to refilter when the
    /// footprint changes.
    positions: HashMap<VoxelKey, Vec<[f32; 3]>>,
    /// Wanted nodes that were already on the GPU before the profile asked
    /// for them, so the loader was told to fetch them again.
    refetching: HashSet<VoxelKey>,
    /// Heights of the points inside the footprint. `None` until they're
    /// filtered for the current footprint.
    heights: Option<Vec<f32>>,
    /// Kept between frames, and while the shape moves, until there's news.
    histogram: Option<Histogram>,
    histogram_is_stale: bool,
}

/// How far along loading the profile's points is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// The nodes under the footprint hold more points than the limit.
    TooMany,
    Loading {
        /// Nodes received so far.
        loaded: usize,
        /// Nodes that couldn't be loaded, so the profile will miss them.
        failed: usize,
        total: usize,
        /// Points in all `total` nodes, inside the footprint or not.
        points: u64,
    },
}

impl ProfilePoints {
    /// Chooses the nodes for `request`, dropping the profile when `None`.
    /// Called every frame; does nothing unless the request changed.
    pub fn set_request(&mut self, octree: &Octree, request: Option<ProfileRequest>) {
        self.moving = request != self.request;
        if !self.moving {
            return;
        }
        let Some(request) = request else {
            *self = Self::default();
            return;
        };
        self.request = Some(request);
        self.nodes = lod::nodes_in_footprint(octree, &request.footprint, request.point_limit);
        self.wanted = self
            .nodes
            .iter()
            .flat_map(|nodes| nodes.keys.iter().copied())
            .collect();
        // `retain` keeps the entries the closure returns true for.
        self.positions.retain(|key, _| self.wanted.contains(key));
        self.heights = None;
    }

    /// Wanted nodes that haven't arrived yet, coarsest first.
    pub fn missing(&self) -> impl Iterator<Item = &VoxelKey> {
        self.nodes
            .iter()
            .flat_map(|nodes| &nodes.keys)
            .filter(|key| !self.positions.contains_key(key))
    }

    /// Wanted nodes that `is_resident` says are on the GPU, so they arrived
    /// before the profile wanted them and need fetching again. Each is only
    /// returned once.
    pub fn take_refetches(&mut self, is_resident: impl Fn(&VoxelKey) -> bool) -> Vec<VoxelKey> {
        let refetches: Vec<VoxelKey> = self
            .missing()
            .filter(|key| is_resident(key) && !self.refetching.contains(key))
            .copied()
            .collect();
        self.refetching.extend(&refetches);
        refetches
    }

    /// Keeps a newly loaded node's positions, if the profile wants them.
    pub fn receive(&mut self, key: VoxelKey, positions: Vec<[f32; 3]>) {
        self.refetching.remove(&key);
        if !self.wanted.contains(&key) {
            return;
        }
        // Add the new node's heights, unless they'll all be refiltered anyway.
        if let (Some(heights), Some(request)) = (&mut self.heights, &self.request) {
            heights.extend(heights_inside(&request.footprint, &positions));
            self.histogram_is_stale = true;
        }
        self.positions.insert(key, positions);
    }

    pub fn status(&self, failed: impl Fn(&VoxelKey) -> bool) -> Status {
        let Some(nodes) = &self.nodes else {
            return Status::TooMany;
        };
        Status::Loading {
            loaded: self.positions.len(),
            failed: self.missing().filter(|key| failed(key)).count(),
            total: nodes.keys.len(),
            points: nodes.points,
        }
    }

    /// The profile so far, with bands `bin_size` metres tall. While the
    /// shape moves this is the last one made, until it holds still.
    pub fn histogram(&mut self, bin_size: f32) -> Option<&Histogram> {
        if !self.moving
            && self.heights.is_none()
            && let Some(request) = &self.request
        {
            let heights = self
                .positions
                .values()
                .flat_map(|positions| heights_inside(&request.footprint, positions))
                .collect();
            self.heights = Some(heights);
            self.histogram_is_stale = true;
        }
        let bin_size_changed = self
            .histogram
            .as_ref()
            .is_some_and(|histogram| histogram.bin_size != bin_size);
        if (self.histogram_is_stale || bin_size_changed)
            && let Some(heights) = &self.heights
        {
            self.histogram = Histogram::new(heights, bin_size);
            self.histogram_is_stale = false;
        }
        self.histogram.as_ref()
    }
}

/// The heights of the points in `positions` that are inside `footprint`.
fn heights_inside(footprint: &Footprint, positions: &[[f32; 3]]) -> impl Iterator<Item = f32> {
    positions
        .iter()
        .filter(|[x, y, _]| footprint.contains(glam::vec2(*x, *y)))
        .map(|[_, _, z]| *z)
}

/// Point counts in height bands, from the lowest point up.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub bin_size: f32,
    /// Local height of the lowest point, where the first band starts.
    pub lowest: f32,
    /// Points in each band, bottom first.
    pub counts: Vec<u64>,
    pub total: u64,
}

impl Histogram {
    /// `None` when there are no heights to count.
    pub fn new(heights: &[f32], bin_size: f32) -> Option<Self> {
        let lowest = heights.iter().copied().reduce(f32::min)?;
        let mut counts = Vec::new();
        for height in heights {
            // `as` rounds towards zero, which is a floor for positive values.
            let bin = ((height - lowest) / bin_size) as usize;
            if bin >= counts.len() {
                counts.resize(bin + 1, 0);
            }
            counts[bin] += 1;
        }
        Some(Self {
            bin_size,
            lowest,
            total: heights.len() as u64,
            counts,
        })
    }

    /// From the lowest point to the top of the highest band.
    pub fn height(&self) -> f32 {
        self.counts.len() as f32 * self.bin_size
    }
}

/// Room around the plot for the axis labels, in logical pixels.
const LEFT_MARGIN: f32 = 44.0;
const BOTTOM_MARGIN: f32 = 20.0;
const TOP_MARGIN: f32 = 8.0;
const RIGHT_MARGIN: f32 = 16.0;
/// Bars thinner than this on screen are drawn without a gap between them.
const MIN_BAR_HEIGHT_FOR_GAP: f32 = 4.0;

/// Draws `histogram` as horizontal bars, height up the side and point count
/// along the bottom, in the rest of `ui`.
pub fn chart(ui: &mut egui::Ui, histogram: &Histogram) {
    let size = ui.available_size().max(egui::vec2(160.0, 120.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    let plot = Rect::from_min_max(
        rect.min + egui::vec2(LEFT_MARGIN, TOP_MARGIN),
        rect.max - egui::vec2(RIGHT_MARGIN, BOTTOM_MARGIN),
    );
    let visuals = ui.visuals();
    let font = egui::TextStyle::Small.resolve(ui.style());
    let label_color = visuals.weak_text_color();
    let grid = Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);

    let max_count = histogram.counts.iter().copied().max().unwrap_or(1).max(1);
    // Whole points only: half a point makes no sense as a tick.
    let count_step = nice_step(max_count as f32 / 4.0).max(1.0);
    let count_max = (max_count as f32 / count_step).ceil() * count_step;
    let height_max = histogram.height();
    let x = |count: f32| plot.left() + count / count_max * plot.width();
    let y = |height: f32| plot.bottom() - height / height_max * plot.height();

    // Recessive grid lines and labels first, so the bars sit on top.
    let height_step = nice_step(height_max / 6.0);
    // Enough decimals to tell the ticks apart: none for whole metres.
    let decimals = (-height_step.log10()).ceil().max(0.0) as usize;
    for i in 0..=(height_max / height_step).floor() as u32 {
        let height = i as f32 * height_step;
        let at = y(height);
        painter.hline(plot.x_range(), at, grid);
        painter.text(
            egui::pos2(plot.left() - 6.0, at),
            egui::Align2::RIGHT_CENTER,
            format!("{height:.decimals$} m"),
            font.clone(),
            label_color,
        );
    }
    for i in 0..=(count_max / count_step).round() as u32 {
        let count = i as f32 * count_step;
        let at = x(count);
        painter.vline(at, plot.y_range(), grid);
        painter.text(
            egui::pos2(at, plot.bottom() + 4.0),
            egui::Align2::CENTER_TOP,
            format_count(count as u64),
            font.clone(),
            label_color,
        );
    }

    let band_px = plot.height() / histogram.counts.len() as f32;
    let gap = if band_px >= MIN_BAR_HEIGHT_FOR_GAP {
        2.0
    } else {
        0.0
    };
    let hovered_bin = response
        .hover_pos()
        .filter(|pos| plot.y_range().contains(pos.y))
        .map(|pos| ((plot.bottom() - pos.y) / band_px) as usize)
        .filter(|bin| *bin < histogram.counts.len());
    for (bin, &count) in histogram.counts.iter().enumerate() {
        let bottom = y(bin as f32 * histogram.bin_size);
        let band = Rect::from_x_y_ranges(plot.x_range(), (bottom - band_px)..=bottom);
        if hovered_bin == Some(bin) {
            // Highlight the whole band, so the hover target isn't just the bar.
            painter.rect_filled(band, 0.0, visuals.widgets.hovered.weak_bg_fill);
        }
        if count == 0 {
            continue;
        }
        let bar = Rect::from_min_max(
            egui::pos2(plot.left(), band.top() + gap * 0.5),
            egui::pos2(x(count as f32), band.bottom() - gap * 0.5),
        );
        // Round only the data end, leaving the bar square on the baseline.
        let radius = (bar.height() * 0.5).min(4.0) as u8;
        let corners = CornerRadius {
            ne: radius,
            se: radius,
            ..CornerRadius::ZERO
        };
        painter.rect_filled(bar, corners, SUBCLOUD_COLOR);
    }
    painter.vline(plot.left(), plot.y_range(), Stroke::new(1.0, label_color));

    if let Some(bin) = hovered_bin {
        let count = histogram.counts[bin];
        let from = bin as f32 * histogram.bin_size;
        let share = count as f64 / histogram.total.max(1) as f64 * 100.0;
        response.on_hover_ui_at_pointer(|ui| {
            ui.label(format!("{from:.2}–{:.2} m", from + histogram.bin_size));
            ui.strong(format!("{} points ({share:.1}%)", format_count(count)));
        });
    }
}

/// A round step near `rough` for axis ticks: 1, 2 or 5 times a power of 10.
fn nice_step(rough: f32) -> f32 {
    let rough = rough.max(1e-6);
    let power = 10_f32.powf(rough.log10().floor());
    let mantissa = rough / power;
    let nice = if mantissa <= 1.0 {
        1.0
    } else if mantissa <= 2.0 {
        2.0
    } else if mantissa <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * power
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn histogram_should_count_from_the_lowest_point() {
        let histogram = Histogram::new(&[10.0, 10.2, 10.6, 11.9], 0.5);
        assert_eq!(histogram.map(|h| h.counts), Some(vec![2, 1, 0, 1]));
    }

    #[test]
    fn histogram_should_be_none_without_points() {
        assert_eq!(Histogram::new(&[], 0.5), None);
    }

    #[test]
    fn nice_step_should_round_up_to_one_two_or_five() {
        let steps = [0.3, 1.5, 4.0, 7.0, 230.0].map(nice_step);
        let expected = [0.5, 2.0, 5.0, 10.0, 500.0];
        // Powers of ten aren't exact in binary, so allow a little slack.
        let close = steps
            .iter()
            .zip(expected)
            .all(|(step, expected)| (step - expected).abs() < expected * 1e-5);
        assert!(close, "{steps:?}");
    }

    fn circle() -> ProfileRequest {
        ProfileRequest {
            footprint: Footprint::Circle {
                centre: glam::Vec2::ZERO,
                radius: 1.0,
            },
            point_limit: u64::MAX,
        }
    }

    /// A single root node: a cube 100 m across around the origin.
    fn octree() -> Octree {
        let bounds = (glam::Vec3::splat(-50.0), glam::Vec3::splat(50.0));
        Octree::new(50.0, 1.0, bounds, HashMap::from([(VoxelKey::ROOT, 3)]))
    }

    #[test]
    fn histogram_should_only_count_points_inside_the_footprint() {
        let mut profile = ProfilePoints::default();
        profile.set_request(&octree(), Some(circle()));
        profile.receive(
            VoxelKey::ROOT,
            vec![[0.0, 0.0, 1.0], [0.5, 0.0, 2.0], [5.0, 0.0, 3.0]],
        );
        // The next frame, with the request holding still.
        profile.set_request(&octree(), Some(circle()));
        assert_eq!(profile.histogram(1.0).map(|h| h.total), Some(2));
    }

    #[test]
    fn take_refetches_should_return_each_resident_node_once() {
        let mut profile = ProfilePoints::default();
        profile.set_request(&octree(), Some(circle()));
        profile.take_refetches(|_| true);
        assert!(profile.take_refetches(|_| true).is_empty());
    }
}
