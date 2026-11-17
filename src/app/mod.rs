//! The viewer window: the app's state machine and per-frame layout.
//!
//! The pieces live in submodules:
//! - [`session`]: an open cloud, its loader and per-frame LOD.
//! - [`scene`]: drawing the 3D views, Explore mode's included.
//! - [`camera_input`]: mouse and keyboard control of a 3D view's camera.
//! - [`subcloud_mode`]: Subcloud mode's map panel and 3D view.
//! - [`export_ui`]: the Save image section and its job.
//! - [`settings`]: what the user can tweak, as plain data.
//! - [`panels`]: the settings panel's sections.

mod camera_input;
mod export_ui;
mod panels;
mod scene;
mod session;
mod settings;
mod subcloud_mode;

use std::mem;
use std::sync::mpsc::TryRecvError;

use copc_streaming::VoxelKey;
use eframe::egui;

use crate::camera::OrbitCamera;
use crate::cloud::LoadError;
use crate::heights::{HeightKind, HeightSettings};
use crate::loader::{Event, Loader};
use crate::lod::{self, LodSettings, Octree};
use crate::renderer::{EdlSettings, PointRenderer};
use crate::subcloud::Footprint;
use crate::theme::Theme;

use export_ui::{
    ExportAction, ExportContext, ExportJob, ImageExport, export_ui, framed_camera, render_args,
};
use panels::{
    HeightsAction, cloud_summary, heights_ui, section, settings_ui, streaming_stats, view_ui,
};
use scene::{Scene, Viewports, explore_view};
use session::Session;
use settings::{ColorMode, ViewSettings};
use subcloud_mode::SubcloudMode;

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
    export: ImageExport,
    /// Where the open cloud's heights were read from, as asked.
    heights: HeightSettings,
    /// The same, as edited in the Heights section until Reload applies it.
    heights_draft: HeightSettings,
    /// The UI's colours, fonts and spacing, as applied to the egui context.
    theme: Theme,
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
    /// `initial_source`, if given, starts loading immediately.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        initial_source: Option<String>,
        heights: HeightSettings,
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
            export: ImageExport {
                match_view: true,
                size: [1920, 1080],
                job: None,
                writing: None,
                status: None,
            },
            heights_draft: heights.clone(),
            heights,
            theme,
        };
        if !app.url_input.trim().is_empty() {
            app.start_loading(&cc.egui_ctx);
        }
        Ok(app)
    }

    fn start_loading(&mut self, ctx: &egui::Context) {
        let source = self.url_input.trim().to_owned();
        self.heights_draft = self.heights.clone();
        let loader = Loader::spawn(source.clone(), self.heights.clone(), ctx.clone());
        // Replacing the state drops any previous loader, which stops its thread.
        self.state = LoadState::Opening { source, loader };
        self.renderer.clear_nodes();
        // A subcloud, or an image's view, is in the old cloud's coordinates.
        self.subcloud = None;
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

impl eframe::App for ViewerApp {
    /// The gutter, showing between the panels.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.colors.gutter.to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_loader();

        // Panels are separated by gaps, not lines.
        egui::Panel::top("url_bar")
            .frame(self.theme.header_frame())
            .show_separator_line(false)
            .show(ui, |ui| self.url_bar(ui));

        // Subcloud mode's map. Side panels have to be laid out before the
        // central panel, but the map is only drawn after it, once the nodes
        // for both views have been chosen.
        let map_frame = match (&self.state, self.mode) {
            (LoadState::Open(session), Mode::Subcloud) => {
                let bounds = session.octree.bounds();
                let subcloud_mode = self
                    .subcloud
                    .get_or_insert_with(|| SubcloudMode::new(bounds));
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

        // The cloud's settings, docked on the right like an inspector. Side
        // panels are laid out before the central panel, so a size that
        // follows the view shows the last frame's, which is the same unless
        // the window is being resized.
        let mut heights_action = None;
        let mut export_action = None;
        if let LoadState::Open(session) = &self.state {
            let in_subcloud = self.mode == Mode::Subcloud;
            // The view controls act on whichever 3D view is showing.
            let camera = match (self.mode, &mut self.subcloud) {
                (Mode::Subcloud, Some(subcloud_mode)) => &mut subcloud_mode.camera,
                _ => &mut self.camera,
            };
            let max_image_size = self.renderer.max_image_size();
            let export_progress = session.export_progress(&self.renderer);
            let theme = &self.theme;
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
                            section(ui, theme, |ui| streaming_stats(ui, session, &self.renderer));
                            section(ui, theme, |ui| view_ui(ui, camera));
                            heights_action = section(ui, theme, |ui| {
                                heights_ui(ui, session, &mut self.heights_draft, &self.heights)
                            });
                            section(ui, theme, |ui| settings_ui(ui, &mut self.settings, session));
                            export_action = section(ui, theme, |ui| {
                                export_ui(
                                    ui,
                                    &mut self.export,
                                    ExportContext {
                                        max_size: max_image_size,
                                        progress: export_progress,
                                        in_subcloud,
                                    },
                                )
                            });
                            // On the panel rather than in a box, so it can be the faintest text.
                            let help =
                                |text: &str| egui::RichText::new(text).color(theme.colors.text_tertiary);
                            ui.label(help(
                                "Drag: orbit\nRight-drag: pan\nScroll: zoom\nDouble-click: reset view\n\
                                 Gizmo: click an axis to look along it\n\
                                 7 / 1 / 3: top / front / right (Ctrl: opposite)\n\
                                 5: perspective / orthographic",
                            ));
                            if in_subcloud {
                                ui.label(help(
                                    "Ctrl+scroll: slide the subcloud forward (towards its round \
                                     handle) or back\n\
                                     Ctrl+Shift+scroll: slide it right or left",
                                ));
                            }
                        });
                });
        }

        let export_view = self.export.job.as_ref().map(ExportJob::view);
        let image_aspect = (!self.export.match_view).then(|| {
            let [width, height] = self.export.size;
            width as f32 / height as f32
        });

        let mut retry_with_auto_heights = None;
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
                            export_view: export_view.as_ref(),
                            image_aspect,
                            theme: &self.theme,
                        };
                        match (self.mode, &mut self.subcloud) {
                            (Mode::Subcloud, Some(subcloud_mode)) => {
                                subcloud_mode.view(ui, &mut scene, map_frame);
                            }
                            _ => explore_view(ui, &mut scene, &mut self.camera),
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
        if let Some(source) = retry_with_auto_heights {
            self.heights = HeightSettings::default();
            self.url_input = source;
            self.start_loading(ui.ctx());
        }

        if let LoadState::Open(session) = &self.state {
            let clip = match (self.mode, &self.subcloud) {
                (Mode::Subcloud, Some(subcloud_mode)) => Some(subcloud_mode.subcloud.footprint()),
                _ => None,
            };
            let camera = match (self.mode, &mut self.subcloud) {
                (Mode::Subcloud, Some(subcloud_mode)) => &mut subcloud_mode.camera,
                _ => &mut self.camera,
            };
            let pixels_per_point = ui.ctx().pixels_per_point();
            // The panel's rect includes the gap kept around the view.
            let view_rect = central.response.rect - self.theme.panel_outer_margin();
            let view_size = view_rect.size() * pixels_per_point;
            let view_size_px = [view_size.x, view_size.y].map(|side| side.round() as u32);
            if self.export.match_view {
                let max_image_size = self.renderer.max_image_size();
                self.export.size = view_size_px.map(|side| side.clamp(1, max_image_size));
            }

            match export_action {
                Some(ExportAction::Save) => {
                    // Blocks this frame until the dialog closes, which is fine:
                    // nothing else can be clicked meanwhile anyway.
                    let path = rfd::FileDialog::new()
                        .add_filter("PNG image", &["png"])
                        .set_file_name("copcat.png")
                        .save_file();
                    if let Some(mut path) = path {
                        if path.extension().is_none() {
                            path.set_extension("png");
                        }
                        self.export.job = Some(ExportJob {
                            path,
                            size: self.export.size,
                            camera: framed_camera(camera, view_size_px, self.export.size),
                            clip,
                            pixels_per_point,
                        });
                        self.export.status = None;
                    }
                }
                Some(ExportAction::CopyCommand) => {
                    let args = render_args(
                        session,
                        &self.settings,
                        &self.heights,
                        &framed_camera(camera, view_size_px, self.export.size),
                        self.export.size,
                        pixels_per_point,
                    );
                    ui.ctx().copy_text(args.command_line());
                    self.export.status = Some(Ok("Copied a copcat render command".to_owned()));
                }
                None => {}
            }
        }

        match heights_action {
            Some(HeightsAction::Show(kind)) => self.show_height(kind),
            Some(HeightsAction::Reload) => {
                if let LoadState::Open(session) = &self.state {
                    self.url_input = session.info.source.clone();
                }
                self.heights = self.heights_draft.clone();
                self.start_loading(ui.ctx());
            }
            None => {}
        }

        self.finish_export(ui.ctx());
    }
}

impl ViewerApp {
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
