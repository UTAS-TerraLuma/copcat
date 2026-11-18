//! The command palette in the real viewer app, driven headlessly by
//! egui_kittest through key presses, typed text and AccessKit labels.
//!
//! Needs a GPU adapter, real or software, as `ui_kittest.rs` does. Without one
//! the tests print SKIPPED and pass, unless `COPCAT_REQUIRE_GPU` is set.

mod common;

use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Key, Modifiers, accesskit::Role, accesskit::Toggled};
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

/// How long opening the 70 KB fixture may take before the test gives up. It
/// normally takes well under a second; this only stops a hang.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// Held by each test while its viewer is alive, so only one at a time has a
/// GPU device: two egui_kittest wgpu harnesses on parallel test threads
/// crashed now and then (an access violation) on Windows with WARP.
static ONE_VIEWER_AT_A_TIME: Mutex<()> = Mutex::new(());

fn has_adapter() -> bool {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
}

/// The viewer with nothing open, and a channel that hears each repaint
/// request, so a test can block on the loader instead of sleeping.
fn idle_viewer() -> (Harness<'static, impl eframe::App>, mpsc::Receiver<()>) {
    let (wake, woken) = mpsc::channel();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(960.0, 600.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| {
            cc.egui_ctx.set_request_repaint_callback(move |_| {
                let _ = wake.send(());
            });
            copcat::viewer_app(cc, None, copcat::HeightSettings::default())
                .expect("the viewer needs the wgpu renderer")
        });
    harness.run_ok();
    (harness, woken)
}

fn fixture_path() -> String {
    common::fixture().to_str().unwrap().to_owned()
}

/// The label in the row after the one labelled `name`, in the side panel's
/// two-column grids.
fn text_after(harness: &Harness<'_, impl eframe::App>, name: &str) -> Option<String> {
    let labels: Vec<_> = harness.query_all_by_role(Role::Label).collect();
    let at = labels
        .iter()
        .position(|node| node.value().as_deref() == Some(name))?;
    labels.get(at + 1)?.value()
}

/// Blocks until the fixture's summary shows, which it does once it's open.
fn wait_for_fixture(harness: &mut Harness<'_, impl eframe::App>, woken: &mpsc::Receiver<()>) {
    let started = Instant::now();
    loop {
        harness.step();
        if text_after(harness, "Source").as_deref() == Some("fixture.copc.laz") {
            break;
        }
        let left = LOAD_TIMEOUT.saturating_sub(started.elapsed());
        assert!(!left.is_zero(), "the fixture didn't open");
        let _ = woken.recv_timeout(left);
    }
    harness.run_ok();
}

fn press(harness: &mut Harness<'_, impl eframe::App>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_ok();
}

fn command_palette(harness: &mut Harness<'_, impl eframe::App>) {
    press(harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
}

fn quick_open(harness: &mut Harness<'_, impl eframe::App>) {
    press(harness, Modifiers::COMMAND, Key::P);
}

/// Types `text` into the palette's field, which has the keyboard focus.
fn type_text(harness: &mut Harness<'_, impl eframe::App>, text: &str) {
    harness.event(egui::Event::Text(text.to_owned()));
    harness.run_ok();
}

/// The palette's field: the text input with the focus.
fn palette_text(harness: &Harness<'_, impl eframe::App>) -> Option<String> {
    harness
        .query_all_by_role(Role::TextInput)
        .find(|node| node.is_focused())
        .and_then(|node| node.value())
}

/// Whether the palette has a row titled `title`, ignoring the shortcut or
/// detail after it.
fn has_row(harness: &Harness<'_, impl eframe::App>, title: &str) -> bool {
    row_enabled(harness, title).is_some()
}

/// Whether the row titled `title` can be chosen, if there is one.
fn row_enabled(harness: &Harness<'_, impl eframe::App>, title: &str) -> Option<bool> {
    harness
        .query_all_by_role(Role::Button)
        .find(|node| {
            node.accesskit_node()
                .label()
                .is_some_and(|label| label == title || label.starts_with(&format!("{title} ")))
        })
        .map(|node| !node.accesskit_node().is_disabled())
}

fn is_selected(harness: &Harness<'_, impl eframe::App>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().toggled() == Some(Toggled::True)
}

#[test]
fn palette_should_run_commands_and_open_a_typed_path() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut harness, woken) = idle_viewer();

    // Command mode lists commands, filtered by what's typed, greying out
    // what can't run with nothing open.
    command_palette(&mut harness);
    assert_eq!(palette_text(&harness).as_deref(), Some(">"));
    assert!(has_row(&harness, "Toggle diagnostics"));
    type_text(&mut harness, "subcloud");
    assert_eq!(palette_text(&harness).as_deref(), Some(">subcloud"));
    assert_eq!(row_enabled(&harness, "Return to the subcloud"), Some(false));
    assert!(!has_row(&harness, "Toggle diagnostics"));
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(
        !has_row(&harness, "Return to the subcloud"),
        "Esc should close it"
    );

    // Open mode opens a path typed into it.
    let fixture = fixture_path();
    quick_open(&mut harness);
    assert_eq!(palette_text(&harness).as_deref(), Some(""));
    type_text(&mut harness, &fixture);
    assert_eq!(
        row_enabled(&harness, &format!("Open {fixture}")),
        Some(true)
    );
    press(&mut harness, Modifiers::NONE, Key::Enter);
    wait_for_fixture(&mut harness, &woken);

    // Keys typed into the palette aren't shortcuts: 5 would switch to
    // orthographic.
    assert!(is_selected(&harness, "Perspective"));
    command_palette(&mut harness);
    harness.key_press(Key::Num5);
    type_text(&mut harness, "5");
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(is_selected(&harness, "Perspective"));

    // Enter runs the best match.
    assert!(!is_selected(&harness, "Class"));
    command_palette(&mut harness);
    type_text(&mut harness, "class");
    assert_eq!(
        row_enabled(&harness, "Colour by classification"),
        Some(true)
    );
    press(&mut harness, Modifiers::NONE, Key::Enter);
    assert!(is_selected(&harness, "Class"));
    assert!(!has_row(&harness, "Colour by classification"));

    // Arrow keys move down the list.
    command_palette(&mut harness);
    type_text(&mut harness, "view from");
    // The frame the key arrives on must look like the one after it: the
    // field once lost the focus for that frame, so its outline flashed.
    harness.key_press(Key::ArrowDown);
    harness.step();
    let on_the_press = harness.render().expect("a frame");
    harness.step();
    let after_it = harness.render().expect("a frame");
    assert!(
        on_the_press == after_it,
        "the palette flashed on an arrow key"
    );
    harness.run_ok();
    press(&mut harness, Modifiers::NONE, Key::Enter);
    // The second view, Bottom, is axis-aligned, which switches to
    // orthographic.
    assert!(is_selected(&harness, "Orthographic"));

    // Clicking a row runs it.
    assert!(text_after(&harness, "Nodes drawn").is_none());
    command_palette(&mut harness);
    type_text(&mut harness, "diagnostics");
    harness
        .get_by(|node| {
            node.role() == Role::Button
                && node
                    .label()
                    .is_some_and(|label| label.starts_with("Toggle diagnostics"))
        })
        .click();
    harness.run_ok();
    assert!(text_after(&harness, "Nodes drawn").is_some());

    // Open mode remembers what was opened.
    quick_open(&mut harness);
    assert_eq!(row_enabled(&harness, "fixture.copc.laz"), Some(true));
    // Its shortcuts switch the open palette between modes.
    command_palette(&mut harness);
    assert_eq!(palette_text(&harness).as_deref(), Some(">"));
    assert!(!has_row(&harness, "fixture.copc.laz"));
    quick_open(&mut harness);
    assert_eq!(palette_text(&harness).as_deref(), Some(""));
    press(&mut harness, Modifiers::NONE, Key::Escape);
    assert!(palette_text(&harness).is_none());
}

