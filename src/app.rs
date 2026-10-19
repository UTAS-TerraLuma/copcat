use std::collections::{HashMap, HashSet};
use std::mem;
use std::sync::mpsc::TryRecvError;

use copc_streaming::VoxelKey;
use eframe::egui;

use crate::camera::{OrbitCamera, Projection, ViewPreset};
use crate::cloud::CloudInfo;
use crate::gizmo;
use crate::loader::{Event, Loader, OpenedCloud};
use crate::lod::{self, LodSettings, Octree, Selection};
use crate::profile::{self, ProfilePoints, ProfileRequest};
use crate::renderer::{
    self, EdlSettings, LineVertex, PointColor, PointRenderer, PointStyle, Viewport,
};
use crate::subcloud::{self, Footprint, ShapeKind, Subcloud};
use crate::subcloud_map::{self, MapFrame, SubcloudMap};

/// The GPU cache may hold this many times the point budget. The headroom keeps
/// recently seen nodes around, so looking back at them doesn't refetch.
const GPU_CACHE_FACTOR: u64 = 3;

/// Gap between the 3D view's edges and the gizmo, in logical pixels.
const OVERLAY_MARGIN: f32 = 10.0;

/// Numpad-style view keys, as in Blender: `(key, view, view with Ctrl)`.
const VIEW_SHORTCUTS: [(egui::Key, ViewPreset, ViewPreset); 3] = [
    (egui::Key::Num7, ViewPreset::Top, ViewPreset::Bottom),
    (egui::Key::Num1, ViewPreset::Front, ViewPreset::Back),
    (egui::Key::Num3, ViewPreset::Right, ViewPreset::Left),
];
const PROJECTION_SHORTCUT: egui::Key = egui::Key::Num5;

// Debug bounds colours, as straight RGBA.
const FETCHING_COLOR: [f32; 4] = [1.0, 0.55, 0.1, 1.0];
const QUEUED_COLOR: [f32; 4] = [0.35, 0.65, 1.0, 0.8];
const LOADED_COLOR: [f32; 4] = [0.4, 0.9, 0.5, 0.3];
const FAILED_COLOR: [f32; 4] = [1.0, 0.2, 0.2, 1.0];

pub struct ViewerApp {
    url_input: String,
    state: LoadState,
    renderer: PointRenderer,
    viewports: Viewports,
    /// The Explore mode camera.
    camera: OrbitCamera,
    settings: ViewSettings,
    mode: Mode,
    /// Kept while in Explore mode, so switching back returns to the same
    /// subcloud. Created the first time Subcloud mode is shown for a cloud.
    subcloud: Option<SubcloudMode>,
}

/// What the main area shows. Switched from the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Mode {
    /// The whole cloud in one 3D view.
    Explore,
    /// A map to draw a line, circle or square on, and a 3D view of just
    /// the points above it.
    Subcloud,
}

/// Where each image is rendered.
struct Viewports {
    /// The 3D view, in either mode.
    main: Viewport,
    /// The subcloud map.
    map: Viewport,
}

/// Subcloud mode's state, which lasts while the cloud is open.
struct SubcloudMode {
    subcloud: Subcloud,
    /// The 3D view of the subcloud. Separate from the Explore camera, so
    /// switching modes returns each view to where it was.
    camera: OrbitCamera,
    map: SubcloudMap,
    /// The cloud's local bounds.
    bounds: (glam::Vec3, glam::Vec3),
    /// Point `camera` side-on at the subcloud next frame, once the 3D
    /// view's aspect ratio is known.
    needs_aim: bool,
    /// How far one Ctrl+scroll notch slides the subcloud, as a percentage
    /// of its thickness.
    sweep_step_percent: f32,
    profile: ProfileSettings,
}

