//! Reading STAC JSON: pure functions of a document and the URL it came
//! from. Every field is optional here; anything missing or the wrong type
//! is skipped rather than failing the listing.

use serde_json::Value;
use url::Url;

use super::{Asset, AssetKind, Entry, Href, Item, Node, StacError};

/// The media type of a COPC file.
pub(crate) const COPC_MEDIA_TYPE: &str = "application/vnd.laszip+copc";

/// How a document lists what's in it.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Parsed {
    /// Nodes, and items given in full.
    pub(crate) entries: Vec<Entry>,
    /// Item documents to fetch, in a static catalog.
    pub(crate) item_links: Vec<Url>,
    pub(crate) next_page: Option<Url>,
    /// The listing is somewhere else: an API landing page's collections, or
    /// an API collection's items.
    pub(crate) follow: Option<Url>,
}

/// A link, with its href resolved.
#[derive(Debug)]
struct Link {
    rel: String,
    href: Url,
    title: Option<String>,
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The document's links that can be followed with a GET, resolved against
/// `base`. Links that can't be resolved, or need another method, are left
/// out.
fn links(base: &Url, doc: &Value) -> Vec<Link> {
    let Some(links) = doc.get("links").and_then(Value::as_array) else {
        return Vec::new();
    };
    links
        .iter()
        .filter_map(|link| {
            let rel = string(link, "rel")?;
            let method = string(link, "method").unwrap_or("GET");
            if !method.eq_ignore_ascii_case("GET") {
                return None;
            }
            let href = base.join(string(link, "href")?).ok()?;
            Some(Link {
                rel: rel.to_owned(),
                href,
                title: string(link, "title").map(str::to_owned),
            })
        })
        .collect()
}

fn first_link<'a>(links: &'a [Link], rel: &str) -> Option<&'a Link> {
    links.iter().find(|link| link.rel == rel)
}

/// Whether `doc` is a STAC API landing page rather than a static catalog.
pub(crate) fn is_api(doc: &Value) -> bool {
    let conforms = doc
        .get("conformsTo")
        .and_then(Value::as_array)
        .is_some_and(|classes| !classes.is_empty());
    let has_rel = |rel: &str| {
        doc.get("links")
            .and_then(Value::as_array)
            .is_some_and(|links| {
                links
                    .iter()
                    .any(|link| link.get("rel").and_then(Value::as_str) == Some(rel))
            })
    };
    conforms || has_rel("data") || has_rel("search")
}

/// Whether `doc` looks like any STAC document this client can list.
pub(crate) fn is_stac(doc: &Value) -> bool {
    let Some(object) = doc.as_object() else {
        return false;
    };
    let typed = matches!(
        string(doc, "type"),
        Some("Catalog" | "Collection" | "Feature" | "FeatureCollection")
    );
    typed
        || object.contains_key("stac_version")
        || object.contains_key("collections")
        || object.contains_key("conformsTo")
}

/// The document's title, or failing that its id.
pub(crate) fn title(doc: &Value) -> Option<String> {
    string(doc, "title")
        .or_else(|| string(doc, "id"))
        .map(str::to_owned)
}

/// `url` with `limit=<limit>` added to its query, unless it has a limit.
pub fn with_limit(url: &Url, limit: usize) -> Url {
    if url.query_pairs().any(|(key, _)| key == "limit") {
        return url.clone();
    }
    let mut url = url.clone();
    url.query_pairs_mut()
        .append_pair("limit", &limit.to_string());
    url
}

