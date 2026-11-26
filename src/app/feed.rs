//! Palette items fetched in the background, a page at a time: a [`Feed`]
//! holds what's been fetched so far and fetches more on a thread when
//! asked; a [`FeedProvider`] lists it in the palette, and the palette's
//! refresh button fetches it again.
//!
//! A feed keeps fetching page after page by itself until it has
//! [`AUTO_LOAD_LIMIT`] items, so a small catalog is listed (and filtered) in
//! full without asking; past that, a "Load more…" item fetches one page at a
//! time.
//!
//! A feed outlives the provider showing it, so a connector can keep its
//! feeds for the session: browsing back into a catalog shows what was
//! fetched before, at once, and fetches it again behind it if that was more
//! than [`REVALIDATE_AFTER`] ago.
//!
//! Fetching again over a list that's showing (a refresh, a revalidation, or
//! a list [`Feed::seed`]ed from the disk cache) keeps showing the old list
//! until the new one is whole, then swaps it in, so the list never empties
//! or fills in twice. If that fetch fails, the old list stays, with a note
//! saying how old it is.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use crate::cache::{self, Clock};

use super::palette::{Env, ItemAction, Listing, LocalAction, PaletteItem, Provider, Status};

/// What one fetch found: some items, and how to fetch the next ones.
pub(crate) struct Chunk {
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) more: Option<More>,
}

#[cfg(test)]
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

/// How many items a feed fetches by itself, page after page, before it waits
/// for "Load more…". Enough for the small catalogs Copcat is used with.
pub(crate) const AUTO_LOAD_LIMIT: usize = 1000;

/// How old a whole list may be before showing it again fetches it again
/// behind it. Short enough to pick up a new survey in the same session,
/// long enough that going back and forth doesn't refetch every time.
pub(crate) const REVALIDATE_AFTER: Duration = Duration::from_secs(5 * 60);

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
    /// Fetches the next page by itself while it has fewer items than this.
    auto_limit: usize,
    clock: Clock,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    started: bool,
    /// What's shown.
    items: Vec<PaletteItem>,
    more: Option<More>,
    loading: bool,
    error: Option<String>,
    /// Bumped by a refresh, so a fetch from before it is dropped.
    generation: u64,
    /// While fetching again over `items`: what's arrived so far, swapped in
    /// once it's whole.
    pending: Option<Vec<PaletteItem>>,
    /// When `items` were last fetched whole, or `None` while they're still
    /// arriving for the first time.
    fetched: Option<SystemTime>,
    /// Why fetching again over `items` failed.
    stale: Option<String>,
}

impl Feed {
    /// A feed whose first fetch `start` makes, calling `wake` as fetches
    /// finish. Nothing is fetched until [`Feed::start`].
    #[cfg(test)]
    pub(crate) fn new(wake: Wake, start: impl Fn() -> Load + Send + Sync + 'static) -> Arc<Self> {
        Self::with_options(wake, AUTO_LOAD_LIMIT, cache::system_clock(), start)
    }

