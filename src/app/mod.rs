//! The viewer window: the app's state machine, the per-frame layout and the
//! command dispatcher.
//!
//! The pieces live in submodules:
//! - [`commands`]: every [`Command`] the user can give, with ids, titles and
//!   shortcuts, and the [`Keymap`](commands::Keymap).
//! - [`settings`]: what the user can tweak, as plain data.
//! - [`session`]: an open cloud, its loader and per-frame LOD.
//! - [`scene`]: drawing the 3D views, Explore mode's included.
//! - [`camera_input`]: mouse control of a 3D view's camera.
//! - [`subcloud_mode`]: Subcloud mode's map panel and 3D view.
//! - [`subcloud_tool`]: drawing a subcloud on the explore view.
//! - [`export_ui`]: the Save image section and its job.
//! - [`open_ui`]: the Open menu and the Open URL dialog.
//! - [`connectors`]: data sources (STAC catalogs and folders) to browse to
//!   clouds in, and [`feed`], their listings fetched in the background.
//! - [`palette`]: the command palette (Ctrl+Shift+P) and quick open (Ctrl+P).
//! - [`panels`]: the settings panel's sections and the status bar.
//! - [`user_config`]: the settings files applied, and the settings commands.

mod camera_input;
mod commands;
mod connectors;
mod export_ui;
mod feed;
mod open_ui;
mod palette;
mod panels;
mod scene;
mod session;
mod settings;
mod subcloud_mode;
mod subcloud_tool;
mod user_config;

use std::mem;
use std::sync::mpsc::TryRecvError;

use copc_streaming::VoxelKey;
use eframe::egui;

use crate::camera::OrbitCamera;
use crate::cloud::LoadError;
use crate::config::Locations;
use crate::heights::{HeightKind, HeightSettings};
use crate::loader::{Event, Loader};
use crate::lod::{self, Octree};
use crate::renderer::PointRenderer;
use crate::subcloud::{self, Footprint, ShapeKind, Subcloud};
use crate::theme::Theme;

use commands::{Command, Keymap};
use connectors::ConnectorHub;
use export_ui::{ExportContext, ExportJob, ImageExport, export_ui};
use palette::{Palette, PaletteMode};
use panels::{
    HeightsAction, cloud_summary, diagnostics_ui, display_ui, heights_ui, performance_ui, section,
    status_bar, view_ui,
};
use scene::{Scene, Viewports, explore_view};
use session::Session;
use settings::Settings;
use subcloud_mode::{Reframe, SubcloudMode};
use subcloud_tool::SubcloudTool;
use user_config::UserConfig;

pub struct ViewerApp {
    state: LoadState,
    renderer: PointRenderer,
    viewports: Viewports,
    /// The Explore mode camera.
    camera: OrbitCamera,
    settings: Settings,
    mode: Mode,
    /// Kept while in Explore mode, so switching back returns to the same
    /// subcloud. Created when the first subcloud is drawn on a cloud.
    subcloud: Option<SubcloudMode>,
    /// The subcloud tool picked in Explore mode, if any.
    tool: SubcloudTool,
    export: ImageExport,
    /// Where the open cloud's heights were read from, as asked.
    heights: HeightSettings,
    /// The same, as edited in the Heights section until Reload applies it.
    heights_draft: HeightSettings,
    /// The UI's colours, fonts and spacing, as applied to the egui context.
    theme: Theme,
    /// Which shortcut runs which command.
    keymap: Keymap,
    /// The settings files, and problems found in them.
    user_config: UserConfig,
    /// Commands asked for this frame, by buttons and shortcuts. They run
    /// together once the frame is laid out, when the 3D view's size is known.
    pending: Vec<Command>,
    /// The Open URL dialog's text, while it's showing.
    url_dialog: Option<String>,
    /// The command palette, and the sources opened this session.
    palette: Palette,
    /// Makes the data sources' listings, and keeps them for the session.
    connector_hub: ConnectorHub,
    /// The data sources, as of the last time the settings were read.
    sources: Vec<connectors::DataSource>,
    /// The 3D view's size in physical pixels, as of the last frame with a
    /// cloud open.
    view_size_px: [u32; 2],
}

/// What the main area shows. Switched from the top bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Mode {
    /// The whole cloud in one 3D view.
    Explore,
    /// A subcloud drawn in Explore mode: a map to edit its line, circle or
    /// square on, and a 3D view of just the points above it.
    Subcloud,
}

