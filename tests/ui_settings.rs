//! The settings files driving the real viewer in egui_kittest: a user file
//! and a workspace file in scratch directories (never the real home), and
//! the UI showing what they say, or a warning when a file is bad.
//!
//! Needs a GPU adapter, as `ui_kittest` does; prints SKIPPED without one
//! unless `COPCAT_REQUIRE_GPU` is set.

mod common;

use std::fs;
use std::path::Path;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

/// Only stops a hang: the fixture opens in well under a second.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Held by each test while its viewer is alive, so only one at a time has a
/// GPU device: two egui_kittest wgpu harnesses on parallel test threads
/// crash now and then (an access violation) on Windows. Seen here in 2 of
/// 40 runs before this lock. See docs/known-issues.md.
static ONE_VIEWER_AT_A_TIME: Mutex<()> = Mutex::new(());

fn has_adapter() -> bool {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
}

fn write(file: &Path, text: &str) {
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

/// The viewer, reading settings from `user_dir` and `workspace`, opening
/// `source` if given. Returns it with a channel that hears its repaint
/// requests, to block on while waiting for the loader.
fn viewer(
    user_dir: &Path,
    workspace: &Path,
    source: Option<String>,
) -> (
    Harness<'static, impl eframe::App + use<>>,
    mpsc::Receiver<()>,
) {
    let locations = copcat::ConfigLocations {
        user_dir: Some(user_dir.to_path_buf()),
        workspace: Some(workspace.to_path_buf()),
    };
    let (wake, woken) = mpsc::channel();
    let harness = Harness::builder()
        .with_size(egui::vec2(960.0, 700.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(move |cc| {
            cc.egui_ctx.set_request_repaint_callback(move |_| {
                let _ = wake.send(());
            });
            copcat::viewer_app_with_settings(
                cc,
                source,
                copcat::HeightSettings::default(),
                locations,
            )
            .expect("the viewer needs the wgpu renderer")
        });
    (harness, woken)
}

/// Steps until the cloud is open and the Display section is showing.
fn wait_for_display_section(
    harness: &mut Harness<'_, impl eframe::App>,
    woken: &mpsc::Receiver<()>,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if harness.query_by_label("Eye-dome lighting").is_some() {
            // The panel's first frame with a cloud open isn't its final
            // layout; run until it settles so clicks land where widgets are.
            harness.run_ok();
            return;
        }
        let left = LOAD_TIMEOUT.saturating_sub(started.elapsed());
        assert!(!left.is_zero(), "the fixture didn't open");
        let _ = woken.recv_timeout(left);
    }
}

/// The first node with `role` labelled `label`.
fn by_role_and_label<'h>(
    harness: &'h Harness<'_, impl eframe::App>,
    role: egui::accesskit::Role,
    label: &'h str,
) -> egui_kittest::Node<'h> {
    harness
        .query_all_by_label(label)
        .find(|node| node.accesskit_node().role() == role)
        .unwrap_or_else(|| panic!("no {role:?} labelled {label}"))
}

/// The value of the number field labelled `label`.
fn field_value(harness: &Harness<'_, impl eframe::App>, label: &str) -> f64 {
    by_role_and_label(harness, egui::accesskit::Role::SpinButton, label)
        .accesskit_node()
        .numeric_value()
        .expect("a number field has a value")
}

/// Whether there's a number field labelled `label`.
fn has_field(harness: &Harness<'_, impl eframe::App>, label: &str) -> bool {
    harness
        .query_all_by_label(label)
        .any(|node| node.accesskit_node().role() == egui::accesskit::Role::SpinButton)
}

/// What the drop-down labelled `label` shows.
fn chosen(harness: &Harness<'_, impl eframe::App>, label: &str) -> String {
    by_role_and_label(harness, egui::accesskit::Role::ComboBox, label)
        .value()
        .expect("a drop-down shows its choice")
}

/// Opens the drop-down labelled `label`, picks `option` and lets the UI
/// settle.
fn choose(harness: &mut Harness<'_, impl eframe::App>, label: &str, option: &str) {
    by_role_and_label(harness, egui::accesskit::Role::ComboBox, label).click();
    harness.run_ok();
    by_role_and_label(harness, egui::accesskit::Role::Button, option).click();
    harness.run_ok();
}
/// The text of every settings warning in the settings bar.
fn warnings(harness: &Harness<'_, impl eframe::App>) -> Vec<String> {
    harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .filter_map(|node| node.value())
        .filter(|text| text.starts_with("Settings:"))
        .collect()
}

