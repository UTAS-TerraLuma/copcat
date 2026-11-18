//! The status bar with the real viewer in egui_kittest: nodes that fail to
//! load are reported there, and Retry loads them.
//!
//! Needs a GPU adapter, like `ui_kittest.rs`; without one the test prints
//! SKIPPED and passes, unless `COPCAT_REQUIRE_GPU` is set.

mod common;

use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;

use common::server::FileServer;

/// Only stops a hang; loading the fixture normally takes well under a second.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// One viewer, so one wgpu device, at a time: see `docs/known-issues.md`.
static ONE_VIEWER_AT_A_TIME: Mutex<()> = Mutex::new(());

fn has_adapter() -> bool {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
}

fn labels(harness: &Harness<'_, impl eframe::App>) -> Vec<String> {
    harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .filter_map(|node| node.value())
        .collect()
}

/// The text of the label after the one reading `name`, as in the
/// Diagnostics section's two-column grid.
fn text_after(harness: &Harness<'_, impl eframe::App>, name: &str) -> Option<String> {
    let labels = labels(harness);
    let at = labels.iter().position(|label| label == name)?;
    labels.get(at + 1).cloned()
}

/// The status bar's "N nodes failed", if it's showing.
fn failed_text(harness: &Harness<'_, impl eframe::App>) -> Option<String> {
    labels(harness)
        .into_iter()
        .find(|label| label.ends_with(" failed") && label.contains(" node"))
}

fn is_loading(harness: &Harness<'_, impl eframe::App>) -> bool {
    labels(harness)
        .iter()
        .any(|label| label.starts_with("Loading "))
}

/// Where the fixture's node data is: after the header and VLRs, before the
/// EVLRs (which hold the hierarchy). Read from the LAS 1.4 header.
fn node_bytes() -> std::ops::Range<u64> {
    let bytes = std::fs::read(common::fixture()).unwrap();
    let point_data = u32::from_le_bytes(bytes[96..100].try_into().unwrap());
    let first_evlr = u64::from_le_bytes(bytes[235..243].try_into().unwrap());
    u64::from(point_data)..first_evlr
}

#[test]
fn status_bar_should_report_failed_nodes_and_retry_them() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let _gpu = ONE_VIEWER_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = FileServer::serve(common::data_dir());
    // The header and hierarchy load; every node fails.
    let nodes = node_bytes();
    server.fail_ranges(Some(nodes.clone()));
    let url = server.url("fixture.copc.laz");

    let (wake, woken) = mpsc::channel();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(960.0, 600.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| {
            cc.egui_ctx.set_request_repaint_callback(move |_| {
                let _ = wake.send(());
            });
            copcat::viewer_app(cc, Some(url), copcat::HeightSettings::default())
                .expect("the viewer needs the wgpu renderer")
        });
    // Blocks until the loader (or egui) asks for a repaint, or gives up.
    let started = Instant::now();
    let wait = |what: &str| {
        let left = LOAD_TIMEOUT.saturating_sub(started.elapsed());
        assert!(!left.is_zero(), "timed out waiting for {what}");
        let _ = woken.recv_timeout(left);
    };

    // Open Diagnostics, for the error text, then wait for the failures.
    loop {
        harness.step();
        if text_after(&harness, "Nodes drawn").is_none()
            && let Some(header) = harness.query_by_label("Diagnostics")
        {
            header.scroll_to_me();
            harness.step();
            harness.get_by_label("Diagnostics").click();
            harness.step();
        }
        let diagnostics_open = text_after(&harness, "Nodes drawn").is_some();
        if diagnostics_open && failed_text(&harness).is_some() && !is_loading(&harness) {
            break;
        }
        wait("nodes to fail");
    }
    let error = text_after(&harness, "Last error").expect("a Last error row in Diagnostics");
    assert!(error.contains("503"), "{error}");

    // The server recovers; Retry asks again.
    server.fail_ranges(None);
    harness.get_by_label("Retry").click();
    loop {
        harness.step();
        let drawn = text_after(&harness, "Nodes drawn");
        let requested = text_after(&harness, "Requested");
        if let (Some(drawn), Some(requested)) = (&drawn, &requested) {
            let mut numbers = drawn
                .split_whitespace()
                .filter_map(|w| w.parse::<u32>().ok());
            let all_drawn =
                matches!((numbers.next(), numbers.next()), (Some(d), Some(s)) if d == s && s > 0);
            if all_drawn && requested.starts_with("0 ") {
                break;
            }
        }
        wait("the retried nodes to load");
    }
    harness.run_ok();
    assert_eq!(failed_text(&harness), None);
    assert!(harness.query_by_label("Retry").is_none());

    // The same node ranges failed, then loaded.
    let node_requests: Vec<u16> = server
        .requests()
        .iter()
        .filter(|request| {
            request
                .range
                .as_deref()
                .and_then(|range| range.strip_prefix("bytes="))
                .and_then(|range| range.split('-').next()?.parse::<u64>().ok())
                .is_some_and(|first| nodes.contains(&first))
        })
        .map(|request| request.status)
        .collect();
    assert!(node_requests.contains(&503), "{node_requests:?}");
    assert!(node_requests.contains(&206), "{node_requests:?}");
}
