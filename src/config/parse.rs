//! Reading a settings file's JSON into a [`Config`] value by value, so one
//! bad value costs only itself, and writing a [`Config`] back as JSON.

use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::{
    Binding, ColorModeName, Config, FolderConnector, StacConnector, Warning, jsonc,
    projection_name, ranges,
};
use crate::camera::Projection;

const TOP_KEYS: &[&str] = &[
    "$schema",
    "theme",
    "display",
    "performance",
    "export",
    "keybindings",
    "connectors",
];

pub(super) fn parse(text: &str, file: Option<&Path>) -> (Config, Vec<Warning>) {
    let mut reader = Reader {
        file,
        warnings: Vec::new(),
    };
    let json = jsonc::to_json(text);
    if json.trim().is_empty() {
        return (Config::default(), reader.warnings);
    }
    let config = match serde_json::from_str::<Value>(&json) {
        Ok(value) => reader.config(&value),
        Err(error) => {
            reader.warn(format!("not valid JSON, so none of it was used: {error}"));
            Config::default()
        }
    };
    (config, reader.warnings)
}

struct Reader<'a> {
    file: Option<&'a Path>,
    warnings: Vec<Warning>,
}

/// `key` under `path`, as a dotted path for messages.
fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

/// A JSON value's kind, for messages.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

