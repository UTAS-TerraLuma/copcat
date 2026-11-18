//! The Diagnostics section: how streaming is going, timings and the debug
//! node bounds. Hidden until asked for with
//! [`Command::ToggleDiagnostics`].

use eframe::egui;

use crate::cloud::CloudInfo;
use crate::format::format_count;
use crate::renderer::PointRenderer;

use super::widgets::{legend_color, mono, stat_grid, stat_row};
use crate::app::commands::Command;
use crate::app::session::{FAILED_COLOR, FETCHING_COLOR, LOADED_COLOR, QUEUED_COLOR, Session};
use crate::app::settings::DiagnosticsSettings;

/// A collapsing section, open while `settings.visible`. Opening or closing
/// it asks for [`Command::ToggleDiagnostics`].
pub(crate) fn diagnostics_ui(
    ui: &mut egui::Ui,
    session: &Session,
    renderer: &PointRenderer,
    settings: &mut DiagnosticsSettings,
    commands: &mut Vec<Command>,
) {
    let header = egui::CollapsingHeader::new("Diagnostics")
        .id_salt("diagnostics")
        .open(Some(settings.visible))
        .show(ui, |ui| {
            streaming_stats(ui, session, renderer);
            timings(ui, &session.info);
            bounds_ui(ui, settings);
        });
    if header.header_response.clicked() {
        commands.push(Command::ToggleDiagnostics);
    }
}

fn streaming_stats(ui: &mut egui::Ui, session: &Session, renderer: &PointRenderer) {
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

fn timings(ui: &mut egui::Ui, info: &CloudInfo) {
    let [ox, oy, oz] = info.origin;
    stat_grid(ui, "cloud_timings", |ui| {
        stat_row(ui, "Centre", mono(format!("{ox:.2}, {oy:.2}, {oz:.2}")))
            .on_hover_text("The octree's centre: X, Y and Z");
        stat_row(
            ui,
            "Open time",
            mono(format!("{:.2} s", info.open_time.as_secs_f64())),
        );
    });
}

/// The debug node bounds, and their legend.
fn bounds_ui(ui: &mut egui::Ui, settings: &mut DiagnosticsSettings) {
    ui.checkbox(&mut settings.show_bounds, "Show node bounds")
        .on_hover_text("Draw the octree cube of each node that's wanted or downloading");
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
