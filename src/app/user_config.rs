//! The settings files ([`crate::config`]) applied to the viewer: display,
//! performance and export defaults into [`Settings`], keybindings into the
//! [`Keymap`], the theme by name. Also the settings commands, and the bar
//! that shows problems with the files.
//!
//! The files are the source of truth: changes made in the UI last for the
//! session unless saved with [`SettingsCommand::SaveDisplayDefaults`].

use std::collections::BTreeMap;
use std::path::Path;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};

use crate::camera::{OrbitCamera, Projection};
use crate::config::{
    self, Binding, ColorModeName, Config, Connectors, DisplayConfig, EdlConfig, Locations,
    PerformanceConfig, PointSizeModeName, Warning,
};
use crate::theme::Theme;

use super::ViewerApp;
use super::commands::{Keymap, SettingsCommand};
use super::settings::{ColorMode, ExportSettings, PointSizeMode, Settings, ViewSettings};

/// The settings files and what came of reading them.
#[derive(Debug, Default)]
pub(crate) struct UserConfig {
    /// Where the files are. `None` reads and writes none, for tests that
    /// mustn't see the developer's own settings.
    pub(crate) locations: Option<Locations>,
    /// Places to open clouds from: the data sources in quick open.
    pub(crate) connectors: Connectors,
    /// Shown at the foot of the window until dismissed.
    pub(crate) messages: Vec<Message>,
}

/// A line in the settings bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) text: String,
    pub(crate) is_warning: bool,
}

impl UserConfig {
    pub(crate) fn new(locations: Option<Locations>) -> Self {
        Self {
            locations,
            ..Default::default()
        }
    }

    pub(crate) fn warn(&mut self, text: impl Into<String>) {
        let text = text.into();
        eprintln!("copcat settings: {text}");
        self.messages.push(Message {
            text,
            is_warning: true,
        });
    }

    pub(crate) fn inform(&mut self, text: impl Into<String>) {
        self.messages.push(Message {
            text: text.into(),
            is_warning: false,
        });
    }
}

impl From<ColorModeName> for ColorMode {
    fn from(name: ColorModeName) -> Self {
        match name {
            ColorModeName::Height => Self::Height,
            ColorModeName::Rgb => Self::Rgb,
            ColorModeName::Classification => Self::Classification,
        }
    }
}

impl From<ColorMode> for ColorModeName {
    fn from(mode: ColorMode) -> Self {
        match mode {
            ColorMode::Height => Self::Height,
            ColorMode::Rgb => Self::Rgb,
            ColorMode::Classification => Self::Classification,
        }
    }
}

impl From<PointSizeModeName> for PointSizeMode {
    fn from(name: PointSizeModeName) -> Self {
        match name {
            PointSizeModeName::Adaptive => Self::Adaptive,
            PointSizeModeName::Fixed => Self::Fixed,
        }
    }
}

impl From<PointSizeMode> for PointSizeModeName {
    fn from(mode: PointSizeMode) -> Self {
        match mode {
            PointSizeMode::Adaptive => Self::Adaptive,
            PointSizeMode::Fixed => Self::Fixed,
        }
    }
}