#[test]
fn settings_files_should_set_the_display_defaults() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("settings_files_should_set_the_display_defaults");
    let user_dir = scratch.join("home").join(".copcat");
    let workspace = scratch.join("project");
    write(
        &user_dir.join("settings.json"),
        r#"// The user's settings.
        {
          "display": {
            "colorMode": "classification",
            "pointSizeMode": "fixed",
            "pointSize": 4.0,
            "edl": { "strength": 2.5, "radius": 2.0 },
          },
        }"#,
    );
    // The workspace overrides one EDL value and keeps the user's other.
    write(
        &workspace.join(".copcat").join("settings.json"),
        r#"{ "display": { "edl": { "strength": 3.0 } } }"#,
    );

    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (mut harness, woken) = viewer(&user_dir, &workspace, Some(fixture));
    wait_for_display_section(&mut harness, &woken);

    assert_eq!(
        chosen(&harness, "Colour"),
        "Class",
        "colour by class from the user file"
    );
    assert_eq!(chosen(&harness, "Point size"), "Fixed");
    assert_eq!(field_value(&harness, "Size (px)"), 4.0);
    assert!(
        !has_field(&harness, "Fill"),
        "no adaptive controls when fixed"
    );
    assert_eq!(field_value(&harness, "Strength"), 3.0, "from the workspace");
    assert_eq!(
        field_value(&harness, "Radius (px)"),
        2.0,
        "from the user file"
    );
    assert_eq!(
        warnings(&harness),
        Vec::<String>::new(),
        "good files shouldn't warn"
    );
}

#[test]
fn adaptive_point_sizes_from_the_files_should_reach_the_ui() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("adaptive_point_sizes_from_the_files");
    let user_dir = scratch.join("home").join(".copcat");
    let workspace = scratch.join("project");
    // Adaptive sizes without `pointSizeMode` don't switch to adaptive.
    write(
        &user_dir.join("settings.json"),
        r#"{ "display": { "fill": 2.5, "minPointSize": 2.0, "pointSize": 7.0 } }"#,
    );
    write(
        &workspace.join(".copcat").join("settings.json"),
        r#"{ "display": { "maxPointSize": 30.0 } }"#,
    );

    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (mut harness, woken) = viewer(&user_dir, &workspace, Some(fixture));
    wait_for_display_section(&mut harness, &woken);

    assert_eq!(
        chosen(&harness, "Point size"),
        "Fixed",
        "still the default mode"
    );
    assert_eq!(
        field_value(&harness, "Size (px)"),
        7.0,
        "from the user file"
    );
    assert!(
        !has_field(&harness, "Fill"),
        "no adaptive controls when fixed"
    );

    // The files' adaptive sizes are what Adaptive then uses.
    choose(&mut harness, "Point size", "Adaptive");
    assert!(!has_field(&harness, "Size (px)"));
    assert_eq!(field_value(&harness, "Fill"), 2.5, "from the user file");
    harness.get_by_label("Advanced").click();
    harness.run_ok();
    assert_eq!(field_value(&harness, "Min (px)"), 2.0, "from the user file");
    assert_eq!(
        field_value(&harness, "Max (px)"),
        30.0,
        "from the workspace"
    );
    assert_eq!(
        warnings(&harness),
        Vec::<String>::new(),
        "good files shouldn't warn"
    );
}

