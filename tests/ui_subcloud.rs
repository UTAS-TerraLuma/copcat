//! Drawing a subcloud from Explore mode in the real viewer, driven by
//! egui_kittest: load the fixture, look straight down, pick the circle tool,
//! drag a circle over the building and check the subcloud view opens with
//! that circle and its map, then close it.
//!
//! Needs a GPU adapter, real or software; without one the test prints
//! SKIPPED and passes, unless `COPCAT_REQUIRE_GPU` is set. Update the
//! snapshot with `UPDATE_SNAPSHOTS=1 cargo test --test ui_subcloud`.

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

/// Only stops a hang: loading the fixture normally takes well under a second.
const LOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// The building's roof in the fixture, in file coordinates: 18 × 12 m from
/// (500012, 5400038). See `tests/data/generate_fixture.py`.
const BUILDING_CENTRE: (f64, f64) = (500_021.0, 5_400_044.0);
const CIRCLE_RADIUS: f64 = 10.0;

/// The fixture's bounds, which the top view is framed on.
const BOUNDS_MIN: [f64; 3] = [500_000.0, 5_400_000.0, 100.0];
const BOUNDS_MAX: [f64; 3] = [500_100.0, 5_400_060.0, 125.0];

fn has_adapter() -> bool {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    !pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).is_empty()
}

/// The text of the label after the one reading `name`.
fn text_after(harness: &Harness<'_, impl eframe::App>, name: &str) -> Option<String> {
    let labels: Vec<_> = harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .collect();
    let at = labels
        .iter()
        .position(|node| node.value().as_deref() == Some(name))?;
    labels.get(at + 1)?.value()
}

/// The label whose text starts with `prefix`.
fn label_starting(harness: &Harness<'_, impl eframe::App>, prefix: &str) -> Option<String> {
    harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .filter_map(|node| node.value())
        .find(|text| text.starts_with(prefix))
}

/// Steps until every node the views want is drawn, blocking on the loader's
/// repaint requests in between. `woken` gets one message per request.
fn wait_until_loaded(harness: &mut Harness<'_, impl eframe::App>, woken: &mpsc::Receiver<()>) {
    let started = Instant::now();
    loop {
        harness.step();
        // The streaming numbers are in the Diagnostics section, which starts
        // collapsed: open it once the section is there.
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
                return;
            }
        }
        let left = LOAD_TIMEOUT.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "the fixture didn't finish loading: nodes drawn {drawn:?}, requested {requested:?}"
        );
        let _ = woken.recv_timeout(left);
    }
}

/// Where a ground point in file coordinates is in the 3D view `view`,
/// looking straight down at the fixture as the Top view frames it: centred
/// on the bounds, `2 × half height` metres tall, north up.
fn top_view_screen(view: egui::Rect, (east, north): (f64, f64)) -> egui::Pos2 {
    let size: Vec<f64> = (0..3).map(|i| BOUNDS_MAX[i] - BOUNDS_MIN[i]).collect();
    // `OrbitCamera::frame_bounds`: a sphere around the box, at a distance
    // that fits it in a 45° field of view, then the orthographic view shows
    // what that field of view did at the target.
    let radius = 0.5 * (size[0] * size[0] + size[1] * size[1] + size[2] * size[2]).sqrt();
    let half_fov = 22.5_f64.to_radians();
    let half_height = radius / half_fov.sin() * half_fov.tan();
    let px_per_metre = f64::from(view.height()) / (2.0 * half_height);
    let centre = view.center();
    let dx = (east - (BOUNDS_MIN[0] + BOUNDS_MAX[0]) / 2.0) * px_per_metre;
    let dy = (north - (BOUNDS_MIN[1] + BOUNDS_MAX[1]) / 2.0) * px_per_metre;
    egui::pos2(centre.x + dx as f32, centre.y - dy as f32)
}

