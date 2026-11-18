//! The open cloud's summary: what it is and how big.

use eframe::egui;

use crate::cloud::CloudInfo;
use crate::format::{file_name, format_count};

use super::widgets::{mono, stat_grid, stat_row};

pub(crate) fn cloud_summary(ui: &mut egui::Ui, info: &CloudInfo) {
    let [min_x, min_y, min_z] = info.bounds_min;
    let [max_x, max_y, max_z] = info.bounds_max;

    stat_grid(ui, "cloud_summary", |ui| {
        // Full URLs run to hundreds of characters; the file name is enough
        // to identify the cloud, with the full source on hover.
        stat_row(ui, "Source", file_name(&info.source)).on_hover_text(&info.source);
        stat_row(ui, "Total points", mono(format_count(info.total_points)));
        stat_row(
            ui,
            "Extent (m)",
            mono(format!(
                "{:.1} × {:.1} × {:.1}",
                max_x - min_x,
                max_y - min_y,
                max_z - min_z
            )),
        );
    });
}