impl Reader<'_> {
    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(Warning::new(self.file, message));
    }

    fn config(&mut self, value: &Value) -> Config {
        let mut config = Config::default();
        let Some(top) = self.object(value, "", TOP_KEYS) else {
            return config;
        };
        config.theme = self.string(top, "", "theme");
        if let Some(display) = self.child(
            top,
            "",
            "display",
            &["colorMode", "pointSize", "edl", "projection"],
        ) {
            let path = "display";
            config.display.color_mode = self.choice(
                display,
                path,
                "colorMode",
                &[
                    ("height", ColorModeName::Height),
                    ("rgb", ColorModeName::Rgb),
                    ("classification", ColorModeName::Classification),
                ],
            );
            config.display.point_size = self.f32_in(display, path, "pointSize", ranges::POINT_SIZE);
            config.display.projection = self.choice(
                display,
                path,
                "projection",
                &[
                    ("perspective", Projection::Perspective),
                    ("orthographic", Projection::Orthographic),
                ],
            );
            if let Some(edl) = self.child(display, path, "edl", &["enabled", "strength", "radius"])
            {
                let path = "display.edl";
                config.display.edl.enabled = self.bool(edl, path, "enabled");
                config.display.edl.strength =
                    self.f32_in(edl, path, "strength", ranges::EDL_STRENGTH);
                config.display.edl.radius = self.f32_in(edl, path, "radius", ranges::EDL_RADIUS);
            }
        }
        if let Some(performance) = self.child(top, "", "performance", &["pointBudget", "spacingPx"])
        {
            let path = "performance";
            let budget = ranges::POINT_BUDGET;
            // Exact: both ends are well under 2^53.
            let budget = *budget.start() as f64..=*budget.end() as f64;
            config.performance.point_budget = self
                .number_in(performance, path, "pointBudget", budget)
                .map(|value| value.round() as u64);
            config.performance.spacing_px =
                self.f32_in(performance, path, "spacingPx", ranges::SPACING_PX);
        }
        if let Some(export) = self.child(top, "", "export", &["matchView", "width", "height"]) {
            let path = "export";
            config.export.match_view = self.bool(export, path, "matchView");
            let side = as_f64(&ranges::EXPORT_SIDE);
            config.export.width = self
                .number_in(export, path, "width", side.clone())
                .map(|value| value.round() as u32);
            config.export.height = self
                .number_in(export, path, "height", side)
                .map(|value| value.round() as u32);
        }
        if let Some(bindings) = top.get("keybindings").filter(|value| !value.is_null()) {
            self.keybindings(bindings, &mut config);
        }
        if let Some(connectors) = self.child(top, "", "connectors", &["stac", "folders"]) {
            self.connectors(connectors, &mut config);
        }
        config
    }

    fn keybindings(&mut self, value: &Value, config: &mut Config) {
        let Some(bindings) = self.object(value, "keybindings", &[]) else {
            return;
        };
        for (id, shortcut) in bindings {
            let shortcut = match shortcut {
                Value::Null => None,
                Value::String(text) if text.trim().is_empty() => None,
                Value::String(text) => Some(text.trim().to_owned()),
                other => {
                    self.warn(format!(
                        "keybindings.{id} should be a shortcut like \"Ctrl+Shift+P\", or null \
                         for none, not {}",
                        kind(other)
                    ));
                    continue;
                }
            };
            config.keybindings.insert(
                id.clone(),
                Binding {
                    shortcut,
                    file: self.file.map(Path::to_path_buf),
                },
            );
        }
    }

    fn connectors(&mut self, connectors: &Map<String, Value>, config: &mut Config) {
        for (index, entry) in self.list(connectors, "connectors", "stac") {
            let path = format!("connectors.stac[{index}]");
            let Some(entry) = self.object(entry, &path, &["name", "url"]) else {
                continue;
            };
            let (Some(name), Some(url)) = (
                self.required_string(entry, &path, "name"),
                self.required_string(entry, &path, "url"),
            ) else {
                continue;
            };
            match url::Url::parse(&url) {
                Ok(parsed) if matches!(parsed.scheme(), "http" | "https") => {}
                _ => {
                    self.warn(format!(
                        "{path}.url should be an http or https URL, not {url:?}"
                    ));
                    continue;
                }
            }
            self.check_unique(
                config.connectors.stac.iter().map(|c| &c.name),
                &name,
                "stac",
            );
            config.connectors.stac = super::merge_by_name(
                std::mem::take(&mut config.connectors.stac),
                vec![StacConnector { name, url }],
            );
        }
        for (index, entry) in self.list(connectors, "connectors", "folders") {
            let path = format!("connectors.folders[{index}]");
            let Some(entry) = self.object(entry, &path, &["name", "path"]) else {
                continue;
            };
            let (Some(name), Some(folder)) = (
                self.required_string(entry, &path, "name"),
                self.required_string(entry, &path, "path"),
            ) else {
                continue;
            };
            let path = self.resolve(PathBuf::from(folder));
            self.check_unique(
                config.connectors.folders.iter().map(|c| &c.name),
                &name,
                "folders",
            );
            config.connectors.folders = super::merge_by_name(
                std::mem::take(&mut config.connectors.folders),
                vec![FolderConnector { name, path }],
            );
        }
    }

    fn check_unique<'n>(
        &mut self,
        mut names: impl Iterator<Item = &'n String>,
        name: &str,
        list: &str,
    ) {
        if names.any(|seen| seen == name) {
            self.warn(format!(
                "connectors.{list} has two entries named {name:?}; the last one is used"
            ));
        }
    }

    /// Relative to the folder holding the file's `.copcat` directory.
    fn resolve(&self, path: PathBuf) -> PathBuf {
        if path.is_absolute() {
            return path;
        }
        match self.file.and_then(Path::parent).and_then(Path::parent) {
            Some(base) => base.join(path),
            None => path,
        }
    }

    /// `value` as an object, warning about keys not in `known` (unless
    /// `known` is empty, which allows any).
    fn object<'v>(
        &mut self,
        value: &'v Value,
        path: &str,
        known: &[&str],
    ) -> Option<&'v Map<String, Value>> {
        let Some(map) = value.as_object() else {
            let what = if path.is_empty() {
                "the file".to_owned()
            } else {
                path.to_owned()
            };
            self.warn(format!("{what} should be an object, not {}", kind(value)));
            return None;
        };
        if !known.is_empty() {
            for key in map.keys().filter(|key| !known.contains(&key.as_str())) {
                self.warn(format!("unknown setting {}", join(path, key)));
            }
        }
        Some(map)
    }

    fn child<'v>(
        &mut self,
        map: &'v Map<String, Value>,
        path: &str,
        key: &str,
        known: &[&str],
    ) -> Option<&'v Map<String, Value>> {
        match map.get(key)? {
            Value::Null => None,
            value => self.object(value, &join(path, key), known),
        }
    }

    fn list<'v>(
        &mut self,
        map: &'v Map<String, Value>,
        path: &str,
        key: &str,
    ) -> Vec<(usize, &'v Value)> {
        match map.get(key) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items.iter().enumerate().collect(),
            Some(other) => {
                self.warn(format!(
                    "{} should be a list, not {}",
                    join(path, key),
                    kind(other)
                ));
                Vec::new()
            }
        }
    }

    /// The value at `key`, or `None` without a warning if it's missing or
    /// null, or with one if `read` rejects it.
    fn value<T>(
        &mut self,
        map: &Map<String, Value>,
        path: &str,
        key: &str,
        expected: &str,
        read: impl FnOnce(&Value) -> Option<T>,
    ) -> Option<T> {
        let value = map.get(key).filter(|value| !value.is_null())?;
        let read = read(value);
        if read.is_none() {
            self.warn(format!(
                "{} should be {expected}, not {}",
                join(path, key),
                describe(value)
            ));
        }
        read
    }

    fn bool(&mut self, map: &Map<String, Value>, path: &str, key: &str) -> Option<bool> {
        self.value(map, path, key, "true or false", Value::as_bool)
    }

    fn string(&mut self, map: &Map<String, Value>, path: &str, key: &str) -> Option<String> {
        self.value(map, path, key, "a string", |value| {
            value.as_str().map(str::to_owned)
        })
    }

    /// A non-empty string that must be there.
    fn required_string(
        &mut self,
        map: &Map<String, Value>,
        path: &str,
        key: &str,
    ) -> Option<String> {
        let text = self.string(map, path, key);
        let text = text
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty());
        match (text, map.get(key)) {
            (Some(text), _) => return Some(text.to_owned()),
            // Wrong type: `string` has said so.
            (None, Some(value)) if !value.is_null() && !value.is_string() => {
                self.warn(format!("skipped {path}"));
            }
            (None, _) => self.warn(format!(
                "{} is missing; skipped this entry",
                join(path, key)
            )),
        }
        None
    }

    fn choice<T: Copy>(
        &mut self,
        map: &Map<String, Value>,
        path: &str,
        key: &str,
        choices: &[(&str, T)],
    ) -> Option<T> {
        let names: Vec<_> = choices
            .iter()
            .map(|(name, _)| format!("{name:?}"))
            .collect();
        let expected = format!("one of {}", names.join(", "));
        self.value(map, path, key, &expected, |value| {
            let text = value.as_str()?.trim();
            choices
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(text))
                .map(|(_, choice)| *choice)
        })
    }

    /// A number, clamped to `range` with a warning if it's outside.
    fn number_in(
        &mut self,
        map: &Map<String, Value>,
        path: &str,
        key: &str,
        range: RangeInclusive<f64>,
    ) -> Option<f64> {
        let number = self.value(map, path, key, "a number", Value::as_f64)?;
        let clamped = number.clamp(*range.start(), *range.end());
        if clamped != number {
            self.warn(format!(
                "{} is {number}, outside {} to {}; using {clamped}",
                join(path, key),
                range.start(),
                range.end()
            ));
        }
        Some(clamped)
    }

    fn f32_in(
        &mut self,
        map: &Map<String, Value>,
        path: &str,
        key: &str,
        range: RangeInclusive<f32>,
    ) -> Option<f32> {
        self.number_in(map, path, key, as_f64(&range))
            .map(|value| value as f32)
    }
}