/// The density profile window's settings.
struct ProfileSettings {
    /// Whether the window is open. The profile's points are only loaded
    /// while it is.
    show: bool,
    /// Height of each band, in metres.
    bin_size: f32,
    point_limit: u64,
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
    /// Nodes for the subcloud map, when it's shown. Mostly the same coarse
    /// nodes as `selection`, plus those for parts of the cloud out of view.
    map_selection: Selection,
    /// Full-resolution points for the density profile, when it's shown.
    profile: ProfilePoints,
    /// The lowest and highest point of every node loaded so far, kept after
    /// the node is evicted (it's tiny), for framing subclouds.
    height_ranges: HashMap<VoxelKey, (f32, f32)>,
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
        let renderer = PointRenderer::new(render_state);
        let viewports = Viewports {
            main: renderer.create_viewport(),
            map: renderer.create_viewport(),
        };
        let mut app = Self {
            url_input: initial_source.unwrap_or_default(),
            state: LoadState::Idle,
            renderer,
            viewports,
            mode: Mode::Explore,
            subcloud: None,
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
        // A subcloud is in the old cloud's coordinates.
        self.subcloud = None;
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
            let is_open = matches!(self.state, LoadState::Open(_));
            ui.add_enabled_ui(is_open, |ui| {
                ui.selectable_value(&mut self.mode, Mode::Explore, "Explore")
                    .on_hover_text("Look around the whole cloud");
                ui.selectable_value(&mut self.mode, Mode::Subcloud, "Subcloud")
                    .on_hover_text(
                        "Draw a line, circle or square on a map and view just the points \
                         above it",
                    );
            });
            ui.separator();
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
            map_selection: Selection::default(),
            profile: ProfilePoints::default(),
            height_ranges: HashMap::new(),
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
                            if let Some(range) = height_range(&node.positions) {
                                self.height_ranges.insert(key, range);
                            }
                            // Moved rather than copied: the GPU has its own copy now.
                            self.profile.receive(key, node.positions);
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

    /// Picks the nodes for this view (and the map's and density profile's,
    /// if given), asks the loader for missing ones and frees GPU memory if
    /// the cache is over budget.
    fn update(
        &mut self,
        renderer: &mut PointRenderer,
        view: &lod::View,
        map_view: Option<&lod::View>,
        profile: Option<ProfileRequest>,
        settings: &LodSettings,
    ) {
        self.selection = lod::select_nodes(&self.octree, view, settings);
        self.map_selection = map_view
            .map(|view| lod::select_nodes(&self.octree, view, &subcloud_map::LOD))
            .unwrap_or_default();
        self.profile.set_request(&self.octree, profile);

        // Profile nodes already on the GPU arrived before the profile wanted
        // them, so it has no copy: have the loader fetch them again. Nodes
        // still downloading will reach the profile anyway.
        let downloading = &self.fetching;
        let resident = |key: &VoxelKey| renderer.has_node(key) && !downloading.contains(key);
        for key in self.profile.take_refetches(resident) {
            self.loader.evicted(key);
        }

        // The 3D view's nodes go first, so the profile and map never delay them.
        let not_loaded = |key: &&VoxelKey| !renderer.has_node(key) && !self.failed.contains_key(key);
        // `insert` returns false for a key already seen, which drops duplicates.
        let mut seen = HashSet::new();
        let wanted: Vec<VoxelKey> = self
            .selection
            .keys
            .iter()
            .filter(&not_loaded)
            .chain(self.profile.missing().filter(|key| !self.failed.contains_key(key)))
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
            .copied()
            .collect();
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

        // Subcloud mode's map. Side panels have to be laid out before the
        // central panel, but the map is only drawn after it, once the nodes
        // for both views have been chosen.
        let map_frame = match (&self.state, self.mode) {
            (LoadState::Open(session), Mode::Subcloud) => {
                let bounds = session.info.local_bounds();
                let subcloud_mode = self
                    .subcloud
                    .get_or_insert_with(|| SubcloudMode::new(bounds));
                let panel = egui::Panel::left("subcloud_map")
                    .resizable(true)
                    .default_size(380.0)
                    .min_size(200.0)
                    .show(ui, |ui| subcloud_mode.map_panel(ui));
                Some(panel.inner)
            }
            _ => None,
        };

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
                LoadState::Open(session) => {
                    let mut scene = Scene {
                        renderer: &mut self.renderer,
                        viewports: &mut self.viewports,
                        session,
                        settings: &self.settings,
                    };
                    match (self.mode, &mut self.subcloud) {
                        (Mode::Subcloud, Some(subcloud_mode)) => {
                            subcloud_mode.view(ui, &mut scene, map_frame);
                        }
                        _ => explore_view(ui, &mut scene, &mut self.camera),
                    }
                }
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
            let in_subcloud = self.mode == Mode::Subcloud;
            // The view controls act on whichever 3D view is showing.
            let camera = match (self.mode, &mut self.subcloud) {
                (Mode::Subcloud, Some(subcloud_mode)) => &mut subcloud_mode.camera,
                _ => &mut self.camera,
            };
            egui::Window::new("Point cloud")
                // egui remembers a window's position by its id. One per mode
                // keeps it over the 3D view, rather than over the map after
                // switching to Subcloud mode.
                .id(egui::Id::new(("point_cloud_window", self.mode)))
                .default_pos(central.response.rect.min + egui::vec2(8.0, 8.0))
                .collapsible(true)
                .resizable(false)
                .max_width(360.0)
                .show(ui.ctx(), |ui| {
                    cloud_summary(ui, &session.info);
                    ui.separator();
                    streaming_stats(ui, session, &self.renderer);
                    ui.separator();
                    view_ui(ui, camera);
                    ui.separator();
                    settings_ui(ui, &mut self.settings, session.info.has_rgb);
                    ui.separator();
                    ui.label(
                        "Drag: orbit\nRight-drag: pan\nScroll: zoom\nDouble-click: reset view\n\
                         Gizmo: click an axis to look along it\n\
                         7 / 1 / 3: top / front / right (Ctrl: opposite)\n\
                         5: perspective / orthographic",
                    );
                    if in_subcloud {
                        ui.label("Ctrl+scroll: slide the subcloud away or towards you");
                    }
                });
        }

        if let (LoadState::Open(session), Mode::Subcloud, Some(subcloud_mode)) =
            (&mut self.state, self.mode, &mut self.subcloud)
            && subcloud_mode.profile.show
        {
            let mut open = true;
            egui::Window::new("Density profile")
                .open(&mut open)
                // Bottom right, clear of the gizmo in the top right.
                .pivot(egui::Align2::RIGHT_BOTTOM)
                .default_pos(central.response.rect.right_bottom() - egui::vec2(8.0, 8.0))
                .default_size([320.0, 420.0])
                .resizable(true)
                .show(ui.ctx(), |ui| profile_ui(ui, session, &mut subcloud_mode.profile));
            subcloud_mode.profile.show = open;
        }
    }
}

