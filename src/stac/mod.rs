//! A small STAC client: enough to browse a catalog down to the point clouds
//! in it. Knows nothing about egui; it blocks, so run it on a thread.
//!
//! Two kinds of source are supported, told apart by what the first document
//! says rather than by configuration:
//!
//! - A **static catalog**: JSON files linked by `child` and `item` links,
//!   with relative hrefs resolved against the document they're in. A source
//!   URL may name the catalog's JSON or the folder it's in, in which case
//!   `catalog.json` in that folder is tried.
//! - A **STAC API**: a landing page whose `data` link lists collections,
//!   each with an `items` link that pages through its items with `limit` and
//!   `next` links.
//!
//! Either way, browsing a URL gives a [`Page`] of [`Entry`]s: nodes to
//! browse into (catalogs, collections), items with their assets, and items
//! that couldn't be read. A page may have [`More`], fetched with
//! [`Client::more`]: the next page of an API's items, or the next batch of a
//! static catalog's item files. (The viewer fetches those by itself up to a
//! limit; see `app::feed`.)

mod client;
mod parse;

pub use client::{Client, ITEM_BATCH, PAGE_LIMIT};
pub use parse::with_limit;

/// What browsing a STAC URL found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page {
    pub entries: Vec<Entry>,
    /// More entries, to fetch with [`Client::more`] when asked for.
    pub more: Option<More>,
}

/// What's left to fetch of a listing: a static catalog's item files not
/// fetched yet, then the next page.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct More {
    pub(crate) item_links: Vec<url::Url>,
    pub(crate) next_page: Option<url::Url>,
}

impl More {
    /// `None` if there's nothing left.
    pub(crate) fn new(item_links: Vec<url::Url>, next_page: Option<url::Url>) -> Option<Self> {
        (!item_links.is_empty() || next_page.is_some()).then_some(Self {
            item_links,
            next_page,
        })
    }

    /// How many item files are known to be left, if that's all there is.
    pub fn known_items(&self) -> Option<usize> {
        self.next_page.is_none().then_some(self.item_links.len())
    }
}

/// One thing in a listing.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A catalog or collection to browse into.
    Node(Node),
    Item(Item),
    /// An item whose document couldn't be fetched or read.
    Unavailable {
        title: String,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub title: String,
    /// "Catalog", "Collection", or what the link says it is.
    pub kind: String,
    /// What to browse to list what's in it.
    pub url: String,
}

/// A STAC item: one survey, scene or tile, with its files.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    /// The item's `title` property, if it has one.
    pub title: Option<String>,
    /// When it was captured: `datetime`, or failing that `start_datetime`,
    /// as written.
    pub datetime: Option<String>,
    pub assets: Vec<Asset>,
}

impl Item {
    /// The title to show: its title, or its id.
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.id)
    }

    /// The date part of [`Item::datetime`], such as `2026-04-14`.
    pub fn date(&self) -> Option<&str> {
        let datetime = self.datetime.as_deref()?;
        Some(datetime.split(['T', ' ']).next().unwrap_or(datetime))
    }

    /// Its COPC point clouds, openable or not.
    pub fn point_clouds(&self) -> impl Iterator<Item = &Asset> {
        self.assets
            .iter()
            .filter(|asset| asset.kind == AssetKind::PointCloud)
    }
}

/// A file attached to an item.
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    /// Its key in the item's `assets`.
    pub key: String,
    pub title: Option<String>,
    pub media_type: Option<String>,
    pub kind: AssetKind,
    /// Where it is, resolved against the item's URL, and whether it can be
    /// opened from there.
    pub href: Href,
}

impl Asset {
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.key)
    }
}

/// What an asset holds, as far as Copcat cares. Rasters (COGs) will be
/// another kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    /// A Cloud-Optimized Point Cloud.
    PointCloud,
    /// Anything Copcat can't show.
    Other,
}

/// An asset's location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Href {
    /// An absolute `http` or `https` URL.
    Openable(String),
    /// Somewhere Copcat can't read from, such as `s3://`: the href as
    /// written, and why.
    NotOpenable { href: String, reason: String },
}

/// Something that stopped a listing.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct StacError(pub String);

impl StacError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// What a source says about itself, for naming it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    /// The document actually read: the URL given, or `catalog.json` in it.
    pub url: String,
    /// Its `title`, or failing that its `id`.
    pub title: Option<String>,
    /// Whether it's an API rather than a static catalog.
    pub is_api: bool,
}