/// Sets what `config` sets in `settings` and `camera`, leaving the rest.
pub(crate) fn apply_settings(config: &Config, settings: &mut Settings, camera: &mut OrbitCamera) {
    let display = &config.display;
    let view = &mut settings.view;
    if let Some(mode) = display.color_mode {
        view.color_mode = mode.into();
    }
    // Only `pointSizeMode` picks the mode; `pointSize` alone doesn't.
    if let Some(mode) = display.point_size_mode {
        view.point_size_mode = mode.into();
    }
    if let Some(size) = display.point_size {
        view.point_size = size;
    }
    let adaptive = &mut view.adaptive_size;
    if let Some(fill) = display.fill {
        adaptive.fill = fill;
    }
    if let Some(min) = display.min_point_size {
        adaptive.min_px = min;
    }
    if let Some(max) = display.max_point_size {
        adaptive.max_px = max;
    }
    // As the sliders do: the largest is never below the smallest.
    adaptive.max_px = adaptive.max_px.max(adaptive.min_px);
    if let Some(enabled) = display.edl.enabled {
        view.edl_enabled = enabled;
    }
    if let Some(strength) = display.edl.strength {
        view.edl.strength = strength;
    }
    if let Some(radius) = display.edl.radius {
        view.edl.radius_px = radius;
    }
    if let Some(projection) = display.projection {
        camera.set_projection(projection);
    }
    if let Some(budget) = config.performance.point_budget {
        view.lod.point_budget = budget;
    }
    if let Some(spacing) = config.performance.spacing_px {
        view.lod.target_spacing_px = spacing;
    }
    let export = &mut settings.export;
    if let Some(match_view) = config.export.match_view {
        export.match_view = match_view;
    }
    if let Some(width) = config.export.width {
        export.size[0] = width;
    }
    if let Some(height) = config.export.height {
        export.size[1] = height;
    }
}

/// The display and performance settings as they are now, as a settings
/// file would say them.
pub(crate) fn current_defaults(view: &ViewSettings, projection: Projection) -> Config {
    Config {
        display: DisplayConfig {
            color_mode: Some(view.color_mode.into()),
            point_size_mode: Some(view.point_size_mode.into()),
            point_size: Some(view.point_size),
            fill: Some(view.adaptive_size.fill),
            min_point_size: Some(view.adaptive_size.min_px),
            max_point_size: Some(view.adaptive_size.max_px),
            edl: EdlConfig {
                enabled: Some(view.edl_enabled),
                strength: Some(view.edl.strength),
                radius: Some(view.edl.radius_px),
            },
            projection: Some(projection),
        },
        performance: PerformanceConfig {
            point_budget: Some(view.lod.point_budget),
            spacing_px: Some(view.lod.target_spacing_px),
        },
        ..Default::default()
    }
}

/// The theme called `name`, or the default theme with a warning if there's
/// no such theme.
pub(crate) fn theme_named(name: Option<&str>, warnings: &mut Vec<Warning>) -> Theme {
    let Some(name) = name else {
        return Theme::default();
    };
    Theme::by_name(name).unwrap_or_else(|| {
        let known: Vec<_> = crate::theme::THEMES
            .iter()
            .map(|(name, _)| format!("{name:?}"))
            .collect();
        warnings.push(Warning::new(
            None,
            format!(
                "there's no theme called {name:?}, so the default is used; themes: {}",
                known.join(", ")
            ),
        ));
        Theme::default()
    })
}

/// The default keymap with `bindings` applied, warning about unknown
/// commands and shortcuts that can't be read.
pub(crate) fn keymap_with(
    bindings: &BTreeMap<String, Binding>,
    warnings: &mut Vec<Warning>,
) -> Keymap {
    let mut keymap = Keymap::default();
    for (id, binding) in bindings {
        let file = binding.file.as_deref();
        let shortcut = match binding.shortcut.as_deref().map(parse_shortcut) {
            None => None,
            Some(Ok(shortcut)) => Some(shortcut),
            Some(Err(problem)) => {
                warnings.push(Warning::new(file, format!("keybindings.{id}: {problem}")));
                continue;
            }
        };
        if !keymap.bind(id, shortcut) {
            warnings.push(Warning::new(
                file,
                format!("keybindings: there's no command with the id {id:?}"),
            ));
        }
    }
    keymap
}

