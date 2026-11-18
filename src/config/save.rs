//! Creating, opening and updating settings files.

use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use super::jsonc;

/// What a new user settings file says: nothing yet, with the schema in
/// comments.
pub(crate) const USER_TEMPLATE: &str = r#"// Copcat user settings. Every key is optional: uncomment and change what
// you want. A workspace's .copcat/settings.json overrides these key by key.
// See "Settings" in Copcat's README for every key.
{
  // "theme": "graphite",
  // "display": {
  //   "colorMode": "rgb",            // "height", "rgb" or "classification"
  //   "pointSize": 2.0,              // 1 to 10
  //   "edl": { "enabled": true, "strength": 1.0, "radius": 1.4 },
  //   "projection": "perspective"    // or "orthographic"
  // },
  // "performance": { "pointBudget": 5000000, "spacingPx": 2.0 },
  // "export": { "matchView": true, "width": 1920, "height": 1080 },
  // "keybindings": { "view.top": "7", "view.reset": "Ctrl+R" },
  // "connectors": {
  //   "stac": [{ "name": "My STAC API", "url": "https://example.com/stac/" }],
  //   "folders": [{ "name": "Survey data", "path": "D:/data/surveys" }]
  // }
}
"#;

/// What a new workspace settings file says.
pub(crate) const WORKSPACE_TEMPLATE: &str = r#"// Copcat workspace settings, for when Copcat is started in this folder.
// They override the user settings (~/.copcat/settings.json) key by key, and
// take the same keys. Relative folder connector paths are relative to this
// folder's parent (the folder holding .copcat). See "Settings" in Copcat's
// README.
{
  // "display": { "colorMode": "classification" },
  // "connectors": {
  //   "folders": [{ "name": "This project", "path": "." }]
  // }
}
"#;

/// Writes `template` to `file`, and the directories it's in, unless the
/// file is already there. Returns whether it wrote it.
pub(crate) fn create_if_missing(file: &Path, template: &str) -> io::Result<bool> {
    if file.exists() {
        return Ok(false);
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
    {
        Ok(mut created) => {
            io::Write::write_all(&mut created, template.as_bytes())?;
            Ok(true)
        }
        // Created since `exists` looked: leave it.
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

/// Opens `file` in the program the system uses for it, without waiting.
pub(crate) fn open_in_editor(file: &Path) -> io::Result<()> {
    let mut command = if cfg!(windows) {
        // Explorer opens a file with its associated program, and needs no
        // quoting, unlike `cmd /c start`.
        std::process::Command::new("explorer")
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    command.arg(file).spawn().map(drop)
}

/// Writes `updates` into the settings file `file`, key by key at every
/// depth, keeping every other key. A missing file is created. Refuses (and
/// leaves the file alone) if what's there isn't a JSON object.
///
/// Returns whether comments or trailing commas were dropped: the file is
/// rewritten as plain JSON.
pub(crate) fn save_over(file: &Path, updates: &Value) -> Result<bool, String> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("couldn't read it: {error}")),
    };
    let json = jsonc::to_json(&text);
    let had_extras = json.trim() != text.trim_start_matches('\u{feff}').trim();
    let mut current = if json.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&json)
            .map_err(|error| format!("not valid JSON, so it wasn't changed: {error}"))?
    };
    if !current.is_object() {
        return Err("not a JSON object, so it wasn't changed".to_owned());
    }
    merge_into(&mut current, updates);
    let mut out = serde_json::to_string_pretty(&current).map_err(|error| error.to_string())?;
    out.push('\n');
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)
            .map_err(|error| format!("couldn't create {}: {error}", dir.display()))?;
    }
    // Written beside it then renamed over it, so a failed write can't leave
    // half a file.
    let partial = file.with_extension("json.partial");
    fs::write(&partial, out).map_err(|error| format!("couldn't write it: {error}"))?;
    fs::rename(&partial, file).map_err(|error| format!("couldn't replace it: {error}"))?;
    Ok(had_extras)
}

/// `over`'s keys written into `base`, merging objects at every depth.
fn merge_into(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                match base.get_mut(key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge_into(existing, value);
                    }
                    _ => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, over) => *base = over.clone(),
    }
}