/// The density profile window: settings, loading progress and the chart.
fn profile_ui(ui: &mut egui::Ui, session: &mut Session, settings: &mut ProfileSettings) {
    ui.horizontal(|ui| {
        ui.label("Band height");
        ui.add(
            egui::DragValue::new(&mut settings.bin_size)
                .range(profile::MIN_BIN_SIZE..=10.0)
                .speed(0.01)
                .max_decimals(2)
                .suffix(" m"),
        );
    });
    ui.add(
        egui::Slider::new(&mut settings.point_limit, 100_000..=20_000_000)
            .logarithmic(true)
            .custom_formatter(|value, _| format_count(value as u64))
            .text("Point limit"),
    )
    .on_hover_text(
        "The most points to load for the profile. It loads every level of detail \
         under the shape, and the octree's boxes overlap the shape's edges, so this \
         counts some points outside it too.",
    );
    ui.separator();

    let failed = &session.failed;
    let loading = match session.profile.status(|key| failed.contains_key(key)) {
        profile::Status::TooMany => {
            ui.label(format!(
                "There are more than {} points under the shape. Make it smaller or \
                 raise the point limit.",
                format_count(settings.point_limit)
            ));
            return;
        }
        profile::Status::Loading {
            loaded,
            failed,
            total,
            points,
        } => {
            let loading = loaded + failed < total;
            if loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!(
                        "Loading {loaded} of {total} nodes ({} points)",
                        format_count(points)
                    ));
                });
            }
            if failed > 0 {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("{failed} nodes failed to load, so their points are missing"),
                );
            }
            loading
        }
    };

    let origin_z = session.info.origin[2];
    match session.profile.histogram(settings.bin_size) {
        Some(histogram) => {
            let lowest = f64::from(histogram.lowest) + origin_z;
            ui.label(format!(
                "{} points, the lowest at {lowest:.2} m",
                format_count(histogram.total)
            ));
            ui.weak(format!(
                "Points in each {} m band, by height above the lowest point",
                settings.bin_size
            ));
            profile::chart(ui, histogram);
        }
        None if loading => {}
        None => {
            ui.label("There are no points inside the shape.");
        }
    }
}

