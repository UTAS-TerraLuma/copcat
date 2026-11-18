//! Opening a cloud: the toolbar's Open menu, the idle screen's buttons, the
//! file dialog and the Open URL dialog. Each asks for a [`Command`] rather
//! than opening anything itself.

use eframe::egui;

use super::commands::{Command, ConnectorCommand, Keymap};
use super::connectors::{DataSource, SourceKind};

/// What the file dialog offers. COPC files end `.copc.laz`, which the
/// dialog can't match as one extension, so it shows every `.laz`.
const FILE_FILTER: (&str, &[&str]) = ("COPC point cloud (*.copc.laz)", &["laz"]);

/// A button for `command`, titled as the command is, with its shortcut.
pub(crate) fn command_button(
    ui: &mut egui::Ui,
    keymap: &Keymap,
    command: &Command,
) -> egui::Response {
    let mut button = egui::Button::new(command.title());
    if let Some(shortcut) = keymap.shortcut(command.id()) {
        button = button.shortcut_text(ui.ctx().format_shortcut(&shortcut));
    }
    ui.add(button)
}

/// The toolbar's Open menu: open a file or URL, or browse a data source in
/// the palette.
pub(crate) fn open_menu(
    ui: &mut egui::Ui,
    keymap: &Keymap,
    sources: &[DataSource],
    commands: &mut Vec<Command>,
) {
    ui.menu_button("Open…", |ui| {
        for command in [Command::OpenFileDialog, Command::OpenUrlDialog] {
            if command_button(ui, keymap, &command).clicked() {
                commands.push(command);
                ui.close();
            }
        }
        ui.separator();
        ui.label(egui::RichText::new("Data sources").weak());
        for source in sources {
            let kind = match source.kind {
                SourceKind::Stac(_) => "STAC",
                SourceKind::Folder(_) => "Folder",
            };
            let button = egui::Button::new(&source.name).shortcut_text(kind);
            if ui.add(button).clicked() {
                commands.push(Command::Connector(ConnectorCommand::Browse(
                    source.name.clone(),
                )));
                ui.close();
            }
        }
        for command in [ConnectorCommand::AddStac, ConnectorCommand::AddFolder] {
            if command_button(ui, keymap, &Command::Connector(command.clone())).clicked() {
                commands.push(Command::Connector(command));
                ui.close();
            }
        }
    })
    .response
    .on_hover_text("Open a COPC file, stream one from a URL, or browse a data source");
}

/// What the main area shows before a cloud is open: how to open one.
pub(crate) fn idle_ui(ui: &mut egui::Ui, keymap: &Keymap, commands: &mut Vec<Command>) {
    ui.heading("No point cloud open");
    ui.label(
        "Open a COPC file (.copc.laz) from disk, or stream one from a URL. You can also \
         give a path or URL on the command line.",
    );
    ui.horizontal(|ui| {
        for command in [Command::OpenFileDialog, Command::OpenUrlDialog] {
            if command_button(ui, keymap, &command).clicked() {
                commands.push(command);
            }
        }
    });
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

/// The Open URL dialog, while `text` is `Some`. Returns the command to
/// open what was typed, once the user asks to, and closes the dialog then
/// or when it's cancelled.
pub(crate) fn url_dialog(ctx: &egui::Context, text: &mut Option<String>) -> Option<Command> {
    let typed = text.as_mut()?;
    let mut open = None;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("open_url_dialog")).show(ctx, |ui| {
        ui.set_width(480.0_f32.min(ctx.content_rect().width() - 64.0));
        ui.heading("Open URL");
        let label = ui.label("COPC URL or file path");
        let field = ui
            .add(
                egui::TextEdit::singleline(typed)
                    .hint_text("https://… or C:\\path\\to\\file.copc.laz")
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(label.id);
        // Ready to type into as soon as it opens.
        if ui.memory(|memory| memory.focused().is_none()) {
            field.request_focus();
        }
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        let source = clean_source(typed);
        ui.horizontal(|ui| {
            let open_clicked = ui
                .add_enabled(source.is_some(), egui::Button::new("Open"))
                .clicked();
            if (open_clicked || entered)
                && let Some(source) = source
            {
                open = Some(Command::OpenSource(source));
            }
            cancel = ui.button("Cancel").clicked();
        });
    });
    if open.is_some() || cancel || modal.should_close() {
        *text = None;
    }
    open
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
