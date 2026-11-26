use std::fs;
use std::path::{Path, PathBuf};

use proptest::prelude::*;
use serde_json::json;

use super::*;

fn parse(text: &str) -> (Config, Vec<Warning>) {
    Config::parse(text, None)
}

/// Parses `text`, insisting there are no warnings.
fn parse_clean(text: &str) -> Config {
    let (config, warnings) = parse(text);
    assert!(warnings.is_empty(), "{warnings:?}");
    config
}

fn messages(warnings: &[Warning]) -> Vec<String> {
    warnings.iter().map(|w| w.message.clone()).collect()
}

fn write(dir: &Path, relative: &str, text: &str) -> PathBuf {
    let file = dir.join(relative);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, text).unwrap();
    file
}

const FULL: &str = r#"{
  "theme": "graphite",
  "display": {
    "colorMode": "classification",
    "pointSizeMode": "fixed",
    "pointSize": 3.5,
    "fill": 2.5,
    "minPointSize": 1.5,
    "maxPointSize": 32.0,
    "edl": { "enabled": false, "strength": 2.0, "radius": 2.5 },
    "projection": "orthographic"
  },
  "performance": { "pointBudget": 2000000, "spacingPx": 4.0 },
  "export": { "matchView": false, "width": 800, "height": 600 },
  "keybindings": { "view.top": "Ctrl+7", "view.reset": null },
  "connectors": {
    "stac": [{ "name": "NatureScan", "url": "https://stacapi.naturescan.earth/" }]
  }
}"#;

#[test]
fn a_full_file_should_set_everything() {
    let config = parse_clean(FULL);
    assert_eq!(config.theme.as_deref(), Some("graphite"));
    assert_eq!(
        config.display.color_mode,
        Some(ColorModeName::Classification)
    );
    assert_eq!(
        config.display.point_size_mode,
        Some(PointSizeModeName::Fixed)
    );
    assert_eq!(config.display.point_size, Some(3.5));
    assert_eq!(config.display.fill, Some(2.5));
    assert_eq!(config.display.min_point_size, Some(1.5));
    assert_eq!(config.display.max_point_size, Some(32.0));
    assert_eq!(config.display.edl.enabled, Some(false));
    assert_eq!(config.display.edl.strength, Some(2.0));
    assert_eq!(config.display.edl.radius, Some(2.5));
    assert_eq!(config.display.projection, Some(Projection::Orthographic));
    assert_eq!(config.performance.point_budget, Some(2_000_000));
    assert_eq!(config.performance.spacing_px, Some(4.0));
    assert_eq!(config.export.match_view, Some(false));
    assert_eq!(config.export.width, Some(800));
    assert_eq!(config.export.height, Some(600));
    assert_eq!(
        config.keybindings["view.top"].shortcut.as_deref(),
        Some("Ctrl+7")
    );
    assert_eq!(config.keybindings["view.reset"].shortcut, None);
    assert_eq!(
        config.connectors.stac,
        vec![StacConnector {
            name: "NatureScan".to_owned(),
            url: "https://stacapi.naturescan.earth/".to_owned(),
        }]
    );
}

#[test]
fn an_empty_file_should_set_nothing() {
    assert_eq!(parse_clean(""), Config::default());
    assert_eq!(parse_clean("  // only a comment\n"), Config::default());
    assert_eq!(parse_clean("{}"), Config::default());
}

