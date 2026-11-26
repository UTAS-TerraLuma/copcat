//! Data sources (connectors): places to browse to clouds from without
//! leaving the viewer, in quick open (Ctrl+P) and from the menu bar's
//! Open > From STAC….
//!
//! - A **STAC source** browses a STAC catalog or API ([`crate::stac`]):
//!   catalogs and collections, then items, then an item's COPC assets.
//! - A **folder source** lists every COPC file under a folder
//!   ([`crate::folder_scan`]).
//!
//! Sources come from the settings files' `connectors`, plus the workspace
//! folder as an implicit folder source. Listings are fetched on threads into
//! [`Feed`]s that [`ConnectorHub`] keeps for the session, so browsing back
//! into something shows it at once. STAC listings are also kept on disk
//! ([`crate::cache::stac`]) once fetched whole, so they show at once in the
//! next session too, while they're fetched again behind them. The commands
//! here add and remove sources in the user settings file, and clear the
//! cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::SystemTime;

use eframe::egui;
use serde_json::json;

use crate::cache::stac::CachedListing;
use crate::cache::{Caches, Clock, Removed, StacCache};
use crate::config::{self, ConnectorList, Connectors};
use crate::folder_scan::{self, Limits};
use crate::format::file_name;
use crate::stac::{self, Entry, Href, Item};

use super::ViewerApp;
use super::commands::{Command, ConnectorCommand};
use super::feed::{AUTO_LOAD_LIMIT, Chunk, Feed, FeedProvider, Load, More};
use super::open_ui::clean_source;
use super::palette::{
    Browse, Env, ItemAction, ListProvider, Listing, PaletteItem, PaletteMode, Provider,
};
use super::thumbnails;

/// What a data source browses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SourceKind {
    /// A STAC catalog or API, by URL.
    Stac(String),
    Folder(PathBuf),
}

impl SourceKind {
    fn list(&self) -> ConnectorList {
        match self {
            Self::Stac(_) => ConnectorList::Stac,
            Self::Folder(_) => ConnectorList::Folders,
        }
    }

    /// "STAC" or "Folder", and where.
    fn describe(&self) -> String {
        match self {
            Self::Stac(url) => format!("STAC source · {url}"),
            Self::Folder(path) => format!("Folder source · {}", path.display()),
        }
    }
}

/// A place to browse to clouds from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DataSource {
    pub(crate) name: String,
    pub(crate) kind: SourceKind,
}

/// A source to add to the user settings file.
pub(crate) type NewSource = DataSource;

/// The configured sources, STAC first, then the workspace folder, unless
/// it's too big to scan or a configured source has its name.
pub(crate) fn data_sources(connectors: &Connectors, workspace: Option<&Path>) -> Vec<DataSource> {
    let mut sources: Vec<_> = connectors
        .stac
        .iter()
        .map(|stac| DataSource {
            name: stac.name.clone(),
            kind: SourceKind::Stac(stac.url.clone()),
        })
        .chain(connectors.folders.iter().map(|folder| DataSource {
            name: folder.name.clone(),
            kind: SourceKind::Folder(folder.path.clone()),
        }))
        .collect();
    let home = config::home_dir();
    if let Some(workspace) = workspace.and_then(|dir| workspace_source(dir, home.as_deref()))
        && !sources.iter().any(|source| source.name == workspace.name)
    {
        sources.push(workspace);
    }
    sources
}

/// The workspace folder as a folder source, unless it's the home folder or
/// a drive or filesystem root, which would take too long to scan, or isn't
/// a folder.
pub(crate) fn workspace_source(workspace: &Path, home: Option<&Path>) -> Option<DataSource> {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let dir = canonical(workspace);
    let is_home = home.is_some_and(|home| canonical(home) == dir);
    if is_home || dir.parent().is_none() || !dir.is_dir() {
        return None;
    }
    let name = dir.file_name()?.to_string_lossy().into_owned();
    Some(DataSource {
        name: format!("Workspace: {name}"),
        kind: SourceKind::Folder(workspace.to_path_buf()),
    })
}

/// A name for a new STAC source: the catalog's title, or the URL's host.
pub(crate) fn default_stac_name(title: Option<&str>, url: &str) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            url::Url::parse(url)
                .ok()?
                .host_str()
                .map(|host| host.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_else(|| "STAC source".to_owned())
}

