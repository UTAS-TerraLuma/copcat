//! Palette items fetched in the background, a page at a time: a [`Feed`]
//! holds what's been fetched so far and fetches more on a thread when
//! asked; a [`FeedProvider`] lists it in the palette, with "Load more…" and
//! "Refresh" items.
//!
//! A feed outlives the provider showing it, so a connector can keep its
//! feeds for the session: browsing back into a catalog shows what was
//! fetched before, at once, rather than fetching it again.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::palette::{Env, ItemAction, Listing, LocalAction, PaletteItem, Provider};

/// What one fetch found: some items, and how to fetch the next ones.
pub(crate) struct Chunk {
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) more: Option<More>,
}

impl Chunk {
    /// Everything there is.
    pub(crate) fn all(items: Vec<PaletteItem>) -> Self {
        Self { items, more: None }
    }
}

/// A fetch, run on a thread: the items, or why there are none.
pub(crate) type Load = Box<dyn FnOnce() -> Result<Chunk, String> + Send>;

/// The next fetch, and what it fetches, for the "Load more…" item.
pub(crate) struct More {
    pub(crate) detail: String,
    pub(crate) load: Load,
}

type Start = dyn Fn() -> Load + Send + Sync;

/// Called when a fetch is done, so the palette shows it: in the app, a
/// repaint request.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// Items fetched so far, shared between the threads fetching them and the
/// palette showing them. See the module docs.
pub(crate) struct Feed {
    /// Makes the first fetch, again on a refresh.
    start: Box<Start>,
    /// Called when a fetch is done.
    wake: Wake,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    started: bool,
    items: Vec<PaletteItem>,
    more: Option<More>,
    loading: bool,
    error: Option<String>,
    /// Bumped by a refresh, so a fetch from before it is dropped.
    generation: u64,
}

impl Feed {
    /// A feed whose first fetch `start` makes, calling `wake` as fetches
    /// finish. Nothing is fetched until [`Feed::start`].
    pub(crate) fn new(wake: Wake, start: impl Fn() -> Load + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            start: Box::new(start),
            wake,
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic on a fetch thread happens outside the lock; the state is
        // always whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Fetches the first items, unless that's been done.
    pub(crate) fn start(self: &Arc<Self>) {
        let mut state = self.state();
        if !state.started {
            state.started = true;
            self.spawn(&mut state, (self.start)());
        }
    }

    /// Forgets what was fetched, and fetches it again.
    pub(crate) fn refresh(self: &Arc<Self>) {
        let mut state = self.state();
        state.generation += 1;
        state.items.clear();
        state.more = None;
        state.error = None;
        state.started = true;
        self.spawn(&mut state, (self.start)());
    }

    /// Fetches the next items, if there are more and nothing's being
    /// fetched.
    pub(crate) fn load_more(self: &Arc<Self>) {
        let mut state = self.state();
        if state.loading {
            return;
        }
        if let Some(more) = state.more.take() {
            self.spawn(&mut state, more.load);
        }
    }

    fn spawn(self: &Arc<Self>, state: &mut State, load: Load) {
        state.loading = true;
        let generation = state.generation;
        let feed = Arc::clone(self);
        std::thread::spawn(move || {
            // Caught so a bug in a fetch shows as an error rather than a
            // spinner that never stops.
            let fetched = std::panic::catch_unwind(std::panic::AssertUnwindSafe(load))
                .unwrap_or_else(|_| Err("fetching failed unexpectedly".to_owned()));
            let mut state = feed.state();
            if state.generation != generation {
                return;
            }
            state.loading = false;
            match fetched {
                Ok(chunk) => {
                    state.items.extend(chunk.items);
                    state.more = chunk.more;
                }
                Err(error) => state.error = Some(error),
            }
            drop(state);
            (feed.wake)();
        });
    }

    /// What's been fetched, with items to fetch more or again.
    fn listing(&self) -> Listing {
        let state = self.state();
        let mut items = state.items.clone();
        if !state.loading {
            if let Some(more) = &state.more {
                items.push(
                    PaletteItem::new("Load more…", ItemAction::Local(LocalAction::LoadMore))
                        .with_detail(more.detail.clone()),
                );
            }
            let refresh = if state.error.is_some() {
                "Try again"
            } else {
                "Refresh"
            };
            items.push(
                PaletteItem::new(refresh, ItemAction::Local(LocalAction::Refresh))
                    .with_detail("fetch this list again"),
            );
        }
        Listing {
            items,
            loading: state.loading,
            error: state.error.clone(),
            ranked: false,
        }
    }
}

/// Lists a [`Feed`] in the palette, starting it if it hasn't started.
pub(crate) struct FeedProvider {
    title: String,
    feed: Arc<Feed>,
}

impl FeedProvider {
    pub(crate) fn new(title: impl Into<String>, feed: Arc<Feed>) -> Self {
        feed.start();
        Self {
            title: title.into(),
            feed,
        }
    }
}

impl Provider for FeedProvider {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn items(&mut self, _query: &str, _env: &Env<'_>) -> Listing {
        let listing = self.feed.listing();
        if listing.loading && listing.items.is_empty() {
            return Listing::loading();
        }
        listing
    }

