//! STAC listings kept between runs: what the palette lists for one level of
//! one data source (its root, a catalog or collection, a collection's
//! items), as read from the server, so browsing there again shows it at
//! once while it's fetched again.
//!
//! One JSON file per listing, `<source hash>-<listing hash>.json`, so a
//! source's files can be found by name when it's removed. A file holds the
//! entries as [`crate::stac`] reads them, keeping only what a row needs:
//! titles, kinds and links of catalogs and collections, and items' ids,
//! titles, dates, thumbnails and COPC assets (their other assets aren't
//! listed, so they're left out). Never point cloud data, and never the
//! STAC JSON itself, which is many times bigger.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use super::{Clock, DiskCache, Limits, MB, Removed, stable_hash};
use crate::stac::{Asset, AssetKind, Entry, Href, Item, Node};

/// Bumped when what's in a file changes; a file with another version is a
/// miss, and is overwritten by the next fetch.
pub(crate) const FORMAT: u64 = 1;

/// A listing of 1000 items is a few hundred KB.
pub(crate) const LIMITS: Limits = Limits {
    total: 20 * MB,
    file: 4 * MB,
    max_age: Duration::from_secs(30 * 24 * 60 * 60),
};

/// One listing, as fetched.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CachedListing {
    /// The data source's URL, as in the settings.
    pub(crate) source: String,
    /// What was listed: the source's URL for its root, or a node's URL.
    pub(crate) listing: String,
    pub(crate) fetched: SystemTime,
    pub(crate) entries: Vec<Entry>,
    /// The listing went on past these, beyond what's fetched by itself.
    pub(crate) truncated: bool,
}

/// The listings, in one folder. Cheap to clone.
#[derive(Clone)]
pub(crate) struct StacCache {
    disk: DiskCache,
}

impl StacCache {
    pub(crate) fn new(dir: PathBuf, clock: Clock) -> Self {
        Self::with_limits(dir, LIMITS, clock)
    }

    pub(crate) fn with_limits(dir: PathBuf, limits: Limits, clock: Clock) -> Self {
        Self {
            disk: DiskCache::new(dir, limits, clock),
        }
    }

    fn source_prefix(source: &str) -> String {
        format!("{}-", stable_hash(source))
    }

    fn file_name(source: &str, listing: &str) -> String {
        format!(
            "{}{}.json",
            Self::source_prefix(source),
            stable_hash(listing)
        )
    }

    /// The listing kept for `listing` in `source`, if there's a readable
    /// one of this version.
    pub(crate) fn load(&self, source: &str, listing: &str) -> Option<CachedListing> {
        let bytes = self.disk.read(&Self::file_name(source, listing))?;
        let doc: Value = serde_json::from_slice(&bytes).ok()?;
        let cached = from_json(&doc)?;
        // Guards against two URLs with the same hash.
        (cached.source == source && cached.listing == listing).then_some(cached)
    }

    /// Keeps `listing`, replacing what was kept for it. Returns whether it
    /// was written.
    pub(crate) fn store(&self, listing: &CachedListing) -> bool {
        let Ok(bytes) = serde_json::to_vec(&to_json(listing)) else {
            return false;
        };
        self.disk
            .write(&Self::file_name(&listing.source, &listing.listing), &bytes)
    }

    /// Removes every listing of `source`.
    pub(crate) fn remove_source(&self, source: &str) -> Removed {
        self.disk.remove_prefixed(&Self::source_prefix(source))
    }

    pub(crate) fn clear(&self) -> Removed {
        self.disk.clear()
    }

    pub(crate) fn prune(&self) -> Removed {
        self.disk.prune()
    }
}