#[test]
fn a_partial_file_should_set_only_what_it_says() {
    let config = parse_clean(r#"{"display": {"edl": {"strength": 2.5}}}"#);
    assert_eq!(
        config,
        Config {
            display: DisplayConfig {
                edl: EdlConfig {
                    strength: Some(2.5),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    );
}

#[test]
fn the_templates_should_parse_to_nothing() {
    assert_eq!(parse_clean(USER_TEMPLATE), Config::default());
    assert_eq!(parse_clean(WORKSPACE_TEMPLATE), Config::default());
}

#[test]
fn templates_should_be_valid_once_uncommented() {
    // Every commented key in the user template is a real one: uncommenting
    // them all gives a file that loads without warnings.
    let uncomment = |template: &str| -> String {
        template
            .lines()
            .map(|line| match line.trim_start().strip_prefix("// ") {
                Some(rest) if line.starts_with("  //") => format!("{rest}\n"),
                _ => format!("{line}\n"),
            })
            .collect()
    };
    let user = parse_clean(&uncomment(USER_TEMPLATE));
    assert_eq!(user.display.color_mode, Some(ColorModeName::Rgb));
    assert_eq!(user.display.point_size_mode, Some(PointSizeModeName::Fixed));
    assert!(user.display.fill.is_some());
    assert!(user.display.min_point_size.is_some());
    assert!(user.display.max_point_size.is_some());
    assert!(user.display.point_size.is_some());
    assert_eq!(user.connectors.stac.len(), 1);
    assert_eq!(user.keybindings.len(), 2);
    let workspace = parse_clean(&uncomment(WORKSPACE_TEMPLATE));
    assert_eq!(workspace.connectors.stac.len(), 1);
}

#[test]
fn comments_and_trailing_commas_should_be_allowed() {
    let config = parse_clean("{\n // why\n \"display\": {\"pointSize\": 4,},\n}");
    assert_eq!(config.display.point_size, Some(4.0));
}

#[test]
fn choices_should_ignore_case_and_spaces() {
    let config = parse_clean(
        r#"{"display": {"colorMode": " RGB ", "projection": "Perspective", "pointSizeMode": "Fixed "}}"#,
    );
    assert_eq!(config.display.color_mode, Some(ColorModeName::Rgb));
    assert_eq!(
        config.display.point_size_mode,
        Some(PointSizeModeName::Fixed)
    );
    assert_eq!(config.display.projection, Some(Projection::Perspective));
}

#[test]
fn invalid_json_should_warn_and_set_nothing() {
    let file = Path::new("/home/me/.copcat/settings.json");
    let (config, warnings) = Config::parse("{\"theme\": \"graphite\",, }", Some(file));
    assert_eq!(config, Config::default());
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0].file.as_deref(), Some(file));
    let shown = warnings[0].to_string();
    assert!(shown.contains("settings.json"), "{shown}");
    assert!(shown.contains("line 1"), "{shown}");
}

#[test]
fn a_file_that_isnt_an_object_should_warn() {
    let (config, warnings) = parse("[1, 2]");
    assert_eq!(config, Config::default());
    assert_eq!(
        messages(&warnings),
        ["the file should be an object, not a list"]
    );
}

#[test]
fn unknown_keys_should_warn_but_keep_the_rest() {
    let (config, warnings) =
        parse(r#"{"colour": 1, "display": {"pointSize": 3, "shiny": true, "edl": {"glow": 1}}}"#);
    assert_eq!(config.display.point_size, Some(3.0));
    assert_eq!(
        messages(&warnings),
        [
            "unknown setting colour",
            "unknown setting display.shiny",
            "unknown setting display.edl.glow"
        ]
    );
}

#[test]
fn wrong_types_should_warn_and_be_skipped() {
    let (config, warnings) = parse(
        r#"{
            "theme": 3,
            "display": {"pointSize": "big", "colorMode": "rainbow", "pointSizeMode": "huge", "fill": "lots", "edl": {"enabled": "yes", "strength": 2}},
            "performance": [],
            "keybindings": {"view.top": 7}
        }"#,
    );
    assert_eq!(config.theme, None);
    assert_eq!(config.display.point_size, None);
    assert_eq!(config.display.color_mode, None);
    assert_eq!(config.display.point_size_mode, None);
    assert_eq!(config.display.fill, None);
    assert_eq!(config.display.edl.enabled, None);
    // The good value beside the bad ones is still used.
    assert_eq!(config.display.edl.strength, Some(2.0));
    assert!(config.keybindings.is_empty());
    let messages = messages(&warnings);
    assert_eq!(messages.len(), 8, "{messages:#?}");
    assert!(messages.contains(
        &"display.pointSizeMode should be one of \"adaptive\", \"fixed\", not \"huge\"".to_owned()
    ));
    assert!(messages.contains(&"display.fill should be a number, not \"lots\"".to_owned()));
    assert!(messages.contains(&"theme should be a string, not 3".to_owned()));
    assert!(messages.contains(
        &"display.colorMode should be one of \"height\", \"rgb\", \"classification\", not \"rainbow\""
            .to_owned()
    ));
    assert!(messages.contains(&"performance should be an object, not a list".to_owned()));
}

#[test]
fn null_should_mean_not_set() {
    assert_eq!(
        parse_clean(r#"{"theme": null, "display": null, "connectors": {"stac": null}}"#),
        Config::default()
    );
}

#[test]
fn numbers_out_of_range_should_be_clamped_with_a_warning() {
    let (config, warnings) = parse(
        r#"{
            "display": {"pointSize": 50, "fill": 0.1, "minPointSize": 0, "maxPointSize": 100, "edl": {"strength": 0, "radius": 9}},
            "performance": {"pointBudget": 10, "spacingPx": 100},
            "export": {"width": 0, "height": 100000}
        }"#,
    );
    assert_eq!(config.display.point_size, Some(*ranges::POINT_SIZE.end()));
    assert_eq!(config.display.fill, Some(*ranges::FILL.start()));
    assert_eq!(
        config.display.min_point_size,
        Some(*ranges::MIN_POINT_SIZE.start())
    );
    assert_eq!(
        config.display.max_point_size,
        Some(*ranges::MAX_POINT_SIZE.end())
    );
    assert_eq!(
        config.display.edl.strength,
        Some(*ranges::EDL_STRENGTH.start())
    );
    assert_eq!(config.display.edl.radius, Some(*ranges::EDL_RADIUS.end()));
    assert_eq!(
        config.performance.point_budget,
        Some(*ranges::POINT_BUDGET.start())
    );
    assert_eq!(
        config.performance.spacing_px,
        Some(*ranges::SPACING_PX.end())
    );
    assert_eq!(config.export.width, Some(1));
    assert_eq!(config.export.height, Some(*ranges::EXPORT_SIDE.end()));
    assert_eq!(warnings.len(), 10, "{warnings:#?}");
    assert_eq!(
        warnings[0].message,
        "display.pointSize is 50, outside 1 to 10; using 10"
    );
    assert_eq!(
        warnings[1].message,
        "display.fill is 0.1, outside 0.5 to 4; using 0.5"
    );
}

#[test]
fn a_fractional_point_budget_should_round() {
    let config = parse_clean(r#"{"performance": {"pointBudget": 1000000.6}}"#);
    assert_eq!(config.performance.point_budget, Some(1_000_001));
}

#[test]
fn an_empty_keybinding_should_unbind() {
    let config = parse_clean(r#"{"keybindings": {"view.top": ""}}"#);
    assert_eq!(config.keybindings["view.top"].shortcut, None);
}

#[test]
fn keybindings_should_remember_their_file() {
    let file = Path::new("/w/.copcat/settings.json");
    let (config, _) = Config::parse(r#"{"keybindings": {"view.top": "T"}}"#, Some(file));
    assert_eq!(config.keybindings["view.top"].file.as_deref(), Some(file));
}

#[test]
fn bad_connectors_should_be_skipped_with_warnings() {
    let (config, warnings) = parse(
        r#"{"connectors": {
            "stac": [
                {"name": "Good", "url": "https://example.com/stac"},
                {"name": "", "url": "https://example.com"},
                {"name": "Ftp", "url": "ftp://example.com"},
                {"url": "https://example.com"},
                "just a string",
                {"name": "Extra", "url": "http://e.com", "token": "x"}
            ]
        }}"#,
    );
    let names: Vec<_> = config
        .connectors
        .stac
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["Good", "Extra"]);
    let messages = messages(&warnings);
    assert!(
        messages.contains(&"connectors.stac[1].name is missing; skipped this entry".to_owned())
    );
    assert!(
        messages.contains(
            &"connectors.stac[2].url should be an http or https URL, not \"ftp://example.com\""
                .to_owned()
        )
    );
    assert!(messages.contains(&"unknown setting connectors.stac[5].token".to_owned()));
    assert_eq!(messages.len(), 5, "{messages:#?}");
}

#[test]
fn an_old_folders_list_should_be_an_unknown_setting() {
    // Folder sources were removed; a file that still lists them loads its
    // STAC sources and warns about the rest, without failing.
    let (config, warnings) = parse(
        r#"{"connectors": {
            "stac": [{"name": "S", "url": "https://s.example/"}],
            "folders": [{"name": "Data", "path": "D:/data"}]
        }}"#,
    );
    assert_eq!(config.connectors.stac.len(), 1);
    assert_eq!(messages(&warnings), ["unknown setting connectors.folders"]);
}