impl SubcloudMode {
    fn new(bounds: (glam::Vec3, glam::Vec3)) -> Self {
        let (min, max) = bounds;
        Self {
            subcloud: Subcloud::new(min, max),
            camera: OrbitCamera::default(),
            map: SubcloudMap::new(bounds),
            bounds,
            needs_aim: true,
            sweep_step_percent: subcloud::DEFAULT_SWEEP_STEP_PERCENT,
            profile: ProfileSettings {
                show: false,
                bin_size: profile::DEFAULT_BIN_SIZE,
                point_limit: profile::DEFAULT_POINT_LIMIT,
            },
        }
    }

    /// The left panel: subcloud settings above the map.
    fn map_panel(&mut self, ui: &mut egui::Ui) -> MapFrame {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Shape");
            for shape in ShapeKind::ALL {
                let response = ui
                    .selectable_value(&mut self.subcloud.shape, shape, shape.label())
                    .on_hover_text(shape.description());
                if response.changed() {
                    self.needs_aim = true;
                }
            }
        });
        self.shape_controls(ui);

        let (thickness_name, hint) = match self.subcloud.shape {
            ShapeKind::Line => ("width", "Drag: draw · Drag A, B, ■ or the strip: edit"),
            ShapeKind::Circle => ("diameter", "Drag: draw · Drag ■: resize · Drag inside: move"),
            ShapeKind::Square => ("side", "Drag: draw · Drag ■: resize and turn · Drag inside: move"),
        };
        ui.horizontal(|ui| {
            ui.label("Step");
            ui.add(
                egui::DragValue::new(&mut self.sweep_step_percent)
                    .range(1.0..=500.0)
                    .speed(1.0)
                    .max_decimals(0)
                    .suffix(" %"),
            )
            .on_hover_text(
                "How far one Ctrl+scroll notch slides the subcloud, as a percentage \
                 of its thickness. 100% steps to the next slice with no gap or overlap.",
            );
            // The thickness doesn't depend on which way the slide goes.
            let (_, thickness) = self.subcloud.sweep_axis(glam::Vec2::X);
            let step = thickness * self.sweep_step_percent / 100.0;
            ui.weak(format!("of {thickness_name} ({step:.2} m)"));
        });
        ui.horizontal(|ui| {
            if self.subcloud.shape == ShapeKind::Line
                && ui
                    .button("Swap ends")
                    .on_hover_text("View the slice from the other side")
                    .clicked()
            {
                let line = &mut self.subcloud.line;
                (line.start, line.end) = (line.end, line.start);
                self.needs_aim = true;
            }
            if ui
                .button("Reset view")
                .on_hover_text(
                    "Look at the subcloud side-on again, fitted to the points loaded so \
                     far (or double-click it)",
                )
                .clicked()
            {
                self.needs_aim = true;
            }
            ui.checkbox(&mut self.profile.show, "Density profile")
                .on_hover_text("Count the subcloud's points in each height band");
        });
        ui.weak(format!("{hint}\nRight-drag: pan · Scroll: zoom"));
        ui.add_space(4.0);

        let (frame, change) = self.map.show(ui, &mut self.subcloud);
        match change {
            subcloud_map::Change::Turned => self.needs_aim = true,
            // Follow the subcloud without losing the user's angle and zoom.
            subcloud_map::Change::Moved(offset) => self.camera.target += offset.extend(0.0),
            subcloud_map::Change::Resized | subcloud_map::Change::None => {}
        }
        frame
    }

    /// Exact sizes (and a square's rotation) for the current shape.
    fn shape_controls(&mut self, ui: &mut egui::Ui) {
        // A nested `fn` rather than a closure: closures can't say that the
        // widget they return borrows `value`, but `'_` in a fn signature can.
        fn size(value: &mut f32) -> egui::DragValue<'_> {
            egui::DragValue::new(value)
                .range(subcloud::MIN_SIZE..=10_000.0)
                .speed(0.05)
                .max_decimals(2)
                .suffix(" m")
        }
        ui.horizontal(|ui| match self.subcloud.shape {
            ShapeKind::Line => {
                ui.label("Width");
                ui.add(size(&mut self.subcloud.line.width));
                ui.label(format!("Length {:.1} m", self.subcloud.line.length()));
            }
            ShapeKind::Circle => {
                ui.label("Radius");
                ui.add(size(&mut self.subcloud.circle.radius));
            }
            ShapeKind::Square => {
                let square = &mut self.subcloud.square;
                ui.label("Radius");
                ui.add(size(&mut square.radius))
                    .on_hover_text("From the centre to the middle of each side");
                ui.label("Rotation");
                // Shown in degrees, stored in radians.
                if ui.drag_angle(&mut square.rotation).changed() {
                    self.needs_aim = true;
                }
            }
        });
    }

    /// The 3D view of just the subcloud, then the map, which needs this
    /// frame's node selection.
    fn view(&mut self, ui: &mut egui::Ui, scene: &mut Scene<'_>, map_frame: Option<MapFrame>) {
        let area = ViewArea::allocate(ui);
        let input = camera_controls(ui, &area.response, &mut self.camera);
        if input.reset {
            self.needs_aim = true;
        }
        if input.pinch_px != 0.0 {
            self.sweep(ui, input.pinch_px);
        }
        let footprint = self.subcloud.footprint();
        if self.needs_aim {
            // A small plot is lost in the height of the whole cloud, so
            // frame just the height of the points loaded around it.
            let (min, max) = self.bounds;
            let session = &scene.session;
            let z_range = lod::z_range_under(&session.octree, &footprint, &session.height_ranges)
                .unwrap_or((min.z, max.z));
            self.subcloud
                .aim(&mut self.camera, z_range, area.rect.aspect_ratio());
            self.needs_aim = false;
        }
        show_gizmo(ui, area.rect, &mut self.camera);

        let view = lod::View::new(&self.camera, area.size_px).clipped_to(footprint);
        let map_view = map_frame
            .as_ref()
            .map(|frame| lod::View::new(self.map.camera(), frame.size_px));
        let profile = self.profile.show.then_some(ProfileRequest {
            footprint,
            point_limit: self.profile.point_limit,
        });
        scene.session.update(
            scene.renderer,
            &view,
            map_view.as_ref(),
            profile,
            &scene.settings.lod,
        );
        let texture_id = scene.render_main(&area, &self.camera, Some(footprint));
        area.paint(ui, texture_id);

        if let Some(frame) = map_frame {
            let style = scene.style(area.pixels_per_point, None);
            let texture_id = scene.renderer.render(
                &mut scene.viewports.map,
                frame.size_px,
                self.map.camera(),
                &style,
                &scene.session.map_selection.keys,
                &[],
            );
            self.map.paint(&frame, texture_id, &self.subcloud);
        }
    }

    /// Slides the subcloud for a Ctrl+scroll of `pinch_px`. Scrolling up
    /// moves it away from the viewer, whichever side they're looking from.
    fn sweep(&mut self, ui: &egui::Ui, pinch_px: f32) {
        let px_per_notch = ui.ctx().options(|o| o.input_options.line_scroll_speed);
        let (away, thickness) = self.subcloud.sweep_axis(self.camera.facing());
        let step = thickness * self.sweep_step_percent / 100.0;
        let offset = away * (pinch_px / px_per_notch * step);
        self.subcloud.translate(offset);
        // The camera moves with it, so the subcloud stays in the same place on screen.
        self.camera.target += offset.extend(0.0);
    }
}

