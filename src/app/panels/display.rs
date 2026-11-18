//! How points look: colour, point size and eye-dome lighting, and the
//! Performance section's level of detail settings.

use eframe::egui;

use crate::classification;
use crate::config::ranges;
use crate::format::format_count;
use crate::lod::LodSettings;

use super::widgets::{fitted_slider, legend_color, settings_grid, text_width};
use crate::app::commands::Command;
use crate::app::session::Session;
use crate::app::settings::{ColorMode, PointSizeMode, ViewSettings};

/// The width of the widest label, so grids given it line their controls up.
fn label_width(ui: &egui::Ui, labels: &[&str]) -> f32 {
    labels
        .iter()
        .map(|label| text_width(ui, *label, egui::TextStyle::Body))
        .fold(0.0, f32::max)
}

/// A settings row: `label` in secondary text, then a slider labelled by it.
fn slider_row(ui: &mut egui::Ui, label: &str, slider: egui::Slider<'_>) -> egui::Response {
    let label = ui.weak(label);
    let slider = fitted_slider(ui, slider).labelled_by(label.id);
    ui.end_row();
    slider
}

/// The Display section. Colour mode and eye-dome lighting ask for commands;
/// the sliders change `settings` directly.
pub(crate) fn display_ui(
    ui: &mut egui::Ui,
    settings: &mut ViewSettings,
    session: &Session,
    commands: &mut Vec<Command>,
) {
    // One label column for every grid here, so their controls line up.
    let label_width = label_width(ui, &["Colour", "Point size"]);

    settings_grid(ui, "display_settings", label_width, |ui| {
        ui.weak("Colour");
        ui.horizontal_wrapped(|ui| {
            for mode in ColorMode::ALL {
                let available = mode != ColorMode::Rgb || session.info.has_rgb;
                ui.add_enabled_ui(available, |ui| {
                    let radio = ui
                        .radio(settings.color_mode == mode, mode.label())
                        .on_disabled_hover_text("This file has no colour");
                    if radio.clicked() {
                        commands.push(Command::SetColorMode(mode));
                    }
                });
            }
        });
        ui.end_row();
    });
    if settings.color_mode == ColorMode::Classification {
        class_legend_ui(ui, &session.classes_seen);
    }
    point_size_ui(ui, settings, label_width);

    let mut edl_enabled = settings.edl_enabled;
    if ui
        .checkbox(&mut edl_enabled, "Eye-dome lighting")
        .on_hover_text("Shades points by how much nearer their neighbours are, to show depth.")
        .changed()
    {
        commands.push(Command::ToggleEdl);
    }
    if settings.edl_enabled {
        // Narrower by the indent, so these sliders line up with the others.
        let indented_width = label_width - ui.spacing().indent;
        ui.indent("edl_options", |ui| {
            settings_grid(ui, "edl_settings", indented_width, |ui| {
                slider_row(
                    ui,
                    "Strength",
                    egui::Slider::new(&mut settings.edl.strength, ranges::EDL_STRENGTH),
                );
                slider_row(
                    ui,
                    "Radius (px)",
                    egui::Slider::new(&mut settings.edl.radius_px, ranges::EDL_RADIUS),
                );
            });
        });
    }
}

/// Point size: Adaptive (a fill factor, with the smallest and largest sizes
/// under Advanced) or Fixed (one size).
fn point_size_ui(ui: &mut egui::Ui, settings: &mut ViewSettings, label_width: f32) {
    settings_grid(ui, "point_settings", label_width, |ui| {
        ui.weak("Point size");
        ui.horizontal_wrapped(|ui| {
            for mode in PointSizeMode::ALL {
                let hint = match mode {
                    PointSizeMode::Adaptive => {
                        "Size points by how far apart they are on screen, so surfaces look \
                         filled close up without heaps of overlap far away"
                    }
                    PointSizeMode::Fixed => "Draw every point the same size",
                };
                ui.radio_value(&mut settings.point_size_mode, mode, mode.label())
                    .on_hover_text(hint);
            }
        });
        ui.end_row();
        match settings.point_size_mode {
            PointSizeMode::Adaptive => {
                slider_row(
                    ui,
                    "Fill",
                    egui::Slider::new(&mut settings.adaptive_size.fill, ranges::FILL),
                )
                .on_hover_text(
                    "Point size as a multiple of the points' spacing on screen. Higher fills \
                     gaps, lower shows single points.",
                );
            }
            PointSizeMode::Fixed => {
                slider_row(
                    ui,
                    "Size (px)",
                    egui::Slider::new(&mut settings.point_size, ranges::POINT_SIZE),
                );
            }
        }
    });
    if settings.point_size_mode == PointSizeMode::Adaptive {
        let size = &mut settings.adaptive_size;
        egui::CollapsingHeader::new("Advanced")
            .id_salt("adaptive_size_advanced")
            .show(ui, |ui| {
                let label_width = label_width - ui.spacing().indent;
                settings_grid(ui, "adaptive_size_settings", label_width, |ui| {
                    slider_row(
                        ui,
                        "Min (px)",
                        egui::Slider::new(&mut size.min_px, ranges::MIN_POINT_SIZE)
                            .logarithmic(true),
                    )
                    .on_hover_text(
                        "The smallest points are drawn. Detail finer than this can show isn't \
                         loaded.",
                    );
                    slider_row(
                        ui,
                        "Max (px)",
                        egui::Slider::new(&mut size.max_px, ranges::MAX_POINT_SIZE)
                            .logarithmic(true),
                    )
                    .on_hover_text("The biggest points are drawn, close up past the finest detail");
                });
            });
        // Keep the range the right way round whichever slider moved.
        size.max_px = size.max_px.max(size.min_px);
    }
}

/// The Performance section, collapsed at first: how many points to draw and
/// how much detail to load.
pub(crate) fn performance_ui(ui: &mut egui::Ui, lod: &mut LodSettings) {
    egui::CollapsingHeader::new("Performance")
        .id_salt("performance")
        .show(ui, |ui| {
            let label_width = label_width(ui, &["Point budget", "Spacing (px)"]);
            settings_grid(ui, "lod_settings", label_width, |ui| {
                slider_row(
                    ui,
                    "Point budget",
                    egui::Slider::new(&mut lod.point_budget, ranges::POINT_BUDGET)
                        .logarithmic(true)
                        .custom_formatter(|value, _| format_count(value as u64)),
                )
                .on_hover_text("The most points to draw at once");
                let spacing_hint = "The target spacing: nodes are refined until their points \
                                    are at most this far apart on screen. Lower means more \
                                    detail, up to the point budget. With adaptive point \
                                    sizes, detail stops where points would be drawn bigger \
                                    than their spacing (2 × Min ÷ Fill).";
                slider_row(
                    ui,
                    "Spacing (px)",
                    egui::Slider::new(&mut lod.target_spacing_px, ranges::SPACING_PX)
                        .logarithmic(true),
                )
                .on_hover_text(spacing_hint);
            });
        });
}

/// A swatch and name for each class seen so far.
fn class_legend_ui(ui: &mut egui::Ui, classes_seen: &[bool; 256]) {
    ui.indent("class_legend", |ui| {
        for class in (0..=u8::MAX).filter(|&class| classes_seen[usize::from(class)]) {
            let [r, g, b] = classification::color(class);
            // Only the swatch is coloured: dark classes would be hard to read.
            ui.horizontal(|ui| {
                ui.colored_label(legend_color([r, g, b, 1.0]), "■");
                ui.add(
                    egui::Label::new(format!("{class} {}", classification::name(class))).truncate(),
                );
            });
        }
    });
}
