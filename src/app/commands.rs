//! Everything the user can ask the viewer to do from a button, a menu or a
//! shortcut, as one [`Command`] type with a single dispatcher
//! ([`ViewerApp::run`](super::ViewerApp::run)).
//!
//! Each command has metadata ([`CommandInfo`]): a stable id, a title for
//! people and an optional default shortcut. [`Command::all`] lists every
//! command that needs no argument, so a command palette can offer them, and
//! [`Keymap`] maps shortcuts to commands by id, so a settings file can
//! rebind them.

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};

use crate::camera::{Projection, ViewPreset};

use super::Mode;
use super::settings::ColorMode;

/// Something the user asked for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    /// Open a COPC file path or URL.
    OpenSource(String),
    /// Choose a file to open.
    OpenFileDialog,
    /// Type a URL or path to open.
    OpenUrlDialog,
    SetMode(Mode),
    /// Look along one of the 3D view camera's axes.
    SetView(ViewPreset),
    SetProjection(Projection),
    ToggleProjection,
    /// Point the 3D view back at the whole cloud, or at the subcloud.
    ResetView,
    /// Turn eye-dome lighting on or off.
    ToggleEdl,
    SetColorMode(ColorMode),
    /// Save the 3D view as a PNG, once the detail for it has loaded.
    SaveImage,
    /// Copy a `copcat render` command for the 3D view.
    CopyCommandLine,
    /// Show or hide the Diagnostics section.
    ToggleDiagnostics,
    /// Open, reload or save the settings files.
    Settings(SettingsCommand),
}

/// What to do with the settings files (see `crate::config`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsCommand {
    /// Open the user settings file, creating it if it's missing.
    OpenUser,
    /// Open the workspace settings file, creating it if it's missing.
    OpenWorkspace,
    /// Read the settings files again and apply them.
    Reload,
    /// Write the display and performance settings into the user file.
    SaveDisplayDefaults,
}

impl SettingsCommand {
    pub(crate) const ALL: [Self; 4] = [
        Self::OpenUser,
        Self::OpenWorkspace,
        Self::Reload,
        Self::SaveDisplayDefaults,
    ];
}

/// What people and settings files know a command by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CommandInfo {
    /// Stable, for settings files: dotted lowercase, never reused.
    pub(crate) id: &'static str,
    /// For menus and a command palette.
    pub(crate) title: &'static str,
    pub(crate) default_shortcut: Option<KeyboardShortcut>,
}

const fn key(key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(Modifiers::NONE, key))
}

/// Ctrl, or Cmd on a Mac, with `key`.
const fn command_key(key: Key) -> Option<KeyboardShortcut> {
    Some(KeyboardShortcut::new(Modifiers::COMMAND, key))
}

const fn info(
    id: &'static str,
    title: &'static str,
    default_shortcut: Option<KeyboardShortcut>,
) -> CommandInfo {
    CommandInfo {
        id,
        title,
        default_shortcut,
    }
}

impl Command {
    /// Every command that needs no argument, in the order a palette might
    /// list them. Leaves out [`Command::OpenSource`], which needs a source.
    pub(crate) fn all() -> Vec<Self> {
        let mut commands = vec![
            Self::OpenFileDialog,
            Self::OpenUrlDialog,
            Self::SetMode(Mode::Explore),
            Self::SetMode(Mode::Subcloud),
        ];
        commands.extend(ViewPreset::ALL.map(Self::SetView));
        commands.extend([
            Self::SetProjection(Projection::Perspective),
            Self::SetProjection(Projection::Orthographic),
            Self::ToggleProjection,
            Self::ResetView,
            Self::ToggleEdl,
        ]);
        commands.extend(ColorMode::ALL.map(Self::SetColorMode));
        commands.extend([
            Self::SaveImage,
            Self::CopyCommandLine,
            Self::ToggleDiagnostics,
        ]);
        commands.extend(SettingsCommand::ALL.map(Self::Settings));
        commands
    }