enum LoadState {
    Idle,
    /// The loader thread is reading the header and hierarchy.
    Opening {
        source: String,
        loader: Loader,
    },
    /// Boxed because a session is much bigger than the other variants.
    Open(Box<Session>),
    Failed {
        source: String,
        error: String,
        /// The heights couldn't be found where the settings said.
        bad_heights: bool,
    },
}

impl ViewerApp {
    /// `initial_source`, if given, starts loading immediately. Settings
    /// are read from the files at `config_locations`, or none if `None`.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_source: Option<String>,
        heights: HeightSettings,
        config_locations: Option<Locations>,
    ) -> Result<Self, &'static str> {
        let render_state = cc
            .wgpu_render_state
            .as_ref()
            .ok_or("the wgpu renderer is not available")?;
        #[cfg(windows)]
        crate::windows_dpi::defer_dpi_changes_while_dragging(cc);
        let renderer = PointRenderer::new(render_state);
        let theme = Theme::default();
        theme.apply(&cc.egui_ctx);
        let viewports = Viewports {
            main: renderer.create_viewport(),
            map: renderer.create_viewport(),
        };
        let mut app = Self {
            state: LoadState::Idle,
            renderer,
            viewports,
            mode: Mode::Explore,
            subcloud: None,
            tool: SubcloudTool::default(),
            camera: OrbitCamera::default(),
            settings: Settings::default(),
            export: ImageExport::default(),
            heights_draft: heights.clone(),
            heights,
            theme,
            keymap: Keymap::default(),
            user_config: UserConfig::new(config_locations),
            pending: Vec::new(),
            url_dialog: None,
            palette: Palette::default(),
            connector_hub: ConnectorHub::new(cc.egui_ctx.clone()),
            sources: Vec::new(),
            view_size_px: [1, 1],
        };
        app.load_user_config(&cc.egui_ctx);
        if let Some(source) = initial_source.as_deref().and_then(open_ui::clean_source) {
            app.start_loading(&cc.egui_ctx, source);
        }
        Ok(app)
    }

    fn start_loading(&mut self, ctx: &egui::Context, source: String) {
        self.heights_draft = self.heights.clone();
        self.palette.remember(&source);
        let loader = Loader::spawn(source.clone(), self.heights.clone(), ctx.clone());
        // Replacing the state drops any previous loader, which stops its thread.
        self.state = LoadState::Opening { source, loader };
        self.renderer.clear_nodes();
        // A subcloud, or an image's view, is in the old cloud's coordinates.
        self.subcloud = None;
        self.tool.cancel();
        self.mode = Mode::Explore;
        self.export.job = None;
    }

    /// Applies whatever the loader thread has sent since the last frame.
    fn poll_loader(&mut self) {
        // Take the state out (leaving `Idle` behind) so the match owns it and
        // can move the loader into the next state, then put the result back.
        self.state = match mem::replace(&mut self.state, LoadState::Idle) {
            LoadState::Opening { source, loader } => self.poll_opening(source, loader),
            LoadState::Open(mut session) => match session.receive(&mut self.renderer) {
                Ok(()) => {
                    // The shown height's range grows as nodes arrive.
                    let (min, max) = session.octree.bounds();
                    self.renderer.set_z_range([min.z, max.z]);
                    let root_loaded = session
                        .octree
                        .known_heights(session.shown)
                        .contains_key(&VoxelKey::ROOT);
                    if session.frame_on_root && root_loaded {
                        session.frame_on_root = false;
                        self.camera.frame_bounds(min, max);
                    }
                    LoadState::Open(session)
                }
                Err(error) => LoadState::Failed {
                    source: session.info.source.clone(),
                    error,
                    bad_heights: false,
                },
            },
            other => other,
        };
    }

    fn poll_opening(&mut self, source: String, loader: Loader) -> LoadState {
        match loader.try_recv() {
            Ok(Event::Opened(opened)) => {
                let session = Session::new(loader, *opened);
                let (min, max) = session.octree.bounds();
                self.renderer.set_z_range([min.z, max.z]);
                self.renderer.set_octree_shape(session.octree.shape());
                self.camera.frame_bounds(min, max);
                LoadState::Open(Box::new(session))
            }
            Ok(Event::Failed(error)) => LoadState::Failed {
                source,
                bad_heights: matches!(error, LoadError::Heights(_)),
                error: error.to_string(),
            },
            // Node events only come after `Opened`, once the UI asks for nodes.
            Ok(Event::Fetching(_) | Event::Node { .. }) | Err(TryRecvError::Empty) => {
                LoadState::Opening { source, loader }
            }
            Err(TryRecvError::Disconnected) => LoadState::Failed {
                source,
                error: "the loader thread crashed".to_owned(),
                bad_heights: false,
            },
        }
    }

    /// The toolbar: the modes, the subcloud tools and the Open menu.
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let is_open = matches!(self.state, LoadState::Open(_));
            ui.add_enabled_ui(is_open, |ui| {
                let explore = ui
                    .selectable_label(self.mode == Mode::Explore, "Explore")
                    .on_hover_text("Look around the whole cloud");
                if explore.clicked() {
                    self.pending.push(Command::SetMode(Mode::Explore));
                }
                let subcloud = ui
                    .add_enabled(
                        self.subcloud.is_some(),
                        egui::Button::selectable(self.mode == Mode::Subcloud, "Subcloud"),
                    )
                    .on_hover_text("Back to the subcloud you drew last")
                    .on_disabled_hover_text(
                        "Draw a subcloud first: pick Line, Circle or Square, then drag on \
                         the cloud",
                    );
                if subcloud.clicked() {
                    self.pending.push(Command::SetMode(Mode::Subcloud));
                }
                // Subcloud mode has its own shape buttons on the map.
                if self.mode == Mode::Explore {
                    ui.separator();
                    self.tool_buttons(ui);
                }
            });
            ui.separator();
            open_ui::open_menu(ui, &self.keymap, &self.sources, &mut self.pending);
        });
    }

    /// Line, Circle and Square: pick one, then drag on the explore view to
    /// draw that subcloud. Clicking the picked one again puts it down.
    fn tool_buttons(&mut self, ui: &mut egui::Ui) {
        ui.label("Draw subcloud");
        for shape in ShapeKind::ALL {
            let picked = self.tool.picked() == Some(shape);
            let how = match shape {
                ShapeKind::Line => "from one end to the other",
                ShapeKind::Circle | ShapeKind::Square => "from its centre to its edge",
            };
            let response = ui
                .selectable_label(picked, shape.label())
                .on_hover_text(format!(
                    "{}. Then drag on the cloud to draw it, {how}. Esc cancels.",
                    shape.description()
                ));
            if response.clicked() {
                self.pending.push(if picked {
                    Command::CancelSubcloudTool
                } else {
                    Command::SubcloudTool(shape)
                });
            }
        }
    }

    /// The 3D view's camera: Subcloud mode's while it's showing, else
    /// Explore's.
    fn active_camera(&mut self) -> &mut OrbitCamera {
        match (self.mode, &mut self.subcloud) {
            (Mode::Subcloud, Some(subcloud_mode)) => &mut subcloud_mode.camera,
            _ => &mut self.camera,
        }
    }

    /// Whether `command` can do anything right now. A command palette can
    /// grey out the rest.
    pub(crate) fn can_run(&self, command: &Command) -> bool {
        let is_open = matches!(self.state, LoadState::Open(_));
        match command {
            Command::OpenSource(_)
            | Command::OpenFileDialog
            | Command::OpenUrlDialog
            | Command::ToggleDiagnostics
            | Command::Settings(_)
            | Command::ShowCommands
            | Command::QuickOpen
            | Command::Connector(_) => true,
            Command::SaveImage => is_open && !self.export.is_busy(),
            // There's nothing to return to until a subcloud has been drawn.
            Command::SetMode(Mode::Subcloud) => is_open && self.subcloud.is_some(),
            Command::CancelSubcloudTool => self.tool.is_active(),
            Command::SetMode(Mode::Explore)
            | Command::SubcloudTool(_)
            | Command::SetView(_)
            | Command::SetProjection(_)
            | Command::ToggleProjection
            | Command::ResetView
            | Command::ToggleEdl
            | Command::SetColorMode(_)
            | Command::CopyCommandLine => is_open,
            Command::RetryFailedNodes => {
                matches!(&self.state, LoadState::Open(session) if !session.failed.is_empty())
            }
        }
    }

    /// Does what `command` asks, if it can be done now. Every button,
    /// menu item and shortcut goes through here.
    pub(crate) fn run(&mut self, ctx: &egui::Context, command: Command) {
        if !self.can_run(&command) {
            return;
        }
        match command {
            Command::OpenSource(source) => {
                if let Some(source) = open_ui::clean_source(&source) {
                    self.start_loading(ctx, source);
                }
            }
            Command::OpenFileDialog => {
                if let Some(source) = open_ui::pick_file() {
                    self.run(ctx, Command::OpenSource(source));
                }
            }
            Command::OpenUrlDialog => self.url_dialog = Some(String::new()),
            Command::SetMode(mode) => {
                self.mode = mode;
                self.tool.cancel();
            }
            Command::SubcloudTool(shape) => {
                // Drawn on the explore view, so go back to it.
                self.mode = Mode::Explore;
                let line_width = self
                    .subcloud
                    .as_ref()
                    .map_or(subcloud::DEFAULT_WIDTH, |mode| mode.subcloud.line.width);
                self.tool.pick(shape, line_width);
            }
            Command::CancelSubcloudTool => self.tool.cancel(),
            Command::SetView(preset) => self.active_camera().set_view(preset),
            Command::SetProjection(projection) => {
                self.active_camera().set_projection(projection);
            }
            Command::ToggleProjection => {
                let camera = self.active_camera();
                camera.set_projection(camera.projection().toggled());
            }
            Command::ResetView => self.reset_view(),
            Command::ToggleEdl => {
                let view = &mut self.settings.view;
                view.edl_enabled = !view.edl_enabled;
            }
            Command::SetColorMode(mode) => self.settings.view.color_mode = mode,
            Command::SaveImage => self.save_image(ctx),
            Command::CopyCommandLine => self.copy_render_command(ctx),
            Command::RetryFailedNodes => {
                if let LoadState::Open(session) = &mut self.state {
                    session.retry_failed();
                }
            }
            Command::ToggleDiagnostics => {
                let diagnostics = &mut self.settings.diagnostics;
                diagnostics.visible = !diagnostics.visible;
            }
            Command::Settings(command) => self.run_settings_command(ctx, command),
            Command::ShowCommands => self.palette.open(PaletteMode::Commands),
            Command::QuickOpen => self.palette.open(PaletteMode::Open),
            Command::Connector(command) => self.run_connector_command(command),
        }
        // The change shows next frame.
        ctx.request_repaint();
    }

    /// Points the 3D view at the whole cloud, or side-on at the subcloud,
    /// as a double-click on it does.
    fn reset_view(&mut self) {
        match (self.mode, &mut self.subcloud, &self.state) {
            (Mode::Subcloud, Some(subcloud_mode), _) => {
                subcloud_mode.reframe = Some(Reframe::Aim);
            }
            (_, _, LoadState::Open(session)) => {
                let (min, max) = session.octree.bounds();
                self.camera.frame_bounds(min, max);
            }
            _ => {}
        }
    }

    /// The cloud's settings, docked on the right like an inspector.
    fn settings_panel(&mut self, ui: &mut egui::Ui) -> Option<HeightsAction> {
        let LoadState::Open(session) = &self.state else {
            return None;
        };
        let in_subcloud = self.mode == Mode::Subcloud;
        // The view controls act on whichever 3D view is showing.
        let camera = match (self.mode, &mut self.subcloud) {
            (Mode::Subcloud, Some(subcloud_mode)) => &mut subcloud_mode.camera,
            _ => &mut self.camera,
        };
        let export_context = ExportContext {
            max_size: self.renderer.max_image_size(),
            progress: session.export_progress(&self.renderer),
            in_subcloud,
        };
        let theme = &self.theme;
        let settings = &mut self.settings;
        let commands = &mut self.pending;
        let keymap = &self.keymap;
        let mut heights_action = None;
        // Side panels are laid out before the central panel, so a size that
        // follows the view shows the last frame's, which is the same unless
        // the window is being resized.
        egui::Panel::right("point_cloud")
            .frame(theme.panel_frame())
            .show_separator_line(false)
            .resizable(true)
            .default_size(340.0)
            .min_size(240.0)
            .show(ui, |ui| {
                ui.heading("Point cloud");
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        section(ui, theme, |ui| cloud_summary(ui, &session.info));
                        section(ui, theme, |ui| {
                            display_ui(ui, &mut settings.view, session, commands);
                        });
                        section(ui, theme, |ui| view_ui(ui, camera, commands));
                        heights_action = section(ui, theme, |ui| {
                            heights_ui(ui, session, &mut self.heights_draft, &self.heights)
                        });
                        section(ui, theme, |ui| {
                            export_ui(
                                ui,
                                &self.export,
                                &mut settings.export,
                                export_context,
                                keymap,
                                commands,
                            );
                        });
                        section(ui, theme, |ui| performance_ui(ui, &mut settings.view.lod));
                        section(ui, theme, |ui| {
                            diagnostics_ui(
                                ui,
                                session,
                                &self.renderer,
                                &mut settings.diagnostics,
                                commands,
                            );
                        });
                        controls_help(ui, theme, in_subcloud);
                    });
            });
        heights_action
    }
}