#[test]
fn duplicate_connector_names_in_one_file_should_keep_the_last() {
    let (config, warnings) = parse(
        r#"{"connectors": {"stac": [
            {"name": "A", "url": "https://one.example"},
            {"name": "A", "url": "https://two.example"}
        ]}}"#,
    );
    assert_eq!(config.connectors.stac.len(), 1);
    assert_eq!(config.connectors.stac[0].url, "https://two.example");
    assert_eq!(warnings.len(), 1);
}

#[test]
fn merge_should_override_key_by_key() {
    let user = parse_clean(
        r#"{"theme": "graphite", "display": {"colorMode": "height", "edl": {"strength": 2, "radius": 2}}}"#,
    );
    let workspace = parse_clean(r#"{"display": {"edl": {"radius": 3}}}"#);
    let merged = user.merge(workspace);
    assert_eq!(merged.theme.as_deref(), Some("graphite"));
    assert_eq!(merged.display.color_mode, Some(ColorModeName::Height));
    assert_eq!(merged.display.edl.strength, Some(2.0));
    assert_eq!(merged.display.edl.radius, Some(3.0));
}

#[test]
fn merge_should_layer_point_size_settings_key_by_key() {
    let user = parse_clean(
        r#"{"display": {"pointSizeMode": "fixed", "pointSize": 4, "fill": 2, "minPointSize": 2}}"#,
    );
    // Sets the fixed size only: the user file's mode stays.
    let workspace = parse_clean(r#"{"display": {"pointSize": 6, "maxPointSize": 30}}"#);
    let merged = user.merge(workspace);
    assert_eq!(
        merged.display.point_size_mode,
        Some(PointSizeModeName::Fixed)
    );
    assert_eq!(merged.display.point_size, Some(6.0));
    assert_eq!(merged.display.fill, Some(2.0));
    assert_eq!(merged.display.min_point_size, Some(2.0));
    assert_eq!(merged.display.max_point_size, Some(30.0));

    let back = parse_clean(r#"{"display": {"pointSizeMode": "adaptive"}}"#);
    let merged = merged.merge(back);
    assert_eq!(
        merged.display.point_size_mode,
        Some(PointSizeModeName::Adaptive)
    );
    assert_eq!(merged.display.point_size, Some(6.0));
}

#[test]
fn point_size_alone_should_not_set_the_mode() {
    let config = parse_clean(r#"{"display": {"pointSize": 5}}"#);
    assert_eq!(config.display.point_size, Some(5.0));
    assert_eq!(config.display.point_size_mode, None);
}

#[test]
fn merge_should_combine_keybindings_by_id() {
    let user = parse_clean(r#"{"keybindings": {"view.top": "T", "view.front": "F"}}"#);
    let workspace = parse_clean(r#"{"keybindings": {"view.top": null}}"#);
    let merged = user.merge(workspace);
    assert_eq!(merged.keybindings["view.top"].shortcut, None);
    assert_eq!(
        merged.keybindings["view.front"].shortcut.as_deref(),
        Some("F")
    );
}

#[test]
fn merge_should_combine_connectors_by_name() {
    let user = parse_clean(
        r#"{"connectors": {"stac": [
            {"name": "A", "url": "https://a.example"},
            {"name": "B", "url": "https://b.example"}
        ]}}"#,
    );
    let workspace = parse_clean(
        r#"{"connectors": {"stac": [
            {"name": "C", "url": "https://c.example"},
            {"name": "A", "url": "https://a2.example"}
        ]}}"#,
    );
    let merged = user.merge(workspace);
    let entries: Vec<_> = merged
        .connectors
        .stac
        .iter()
        .map(|c| (c.name.as_str(), c.url.as_str()))
        .collect();
    assert_eq!(
        entries,
        [
            ("A", "https://a2.example"),
            ("B", "https://b.example"),
            ("C", "https://c.example")
        ]
    );
}