#[test]
fn a_bad_settings_file_should_show_a_warning_and_keep_the_good_one() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("a_bad_settings_file_should_show_a_warning");
    let user_dir = scratch.join("home").join(".copcat");
    let workspace = scratch.join("project");
    write(
        &user_dir.join("settings.json"),
        r#"{ "display": { "colorMode": "height", "pointSize": "huge" } }"#,
    );
    write(
        &workspace.join(".copcat").join("settings.json"),
        r#"{ "display": { "edl": { "strength": 3.0 }"#,
    );

    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (mut harness, woken) = viewer(&user_dir, &workspace, Some(fixture));
    wait_for_display_section(&mut harness, &woken);

    // The user file's good value is used; the broken workspace file isn't.
    assert_eq!(chosen(&harness, "Colour"), "Height");
    assert_eq!(chosen(&harness, "Point size"), "Fixed", "the default mode");
    // The bad fixed size isn't used.
    assert_eq!(field_value(&harness, "Size (px)"), 2.0, "the default");
    assert_eq!(field_value(&harness, "Strength"), 1.0, "the default");

    let shown = warnings(&harness);
    assert_eq!(shown.len(), 2, "{shown:#?}");
    assert!(
        shown
            .iter()
            .any(|w| w.contains("pointSize should be a number")),
        "{shown:#?}"
    );
    let workspace_file = workspace.join(".copcat").join("settings.json");
    assert!(
        shown
            .iter()
            .any(|w| w.contains(&workspace_file.display().to_string())
                && w.contains("not valid JSON")),
        "{shown:#?}"
    );

    // Non-blocking: the warning goes once dismissed, and the app carries on.
    harness.get_by_label("Dismiss").click();
    harness.run_ok();
    assert_eq!(warnings(&harness), Vec::<String>::new());
    assert_eq!(chosen(&harness, "Colour"), "Height");
    choose(&mut harness, "Point size", "Adaptive");
    assert_eq!(field_value(&harness, "Fill"), 1.5, "the default");
}

#[test]
fn an_unknown_theme_and_bad_shortcut_should_warn_without_a_cloud() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("an_unknown_theme_and_bad_shortcut_should_warn");
    let user_dir = scratch.join("home");
    write(
        &user_dir.join("settings.json"),
        r#"{ "theme": "neon", "keybindings": { "file.open": "Ctrl+Banana", "file.open_url": "Ctrl+U" } }"#,
    );
    let (mut harness, _woken) = viewer(&user_dir, &scratch.join("nowhere"), None);
    harness.run_ok();

    let shown = warnings(&harness);
    assert_eq!(shown.len(), 2, "{shown:#?}");
    assert!(shown.iter().any(|w| w.contains("\"neon\"")), "{shown:#?}");
    assert!(shown.iter().any(|w| w.contains("Banana")), "{shown:#?}");
    // The good binding is shown on its button, and the bad one keeps its
    // default.
    let open_url = harness.get_by_label_contains("Open URL");
    let text = format!("{:?}", open_url.accesskit_node());
    assert!(text.contains("Ctrl+U"), "{text}");
    let open_file = harness.get_by_label_contains("Open file");
    let text = format!("{:?}", open_file.accesskit_node());
    assert!(text.contains("Ctrl+O"), "{text}");
}

#[test]
fn save_and_reload_should_round_trip_through_the_user_file() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("save_and_reload_should_round_trip");
    let user_dir = scratch.join("home");
    let user_file = user_dir.join("settings.json");
    // The settings commands are bound to keys here, as there's no palette
    // to run them from in a test.
    write(
        &user_file,
        r#"{
          "theme": "graphite",
          "display": { "pointSizeMode": "fixed", "pointSize": 3.0, "fill": 2.0 },
          "keybindings": {
            "settings.save_display_defaults": "F9",
            "settings.reload": "F10",
            "display.color_height": "H"
          },
          "connectors": { "stac": [{ "name": "S", "url": "https://stac.example/" }] }
        }"#,
    );
    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"), Some(fixture));
    wait_for_display_section(&mut harness, &woken);
    assert_eq!(chosen(&harness, "Point size"), "Fixed");
    assert_eq!(field_value(&harness, "Size (px)"), 3.0);

    // Changes in the window, saved as the default.
    harness.key_press(egui::Key::H);
    harness.run_ok();
    assert_eq!(chosen(&harness, "Colour"), "Height");
    choose(&mut harness, "Point size", "Adaptive");
    assert_eq!(field_value(&harness, "Fill"), 2.0, "from the file");
    harness.key_press(egui::Key::F9);
    harness.run_ok();

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&user_file).unwrap()).unwrap();
    assert_eq!(saved["display"]["colorMode"], "height");
    assert_eq!(saved["display"]["pointSizeMode"], "adaptive");
    assert_eq!(saved["display"]["pointSize"], 3.0, "kept for fixed mode");
    assert_eq!(saved["display"]["fill"], 2.0);
    assert_eq!(saved["display"]["minPointSize"], 1.0);
    assert_eq!(saved["display"]["maxPointSize"], 20.0);
    assert_eq!(saved["display"]["edl"]["strength"], 1.0);
    assert_eq!(saved["performance"]["pointBudget"], 5_000_000);
    // Everything else in the file is kept.
    assert_eq!(saved["theme"], "graphite");
    assert_eq!(saved["keybindings"]["settings.reload"], "F10");
    assert_eq!(saved["connectors"]["stac"][0]["name"], "S");
    assert!(
        harness
            .query_all_by_role(egui::accesskit::Role::Label)
            .filter_map(|node| node.value())
            .any(|text| text.starts_with("Saved the display settings")),
        "saving should say so"
    );

    // An edit to the file shows once reloaded.
    let edited = fs::read_to_string(&user_file)
        .unwrap()
        .replace("\"pointSize\": 3.0", "\"pointSize\": 6.0")
        .replace(
            "\"pointSizeMode\": \"adaptive\"",
            "\"pointSizeMode\": \"fixed\"",
        );
    fs::write(&user_file, edited).unwrap();
    harness.key_press(egui::Key::F10);
    harness.run_ok();
    assert_eq!(chosen(&harness, "Point size"), "Fixed", "the edited mode");
    assert_eq!(field_value(&harness, "Size (px)"), 6.0);
    assert_eq!(
        chosen(&harness, "Colour"),
        "Height",
        "the saved colour mode"
    );
    assert_eq!(warnings(&harness), Vec::<String>::new());
}