/// Every palette row's label and where it is, top to bottom.
fn row_rects(harness: &Harness<'_, impl eframe::App>) -> Vec<(String, egui::Rect)> {
    let mut rows: Vec<_> = harness
        .query_all_by_role(Role::Button)
        .filter_map(|node| Some((node.accesskit_node().label()?, node.rect())))
        .collect();
    rows.sort_by(|a, b| a.1.top().total_cmp(&b.1.top()));
    rows
}

#[test]
fn palette_rows_should_not_move_as_the_selection_or_hover_moves() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut harness, woken) = idle_viewer();
    // With a cloud open, so the view commands can be selected.
    quick_open(&mut harness);
    type_text(&mut harness, &fixture_path());
    press(&mut harness, Modifiers::NONE, Key::Enter);
    wait_for_fixture(&mut harness, &woken);
    harness.remove_cursor();
    command_palette(&mut harness);
    type_text(&mut harness, "view from");
    let before = row_rects(&harness);

    // Selected and plain rows are the same height.
    let palette_rows: Vec<_> = before
        .iter()
        .filter(|(label, _)| label.starts_with("View from"))
        .collect();
    assert_eq!(row_enabled(&harness, "View from top"), Some(true));
    assert!(palette_rows.len() > 3, "{before:#?}");
    let height = palette_rows[0].1.height();
    for (label, rect) in &palette_rows {
        assert_eq!(rect.height(), height, "{label:?} is a different height");
    }

    // Moving the selection down leaves every row where it was.
    harness.key_press(Key::ArrowDown);
    harness.run_ok();
    assert_eq!(row_rects(&harness), before, "after ArrowDown");
    harness.key_press(Key::ArrowDown);
    harness.run_ok();
    assert_eq!(row_rects(&harness), before, "after a second ArrowDown");

    // So does hovering one.
    let (_, third) = palette_rows[3];
    harness.hover_at(third.center());
    harness.run_ok();
    assert_eq!(row_rects(&harness), before, "while hovering a row");
}

#[test]
fn palette_should_ask_for_open_urls_argument_itself() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (mut harness, woken) = idle_viewer();

    // `>` in open mode switches to commands.
    quick_open(&mut harness);
    type_text(&mut harness, ">open url");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    // Still open, asking for the URL in its own field.
    assert_eq!(palette_text(&harness).as_deref(), Some(""));
    assert!(harness.query_by_label("Open URL").is_some());

    let fixture = fixture_path();
    type_text(&mut harness, &fixture);
    assert!(has_row(&harness, &format!("Open URL: {fixture}")));
    press(&mut harness, Modifiers::NONE, Key::Enter);
    wait_for_fixture(&mut harness, &woken);
    assert!(palette_text(&harness).is_none());
}