#[test]
fn load_should_layer_the_workspace_over_the_user_file() {
    let temp = tempfile::tempdir().unwrap();
    let user_dir = temp.path().join("home").join(DIR_NAME);
    let workspace = temp.path().join("project");
    write(
        &user_dir,
        FILE_NAME,
        r#"{"display": {"pointSize": 3, "colorMode": "height"}}"#,
    );
    write(
        &workspace,
        ".copcat/settings.json",
        r#"{"display": {"pointSize": 5}, "connectors": {"stac": [{"name": "P", "url": "https://p.example/"}]}}"#,
    );
    let loaded = Locations {
        user_dir: Some(user_dir),
        workspace: Some(workspace.clone()),
    }
    .load();
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.config.display.point_size, Some(5.0));
    assert_eq!(
        loaded.config.display.color_mode,
        Some(ColorModeName::Height)
    );
    assert_eq!(loaded.config.connectors.stac[0].name, "P");
}

#[test]
fn load_should_skip_missing_files_quietly() {
    let temp = tempfile::tempdir().unwrap();
    let loaded = Locations {
        user_dir: Some(temp.path().join("nothing")),
        workspace: Some(temp.path().join("nowhere")),
    }
    .load();
    assert_eq!(loaded, Loaded::default());
}

#[test]
fn load_should_keep_the_good_file_when_the_other_is_bad() {
    let temp = tempfile::tempdir().unwrap();
    let user_dir = temp.path().join("user");
    write(&user_dir, FILE_NAME, r#"{"display": {"pointSize": 4}}"#);
    let bad = write(temp.path(), "ws/.copcat/settings.json", "{ nope");
    let loaded = Locations {
        user_dir: Some(user_dir),
        workspace: Some(temp.path().join("ws")),
    }
    .load();
    assert_eq!(loaded.config.display.point_size, Some(4.0));
    assert_eq!(loaded.warnings.len(), 1);
    assert_eq!(loaded.warnings[0].file.as_deref(), Some(bad.as_path()));
}

#[test]
fn the_workspace_file_should_not_be_the_user_file_twice() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(), ".copcat/settings.json", "{}");
    let locations = Locations {
        user_dir: Some(temp.path().join(DIR_NAME)),
        workspace: Some(temp.path().to_path_buf()),
    };
    assert_eq!(locations.workspace_file(), None);
    assert!(locations.user_file().is_some());
}

