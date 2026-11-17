//! Read-only facts about the open cloud and how streaming it is going.

use eframe::egui;

use crate::cloud::CloudInfo;
use crate::format::{file_name, format_count};
use crate::renderer::PointRenderer;

use super::widgets::{mono, stat_grid, stat_row};
use crate::app::session::Session;

pub(crate) fn cloud_summary(ui: &mut egui::Ui, info: &CloudInfo) {
    let [min_x, min_y, min_z] = info.bounds_min;
    let [max_x, max_y, max_z] = info.bounds_max;
    let [ox, oy, oz] = info.origin;

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
        stat_row(ui, "Centre", mono(format!("{ox:.2}, {oy:.2}, {oz:.2}")))
            .on_hover_text("The octree's centre: X, Y and Z");
        stat_row(
            ui,
            "Open time",
            mono(format!("{:.2} s", info.open_time.as_secs_f64())),
        );
    });
}

pub(crate) fn streaming_stats(ui: &mut egui::Ui, session: &Session, renderer: &PointRenderer) {
    let selected = session.selection.keys.len();
    let drawn = session
        .selection
        .keys
        .iter()
        .filter(|key| renderer.has_node(key))
        .count();

    stat_grid(ui, "streaming_stats", |ui| {
        stat_row(
            ui,
            "Points drawn",
            mono(format!(
                "{} of {}",
                format_count(session.drawn_points(renderer)),
                format_count(session.selection.points)
            )),
        )
        .on_hover_text("Points drawn, of those selected for the view");
        stat_row(ui, "Nodes drawn", mono(format!("{drawn} of {selected}")))
            .on_hover_text("Nodes drawn, of those selected for the view");
        stat_row(
            ui,
            "Nodes in file",
            mono(session.octree.node_count().to_string()),
        );
        stat_row(
            ui,
            "Requested",
            mono(format!(
                "{} ({} downloading)",
                session.requested.len(),
                session.fetching.len()
            )),
        )
        .on_hover_text("Nodes waiting to load, and how many of those are downloading");
        stat_row(
            ui,
            "GPU cache",
            mono(format!(
                "{} in {} nodes",
                format_count(renderer.resident_points()),
                renderer.node_count()
            )),
        )
        .on_hover_text("Points kept on the GPU, and the nodes they're in");

        if let Some(error) = session.failed.values().next() {
            ui.colored_label(ui.visuals().error_fg_color, "Failed");
            ui.add(egui::Label::new(mono(format!("{} nodes", session.failed.len()))).truncate())
                .on_hover_text(error);
            ui.end_row();
        }
    });
}
