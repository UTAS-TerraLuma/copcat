//! User configuration: JSON settings files, as in VS Code or Zed. Knows
//! nothing about egui; the viewer applies a [`Config`] to its own settings.
//!
//! # Files and precedence
//!
//! Settings are layered, each layer overriding the one before it key by key:
//!
//! 1. the built-in defaults;
//! 2. the user file, `~/.copcat/settings.json` (the directory can be moved
//!    with `COPCAT_CONFIG_DIR`);
//! 3. the workspace file, `.copcat/settings.json` in the folder the viewer
//!    was started in (or `COPCAT_WORKSPACE`).
//!
//! Every key is optional, so a file can set just one thing. Objects merge
//! key by key at every depth: a workspace file setting `display.edl.radius`
//! keeps the user file's `display.edl.strength`. `keybindings` merge by
//! command id. Connector lists merge by `name`: a workspace connector with
//! the same name as a user one replaces it, in the user one's place; others
//! are added after the user's.
//!
//! Files are JSON with comments and trailing commas allowed (JSONC). A file
//! that can't be read is skipped; a value that's the wrong type or unknown
//! is skipped; a number out of range is clamped to what the UI allows. Each
//! problem becomes a [`Warning`] naming the file, and loading carries on.

mod jsonc;
mod parse;
mod save;

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use crate::camera::Projection;

pub(crate) use save::{
    ConnectorList, USER_TEMPLATE, WORKSPACE_TEMPLATE, add_connector, create_if_missing,
    open_in_editor, remove_connector, save_over,
};

/// Every settings file's name.
pub const FILE_NAME: &str = "settings.json";
/// The directory a workspace's settings file is in, and the user's under
/// their home directory.
pub const DIR_NAME: &str = ".copcat";
/// Where the user's settings directory is, in place of `~/.copcat`.
pub const CONFIG_DIR_ENV: &str = "COPCAT_CONFIG_DIR";
/// The workspace folder, in place of the current directory.
pub const WORKSPACE_ENV: &str = "COPCAT_WORKSPACE";

/// The values a settings file may give, matching the UI's sliders. Values
/// outside them are clamped.
pub mod ranges {
    use std::ops::RangeInclusive;

    pub const POINT_SIZE: RangeInclusive<f32> = 1.0..=10.0;
    pub const EDL_STRENGTH: RangeInclusive<f32> = 0.1..=5.0;
    pub const EDL_RADIUS: RangeInclusive<f32> = 1.0..=4.0;
    pub const POINT_BUDGET: RangeInclusive<u64> = 500_000..=30_000_000;
    pub const SPACING_PX: RangeInclusive<f32> = 0.5..=20.0;
    /// An image side in pixels. The viewer clamps further to what the GPU
    /// can draw.
    pub const EXPORT_SIDE: RangeInclusive<u32> = 1..=16_384;
}

/// What the settings files say, merged. `None` and empty mean "not set":
/// keep the default.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    /// A theme's name, such as `graphite`.
    pub theme: Option<String>,
    pub display: DisplayConfig,
    pub performance: PerformanceConfig,
    pub export: ExportConfig,
    /// Shortcuts by command id.
    pub keybindings: BTreeMap<String, Binding>,
    pub connectors: Connectors,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisplayConfig {
    pub color_mode: Option<ColorModeName>,
    pub point_size: Option<f32>,
    pub edl: EdlConfig,
    pub projection: Option<Projection>,
}

/// Eye-dome lighting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EdlConfig {
    pub enabled: Option<bool>,
    pub strength: Option<f32>,
    /// In physical pixels.
    pub radius: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerformanceConfig {
    pub point_budget: Option<u64>,
    pub spacing_px: Option<f32>,
}

/// The Save image section.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportConfig {
    pub match_view: Option<bool>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// How points are coloured, as a settings file names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorModeName {
    Height,
    Rgb,
    Classification,
}

impl ColorModeName {
    pub const ALL: [Self; 3] = [Self::Height, Self::Rgb, Self::Classification];

    pub fn name(self) -> &'static str {
        match self {
            Self::Height => "height",
            Self::Rgb => "rgb",
            Self::Classification => "classification",
        }
    }
}

pub(crate) fn projection_name(projection: Projection) -> &'static str {
    match projection {
        Projection::Perspective => "perspective",
        Projection::Orthographic => "orthographic",
    }
}

/// A shortcut for a command, as written, and which file said so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// Such as `Ctrl+Shift+P`. `None` (`null` or `""` in the file) leaves
    /// the command without a shortcut.
    pub shortcut: Option<String>,
    pub file: Option<PathBuf>,
}

/// Places to open clouds from, for a connector browser.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Connectors {
    pub stac: Vec<StacConnector>,
    pub folders: Vec<FolderConnector>,
}

/// A STAC catalog, static or an API, to browse for clouds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StacConnector {
    pub name: String,
    /// An `http` or `https` URL.
    pub url: String,
}

/// A folder of clouds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderConnector {
    pub name: String,
    /// Relative paths in a file are resolved against the folder holding
    /// that file's `.copcat` directory, so this is absolute when it came
    /// from a file.
    pub path: PathBuf,
}

trait Named {
    fn name(&self) -> &str;
}

impl Named for StacConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