/// What every 3D view needs from the app, borrowed for one frame.
struct Scene<'a> {
    renderer: &'a mut PointRenderer,
    viewports: &'a mut Viewports,
    session: &'a mut Session,
    settings: &'a ViewSettings,
}

impl Scene<'_> {
    /// Point style from the settings. Sizes are set in logical pixels, so
    /// they're scaled to physical pixels like the render target.
    fn style(&self, pixels_per_point: f32, clip: Option<Footprint>) -> PointStyle {
        let settings = self.settings;
        PointStyle {
            size_px: settings.point_size * pixels_per_point,
            color: self.session.point_color(settings.color_mode),
            edl: settings.edl_enabled.then_some(EdlSettings {
                radius_px: settings.edl.radius_px * pixels_per_point,
                ..settings.edl
            }),
            clip,
        }
    }

    /// Draws the selected nodes (and debug bounds) into the main viewport.
    fn render_main(
        &mut self,
        area: &ViewArea,
        camera: &OrbitCamera,
        clip: Option<Footprint>,
    ) -> egui::TextureId {
        let lines = if self.settings.show_bounds {
            self.session
                .bounds_lines(self.renderer, self.settings.show_loaded_bounds)
        } else {
            Vec::new()
        };
        let style = self.style(area.pixels_per_point, clip);
        self.renderer.render(
            &mut self.viewports.main,
            area.size_px,
            camera,
            &style,
            &self.session.selection.keys,
            &lines,
        )
    }
}

