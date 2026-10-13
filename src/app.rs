use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::mpsc::TryRecvError;

use copc_streaming::VoxelKey;
use eframe::egui;

use crate::camera::OrbitCamera;
use crate::cloud::CloudInfo;
use crate::loader::{Event, Loader, OpenedCloud};
use crate::lod::{self, LodSettings, Octree, Selection};
use crate::renderer::{self, EdlSettings, LineVertex, PointColor, PointRenderer, PointStyle};

/// The GPU cache may hold this many times the point budget. The headroom keeps
/// recently seen nodes around, so looking back at them doesn't refetch.
const GPU_CACHE_FACTOR: u64 = 3;

// Debug bounds colours, as straight RGBA.
const FETCHING_COLOR: [f32; 4] = [1.0, 0.55, 0.1, 1.0];
const QUEUED_COLOR: [f32; 4] = [0.35, 0.65, 1.0, 0.8];
const LOADED_COLOR: [f32; 4] = [0.4, 0.9, 0.5, 0.3];
const FAILED_COLOR: [f32; 4] = [1.0, 0.2, 0.2, 1.0];

pub struct ViewerApp {
    url_input: String,
    state: LoadState,
    renderer: PointRenderer,
    camera: OrbitCamera,
    settings: ViewSettings,
}

/// Everything the user can tweak in the floating window.
struct ViewSettings {
    /// Point diameter in logical pixels (scaled by the display's DPI when drawn).
    point_size: f32,
    lod: LodSettings,
    /// Draw the octree cube of each node that's wanted or being downloaded.
    show_bounds: bool,
    /// Also draw the cubes of nodes that are already loaded.
    show_loaded_bounds: bool,
    /// Preferred colouring. RGB falls back to height for files without colour.
    color_mode: ColorMode,
    edl_enabled: bool,
    edl: EdlSettings,
}

#[derive(Clone, Copy, PartialEq)]
enum ColorMode {
    Height,
    Rgb,
}

enum LoadState {
    Idle,
    /// The loader thread is reading the header and hierarchy.
    Opening { source: String, loader: Loader },
    /// Boxed because a session is much bigger than the other variants.
    Open(Box<Session>),
    Failed { source: String, error: String },
}

/// An open cloud: the loader thread plus what the UI knows about each node.
struct Session {
    loader: Loader,
    info: CloudInfo,
    octree: Octree,
    /// Nodes chosen for the current camera, most important first.
    selection: Selection,
    /// The last list sent to the loader: selected nodes that aren't loaded yet.
    requested: Vec<VoxelKey>,
    /// Nodes the loader is downloading right now.
    fetching: HashSet<VoxelKey>,
    /// Nodes that failed, with the reason. These aren't retried.
    failed: HashMap<VoxelKey, String>,
    /// Whether any colour value so far needed more than 8 bits. LAS says RGB
    /// is 16-bit, but some writers store 0..255, which would look black.
    rgb_is_16_bit: bool,
}

