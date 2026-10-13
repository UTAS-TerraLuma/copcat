use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui;

use crate::camera::OrbitCamera;
use crate::cloud::{self, LoadError, PointCloud};
use crate::renderer::PointRenderer;

pub struct ViewerApp {
    url_input: String,
    state: LoadState,
    renderer: PointRenderer,
    camera: OrbitCamera,
    /// Point diameter in logical pixels (scaled by the display's DPI when drawn).
    point_size: f32,
}

enum LoadState {
    Idle,
    /// A background thread is loading; its result arrives on `receiver`.
    Loading {
        source: String,
        receiver: Receiver<Result<PointCloud, LoadError>>,
    },
    Loaded(PointCloud),
    Failed {
        source: String,
        error: String,
    },
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
        let mut app = Self {
            url_input: initial_source.unwrap_or_default(),
            state: LoadState::Idle,
            renderer: PointRenderer::new(render_state),
            camera: OrbitCamera::default(),
            point_size: 2.0,
        };
        if !app.url_input.trim().is_empty() {
            app.start_loading(&cc.egui_ctx);
        }
        Ok(app)
    }

    fn start_loading(&mut self, ctx: &egui::Context) {
        let source = self.url_input.trim().to_owned();
        let (sender, receiver) = mpsc::channel();
        let ctx = ctx.clone();
        let input = source.clone();
        thread::spawn(move || {
            let result = cloud::load_root_node(&input);
            // Sending fails only if the user started another load and the old
            // receiver was dropped, in which case nobody wants this result.
            let _ = sender.send(result);
            // egui only redraws on input, so wake it to show the result.
            ctx.request_repaint();
        });
        // Replacing a previous `Loading` state drops its receiver, which is how
        // a superseded load gets ignored.
        self.state = LoadState::Loading { source, receiver };
    }

    fn poll_loading(&mut self) {
        let LoadState::Loading { source, receiver } = &self.state else {
            return;
        };
        self.state = match receiver.try_recv() {
            Ok(Ok(cloud)) => {
                let (min, max) = cloud.local_bounds();
                self.renderer.set_points(&cloud.positions, [min.z, max.z]);
                self.camera.frame_bounds(min, max);
                LoadState::Loaded(cloud)
            }
            Ok(Err(error)) => LoadState::Failed {
                source: source.clone(),
                error: error.to_string(),
            },
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => LoadState::Failed {
                source: source.clone(),
                error: "the loader thread crashed".to_owned(),
            },
        };
    }

    fn url_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("COPC URL:");
            let is_loading = matches!(self.state, LoadState::Loading { .. });
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_loading();

        egui::Panel::top("url_bar").show(ui, |ui| self.url_bar(ui));

        if let LoadState::Loaded(cloud) = &self.state {
            egui::Panel::left("cloud_info").show(ui, |ui| {
                cloud_summary(ui, cloud);
                ui.separator();
                ui.add(egui::Slider::new(&mut self.point_size, 1.0..=10.0).text("Point size"));
                ui.separator();
                ui.label("Drag: orbit\nRight-drag: pan\nScroll: zoom\nDouble-click: reset view");
            });
        }

        // The 3D view fills its panel edge to edge; the text states keep margins.
        let frame = match self.state {
            LoadState::Loaded(_) => egui::Frame::NONE,
            _ => egui::Frame::central_panel(ui.style()),
        };
        egui::CentralPanel::default()
            .frame(frame)
            .show(ui, |ui| match &self.state {
                LoadState::Idle => {
                    ui.label("Paste a COPC URL or file path above and press Load.");
                }
                LoadState::Loading { source, .. } => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(format!("Loading {source}"));
                    });
                }
                LoadState::Loaded(cloud) => point_view(
                    ui,
                    &mut self.renderer,
                    &mut self.camera,
                    cloud,
                    self.point_size,
                ),
                LoadState::Failed { source, error } => {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        format!("Failed to load {source}"),
                    );
                    ui.label(error);
                }
            });
    }
}

/// The interactive 3D viewport. Takes fields separately, rather than `&mut self`,
/// so the caller can keep borrowing `self.state` for `cloud` at the same time.
fn point_view(
    ui: &mut egui::Ui,
    renderer: &mut PointRenderer,
    camera: &mut OrbitCamera,
    cloud: &PointCloud,
    point_size: f32,
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
        let (min, max) = cloud.local_bounds();
        camera.frame_bounds(min, max);
    }

    // Render at physical pixel resolution so points stay crisp on high-DPI screens.
    let pixels_per_point = ui.ctx().pixels_per_point();
    let size_px = [
        (rect.width() * pixels_per_point).round() as u32,
        (rect.height() * pixels_per_point).round() as u32,
    ];
    let texture_id = renderer.render(size_px, camera, point_size * pixels_per_point);
    let full_uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    ui.painter()
        .image(texture_id, rect, full_uv, egui::Color32::WHITE);
}

fn cloud_summary(ui: &mut egui::Ui, cloud: &PointCloud) {
    let [min_x, min_y, min_z] = cloud.bounds_min;
    let [max_x, max_y, max_z] = cloud.bounds_max;
    let [ox, oy, oz] = cloud.origin;

    ui.heading("Point cloud");
    egui::Grid::new("cloud_summary")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            ui.label("Source");
            ui.label(&cloud.source);
            ui.end_row();

            ui.label("Total points");
            ui.label(cloud.total_points.to_string());
            ui.end_row();

            ui.label("Root node points");
            ui.label(cloud.positions.len().to_string());
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

            ui.label("Load time");
            ui.label(format!("{:.2} s", cloud.load_time.as_secs_f64()));
            ui.end_row();
        });
}
