//! The status bar along the foot of the window: how many points are drawn,
//! whether data is still streaming, and nodes that failed (with a retry).
//! The full numbers are in the Diagnostics section.

use std::f32::consts::TAU;

use eframe::egui;

use crate::app::LoadState;
use crate::app::commands::Command;
use crate::format::format_count;
use crate::renderer::PointRenderer;
use crate::theme::Theme;

/// What the status bar says, taken from the app's state.
#[derive(Clone, Debug, Default, PartialEq)]
struct Status {
    opening: bool,
    open_failed: bool,
    /// Points drawn, once a cloud is open.
    drawn: Option<u64>,
    /// Nodes asked for and not loaded yet.
    waiting: usize,
    /// Nodes that failed, and the last error.
    failed: Option<(usize, String)>,
}

impl Status {
    fn of(state: &LoadState, renderer: &PointRenderer) -> Self {
        match state {
            LoadState::Idle => Self::default(),
            LoadState::Opening { .. } => Self {
                opening: true,
                ..Self::default()
            },
            LoadState::Open(session) => Self {
                drawn: Some(session.drawn_points(renderer)),
                waiting: session.requested.len(),
                failed: session
                    .last_error
                    .as_ref()
                    .map(|error| (session.failed.len(), error.clone())),
                ..Self::default()
            },
            LoadState::Failed { .. } => Self {
                open_failed: true,
                ..Self::default()
            },
        }
    }
}

/// One line of small, faint text, left to right: points drawn, loading,
/// failures. Empty when there's nothing to say, but always the same height,
/// so the panels above don't move.
pub(crate) fn status_bar(
    ui: &mut egui::Ui,
    state: &LoadState,
    renderer: &PointRenderer,
    theme: &Theme,
    commands: &mut Vec<Command>,
) {
    status_line(ui, &Status::of(state, renderer), theme, commands);
}

fn status_line(ui: &mut egui::Ui, status: &Status, theme: &Theme, commands: &mut Vec<Command>) {
    let text = |text: String| {
        egui::RichText::new(text)
            .small()
            .color(theme.colors.text_secondary)
    };
    ui.horizontal_centered(|ui| {
        if status.opening {
            spinner(ui, theme, theme.fonts.small_size);
            ui.label(text("Opening…".to_owned()));
        }
        if let Some(drawn) = status.drawn {
            let label = ui
                .label(text(format!("{} points drawn", format_count(drawn))))
                .on_hover_text("Points drawn in the 3D view");
            // Kept as wide as the widest count ("000.00 M"), so what follows
            // doesn't shuffle along as the count changes.
            let widest = ui
                .painter()
                .layout_no_wrap(
                    "000.00 M points drawn".to_owned(),
                    egui::TextStyle::Small.resolve(ui.style()),
                    theme.colors.text_secondary,
                )
                .size()
                .x;
            ui.add_space((widest - label.rect.width()).max(0.0));
        }
        if let Some(waiting) = loading_text(status.waiting) {
            spinner(ui, theme, theme.fonts.small_size);
            ui.label(text(waiting))
                .on_hover_text("Nodes waiting to load. See Diagnostics for more.");
        }
        if let Some((count, error)) = &status.failed {
            let failed = egui::RichText::new(failed_text(*count))
                .small()
                .color(theme.colors.error);
            ui.label(failed)
                .on_hover_text(format!("The last error: {error}"));
            let retry = egui::Button::new(egui::RichText::new("Retry").small()).small();
            if ui
                .add(retry)
                .on_hover_text("Ask for the failed nodes again")
                .clicked()
            {
                commands.push(Command::RetryFailedNodes);
            }
        }
        if status.open_failed {
            let failed = egui::RichText::new("Failed to open")
                .small()
                .color(theme.colors.error);
            ui.label(failed);
        }
    });
}