/// How to drive the 3D view, in the faintest text at the foot of the panel.
fn controls_help(ui: &mut egui::Ui, theme: &Theme, in_subcloud: bool) {
    // On the panel rather than in a box, so it can be the faintest text.
    let help = |text: &str| egui::RichText::new(text).color(theme.colors.text_tertiary);
    ui.label(help(
        "Drag: orbit\nRight-drag: pan\nScroll: zoom\nDouble-click: reset view\n\
         Gizmo: click an axis to look along it\n\
         7 / 1 / 3: top / front / right (Ctrl: opposite)\n\
         5: perspective / orthographic",
    ));
    if in_subcloud {
        ui.label(help(
            "Ctrl+scroll: slide the subcloud forward (towards its round handle) or back\n\
             Ctrl+Shift+scroll: slide it right or left",
        ));
    }
}

impl eframe::App for ViewerApp {
    /// The gutter, showing between the panels.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.colors.gutter.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_loader();

        // Digits typed into text fields, or the palette, aren't shortcuts.
        if !ui.ctx().text_edit_focused() && self.url_dialog.is_none() && !self.palette.is_open() {
            let pressed = ui.input_mut(|input| self.keymap.consume(input));
            self.pending.extend(pressed);
        }

        // Panels are separated by gaps, not lines.
        egui::Panel::top("top_bar")
            .frame(self.theme.header_frame())
            .show_separator_line(false)
            .show(ui, |ui| self.top_bar(ui));
        // Added before the settings messages, so it's the bottom-most and
        // they stack above it.
        egui::Panel::bottom("status_bar")
            .frame(self.theme.status_bar_frame())
            .show_separator_line(false)
            .resizable(false)
            .exact_size(self.theme.status_bar_height())
            .show(ui, |ui| {
                status_bar(
                    ui,
                    &self.state,
                    &self.renderer,
                    &self.theme,
                    &mut self.pending,
                );
            });
        self.settings_bar(ui);

