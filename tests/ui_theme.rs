//! Themes in the real viewer, driven by egui_kittest: the theme selector
//! previewing, restoring and saving a theme, a settings file starting the
//! viewer in Graphite Light, and a snapshot of it with the fixture open.
//!
//! Settings are read from scratch directories, never the real home. Needs a
//! GPU adapter, as `ui_kittest` does; prints SKIPPED without one unless
//! `COPCAT_REQUIRE_GPU` is set. Update the snapshot with
//! `UPDATE_SNAPSHOTS=1 cargo test --test ui_theme`.

mod common;

use std::fs;
use std::path::Path;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, accesskit::Role};
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

/// Only stops a hang: the fixture opens in well under a second.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Held by each test while its viewer is alive, so only one at a time has a
/// GPU device: two egui_kittest wgpu harnesses on parallel test threads
/// crash now and then on Windows. See docs/known-issues.md.
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
/// `source` if given, with a channel that hears its repaint requests.
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
    let mut harness = Harness::builder()
        .with_size(egui::vec2(960.0, 600.0))
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
    harness.run_ok();
    (harness, woken)
}

fn press(harness: &mut Harness<'_, impl eframe::App>, key: Key) {
    harness.key_press(key);
    harness.run_ok();
}

/// Opens the command palette and runs Select theme… from it.
fn open_theme_selector(harness: &mut Harness<'_, impl eframe::App>) {
    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    harness.run_ok();
    harness.event(egui::Event::Text("select theme".to_owned()));
    harness.run_ok();
    press(harness, Key::Enter);
    assert!(
        harness.query_by_label("Select theme").is_some(),
        "the theme list should show"
    );
}

/// The palette row titled `title`, ignoring its detail: its full label.
fn row_label(harness: &Harness<'_, impl eframe::App>, title: &str) -> Option<String> {
    harness
        .query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .find(|label| label == title || label.starts_with(&format!("{title} ")))
}

/// A left click at `pos`, on nothing in particular.
fn click_at(harness: &mut Harness<'_, impl eframe::App>, pos: egui::Pos2) {
    harness.hover_at(pos);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    harness.run_ok();
}

fn is_dark(harness: &Harness<'_, impl eframe::App>) -> bool {
    harness.ctx.global_style().visuals.dark_mode
}

/// The panels' fill, as the theme applied it.
fn panel_fill(harness: &Harness<'_, impl eframe::App>) -> egui::Color32 {
    harness.ctx.global_style().visuals.window_fill
}

/// The text of every message in the settings bar.
fn messages(harness: &Harness<'_, impl eframe::App>) -> Vec<String> {
    harness
        .query_all_by_role(Role::Label)
        .filter_map(|node| node.value())
        .filter(|text| text.starts_with("Saved the theme") || text.starts_with("Settings:"))
        .collect()
}

fn saved_theme(file: &Path) -> serde_json::Value {
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
    saved["theme"].clone()
}

#[test]
fn theme_selector_should_preview_restore_and_save_a_theme() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("theme_selector_should_preview_restore_and_save");
    let user_dir = scratch.join("home");
    let user_file = user_dir.join("settings.json");
    // Something else in the file, to check saving keeps it.
    write(&user_file, r#"{ "display": { "colorMode": "height" } }"#);
    let workspace = scratch.join("nowhere");

    let (mut harness, _woken) = viewer(&user_dir, &workspace, None);
    assert!(is_dark(&harness), "Graphite Dark is the default");
    let dark_fill = panel_fill(&harness);

    // The list starts on the theme showing, marked current.
    open_theme_selector(&mut harness);
    assert_eq!(
        row_label(&harness, "Graphite Dark").as_deref(),
        Some("Graphite Dark current")
    );
    assert_eq!(
        row_label(&harness, "Graphite Light").as_deref(),
        Some("Graphite Light")
    );
    assert!(is_dark(&harness));

    // Down previews Graphite Light; Esc puts Graphite Dark back.
    press(&mut harness, Key::ArrowDown);
    assert!(!is_dark(&harness), "the selected theme should show");
    assert_ne!(panel_fill(&harness), dark_fill);
    press(&mut harness, Key::Escape);
    assert!(is_dark(&harness), "Esc should restore Graphite Dark");
    assert_eq!(panel_fill(&harness), dark_fill);
    assert!(harness.query_by_label("Select theme").is_none());

    // So does clicking away.
    open_theme_selector(&mut harness);
    press(&mut harness, Key::ArrowDown);
    assert!(!is_dark(&harness));
    click_at(&mut harness, egui::pos2(20.0, 560.0));
    assert!(
        is_dark(&harness),
        "clicking away should restore Graphite Dark"
    );
    // Nothing was saved yet.
    assert_eq!(saved_theme(&user_file), serde_json::Value::Null);

    // Enter keeps Graphite Light and saves it, keeping the rest of the file.
    open_theme_selector(&mut harness);
    press(&mut harness, Key::ArrowDown);
    press(&mut harness, Key::Enter);
    assert!(!is_dark(&harness), "Enter should keep Graphite Light");
    assert!(harness.query_by_label("Select theme").is_none());
    assert_eq!(saved_theme(&user_file), "graphite-light");
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&user_file).unwrap()).unwrap();
    assert_eq!(saved["display"]["colorMode"], "height");
    let shown = messages(&harness);
    assert!(
        shown.len() == 1 && shown[0].starts_with("Saved the theme Graphite Light to "),
        "{shown:#?}"
    );
    let light_fill = panel_fill(&harness);

    // The selector now marks Graphite Light current, and Esc keeps it.
    open_theme_selector(&mut harness);
    assert_eq!(
        row_label(&harness, "Graphite Light").as_deref(),
        Some("Graphite Light current")
    );
    press(&mut harness, Key::ArrowUp);
    assert!(is_dark(&harness));
    press(&mut harness, Key::Escape);
    assert!(!is_dark(&harness));
    drop(harness);

    // A viewer started with that file starts in Graphite Light.
    let (harness, _woken) = viewer(&user_dir, &workspace, None);
    assert!(!is_dark(&harness), "the saved theme should load");
    assert_eq!(panel_fill(&harness), light_fill);
    assert!(messages(&harness).is_empty(), "{:#?}", messages(&harness));
}