#[test]
fn create_if_missing_should_write_the_template_once() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("a").join("b").join(FILE_NAME);
    assert!(create_if_missing(&file, USER_TEMPLATE).unwrap());
    assert_eq!(fs::read_to_string(&file).unwrap(), USER_TEMPLATE);
    fs::write(&file, "{\"theme\": \"x\"}").unwrap();
    assert!(!create_if_missing(&file, USER_TEMPLATE).unwrap());
    assert_eq!(fs::read_to_string(&file).unwrap(), "{\"theme\": \"x\"}");
}

#[test]
fn save_over_should_keep_other_keys() {
    let temp = tempfile::tempdir().unwrap();
    let file = write(
        temp.path(),
        FILE_NAME,
        r#"{
  "theme": "graphite",
  "display": {"colorMode": "height", "edl": {"radius": 3.0}},
  "keybindings": {"view.top": "T"},
  "someFutureKey": [1, 2]
}"#,
    );
    let dropped = save_over(
        &file,
        &json!({"display": {"pointSize": 4.0, "edl": {"strength": 2.0}}}),
    )
    .unwrap();
    assert!(!dropped);
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({
            "theme": "graphite",
            "display": {"colorMode": "height", "edl": {"radius": 3.0, "strength": 2.0}, "pointSize": 4.0},
            "keybindings": {"view.top": "T"},
            "someFutureKey": [1, 2]
        })
    );
    // In the same order as before, new keys last.
    let keys: Vec<_> = saved.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["theme", "display", "keybindings", "someFutureKey"]);
}

#[test]
fn save_over_should_create_a_missing_file() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("new").join(FILE_NAME);
    save_over(&file, &json!({"theme": "graphite"})).unwrap();
    assert_eq!(read_file(&file).0.theme.as_deref(), Some("graphite"));
}

