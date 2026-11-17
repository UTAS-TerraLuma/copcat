//! Small layout helpers the panel sections share.

use eframe::egui;

use crate::theme::Theme;

/// One section of the Point cloud window, in a box filling its width.
pub(crate) fn section<R>(
    ui: &mut egui::Ui,
    theme: &Theme,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    theme
        .box_frame()
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
        .inner
}

/// A striped two-column grid of read-only values, whose rows are added with
/// [`stat_row`].
pub(crate) fn stat_grid(ui: &mut egui::Ui, id: &str, add_rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .striped(true)
        .show(ui, add_rows);
}

/// A row of a [`stat_grid`]: the label in secondary text, then the value,
/// cut short with "…" where the box is too narrow for it. Returns the value's
/// response, for hover text.
pub(crate) fn stat_row(
    ui: &mut egui::Ui,
    label: &str,
    value: impl Into<egui::WidgetText>,
) -> egui::Response {
    ui.weak(label);
    let value = ui
        .horizontal(|ui| {
            // The value's cell fills the row, so the stripes run across the
            // whole box, however short the value.
            ui.set_min_width(ui.available_width());
            ui.add(egui::Label::new(value).truncate())
        })
        .inner;
    ui.end_row();
    value
}

/// A value, such as a number, set in the monospaced font.
pub(crate) fn mono(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).monospace()
}

/// How wide `text` is on one line, in `style` unless it says otherwise.
pub(crate) fn text_width(
    ui: &egui::Ui,
    text: impl Into<egui::WidgetText>,
    style: egui::TextStyle,
) -> f32 {
    text.into()
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, style)
        .size()
        .x
}

/// A two-column grid of settings, labels on the left. Grids given the same
/// `label_width` line their controls up.
pub(crate) fn settings_grid(
    ui: &mut egui::Ui,
    id: &str,
    label_width: f32,
    add_rows: impl FnOnce(&mut egui::Ui),
) {
    egui::Grid::new(id)
        .num_columns(2)
        .min_col_width(label_width)
        .show(ui, add_rows);
}

/// A slider as wide as the rest of the row, leaving room for its value.
pub(crate) fn fitted_slider(ui: &mut egui::Ui, slider: egui::Slider<'_>) -> egui::Response {
    // Wide enough for the longest value the sliders show, such as a point
    // budget of "30.00 M".
    let value_width = text_width(ui, mono("00.00 M"), egui::TextStyle::Monospace)
        + 2.0 * ui.spacing().button_padding.x;
    let spacing = ui.spacing();
    let value_width = value_width.max(spacing.interact_size.x);
    let slider_width =
        (ui.available_width() - value_width - spacing.item_spacing.x).max(spacing.interact_size.y);
    ui.spacing_mut().slider_width = slider_width;
    ui.add(slider)
}

/// Opaque version of a debug line colour, for the legend.
pub(crate) fn legend_color([r, g, b, _]: [f32; 4]) -> egui::Color32 {
    let to_byte = |channel: f32| (channel * 255.0).round() as u8;
    egui::Color32::from_rgb(to_byte(r), to_byte(g), to_byte(b))
}
