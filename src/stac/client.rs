//! Fetching STAC documents over blocking HTTP.

use std::thread;
use std::time::Duration;

use serde_json::Value;
use url::Url;

use super::parse::{self, Parsed};
use super::{Entry, More, Page, SourceInfo, StacError};

/// How many items an API is asked for per page.
pub const PAGE_LIMIT: usize = 50;
/// How many of a static catalog's item files are fetched per batch.
pub const ITEM_BATCH: usize = 24;
/// How many item files are fetched at once.
const PARALLEL_FETCHES: usize = 8;
/// The most a document may be: big enough for a page of items with long
/// asset lists, small enough that a wrong URL can't fill memory.
const MAX_DOCUMENT_BYTES: u64 = 32 * 1024 * 1024;
/// How many `follow`s (landing page to collections, collection to items) a
/// listing may take, so links in a loop can't spin forever.
const MAX_FOLLOWS: usize = 3;

/// Fetches and reads STAC documents. Cheap to clone; the clones share
/// connections.
#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(concat!("copcat/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent }
    }

    /// Fetches `url` as JSON.
    fn get_json(&self, url: &Url) -> Result<Value, StacError> {
        let mut response = self
            .agent
            .get(url.as_str())
            .header("Accept", "application/json, application/geo+json")
            .call()
            .map_err(|error| StacError::new(describe_http_error(url, &error)))?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_DOCUMENT_BYTES)
            .read_to_vec()
            .map_err(|error| match error {
                ureq::Error::BodyExceedsLimit(_) => StacError::new(format!(
                    "{url} is bigger than {} MB, too big for a STAC document",
                    MAX_DOCUMENT_BYTES / (1024 * 1024)
                )),
                error => StacError::new(describe_http_error(url, &error)),
            })?;
        serde_json::from_slice(&bytes)
            .map_err(|error| StacError::new(format!("{url} isn't JSON ({error})")))
    }

    /// The source's first document: `source` itself if it's STAC, else
    /// `catalog.json` in it, for a static catalog given by its folder.
    fn root(&self, source: &str) -> Result<(Url, Value), StacError> {
        let url = Url::parse(source.trim())
            .map_err(|error| StacError::new(format!("{source:?} isn't a URL ({error})")))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(StacError::new(format!(
                "{source} should be an http or https URL"
            )));
        }
        let first = self
            .get_json(&url)
            .and_then(|doc| match parse::is_stac(&doc) {
                true => Ok(doc),
                false => Err(StacError::new(format!(
                    "{url} isn't a STAC catalog, collection or item"
                ))),
            });
        let error = match first {
            Ok(doc) => return Ok((url, doc)),
            Err(error) => error,
        };
        let Some(catalog) = catalog_json_in(&url) else {
            return Err(error);
        };
        match self.get_json(&catalog) {
            Ok(doc) if parse::is_stac(&doc) => Ok((catalog, doc)),
            _ => Err(StacError::new(format!(
                "{error}; nor is there a catalog.json in it"
            ))),
        }
    }

    /// What the source at `source` calls itself, and where its document is.
    pub fn describe(&self, source: &str) -> Result<SourceInfo, StacError> {
        let (url, doc) = self.root(source)?;
        Ok(SourceInfo {
            url: url.to_string(),
            title: parse::title(&doc),
            is_api: parse::is_api(&doc),
        })
    }

    /// What's at the root of the source at `source`.
    pub fn browse_root(&self, source: &str) -> Result<Page, StacError> {
        let (url, doc) = self.root(source)?;
        self.list(url, doc)
    }

    /// What's at `url`: a node's listing, as [`super::Node::url`] gives.
    pub fn browse(&self, url: &str) -> Result<Page, StacError> {
        let url = Url::parse(url)
            .map_err(|error| StacError::new(format!("{url:?} isn't a URL ({error})")))?;
        let doc = self.get_json(&url)?;
        self.list(url, doc)
    }

    /// The rest of a listing, a batch or page at a time.
    pub fn more(&self, more: &More) -> Result<Page, StacError> {
        if !more.item_links.is_empty() {
            return Ok(self.item_batch(more.item_links.clone(), more.next_page.clone()));
        }
        match &more.next_page {
            Some(next) => self.browse(next.as_str()),
            None => Ok(Page::default()),
        }
    }

    fn list(&self, mut url: Url, mut doc: Value) -> Result<Page, StacError> {
        let mut parsed = parse::parse_document(&url, &doc)?;
        for _ in 0..MAX_FOLLOWS {
            let Some(follow) = parsed.follow.take() else {
                break;
            };
            url = follow;
            doc = self.get_json(&url)?;
            parsed = parse::parse_document(&url, &doc)?;
        }
        let Parsed {
            entries,
            item_links,
            next_page,
            follow,
        } = parsed;
        if follow.is_some() {
            return Err(StacError::new(format!(
                "{url} links on to more listings than expected"
            )));
        }
        let mut page = self.item_batch(item_links, next_page);
        page.entries.splice(0..0, entries);
        Ok(page)
    }

    /// The first [`ITEM_BATCH`] of `links` fetched as items, the rest left
    /// for later with `next_page` after them.
    fn item_batch(&self, mut links: Vec<Url>, next_page: Option<Url>) -> Page {
        let rest = links.split_off(links.len().min(ITEM_BATCH));
        let entries = self.fetch_items(&links);
        Page {
            entries,
            more: More::new(rest, next_page),
        }
    }

    /// Each item document at `links`, in order, a few at a time. One that
    /// can't be read is listed as unavailable rather than failing the rest.
    fn fetch_items(&self, links: &[Url]) -> Vec<Entry> {
        if links.is_empty() {
            return Vec::new();
        }
        let per_thread = links.len().div_ceil(PARALLEL_FETCHES);
        thread::scope(|scope| {
            let workers: Vec<_> = links
                .chunks(per_thread)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .map(|link| self.fetch_item(link))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .zip(links.chunks(per_thread))
                .flat_map(|(worker, chunk)| {
                    worker.join().unwrap_or_else(|_| {
                        chunk
                            .iter()
                            .map(|link| unavailable(link, "reading it failed"))
                            .collect()
                    })
                })
                .collect()
        })
    }

    fn fetch_item(&self, link: &Url) -> Entry {
        match self.get_json(link) {
            Ok(doc) => match doc.get("type").and_then(Value::as_str) {
                Some("Feature") => Entry::Item(parse::parse_item(link, &doc)),
                _ => unavailable(link, "it isn't a STAC item"),
            },
            Err(error) => unavailable(link, &error.0),
        }
    }
}