    /// [`Feed::new`], fetching by itself until it has `auto_limit` items,
    /// and telling the time with `clock`.
    pub(crate) fn with_options(
        wake: Wake,
        auto_limit: usize,
        clock: Clock,
        start: impl Fn() -> Load + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            start: Box::new(start),
            wake,
            auto_limit,
            clock,
            state: Mutex::default(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic on a fetch thread happens outside the lock; the state is
        // always whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Shows `items`, fetched at `fetched` (from the disk cache), until the
    /// first fetch replaces them. Only before [`Feed::start`].
    pub(crate) fn seed(&self, items: Vec<PaletteItem>, fetched: SystemTime) {
        let mut state = self.state();
        if !state.started {
            state.items = items;
            state.fetched = Some(fetched);
        }
    }

    /// Fetches the first items, unless that's been done; fetches them again
    /// behind what's shown if that was more than [`REVALIDATE_AFTER`] ago.
    pub(crate) fn start(self: &Arc<Self>) {
        let mut state = self.state();
        if !state.started {
            state.started = true;
            if !state.items.is_empty() {
                state.pending = Some(Vec::new());
            }
            self.spawn(&mut state, (self.start)());
            return;
        }
        let old = state.fetched.is_some_and(|fetched| {
            (self.clock)().duration_since(fetched).unwrap_or_default() > REVALIDATE_AFTER
        });
        if old && !state.loading && state.error.is_none() {
            drop(state);
            self.refresh();
        }
    }

    /// Fetches the list again: behind what's shown, swapping it in once
    /// it's whole, or from empty if nothing's shown.
    pub(crate) fn refresh(self: &Arc<Self>) {
        let mut state = self.state();
        state.generation += 1;
        state.error = None;
        state.stale = None;
        if state.items.is_empty() {
            state.more = None;
            state.pending = None;
        } else {
            state.pending = Some(Vec::new());
        }
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
                Ok(chunk) => feed.arrived(&mut state, chunk),
                Err(error) => {
                    if state.pending.take().is_some() {
                        state.stale = Some(error);
                    } else {
                        state.error = Some(error);
                    }
                }
            }
            drop(state);
            (feed.wake)();
        });
    }

    /// Adds a fetched chunk, and fetches the next by itself under the
    /// limit; otherwise the list is whole (as far as it's fetched by
    /// itself), and replaces what was shown if it was fetched behind it.
    fn arrived(self: &Arc<Self>, state: &mut State, chunk: Chunk) {
        let count = match &mut state.pending {
            Some(pending) => {
                pending.extend(chunk.items);
                pending.len()
            }
            None => {
                state.items.extend(chunk.items);
                state.items.len()
            }
        };
        match chunk.more {
            Some(more) if count < self.auto_limit => self.spawn(state, more.load),
            more => {
                if let Some(pending) = state.pending.take() {
                    state.items = pending;
                }
                state.more = more;
                state.fetched = Some((self.clock)());
                state.stale = None;
            }
        }
    }

    /// What's shown, with an item to fetch more once the feed has stopped
    /// by itself.
    fn listing(&self) -> Listing {
        let state = self.state();
        let mut items = state.items.clone();
        if !state.loading
            && let Some(more) = &state.more
        {
            items.push(
                PaletteItem::new("Load more…", ItemAction::Local(LocalAction::LoadMore))
                    .with_detail(more.detail.clone()),
            );
        }
        Listing {
            items,
            loading: state.loading,
            error: state.error.clone(),
            ranked: false,
        }
    }

    /// What the palette says about the list beside its breadcrumb.
    fn status(&self) -> Option<Status> {
        let state = self.state();
        if state.loading {
            return Some(match state.pending {
                Some(_) => Status::Updating,
                None => Status::Loading(state.items.len()),
            });
        }
        let stale = state.stale.as_ref()?;
        let fetched = state.fetched?;
        Some(Status::Note {
            text: format!(
                "Offline: showing the list from {}",
                cache::ago((self.clock)(), fetched)
            ),
            hover: format!("Couldn't fetch it again: {stale}"),
        })
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

    fn can_refresh(&self) -> bool {
        true
    }

    fn fetching(&self) -> Option<usize> {
        let state = self.feed.state();
        state.loading.then_some(state.items.len())
    }

    fn status(&self) -> Option<Status> {
        self.feed.status()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;

    use super::*;
    use crate::app::commands::{Command, Keymap};
    use crate::cache::tests::FakeClock;

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
    fn a_feed_should_fetch_every_page_by_itself_under_its_limit() {
        let waker = context();
        let feed = Feed::new(waker.0.clone(), || paged(1));
        let mut provider = FeedProvider::new("Items", feed);
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["1a", "1b", "2a", "2b", "3a", "3b"]);
        assert_eq!(provider.fetching(), None);
    }

    #[test]
    fn a_feed_should_stop_at_its_limit_then_offer_more_then_refresh() {
        let waker = context();
        let feed = Feed::with_options(waker.0.clone(), 3, cache::system_clock(), || paged(1));
        let mut provider = FeedProvider::new("Items", feed);
        // Two pages make the three it fetches by itself.
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["1a", "1b", "2a", "2b", "Load more…"]);
        assert_eq!(listing.items[4].detail, "page 3");

        provider.act(LocalAction::LoadMore);
        provider.act(LocalAction::LoadMore); // Ignored while loading.
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["1a", "1b", "2a", "2b", "3a", "3b"]);

        assert!(provider.can_refresh());
        provider.act(LocalAction::Refresh);
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["1a", "1b", "2a", "2b", "Load more…"]);
    }

    #[test]
    fn a_feed_should_say_how_many_it_has_while_fetching_more() {
        let waker = context();
        let (release, gate) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let feed = Feed::new(waker.0.clone(), move || {
            let gate = Arc::clone(&gate);
            Box::new(move || {
                Ok(Chunk {
                    items: vec![item("first")],
                    more: Some(More {
                        detail: String::new(),
                        load: Box::new(move || {
                            gate.lock().unwrap().recv().unwrap();
                            Ok(Chunk::all(vec![item("second")]))
                        }),
                    }),
                })
            })
        });
        let mut provider = FeedProvider::new("Items", feed);
        let keymap = Keymap::default();
        let env = Env {
            can_run: &|_| true,
            keymap: &keymap,
        };
        // The first page has arrived; the second waits for the gate.
        waker.1.recv_timeout(Duration::from_secs(30)).unwrap();
        let listing = provider.items("", &env);
        assert!(listing.loading);
        assert_eq!(titles(&listing), ["first"]);
        assert_eq!(provider.fetching(), Some(1));
        release.send(()).unwrap();
        assert_eq!(titles(&settled(&mut provider, &waker)), ["first", "second"]);
        assert_eq!(provider.fetching(), None);
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
        assert_eq!(titles(&listing), ["only"]);
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_fetch_should_show_why() {
        let waker = context();
        let wake = waker.0.clone();
        let feed = Feed::new(wake, || Box::new(|| Err("HTTP 403".to_owned())));
        let mut provider = FeedProvider::new("Items", feed);
        let listing = settled(&mut provider, &waker);
        assert_eq!(listing.error.as_deref(), Some("HTTP 403"));
        assert!(listing.items.is_empty());
        assert!(provider.can_refresh(), "the refresh button tries again");
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

    /// A fetch that waits until `release` is sent to, then gives `items`.
    fn gated(items: &[&str]) -> (mpsc::Sender<()>, impl Fn() -> Load + Send + Sync + 'static) {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let items: Vec<String> = items.iter().map(|&s| s.to_owned()).collect();
        let start = move || -> Load {
            let gate = Arc::clone(&gate);
            let items = items.clone();
            Box::new(move || {
                gate.lock().unwrap().recv().unwrap();
                Ok(Chunk::all(items.iter().map(|s| item(s)).collect()))
            })
        };
        (release, start)
    }

    fn env_items(provider: &mut FeedProvider) -> Listing {
        let keymap = Keymap::default();
        let env = Env {
            can_run: &|_| true,
            keymap: &keymap,
        };
        provider.items("", &env)
    }

    #[test]
    fn a_seeded_feed_should_show_the_seed_at_once_then_swap_in_the_fetch() {
        let waker = context();
        let clock = FakeClock::new();
        let (release, start) = gated(&["new", "old 2"]);
        let feed = Feed::with_options(waker.0.clone(), AUTO_LOAD_LIMIT, clock.clock(), start);
        feed.seed(vec![item("old 1"), item("old 2")], clock.now());
        let mut provider = FeedProvider::new("Items", feed);

        let listing = env_items(&mut provider);
        assert_eq!(titles(&listing), ["old 1", "old 2"], "before any reply");
        assert_eq!(provider.status(), Some(Status::Updating));

        release.send(()).unwrap();
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["new", "old 2"]);
        assert_eq!(provider.status(), None);
    }

    #[test]
    fn a_seeded_feed_whose_fetch_fails_should_keep_the_seed_saying_how_old() {
        let waker = context();
        let clock = FakeClock::new();
        let feed = Feed::with_options(waker.0.clone(), AUTO_LOAD_LIMIT, clock.clock(), || {
            Box::new(|| Err("couldn't find the host".to_owned()))
        });
        feed.seed(vec![item("old")], clock.now());
        clock.advance(Duration::from_secs(3 * 24 * 60 * 60));
        let mut provider = FeedProvider::new("Items", feed);
        let listing = settled(&mut provider, &waker);
        assert_eq!(titles(&listing), ["old"]);
        assert_eq!(listing.error, None, "no error in place of the list");
        assert_eq!(
            provider.status(),
            Some(Status::Note {
                text: "Offline: showing the list from 3 days ago".to_owned(),
                hover: "Couldn't fetch it again: couldn't find the host".to_owned(),
            })
        );
    }

    #[test]
    fn a_refresh_should_keep_the_old_list_until_the_new_one_is_whole() {
        let waker = context();
        let (release, gate) = mpsc::channel::<()>();
        let gate = Arc::new(Mutex::new(gate));
        let fetches = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&fetches);
        // The second fetch's second page waits for the gate.
        let feed = Feed::new(waker.0.clone(), move || {
            let second = counted.fetch_add(1, Ordering::SeqCst) == 1;
            let gate = Arc::clone(&gate);
            Box::new(move || {
                Ok(Chunk {
                    items: vec![item(if second { "fresh 1" } else { "old 1" })],
                    more: Some(More {
                        detail: String::new(),
                        load: Box::new(move || {
                            if second {
                                gate.lock().unwrap().recv().unwrap();
                            }
                            Ok(Chunk::all(vec![item(if second {
                                "fresh 2"
                            } else {
                                "old 2"
                            })]))
                        }),
                    }),
                })
            })
        });
        let mut provider = FeedProvider::new("Items", feed);
        assert_eq!(titles(&settled(&mut provider, &waker)), ["old 1", "old 2"]);
        provider.act(LocalAction::Refresh);
        // The fresh first page has arrived, the second hasn't.
        waker.1.recv_timeout(Duration::from_secs(30)).unwrap();
        let listing = env_items(&mut provider);
        assert_eq!(titles(&listing), ["old 1", "old 2"], "not half a list");
        assert_eq!(provider.status(), Some(Status::Updating));
        release.send(()).unwrap();
        assert_eq!(
            titles(&settled(&mut provider, &waker)),
            ["fresh 1", "fresh 2"]
        );
    }

    #[test]
    fn showing_a_feed_again_should_fetch_it_again_only_once_its_old() {
        let waker = context();
        let clock = FakeClock::new();
        let fetches = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&fetches);
        let feed = Feed::with_options(waker.0.clone(), AUTO_LOAD_LIMIT, clock.clock(), move || {
            let n = counted.fetch_add(1, Ordering::SeqCst);
            Box::new(move || Ok(Chunk::all(vec![item(&format!("fetch {n}"))])))
        });
        let mut first = FeedProvider::new("Items", Arc::clone(&feed));
        settled(&mut first, &waker);
        clock.advance(REVALIDATE_AFTER / 2);
        let mut second = FeedProvider::new("Items", Arc::clone(&feed));
        assert_eq!(titles(&settled(&mut second, &waker)), ["fetch 0"]);
        assert_eq!(fetches.load(Ordering::SeqCst), 1);

        clock.advance(REVALIDATE_AFTER);
        let mut third = FeedProvider::new("Items", feed);
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        let listing = env_items(&mut third);
        assert!(!listing.items.is_empty(), "the old list shows meanwhile");
        assert_eq!(titles(&settled(&mut third, &waker)), ["fetch 1"]);
    }
}
