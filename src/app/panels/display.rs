//! The display settings: colour, point size, eye-dome lighting and level of
//! detail, plus the debug bounds.

use eframe::egui;

use crate::classification;
use crate::format::format_count;

use super::widgets::{fitted_slider, legend_color, settings_grid, text_width};
use crate::app::session::{FAILED_COLOR, FETCHING_COLOR, LOADED_COLOR, QUEUED_COLOR, Session};
use crate::app::settings::{ColorMode, ViewSettings};

pub(crate) fn settings_ui(ui: &mut egui::Ui, settings: &mut ViewSettings, session: &Session) {
    // One label column for every grid here, so their controls line up.
    let label_width = ["Colour", "Point size", "Point budget", "Spacing (px)"]
        .map(|label| text_width(ui, label, egui::TextStyle::Body))
        .into_iter()
        .fold(0.0, f32::max);

    settings_grid(ui, "display_settings", label_width, |ui| {
        ui.weak("Colour");
        ui.horizontal_wrapped(|ui| {
            ui.radio_value(&mut settings.color_mode, ColorMode::Height, "Height");
            ui.add_enabled_ui(session.info.has_rgb, |ui| {
                ui.radio_value(&mut settings.color_mode, ColorMode::Rgb, "RGB")
                    .on_disabled_hover_text("This file has no colour");
            });
            ui.radio_value(&mut settings.color_mode, ColorMode::Classification, "Class");
        });
        ui.end_row();
    });
    if settings.color_mode == ColorMode::Classification {
        class_legend_ui(ui, &session.classes_seen);
    }
    settings_grid(ui, "point_settings", label_width, |ui| {
        ui.weak("Point size");
        fitted_slider(ui, egui::Slider::new(&mut settings.point_size, 1.0..=10.0));
        ui.end_row();
    });

    ui.checkbox(&mut settings.edl_enabled, "Eye-dome lighting")
        .on_hover_text("Shades points by how much nearer their neighbours are, to show depth.");
    if settings.edl_enabled {
        // Narrower by the indent, so these sliders line up with the others.
        let indented_width = label_width - ui.spacing().indent;
        ui.indent("edl_options", |ui| {
            settings_grid(ui, "edl_settings", indented_width, |ui| {
                ui.weak("Strength");
                fitted_slider(ui, egui::Slider::new(&mut settings.edl.strength, 0.1..=5.0));
                ui.end_row();
                ui.weak("Radius (px)");
                fitted_slider(
                    ui,
                    egui::Slider::new(&mut settings.edl.radius_px, 1.0..=4.0),
                );
                ui.end_row();
            });
        });
    }

    ui.separator();
    settings_grid(ui, "lod_settings", label_width, |ui| {
        ui.weak("Point budget");
        fitted_slider(
            ui,
            egui::Slider::new(&mut settings.lod.point_budget, 500_000..=30_000_000)
                .logarithmic(true)
                .custom_formatter(|value, _| format_count(value as u64)),
        );
        ui.end_row();
        let spacing_hint = "The target spacing: nodes are refined until their points are at \
                            most this far apart on screen. Lower means more detail, up to the \
                            point budget.";
        ui.weak("Spacing (px)").on_hover_text(spacing_hint);
        fitted_slider(
            ui,
            egui::Slider::new(&mut settings.lod.target_spacing_px, 0.5..=20.0).logarithmic(true),
        )
        .on_hover_text(spacing_hint);
        ui.end_row();
    });

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

/// A swatch and name for each class seen so far.
pub(crate) fn class_legend_ui(ui: &mut egui::Ui, classes_seen: &[bool; 256]) {
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