#[test]
fn save_over_should_report_dropped_comments() {
    let temp = tempfile::tempdir().unwrap();
    let file = write(temp.path(), FILE_NAME, USER_TEMPLATE);
    assert!(save_over(&file, &json!({"display": {"pointSize": 2.0}})).unwrap());
    assert_eq!(read_file(&file).0.display.point_size, Some(2.0));
}

#[test]
fn save_over_should_leave_an_invalid_file_alone() {
    let temp = tempfile::tempdir().unwrap();
    let file = write(temp.path(), FILE_NAME, "{ broken");
    assert!(save_over(&file, &json!({"theme": "graphite"})).is_err());
    assert_eq!(fs::read_to_string(&file).unwrap(), "{ broken");
    let array = write(temp.path(), "array.json", "[]");
    assert!(save_over(&array, &json!({"theme": "graphite"})).is_err());
}

#[test]
fn add_connector_should_append_and_keep_other_keys() {
    let temp = tempfile::tempdir().unwrap();
    let file = write(
        temp.path(),
        FILE_NAME,
        r#"{"theme": "graphite", "connectors": {"other": [{"name": "F", "path": "."}],
            "stac": [{"name": "A", "url": "https://a.example/"}]}}"#,
    );
    add_connector(&file, json!({"name": "B", "url": "https://b.example/"})).unwrap();
    // The same name replaces the old one, in its place.
    add_connector(&file, json!({"name": "A", "url": "https://a2.example/"})).unwrap();
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(
        saved,
        json!({"theme": "graphite", "connectors": {"other": [{"name": "F", "path": "."}],
            "stac": [{"name": "A", "url": "https://a2.example/"}, {"name": "B", "url": "https://b.example/"}]}})
    );
}

#[test]
fn add_connector_should_create_the_file_and_list() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("home").join(FILE_NAME);
    add_connector(&file, json!({"name": "Data", "url": "https://d.example/"})).unwrap();
    let (config, warnings) = read_file(&file);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(config.connectors.stac[0].name, "Data");
}

#[test]
fn add_connector_should_refuse_a_connectors_key_of_the_wrong_type() {
    let temp = tempfile::tempdir().unwrap();
    let file = write(temp.path(), FILE_NAME, r#"{"connectors": {"stac": "x"}}"#);
    let entry = json!({"name": "B", "url": "https://b.example/"});
    assert!(add_connector(&file, entry).is_err());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        r#"{"connectors": {"stac": "x"}}"#
    );
}

#[test]
fn remove_connector_should_remove_by_name_or_say_it_isnt_there() {
    let temp = tempfile::tempdir().unwrap();
    let file = write(
        temp.path(),
        FILE_NAME,
        r#"{"connectors": {"stac": [{"name": "A", "url": "https://a.example/"},
            {"name": "B", "url": "https://b.example/"}]}}"#,
    );
    remove_connector(&file, "A").unwrap();
    let error = remove_connector(&file, "C").unwrap_err();
    assert!(
        error.contains("no connectors.stac entry named \"C\""),
        "{error}"
    );
    let names: Vec<_> = read_file(&file)
        .0
        .connectors
        .stac
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["B"]);
}