        // Subcloud mode's map. Side panels have to be laid out before the
        // central panel, but the map is only drawn after it, once the nodes
        // for both views have been chosen.
        let map_frame = match (&self.state, self.mode, &mut self.subcloud) {
            (LoadState::Open(_), Mode::Subcloud, Some(subcloud_mode)) => {
                let panel = egui::Panel::left("subcloud_map")
                    .frame(self.theme.panel_frame())
                    .show_separator_line(false)
                    .resizable(true)
                    .default_size(380.0)
                    .min_size(200.0)
                    .show(ui, |ui| subcloud_mode.map_panel(ui));
                Some(panel.inner)
            }
            _ => None,
        };

        let heights_action = self.settings_panel(ui);

        let export_view = self.export.job.as_ref().map(ExportJob::view);
        let image_aspect = (!self.settings.export.match_view).then(|| {
            let [width, height] = self.settings.export.size;
            width as f32 / height as f32
        });

        let mut retry_with_auto_heights = None;
        let mut drawn = None;
        // The 3D view fills its panel edge to edge, keeping only the gap
        // between panels; the text states are padded.
        let frame = match self.state {
            LoadState::Open(_) => egui::Frame::NONE.outer_margin(self.theme.panel_outer_margin()),
            _ => self.theme.panel_frame().fill(self.theme.colors.viewport),
        };
        let central =
            egui::CentralPanel::default()
                .frame(frame)
                .show(ui, |ui| match &mut self.state {
                    LoadState::Idle => open_ui::idle_ui(ui, &self.keymap, &mut self.pending),
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
                            settings: &self.settings.view,
                            map_style: &self.settings.map,
                            diagnostics: &self.settings.diagnostics,
                            export_view: export_view.as_ref(),
                            image_aspect,
                            theme: &self.theme,
                        };
                        match (self.mode, &mut self.subcloud) {
                            (Mode::Subcloud, Some(subcloud_mode)) => {
                                subcloud_mode.view(ui, &mut scene, map_frame);
                            }
                            _ => {
                                drawn =
                                    explore_view(ui, &mut scene, &mut self.camera, &mut self.tool);
                            }
                        }
                    }
                    LoadState::Failed {
                        source,
                        error,
                        bad_heights,
                    } => {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            format!("Failed to load {source}"),
                        );
                        ui.label(error.as_str());
                        if *bad_heights && ui.button("Open with automatic heights").clicked() {
                            retry_with_auto_heights = Some(source.clone());
                        }
                    }
                });
        if let Some(subcloud) = drawn {
            self.open_subcloud(subcloud);
        }
        if let Some(source) = retry_with_auto_heights {
            self.heights = HeightSettings::default();
            self.start_loading(ui.ctx(), source);
        }

        if matches!(self.state, LoadState::Open(_)) {
            let pixels_per_point = ui.ctx().pixels_per_point();
            // The panel's rect includes the gap kept around the view.
            let view_rect = central.response.rect - self.theme.panel_outer_margin();
            let view_size = view_rect.size() * pixels_per_point;
            self.view_size_px = [view_size.x, view_size.y].map(|side| side.round() as u32);
            if self.settings.export.match_view {
                let max_image_size = self.renderer.max_image_size();
                self.settings.export.size =
                    self.view_size_px.map(|side| side.clamp(1, max_image_size));
            }
        }

        match heights_action {
            Some(HeightsAction::Show(kind)) => self.show_height(kind),
            Some(HeightsAction::Reload) => {
                if let LoadState::Open(session) = &self.state {
                    let source = session.info.source.clone();
                    self.heights = self.heights_draft.clone();
                    self.start_loading(ui.ctx(), source);
                }
            }
            None => {}
        }

        if let Some(command) = open_ui::url_dialog(ui.ctx(), &mut self.url_dialog) {
            self.pending.push(command);
        }
        self.palette_ui(ui.ctx());
        for command in mem::take(&mut self.pending) {
            self.run(ui.ctx(), command);
        }

        self.finish_export(ui.ctx());
    }
}