impl Named for FolderConnector {
    fn name(&self) -> &str {
        &self.name
    }
}

/// `base` with `over`'s entries in place of `base`'s of the same name, and
/// the rest of `over` after them.
fn merge_by_name<T: Named>(mut base: Vec<T>, over: Vec<T>) -> Vec<T> {
    for entry in over {
        match base.iter_mut().find(|old| old.name() == entry.name()) {
            Some(old) => *old = entry,
            None => base.push(entry),
        }
    }
    base
}

impl Config {
    /// `self` with everything `over` sets in place of what `self` says.
    #[must_use]
    pub fn merge(self, over: Config) -> Config {
        let mut keybindings = self.keybindings;
        keybindings.extend(over.keybindings);
        Config {
            theme: over.theme.or(self.theme),
            display: DisplayConfig {
                color_mode: over.display.color_mode.or(self.display.color_mode),
                point_size: over.display.point_size.or(self.display.point_size),
                edl: EdlConfig {
                    enabled: over.display.edl.enabled.or(self.display.edl.enabled),
                    strength: over.display.edl.strength.or(self.display.edl.strength),
                    radius: over.display.edl.radius.or(self.display.edl.radius),
                },
                projection: over.display.projection.or(self.display.projection),
            },
            performance: PerformanceConfig {
                point_budget: over
                    .performance
                    .point_budget
                    .or(self.performance.point_budget),
                spacing_px: over.performance.spacing_px.or(self.performance.spacing_px),
            },
            export: ExportConfig {
                match_view: over.export.match_view.or(self.export.match_view),
                width: over.export.width.or(self.export.width),
                height: over.export.height.or(self.export.height),
            },
            keybindings,
            connectors: Connectors {
                stac: merge_by_name(self.connectors.stac, over.connectors.stac),
                folders: merge_by_name(self.connectors.folders, over.connectors.folders),
            },
        }
    }

    /// Reads a settings file's text. `file` names it in warnings and is
    /// where relative folder paths are resolved from; `None` leaves them
    /// relative.
    pub fn parse(text: &str, file: Option<&Path>) -> (Config, Vec<Warning>) {
        parse::parse(text, file)
    }

    /// As a settings file would say it, leaving out what isn't set.
    pub fn to_json(&self) -> serde_json::Value {
        parse::to_json(self)
    }
}

/// Something wrong with a settings file. Loading carries on without the
/// bad part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub file: Option<PathBuf>,
    pub message: String,
}

impl Warning {
    pub fn new(file: Option<&Path>, message: impl Into<String>) -> Self {
        Self {
            file: file.map(Path::to_path_buf),
            message: message.into(),
        }
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.file {
            Some(file) => write!(f, "{}: {}", file.display(), self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// Where the settings files are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Locations {
    /// The user's settings directory, normally `~/.copcat`.
    pub user_dir: Option<PathBuf>,
    /// The workspace folder, whose `.copcat` directory holds its settings.
    pub workspace: Option<PathBuf>,
}

impl Locations {
    /// The user directory from `COPCAT_CONFIG_DIR` or the home directory,
    /// and the workspace from `COPCAT_WORKSPACE` or the current directory.
    pub fn from_env() -> Self {
        let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            user_dir: var(CONFIG_DIR_ENV)
                .map(PathBuf::from)
                .or_else(|| home_dir().map(|home| home.join(DIR_NAME))),
            workspace: var(WORKSPACE_ENV)
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok()),
        }
    }

    pub fn user_file(&self) -> Option<PathBuf> {
        self.user_dir.as_ref().map(|dir| dir.join(FILE_NAME))
    }

    /// `None` without a workspace, or when the workspace's settings file is
    /// the user's (started from the home directory), so it isn't read twice.
    pub fn workspace_file(&self) -> Option<PathBuf> {
        let file = self.workspace.as_ref()?.join(DIR_NAME).join(FILE_NAME);
        let same_as_user = self.user_file().is_some_and(|user| same_file(&user, &file));
        (!same_as_user).then_some(file)
    }

    /// Loads every settings file, in order of precedence.
    pub fn load(&self) -> Loaded {
        let mut loaded = Loaded::default();
        for file in [self.user_file(), self.workspace_file()]
            .into_iter()
            .flatten()
        {
            let (config, warnings) = read_file(&file);
            loaded.config = std::mem::take(&mut loaded.config).merge(config);
            loaded.warnings.extend(warnings);
        }
        loaded
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// The home directory, from `USERPROFILE` on Windows or `HOME`.
pub(crate) fn home_dir() -> Option<PathBuf> {
    let var = |name| std::env::var_os(name).filter(|value| !value.is_empty());
    let home = if cfg!(windows) {
        var("USERPROFILE").or_else(|| var("HOME"))
    } else {
        var("HOME")
    };
    home.map(PathBuf::from)
}

/// The merged settings and every problem found on the way.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<Warning>,
}

/// One settings file. A missing file is an empty one.
pub fn read_file(file: &Path) -> (Config, Vec<Warning>) {
    match std::fs::read_to_string(file) {
        Ok(text) => Config::parse(&text, Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Default::default(),
        Err(error) => (
            Config::default(),
            vec![Warning::new(
                Some(file),
                format!("couldn't read it: {error}"),
            )],
        ),
    }
}

#[cfg(test)]
mod tests;
