use eframe::egui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("COPC Viewer")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        "COPC Viewer",
        options,
        Box::new(|_cc| Ok(Box::new(ViewerApp::default()))),
    )
}

#[derive(Default)]
struct ViewerApp {
    url_input: String,
    loaded_url: Option<String>,
}

impl eframe::App for ViewerApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("url_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("COPC URL:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.url_input)
                        .hint_text("https://… or C:\\path\\to\\file.copc.laz")
                        .desired_width(f32::INFINITY),
                );
                let enter_pressed =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Load").clicked() || enter_pressed {
                    self.loaded_url = Some(self.url_input.trim().to_owned());
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| match &self.loaded_url {
            Some(url) => ui.label(format!("Would load: {url}")),
            None => ui.label("Paste a COPC URL above and press Load."),
        });
    }
}