fn seconds_since_epoch(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn to_json(listing: &CachedListing) -> Value {
    json!({
        "format": FORMAT,
        "source": listing.source,
        "listing": listing.listing,
        "fetched": seconds_since_epoch(listing.fetched),
        "truncated": listing.truncated,
        "entries": listing.entries.iter().map(entry_json).collect::<Vec<_>>(),
    })
}

fn entry_json(entry: &Entry) -> Value {
    match entry {
        Entry::Node(node) => json!({
            "node": { "title": node.title, "kind": node.kind, "url": node.url }
        }),
        Entry::Item(item) => json!({
            "item": {
                "id": item.id,
                "title": item.title,
                "datetime": item.datetime,
                "thumbnail": item.thumbnail,
                "clouds": item.point_clouds().map(asset_json).collect::<Vec<_>>(),
            }
        }),
        Entry::Unavailable { title, reason } => json!({
            "unavailable": { "title": title, "reason": reason }
        }),
    }
}

fn asset_json(asset: &Asset) -> Value {
    let mut value = json!({
        "key": asset.key,
        "title": asset.title,
        "media_type": asset.media_type,
    });
    match &asset.href {
        Href::Openable(href) => value["href"] = json!(href),
        Href::NotOpenable { href, reason } => {
            value["href"] = json!(href);
            value["not_openable"] = json!(reason);
        }
    }
    value
}

fn from_json(doc: &Value) -> Option<CachedListing> {
    if doc.get("format")?.as_u64()? != FORMAT {
        return None;
    }
    let text = |key: &str| doc.get(key)?.as_str().map(str::to_owned);
    let entries = doc
        .get("entries")?
        .as_array()?
        .iter()
        .map(entry_from_json)
        .collect::<Option<Vec<_>>>()?;
    Some(CachedListing {
        source: text("source")?,
        listing: text("listing")?,
        fetched: SystemTime::UNIX_EPOCH + Duration::from_secs(doc.get("fetched")?.as_u64()?),
        entries,
        truncated: doc.get("truncated")?.as_bool()?,
    })
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_owned)
}

/// A string, or `null` for none; anything else is corrupt.
fn optional(value: &Value, key: &str) -> Option<Option<String>> {
    match value.get(key)? {
        Value::Null => Some(None),
        Value::String(text) => Some(Some(text.clone())),
        _ => None,
    }
}

fn entry_from_json(value: &Value) -> Option<Entry> {
    if let Some(node) = value.get("node") {
        return Some(Entry::Node(Node {
            title: string(node, "title")?,
            kind: string(node, "kind")?,
            url: string(node, "url")?,
        }));
    }
    if let Some(item) = value.get("item") {
        let assets = item
            .get("clouds")?
            .as_array()?
            .iter()
            .map(asset_from_json)
            .collect::<Option<Vec<_>>>()?;
        return Some(Entry::Item(Item {
            id: string(item, "id")?,
            title: optional(item, "title")?,
            datetime: optional(item, "datetime")?,
            assets,
            thumbnail: optional(item, "thumbnail")?,
        }));
    }
    let unavailable = value.get("unavailable")?;
    Some(Entry::Unavailable {
        title: string(unavailable, "title")?,
        reason: string(unavailable, "reason")?,
    })
}