#[test]
fn a_workspace_theme_should_win_when_settings_are_next_loaded() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("a_workspace_theme_should_win");
    let user_dir = scratch.join("home");
    let user_file = user_dir.join("settings.json");
    // Commented, as the template is, and with a shortcut to reload by.
    write(
        &user_file,
        "// Mine.\n{ \"theme\": \"graphite\", \"keybindings\": { \"settings.reload\": \"F10\" } }",
    );
    let workspace = scratch.join("project");
    write(
        &workspace.join(".copcat").join("settings.json"),
        r#"{ "theme": "graphite-dark" }"#,
    );

    let (mut harness, _woken) = viewer(&user_dir, &workspace, None);
    assert!(is_dark(&harness));
    open_theme_selector(&mut harness);
    press(&mut harness, Key::ArrowDown);
    press(&mut harness, Key::Enter);

    // Chosen for this session, saved, and the override pointed out.
    assert!(!is_dark(&harness));
    assert_eq!(saved_theme(&user_file), "graphite-light");
    let shown = messages(&harness);
    assert_eq!(shown.len(), 1, "{shown:#?}");
    assert!(
        shown[0].contains("(its comments were removed)")
            && shown[0].contains("The workspace settings file sets a theme too"),
        "{shown:#?}"
    );

    // Reloading reads the workspace's theme over the user's.
    press(&mut harness, Key::F10);
    assert!(
        is_dark(&harness),
        "the workspace's theme should win on reload"
    );
}

#[test]
fn an_unknown_theme_in_a_settings_file_should_warn_and_stay_dark() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("an_unknown_theme_in_a_settings_file");
    let user_dir = scratch.join("home");
    write(
        &user_dir.join("settings.json"),
        r#"{ "theme": "graphite-sepia" }"#,
    );
    let (harness, _woken) = viewer(&user_dir, &scratch.join("nowhere"), None);
    assert!(is_dark(&harness));
    let shown = messages(&harness);
    assert_eq!(shown.len(), 1, "{shown:#?}");
    assert!(
        shown[0].contains("\"graphite-sepia\"") && shown[0].contains("\"graphite-light\""),
        "{shown:#?}"
    );
}

/// The label in the row after the one labelled `name`: its text and rect.
fn value_after(
    harness: &Harness<'_, impl eframe::App>,
    name: &str,
) -> Option<(String, egui::Rect)> {
    let labels: Vec<_> = harness.query_all_by_role(Role::Label).collect();
    let at = labels
        .iter()
        .position(|node| node.value().as_deref() == Some(name))?;
    let value = labels.get(at + 1)?;
    Some((value.value()?, value.rect()))
}

fn text_after(harness: &Harness<'_, impl eframe::App>, name: &str) -> Option<String> {
    value_after(harness, name).map(|(text, _)| text)
}

/// Steps until every node the view wants is drawn, opening the Diagnostics
/// section to read the streaming numbers.
fn wait_until_loaded(harness: &mut Harness<'_, impl eframe::App>, woken: &mpsc::Receiver<()>) {
    let started = Instant::now();
    loop {
        harness.step();
        if text_after(harness, "Nodes drawn").is_none()
            && let Some(header) = harness.query_by_label("Diagnostics")
        {
            header.scroll_to_me();
            harness.step();
            harness.get_by_label("Diagnostics").click();
            harness.step();
        }
        let drawn = text_after(harness, "Nodes drawn");
        let requested = text_after(harness, "Requested");
        if let (Some(drawn), Some(requested)) = (&drawn, &requested) {
            let mut numbers = drawn
                .split_whitespace()
                .filter_map(|w| w.parse::<u32>().ok());
            let all_drawn =
                matches!((numbers.next(), numbers.next()), (Some(d), Some(s)) if d == s && s > 0);
            if all_drawn && requested.ends_with("(0 downloading)") {
                break;
            }
        }
        let left = LOAD_TIMEOUT.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "the fixture didn't finish loading: nodes drawn {drawn:?}, requested {requested:?}"
        );
        let _ = woken.recv_timeout(left);
    }
    harness.run_ok();
}

/// Hides what changes between runs: the open time and the fixture's path.
fn mask_run_specifics(harness: &mut Harness<'_, impl eframe::App>) {
    let mut masks: Vec<egui::Rect> = harness
        .query_all_by_value(common::fixture().to_str().unwrap())
        .map(|node| node.rect())
        .collect();
    masks.extend(value_after(harness, "Open time").map(|(_, rect)| rect));
    for rect in masks {
        common::mask_shown(harness, rect);
    }
}

#[test]
fn viewer_should_draw_the_fixture_in_graphite_light() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("viewer_should_draw_the_fixture_in_graphite_light");
    let user_dir = scratch.join("home");
    write(
        &user_dir.join("settings.json"),
        r#"{ "theme": "graphite-light" }"#,
    );
    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"), Some(fixture));
    wait_until_loaded(&mut harness, &woken);
    assert!(!is_dark(&harness));
    // The default view and colours: RGB points with eye-dome lighting, seen
    // from the south, on the light viewport.
    harness.remove_cursor();
    harness.run_ok();
    mask_run_specifics(&mut harness);
    harness.snapshot("viewer_fixture_graphite_light");
}
