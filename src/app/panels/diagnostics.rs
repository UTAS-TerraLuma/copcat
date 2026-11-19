//! The Diagnostics section: how streaming is going, timings and the debug
//! node bounds. Hidden until asked for with
//! [`Command::ToggleDiagnostics`].

use eframe::egui;

use crate::cloud::CloudInfo;
use crate::format::format_count;
use crate::renderer::PointRenderer;

use crate::theme::Theme;

use super::widgets::{
    Fold, folding_section, legend_color, mono, prop_control, stat_grid, stat_row, stat_row_with,
};
use crate::app::commands::Command;
use crate::app::session::{FAILED_COLOR, FETCHING_COLOR, LOADED_COLOR, QUEUED_COLOR, Session};
use crate::app::settings::DiagnosticsSettings;

/// A folding section, open while `settings.visible`. Opening or closing it
/// asks for [`Command::ToggleDiagnostics`].
pub(crate) fn diagnostics_ui(
    ui: &mut egui::Ui,
    theme: &Theme,
    session: &Session,
    renderer: &PointRenderer,
    settings: &mut DiagnosticsSettings,
    commands: &mut Vec<Command>,
) {
    let visible = settings.visible;
    let fold = Fold::new("diagnostics", "Diagnostics").open(visible);
    let shown = folding_section(ui, theme, fold, |ui| {
        streaming_stats(ui, session, renderer);
        timings(ui, &session.info);
        bounds_ui(ui, settings);
    });
    if shown.header.clicked() {
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

        if let Some(error) = &session.last_error {
            let error_color = ui.visuals().error_fg_color;
            stat_row_with(ui, egui::RichText::new("Failed").color(error_color), |ui| {
                let failed = mono(format!("{} nodes", session.failed.len()));
                ui.horizontal(|ui| ui.add(egui::Label::new(failed).truncate()))
                    .inner
            })
            .on_hover_text("Nodes that failed to load. Retry in the status bar asks again.");
            stat_row_with(
                ui,
                egui::RichText::new("Last error").color(error_color),
                |ui| ui.add(egui::Label::new(error.as_str()).wrap()),
            );
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
    prop_control(ui, "show_bounds", |ui| {
        ui.checkbox(&mut settings.show_bounds, "Show node bounds")
            .on_hover_text("Draw the octree cube of each node that's wanted or downloading");
    });
    if settings.show_bounds {
        prop_control(ui, "bounds_options", |ui| {
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