    /// The argument-free command with this id, as a settings file or
    /// palette names it.
    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::all().into_iter().find(|command| command.id() == id)
    }

    pub(crate) fn id(&self) -> &'static str {
        self.info().id
    }

    pub(crate) fn title(&self) -> &'static str {
        self.info().title
    }

    pub(crate) fn info(&self) -> CommandInfo {
        match self {
            Self::OpenSource(_) => info("file.open_source", "Open source", None),
            Self::OpenFileDialog => info("file.open", "Open file…", command_key(Key::O)),
            Self::OpenUrlDialog => info("file.open_url", "Open URL…", command_key(Key::L)),
            Self::SetMode(Mode::Explore) => info("mode.explore", "Explore mode", None),
            Self::SetMode(Mode::Subcloud) => info("mode.subcloud", "Subcloud mode", None),
            // Numpad-style view keys, as in Blender, with Ctrl for the
            // opposite view.
            Self::SetView(preset) => match preset {
                ViewPreset::Top => info("view.top", "View from top", key(Key::Num7)),
                ViewPreset::Bottom => {
                    info("view.bottom", "View from bottom", command_key(Key::Num7))
                }
                ViewPreset::Front => info("view.front", "View from front", key(Key::Num1)),
                ViewPreset::Back => info("view.back", "View from back", command_key(Key::Num1)),
                ViewPreset::Right => info("view.right", "View from right", key(Key::Num3)),
                ViewPreset::Left => info("view.left", "View from left", command_key(Key::Num3)),
            },
            Self::SetProjection(Projection::Perspective) => {
                info("view.perspective", "Perspective projection", None)
            }
            Self::SetProjection(Projection::Orthographic) => {
                info("view.orthographic", "Orthographic projection", None)
            }
            Self::ToggleProjection => info(
                "view.toggle_projection",
                "Toggle perspective / orthographic",
                key(Key::Num5),
            ),
            Self::ResetView => info("view.reset", "Reset view", None),
            Self::ToggleEdl => info("display.toggle_edl", "Toggle eye-dome lighting", None),
            Self::SetColorMode(ColorMode::Height) => {
                info("display.color_height", "Colour by height", None)
            }
            Self::SetColorMode(ColorMode::Rgb) => info("display.color_rgb", "Colour by RGB", None),
            Self::SetColorMode(ColorMode::Classification) => {
                info("display.color_class", "Colour by classification", None)
            }
            Self::SaveImage => info("export.save_image", "Save image…", command_key(Key::S)),
            Self::CopyCommandLine => info("export.copy_command", "Copy render command", None),
            Self::ToggleDiagnostics => info(
                "view.toggle_diagnostics",
                "Toggle diagnostics",
                Some(KeyboardShortcut::new(
                    Modifiers::COMMAND.plus(Modifiers::SHIFT),
                    Key::D,
                )),
            ),
            Self::Settings(command) => match command {
                SettingsCommand::OpenUser => info("settings.open_user", "Open user settings", None),
                SettingsCommand::OpenWorkspace => {
                    info("settings.open_workspace", "Open workspace settings", None)
                }
                SettingsCommand::Reload => info("settings.reload", "Reload settings", None),
                SettingsCommand::SaveDisplayDefaults => info(
                    "settings.save_display_defaults",
                    "Save current display settings as defaults",
                    None,
                ),
            },
        }
    }
}

/// Which shortcut runs which command. Starts from the default shortcuts;
/// [`Keymap::bind`] changes them by command id.
#[derive(Clone, Debug)]
pub(crate) struct Keymap {
    /// Kept with shortcuts using more modifiers first: egui ignores extra
    /// Shift and Alt when matching, so Ctrl+Shift+D must be tried before
    /// Ctrl+D.
    bindings: Vec<(KeyboardShortcut, Command)>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut keymap = Self {
            bindings: Vec::new(),
        };
        for command in Command::all() {
            if let Some(shortcut) = command.info().default_shortcut {
                keymap.insert(shortcut, command);
            }
        }
        keymap
    }
}

impl Keymap {
    /// Runs the command with `id` on `shortcut`, or on nothing if `None`,
    /// in place of its old shortcut. A command already on `shortcut` loses
    /// it. Returns false if no argument-free command has that id.
    pub(crate) fn bind(&mut self, id: &str, shortcut: Option<KeyboardShortcut>) -> bool {
        let Some(command) = Command::from_id(id) else {
            return false;
        };
        self.bindings
            .retain(|(bound, bound_command)| bound_command.id() != id && Some(*bound) != shortcut);
        if let Some(shortcut) = shortcut {
            self.insert(shortcut, command);
        }
        true
    }