#[test]
fn number_fields_should_step_take_typed_values_and_clamp() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("number_fields_should_step");
    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (mut harness, woken) = viewer(
        &scratch.join("home"),
        &scratch.join("nowhere"),
        Some(fixture),
    );
    wait_for_display_section(&mut harness, &woken);
    // Size (px) is 2 by default, from 1 to 10 in steps of 0.5.
    assert_eq!(field_value(&harness, "Size (px)"), 2.0);
    click(&mut harness, "Increase Size (px)");
    assert_eq!(field_value(&harness, "Size (px)"), 2.5);
    for _ in 0..4 {
        click(&mut harness, "Decrease Size (px)");
    }
    assert_eq!(
        field_value(&harness, "Size (px)"),
        1.0,
        "stops at the minimum"
    );

    // Click to type, Enter to keep: a value past the end is clamped.
    type_into(&mut harness, "Size (px)", "4.5", egui::Key::Enter);
    assert_eq!(field_value(&harness, "Size (px)"), 4.5);
    type_into(&mut harness, "Size (px)", "50", egui::Key::Enter);
    assert_eq!(
        field_value(&harness, "Size (px)"),
        10.0,
        "clamped to the maximum"
    );
    // Esc leaves the value as it was.
    type_into(&mut harness, "Size (px)", "3", egui::Key::Escape);
    assert_eq!(field_value(&harness, "Size (px)"), 10.0);
    click(&mut harness, "Increase Size (px)");
    assert_eq!(
        field_value(&harness, "Size (px)"),
        10.0,
        "stops at the maximum"
    );

    // The point budget reads counts as it shows them.
    harness.get_by_label("Performance").scroll_to_me();
    harness.run_ok();
    click(&mut harness, "Performance");
    type_into(&mut harness, "Point budget", "750 k", egui::Key::Enter);
    assert_eq!(field_value(&harness, "Point budget"), 750_000.0);
}

/// Clicks what's labelled `label` and lets the UI settle.
fn click(harness: &mut Harness<'_, impl eframe::App>, label: &str) {
    harness.get_by_label(label).click();
    harness.run_ok();
}

/// Clicks the number field labelled `label` to type into it, types `text`
/// and presses `key`.
fn type_into(harness: &mut Harness<'_, impl eframe::App>, label: &str, text: &str, key: egui::Key) {
    by_role_and_label(harness, egui::accesskit::Role::SpinButton, label).click();
    harness.run_ok();
    // Still a spin button while typing, now focused.
    let field = by_role_and_label(harness, egui::accesskit::Role::SpinButton, label);
    assert!(field.is_focused(), "clicking {label} should start typing");
    field.type_text(text);
    harness.run_ok();
    harness.key_press(key);
    harness.run_ok();
}