#[test]
fn floats_should_be_written_briefly() {
    let config = Config {
        display: DisplayConfig {
            edl: EdlConfig {
                radius: Some(1.4),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(
        config.to_json().to_string(),
        r#"{"display":{"edl":{"radius":1.4}}}"#
    );
}

#[test]
fn an_empty_config_should_write_an_empty_object() {
    assert_eq!(Config::default().to_json(), json!({}));
}

#[test]
fn a_full_file_should_round_trip() {
    let config = parse_clean(FULL);
    assert_eq!(parse_clean(&config.to_json().to_string()), config);
}

fn in_range(range: std::ops::RangeInclusive<f32>) -> impl Strategy<Value = f32> {
    *range.start()..=*range.end()
}

fn name() -> impl Strategy<Value = String> {
    "[A-Za-z][A-Za-z0-9 ]{0,12}[A-Za-z0-9]"
}

prop_compose! {
    fn arb_config()(
        theme in proptest::option::of("[a-z]{1,10}"),
        color_mode in proptest::option::of(proptest::sample::select(ColorModeName::ALL.to_vec())),
        point_size_mode in proptest::option::of(proptest::sample::select(PointSizeModeName::ALL.to_vec())),
        point_size in proptest::option::of(in_range(ranges::POINT_SIZE)),
        fill in proptest::option::of(in_range(ranges::FILL)),
        min_point_size in proptest::option::of(in_range(ranges::MIN_POINT_SIZE)),
        max_point_size in proptest::option::of(in_range(ranges::MAX_POINT_SIZE)),
        enabled in proptest::option::of(any::<bool>()),
        strength in proptest::option::of(in_range(ranges::EDL_STRENGTH)),
        radius in proptest::option::of(in_range(ranges::EDL_RADIUS)),
        projection in proptest::option::of(proptest::sample::select(vec![
            Projection::Perspective,
            Projection::Orthographic,
        ])),
        point_budget in proptest::option::of(ranges::POINT_BUDGET),
        spacing_px in proptest::option::of(in_range(ranges::SPACING_PX)),
        match_view in proptest::option::of(any::<bool>()),
        width in proptest::option::of(ranges::EXPORT_SIDE),
        height in proptest::option::of(ranges::EXPORT_SIDE),
        bindings in proptest::collection::btree_map(
            "[a-z]{1,6}\\.[a-z_]{1,8}",
            proptest::option::of("(Ctrl\\+)?(Shift\\+)?[A-Z0-9]"),
            0..4,
        ),
        stac in proptest::collection::btree_map(name(), "[a-z]{1,8}", 0..3),
    ) -> Config {
        Config {
            theme,
            display: DisplayConfig {
                color_mode,
                point_size_mode,
                point_size,
                fill,
                min_point_size,
                max_point_size,
                edl: EdlConfig { enabled, strength, radius },
                projection,
            },
            performance: PerformanceConfig { point_budget, spacing_px },
            export: ExportConfig { match_view, width, height },
            keybindings: bindings
                .into_iter()
                .map(|(id, shortcut)| (id, Binding { shortcut, file: None }))
                .collect(),
            connectors: Connectors {
                stac: stac
                    .into_iter()
                    .map(|(name, host)| StacConnector { name, url: format!("https://{host}.example/") })
                    .collect(),
            },
        }
    }
}

proptest! {
    #[test]
    fn any_config_should_round_trip_through_json(config in arb_config()) {
        let text = serde_json::to_string_pretty(&config.to_json()).unwrap();
        let (read, warnings) = Config::parse(&text, None);
        prop_assert!(warnings.is_empty(), "{:?}", warnings);
        prop_assert_eq!(read, config);
    }

    #[test]
    fn merging_with_an_empty_config_should_change_nothing(config in arb_config()) {
        prop_assert_eq!(config.clone().merge(Config::default()), config.clone());
        prop_assert_eq!(Config::default().merge(config.clone()), config);
    }

    #[test]
    fn a_config_merged_over_itself_should_be_itself(config in arb_config()) {
        prop_assert_eq!(config.clone().merge(config.clone()), config);
    }

    #[test]
    fn parsing_any_text_should_not_panic(text in "\\PC{0,200}") {
        let _ = Config::parse(&text, None);
    }

    #[test]
    fn parsing_any_json_should_not_panic(value in arb_json()) {
        let _ = Config::parse(&value.to_string(), None);
    }
}

fn arb_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        (-1e9f64..1e9).prop_map(serde_json::Value::from),
        "[a-zA-Z.]{0,12}".prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(4, 32, 6, |inner| {
        let keys = prop_oneof![
            Just("display".to_owned()),
            Just("edl".to_owned()),
            Just("connectors".to_owned()),
            Just("stac".to_owned()),
            Just("name".to_owned()),
            Just("keybindings".to_owned()),
            Just("pointSize".to_owned()),
            Just("pointSizeMode".to_owned()),
            Just("minPointSize".to_owned()),
            "[a-z]{1,6}",
        ];
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(serde_json::Value::Array),
            proptest::collection::btree_map(keys, inner, 0..4)
                .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
        ]
    })
}