/// A value as a message quotes it: short ones in full.
fn describe(value: &Value) -> String {
    match value {
        Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            let text = value.to_string();
            if text.chars().count() <= 40 {
                text
            } else {
                kind(value).to_owned()
            }
        }
        _ => kind(value).to_owned(),
    }
}

fn as_f64<T: Copy + Into<f64>>(range: &RangeInclusive<T>) -> RangeInclusive<f64> {
    (*range.start()).into()..=(*range.end()).into()
}

/// An `f32` as JSON, written as briefly as it reads back (`1.4`, not
/// `1.399999976158142`).
fn f32_json(value: f32) -> Value {
    let shortest: f64 = value.to_string().parse().unwrap_or(f64::from(value));
    json!(shortest)
}

/// Inserts `value` at `key` if there is one.
fn put(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), value);
    }
}

/// Inserts `object` at `key` unless it's empty.
fn put_object(map: &mut Map<String, Value>, key: &str, object: Map<String, Value>) {
    if !object.is_empty() {
        map.insert(key.to_owned(), Value::Object(object));
    }
}

pub(super) fn to_json(config: &Config) -> Value {
    let mut top = Map::new();
    put(&mut top, "theme", config.theme.clone().map(Value::from));

    let display = &config.display;
    let mut edl = Map::new();
    put(&mut edl, "enabled", display.edl.enabled.map(Value::from));
    put(&mut edl, "strength", display.edl.strength.map(f32_json));
    put(&mut edl, "radius", display.edl.radius.map(f32_json));
    let mut display_json = Map::new();
    put(
        &mut display_json,
        "colorMode",
        display.color_mode.map(|mode| Value::from(mode.name())),
    );
    put(
        &mut display_json,
        "pointSize",
        display.point_size.map(f32_json),
    );
    put_object(&mut display_json, "edl", edl);
    put(
        &mut display_json,
        "projection",
        display
            .projection
            .map(|projection| Value::from(projection_name(projection))),
    );
    put_object(&mut top, "display", display_json);

    let mut performance = Map::new();
    put(
        &mut performance,
        "pointBudget",
        config.performance.point_budget.map(Value::from),
    );
    put(
        &mut performance,
        "spacingPx",
        config.performance.spacing_px.map(f32_json),
    );
    put_object(&mut top, "performance", performance);

    let mut export = Map::new();
    put(
        &mut export,
        "matchView",
        config.export.match_view.map(Value::from),
    );
    put(&mut export, "width", config.export.width.map(Value::from));
    put(&mut export, "height", config.export.height.map(Value::from));
    put_object(&mut top, "export", export);

    let bindings: Map<_, _> = config
        .keybindings
        .iter()
        .map(|(id, binding)| (id.clone(), json!(binding.shortcut)))
        .collect();
    put_object(&mut top, "keybindings", bindings);

    let mut connectors = Map::new();
    if !config.connectors.stac.is_empty() {
        let stac = config
            .connectors
            .stac
            .iter()
            .map(|c| json!({"name": c.name, "url": c.url}))
            .collect();
        connectors.insert("stac".to_owned(), Value::Array(stac));
    }
    if !config.connectors.folders.is_empty() {
        let folders = config
            .connectors
            .folders
            .iter()
            .map(|c| json!({"name": c.name, "path": c.path.display().to_string()}))
            .collect();
        connectors.insert("folders".to_owned(), Value::Array(folders));
    }
    put_object(&mut top, "connectors", connectors);
    Value::Object(top)
}