/// A name for a link without a title: its file's name, or the folder's for
/// a `catalog.json` or `collection.json`.
fn name_from_href(href: &Url) -> String {
    let mut parts: Vec<_> = href
        .path_segments()
        .map(|segments| segments.filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let last = parts.pop().unwrap_or_default();
    let generic = matches!(
        last.to_ascii_lowercase().as_str(),
        "catalog.json" | "collection.json"
    );
    let name = match (generic, parts.pop()) {
        (true, Some(folder)) => folder,
        _ => last.strip_suffix(".json").unwrap_or(last),
    };
    let decoded = percent_decode(name);
    if decoded.is_empty() {
        href.to_string()
    } else {
        decoded
    }
}

fn percent_decode(text: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", text.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| text.to_owned())
}

/// What `doc`, read from `url`, lists. Fails only if it isn't STAC at all.
pub(crate) fn parse_document(url: &Url, doc: &Value) -> Result<Parsed, StacError> {
    if !is_stac(doc) {
        return Err(StacError::new(format!(
            "{url} isn't a STAC catalog, collection or item"
        )));
    }
    let links = links(url, doc);
    let next_page = first_link(&links, "next").map(|link| link.href.clone());

    // An item on its own.
    if string(doc, "type") == Some("Feature") {
        return Ok(Parsed {
            entries: vec![Entry::Item(parse_item(url, doc))],
            ..Parsed::default()
        });
    }
    // A page of items: an API's, or a static item collection.
    if let Some(features) = doc.get("features").and_then(Value::as_array) {
        let entries = features
            .iter()
            .filter(|feature| feature.is_object())
            .map(|feature| Entry::Item(parse_item(url, feature)))
            .collect();
        return Ok(Parsed {
            entries,
            next_page,
            ..Parsed::default()
        });
    }
    // An API's collections.
    if let Some(collections) = doc.get("collections").and_then(Value::as_array) {
        let entries = collections
            .iter()
            .filter_map(|collection| collection_node(url, collection))
            .map(Entry::Node)
            .collect();
        return Ok(Parsed {
            entries,
            next_page,
            ..Parsed::default()
        });
    }

    let children: Vec<_> = links
        .iter()
        .filter(|link| link.rel == "child")
        .map(|link| {
            Entry::Node(Node {
                title: link
                    .title
                    .clone()
                    .unwrap_or_else(|| name_from_href(&link.href)),
                kind: "Catalog".to_owned(),
                url: link.href.to_string(),
            })
        })
        .collect();
    let item_links: Vec<_> = links
        .iter()
        .filter(|link| link.rel == "item")
        .map(|link| link.href.clone())
        .collect();

    // An API landing page lists its collections at its `data` link.
    if is_api(doc)
        && let Some(data) = first_link(&links, "data")
    {
        return Ok(Parsed {
            follow: Some(data.href.clone()),
            ..Parsed::default()
        });
    }
    // An API collection pages through its items at its `items` link.
    if item_links.is_empty()
        && children.is_empty()
        && let Some(items) = first_link(&links, "items")
    {
        return Ok(Parsed {
            follow: Some(with_limit(&items.href, super::PAGE_LIMIT)),
            ..Parsed::default()
        });
    }
    Ok(Parsed {
        entries: children,
        item_links,
        next_page,
        follow: None,
    })
}

/// A collection in an API's `/collections`, browsing straight to its items
/// where it links to them.
fn collection_node(base: &Url, collection: &Value) -> Option<Node> {
    let links = links(base, collection);
    let items = first_link(&links, "items").map(|link| with_limit(&link.href, super::PAGE_LIMIT));
    let url = items
        .or_else(|| first_link(&links, "self").map(|link| link.href.clone()))
        .or_else(|| {
            let id = string(collection, "id")?;
            let mut base = base.clone();
            base.path_segments_mut().ok()?.pop_if_empty().push(id);
            Some(base)
        })?;
    Some(Node {
        title: title(collection).unwrap_or_else(|| name_from_href(&url)),
        kind: "Collection".to_owned(),
        url: url.to_string(),
    })
}

/// An item, its asset hrefs resolved against its own `self` link if it has
/// one, else against `url`, the document it was in.
pub(crate) fn parse_item(url: &Url, feature: &Value) -> Item {
    let base = first_link(&links(url, feature), "self")
        .map(|link| link.href.clone())
        .unwrap_or_else(|| url.clone());
    let properties = feature.get("properties").unwrap_or(&Value::Null);
    let id = string(feature, "id").unwrap_or("untitled item").to_owned();
    let title = string(properties, "title")
        .filter(|title| *title != id)
        .map(str::to_owned);
    let datetime = string(properties, "datetime")
        .or_else(|| string(properties, "start_datetime"))
        .map(str::to_owned);
    let thumbnail = feature
        .get("assets")
        .and_then(Value::as_object)
        .and_then(|assets| thumbnail(&base, assets));
    let assets = feature
        .get("assets")
        .and_then(Value::as_object)
        .map(|assets| {
            assets
                .iter()
                .filter_map(|(key, asset)| {
                    let href = string(asset, "href")?;
                    let media_type = string(asset, "type").map(str::to_owned);
                    Some(Asset {
                        key: key.clone(),
                        title: string(asset, "title").map(str::to_owned),
                        kind: classify(media_type.as_deref(), href),
                        media_type,
                        href: resolve_href(&base, href),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Item {
        id,
        title,
        datetime,
        assets,
        thumbnail,
    }
}

/// The URL of an item's thumbnail: the asset with the `thumbnail` role, else
/// the one keyed `thumbnail`, if it's a PNG or JPEG (by media type, or by
/// name when it has none) at an http or https URL.
pub(crate) fn thumbnail(base: &Url, assets: &serde_json::Map<String, Value>) -> Option<String> {
    let has_role = |asset: &Value| {
        asset
            .get("roles")
            .and_then(Value::as_array)
            .is_some_and(|roles| roles.iter().any(|role| role.as_str() == Some("thumbnail")))
    };
    let asset = assets
        .values()
        .find(|asset| has_role(asset))
        .or_else(|| assets.get("thumbnail"))?;
    let href = string(asset, "href")?;
    let picture = match string(asset, "type") {
        Some(media_type) => {
            let essence = media_type.split(';').next().unwrap_or("").trim();
            ["image/png", "image/jpeg", "image/jpg"]
                .iter()
                .any(|known| essence.eq_ignore_ascii_case(known))
        }
        None => {
            let path = href.split(['?', '#']).next().unwrap_or(href);
            let path = path.to_ascii_lowercase();
            [".png", ".jpg", ".jpeg"]
                .iter()
                .any(|extension| path.ends_with(extension))
        }
    };
    match resolve_href(base, href) {
        Href::Openable(url) if picture => Some(url),
        _ => None,
    }
}

/// What an asset with this media type and href holds.
pub(crate) fn classify(media_type: Option<&str>, href: &str) -> AssetKind {
    let copc_type = media_type.is_some_and(|media_type| {
        media_type
            .split(';')
            .next()
            .is_some_and(|essence| essence.trim().eq_ignore_ascii_case(COPC_MEDIA_TYPE))
    });
    let path = href.split(['?', '#']).next().unwrap_or(href);
    let copc_name = path.to_ascii_lowercase().ends_with(".copc.laz");
    if copc_type || copc_name {
        AssetKind::PointCloud
    } else {
        AssetKind::Other
    }
}

/// `href` resolved against `base`, and whether it can be opened.
pub(crate) fn resolve_href(base: &Url, href: &str) -> Href {
    let not_openable = |reason: String| Href::NotOpenable {
        href: href.to_owned(),
        reason,
    };
    match base.join(href) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => Href::Openable(url.to_string()),
        Ok(url) => not_openable(format!(
            "{}:// links can't be opened; it needs an http or https URL",
            url.scheme()
        )),
        Err(error) => not_openable(format!("its href can't be read ({error})")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    fn nodes(parsed: &Parsed) -> Vec<(&str, &str)> {
        parsed
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Node(node) => Some((node.title.as_str(), node.url.as_str())),
                _ => None,
            })
            .collect()
    }

    fn items(parsed: &Parsed) -> Vec<&Item> {
        parsed
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Item(item) => Some(item),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_static_catalog_should_list_children_and_item_links_resolved() {
        let doc = json!({
            "type": "Catalog", "id": "root", "stac_version": "1.1.0",
            "links": [
                {"rel": "self", "href": "./catalog.json"},
                {"rel": "root", "href": "./catalog.json"},
                {"rel": "child", "href": "./lidar/collection.json", "title": "Lidar"},
                {"rel": "child", "href": "sub/catalog.json"},
                {"rel": "item", "href": "./a/a.json"},
                {"rel": "license", "href": "https://example.com/licence"}
            ]
        });
        let parsed = parse_document(&url("https://example.com/stac/catalog.json"), &doc).unwrap();
        assert_eq!(
            nodes(&parsed),
            [
                ("Lidar", "https://example.com/stac/lidar/collection.json"),
                ("sub", "https://example.com/stac/sub/catalog.json"),
            ]
        );
        assert_eq!(
            parsed.item_links,
            [url("https://example.com/stac/a/a.json")]
        );
        assert_eq!(parsed.follow, None);
    }

    #[test]
    fn an_api_landing_page_should_follow_its_data_link() {
        let doc = json!({
            "type": "Catalog", "id": "api", "conformsTo": ["https://api.stacspec.org/v1.0.0/core"],
            "links": [
                {"rel": "data", "href": "https://example.com/collections"},
                {"rel": "search", "href": "https://example.com/search", "method": "POST"}
            ]
        });
        let parsed = parse_document(&url("https://example.com/"), &doc).unwrap();
        assert_eq!(parsed.follow, Some(url("https://example.com/collections")));
        assert!(is_api(&doc));
    }

    #[test]
    fn collections_should_browse_straight_to_their_items_with_a_limit() {
        let doc = json!({
            "collections": [
                {"id": "lidar", "title": "Lidar surveys", "links": [
                    {"rel": "self", "href": "https://example.com/collections/lidar"},
                    {"rel": "items", "href": "https://example.com/collections/lidar/items"}
                ]},
                {"id": "bare"}
            ],
            "links": [{"rel": "next", "href": "https://example.com/collections?page=2"}]
        });
        let parsed = parse_document(&url("https://example.com/collections"), &doc).unwrap();
        assert_eq!(
            nodes(&parsed),
            [
                (
                    "Lidar surveys",
                    "https://example.com/collections/lidar/items?limit=250"
                ),
                ("bare", "https://example.com/collections/bare"),
            ]
        );
        assert_eq!(
            parsed.next_page,
            Some(url("https://example.com/collections?page=2"))
        );
    }

    #[test]
    fn an_api_collection_should_follow_its_items_link() {
        let doc = json!({
            "type": "Collection", "id": "lidar",
            "links": [{"rel": "items", "href": "./lidar/items?limit=10"}]
        });
        let parsed = parse_document(&url("https://example.com/collections/"), &doc).unwrap();
        assert_eq!(
            parsed.follow,
            Some(url("https://example.com/collections/lidar/items?limit=10"))
        );
    }

    #[test]
    fn an_items_page_should_list_items_and_its_next_page() {
        let doc = json!({
            "type": "FeatureCollection",
            "features": [
                {"type": "Feature", "id": "a", "properties": {"datetime": "2026-04-14T02:00:00Z"},
                 "links": [{"rel": "self", "href": "https://example.com/items/a"}],
                 "assets": {"pc": {"href": "a.copc.laz"}}},
                "not an item"
            ],
            "links": [
                {"rel": "next", "href": "https://example.com/items?offset=1", "method": "GET"},
                {"rel": "prev", "href": "https://example.com/items?offset=0"}
            ]
        });
        let parsed = parse_document(&url("https://example.com/items?limit=1"), &doc).unwrap();
        let items = items(&parsed);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].date(), Some("2026-04-14"));
        // Resolved against the item's own self link.
        assert_eq!(
            items[0].assets[0].href,
            Href::Openable("https://example.com/items/a.copc.laz".to_owned())
        );
        assert_eq!(
            parsed.next_page,
            Some(url("https://example.com/items?offset=1"))
        );
    }

    #[test]
    fn a_next_link_needing_post_should_not_be_followed() {
        let doc = json!({
            "type": "FeatureCollection", "features": [],
            "links": [{"rel": "next", "href": "https://example.com/search", "method": "POST",
                       "body": {"token": "x"}}]
        });
        let parsed = parse_document(&url("https://example.com/search"), &doc).unwrap();
        assert_eq!(parsed.next_page, None);
    }

    #[test]
    fn json_that_isnt_stac_should_be_an_error() {
        for doc in [json!([1, 2]), json!("text"), json!({"name": "x"})] {
            let error = parse_document(&url("https://example.com/x.json"), &doc).unwrap_err();
            assert!(error.0.contains("isn't a STAC"), "{error}");
        }
    }

    #[test]
    fn items_should_tolerate_missing_and_wrong_fields() {
        let feature = json!({
            "type": "Feature", "properties": "nope",
            "assets": {"a": {"href": 3}, "b": {"title": "no href"}, "c": {"href": "c.tif", "type": 7}}
        });
        let item = parse_item(&url("https://example.com/i.json"), &feature);
        assert_eq!(item.id, "untitled item");
        assert_eq!(item.datetime, None);
        assert_eq!(item.assets.len(), 1);
        assert_eq!(item.assets[0].media_type, None);
    }

    #[test]
    fn item_titles_should_fall_back_to_the_id() {
        let item = parse_item(
            &url("https://example.com/i.json"),
            &json!({"id": "plot-1", "properties": {"title": "plot-1", "start_datetime": "2024-08-28"}}),
        );
        assert_eq!(item.title, None);
        assert_eq!(item.display_title(), "plot-1");
        assert_eq!(item.date(), Some("2024-08-28"));
    }

    #[test]
    fn assets_should_be_classified_by_media_type_or_name() {
        assert_eq!(
            classify(Some("application/vnd.laszip+copc"), "x.bin"),
            AssetKind::PointCloud
        );
        assert_eq!(
            classify(Some("Application/VND.laszip+copc; version=1.0"), "x"),
            AssetKind::PointCloud
        );
        assert_eq!(classify(None, "a/B.COPC.LAZ"), AssetKind::PointCloud);
        assert_eq!(
            classify(None, "https://x/a.copc.laz?sig=1"),
            AssetKind::PointCloud
        );
        assert_eq!(
            classify(Some("application/vnd.laszip"), "a.laz"),
            AssetKind::Other
        );
        assert_eq!(
            classify(Some("image/tiff; application=geotiff"), "a.tif"),
            AssetKind::Other
        );
    }

    #[test]
    fn hrefs_should_resolve_and_say_why_they_cant_be_opened() {
        let base = url("https://example.com/stac/a/a.json");
        assert_eq!(
            resolve_href(&base, "../../data/a.copc.laz"),
            Href::Openable("https://example.com/data/a.copc.laz".to_owned())
        );
        assert_eq!(
            resolve_href(&base, "https://other.example/x.copc.laz"),
            Href::Openable("https://other.example/x.copc.laz".to_owned())
        );
        let Href::NotOpenable { href, reason } = resolve_href(&base, "s3://bucket/x.copc.laz")
        else {
            panic!("s3 isn't openable");
        };
        assert_eq!(href, "s3://bucket/x.copc.laz");
        assert!(
            reason.starts_with("s3:// links can't be opened"),
            "{reason}"
        );
    }

    fn thumbnail_of(assets: Value) -> Option<String> {
        thumbnail(
            &url("https://example.com/stac/a/a.json"),
            assets.as_object().unwrap(),
        )
    }

    #[test]
    fn a_thumbnail_should_be_found_by_role_then_key() {
        let by_role = json!({
            "preview": {"href": "preview.png", "type": "image/png", "roles": ["thumbnail"]},
            "thumbnail": {"href": "other.png", "type": "image/png"}
        });
        assert_eq!(
            thumbnail_of(by_role).as_deref(),
            Some("https://example.com/stac/a/preview.png")
        );
        let by_key = json!({"thumbnail": {"href": "https://x.example/t.JPG?sig=1"}});
        assert_eq!(
            thumbnail_of(by_key).as_deref(),
            Some("https://x.example/t.JPG?sig=1")
        );
        assert_eq!(
            thumbnail_of(json!({"pc": {"href": "a.copc.laz", "roles": ["data"]}})),
            None
        );
    }

    #[test]
    fn a_thumbnail_should_be_a_png_or_jpeg_at_an_http_url() {
        let typed = |media_type: &str| {
            thumbnail_of(json!({"thumbnail": {"href": "t", "type": media_type}}))
        };
        assert!(typed("image/jpeg").is_some());
        assert!(typed("IMAGE/PNG; charset=binary").is_some());
        assert_eq!(typed("image/webp"), None);
        assert_eq!(typed("image/tiff; application=geotiff"), None);
        assert_eq!(
            thumbnail_of(json!({"thumbnail": {"href": "t.gif"}})),
            None,
            "an untyped picture that isn't PNG or JPEG by name"
        );
        assert_eq!(
            thumbnail_of(json!({"thumbnail": {"href": "s3://b/t.png", "type": "image/png"}})),
            None
        );
    }

    #[test]
    fn the_fixture_static_item_should_have_a_thumbnail() {
        let text = include_str!("../../tests/data/stac/static/site-a/site-a.json");
        let doc: Value = serde_json::from_str(text).unwrap();
        let item = parse_item(
            &url("http://127.0.0.1:1/stac/static/site-a/site-a.json"),
            &doc,
        );
        assert_eq!(
            item.thumbnail.as_deref(),
            Some("http://127.0.0.1:1/stac/static/site-a/site-a-thumbnail.png")
        );
    }

    #[test]
    fn with_limit_should_add_a_limit_once() {
        let limited = with_limit(&url("https://example.com/items?collections=a"), 50);
        assert_eq!(
            limited.as_str(),
            "https://example.com/items?collections=a&limit=50"
        );
        assert_eq!(with_limit(&limited, 10), limited);
    }

    #[test]
    fn untitled_links_should_be_named_from_their_href() {
        assert_eq!(
            name_from_href(&url("https://x/stac/site%201/collection.json")),
            "site 1"
        );
        assert_eq!(name_from_href(&url("https://x/stac/plot-7.json")), "plot-7");
    }

    #[test]
    fn the_fixture_static_item_should_have_its_point_cloud() {
        let text = include_str!("../../tests/data/stac/static/site-a/site-a.json");
        let doc: Value = serde_json::from_str(text).unwrap();
        let item = parse_item(
            &url("http://127.0.0.1:1/stac/static/site-a/site-a.json"),
            &doc,
        );
        let clouds: Vec<_> = item.point_clouds().collect();
        assert_eq!(clouds.len(), 1);
        assert_eq!(
            clouds[0].href,
            Href::Openable("http://127.0.0.1:1/fixture.copc.laz".to_owned())
        );
    }
}
