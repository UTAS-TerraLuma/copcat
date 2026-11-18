//! The STAC client against the fixture catalogs in `tests/data/stac`, served
//! over HTTP by the test server: a static catalog, and a fake STAC API with
//! two pages of items.
//!
//! The `#[ignore]`d tests at the end browse two real catalogs over the
//! internet: `cargo test --test stac_client -- --ignored`.

mod common;

use common::server::FileServer;
use copcat::stac::{Client, Entry, Href, Item, Page};

fn titles(page: &Page) -> Vec<String> {
    page.entries
        .iter()
        .map(|entry| match entry {
            Entry::Node(node) => node.title.clone(),
            Entry::Item(item) => item.display_title().to_owned(),
            Entry::Unavailable { title, .. } => title.clone(),
        })
        .collect()
}

fn node_url(page: &Page, title: &str) -> String {
    page.entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Node(node) if node.title == title => Some(node.url.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no node {title:?} in {:?}", titles(page)))
}

fn item<'p>(page: &'p Page, title: &str) -> &'p Item {
    page.entries
        .iter()
        .find_map(|entry| match entry {
            Entry::Item(item) if item.display_title() == title => Some(item),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no item {title:?} in {:?}", titles(page)))
}

fn cloud_urls(item: &Item) -> Vec<Href> {
    item.point_clouds()
        .map(|asset| asset.href.clone())
        .collect()
}

#[test]
fn a_static_catalog_should_browse_down_to_its_point_clouds() {
    let server = FileServer::serve(common::data_dir());
    let client = Client::new();
    // Given by its folder: catalog.json in it is found.
    let root = client.browse_root(&server.url("stac/static")).unwrap();
    assert_eq!(titles(&root), ["Fixture survey"]);
    assert_eq!(root.more, None);

    let survey = client.browse(&node_url(&root, "Fixture survey")).unwrap();
    assert_eq!(titles(&survey), ["Site A", "Site B", "Site C"]);
    let site_a = item(&survey, "Site A");
    assert_eq!(site_a.date(), Some("2026-04-14"));
    assert_eq!(
        cloud_urls(site_a),
        [Href::Openable(server.url("fixture.copc.laz"))]
    );
    assert!(cloud_urls(item(&survey, "Site B")).is_empty());
    let site_c = cloud_urls(item(&survey, "Site C"));
    assert_eq!(site_c.len(), 2);
    assert!(matches!(&site_c[1], Href::NotOpenable { href, .. } if href.starts_with("s3://")));

    let info = client.describe(&server.url("stac/static/")).unwrap();
    assert_eq!(info.title.as_deref(), Some("Fixture catalog"));
    assert_eq!(info.url, server.url("stac/static/catalog.json"));
    assert!(!info.is_api);
}

#[test]
fn a_stac_api_should_page_through_items_on_request() {
    let server = FileServer::serve(common::data_dir());
    let client = Client::new();
    let root = client.browse_root(&server.url("stac/api/")).unwrap();
    assert_eq!(titles(&root), ["Fixture flights"]);

    let flights = client.browse(&node_url(&root, "Fixture flights")).unwrap();
    assert_eq!(titles(&flights), ["flight-1", "flight-2"]);
    assert!(cloud_urls(item(&flights, "flight-1")).is_empty());
    assert_eq!(
        cloud_urls(item(&flights, "flight-2")),
        [Href::Openable(server.url("fixture.copc.laz"))]
    );
    // The first page was asked for with a limit; the next isn't fetched
    // until asked for (the viewer asks by itself, up to a limit).
    let items_requests: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path.contains("/items"))
        .collect();
    assert_eq!(items_requests.len(), 1, "{items_requests:?}");
    assert_eq!(items_requests[0].query.as_deref(), Some("limit=250"));

    let more = flights.more.as_ref().expect("a next page");
    let page_2 = client.more(more).unwrap();
    assert_eq!(titles(&page_2), ["flight-3"]);
    let page_3 = client
        .more(page_2.more.as_ref().expect("a third page"))
        .unwrap();
    assert_eq!(titles(&page_3), ["Late flight"]);
    assert_eq!(page_3.more, None);

    let info = client.describe(&server.url("stac/api/")).unwrap();
    assert_eq!(info.title.as_deref(), Some("Fixture STAC API"));
    assert!(info.is_api);
}