impl ViewerApp {
    /// Shows just the points inside a newly drawn `subcloud`, in Subcloud
    /// mode. The Explore camera stays where it was, for coming back.
    fn open_subcloud(&mut self, subcloud: Subcloud) {
        let LoadState::Open(session) = &self.state else {
            return;
        };
        match &mut self.subcloud {
            Some(subcloud_mode) => subcloud_mode.replace(subcloud),
            None => {
                self.subcloud = Some(SubcloudMode::new(
                    subcloud,
                    session.octree.bounds(),
                    session.info.origin,
                ));
            }
        }
        self.mode = Mode::Subcloud;
    }

    /// Switches the points to `kind` heights, moving each camera to follow.
    fn show_height(&mut self, kind: HeightKind) {
        let LoadState::Open(session) = &mut self.state else {
            return;
        };
        let from = session.shown;
        follow_height(&mut self.camera, &session.octree, from, kind);
        if let Some(subcloud_mode) = &mut self.subcloud {
            follow_height(&mut subcloud_mode.camera, &session.octree, from, kind);
        }
        session.show_height(kind);
        if let Some(subcloud_mode) = &mut self.subcloud {
            subcloud_mode.map.set_bounds(session.octree.bounds());
        }
        // Its camera was placed for the other height.
        self.export.job = None;
    }
}

/// Moves `camera` up or down by how far `to` heights sit from `from` heights
/// around what it's looking at, so switching heights keeps it on the same
/// points. A camera over a hill drops by the hill's height, say, switching
/// from elevation to height above ground.
fn follow_height(camera: &mut OrbitCamera, octree: &Octree, from: HeightKind, to: HeightKind) {
    let around = Footprint::Circle {
        centre: camera.target.truncate(),
        radius: camera.half_height().max(1.0),
    };
    let lowest = |kind| {
        lod::z_range_under(octree, &around, octree.known_heights(kind))
            .or_else(|| octree.height_extent(kind))
            .map(|(low, _)| low)
    };
    if let (Some(from), Some(to)) = (lowest(from), lowest(to)) {
        camera.target.z += to - from;
    }
}
