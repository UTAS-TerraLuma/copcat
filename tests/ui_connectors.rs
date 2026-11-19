//! Data sources (connectors) in the real viewer, driven headlessly by
//! egui_kittest through the keyboard: a STAC catalog served by the test
//! server, or a folder, browsed in quick open down to the fixture, which
//! then opens and loads. Settings come from scratch directories, never the
//! real home.
//!
//! Needs a GPU adapter, as `ui_kittest.rs` does; prints SKIPPED without one
//! unless `COPCAT_REQUIRE_GPU` is set. Snapshots are in `tests/snapshots/`;
//! update them with `UPDATE_SNAPSHOTS=1 cargo test --test ui_connectors`.

mod common;

use std::fs;
use std::path::Path;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use common::server::FileServer;
use eframe::egui::{self, Key, Modifiers, accesskit::Role};
use eframe::egui_wgpu::wgpu;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

/// Only stops a hang: the fixture catalog and cloud load in well under a
/// second.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Held by each test while its viewer is alive, so only one at a time has a
/// GPU device (see `ui_palette.rs`).
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

/// The viewer with nothing open, reading settings from `user_dir` and
/// `workspace`, and a channel that hears each repaint request, so a test
/// can block on background work instead of sleeping.
fn viewer(
    user_dir: &Path,
    workspace: &Path,
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
            copcat::viewer_app_with_settings(cc, None, copcat::HeightSettings::default(), locations)
                .expect("the viewer needs the wgpu renderer")
        });
    harness.run_ok();
    (harness, woken)
}

/// A user settings file declaring `connectors`.
fn user_settings(scratch: &Path, connectors: serde_json::Value) -> std::path::PathBuf {
    let user_dir = scratch.join("home").join(".copcat");
    let settings = serde_json::json!({ "theme": "graphite", "connectors": connectors });
    write(&user_dir.join("settings.json"), &settings.to_string());
    user_dir
}

fn press(harness: &mut Harness<'_, impl eframe::App>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_ok();
}

fn enter(harness: &mut Harness<'_, impl eframe::App>) {
    press(harness, Modifiers::NONE, Key::Enter);
}

fn type_text(harness: &mut Harness<'_, impl eframe::App>, text: &str) {
    harness.event(egui::Event::Text(text.to_owned()));
    harness.run_ok();
}

/// Every palette row's label: its title, then its detail.
fn rows(harness: &Harness<'_, impl eframe::App>) -> Vec<String> {
    harness
        .query_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
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

/// Steps until the palette shows a row titled `title`: its list is fetched
/// on a thread, which asks for a repaint when it's done.
fn wait_for_row(
    harness: &mut Harness<'_, impl eframe::App>,
    woken: &mpsc::Receiver<()>,
    title: &str,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if row_enabled(harness, title).is_some() {
            harness.run_ok();
            return;
        }
        let left = TIMEOUT.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "no row {title:?}; the rows are {:#?}",
            rows(harness)
        );
        let _ = woken.recv_timeout(left);
    }
}

/// Where the palette row titled `title` is.
fn row_rect(harness: &Harness<'_, impl eframe::App>, title: &str) -> egui::Rect {
    harness
        .get_by(|node| {
            node.role() == Role::Button
                && node
                    .label()
                    .is_some_and(|label| label == title || label.starts_with(&format!("{title} ")))
        })
        .rect()
}

/// Steps until egui has a texture whose name ends with `name`: a thumbnail
/// is fetched and decoded on a thread, which asks for a repaint when done.
fn wait_for_texture(
    harness: &mut Harness<'_, impl eframe::App>,
    woken: &mpsc::Receiver<()>,
    name: &str,
) {
    let started = Instant::now();
    loop {
        harness.step();
        let loaded = harness
            .ctx
            .tex_manager()
            .read()
            .allocated()
            .any(|(_, meta)| meta.name.ends_with(name));
        if loaded {
            harness.run_ok();
            return;
        }
        let left = TIMEOUT.saturating_sub(started.elapsed());
        assert!(!left.is_zero(), "no texture {name:?}");
        let _ = woken.recv_timeout(left);
    }
}

/// The label in the row after the one labelled `name`, in the side panel's
/// two-column grids: its text and where it is.
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