#[test]
fn following_more_should_list_a_whole_api_collection_once() {
    let server = FileServer::serve(common::data_dir());
    let client = Client::new();
    let root = client.browse_root(&server.url("stac/api/")).unwrap();
    let mut page = client.browse(&node_url(&root, "Fixture flights")).unwrap();
    let mut listed = titles(&page);
    while let Some(more) = page.more.take() {
        page = client.more(&more).unwrap();
        listed.extend(titles(&page));
    }
    assert_eq!(listed, ["flight-1", "flight-2", "flight-3", "Late flight"]);
    // Each page once, following the next links as given.
    let queries: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|request| request.path.contains("/items"))
        .map(|request| format!("{}?{}", request.path, request.query.unwrap_or_default()))
        .collect();
    assert_eq!(
        queries,
        [
            "/stac/api/collections/fixture/items?limit=250",
            "/stac/api/collections/fixture/items/page-2?limit=2",
            "/stac/api/collections/fixture/items/page-3?limit=2",
        ]
    );
}

#[test]
fn a_missing_or_wrong_catalog_should_say_what_is_wrong() {
    let server = FileServer::serve(common::data_dir());
    let client = Client::new();
    let missing = client.browse_root(&server.url("stac/nowhere")).unwrap_err();
    assert!(missing.0.contains("404"), "{missing}");
    assert!(missing.0.contains("catalog.json"), "{missing}");
    let not_json = client.browse_root(&server.url("README.md")).unwrap_err();
    assert!(not_json.0.contains("isn't JSON"), "{not_json}");
    let laz = client.browse(&server.url("fixture.copc.laz")).unwrap_err();
    assert!(laz.0.contains("isn't JSON"), "{laz}");
}

/// The first openable point cloud found browsing depth-first from `page`,
/// following at most a few nodes at each level.
fn find_point_cloud(client: &Client, page: &Page, depth: usize) -> Option<String> {
    for entry in &page.entries {
        if let Entry::Item(item) = entry
            && let Some(Href::Openable(url)) = item.point_clouds().map(|a| a.href.clone()).next()
        {
            return Some(url);
        }
    }
    if depth == 0 {
        return None;
    }
    page.entries.iter().take(3).find_map(|entry| match entry {
        Entry::Node(node) => {
            let below = client.browse(&node.url).ok()?;
            find_point_cloud(client, &below, depth - 1)
        }
        _ => None,
    })
}

#[test]
#[ignore = "fetches a real static catalog over the internet"]
fn the_aeo_workshop_static_catalog_should_have_point_clouds() {
    let client = Client::new();
    let source = "https://object-store.rc.nectar.org.au/v1/AUTH_4df2f67c2eed48a2aaeeed008a4bf0de/aeo2026-workshop";
    let info = client.describe(source).unwrap();
    assert!(info.url.ends_with("/catalog.json"), "{info:?}");
    let root = client.browse_root(source).unwrap();
    let url = find_point_cloud(&client, &root, 3).expect("a COPC asset");
    println!("found {url}");
    assert!(url.ends_with(".copc.laz"), "{url}");
}

#[test]
#[ignore = "fetches a real STAC API over the internet"]
fn the_naturescan_api_should_list_in_full_with_point_clouds() {
    let client = Client::new();
    let root = client
        .browse_root("https://stacapi.naturescan.earth/")
        .unwrap();
    let Some(Entry::Node(collection)) = root.entries.first() else {
        panic!("no collections: {:?}", titles(&root));
    };
    let mut page = client.browse(&collection.url).unwrap();
    let url = find_point_cloud(&client, &page, 0).expect("a COPC asset");
    println!("found {url}");
    let mut listed = titles(&page);
    let mut pages = 1;
    while let Some(more) = page.more.take() {
        page = client.more(&more).unwrap();
        listed.extend(titles(&page));
        pages += 1;
        assert!(pages < 50, "too many pages");
    }
    println!("{} items in {pages} pages", listed.len());
    let mut unique = listed.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), listed.len(), "no item twice");
}
