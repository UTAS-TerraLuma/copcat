//! How points look: colour, point size and eye-dome lighting, and the
//! Performance section's level of detail settings.

use eframe::egui;

use crate::classification;
use crate::config::ranges;
use crate::format::format_count;
use crate::lod::LodSettings;

use super::widgets::{NumberField, choice_combo, legend_color, settings_grid, text_width};
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

/// A settings row: the field's name in secondary text, then the field,
/// labelled by it.
fn number_row(ui: &mut egui::Ui, field: NumberField<'_>) -> egui::Response {
    let label = ui.weak(field.name());
    let field = ui.add(field).labelled_by(label.id);
    ui.end_row();
    field
}

/// The Display section. Colour mode and eye-dome lighting ask for commands;
/// the other controls change `settings` directly.
pub(crate) fn display_ui(
    ui: &mut egui::Ui,
    settings: &mut ViewSettings,
    session: &Session,
    commands: &mut Vec<Command>,
) {
    // One label column for every grid here, so their controls line up.
    let label_width = label_width(ui, &["Colour", "Point size"]);

    settings_grid(ui, "display_settings", label_width, |ui| {
        let label = ui.weak("Colour");
        let labels = ColorMode::ALL.map(ColorMode::label);
        choice_combo(
            ui,
            "color_mode",
            settings.color_mode.label(),
            labels,
            |ui| {
                for mode in ColorMode::ALL {
                    let available = mode != ColorMode::Rgb || session.info.has_rgb;
                    let option = ui
                        .add_enabled(
                            available,
                            egui::Button::selectable(settings.color_mode == mode, mode.label()),
                        )
                        .on_disabled_hover_text("This file has no colour");
                    if option.clicked() {
                        commands.push(Command::SetColorMode(mode));
                    }
                }
            },
        )
        .labelled_by(label.id);
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
        // Narrower by the indent, so these fields line up with the others.
        let indented_width = label_width - ui.spacing().indent;
        ui.indent("edl_options", |ui| {
            settings_grid(ui, "edl_settings", indented_width, |ui| {
                number_row(
                    ui,
                    NumberField::new(&mut settings.edl.strength, ranges::EDL_STRENGTH, "Strength")
                        .step(0.1)
                        .precision(0.01),
                );
                number_row(
                    ui,
                    NumberField::new(
                        &mut settings.edl.radius_px,
                        ranges::EDL_RADIUS,
                        "Radius (px)",
                    )
                    .step(0.1)
                    .precision(0.01),
                );
            });
        });
    }
}

/// Point size: Fixed (one size) or Adaptive (a fill factor, with the
/// smallest and largest sizes under Advanced).
fn point_size_ui(ui: &mut egui::Ui, settings: &mut ViewSettings, label_width: f32) {
    settings_grid(ui, "point_settings", label_width, |ui| {
        let label = ui.weak("Point size");
        let mode = &mut settings.point_size_mode;
        let labels = PointSizeMode::ALL.map(PointSizeMode::label);
        choice_combo(ui, "point_size_mode", mode.label(), labels, |ui| {
            for option in PointSizeMode::ALL {
                let hint = match option {
                    PointSizeMode::Adaptive => {
                        "Size points by how far apart they are on screen, so surfaces look \
                         filled close up without heaps of overlap far away"
                    }
                    PointSizeMode::Fixed => "Draw every point the same size",
                };
                ui.selectable_value(mode, option, option.label())
                    .on_hover_text(hint);
            }
        })
        .labelled_by(label.id);
        ui.end_row();
        match settings.point_size_mode {
            PointSizeMode::Adaptive => {
                number_row(
                    ui,
                    NumberField::new(&mut settings.adaptive_size.fill, ranges::FILL, "Fill")
                        .step(0.1)
                        .precision(0.01),
                )
                .on_hover_text(
                    "Point size as a multiple of the points' spacing on screen. Higher fills \
                     gaps, lower shows single points.",
                );
            }
            PointSizeMode::Fixed => {
                number_row(
                    ui,
                    NumberField::new(&mut settings.point_size, ranges::POINT_SIZE, "Size (px)")
                        .step(0.5)
                        .precision(0.1),
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
                    number_row(
                        ui,
                        NumberField::new(&mut size.min_px, ranges::MIN_POINT_SIZE, "Min (px)")
                            .step(0.5)
                            .precision(0.1)
                            .logarithmic(),
                    )
                    .on_hover_text(
                        "The smallest points are drawn. Detail finer than this can show isn't \
                         loaded.",
                    );
                    number_row(
                        ui,
                        NumberField::new(&mut size.max_px, ranges::MAX_POINT_SIZE, "Max (px)")
                            .step(1.0)
                            .precision(0.1)
                            .logarithmic(),
                    )
                    .on_hover_text("The biggest points are drawn, close up past the finest detail");
                });
            });
        // Keep the range the right way round whichever field moved.
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
                number_row(
                    ui,
                    NumberField::new(&mut lod.point_budget, ranges::POINT_BUDGET, "Point budget")
                        .step(1e6)
                        .precision(1e4)
                        .logarithmic()
                        .formatter(|value| format_count(value as u64))
                        .parser(parse_count),
                )
                .on_hover_text("The most points to draw at once");
                let spacing_hint = "The target spacing: nodes are refined until their points \
                                    are at most this far apart on screen. Lower means more \
                                    detail, up to the point budget. With adaptive point \
                                    sizes, detail stops where points would be drawn bigger \
                                    than their spacing (2 × Min ÷ Fill).";
                number_row(
                    ui,
                    NumberField::new(
                        &mut lod.target_spacing_px,
                        ranges::SPACING_PX,
                        "Spacing (px)",
                    )
                    .step(0.5)
                    .precision(0.1)
                    .logarithmic(),
                )
                .on_hover_text(spacing_hint);
            });
        });
}

/// A count typed in, as it's shown or as plain digits: `5 M`, `2.5m`,
/// `750 k`, `5000000` or `5,000,000`.
fn parse_count(text: &str) -> Option<f64> {
    let text: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, ',' | '_'))
        .collect::<String>()
        .to_lowercase();
    let (number, scale) = match text.strip_suffix('m') {
        Some(number) => (number, 1e6),
        None => match text.strip_suffix('k') {
            Some(number) => (number, 1e3),
            None => (text.as_str(), 1.0),
        },
    };
    let value = number.parse::<f64>().ok()? * scale;
    value.is_finite().then_some(value.round())
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

#[cfg(test)]
mod tests {
    use super::parse_count;

    #[test]
    fn parse_count_should_read_counts_as_shown_and_as_digits() {
        assert_eq!(parse_count("5.00 M"), Some(5e6));
        assert_eq!(parse_count("2.5m"), Some(2.5e6));
        assert_eq!(parse_count("750 k"), Some(750e3));
        assert_eq!(parse_count("5,000,000"), Some(5e6));
        assert_eq!(parse_count("lots"), None);
    }
}