fn asset_from_json(value: &Value) -> Option<Asset> {
    let href = string(value, "href")?;
    let href = match optional(value, "not_openable").unwrap_or(None) {
        Some(reason) => Href::NotOpenable { href, reason },
        None => Href::Openable(href),
    };
    Some(Asset {
        key: string(value, "key")?,
        title: optional(value, "title")?,
        media_type: optional(value, "media_type")?,
        kind: AssetKind::PointCloud,
        href,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use super::super::tests::FakeClock;
    use super::*;

    fn cloud(key: &str, href: Href) -> Asset {
        Asset {
            key: key.to_owned(),
            title: Some(format!("{key} title")),
            media_type: Some("application/vnd.laszip+copc".to_owned()),
            kind: AssetKind::PointCloud,
            href,
        }
    }

    fn listing(source: &str, listing: &str, at: SystemTime) -> CachedListing {
        CachedListing {
            source: source.to_owned(),
            listing: listing.to_owned(),
            // Kept to the second.
            fetched: SystemTime::UNIX_EPOCH + Duration::from_secs(seconds_since_epoch(at)),
            entries: vec![
                Entry::Node(Node {
                    title: "Survey".to_owned(),
                    kind: "Collection".to_owned(),
                    url: "https://s.example/survey/collection.json".to_owned(),
                }),
                Entry::Item(Item {
                    id: "plot-1".to_owned(),
                    title: Some("Plot one".to_owned()),
                    datetime: Some("2026-04-14T02:00:00Z".to_owned()),
                    assets: vec![
                        cloud(
                            "pc",
                            Href::Openable("https://s.example/a.copc.laz".to_owned()),
                        ),
                        cloud(
                            "s3",
                            Href::NotOpenable {
                                href: "s3://b/a.copc.laz".to_owned(),
                                reason: "no s3".to_owned(),
                            },
                        ),
                    ],
                    thumbnail: Some("https://s.example/a.png".to_owned()),
                }),
                Entry::Item(Item {
                    id: "plot-2".to_owned(),
                    title: None,
                    datetime: None,
                    assets: Vec::new(),
                    thumbnail: None,
                }),
                Entry::Unavailable {
                    title: "broken.json".to_owned(),
                    reason: "HTTP 404".to_owned(),
                },
            ],
            truncated: false,
        }
    }

    fn cache(dir: &std::path::Path, clock: &Arc<FakeClock>) -> StacCache {
        StacCache::new(dir.join("stac"), clock.clock())
    }

    #[test]
    fn a_listing_should_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), &clock);
        let kept = listing("https://s.example/", "https://s.example/", clock.now());
        assert!(cache.store(&kept));
        assert_eq!(
            cache.load("https://s.example/", "https://s.example/"),
            Some(kept)
        );
        assert_eq!(cache.load("https://s.example/", "https://other/"), None);
    }

    #[test]
    fn only_point_cloud_assets_should_be_kept() {
        let mut kept = listing("s", "l", SystemTime::UNIX_EPOCH);
        let Entry::Item(item) = &mut kept.entries[1] else {
            panic!()
        };
        let mut raster = cloud(
            "dsm",
            Href::Openable("https://s.example/dsm.tif".to_owned()),
        );
        raster.kind = AssetKind::Other;
        item.assets.push(raster);
        let read = from_json(&to_json(&kept)).unwrap();
        let Entry::Item(item) = &read.entries[1] else {
            panic!()
        };
        assert_eq!(item.assets.len(), 2);
    }

    #[test]
    fn a_corrupt_or_other_version_file_should_be_a_miss_then_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), &clock);
        let kept = listing("s", "l", clock.now());
        let file = temp
            .path()
            .join("stac")
            .join(StacCache::file_name("s", "l"));
        fs::create_dir_all(file.parent().unwrap()).unwrap();

        for bad in [
            "{ not json".to_owned(),
            "".to_owned(),
            to_json(&kept)
                .to_string()
                .replace("\"format\":1", "\"format\":0"),
            to_json(&kept)
                .to_string()
                .replace("\"kind\":", "\"kinds\":"),
            "[1, 2]".to_owned(),
        ] {
            fs::write(&file, &bad).unwrap();
            assert_eq!(cache.load("s", "l"), None, "{bad}");
        }
        assert!(cache.store(&kept));
        assert_eq!(cache.load("s", "l"), Some(kept));
    }

    #[test]
    fn removing_a_source_should_remove_only_its_listings() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), &clock);
        for (source, list) in [("a", "a"), ("a", "a/x"), ("b", "b")] {
            assert!(cache.store(&listing(source, list, clock.now())));
        }
        assert_eq!(cache.remove_source("a").files, 2);
        assert_eq!(cache.load("a", "a"), None);
        assert_eq!(cache.load("a", "a/x"), None);
        assert!(cache.load("b", "b").is_some());
    }

    #[test]
    fn file_names_should_be_stable() {
        assert_eq!(
            StacCache::file_name("https://stacapi.naturescan.earth/", "x"),
            format!(
                "{}-{}.json",
                stable_hash("https://stacapi.naturescan.earth/"),
                stable_hash("x")
            )
        );
        assert_eq!(
            StacCache::file_name("", "a"),
            "cbf29ce484222325-af63dc4c8601ec8c.json"
        );
    }

    #[test]
    fn listings_should_expire_and_be_evicted_by_the_cap() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let one = serde_json::to_vec(&to_json(&listing("s", "0", clock.now())))
            .unwrap()
            .len() as u64;
        let limits = Limits {
            total: one * 2 + one / 2,
            file: one * 2,
            max_age: Duration::from_secs(100),
        };
        let cache = StacCache::with_limits(temp.path().join("stac"), limits, clock.clock());
        for list in ["0", "1", "2"] {
            assert!(cache.store(&listing("s", list, clock.now())));
            clock.advance(Duration::from_secs(1));
        }
        assert_eq!(cache.load("s", "0"), None, "the least recently used");
        assert!(cache.load("s", "1").is_some());
        clock.advance(Duration::from_secs(200));
        cache.prune();
        assert_eq!(cache.load("s", "2"), None, "expired");
    }
}
