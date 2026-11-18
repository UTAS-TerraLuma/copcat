//! A small indicator of loading activity and failures, for the toolbar, so
//! it's clear when data is still streaming. The full numbers are in the
//! Diagnostics section.

use eframe::egui;

use crate::app::LoadState;

/// The indicator, for a right-to-left layout: the first item added is the
/// rightmost.
pub(crate) fn status_indicator(ui: &mut egui::Ui, state: &LoadState) {
    match state {
        LoadState::Idle => {}
        LoadState::Opening { .. } => {
            ui.weak("Opening…");
            ui.spinner();
        }
        LoadState::Open(session) => {
            if let Some(error) = session.failed.values().next() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    failed_text(session.failed.len()),
                )
                .on_hover_text(error);
            }
            if let Some(text) = loading_text(session.requested.len()) {
                ui.weak(text)
                    .on_hover_text("Nodes waiting to load. See Diagnostics for more.");
                ui.spinner();
            }
        }
        LoadState::Failed { .. } => {
            ui.colored_label(ui.visuals().error_fg_color, "Failed to open");
        }
    }
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
