//! The status bar along the foot of the window: whether data is still
//! streaming, nodes that failed (with a retry), and how many points are
//! drawn. The full numbers are in the Diagnostics section.

use eframe::egui;

use crate::app::LoadState;
use crate::app::commands::Command;
use crate::format::format_count;
use crate::renderer::PointRenderer;
use crate::theme::Theme;

/// One line of small, faint text. Empty when there's nothing to say, but
/// always the same height, so the panels above don't move.
pub(crate) fn status_bar(
    ui: &mut egui::Ui,
    state: &LoadState,
    renderer: &PointRenderer,
    theme: &Theme,
    commands: &mut Vec<Command>,
) {
    let text = |text: String| {
        egui::RichText::new(text)
            .small()
            .color(theme.colors.text_secondary)
    };
    let spinner = || egui::Spinner::new().size(theme.fonts.small_size);
    ui.horizontal_centered(|ui| match state {
        LoadState::Idle => {}
        LoadState::Opening { .. } => {
            ui.add(spinner());
            ui.label(text("Opening…".to_owned()));
        }
        LoadState::Open(session) => {
            if let Some(waiting) = loading_text(session.requested.len()) {
                ui.add(spinner());
                ui.label(text(waiting))
                    .on_hover_text("Nodes waiting to load. See Diagnostics for more.");
            }
            if let Some(error) = &session.last_error {
                let failed = egui::RichText::new(failed_text(session.failed.len()))
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let drawn = session.drawn_points(renderer);
                ui.label(text(format!("{} points drawn", format_count(drawn))))
                    .on_hover_text("Points drawn in the 3D view");
            });
        }
        LoadState::Failed { .. } => {
            let failed = egui::RichText::new("Failed to open")
                .small()
                .color(theme.colors.error);
            ui.label(failed);
        }
    });
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
}