/// Steps until the cloud is open and every node it wants is drawn, as
/// `ui_kittest.rs` does, opening the Diagnostics section to see.
fn wait_until_drawn(harness: &mut Harness<'_, impl eframe::App>, woken: &mpsc::Receiver<()>) {
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
        let left = TIMEOUT.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "the cloud didn't finish loading: nodes drawn {drawn:?}, requested {requested:?}"
        );
        let _ = woken.recv_timeout(left);
    }
    harness.run_ok();
}

fn assert_fixture_is_open(harness: &Harness<'_, impl eframe::App>) {
    assert_eq!(
        text_after(harness, "Source").as_deref(),
        Some("fixture.copc.laz")
    );
    assert_eq!(
        text_after(harness, "Total points").as_deref(),
        Some("7.7 k")
    );
}

fn lock_gpu() -> Option<std::sync::MutexGuard<'static, ()>> {
    if !has_adapter() {
        common::skip_or_fail_without_gpu("couldn't find a GPU: no wgpu adapters");
        return None;
    }
    Some(
        ONE_VIEWER_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

#[test]
fn a_static_stac_catalog_should_browse_down_to_a_cloud_and_open_it() {
    let Some(_gpu) = lock_gpu() else { return };
    let server = FileServer::serve(common::data_dir());
    let scratch = common::scratch_dir("a_static_stac_catalog_should_browse_down_to_a_cloud");
    let user_dir = user_settings(
        &scratch,
        serde_json::json!({ "stac": [{ "name": "Fixture catalog", "url": server.url("stac/static/") }] }),
    );
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"));

    // Quick open lists the source by name, marked as STAC.
    press(&mut harness, Modifiers::COMMAND, Key::P);
    assert_eq!(row_enabled(&harness, "Fixture catalog"), Some(true));
    assert!(
        rows(&harness)
            .iter()
            .any(|row| row.starts_with("Fixture catalog STAC source")),
        "{:#?}",
        rows(&harness)
    );
    type_text(&mut harness, "fixture cat");
    enter(&mut harness);

    // Catalog, then collection, then its items.
    wait_for_row(&mut harness, &woken, "Fixture survey");
    assert!(
        harness.query_by_label("Fixture catalog").is_some(),
        "the breadcrumb"
    );
    let plain_row = row_rect(&harness, "Fixture survey");
    enter(&mut harness);
    wait_for_row(&mut harness, &woken, "Site A");
    // Site A's thumbnail arrives; rows with and without one are as tall as
    // rows without any, with their titles lined up.
    wait_for_texture(&mut harness, &woken, "site-a-thumbnail.png");
    let (site_a, site_b) = (row_rect(&harness, "Site A"), row_rect(&harness, "Site B"));
    assert_eq!(site_a.height(), plain_row.height());
    assert_eq!(site_b.height(), plain_row.height());
    assert_eq!(site_a.left(), site_b.left());
    assert_eq!(
        row_enabled(&harness, "Site B"),
        Some(false),
        "no point cloud"
    );
    assert_eq!(
        row_enabled(&harness, "Site C"),
        Some(true),
        "two point clouds"
    );
    type_text(&mut harness, "site a");
    enter(&mut harness);

    wait_until_drawn(&mut harness, &woken);
    assert_fixture_is_open(&harness);
    assert!(
        server
            .requests()
            .iter()
            .any(|request| request.path == "/fixture.copc.laz" && request.status == 206),
        "the cloud should stream from the server"
    );

    harness.remove_cursor();
    harness.run_ok();
    // The open time changes every run, and the server's port too.
    let (_, open_time) = value_after(&harness, "Open time").expect("an Open time row");
    common::mask_shown(&mut harness, open_time);
    let with_port: Vec<_> = harness
        .query_all_by_role(Role::Label)
        .filter(|node| node.value().is_some_and(|text| text.contains("127.0.0.1")))
        .map(|node| node.rect())
        .collect();
    for rect in with_port {
        common::mask_shown(&mut harness, rect);
    }
    common::snapshot(&mut harness, "connector_static_stac_fixture");
}

/// How many times the server was asked for the fixture API's first page.
fn first_page_requests(server: &FileServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|request| request.path == "/stac/api/collections/fixture/items")
        .count()
}