/// A name for a new folder source: the folder's name.
pub(crate) fn default_folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The palette row for a STAC item: opens its point cloud if it has one,
/// lists them if it has several, and is greyed out, saying why, if it has
/// none that can be opened.
pub(crate) fn item_row(item: &Item) -> PaletteItem {
    let clouds: Vec<_> = item.point_clouds().collect();
    let mut detail: Vec<String> = item.date().map(str::to_owned).into_iter().collect();
    if item.title.is_some() {
        detail.push(item.id.clone());
    }
    let title = item.display_title().to_owned();
    let row = match clouds.as_slice() {
        [] => {
            detail.push("no COPC point cloud".to_owned());
            disabled(title)
        }
        [cloud] => match &cloud.href {
            Href::Openable(url) => {
                PaletteItem::new(title, ItemAction::Run(Command::OpenSource(url.clone())))
            }
            Href::NotOpenable { reason, .. } => {
                detail.push(format!("its point cloud can't be opened: {reason}"));
                disabled(title)
            }
        },
        several => {
            detail.push(format!("{} point clouds", several.len()));
            let assets: Vec<_> = several
                .iter()
                .map(|asset| {
                    let name = asset.display_title().to_owned();
                    match &asset.href {
                        Href::Openable(url) => PaletteItem::new(
                            name,
                            ItemAction::Run(Command::OpenSource(url.clone())),
                        )
                        .with_detail(file_name(url)),
                        Href::NotOpenable { reason, .. } => disabled(name).with_detail(reason),
                    }
                })
                .collect();
            let list_title = title.clone();
            PaletteItem::new(
                title,
                ItemAction::Browse(Browse::new(move || {
                    Box::new(ListProvider {
                        title: list_title.clone(),
                        items: assets.clone(),
                    })
                })),
            )
        }
    };
    row.with_detail(detail.join(" · "))
        .with_thumbnail(item.thumbnail.clone().filter(|_| thumbnails::ENABLED))
}

/// A greyed-out row: a note, or something that can't be opened.
fn disabled(title: impl Into<String>) -> PaletteItem {
    PaletteItem {
        enabled: false,
        ..PaletteItem::new(title, ItemAction::Run(Command::ResetView))
    }
}

