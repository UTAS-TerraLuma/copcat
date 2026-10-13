use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;

use eframe::egui;

use crate::cloud::{self, LoadError, PointCloud};

#[derive(Default)]
pub struct ViewerApp {
    url_input: String,
    state: LoadState,
}

#[derive(Default)]
enum LoadState {
    #[default]
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
            Ok(Ok(cloud)) => LoadState::Loaded(cloud),
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

        egui::CentralPanel::default().show(ui, |ui| match &self.state {
            LoadState::Idle => {
                ui.label("Paste a COPC URL or file path above and press Load.");
            }
            LoadState::Loading { source, .. } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Loading {source}"));
                });
            }
            LoadState::Loaded(cloud) => cloud_summary(ui, cloud),
            LoadState::Failed { source, error } => {
                ui.colored_label(ui.visuals().error_fg_color, format!("Failed to load {source}"));
                ui.label(error);
            }
        });
    }
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