fn unavailable(link: &Url, reason: &str) -> Entry {
    let title = link
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|name| !name.is_empty())
        .unwrap_or(link.as_str())
        .to_owned();
    Entry::Unavailable {
        title,
        reason: reason.to_owned(),
    }
}

/// `catalog.json` in the folder `url` names, unless `url` already names a
/// JSON file.
fn catalog_json_in(url: &Url) -> Option<Url> {
    if url.path().to_ascii_lowercase().ends_with(".json") {
        return None;
    }
    let mut folder = url.clone();
    folder.set_query(None);
    folder.set_fragment(None);
    if !folder.path().ends_with('/') {
        let path = format!("{}/", folder.path());
        folder.set_path(&path);
    }
    folder.join("catalog.json").ok()
}

/// A request's failure, for people.
fn describe_http_error(url: &Url, error: &ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(code) => {
            let reason = match code {
                401 | 403 => " (it may need signing in, which isn't supported yet)",
                404 => " (not found)",
                _ => "",
            };
            format!("{url} replied HTTP {code}{reason}")
        }
        ureq::Error::Timeout(_) => format!("{url} took too long to reply"),
        ureq::Error::HostNotFound => format!(
            "couldn't find the host {}",
            url.host_str().unwrap_or("in the URL")
        ),
        error => format!("couldn't fetch {url}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_json_should_be_tried_in_a_folder() {
        let folder = Url::parse("https://example.com/v1/AUTH_x/workshop").unwrap();
        assert_eq!(
            catalog_json_in(&folder).unwrap().as_str(),
            "https://example.com/v1/AUTH_x/workshop/catalog.json"
        );
        let slash = Url::parse("https://example.com/stac/?x=1").unwrap();
        assert_eq!(
            catalog_json_in(&slash).unwrap().as_str(),
            "https://example.com/stac/catalog.json"
        );
        let file = Url::parse("https://example.com/stac/catalog.json").unwrap();
        assert_eq!(catalog_json_in(&file), None);
    }

    #[test]
    fn root_should_reject_urls_it_cant_fetch_from() {
        let client = Client::new();
        assert!(
            client
                .browse_root("not a url")
                .unwrap_err()
                .0
                .contains("isn't a URL")
        );
        assert!(
            client
                .browse_root("ftp://example.com/catalog.json")
                .unwrap_err()
                .0
                .contains("http or https")
        );
    }
}