impl ViewerApp {
    /// `initial_source`, if given, starts loading immediately.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_source: Option<String>,
    ) -> Result<Self, &'static str> {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("the wgpu renderer is not available")?;
        #[cfg(windows)]
        crate::windows_dpi::defer_dpi_changes_while_dragging(cc);
        let mut app = Self {
            url_input: initial_source.unwrap_or_default(),
            state: LoadState::Idle,
            renderer: PointRenderer::new(render_state),
            camera: OrbitCamera::default(),
            settings: ViewSettings {
                point_size: 2.0,
                lod: LodSettings::default(),
                show_bounds: false,
                show_loaded_bounds: false,
                color_mode: ColorMode::Rgb,
                edl_enabled: true,
                edl: EdlSettings {
                    strength: 1.0,
                    radius_px: 1.4,
                },
            },
        };
        if !app.url_input.trim().is_empty() {
            app.start_loading(&cc.egui_ctx);
        }
        Ok(app)
    }

    fn start_loading(&mut self, ctx: &egui::Context) {
        let source = self.url_input.trim().to_owned();
        let loader = Loader::spawn(source.clone(), ctx.clone());
        // Replacing the state drops any previous loader, which stops its thread.
        self.state = LoadState::Opening { source, loader };
        self.renderer.clear_nodes();
    }

    /// Applies whatever the loader thread has sent since the last frame.
    fn poll_loader(&mut self) {
        // Take the state out (leaving `Idle` behind) so the match owns it and
        // can move the loader into the next state, then put the result back.
        self.state = match mem::replace(&mut self.state, LoadState::Idle) {
            LoadState::Opening { source, loader } => self.poll_opening(source, loader),
            LoadState::Open(mut session) => match session.receive(&mut self.renderer) {
                Ok(()) => LoadState::Open(session),
                Err(error) => LoadState::Failed {
                    source: session.info.source.clone(),
                    error,
                },
            },
            other => other,
        };
    }

    fn poll_opening(&mut self, source: String, loader: Loader) -> LoadState {
        match loader.try_recv() {
            Ok(Event::Opened(opened)) => {
                let (min, max) = opened.info.local_bounds();
                self.renderer.set_z_range([min.z, max.z]);
                self.camera.frame_bounds(min, max);
                LoadState::Open(Box::new(Session::new(loader, *opened)))
            }
            Ok(Event::Failed(error)) => LoadState::Failed {
                source,
                error: error.to_string(),
            },
            // Node events only come after `Opened`, once the UI asks for nodes.
            Ok(Event::Fetching(_) | Event::Node { .. }) | Err(TryRecvError::Empty) => {
                LoadState::Opening { source, loader }
            }
            Err(TryRecvError::Disconnected) => LoadState::Failed {
                source,
                error: "the loader thread crashed".to_owned(),
            },
        }
    }

    fn url_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("COPC URL:");
            let is_loading = matches!(self.state, LoadState::Opening { .. });
            let load_button_width = 60.0;
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.url_input)
                    .hint_text("https://… or C:\\path\\to\\file.copc.laz")
                    .desired_width(ui.available_width() - load_button_width),
            );
            let enter_pressed =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let clicked = ui
                .add_enabled(!is_loading, egui::Button::new("Load"))
                .clicked();
            if (clicked || enter_pressed) && !is_loading {
                self.start_loading(ui.ctx());
            }
        });
    }
}

impl Session {
    fn new(loader: Loader, opened: OpenedCloud) -> Self {
        Self {
            loader,
            info: opened.info,
            octree: opened.octree,
            selection: Selection::default(),
            requested: Vec::new(),
            fetching: HashSet::new(),
            failed: HashMap::new(),
            rgb_is_16_bit: false,
        }
    }