/// A small, thin arc going round in the faintest text colour. egui's
/// spinner is heavier, and as dark as the strongest text. Sized to sit
/// beside text of `size` points.
pub(crate) fn spinner(ui: &mut egui::Ui, theme: &Theme, size: f32) {
    let side = (size * 0.9).round();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    // Animated off egui's clock, so tests can step it.
    ui.ctx().request_repaint();
    let time = ui.input(|input| input.time) as f32;
    let start = (time * 1.25).fract() * TAU;
    let sweep = 0.7 * TAU;
    let radius = side / 2.0 - 1.0;
    let points = (0..=24)
        .map(|i| {
            let angle = start + sweep * i as f32 / 24.0;
            rect.center() + radius * egui::vec2(angle.cos(), angle.sin())
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(1.25, theme.colors.text_tertiary),
    ));
}

/// What's still loading, if anything.
fn loading_text(waiting: usize) -> Option<String> {
    match waiting {
        0 => None,
        1 => Some("Loading 1 node".to_owned()),
        _ => Some(format!("Loading {waiting} nodes")),
    }
}

fn failed_text(failed: usize) -> String {
    match failed {
        1 => "1 node failed".to_owned(),
        _ => format!("{failed} nodes failed"),
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    #[test]
    fn loading_text_should_be_none_once_everything_has_loaded() {
        assert_eq!(loading_text(0), None);
    }

    #[test]
    fn loading_text_should_count_nodes() {
        assert_eq!(loading_text(1).as_deref(), Some("Loading 1 node"));
        assert_eq!(loading_text(12).as_deref(), Some("Loading 12 nodes"));
    }

    #[test]
    fn failed_text_should_count_nodes() {
        assert_eq!(failed_text(1), "1 node failed");
        assert_eq!(failed_text(3), "3 nodes failed");
    }

    /// The status line's labels for `status`: text and rect, left to right.
    fn labels(status: Status) -> Vec<(String, egui::Rect)> {
        let theme = Theme::default();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(600.0, 40.0))
            .build_ui(move |ui| {
                theme.apply(ui.ctx());
                status_line(ui, &status, &theme, &mut Vec::new());
            });
        harness.step();
        harness
            .query_all_by_role(egui::accesskit::Role::Label)
            .filter_map(|node| Some((node.value()?, node.rect())))
            .collect()
    }

    fn left_of(labels: &[(String, egui::Rect)], prefix: &str) -> f32 {
        labels
            .iter()
            .find(|(text, _)| text.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix:?} in {labels:?}"))
            .1
            .left()
    }

    #[test]
    fn points_drawn_should_come_first_then_loading_then_failures() {
        let labels = labels(Status {
            drawn: Some(7739),
            waiting: 3,
            failed: Some((2, "503".to_owned())),
            ..Status::default()
        });
        let texts: Vec<_> = labels.iter().map(|(text, _)| text.as_str()).collect();
        assert_eq!(
            texts,
            ["7.7 k points drawn", "Loading 3 nodes", "2 nodes failed"]
        );
        let points = left_of(&labels, "7.7 k");
        assert!(points < left_of(&labels, "Loading"));
        assert!(left_of(&labels, "Loading") < left_of(&labels, "2 nodes"));
        // At the line's left edge: the harness's 8-point margin.
        assert!((points - 8.0).abs() < 0.5, "{points}");
    }

    #[test]
    fn points_drawn_and_loading_should_stay_put() {
        let idle = labels(Status {
            drawn: Some(5),
            ..Status::default()
        });
        let small = labels(Status {
            drawn: Some(5),
            waiting: 1,
            ..Status::default()
        });
        let big = labels(Status {
            drawn: Some(123_456_789),
            waiting: 1,
            ..Status::default()
        });
        assert_eq!(left_of(&idle, "5 points"), left_of(&small, "5 points"));
        assert_eq!(left_of(&small, "5 points"), left_of(&big, "123.46 M"));
        assert_eq!(left_of(&small, "Loading"), left_of(&big, "Loading"));
    }
}