#[test]
fn a_stac_api_should_list_every_page_by_itself_and_open_a_cloud() {
    let Some(_gpu) = lock_gpu() else { return };
    let server = FileServer::serve(common::data_dir());
    let scratch = common::scratch_dir("a_stac_api_should_list_every_page");
    let user_dir = user_settings(
        &scratch,
        serde_json::json!({ "stac": [{ "name": "Fixture API", "url": server.url("stac/api/") }] }),
    );
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"));

    press(&mut harness, Modifiers::COMMAND, Key::P);
    type_text(&mut harness, "fixture api");
    enter(&mut harness);
    wait_for_row(&mut harness, &woken, "Fixture flights");
    enter(&mut harness);
    // All three pages, without choosing "Load more…".
    wait_for_row(&mut harness, &woken, "Late flight");
    assert_eq!(
        row_enabled(&harness, "flight-1"),
        Some(false),
        "no point cloud"
    );
    for title in ["flight-2", "flight-3"] {
        assert_eq!(row_enabled(&harness, title), Some(true), "{title}");
    }
    assert_eq!(row_enabled(&harness, "Load more…"), None, "nothing left");

    // The refresh button is on the breadcrumb line, above the field, not a
    // row below it; it fetches it all again.
    let field = harness
        .query_all_by_role(Role::TextInput)
        .find(|node| node.is_focused())
        .expect("the palette's field")
        .rect();
    let refresh = harness.get_by_label("Refresh").rect();
    let breadcrumb = harness.get_by_label("Fixture flights").rect();
    assert!(refresh.bottom() <= field.top(), "{refresh:?} {field:?}");
    assert!(refresh.center().y > breadcrumb.top() && refresh.center().y < breadcrumb.bottom());
    assert_eq!(first_page_requests(&server), 1);
    harness.get_by_label("Refresh").click();
    harness.run_ok();
    // The old list stays up while it fetches, so wait on the fetch itself:
    // each page that arrives wakes the viewer.
    let started = Instant::now();
    while first_page_requests(&server) < 2 {
        let left = TIMEOUT.saturating_sub(started.elapsed());
        assert!(!left.is_zero(), "Refresh didn't fetch the first page again");
        let _ = woken.recv_timeout(left);
        harness.step();
    }
    wait_for_row(&mut harness, &woken, "Late flight");
    assert_eq!(first_page_requests(&server), 2);

    // Filtering finds an item from the last page.
    type_text(&mut harness, "late");
    assert_eq!(row_enabled(&harness, "flight-2"), None);
    enter(&mut harness);

    wait_until_drawn(&mut harness, &woken);
    assert_fixture_is_open(&harness);
}

#[test]
fn a_folder_source_should_list_its_clouds_and_open_one() {
    let Some(_gpu) = lock_gpu() else { return };
    let scratch = common::scratch_dir("a_folder_source_should_list_its_clouds");
    let surveys = scratch.join("surveys");
    let copy = surveys.join("site").join("2026").join("fixture.copc.laz");
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    fs::copy(common::fixture(), &copy).unwrap();
    // Hidden folders aren't searched.
    write(&surveys.join(".cache").join("hidden.copc.laz"), "");
    let workspace = scratch.join("project");
    fs::create_dir_all(&workspace).unwrap();
    let user_dir = user_settings(
        &scratch,
        serde_json::json!({ "folders": [{ "name": "Surveys", "path": surveys.to_str().unwrap() }] }),
    );
    let (mut harness, woken) = viewer(&user_dir, &workspace);

    // The workspace is offered too, without any settings.
    press(&mut harness, Modifiers::COMMAND, Key::P);
    assert_eq!(row_enabled(&harness, "Workspace: project"), Some(true));
    assert!(
        rows(&harness)
            .iter()
            .any(|row| row.starts_with("Surveys Folder source")),
        "{:#?}",
        rows(&harness)
    );
    type_text(&mut harness, "surveys");
    enter(&mut harness);
    wait_for_row(&mut harness, &woken, "fixture.copc.laz");
    assert!(
        rows(&harness)
            .iter()
            .any(|row| row == "fixture.copc.laz site/2026"),
        "with its folder: {:#?}",
        rows(&harness)
    );
    assert_eq!(row_enabled(&harness, "hidden.copc.laz"), None);
    enter(&mut harness);

    wait_until_drawn(&mut harness, &woken);
    assert_fixture_is_open(&harness);
}