    /// Uploads newly loaded nodes. Errors if the loader can't continue.
    fn receive(&mut self, renderer: &mut PointRenderer) -> Result<(), String> {
        loop {
            match self.loader.try_recv() {
                Ok(Event::Fetching(keys)) => self.fetching.extend(keys),
                Ok(Event::Node { key, result }) => {
                    self.fetching.remove(&key);
                    match result {
                        Ok(node) => {
                            let colors = node.colors.as_deref();
                            self.rgb_is_16_bit |= colors.is_some_and(uses_16_bit_color);
                            renderer.insert_node(key, &node.positions, colors);
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

    /// Picks the nodes for this view, asks the loader for missing ones and
    /// frees GPU memory if the cache is over budget.
    fn update(&mut self, renderer: &mut PointRenderer, view: &lod::View, settings: &LodSettings) {
        self.selection = lod::select_nodes(&self.octree, view, settings);

        let wanted: Vec<VoxelKey> = self
            .selection
            .keys
            .iter()
            .filter(|key| !renderer.has_node(key) && !self.failed.contains_key(key))
            .copied()
            .collect();
        // Only message the loader when something changed, not every frame.
        if wanted != self.requested {
            self.loader.want(wanted.clone());
            self.requested = wanted;
        }

        let keep: HashSet<VoxelKey> = self.selection.keys.iter().copied().collect();
        let cache_budget = settings.point_budget * GPU_CACHE_FACTOR;
        for key in renderer.evict_to_budget(cache_budget, &keep) {
            self.loader.evicted(key);
        }
    }

    /// The colouring to draw with: RGB only if the file has it.
    fn point_color(&self, mode: ColorMode) -> PointColor {
        match mode {
            ColorMode::Rgb if self.info.has_rgb => PointColor::Rgb {
                // Stretch 0..255 stored in 16 bits to the full range.
                scale: if self.rgb_is_16_bit { 1.0 } else { 257.0 },
            },
            _ => PointColor::Height,
        }
    }

    /// Points in selected nodes that are loaded, i.e. actually drawn.
    fn drawn_points(&self, renderer: &PointRenderer) -> u64 {
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
    fn bounds_lines(&self, renderer: &PointRenderer, include_loaded: bool) -> Vec<LineVertex> {
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

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_loader();

        egui::Panel::top("url_bar").show(ui, |ui| self.url_bar(ui));

        // The 3D view fills its panel edge to edge; the text states keep margins.
        let frame = match self.state {
            LoadState::Open(_) => egui::Frame::NONE,
            _ => egui::Frame::central_panel(ui.style()),
        };
        let central = egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| match &mut self.state {
                LoadState::Idle => {
                    ui.label("Paste a COPC URL or file path above and press Load.");
                }
                LoadState::Opening { source, .. } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Loading {source}"));
                    });
                }
                LoadState::Open(session) => point_view(
                    ui,
                    &mut self.renderer,
                    &mut self.camera,
                    session,
                    &self.settings,
                ),
                LoadState::Failed { source, error } => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Failed to load {source}"),
                    );
                    ui.label(error.as_str());
                }
            });

        // Floats over the 3D view rather than taking a side panel, so long
        // sources can't squeeze the viewport. Shown after the central panel so
        // its default position can sit just inside the viewport's corner.
        if let LoadState::Open(session) = &self.state {
            egui::Window::new("Point cloud")
                .default_pos(central.response.rect.min + egui::vec2(8.0, 8.0))
                .collapsible(true)
                .resizable(false)
                .max_width(360.0)
                .show(ui.ctx(), |ui| {
                    cloud_summary(ui, &session.info);
                    ui.separator();
                    streaming_stats(ui, session, &self.renderer);
                    ui.separator();
                    settings_ui(ui, &mut self.settings, session.info.has_rgb);
                    ui.separator();
                    ui.label(
                        "Drag: orbit\nRight-drag: pan\nScroll: zoom\nDouble-click: reset view",
                    );
                });
        }
    }
}

/// The interactive 3D viewport. Takes fields separately, rather than `&mut self`,
/// so the caller can keep borrowing `self.state` for `session` at the same time.
fn point_view(
    ui: &mut egui::Ui,
    renderer: &mut PointRenderer,
    camera: &mut OrbitCamera,
    session: &mut Session,
    settings: &ViewSettings,
) {
    let (rect, response) =
        ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

    let drag = glam::vec2(response.drag_delta().x, response.drag_delta().y);
    if response.dragged_by(egui::PointerButton::Primary) {
        camera.orbit(drag);
    }
    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        camera.pan(drag, rect.height());
    }
    if response.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 {
            camera.zoom(scroll);
        }
    }
    if response.double_clicked() {
        let (min, max) = session.info.local_bounds();
        camera.frame_bounds(min, max);
    }

    // Render at physical pixel resolution so points stay crisp on high-DPI screens.
    let pixels_per_point = ui.ctx().pixels_per_point();
    let size_px = [
        (rect.width() * pixels_per_point).round() as u32,
        (rect.height() * pixels_per_point).round() as u32,
    ];

    session.update(renderer, &lod::View::new(camera, size_px), &settings.lod);
    let lines = if settings.show_bounds {
        session.bounds_lines(renderer, settings.show_loaded_bounds)
    } else {
        Vec::new()
    };

    // Sizes are set in logical pixels, so scale them like the point size.
    let style = PointStyle {
        size_px: settings.point_size * pixels_per_point,
        color: session.point_color(settings.color_mode),
        edl: settings.edl_enabled.then_some(EdlSettings {
            radius_px: settings.edl.radius_px * pixels_per_point,
            ..settings.edl
        }),
    };
    let texture_id = renderer.render(size_px, camera, &style, &session.selection.keys, &lines);
    let full_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter()
        .image(texture_id, rect, full_uv, egui::Color32::WHITE);
}