    fn act(&mut self, action: LocalAction) {
        match action {
            LocalAction::Refresh => self.feed.refresh(),
            LocalAction::LoadMore => self.feed.load_more(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    use super::*;
    use crate::app::commands::{Command, Keymap};

    fn item(title: &str) -> PaletteItem {
        PaletteItem::new(title, ItemAction::Run(Command::ResetView))
    }

    fn titles(listing: &Listing) -> Vec<&str> {
        listing
            .items
            .iter()
            .map(|item| item.title.as_str())
            .collect()
    }

    /// A waker whose calls arrive on the channel returned.
    fn context() -> (Wake, mpsc::Receiver<()>) {
        let (send, woken) = mpsc::channel();
        let send = Mutex::new(send);
        let wake: Wake = Arc::new(move || {
            let _ = send.lock().unwrap().send(());
        });
        (wake, woken)
    }

    /// The listing once the fetch in flight has finished.
    fn settled(provider: &mut FeedProvider, (_, woken): &(Wake, mpsc::Receiver<()>)) -> Listing {
        let keymap = Keymap::default();
        let env = Env {
            can_run: &|_| true,
            keymap: &keymap,
        };
        loop {
            let listing = provider.items("", &env);
            if !listing.loading {
                return listing;
            }
            woken
                .recv_timeout(Duration::from_secs(30))
                .expect("the fetch should finish");
        }
    }

    /// Pages of two items, numbered, three pages in all.
    fn paged(page: usize) -> Load {
        Box::new(move || {
            Ok(Chunk {
                items: vec![item(&format!("{page}a")), item(&format!("{page}b"))],
                more: (page < 3).then(|| More {
                    detail: format!("page {}", page + 1),
                    load: paged(page + 1),
                }),
            })
        })
    }

    #[test]
    fn a_feed_should_load_then_offer_more_then_refresh() {
        let waker = context();
        let wake = waker.0.clone();
        let feed = Feed::new(wake, || paged(1));
        let mut provider = FeedProvider::new("Items", feed);
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["1a", "1b", "Load more…", "Refresh"]);
        assert_eq!(listing.items[2].detail, "page 2");

        provider.act(LocalAction::LoadMore);
        provider.act(LocalAction::LoadMore); // Ignored while loading.
        let listing = settled(&mut provider, &waker);
        assert_eq!(
            titles(&listing),
            ["1a", "1b", "2a", "2b", "Load more…", "Refresh"]
        );

        provider.act(LocalAction::Refresh);
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["1a", "1b", "Load more…", "Refresh"]);
    }

    #[test]
    fn a_feed_should_be_fetched_once_for_every_provider_showing_it() {
        let waker = context();
        let wake = waker.0.clone();
        let fetches = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&fetches);
        let feed = Feed::new(wake, move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::new(|| Ok(Chunk::all(vec![item("only")])))
        });
        let mut first = FeedProvider::new("Items", Arc::clone(&feed));
        settled(&mut first, &waker);
        let mut second = FeedProvider::new("Items", feed);
        let listing = settled(&mut second, &waker);
        assert_eq!(titles(&listing), ["only", "Refresh"]);
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_fetch_should_show_why_and_offer_to_try_again() {
        let waker = context();
        let wake = waker.0.clone();
        let feed = Feed::new(wake, || Box::new(|| Err("HTTP 403".to_owned())));
        let mut provider = FeedProvider::new("Items", feed);
        let listing = settled(&mut provider, &waker);
        assert_eq!(listing.error.as_deref(), Some("HTTP 403"));
        assert_eq!(titles(&listing), ["Try again"]);
    }

    #[test]
    fn a_panicking_fetch_should_show_an_error() {
        let waker = context();
        let wake = waker.0.clone();
        let feed = Feed::new(wake, || Box::new(|| panic!("bug")));
        let mut provider = FeedProvider::new("Items", feed);
        let listing = settled(&mut provider, &waker);
        assert!(listing.error.is_some());
    }
}
