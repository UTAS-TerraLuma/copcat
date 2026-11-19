//! The Rendering section: how points look (colour, point size and eye-dome
//! lighting) and how much detail is drawn.

use eframe::egui;

use crate::classification;
use crate::config::ranges;
use crate::format::format_count;
use crate::lod::LodSettings;

use super::widgets::{Fold, NumberField, choice_combo, legend_color, prop_control, prop_row};
use crate::app::commands::Command;
use crate::app::session::Session;
use crate::app::settings::{ColorMode, PointSizeMode, ViewSettings};

/// A settings row for a number field, labelled with its name.
fn number_row(ui: &mut egui::Ui, field: NumberField<'_>) -> egui::Response {
    let name = field.name();
    prop_row(ui, name, |ui| ui.add(field))
}

/// The Rendering section's contents. Colour mode and eye-dome lighting ask
/// for commands; the other controls change `settings` directly.
pub(crate) fn rendering_ui(
    ui: &mut egui::Ui,
    settings: &mut ViewSettings,
    session: &Session,
    commands: &mut Vec<Command>,
) {
    prop_row(ui, "Colour", |ui| {
        choice_combo(ui, "color_mode", settings.color_mode.label(), |ui| {
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
        })
    });
    if settings.color_mode == ColorMode::Classification {
        class_legend_ui(ui, &session.classes_seen);
    }
    point_size_ui(ui, settings);

    let mut edl_enabled = settings.edl_enabled;
    let edl = prop_control(ui, "edl_enabled", |ui| {
        ui.checkbox(&mut edl_enabled, "Eye-dome lighting")
            .on_hover_text("Shades points by how much nearer their neighbours are, to show depth.")
    });
    if edl.changed() {
        commands.push(Command::ToggleEdl);
    }
    if settings.edl_enabled {
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
    }
    level_of_detail_ui(ui, &mut settings.lod);
}

/// Point size: Fixed (one size) or Adaptive (a fill factor, with the
/// smallest and largest sizes under Advanced).
fn point_size_ui(ui: &mut egui::Ui, settings: &mut ViewSettings) {
    let mode = &mut settings.point_size_mode;
    prop_row(ui, "Point size", |ui| {
        choice_combo(ui, "point_size_mode", mode.label(), |ui| {
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
    });
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
    if settings.point_size_mode == PointSizeMode::Adaptive {
        let size = &mut settings.adaptive_size;
        Fold::new("adaptive_size_advanced", "Advanced")
            .indented()
            .show(ui, |ui| {
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
        // Keep the range the right way round whichever field moved.
        size.max_px = size.max_px.max(size.min_px);
    }
}

/// How many points to draw and how much detail to load, under a small
/// heading after the settings for how points look.
fn level_of_detail_ui(ui: &mut egui::Ui, lod: &mut LodSettings) {
    ui.separator();
    ui.label(egui::RichText::new("Level of detail").weak());
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
    let spacing_hint = "The target spacing: nodes are refined until their points                         are at most this far apart on screen. Lower means more                         detail, up to the point budget. With adaptive point                         sizes, detail stops where points would be drawn bigger                         than their spacing (2 × Min ÷ Fill).";
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

/// A swatch and name for each class seen so far, in the control column
/// under the Colour drop-down.
fn class_legend_ui(ui: &mut egui::Ui, classes_seen: &[bool; 256]) {
    prop_control(ui, "class_legend", |ui| {
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