/// The space a 3D view takes up this frame.
struct ViewArea {
    rect: egui::Rect,
    response: egui::Response,
    /// Render at physical pixel resolution so points stay crisp on high-DPI screens.
    size_px: [u32; 2],
    pixels_per_point: f32,
    /// The bottom layer, reserved for the 3D image. It can only be rendered
    /// once the gizmo has had its say about the camera, but has to be
    /// painted under the gizmo.
    image_slot: egui::layers::ShapeIdx,
}

impl ViewArea {
    fn allocate(ui: &mut egui::Ui) -> Self {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let pixels_per_point = ui.ctx().pixels_per_point();
        Self {
            rect,
            response,
            size_px: [rect.width(), rect.height()]
                .map(|side| (side * pixels_per_point).round() as u32),
            pixels_per_point,
            image_slot: ui.painter().add(egui::Shape::Noop),
        }
    }

    fn paint(&self, ui: &egui::Ui, texture_id: egui::TextureId) {
        let full_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().set(
            self.image_slot,
            egui::Shape::image(texture_id, self.rect, full_uv, egui::Color32::WHITE),
        );
    }
}

/// Explore mode: the whole cloud in one 3D view.
fn explore_view(ui: &mut egui::Ui, scene: &mut Scene<'_>, camera: &mut OrbitCamera) {
    let area = ViewArea::allocate(ui);
    let input = camera_controls(ui, &area.response, camera);
    if input.reset {
        let (min, max) = scene.session.info.local_bounds();
        camera.frame_bounds(min, max);
    }
    // A touchpad pinch zooms like scrolling. (So does Ctrl+scroll, which
    // egui reports the same way.)
    camera.zoom(input.pinch_px);
    show_gizmo(ui, area.rect, camera);

    let view = lod::View::new(camera, area.size_px);
    scene
        .session
        .update(scene.renderer, &view, None, None, &scene.settings.lod);
    let texture_id = scene.render_main(&area, camera, None);
    area.paint(ui, texture_id);
}