/// The rows for a folder scan of `root`.
pub(crate) fn folder_rows(root: &Path, scan: &folder_scan::Scan) -> Vec<PaletteItem> {
    let mut rows: Vec<_> = scan
        .files
        .iter()
        .map(|relative| {
            let path = root.join(relative).display().to_string();
            let name = relative
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let folder = relative
                .parent()
                .map(|parent| parent.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            PaletteItem::new(name, ItemAction::Run(Command::OpenSource(path))).with_detail(folder)
        })
        .collect();
    if scan.files.is_empty() && scan.not_listed == 0 {
        rows.push(disabled("No COPC files (*.copc.laz) in this folder"));
    }
    if scan.not_listed > 0 {
        rows.push(
            disabled(format!("{} more not shown", scan.not_listed)).with_detail(format!(
                "only the first {} are listed; add a source for a folder further in",
                scan.files.len()
            )),
        );
    }
    if scan.incomplete {
        rows.push(disabled("The search stopped early").with_detail(
            "some folders were too deep or too many to search; add a source further in",
        ));
    }
    rows
}

/// Makes the providers for data sources, keeping what they fetch for the
/// session, and STAC listings on disk if there's a cache. Cheap to clone;
/// the clones share it.
#[derive(Clone)]
pub(crate) struct ConnectorHub {
    ctx: egui::Context,
    client: stac::Client,
    feeds: Arc<Mutex<HashMap<String, Arc<Feed>>>>,
    /// `None` when the viewer has no settings directory (as in tests).
    caches: Option<Caches>,
    clock: Clock,
}

/// What a new feed shows until it's fetched: rows, and when they were
/// fetched.
type Seed = Option<(Vec<PaletteItem>, SystemTime)>;

impl ConnectorHub {
    pub(crate) fn new(ctx: egui::Context, caches: Option<Caches>, clock: Clock) -> Self {
        if let Some(caches) = caches.clone() {
            // Old and excess files from earlier sessions; nothing waits on it.
            std::thread::spawn(move || caches.prune());
        }
        Self {
            ctx,
            client: stac::Client::new(),
            feeds: Arc::default(),
            caches,
            clock,
        }
    }

    fn stac_cache(&self) -> Option<&StacCache> {
        self.caches.as_ref().map(|caches| &caches.stac)
    }

    fn feeds(&self) -> MutexGuard<'_, HashMap<String, Arc<Feed>>> {
        self.feeds.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The provider listing the feed kept as `key`; if there's none yet,
    /// one made with `start`, showing `seed` until it's fetched.
    fn feed(
        &self,
        key: String,
        title: impl Into<String>,
        seed: impl FnOnce() -> Seed,
        start: impl Fn() -> Load + Send + Sync + 'static,
    ) -> Box<dyn Provider> {
        let feed = self
            .feeds()
            .entry(key)
            .or_insert_with(|| {
                let ctx = self.ctx.clone();
                let wake = Arc::new(move || ctx.request_repaint());
                let feed = Feed::with_options(wake, AUTO_LOAD_LIMIT, self.clock.clone(), start);
                if let Some((items, fetched)) = seed() {
                    feed.seed(items, fetched);
                }
                feed
            })
            .clone();
        Box::new(FeedProvider::new(title, feed))
    }

    /// The quick open row for each source, browsing into it.
    pub(crate) fn source_rows(&self, sources: &[DataSource]) -> Vec<PaletteItem> {
        sources
            .iter()
            .map(|source| {
                let hub = self.clone();
                let browsed = source.clone();
                PaletteItem::new(
                    source.name.clone(),
                    ItemAction::Browse(Browse::new(move || hub.browse(&browsed))),
                )
                .with_detail(source.kind.describe())
            })
            .collect()
    }

    /// What's at the root of `source`.
    pub(crate) fn browse(&self, source: &DataSource) -> Box<dyn Provider> {
        match &source.kind {
            SourceKind::Stac(url) => self.stac_listing(url, None, &source.name),
            SourceKind::Folder(path) => {
                let path = path.clone();
                self.feed(
                    format!("folder:{}", path.display()),
                    &source.name,
                    || None,
                    move || {
                        let path = path.clone();
                        Box::new(move || {
                            let scan =
                                folder_scan::scan(&path, Limits::default()).map_err(|error| {
                                    format!("couldn't read {}: {error}", path.display())
                                })?;
                            Ok(Chunk::all(folder_rows(&path, &scan)))
                        })
                    },
                )
            }
        }
    }

    /// A listing in the STAC source at `source`: its root, or the node at
    /// `node`. Shows the listing kept on disk at once, if there is one.
    fn stac_listing(&self, source: &str, node: Option<&str>, title: &str) -> Box<dyn Provider> {
        let source = source.to_owned();
        let node = node.map(str::to_owned);
        let listing = node.clone().unwrap_or_else(|| source.clone());
        let seed = || {
            let cached = self.stac_cache()?.load(&source, &listing)?;
            let rows = cached
                .entries
                .iter()
                .map(|entry| self.stac_row(&source, entry))
                .collect();
            Some((rows, cached.fetched))
        };
        let key = stac_key(&source, &listing);
        let hub = self.clone();
        let (source, node, listing) = (source.clone(), node.clone(), listing.clone());
        self.feed(key, title, seed, move || {
            let hub = hub.clone();
            let (source, node) = (source.clone(), node.clone());
            let recorder = Recorder::new(&hub, &source, &listing);
            Box::new(move || {
                let page = match &node {
                    Some(node) => hub.client.browse(node),
                    None => hub.client.browse_root(&source),
                }
                .map_err(|e| e.0)?;
                Ok(hub.stac_chunk(&source, page, recorder))
            })
        })
    }

    /// A page of STAC entries as rows, with how to fetch the rest.
    fn stac_chunk(&self, source: &str, page: stac::Page, recorder: Recorder) -> Chunk {
        recorder.add(&page);
        let items = page
            .entries
            .iter()
            .map(|entry| self.stac_row(source, entry))
            .collect();
        let more = page.more.map(|more| {
            let detail = match more.known_items() {
                Some(1) => "1 more item".to_owned(),
                Some(count) => format!("{count} more items"),
                None => "the next page".to_owned(),
            };
            let hub = self.clone();
            let source = source.to_owned();
            More {
                detail,
                load: Box::new(move || {
                    let page = hub.client.more(&more).map_err(|e| e.0)?;
                    Ok(hub.stac_chunk(&source, page, recorder))
                }),
            }
        });
        Chunk { items, more }
    }

    fn stac_row(&self, source: &str, entry: &Entry) -> PaletteItem {
        match entry {
            Entry::Node(node) => {
                let hub = self.clone();
                let source = source.to_owned();
                let (title, url) = (node.title.clone(), node.url.clone());
                PaletteItem::new(
                    node.title.clone(),
                    ItemAction::Browse(Browse::new(move || {
                        hub.stac_listing(&source, Some(&url), &title)
                    })),
                )
                .with_detail(node.kind.clone())
            }
            Entry::Item(item) => item_row(item),
            Entry::Unavailable { title, reason } => disabled(title.clone()).with_detail(reason),
        }
    }

    /// Forgets the STAC source at `url`: its listings on disk and this
    /// session.
    pub(crate) fn forget_stac_source(&self, url: &str) {
        let prefix = stac_key(url, "");
        self.feeds().retain(|key, _| !key.starts_with(&prefix));
        if let Some(cache) = self.stac_cache() {
            cache.remove_source(url);
        }
    }

    /// Empties the disk cache, and forgets what's been fetched this
    /// session. Returns what was removed from disk, listings then
    /// thumbnails, or `None` without a cache.
    pub(crate) fn clear_cache(&self) -> Option<(Removed, Removed)> {
        self.feeds().clear();
        let caches = self.caches.as_ref()?;
        Some((caches.stac.clear(), caches.thumbnails.clear()))
    }
}

/// The key of a STAC listing's feed: starting with the source, so a
/// source's feeds can be found to forget them.
fn stac_key(source: &str, listing: &str) -> String {
    format!("stac:{source}\n{listing}")
}

/// Collects a STAC listing's pages as they arrive, and writes them to the
/// disk cache once the listing is whole: every page fetched, or as many as
/// are fetched by themselves ([`AUTO_LOAD_LIMIT`]). Pages fetched after
/// that with "Load more…" aren't kept: the next session shows the first
/// [`AUTO_LOAD_LIMIT`]. A listing that fails part way isn't written, so the
/// cache never holds part of one.
#[derive(Clone)]
struct Recorder {
    cache: Option<StacCache>,
    clock: Clock,
    source: String,
    listing: String,
    /// `None` once written.
    entries: Arc<Mutex<Option<Vec<Entry>>>>,
}

impl Recorder {
    fn new(hub: &ConnectorHub, source: &str, listing: &str) -> Self {
        Self {
            cache: hub.stac_cache().cloned(),
            clock: hub.clock.clone(),
            source: source.to_owned(),
            listing: listing.to_owned(),
            entries: Arc::new(Mutex::new(Some(Vec::new()))),
        }
    }

    fn add(&self, page: &stac::Page) {
        let Some(cache) = &self.cache else {
            return;
        };
        let mut kept = self.entries.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(entries) = kept.as_mut() else {
            return;
        };
        entries.extend(page.entries.iter().cloned());
        let whole = page.more.is_none();
        if !whole && entries.len() < AUTO_LOAD_LIMIT {
            return;
        }
        let entries = kept.take().unwrap_or_default();
        drop(kept);
        cache.store(&CachedListing {
            source: self.source.clone(),
            listing: self.listing.clone(),
            fetched: (self.clock)(),
            entries,
            truncated: !whole,
        });
    }
}

/// Asks for a new STAC source's URL.
struct AddStacProvider {
    hub: ConnectorHub,
    taken: Vec<String>,
}

impl Provider for AddStacProvider {
    fn title(&self) -> String {
        "Add STAC source".to_owned()
    }

    fn hint(&self) -> String {
        "URL of a STAC catalog or API".to_owned()
    }

    fn items(&mut self, query: &str, _env: &Env<'_>) -> Listing {
        let url = clean_source(query).filter(|text| {
            url::Url::parse(text).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
        });
        let item = match url {
            Some(url) => {
                let hub = self.hub.clone();
                let taken = self.taken.clone();
                let given = url.clone();
                PaletteItem::new(
                    format!("Add {url}"),
                    ItemAction::Browse(Browse::new(move || {
                        Box::new(NameProvider::for_stac(&hub, given.clone(), taken.clone()))
                    })),
                )
            }
            None => disabled("Type the http or https URL of a STAC catalog or API"),
        };
        Listing {
            items: vec![item],
            ranked: true,
            ..Listing::default()
        }
    }
}

/// The default name for a source being added, once it's known.
enum DefaultName {
    Ready(String),
    /// Being read from the source, which also checks it's there.
    Fetching(mpsc::Receiver<Result<String, String>>),
    Failed(String),
}

/// Asks for a new source's name, offering a default.
struct NameProvider {
    kind: SourceKind,
    default: DefaultName,
    /// Names already used by sources of the same kind.
    taken: Vec<String>,
}

impl NameProvider {
    /// Fetches the catalog at `url` for its title, which also checks it's a
    /// STAC catalog.
    fn for_stac(hub: &ConnectorHub, url: String, taken: Vec<String>) -> Self {
        let (send, receive) = mpsc::channel();
        let client = hub.client.clone();
        let ctx = hub.ctx.clone();
        let fetched = url.clone();
        std::thread::spawn(move || {
            let named = client
                .describe(&fetched)
                .map(|info| default_stac_name(info.title.as_deref(), &fetched))
                .map_err(|error| error.0);
            let _ = send.send(named);
            ctx.request_repaint();
        });
        Self {
            kind: SourceKind::Stac(url),
            default: DefaultName::Fetching(receive),
            taken,
        }
    }

    fn for_folder(path: PathBuf, taken: Vec<String>) -> Self {
        Self {
            default: DefaultName::Ready(default_folder_name(&path)),
            kind: SourceKind::Folder(path),
            taken,
        }
    }
}

impl Provider for NameProvider {
    fn title(&self) -> String {
        match &self.kind {
            SourceKind::Stac(url) => format!("Name the STAC source {url}"),
            SourceKind::Folder(path) => format!("Name the folder source {}", path.display()),
        }
    }

    fn hint(&self) -> String {
        match &self.default {
            DefaultName::Ready(name) => format!("Name (Enter for \"{name}\")"),
            _ => "Name".to_owned(),
        }
    }

    fn items(&mut self, query: &str, _env: &Env<'_>) -> Listing {
        if let DefaultName::Fetching(receive) = &self.default {
            match receive.try_recv() {
                Ok(Ok(name)) => self.default = DefaultName::Ready(name),
                Ok(Err(error)) => self.default = DefaultName::Failed(error),
                Err(mpsc::TryRecvError::Empty) => return Listing::loading(),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.default = DefaultName::Failed("reading the catalog failed".to_owned());
                }
            }
        }
        let default = match &self.default {
            DefaultName::Ready(name) => name,
            DefaultName::Failed(error) => {
                return Listing {
                    error: Some(error.clone()),
                    ..Listing::default()
                };
            }
            DefaultName::Fetching(_) => return Listing::loading(),
        };
        let name = match query.trim() {
            "" => default.clone(),
            typed => typed.to_owned(),
        };
        let detail = if self.taken.contains(&name) {
            "replaces the source with that name".to_owned()
        } else {
            "saved in your user settings".to_owned()
        };
        let source = NewSource {
            name: name.clone(),
            kind: self.kind.clone(),
        };
        Listing {
            items: vec![
                PaletteItem::new(
                    format!("Save as \"{name}\""),
                    ItemAction::Run(Command::Connector(ConnectorCommand::Save(source))),
                )
                .with_detail(detail),
            ],
            ranked: true,
            ..Listing::default()
        }
    }
}