fn cloud_summary(ui: &mut egui::Ui, info: &CloudInfo) {
    let [min_x, min_y, min_z] = info.bounds_min;
    let [max_x, max_y, max_z] = info.bounds_max;
    let [ox, oy, oz] = info.origin;

    egui::Grid::new("cloud_summary")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            // Full URLs run to hundreds of characters; the file name is enough
            // to identify the cloud, with the full source on hover.
            ui.label("Source");
            ui.add(egui::Label::new(file_name(&info.source)).truncate())
                .on_hover_text(&info.source);
            ui.end_row();

            ui.label("Total points");
            ui.label(format_count(info.total_points));
            ui.end_row();

            ui.label("Extent (m)");
            ui.label(format!(
                "{:.1} × {:.1} × {:.1}",
                max_x - min_x,
                max_y - min_y,
                max_z - min_z
            ));
            ui.end_row();

            ui.label("Octree centre");
            ui.label(format!("{ox:.2}, {oy:.2}, {oz:.2}"));
            ui.end_row();

            ui.label("Open time");
            ui.label(format!("{:.2} s", info.open_time.as_secs_f64()));
            ui.end_row();
        });
}

fn streaming_stats(ui: &mut egui::Ui, session: &Session, renderer: &PointRenderer) {
    let selected = session.selection.keys.len();
    let drawn = session
        .selection
        .keys
        .iter()
        .filter(|key| renderer.has_node(key))
        .count();

    egui::Grid::new("streaming_stats")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label("Points drawn");
            ui.label(format!(
                "{} of {} selected",
                format_count(session.drawn_points(renderer)),
                format_count(session.selection.points)
            ));
            ui.end_row();

            ui.label("Nodes drawn");
            ui.label(format!(
                "{drawn} of {selected} selected ({} in file)",
                session.octree.node_count()
            ));
            ui.end_row();

            ui.label("Requested");
            ui.label(format!(
                "{} nodes ({} downloading)",
                session.requested.len(),
                session.fetching.len()
            ));
            ui.end_row();

            ui.label("GPU cache");
            ui.label(format!(
                "{} nodes, {} points",
                renderer.node_count(),
                format_count(renderer.resident_points())
            ));
            ui.end_row();

            if let Some(error) = session.failed.values().next() {
                ui.colored_label(ui.visuals().error_fg_color, "Failed");
                ui.label(format!("{} nodes", session.failed.len()))
                    .on_hover_text(error);
                ui.end_row();
            }
        });
}