#[test]
fn add_stac_source_should_save_it_and_list_it_in_quick_open() {
    let Some(_gpu) = lock_gpu() else { return };
    let server = FileServer::serve(common::data_dir());
    let scratch = common::scratch_dir("add_stac_source_should_save_it");
    let user_dir = scratch.join("home").join(".copcat");
    let user_file = user_dir.join("settings.json");
    write(
        &user_file,
        r#"{ "theme": "graphite", "display": { "pointSize": 3.0 } }"#,
    );
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"));

    press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    type_text(&mut harness, "add stac");
    enter(&mut harness);
    assert!(
        harness.query_by_label("Add STAC source").is_some(),
        "the breadcrumb"
    );
    let url = server.url("stac/static/");
    type_text(&mut harness, &url);
    enter(&mut harness);
    // The catalog is fetched for its title, the default name.
    wait_for_row(&mut harness, &woken, "Save as \"Fixture catalog\"");
    enter(&mut harness);

    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&user_file).unwrap()).unwrap();
    assert_eq!(
        saved["connectors"]["stac"],
        serde_json::json!([{ "name": "Fixture catalog", "url": url }])
    );
    assert_eq!(saved["theme"], "graphite", "other keys are kept");
    assert_eq!(saved["display"]["pointSize"], 3.0);
    assert!(
        harness
            .query_all_by_role(Role::Label)
            .filter_map(|node| node.value())
            .any(|text| text.starts_with("Added the data source \"Fixture catalog\"")),
        "it should say so"
    );

    press(&mut harness, Modifiers::COMMAND, Key::P);
    assert_eq!(row_enabled(&harness, "Fixture catalog"), Some(true));

    // And it can be removed again.
    // Switching modes in the open palette leaves the cursor after the `>`.
    press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    type_text(&mut harness, "remove data source");
    enter(&mut harness);
    assert_eq!(
        row_enabled(&harness, "Fixture catalog"),
        Some(true),
        "{:#?}",
        rows(&harness)
    );
    enter(&mut harness);
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&user_file).unwrap()).unwrap();
    assert_eq!(saved["connectors"]["stac"], serde_json::json!([]));
    press(&mut harness, Modifiers::COMMAND, Key::P);
    assert_eq!(row_enabled(&harness, "Fixture catalog"), None);
}

/// Copies the directory `from` into `to`, recursively.
fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A copy of the fixture catalogs, served from a scratch directory so a test
/// can change them.
fn served_copy(scratch: &Path) -> (FileServer, std::path::PathBuf) {
    let served = scratch.join("served");
    copy_dir(&common::data_dir().join("stac"), &served.join("stac"));
    (FileServer::serve(&served), served)
}