impl ViewerApp {
    /// The data sources: the settings' connectors and the workspace.
    fn read_sources(&self) -> Vec<DataSource> {
        let workspace = self
            .user_config
            .locations
            .as_ref()
            .and_then(|locations| locations.workspace.as_deref());
        data_sources(&self.user_config.connectors, workspace)
    }

    /// Lists the data sources in quick open, after the settings change.
    pub(super) fn refresh_connectors(&mut self) {
        self.sources = self.read_sources();
        let rows = self.connector_hub.source_rows(&self.sources);
        self.palette.set_connectors(rows);
    }

    /// Lists the data sources in the palette, each browsing into itself,
    /// then a row to add a STAC source, there even with no sources.
    pub(super) fn browse_sources(&mut self) {
        let mut items = self.connector_hub.source_rows(&self.sources);
        let hub = self.connector_hub.clone();
        let taken = self.source_names(ConnectorList::Stac);
        let add = Command::Connector(ConnectorCommand::AddStac);
        items.push(
            PaletteItem::new(
                add.title(),
                ItemAction::Browse(Browse::new(move || {
                    Box::new(AddStacProvider {
                        hub: hub.clone(),
                        taken: taken.clone(),
                    })
                })),
            )
            .with_detail("a STAC catalog or API, by URL"),
        );
        self.palette.open(PaletteMode::Open);
        self.palette.browse(Box::new(ListProvider {
            title: "Data sources".to_owned(),
            items,
        }));
    }