/// A shortcut as a settings file writes it: modifiers and a key joined by
/// `+`, such as `Ctrl+Shift+P`, `7` or `Ctrl++`. Case doesn't matter.
/// `Ctrl` (or `Cmd`) is Ctrl, or Cmd on a Mac; `Alt` is also `Option`.
pub(crate) fn parse_shortcut(text: &str) -> Result<KeyboardShortcut, String> {
    let text = text.trim();
    let (modifier_text, key_name) = if text == "+" {
        ("", "+")
    } else if let Some(modifier_text) = text.strip_suffix("++") {
        (modifier_text, "+")
    } else {
        text.rsplit_once('+').unwrap_or(("", text))
    };
    let mut modifiers = Modifiers::NONE;
    if !modifier_text.is_empty() {
        for part in modifier_text.split('+') {
            let modifier = match part.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" | "mod" => Modifiers::COMMAND,
                "shift" => Modifiers::SHIFT,
                "alt" | "option" | "opt" => Modifiers::ALT,
                other => {
                    return Err(format!(
                        "{other:?} in {text:?} isn't a modifier; use Ctrl, Shift or Alt"
                    ));
                }
            };
            modifiers = modifiers.plus(modifier);
        }
    }
    let key_name = key_name.trim();
    let key = Key::from_name(key_name)
        .or_else(|| {
            Key::ALL
                .iter()
                .copied()
                .find(|key| key.name().eq_ignore_ascii_case(key_name))
        })
        .ok_or_else(|| {
            if key_name.is_empty() {
                format!("{text:?} has no key")
            } else {
                format!("{key_name:?} in {text:?} isn't a key")
            }
        })?;
    Ok(KeyboardShortcut::new(modifiers, key))
}

impl ViewerApp {
    /// Reads the settings files and applies them over the defaults:
    /// display, performance and export settings, the theme and the
    /// keybindings. Problems are reported in the settings bar.
    pub(super) fn load_user_config(&mut self, ctx: &egui::Context) {
        let loaded = self
            .user_config
            .locations
            .as_ref()
            .map(Locations::load)
            .unwrap_or_default();
        let config = loaded.config;
        let mut warnings = loaded.warnings;

        self.settings.view = ViewSettings::default();
        self.settings.export = ExportSettings::default();
        apply_settings(&config, &mut self.settings, &mut self.camera);
        self.theme = theme_named(config.theme.as_deref(), &mut warnings);
        self.theme.apply(ctx);
        self.keymap = keymap_with(&config.keybindings, &mut warnings);
        self.user_config.connectors = config.connectors;
        self.refresh_connectors();

        for warning in warnings {
            self.user_config.warn(warning.to_string());
        }
        ctx.request_repaint();
    }

    pub(super) fn run_settings_command(&mut self, ctx: &egui::Context, command: SettingsCommand) {
        let Some(locations) = self.user_config.locations.clone() else {
            self.user_config
                .warn("settings files aren't used in this session");
            return;
        };
        match command {
            SettingsCommand::OpenUser => match locations.user_file() {
                Some(file) => self.open_settings_file(&file, config::USER_TEMPLATE),
                None => self.user_config.warn(format!(
                    "couldn't find the home directory; set {} to where settings should go",
                    config::CONFIG_DIR_ENV
                )),
            },
            SettingsCommand::OpenWorkspace => {
                let file = locations
                    .workspace
                    .as_ref()
                    .map(|dir| dir.join(config::DIR_NAME).join(config::FILE_NAME));
                match file {
                    Some(file) => self.open_settings_file(&file, config::WORKSPACE_TEMPLATE),
                    None => self.user_config.warn("there's no workspace folder"),
                }
            }
            SettingsCommand::Reload => {
                self.user_config.messages.clear();
                self.load_user_config(ctx);
            }
            SettingsCommand::SaveDisplayDefaults => {
                let Some(file) = locations.user_file() else {
                    self.user_config.warn("couldn't find the home directory");
                    return;
                };
                let defaults = current_defaults(&self.settings.view, self.camera.projection());
                match config::save_over(&file, &defaults.to_json()) {
                    Ok(dropped_comments) => {
                        let note = if dropped_comments {
                            " (its comments were removed)"
                        } else {
                            ""
                        };
                        self.user_config.inform(format!(
                            "Saved the display settings to {}{note}",
                            file.display()
                        ));
                    }
                    Err(problem) => self
                        .user_config
                        .warn(format!("{}: {problem}", file.display())),
                }
            }
        }
    }