/// The cached listings' files.
fn cached_lists(user_dir: &Path) -> Vec<std::path::PathBuf> {
    match fs::read_dir(user_dir.join("cache").join("stac")) {
        Ok(dir) => dir.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Every label's text.
fn labels(harness: &Harness<'_, impl eframe::App>) -> Vec<String> {
    harness
        .query_all_by_role(Role::Label)
        .filter_map(|node| node.value().or_else(|| node.accesskit_node().label()))
        .collect()
}

fn has_label_starting(harness: &Harness<'_, impl eframe::App>, start: &str) -> bool {
    labels(harness).iter().any(|text| text.starts_with(start))
}

/// Steps until a label starts with `start`.
fn wait_for_label(
    harness: &mut Harness<'_, impl eframe::App>,
    woken: &mpsc::Receiver<()>,
    start: &str,
) {
    let started = Instant::now();
    loop {
        harness.step();
        if has_label_starting(harness, start) {
            harness.run_ok();
            return;
        }
        let left = TIMEOUT.saturating_sub(started.elapsed());
        assert!(
            !left.is_zero(),
            "no label {start:?}; the labels are {:#?}",
            labels(harness)
        );
        let _ = woken.recv_timeout(left);
    }
}

fn requests_for(server: &FileServer, path: &str) -> usize {
    server
        .requests()
        .iter()
        .filter(|request| request.path == path)
        .count()
}

#[test]
fn a_stac_list_should_show_from_the_cache_at_once_then_update() {
    let Some(_gpu) = lock_gpu() else { return };
    let scratch = common::scratch_dir("a_stac_list_should_show_from_the_cache");
    let (server, served) = served_copy(&scratch);
    let user_dir = user_settings(
        &scratch,
        serde_json::json!({ "stac": [{ "name": "Fixture API", "url": server.url("stac/api/") }] }),
    );

    // The first session fetches the lists, and keeps them once whole.
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"));
    press(&mut harness, Modifiers::COMMAND, Key::P);
    type_text(&mut harness, "fixture api");
    enter(&mut harness);
    wait_for_row(&mut harness, &woken, "Fixture flights");
    enter(&mut harness);
    wait_for_row(&mut harness, &woken, "Late flight");
    let files = cached_lists(&user_dir);
    assert_eq!(files.len(), 2, "the source's top level and the items");
    for file in &files {
        println!(
            "cached list {}: {} bytes",
            file.display(),
            fs::metadata(file).unwrap().len()
        );
    }
    drop(harness);

    // The collection is renamed, and the server doesn't answer yet.
    let collections = served.join("stac/api/collections/index.json");
    let renamed = fs::read_to_string(&collections)
        .unwrap()
        .replace("\"Fixture flights\"", "\"Renamed flights\"");
    fs::write(&collections, renamed).unwrap();
    server.hold(true);
    let landing_before = requests_for(&server, "/stac/api/");

    // The next session lists them at once, from the cache, and says it's
    // updating them.
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"));
    press(&mut harness, Modifiers::COMMAND, Key::P);
    type_text(&mut harness, "fixture api");
    enter(&mut harness);
    assert_eq!(row_enabled(&harness, "Fixture flights"), Some(true));
    assert!(
        has_label_starting(&harness, "Updating…"),
        "{:#?}",
        labels(&harness)
    );
    enter(&mut harness);
    assert_eq!(row_enabled(&harness, "Late flight"), Some(true));
    for title in ["flight-2", "flight-3"] {
        assert_eq!(row_enabled(&harness, title), Some(true), "{title}");
    }
    assert_eq!(
        requests_for(&server, "/stac/api/"),
        landing_before,
        "no reply yet"
    );

    // Once the server answers, the new list replaces the cached one.
    press(&mut harness, Modifiers::NONE, Key::Escape);
    server.hold(false);
    wait_for_row(&mut harness, &woken, "Renamed flights");
    assert_eq!(row_enabled(&harness, "Fixture flights"), None);
    assert!(requests_for(&server, "/stac/api/") > landing_before);
    assert!(!has_label_starting(&harness, "Updating…"));

    // Clearing the cache deletes the files and says so.
    press(&mut harness, Modifiers::NONE, Key::Escape);
    press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    type_text(&mut harness, "clear data source cache");
    enter(&mut harness);
    assert!(
        has_label_starting(&harness, "Cleared the data source cache: 2 lists"),
        "{:#?}",
        labels(&harness)
    );
    assert!(cached_lists(&user_dir).is_empty());
}

#[test]
fn a_cached_stac_list_should_stay_when_the_server_fails() {
    let Some(_gpu) = lock_gpu() else { return };
    let scratch = common::scratch_dir("a_cached_stac_list_should_stay_when_the_server_fails");
    let (server, _served) = served_copy(&scratch);
    let user_dir = user_settings(
        &scratch,
        serde_json::json!({ "stac": [{ "name": "Fixture API", "url": server.url("stac/api/") }] }),
    );
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"));
    press(&mut harness, Modifiers::COMMAND, Key::P);
    type_text(&mut harness, "fixture api");
    enter(&mut harness);
    wait_for_row(&mut harness, &woken, "Fixture flights");
    drop(harness);

    server.fail_all(true);
    let (mut harness, woken) = viewer(&user_dir, &scratch.join("nowhere"));
    press(&mut harness, Modifiers::COMMAND, Key::P);
    type_text(&mut harness, "fixture api");
    enter(&mut harness);
    wait_for_label(&mut harness, &woken, "Offline: showing the list from");
    assert_eq!(row_enabled(&harness, "Fixture flights"), Some(true));
    assert!(
        !labels(&harness)
            .iter()
            .any(|text| text.contains("HTTP 503")),
        "no error in place of the list: {:#?}",
        labels(&harness)
    );

    // Removing the source deletes its cached lists.
    assert!(!cached_lists(&user_dir).is_empty());
    press(&mut harness, Modifiers::NONE, Key::Escape);
    press(&mut harness, Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    type_text(&mut harness, "remove data source");
    enter(&mut harness);
    assert_eq!(row_enabled(&harness, "Fixture API"), Some(true));
    enter(&mut harness);
    assert!(cached_lists(&user_dir).is_empty());
}