    pub(super) fn run_connector_command(&mut self, command: ConnectorCommand) {
        match command {
            ConnectorCommand::AddStac => {
                let provider = AddStacProvider {
                    hub: self.connector_hub.clone(),
                    taken: self.source_names(ConnectorList::Stac),
                };
                self.palette.open(PaletteMode::Open);
                self.palette.browse(Box::new(provider));
            }
            ConnectorCommand::AddFolder => {
                let Some(path) = rfd::FileDialog::new().pick_folder() else {
                    return;
                };
                let taken = self.source_names(ConnectorList::Folders);
                self.palette.open(PaletteMode::Open);
                self.palette
                    .browse(Box::new(NameProvider::for_folder(path, taken)));
            }
            ConnectorCommand::Remove => {
                let items = self.removable_rows();
                self.palette.open(PaletteMode::Open);
                self.palette.browse(Box::new(ListProvider {
                    title: "Remove data source".to_owned(),
                    items,
                }));
            }
            ConnectorCommand::Save(source) => self.save_source(source),
            ConnectorCommand::Delete(list, name) => self.delete_source(list, &name),
            ConnectorCommand::ClearCache => self.clear_source_cache(),
        }
    }

    fn source_names(&self, list: ConnectorList) -> Vec<String> {
        let connectors = &self.user_config.connectors;
        match list {
            ConnectorList::Stac => connectors.stac.iter().map(|c| c.name.clone()).collect(),
            ConnectorList::Folders => connectors.folders.iter().map(|c| c.name.clone()).collect(),
        }
    }

