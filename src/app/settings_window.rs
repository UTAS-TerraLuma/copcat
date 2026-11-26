//! The Settings window (`settings.open`, File > Settings…): the theme, the
//! default rendering settings and the settings files, laid out as the
//! Point cloud panel's sections are. It works with no cloud open.
//!
//! Like the panel, it changes rendering settings directly and asks for a
//! [`Command`] for everything else: choosing a theme, saving the defaults,
//! opening a settings file.

use eframe::egui;

use crate::theme::THEMES;

use super::commands::{Command, SettingsCommand};
use super::panels::{
    CloudFacts, Fold, choice_combo, folding_section, prop_row, rendering_ui, save_defaults_button,
};
use super::{LoadState, ViewerApp};

/// How wide the window starts, in points: as wide as the panel's default.
const WIDTH: f32 = 340.0;
/// How far below the top of the window it starts, in points.
const TOP: f32 = 60.0;

impl ViewerApp {
    /// Draws the Settings window, if it's open, queueing what it asks for.
    pub(super) fn settings_window_ui(&mut self, ctx: &egui::Context) {
        if !self.settings_window_open {
            return;
        }
        let cloud = match &self.state {
            LoadState::Open(session) => Some(CloudFacts::of(session)),
            _ => None,
        };
        let theme = &self.theme;
        let view = &mut self.settings.view;
        let commands = &mut self.pending;
        let user_file = self.user_config.user_file_name();
        let mut open = true;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(WIDTH)
            .pivot(egui::Align2::CENTER_TOP)
            .default_pos(ctx.content_rect().center_top() + egui::vec2(0.0, TOP))
            .show(ctx, |ui| {
                // As tall as its contents, scrolling only in a short window.
                // The minimum too, else it starts as small as the window
                // was last frame and never grows.
                let max_height = (ctx.content_rect().height() - 2.0 * TOP).max(120.0);
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .min_scrolled_height(max_height)
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        let fold =
                            Fold::new("settings_appearance", "Appearance").default_open(true);
                        folding_section(ui, theme, fold, |ui| {
                            prop_row(ui, "Theme", |ui| {
                                choice_combo(ui, "theme", theme.name.as_str(), |ui| {
                                    for built_in in THEMES {
                                        let current = theme.id() == Some(built_in.id);
                                        let name = (built_in.make)().name;
                                        let option = egui::Button::selectable(current, name);
                                        if ui.add(option).clicked() {
                                            commands.push(Command::SetTheme(built_in.id));
                                        }
                                    }
                                })
                                .on_hover_text("Shown at once and saved to the user settings file")
                            });
                        });

                        let fold = Fold::new("settings_rendering", "Default rendering settings")
                            .default_open(true);
                        folding_section(ui, theme, fold, |ui| {
                            rendering_ui(ui, view, cloud, commands);
                            ui.separator();
                            save_defaults_button(ui, false, user_file.as_deref(), commands);
                        });

                        let fold = Fold::new("settings_files", "Settings files").default_open(true);
                        folding_section(ui, theme, fold, |ui| {
                            let files = [
                                (
                                    "Open user settings (JSON)",
                                    SettingsCommand::OpenUser,
                                    "Your settings, for every folder: created if it's missing",
                                ),
                                (
                                    "Open workspace settings (JSON)",
                                    SettingsCommand::OpenWorkspace,
                                    "This folder's settings, over yours: created if it's missing",
                                ),
                            ];
                            for (title, command, hover) in files {
                                if ui.button(title).on_hover_text(hover).clicked() {
                                    commands.push(Command::Settings(command));
                                }
                            }
                        });
                    });
            });
        self.settings_window_open = open;
    }
}