#[test]
fn circle_tool_should_open_a_subcloud_where_it_was_dragged() {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return;
    }
    let fixture = common::fixture().to_str().unwrap().to_owned();
    let (wake, woken) = mpsc::channel();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 640.0))
        .with_pixels_per_point(1.0)
        .wgpu()
        .build_eframe(|cc| {
            cc.egui_ctx.set_request_repaint_callback(move |_| {
                let _ = wake.send(());
            });
            copcat::viewer_app(cc, Some(fixture), copcat::HeightSettings::default())
                .expect("the viewer needs the wgpu renderer")
        });
    wait_until_loaded(&mut harness, &woken);

    // Nothing to close before a subcloud is drawn.
    assert!(harness.query_by_label(CLOSE).is_none());

    // Top view, from its default shortcut (the panel has no view buttons).
    harness.key_press(egui::Key::Num7);
    harness.run_ok();
    // The tools float over the top-left of the 3D view.
    let view = harness.get_by_label("3D view").rect();
    for tool in ["Line", "Circle", "Square"] {
        let button = harness.get_by_role_and_label(egui::accesskit::Role::Button, tool);
        assert!(view.contains_rect(button.rect()), "{tool} is off the view");
        assert!(button.rect().center().x < view.left() + 60.0, "{tool}");
        assert_eq!(picked(&harness, tool), Some(false), "{tool}");
    }
    harness.get_by_label("Circle").click();
    harness.run_ok();
    assert_eq!(
        picked(&harness, "Circle"),
        Some(true),
        "the circle tool should be picked"
    );

    // A drag that starts on the strip goes to the strip, not the view: it
    // draws nothing, and the circle tool stays picked.
    let square = harness.get_by_label("Square").rect().center();
    harness.drag_at(square);
    harness.run_ok();
    for step in 1..=4 {
        harness.hover_at(square + (view.center() - square) * (step as f32 / 4.0));
        harness.run_ok();
    }
    harness.drop_at(view.center());
    harness.run_ok();
    assert!(harness.query_by_label("Subcloud map").is_none());
    assert_eq!(picked(&harness, "Circle"), Some(true));
    assert_eq!(picked(&harness, "Square"), Some(false));

    // Drag from the building's middle out to the radius, eastwards.
    let centre = top_view_screen(view, BUILDING_CENTRE);
    let edge = top_view_screen(view, (BUILDING_CENTRE.0 + CIRCLE_RADIUS, BUILDING_CENTRE.1));
    harness.hover_at(centre);
    harness.run_ok();
    harness.drag_at(centre);
    harness.run_ok();
    for step in 1..=4 {
        harness.hover_at(centre + (edge - centre) * (step as f32 / 4.0));
        harness.run_ok();
    }
    harness.drop_at(edge);
    harness.run_ok();

    // The subcloud view opened, with the map beside it...
    assert!(
        harness.query_by_label("Subcloud map").is_some(),
        "the map panel should be showing"
    );
    assert!(
        harness
            .query_by_role_and_label(egui::accesskit::Role::Button, CLOSE)
            .is_some(),
        "the map's panel should have a close button"
    );
    // ...and the circle where it was drawn, give or take a pixel's worth.
    let centre_text = label_starting(&harness, "Centre ").expect("a Centre label");
    let numbers: Vec<f64> = centre_text
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter_map(|word| word.parse().ok())
        .collect();
    let [east, north] = numbers[..] else {
        panic!("couldn't read {centre_text:?}");
    };
    assert!(
        (east - BUILDING_CENTRE.0).abs() < 1.0 && (north - BUILDING_CENTRE.1).abs() < 1.0,
        "the circle is centred at {east}, {north}, not on the building"
    );
    let radius = harness
        .get_by_label("Radius")
        .accesskit_node()
        .numeric_value()
        .expect("the radius field's value");
    assert!(
        (radius - CIRCLE_RADIUS).abs() < 1.0,
        "the radius is {radius} m, not {CIRCLE_RADIUS} m"
    );

    // Let the subcloud's nodes load, then picture it.
    wait_until_loaded(&mut harness, &woken);
    harness.run_ok();
    harness.remove_cursor();
    harness.run_ok();
    // The open time and the fixture's absolute path change between runs.
    let mut masks: Vec<egui::Rect> = harness
        .query_all_by_value(common::fixture().to_str().unwrap())
        .map(|node| node.rect())
        .collect();
    let labels: Vec<_> = harness
        .query_all_by_role(egui::accesskit::Role::Label)
        .collect();
    if let Some(at) = labels
        .iter()
        .position(|node| node.value().as_deref() == Some("Open time"))
    {
        masks.push(labels[at + 1].rect());
    }
    drop(labels);
    for rect in masks {
        common::mask_shown(&mut harness, rect);
    }
    common::snapshot(&mut harness, "subcloud_circle_over_building");

    // The close button, at the top right of the map's panel, goes back to
    // Explore mode with no subcloud to return to.
    let map = harness.get_by_label("Subcloud map").rect();
    let close = harness.get_by_role_and_label(egui::accesskit::Role::Button, CLOSE);
    assert!(close.rect().top() < map.top(), "above the map");
    assert!(
        (close.rect().right() - map.right()).abs() < 2.0,
        "at the panel's right"
    );
    close.click();
    harness.run_ok();
    assert!(harness.query_by_label("Subcloud map").is_none());
    assert!(harness.query_by_label(CLOSE).is_none());
    assert_eq!(
        picked(&harness, "Circle"),
        Some(false),
        "back in Explore mode"
    );
    // The 3D view is the explore view, as wide as before.
    let explore = harness.get_by_label("3D view").rect();
    assert_eq!(explore, view);
}

/// The close button's name and tooltip.
const CLOSE: &str = "Close the subcloud";

/// Whether the tool button `name` is picked, if it's showing.
fn picked(harness: &Harness<'_, impl eframe::App>, name: &str) -> Option<bool> {
    let button = harness.query_by_role_and_label(egui::accesskit::Role::Button, name)?;
    Some(button.accesskit_node().toggled() == Some(egui::accesskit::Toggled::True))
}