    /// The configured sources, those in the user file removable, those only
    /// in the workspace file greyed out.
    fn removable_rows(&self) -> Vec<PaletteItem> {
        let in_user_file = self
            .user_config
            .locations
            .as_ref()
            .and_then(config::Locations::user_file)
            .map(|file| config::read_file(&file).0.connectors)
            .unwrap_or_default();
        let user_names = data_sources(&in_user_file, None);
        let configured = data_sources(&self.user_config.connectors, None);
        let rows: Vec<_> = configured
            .into_iter()
            .map(|source| {
                let in_user = user_names
                    .iter()
                    .any(|user| user.name == source.name && user.kind.list() == source.kind.list());
                let detail = source.kind.describe();
                if in_user {
                    PaletteItem::new(
                        source.name.clone(),
                        ItemAction::Run(Command::Connector(ConnectorCommand::Delete(
                            source.kind.list(),
                            source.name,
                        ))),
                    )
                    .with_detail(detail)
                } else {
                    disabled(source.name)
                        .with_detail(format!("{detail} · in the workspace settings file"))
                }
            })
            .collect();
        if rows.is_empty() {
            return vec![disabled("There are no data sources in the settings")];
        }
        rows
    }

    fn save_source(&mut self, source: NewSource) {
        let Some(file) = self.user_settings_file() else {
            return;
        };
        let entry = match &source.kind {
            SourceKind::Stac(url) => json!({ "name": source.name, "url": url }),
            SourceKind::Folder(path) => {
                json!({ "name": source.name, "path": path.display().to_string() })
            }
        };
        match config::add_connector(&file, source.kind.list(), entry) {
            Ok(_) => {
                self.reload_config();
                self.user_config.inform(format!(
                    "Added the data source \"{}\" to {}",
                    source.name,
                    file.display()
                ));
            }
            Err(problem) => self
                .user_config
                .warn(format!("{}: {problem}", file.display())),
        }
    }

    fn delete_source(&mut self, list: ConnectorList, name: &str) {
        let Some(file) = self.user_settings_file() else {
            return;
        };
        let removed_url = self.sources.iter().find_map(|source| match &source.kind {
            SourceKind::Stac(url) if source.name == name && list == ConnectorList::Stac => {
                Some(url.clone())
            }
            _ => None,
        });
        match config::remove_connector(&file, list, name) {
            Ok(_) => {
                self.reload_config();
                // Its cached listings go too, unless a source left (in the
                // workspace file) browses the same URL.
                if let Some(url) = removed_url
                    && !self
                        .sources
                        .iter()
                        .any(|source| source.kind == SourceKind::Stac(url.clone()))
                {
                    self.connector_hub.forget_stac_source(&url);
                }
                self.user_config.inform(format!(
                    "Removed the data source \"{name}\" from {}",
                    file.display()
                ));
            }
            Err(problem) => self
                .user_config
                .warn(format!("{}: {problem}", file.display())),
        }
    }