    /// Creates `file` from `template` if it's missing, then opens it in the
    /// system's editor for it.
    fn open_settings_file(&mut self, file: &Path, template: &str) {
        let opened =
            config::create_if_missing(file, template).and_then(|_| config::open_in_editor(file));
        match opened {
            Ok(()) => self.user_config.inform(format!(
                "Opened {}. Run Reload settings after saving it.",
                file.display()
            )),
            Err(error) => self
                .user_config
                .warn(format!("couldn't open {}: {error}", file.display())),
        }
    }

    /// The bar at the foot of the window listing problems with the settings
    /// files and what the settings commands did, until dismissed.
    pub(super) fn settings_bar(&mut self, ui: &mut egui::Ui) {
        if self.user_config.messages.is_empty() {
            return;
        }
        let theme = &self.theme;
        let messages = &mut self.user_config.messages;
        egui::Panel::bottom("settings_messages")
            .frame(theme.header_frame())
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Dismiss").clicked() {
                        messages.clear();
                    }
                    ui.vertical(|ui| {
                        for message in messages.iter() {
                            let color = if message.is_warning {
                                theme.colors.warn
                            } else {
                                theme.colors.text_secondary
                            };
                            let prefix = if message.is_warning { "Settings: " } else { "" };
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!("{prefix}{}", message.text))
                                        .color(color),
                                )
                                .wrap(),
                            );
                        }
                    });
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn shortcut(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
        KeyboardShortcut::new(modifiers, key)
    }

    #[test]
    fn shortcuts_should_parse() {
        let cases = [
            ("7", shortcut(Modifiers::NONE, Key::Num7)),
            ("Ctrl+7", shortcut(Modifiers::COMMAND, Key::Num7)),
            (
                "Ctrl+Shift+P",
                shortcut(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::P),
            ),
            (
                "shift + alt + f5",
                shortcut(Modifiers::SHIFT.plus(Modifiers::ALT), Key::F5),
            ),
            ("Cmd+O", shortcut(Modifiers::COMMAND, Key::O)),
            ("escape", shortcut(Modifiers::NONE, Key::Escape)),
            ("PageDown", shortcut(Modifiers::NONE, Key::PageDown)),
            ("pagedown", shortcut(Modifiers::NONE, Key::PageDown)),
            ("Ctrl++", shortcut(Modifiers::COMMAND, Key::Plus)),
            ("+", shortcut(Modifiers::NONE, Key::Plus)),
            ("Ctrl+-", shortcut(Modifiers::COMMAND, Key::Minus)),
            ("Space", shortcut(Modifiers::NONE, Key::Space)),
        ];
        for (text, expected) in cases {
            assert_eq!(parse_shortcut(text), Ok(expected), "{text}");
        }
    }

    #[test]
    fn bad_shortcuts_should_say_what_is_wrong() {
        for (text, problem) in [
            ("Hyper+P", "\"hyper\" in \"Hyper+P\" isn't a modifier"),
            ("Ctrl+Banana", "\"Banana\" in \"Ctrl+Banana\" isn't a key"),
            ("Ctrl+", "\"Ctrl+\" has no key"),
            ("Ctrl+P+Q", "\"p\" in \"Ctrl+P+Q\" isn't a modifier"),
        ] {
            let error = parse_shortcut(text).unwrap_err();
            assert!(error.starts_with(problem), "{text}: {error}");
        }
    }

    #[test]
    fn every_default_shortcut_should_parse_from_its_name() {
        // A file can always say what the UI shows.
        let keymap = Keymap::default();
        for command in super::super::commands::Command::all() {
            let Some(shortcut) = keymap.shortcut(command.id()) else {
                continue;
            };
            let text = shortcut.format(&egui::ModifierNames::NAMES, false);
            assert_eq!(parse_shortcut(&text), Ok(shortcut), "{text}");
        }
    }

    fn binding(shortcut: Option<&str>) -> Binding {
        Binding {
            shortcut: shortcut.map(str::to_owned),
            file: Some(PathBuf::from("settings.json")),
        }
    }

    #[test]
    fn keybindings_should_rebind_and_unbind() {
        let bindings = BTreeMap::from([
            ("view.reset".to_owned(), binding(Some("Ctrl+R"))),
            ("view.top".to_owned(), binding(None)),
        ]);
        let mut warnings = Vec::new();
        let keymap = keymap_with(&bindings, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            keymap.shortcut("view.reset"),
            Some(shortcut(Modifiers::COMMAND, Key::R))
        );
        assert_eq!(keymap.shortcut("view.top"), None);
        // The rest keep their defaults.
        assert_eq!(
            keymap.shortcut("view.front"),
            Some(shortcut(Modifiers::NONE, Key::Num1))
        );
    }

    #[test]
    fn bad_keybindings_should_warn_and_keep_the_default() {
        let bindings = BTreeMap::from([
            ("view.nonsense".to_owned(), binding(Some("X"))),
            ("view.top".to_owned(), binding(Some("Ctrl+Banana"))),
        ]);
        let mut warnings = Vec::new();
        let keymap = keymap_with(&bindings, &mut warnings);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.file.is_some()));
        assert_eq!(
            keymap.shortcut("view.top"),
            Some(shortcut(Modifiers::NONE, Key::Num7))
        );
    }

    #[test]
    fn an_unknown_theme_should_warn_and_fall_back() {
        let mut warnings = Vec::new();
        assert_eq!(theme_named(Some("neon"), &mut warnings).name, "Graphite");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("\"graphite\""));
        warnings.clear();
        theme_named(Some("GRAPHITE"), &mut warnings);
        theme_named(None, &mut warnings);
        assert!(warnings.is_empty());
    }

    #[test]
    fn apply_should_set_only_what_the_config_sets() {
        let (config, warnings) = Config::parse(
            r#"{"display": {"colorMode": "classification", "edl": {"strength": 3}, "projection": "orthographic"},
                "performance": {"spacingPx": 4}, "export": {"matchView": false, "width": 640}}"#,
            None,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut settings = Settings::default();
        let mut camera = OrbitCamera::default();
        apply_settings(&config, &mut settings, &mut camera);
        let defaults = Settings::default();
        assert_eq!(settings.view.color_mode, ColorMode::Classification);
        assert_eq!(settings.view.edl.strength, 3.0);
        assert_eq!(settings.view.edl.radius_px, defaults.view.edl.radius_px);
        assert_eq!(settings.view.point_size, defaults.view.point_size);
        assert_eq!(settings.view.point_size_mode, defaults.view.point_size_mode);
        assert_eq!(settings.view.adaptive_size, defaults.view.adaptive_size);
        assert_eq!(settings.view.lod.target_spacing_px, 4.0);
        assert_eq!(
            settings.view.lod.point_budget,
            defaults.view.lod.point_budget
        );
        assert!(!settings.export.match_view);
        assert_eq!(settings.export.size, [640, defaults.export.size[1]]);
        assert_eq!(camera.projection(), Projection::Orthographic);
    }

    #[test]
    #[allow(
        clippy::needless_update,
        reason = "keeps compiling as fields are added to ViewSettings"
    )]
    fn current_defaults_should_round_trip_through_apply() {
        let defaults = ViewSettings::default();
        let view = ViewSettings {
            color_mode: ColorMode::Height,
            point_size_mode: PointSizeMode::Fixed,
            adaptive_size: crate::app::settings::AdaptiveSizeSettings {
                fill: 2.25,
                min_px: 1.5,
                max_px: 32.0,
            },
            point_size: 4.5,
            edl_enabled: false,
            edl: crate::renderer::EdlSettings {
                radius_px: 2.25,
                ..defaults.edl
            },
            lod: crate::lod::LodSettings {
                point_budget: 1_234_567,
                ..defaults.lod
            },
            ..defaults
        };
        let config = current_defaults(&view, Projection::Orthographic);
        let (read, warnings) = Config::parse(&config.to_json().to_string(), None);
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut settings = Settings::default();
        let mut camera = OrbitCamera::default();
        apply_settings(&read, &mut settings, &mut camera);
        assert_eq!(settings.view, view);
        assert_eq!(camera.projection(), Projection::Orthographic);
    }

    #[test]
    fn point_size_alone_should_not_switch_to_fixed() {
        let (config, warnings) = Config::parse(r#"{"display": {"pointSize": 6}}"#, None);
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut settings = Settings::default();
        apply_settings(&config, &mut settings, &mut OrbitCamera::default());
        assert_eq!(settings.view.point_size_mode, PointSizeMode::Adaptive);
        assert_eq!(settings.view.point_size, 6.0, "kept for fixed mode");
    }

    #[test]
    fn point_size_mode_and_adaptive_sizes_should_apply() {
        let (config, warnings) = Config::parse(
            r#"{"display": {"pointSizeMode": "fixed", "pointSize": 3, "fill": 2.5, "minPointSize": 2, "maxPointSize": 40}}"#,
            None,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut settings = Settings::default();
        apply_settings(&config, &mut settings, &mut OrbitCamera::default());
        assert_eq!(settings.view.point_size_mode, PointSizeMode::Fixed);
        assert_eq!(settings.view.point_size, 3.0);
        assert_eq!(settings.view.adaptive_size.fill, 2.5);
        assert_eq!(settings.view.adaptive_size.min_px, 2.0);
        assert_eq!(settings.view.adaptive_size.max_px, 40.0);
    }

    #[test]
    fn a_max_point_size_below_the_min_should_be_raised_to_it() {
        let (config, warnings) = Config::parse(
            r#"{"display": {"minPointSize": 6, "maxPointSize": 3}}"#,
            None,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let mut settings = Settings::default();
        apply_settings(&config, &mut settings, &mut OrbitCamera::default());
        assert_eq!(settings.view.adaptive_size.min_px, 6.0);
        assert_eq!(settings.view.adaptive_size.max_px, 6.0);
    }

    #[test]
    fn the_readme_example_should_load_and_show_the_defaults() {
        let readme = include_str!("../../README.md");
        let settings = &readme[readme.find("## Settings").expect("a Settings section")..];
        // The line after the opening fence, whatever the line endings.
        let fence = settings.find("```jsonc").expect("a jsonc example");
        let start = fence + settings[fence..].find('\n').unwrap() + 1;
        let example = &settings[start..start + settings[start..].find("```").unwrap()];
        let (config, warnings) = Config::parse(example, None);
        assert!(warnings.is_empty(), "{warnings:?}");

        let mut keymap_warnings = Vec::new();
        keymap_with(&config.keybindings, &mut keymap_warnings);
        assert!(keymap_warnings.is_empty(), "{keymap_warnings:?}");
        assert!(Theme::by_name(config.theme.as_deref().unwrap()).is_some());

        // The values it gives are the defaults, as it says.
        let defaults = current_defaults(&ViewSettings::default(), Projection::Perspective);
        assert_eq!(config.display, defaults.display);
        assert_eq!(config.performance, defaults.performance);
        let mut settings = Settings::default();
        apply_settings(&config, &mut settings, &mut OrbitCamera::default());
        assert_eq!(settings.export, ExportSettings::default());
    }

    #[test]
    fn the_readme_should_list_every_command() {
        let readme = include_str!("../../README.md");
        for command in super::super::commands::Command::all() {
            let id = format!("`{}`", command.id());
            assert!(readme.contains(&id), "the README doesn't list {id}");
        }
    }

    #[test]
    fn the_default_settings_should_be_within_the_files_ranges() {
        // Otherwise saving the defaults would write values that load clamped.
        let view = ViewSettings::default();
        let config = current_defaults(&view, Projection::Perspective);
        let (read, warnings) = Config::parse(&config.to_json().to_string(), None);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(read, config);
    }
}
