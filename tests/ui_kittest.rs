//! The real viewer app driven headlessly by egui_kittest: launched with the
//! fixture, waited on until every node it wants is drawn, read through its
//! AccessKit labels and snapshotted with the 3D view in the picture.
//!
//! Needs a GPU adapter, real or software (egui_kittest prefers software
//! ones: WARP on Windows, lavapipe on Linux). Without one the test prints
//! SKIPPED and passes, unless `COPCAT_REQUIRE_GPU` is set.
//!
//! Snapshots live in `tests/snapshots/`. Update them with
//! `UPDATE_SNAPSHOTS=1 cargo test --test ui_kittest`; on a mismatch egui_kittest
//! writes `*.new.png` and `*.diff.png` next to the snapshot.

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

/// How long loading the 70 KB fixture from disk may take before the test
/// gives up. It normally takes well under a second; this only stops a hang.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

fn has_adapter() -> bool {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
}

/// The label in the row after the one labelled `name`, in the side panel's
/// two-column grids: its text and where it is.
fn value_after(
    harness: &Harness<'_, impl eframe::App>,
    name: &str,
) -> Option<(String, egui::Rect)> {
    let labels: Vec<_> = harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .collect();
    let at = labels
        .iter()
        .position(|node| node.value().as_deref() == Some(name))?;
    let value = labels.get(at + 1)?;
    Some((value.value()?, value.rect()))
}

fn text_after(harness: &Harness<'_, impl eframe::App>, name: &str) -> Option<String> {
    value_after(harness, name).map(|(text, _)| text)
}

#[test]
fn viewer_should_load_the_fixture_and_draw_it() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let fixture = common::fixture().to_str().unwrap().to_owned();
    // The loader thread wakes egui when it has news; turn that into a
    // channel so the test blocks on it instead of polling or sleeping.
    let (wake, woken) = mpsc::channel();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(960.0, 600.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| {
            cc.egui_ctx.set_request_repaint_callback(move |_| {
                let _ = wake.send(());
            });
            copcat::viewer_app(cc, Some(fixture), copcat::HeightSettings::default())
                .expect("the viewer needs the wgpu renderer")
        });

    // Done when every selected node is on the GPU and nothing is downloading.
    let started = Instant::now();
    loop {
        harness.step();
        // The streaming numbers are in the Diagnostics section, which starts
        // collapsed: open it once the cloud is open and the section is there.
        if text_after(&harness, "Nodes drawn").is_none()
            && let Some(header) = harness.query_by_label("Diagnostics")
        {
            header.scroll_to_me();
            harness.step();
            harness.get_by_label("Diagnostics").click();
            harness.step();
        }
        let drawn = text_after(&harness, "Nodes drawn");
        let requested = text_after(&harness, "Requested");
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
        // Block until something (the loader, or egui itself) asks for a repaint.
        let _ = woken.recv_timeout(left);
    }
    // One more frame so the last node is in the picture.
    harness.run_ok();

    assert_eq!(
        text_after(&harness, "Source").as_deref(),
        Some("fixture.copc.laz")
    );
    assert_eq!(
        text_after(&harness, "Total points").as_deref(),
        Some("7.7 k")
    );
    assert_eq!(
        text_after(&harness, "Extent (m)").as_deref(),
        Some("100.0 × 60.0 × 25.0")
    );
    // Drive the UI through AccessKit: colour by class, then look straight down.
    // The settings panel scrolls, so bring each control into view first.
    for label in ["Class", "Top"] {
        harness.get_by_label(label).scroll_to_me();
        harness.run_ok();
        harness.get_by_label(label).click();
        harness.run_ok();
    }
    harness.remove_cursor();
    harness.run_ok();
    assert_eq!(
        harness.get_by_label("Class").accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True),
        "the Class colour mode should be selected"
    );

    // The open time changes every run; hide it from the snapshot.
    let (_, open_time) = value_after(&harness, "Open time").expect("an Open time row");
    harness.mask(open_time);
    // So is the fixture's absolute path, wherever it's shown in full.
    let fixture = common::fixture().to_str().unwrap().to_owned();
    let path_boxes: Vec<_> = harness
        .query_all_by_value(&fixture)
        .map(|node| node.rect())
        .collect();
    for rect in path_boxes {
        harness.mask(rect);
    }

    harness.snapshot("viewer_fixture_top_class");
}