    /// Empties the data source cache, saying what went.
    fn clear_source_cache(&mut self) {
        let message = match self.connector_hub.clear_cache() {
            Some((listings, thumbnails)) => {
                let count = |n: usize, one: &str, many: &str| match n {
                    1 => format!("1 {one}"),
                    n => format!("{n} {many}"),
                };
                format!(
                    "Cleared the data source cache: {} and {} ({:.1} MB)",
                    count(listings.files, "list", "lists"),
                    count(thumbnails.files, "thumbnail", "thumbnails"),
                    (listings.bytes + thumbnails.bytes) as f64 / (1024.0 * 1024.0)
                )
            }
            None => "Forgot the data source lists fetched this session (there's no cache on disk \
                     in this session)"
                .to_owned(),
        };
        self.user_config.inform(message);
    }

    /// The user settings file, warning if there's none.
    fn user_settings_file(&mut self) -> Option<PathBuf> {
        let file = self
            .user_config
            .locations
            .as_ref()
            .and_then(config::Locations::user_file);
        if file.is_none() {
            self.user_config
                .warn("there's no user settings file in this session to save data sources to");
        }
        file
    }

    /// Reads the settings files again, as Reload settings does.
    fn reload_config(&mut self) {
        let ctx = self.connector_hub.ctx.clone();
        self.user_config.messages.clear();
        self.load_user_config(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{FolderConnector, StacConnector};
    use crate::stac::{Asset, AssetKind};

    use super::*;

    fn asset(key: &str, href: Href) -> Asset {
        Asset {
            key: key.to_owned(),
            title: None,
            media_type: Some("application/vnd.laszip+copc".to_owned()),
            kind: AssetKind::PointCloud,
            href,
        }
    }

    fn item(assets: Vec<Asset>) -> Item {
        Item {
            id: "plot-1".to_owned(),
            title: None,
            datetime: Some("2026-04-14T02:00:00Z".to_owned()),
            assets,
            thumbnail: None,
        }
    }

    fn openable(url: &str) -> Href {
        Href::Openable(url.to_owned())
    }

    #[test]
    fn an_item_with_one_point_cloud_should_open_it() {
        let row = item_row(&item(vec![asset("pc", openable("https://x/a.copc.laz"))]));
        assert!(row.enabled);
        assert_eq!(row.title, "plot-1");
        assert_eq!(row.detail, "2026-04-14");
        assert!(matches!(
            row.action,
            ItemAction::Run(Command::OpenSource(url)) if url == "https://x/a.copc.laz"
        ));
    }

    #[test]
    fn an_item_with_several_point_clouds_should_list_them() {
        let row = item_row(&item(vec![
            asset("classified", openable("https://x/c.copc.laz")),
            asset(
                "s3",
                Href::NotOpenable {
                    href: "s3://b/n.copc.laz".to_owned(),
                    reason: "no s3".to_owned(),
                },
            ),
        ]));
        assert!(row.enabled);
        assert_eq!(row.detail, "2026-04-14 · 2 point clouds");
        let ItemAction::Browse(Browse(make)) = row.action else {
            panic!("it should browse into its assets");
        };
        let keymap = crate::app::commands::Keymap::default();
        let env = Env {
            can_run: &|_| true,
            keymap: &keymap,
        };
        let listing = make().items("", &env);
        let rows: Vec<_> = listing
            .items
            .iter()
            .map(|row| (row.title.as_str(), row.detail.as_str(), row.enabled))
            .collect();
        assert_eq!(
            rows,
            [("classified", "c.copc.laz", true), ("s3", "no s3", false)]
        );
    }

    #[test]
    fn an_item_without_a_point_cloud_should_be_greyed_out_saying_so() {
        let mut tif = asset("dsm", openable("https://x/dsm.tif"));
        tif.kind = AssetKind::Other;
        let mut titled = item(vec![tif]);
        titled.title = Some("Plot one".to_owned());
        let row = item_row(&titled);
        assert!(!row.enabled);
        assert_eq!(row.title, "Plot one");
        assert_eq!(row.detail, "2026-04-14 · plot-1 · no COPC point cloud");
    }

    #[test]
    fn folder_rows_should_name_files_and_note_whats_not_shown() {
        let scan = folder_scan::Scan {
            files: vec![
                PathBuf::from("a.copc.laz"),
                Path::new("site").join("b.copc.laz"),
            ],
            not_listed: 3,
            incomplete: false,
        };
        let rows = folder_rows(Path::new("data"), &scan);
        let shown: Vec<_> = rows
            .iter()
            .map(|row| (row.title.as_str(), row.detail.as_str(), row.enabled))
            .collect();
        assert_eq!(shown[0], ("a.copc.laz", "", true));
        assert_eq!(shown[1], ("b.copc.laz", "site", true));
        assert_eq!(shown[2].0, "3 more not shown");
        assert!(!shown[2].2);
        let ItemAction::Run(Command::OpenSource(path)) = &rows[1].action else {
            panic!("a file row opens the file");
        };
        assert_eq!(
            Path::new(path),
            Path::new("data").join("site").join("b.copc.laz")
        );
    }

    #[test]
    fn an_empty_folder_should_say_so() {
        let rows = folder_rows(Path::new("data"), &folder_scan::Scan::default());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].enabled);
    }

