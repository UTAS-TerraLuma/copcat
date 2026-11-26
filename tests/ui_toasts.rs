//! Messages as toasts in the real viewer, driven by egui_kittest: they float
//! over the main area without moving anything, information goes by itself
//! as egui's clock is stepped, and warnings stay until dismissed.
//!
//! Settings are read from scratch directories, never the real home. Needs a
//! GPU adapter, as `ui_kittest` does; prints SKIPPED without one unless
//! `COPCAT_REQUIRE_GPU` is set.

mod common;

use std::fs;
use std::path::Path;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

/// Only stops a hang: the fixture opens in well under a second.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// One viewer, so one wgpu device, at a time: see `docs/known-issues.md`.
static ONE_VIEWER_AT_A_TIME: Mutex<()> = Mutex::new(());

fn has_adapter() -> bool {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
}

/// A user settings file binding F9 to Save current rendering settings as defaults
/// (which says "Saved …") and F8 to Open workspace settings (which warns,
/// as there's no workspace).
fn write_settings(user_dir: &Path) {
    fs::create_dir_all(user_dir).unwrap();
    fs::write(
        user_dir.join("settings.json"),
        r#"{ "keybindings": {
              "settings.save_display_defaults": "F9",
              "settings.open_workspace": "F8"
           } }"#,
    )
    .unwrap();
}

/// The viewer reading settings from `user_dir`, with no workspace, opening
/// `source` if given, and a channel that hears its repaint requests.
fn viewer(
    user_dir: &Path,
    source: Option<String>,
) -> (
    Harness<'static, impl eframe::App + use<>>,
    mpsc::Receiver<()>,
) {
    let locations = copcat::ConfigLocations {
        user_dir: Some(user_dir.to_path_buf()),
        workspace: None,
    };
    let (wake, woken) = mpsc::channel();
    let harness = Harness::builder()
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
    (harness, woken)
}

fn labels(harness: &Harness<'_, impl eframe::App>) -> Vec<String> {
    harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .filter_map(|node| node.value())
        .collect()
}

fn saved_toasts(harness: &Harness<'_, impl eframe::App>) -> usize {
    labels(harness)
        .iter()
        .filter(|text| text.starts_with("Saved the rendering settings"))
        .count()
}

/// Steps until the fixture is open and every node it wants has loaded.
fn wait_for_fixture(harness: &mut Harness<'_, impl eframe::App>, woken: &mpsc::Receiver<()>) {
    let started = Instant::now();
    loop {
        harness.step();
        let labels = labels(harness);
        let drawn = labels.iter().any(|text| text.ends_with("points drawn"));
        let loading = labels.iter().any(|text| text.starts_with("Loading"));
        if drawn && !loading && harness.query_by_label("3D view").is_some() {
            harness.run_ok();
            return;
        }
        let left = LOAD_TIMEOUT.saturating_sub(started.elapsed());
        assert!(!left.is_zero(), "the fixture didn't open");
        let _ = woken.recv_timeout(left);
    }
}

/// Where the layout's pieces are: the 3D view, the settings panel, and the
/// status bar's text (only its corner, as the count can change).
fn layout(harness: &Harness<'_, impl eframe::App>) -> (egui::Rect, egui::Rect, egui::Pos2) {
    let view = harness.get_by_label("3D view").rect();
    let panel =
        egui::containers::panel::PanelState::load(&harness.ctx, egui::Id::new("point_cloud"))
            .expect("the settings panel")
            .outer_rect;
    let status = harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .find(|node| {
            node.value()
                .is_some_and(|text| text.ends_with("points drawn"))
        })
        .expect("the status bar")
        .rect()
        .left_top();
    (view, panel, status)
}

/// Steps egui's clock on by `seconds`, a frame (a quarter second) at a time.
fn wait(harness: &mut Harness<'_, impl eframe::App>, seconds: f64) {
    let now = |harness: &Harness<'_, _>| harness.ctx.input(|input| input.time);
    let until = now(harness) + seconds - 0.01;
    while now(harness) < until {
        harness.step();
    }
}

#[test]
fn a_toast_should_float_over_the_view_without_moving_anything_and_go() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("a_toast_should_float_over_the_view");
    let user_dir = scratch.join("home");
    write_settings(&user_dir);
    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (mut harness, woken) = viewer(&user_dir, Some(fixture));
    wait_for_fixture(&mut harness, &woken);
    let before = layout(&harness);

    harness.key_press(egui::Key::F9);
    harness.run_ok();
    assert_eq!(saved_toasts(&harness), 1, "{:#?}", labels(&harness));
    assert_eq!(layout(&harness), before, "a toast mustn't move the layout");
    // Over the bottom-left of the 3D view, above the status bar.
    let (view, _, status) = before;
    let toast = harness
        .get_by_label_contains("Saved the rendering settings")
        .rect();
    assert!(view.contains_rect(toast), "{toast:?} in {view:?}");
    assert!(toast.bottom() < status.y && toast.left() - view.left() < 20.0);

    // It goes by itself after about four seconds.
    wait(&mut harness, 3.0);
    assert_eq!(saved_toasts(&harness), 1, "still there after 3 s");
    wait(&mut harness, 2.0);
    assert_eq!(saved_toasts(&harness), 0, "gone after 5 s");
    harness.run_ok();
    assert_eq!(layout(&harness), before, "nor its going");
}

#[test]
fn a_hovered_toast_should_stay_and_a_warning_until_dismissed() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("a_hovered_toast_should_stay");
    let user_dir = scratch.join("home");
    write_settings(&user_dir);
    let (mut harness, _woken) = viewer(&user_dir, None);
    harness.run_ok();

    harness.key_press(egui::Key::F8);
    harness.run_ok();
    harness.key_press(egui::Key::F9);
    harness.run_ok();
    let warning = "Settings: there's no workspace folder";
    assert!(harness.query_by_label(warning).is_some());
    assert_eq!(saved_toasts(&harness), 1);
    // The newest is nearest the status bar.
    let saved = harness.get_by_label_contains("Saved the rendering").rect();
    assert!(harness.get_by_label(warning).rect().bottom() <= saved.top());

    // Hovered, the information stays.
    harness.hover_at(saved.center());
    wait(&mut harness, 10.0);
    assert_eq!(saved_toasts(&harness), 1, "paused while hovered");
    harness.hover_at(egui::pos2(480.0, 200.0));
    wait(&mut harness, 5.0);
    assert_eq!(saved_toasts(&harness), 0, "gone once the pointer left");

    // The warning stays until its own close button is clicked.
    assert!(harness.query_by_label(warning).is_some());
    harness.get_by_label("Dismiss").click();
    harness.run_ok();
    assert!(harness.query_by_label(warning).is_none());
}

#[test]
fn many_messages_should_show_the_newest_four() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scratch = common::scratch_dir("many_messages_should_show_the_newest_four");
    let user_dir = scratch.join("home");
    write_settings(&user_dir);
    let (mut harness, _woken) = viewer(&user_dir, None);
    harness.run_ok();
    for _ in 0..6 {
        harness.key_press(egui::Key::F8);
        harness.run_ok();
    }
    let warning = "Settings: there's no workspace folder";
    assert_eq!(harness.query_all_by_label(warning).count(), 4);
    assert!(harness.query_by_label("+2 more").is_some());
    assert_eq!(harness.query_all_by_label("Dismiss").count(), 4);
}
