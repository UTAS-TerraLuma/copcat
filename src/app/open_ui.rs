//! Opening a cloud: the menu bar and the file dialog. Each asks for a
//! [`Command`] rather than doing anything itself; a URL is asked for in the
//! command palette.

use eframe::egui;

use super::commands::{Command, Keymap, SettingsCommand};

/// What the file dialog offers. COPC files end `.copc.laz`, which the
/// dialog can't match as one extension, so it shows every `.laz`.
const FILE_FILTER: (&str, &[&str]) = ("COPC point cloud (*.copc.laz)", &["laz"]);

/// The menu bar along the top of the window: File, for `.copcat` files
/// still to come and the settings; Open, which opens a file or URL or
/// browses the data sources; and View, which shows the command palette.
pub(crate) fn menu_bar(ui: &mut egui::Ui, keymap: &Keymap, commands: &mut Vec<Command>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            let later = "Coming later: a .copcat file will keep a view, the cloud and its \
                         settings, to open again";
            for title in ["Open .copcat file…", "Save .copcat file…"] {
                ui.add_enabled(false, egui::Button::new(title))
                    .on_disabled_hover_text(later);
            }
            ui.separator();
            let settings = [
                (
                    SettingsCommand::OpenWindow,
                    "The theme, the default rendering settings and the settings files",
                ),
                (
                    SettingsCommand::OpenUser,
                    "Your settings file, for every folder, in the system's editor",
                ),
                (
                    SettingsCommand::OpenWorkspace,
                    "This folder's settings file, over yours, in the system's editor",
                ),
            ];
            for (command, hover) in settings {
                let command = Command::Settings(command);
                if titled_button(ui, keymap, command.title(), &command)
                    .on_hover_text(hover)
                    .clicked()
                {
                    commands.push(command);
                    ui.close();
                }
            }
        });
        ui.menu_button("Open", |ui| {
            let items = [
                (
                    "From file…",
                    Command::OpenFileDialog,
                    "Choose a COPC file on disk",
                ),
                (
                    "From URL…",
                    Command::OpenUrlDialog,
                    "Paste the URL or path of a COPC file",
                ),
                (
                    "From STAC…",
                    Command::BrowseSources,
                    "Browse a data source to a cloud, or add a STAC catalog",
                ),
            ];
            for (title, command, hover) in items {
                if titled_button(ui, keymap, title, &command)
                    .on_hover_text(hover)
                    .clicked()
                {
                    commands.push(command);
                    ui.close();
                }
            }
        });
        ui.menu_button("View", |ui| {
            let command = Command::ShowCommands;
            if titled_button(ui, keymap, command.title(), &command)
                .on_hover_text("Search the commands, and run one")
                .clicked()
            {
                commands.push(command);
                ui.close();
            }
        });
    });
}

/// A button titled `title` for `command`, with the command's shortcut.
fn titled_button(
    ui: &mut egui::Ui,
    keymap: &Keymap,
    title: &str,
    command: &Command,
) -> egui::Response {
    let mut button = egui::Button::new(title);
    if let Some(shortcut) = keymap.shortcut(command.id()) {
        button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut));
    }
    ui.add(button)
}

/// Asks for a COPC file to open, blocking until the dialog closes.
pub(crate) fn pick_file() -> Option<String> {
    let (name, extensions) = FILE_FILTER;
    rfd::FileDialog::new()
        .add_filter(name, extensions)
        .add_filter("All files", &["*"])
        .pick_file()
        .map(|path| path.display().to_string())
}

/// `text` as a source to open: trimmed, and without the quotes a copied
/// path can have. `None` if that leaves nothing.
pub(crate) fn clean_source(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let unquoted = ['"', '\'']
        .into_iter()
        .find_map(|quote| {
            trimmed
                .strip_prefix(quote)
                .and_then(|rest| rest.strip_suffix(quote))
        })
        .unwrap_or(trimmed)
        .trim();
    (!unquoted.is_empty()).then(|| unquoted.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_source_should_trim_whitespace() {
        assert_eq!(
            clean_source("  https://example.com/a.copc.laz \n").as_deref(),
            Some("https://example.com/a.copc.laz")
        );
    }

    #[test]
    fn clean_source_should_strip_quotes_from_a_copied_path() {
        assert_eq!(
            clean_source(r#""C:\data\site 1.copc.laz""#).as_deref(),
            Some(r"C:\data\site 1.copc.laz")
        );
    }

    #[test]
    fn clean_source_should_keep_a_lone_quote() {
        assert_eq!(clean_source("it's.laz").as_deref(), Some("it's.laz"));
    }

    #[test]
    fn clean_source_should_reject_blank_text() {
        assert_eq!(clean_source("   "), None);
        assert_eq!(clean_source("\"\""), None);
    }
}