    #[test]
    fn stac_names_should_default_to_the_title_then_the_host() {
        assert_eq!(
            default_stac_name(Some(" NatureScan STAC API "), "https://x.example/"),
            "NatureScan STAC API"
        );
        assert_eq!(
            default_stac_name(None, "https://www.stac.example/catalog.json"),
            "stac.example"
        );
        assert_eq!(default_stac_name(Some(""), "nonsense"), "STAC source");
    }

    fn recorder(cache: &StacCache) -> Recorder {
        Recorder {
            cache: Some(cache.clone()),
            clock: crate::cache::system_clock(),
            source: "https://s.example/".to_owned(),
            listing: "https://s.example/items".to_owned(),
            entries: Arc::new(Mutex::new(Some(Vec::new()))),
        }
    }

    fn page(ids: std::ops::Range<usize>, more: bool) -> stac::Page {
        stac::Page {
            entries: ids
                .map(|id| Entry::Unavailable {
                    title: format!("{id}"),
                    reason: String::new(),
                })
                .collect(),
            more: more.then(stac::More::default),
        }
    }

    #[test]
    fn a_listing_should_be_cached_only_once_whole() {
        let temp = tempfile::tempdir().unwrap();
        let cache = StacCache::new(temp.path().to_path_buf(), crate::cache::system_clock());
        let load = || cache.load("https://s.example/", "https://s.example/items");
        let recorder = recorder(&cache);
        recorder.add(&page(0..2, true));
        assert_eq!(load(), None, "part of a list isn't kept");
        recorder.add(&page(2..3, false));
        let kept = load().expect("the whole list");
        assert_eq!(kept.entries.len(), 3);
        assert!(!kept.truncated);
    }

    #[test]
    fn a_listing_should_be_cached_at_the_limit_and_not_again_after_it() {
        let temp = tempfile::tempdir().unwrap();
        let cache = StacCache::new(temp.path().to_path_buf(), crate::cache::system_clock());
        let recorder = recorder(&cache);
        recorder.add(&page(0..600, true));
        recorder.add(&page(600..1200, true));
        let load = || cache.load("https://s.example/", "https://s.example/items");
        let kept = load().expect("as much as is fetched by itself");
        assert_eq!(kept.entries.len(), 1200);
        assert!(kept.truncated);
        // "Load more…" past the limit doesn't grow it.
        recorder.add(&page(1200..1300, false));
        assert_eq!(load().unwrap().entries.len(), 1200);
    }

    #[test]
    fn folder_names_should_default_to_the_folders_name() {
        assert_eq!(default_folder_name(Path::new("/data/surveys")), "surveys");
    }

    #[test]
    fn the_workspace_should_be_a_source_unless_its_home_or_a_root() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let source = workspace_source(&project, None).unwrap();
        assert_eq!(source.name, "Workspace: project");
        assert_eq!(source.kind, SourceKind::Folder(project.clone()));
        assert_eq!(workspace_source(&project, Some(&project)), None);
        let root = project.ancestors().last().unwrap();
        assert_eq!(workspace_source(root, None), None);
        assert_eq!(workspace_source(&temp.path().join("missing"), None), None);
    }

    #[test]
    fn data_sources_should_list_stac_then_folders_then_the_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let connectors = Connectors {
            stac: vec![StacConnector {
                name: "S".to_owned(),
                url: "https://s.example/".to_owned(),
            }],
            folders: vec![FolderConnector {
                name: "F".to_owned(),
                path: PathBuf::from("f"),
            }],
        };
        let names: Vec<_> = data_sources(&connectors, Some(temp.path()))
            .into_iter()
            .map(|source| source.name)
            .collect();
        let workspace = format!(
            "Workspace: {}",
            temp.path()
                .canonicalize()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy()
        );
        assert_eq!(names, ["S".to_owned(), "F".to_owned(), workspace]);
    }
}