fn settings_ui(ui: &mut egui::Ui, settings: &mut ViewSettings, has_rgb: bool) {
    ui.horizontal(|ui| {
        ui.label("Colour:");
        ui.radio_value(&mut settings.color_mode, ColorMode::Height, "Height");
        ui.add_enabled_ui(has_rgb, |ui| {
            ui.radio_value(&mut settings.color_mode, ColorMode::Rgb, "RGB")
                .on_disabled_hover_text("This file has no colour");
        });
    });
    ui.add(egui::Slider::new(&mut settings.point_size, 1.0..=10.0).text("Point size"));

    ui.checkbox(&mut settings.edl_enabled, "Eye-dome lighting")
        .on_hover_text("Shades points by how much nearer their neighbours are, to show depth.");
    if settings.edl_enabled {
        ui.indent("edl_options", |ui| {
            ui.add(egui::Slider::new(&mut settings.edl.strength, 0.1..=5.0).text("Strength"));
            ui.add(egui::Slider::new(&mut settings.edl.radius_px, 1.0..=4.0).text("Radius (px)"));
        });
    }

    ui.separator();
    ui.add(
        egui::Slider::new(&mut settings.lod.point_budget, 500_000..=30_000_000)
            .logarithmic(true)
            .custom_formatter(|value, _| format_count(value as u64))
            .text("Point budget"),
    );
    ui.add(
        egui::Slider::new(&mut settings.lod.target_spacing_px, 0.5..=20.0)
            .logarithmic(true)
            .text("Target spacing (px)"),
    )
    .on_hover_text(
        "Nodes are refined until their points are at most this far apart on screen. \
         Lower means more detail, up to the point budget.",
    );

    ui.checkbox(&mut settings.show_bounds, "Show node bounds");
    if settings.show_bounds {
        ui.indent("bounds_options", |ui| {
            ui.checkbox(&mut settings.show_loaded_bounds, "Include loaded nodes");
            ui.horizontal_wrapped(|ui| {
                for (label, color) in [
                    ("downloading", FETCHING_COLOR),
                    ("queued", QUEUED_COLOR),
                    ("loaded", LOADED_COLOR),
                    ("failed", FAILED_COLOR),
                ] {
                    ui.colored_label(legend_color(color), format!("■ {label}"));
                }
            });
        });
    }
}

/// Whether colours use the 16-bit range LAS specifies, rather than 0..255.
fn uses_16_bit_color(colors: &[[u16; 4]]) -> bool {
    colors.iter().any(|[r, g, b, _]| (*r).max(*g).max(*b) > 255)
}

/// Opaque version of a debug line colour, for the legend.
fn legend_color([r, g, b, _]: [f32; 4]) -> egui::Color32 {
    let to_byte = |channel: f32| (channel * 255.0).round() as u8;
    egui::Color32::from_rgb(to_byte(r), to_byte(g), to_byte(b))
}

/// Short human-readable counts: `950`, `61.2 k`, `10.65 M`.
fn format_count(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{:.1} k", count as f64 / 1e3),
        _ => format!("{:.2} M", count as f64 / 1e6),
    }
}

/// The last segment of a URL or path, ignoring any query string.
fn file_name(source: &str) -> &str {
    let without_query = source.split(['?', '#']).next().unwrap_or(source);
    without_query
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(source)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_should_take_last_url_segment_without_query() {
        assert_eq!(
            file_name("https://example.com/a/b/site.copc.laz?sig=abc"),
            "site.copc.laz"
        );
    }

    #[test]
    fn file_name_should_handle_windows_paths() {
        assert_eq!(file_name(r"C:\data\site 1.copc.laz"), "site 1.copc.laz");
    }

    #[test]
    fn uses_16_bit_color_should_be_false_for_8_bit_values() {
        assert!(!uses_16_bit_color(&[[255, 128, 0, u16::MAX]]));
    }

    #[test]
    fn uses_16_bit_color_should_be_true_above_255() {
        assert!(uses_16_bit_color(&[[0, 0, 0, u16::MAX], [256, 0, 0, u16::MAX]]));
    }

    #[test]
    fn format_count_should_use_millions_for_large_counts() {
        assert_eq!(format_count(10_653_336), "10.65 M");
    }

    #[test]
    fn format_count_should_use_thousands_for_node_sized_counts() {
        assert_eq!(format_count(61_201), "61.2 k");
    }
}