fn show_gizmo(ui: &mut egui::Ui, view_rect: egui::Rect, camera: &mut OrbitCamera) {
    let rect = egui::Rect::from_min_size(
        egui::pos2(
            view_rect.right() - OVERLAY_MARGIN - gizmo::SIZE,
            view_rect.top() + OVERLAY_MARGIN,
        ),
        egui::Vec2::splat(gizmo::SIZE),
    );
    gizmo::show(ui, rect, camera);
}

/// Input over a 3D view that the caller acts on, beyond moving the camera.
struct ViewInput {
    /// Double-clicked: go back to the default view.
    reset: bool,
    /// A Ctrl+scroll or touchpad pinch, as the scroll distance in logical
    /// pixels that would make the same zoom. egui reports Ctrl+scroll as a
    /// zoom gesture rather than a scroll. Positive is scrolling up.
    pinch_px: f32,
}

/// Mouse and keyboard control of a 3D view's camera.
fn camera_controls(
    ui: &egui::Ui,
    response: &egui::Response,
    camera: &mut OrbitCamera,
) -> ViewInput {
    let drag = glam::vec2(response.drag_delta().x, response.drag_delta().y);
    if response.dragged_by(egui::PointerButton::Primary) {
        camera.orbit(drag);
    }
    if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
    {
        camera.pan(drag, response.rect.height());
    }
    let mut input = ViewInput {
        reset: response.double_clicked(),
        pinch_px: 0.0,
    };
    if response.hovered() {
        let (scroll, zoom_delta) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        if scroll != 0.0 {
            camera.zoom(scroll);
        }
        // egui turned these pixels into a zoom factor of exp(px × speed).
        let zoom_speed = ui.ctx().options(|o| o.input_options.scroll_zoom_speed);
        input.pinch_px = zoom_delta.ln() / zoom_speed;
    }

    // Digits typed into text fields aren't shortcuts.
    if !ui.ctx().text_edit_focused() {
        ui.input_mut(|input| {
            for (key, preset, opposite) in VIEW_SHORTCUTS {
                // Ctrl first: a plain-key check would also match Ctrl+key.
                if input.consume_key(egui::Modifiers::COMMAND, key) {
                    camera.set_view(opposite);
                } else if input.consume_key(egui::Modifiers::NONE, key) {
                    camera.set_view(preset);
                }
            }
            if input.consume_key(egui::Modifiers::NONE, PROJECTION_SHORTCUT) {
                camera.set_projection(camera.projection().toggled());
            }
        });
    }
    input
}

fn view_ui(ui: &mut egui::Ui, camera: &mut OrbitCamera) {
    ui.horizontal(|ui| {
        ui.label("Projection:");
        // Not `radio_value`: the camera needs to hear about the click, even on
        // the option already selected, to know the user chose it.
        for (projection, label) in [
            (Projection::Perspective, "Perspective"),
            (Projection::Orthographic, "Orthographic"),
        ] {
            if ui.radio(camera.projection() == projection, label).clicked() {
                camera.set_projection(projection);
            }
        }
    });
    ui.checkbox(&mut camera.auto_orthographic, "Orthographic for axis views")
        .on_hover_text(
            "Snapping to top, front, etc. switches to orthographic; \
             orbiting away switches back to perspective.",
        );
    ui.horizontal_wrapped(|ui| {
        ui.label("View:");
        for preset in ViewPreset::ALL {
            if ui
                .button(preset.label())
                .on_hover_text(preset.description(camera.axes))
                .clicked()
            {
                camera.set_view(preset);
            }
        }
    });
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

/// The lowest and highest Z in `positions`, or `None` if it's empty.
fn height_range(positions: &[[f32; 3]]) -> Option<(f32, f32)> {
    positions
        .iter()
        .map(|[_, _, z]| (*z, *z))
        .reduce(|(low, high), (z, _)| (low.min(z), high.max(z)))
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
pub(crate) fn format_count(count: u64) -> String {
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