    /// The shortcut that runs the command with `id`, if any.
    pub(crate) fn shortcut(&self, id: &str) -> Option<KeyboardShortcut> {
        self.bindings
            .iter()
            .find(|(_, command)| command.id() == id)
            .map(|(shortcut, _)| *shortcut)
    }

    /// The commands whose shortcuts were pressed this frame, consuming
    /// those key presses.
    pub(crate) fn consume(&self, input: &mut egui::InputState) -> Vec<Command> {
        self.bindings
            .iter()
            .filter(|(shortcut, _)| input.consume_shortcut(shortcut))
            .map(|(_, command)| command.clone())
            .collect()
    }

    fn insert(&mut self, shortcut: KeyboardShortcut, command: Command) {
        let at = self
            .bindings
            .partition_point(|(bound, _)| modifier_count(bound) >= modifier_count(&shortcut));
        self.bindings.insert(at, (shortcut, command));
    }
}

fn modifier_count(shortcut: &KeyboardShortcut) -> usize {
    let held = shortcut.modifiers;
    // `command` is Ctrl, or Cmd on a Mac, so the three count as one.
    [
        held.alt,
        held.shift,
        held.ctrl || held.mac_cmd || held.command,
    ]
    .into_iter()
    .filter(|&held| held)
    .count()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    fn every_command() -> Vec<Command> {
        let mut commands = Command::all();
        commands.push(Command::OpenSource("site.copc.laz".to_owned()));
        commands
    }

    #[test]
    fn command_ids_should_be_unique() {
        let commands = every_command();
        let ids: HashSet<_> = commands.iter().map(Command::id).collect();
        assert_eq!(ids.len(), commands.len());
    }

    #[test]
    fn every_command_should_have_a_title() {
        for command in every_command() {
            assert!(!command.title().trim().is_empty(), "{command:?}");
        }
    }

    #[test]
    fn command_ids_should_be_dotted_lowercase() {
        for command in every_command() {
            let id = command.id();
            let well_formed = id.contains('.')
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '.' || c == '_');
            assert!(well_formed, "{id}");
        }
    }

    #[test]
    fn default_shortcuts_should_be_unique() {
        let shortcuts: Vec<_> = Command::all()
            .iter()
            .filter_map(|command| command.info().default_shortcut)
            .collect();
        let unique: HashSet<_> = shortcuts.iter().collect();
        assert_eq!(unique.len(), shortcuts.len());
    }

    #[test]
    fn from_id_should_find_every_listed_command() {
        for command in Command::all() {
            assert_eq!(Command::from_id(command.id()), Some(command));
        }
    }

    #[test]
    fn from_id_should_not_find_open_source() {
        assert_eq!(Command::from_id("file.open_source"), None);
    }

    #[test]
    fn keymap_should_start_with_the_default_shortcuts() {
        let keymap = Keymap::default();
        assert_eq!(
            keymap.shortcut("view.top"),
            Some(KeyboardShortcut::new(Modifiers::NONE, Key::Num7))
        );
    }

    #[test]
    fn keymap_should_try_shortcuts_with_more_modifiers_first() {
        let counts: Vec<_> = Keymap::default()
            .bindings
            .iter()
            .map(|(shortcut, _)| modifier_count(shortcut))
            .collect();
        assert!(counts.is_sorted_by(|a, b| a >= b), "{counts:?}");
    }

    #[test]
    fn bind_should_move_a_command_to_a_new_shortcut() {
        let mut keymap = Keymap::default();
        let f5 = KeyboardShortcut::new(Modifiers::NONE, Key::F5);
        assert!(keymap.bind("view.reset", Some(f5)));
        assert_eq!(keymap.shortcut("view.reset"), Some(f5));
    }

    #[test]
    fn bind_should_take_the_shortcut_from_another_command() {
        let mut keymap = Keymap::default();
        let num7 = KeyboardShortcut::new(Modifiers::NONE, Key::Num7);
        keymap.bind("view.reset", Some(num7));
        assert_eq!(keymap.shortcut("view.top"), None);
    }

    #[test]
    fn bind_should_reject_an_unknown_id() {
        assert!(!Keymap::default().bind("view.nonsense", None));
    }
}
