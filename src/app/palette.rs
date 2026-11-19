//! The command palette: one overlay, opened in two modes, as in VS Code.
//!
//! - **Command mode** (Ctrl+Shift+P, `palette.commands`) lists every
//!   [`Command`], greying out those that can't run now.
//! - **Open mode** (Ctrl+P, `palette.open`) lists sources: an "Open …" item
//!   for a typed URL or existing file, the sources opened this session, and
//!   the ways to open something else. Typing `>` switches to command mode;
//!   command mode is just open mode with a `>` in front.
//!
//! The logic (filtering, ranking, selection, levels) is plain data and
//! functions, tested without egui; [`ViewerApp::palette_ui`] only draws it.
//!
//! # Adding a source of items: providers
//!
//! Everything listed comes from a [`Provider`], which returns a [`Listing`]
//! of [`PaletteItem`]s for the text typed so far. An item has a title, a
//! detail line, and an [`ItemAction`]:
//!
//! - [`ItemAction::Run`] runs a [`Command`] and closes the palette.
//! - [`ItemAction::Prompt`] asks for text in the palette's own field (an
//!   [`ArgPrompt`]), for commands that need an argument such as Open URL.
//! - [`ItemAction::Browse`] drills into a sub-list: it makes a new
//!   [`Provider`] that's pushed on top of the current one. Esc, or Backspace
//!   in an empty field, goes back up a level.
//! - [`ItemAction::Local`] asks the provider showing it to do something to
//!   its own list ([`Provider::act`]), such as fetching the next page.
//!
//! An item can also have a [`PaletteItem::preview`] command, run whenever
//! the selection lands on it, and a provider can ask for a command to run
//! when the palette leaves its list ([`Provider::leave`]): together they let
//! the theme selector show each theme as the arrow keys reach it, and put
//! back the one that was showing if nothing is chosen.
//!
//! A connector (a STAC catalog's collections, then a collection's items,
//! then an item's COPC assets; or the files in a folder) is a provider whose
//! items `Browse` into further providers, ending in items that
//! `Run(Command::OpenSource(url))`. Providers whose items take a while to
//! fetch return [`Listing::loading`] until they're ready, polled each frame;
//! [`FeedProvider`](super::feed::FeedProvider) does that for loads run on a
//! thread, with paging and refreshing, and [`ListProvider`] lists items
//! already in hand. Connectors' roots are offered in open mode through
//! [`Palette::set_connectors`]; see [`super::connectors`].

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};

use super::ViewerApp;
use super::commands::{Command, Keymap};
use super::open_ui::clean_source;
use super::thumbnails::{self, Thumbnails};
use crate::cache::DiskCache;
use crate::theme::Theme;

/// What a provider gets to know about the app, to build its items.
pub(crate) struct Env<'a> {
    /// Whether a command can do anything right now.
    pub(crate) can_run: &'a dyn Fn(&Command) -> bool,
    /// For the shortcut hints.
    pub(crate) keymap: &'a Keymap,
}

/// A row in the palette.
#[derive(Clone, Debug)]
pub(crate) struct PaletteItem {
    pub(crate) title: String,
    /// Secondary text after the title, such as a source's full path. Also
    /// matched against what's typed, less strongly than the title.
    pub(crate) detail: String,
    pub(crate) shortcut: Option<KeyboardShortcut>,
    /// Greyed out and skipped by the selection when false.
    pub(crate) enabled: bool,
    pub(crate) action: ItemAction,
    /// The URL of a small picture to show at the left of the row.
    pub(crate) thumbnail: Option<String>,
    /// Run when the selection moves onto the item, to show what choosing it
    /// would do.
    pub(crate) preview: Option<Command>,
    /// What's in effect now, such as the theme showing: selected when the
    /// list opens.
    pub(crate) current: bool,
}

/// What choosing an item does.
#[derive(Clone, Debug)]
pub(crate) enum ItemAction {
    /// Run a command, closing the palette.
    Run(Command),
    /// Ask for text in the palette, then run the command it makes.
    Prompt(ArgPrompt),
    /// Show a sub-list.
    Browse(Browse),
    /// Ask the provider showing the item to change its list.
    Local(LocalAction),
}

/// What [`ItemAction::Local`] asks of a provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocalAction {
    /// Fetch the list again.
    Refresh,
    /// Fetch the next page.
    LoadMore,
}

/// A command that needs text: the palette's field asks for it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ArgPrompt {
    /// Shown above the field, and in the item that runs the command.
    pub(crate) title: &'static str,
    /// The field's placeholder.
    pub(crate) hint: &'static str,
    /// The command for the text typed, or `None` while it isn't usable.
    pub(crate) finish: fn(&str) -> Option<Command>,
}

/// Asks for a COPC URL or path, for [`Command::OpenUrlDialog`].
pub(crate) const OPEN_URL_PROMPT: ArgPrompt = ArgPrompt {
    title: "Open URL",
    hint: "COPC URL or file path",
    finish: |text| clean_source(text).map(Command::OpenSource),
};

type ProviderFactory = dyn Fn() -> Box<dyn Provider> + Send + Sync;

/// Makes the provider for a sub-list, when its item is chosen.
#[derive(Clone)]
pub(crate) struct Browse(pub(crate) Arc<ProviderFactory>);

impl Browse {
    pub(crate) fn new(make: impl Fn() -> Box<dyn Provider> + Send + Sync + 'static) -> Self {
        Self(Arc::new(make))
    }
}

impl std::fmt::Debug for Browse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Browse(..)")
    }
}

impl PaletteItem {
    /// An item that runs `action`, enabled, with no detail or shortcut.
    pub(crate) fn new(title: impl Into<String>, action: ItemAction) -> Self {
        Self {
            title: title.into(),
            detail: String::new(),
            shortcut: None,
            enabled: true,
            action,
            thumbnail: None,
            preview: None,
            current: false,
        }
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub(crate) fn with_thumbnail(mut self, url: Option<String>) -> Self {
        self.thumbnail = url;
        self
    }

    pub(crate) fn with_preview(mut self, command: Command) -> Self {
        self.preview = Some(command);
        self
    }

    pub(crate) fn with_current(mut self, current: bool) -> Self {
        self.current = current;
        self
    }

    /// `command`, titled and enabled as it is now, with its shortcut. Open
    /// URL asks for its URL in the palette.
    pub(crate) fn command(command: Command, env: &Env<'_>) -> Self {
        let action = match command {
            Command::OpenUrlDialog => ItemAction::Prompt(OPEN_URL_PROMPT),
            _ => ItemAction::Run(command.clone()),
        };
        Self {
            title: command.title().to_owned(),
            detail: String::new(),
            shortcut: env.keymap.shortcut(command.id()),
            enabled: (env.can_run)(&command),
            action,
            thumbnail: None,
            preview: None,
            current: false,
        }
    }
}

/// What a provider has for the text typed so far.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    pub(crate) items: Vec<PaletteItem>,
    /// More items are on their way: the palette shows a spinner.
    pub(crate) loading: bool,
    /// Already filtered and ordered for the query by the provider. If
    /// false, the palette ranks the items with [`rank`].
    pub(crate) ranked: bool,
    /// Why the items couldn't be fetched.
    pub(crate) error: Option<String>,
}

impl Listing {
    /// Items for the palette to rank.
    pub(crate) fn ready(items: Vec<PaletteItem>) -> Self {
        Self {
            items,
            ..Self::default()
        }
    }

    /// Nothing yet: still fetching.
    pub(crate) fn loading() -> Self {
        Self {
            loading: true,
            ..Self::default()
        }
    }
}

/// How a list fetched in the background is getting on, beside the
/// breadcrumb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Fetching, with this many items so far.
    Loading(usize),
    /// Fetching again behind the list showing.
    Updating,
    /// A quiet note, with more on hover.
    Note { text: String, hover: String },
}

/// A source of palette items. See the module docs.
pub(crate) trait Provider: Send {
    /// Shown above the field while this provider's list is showing.
    fn title(&self) -> String;

    /// The field's placeholder.
    fn hint(&self) -> String {
        "Type to filter".to_owned()
    }

    /// The items for `query`, called every frame the list shows, so it
    /// should be cheap: poll background work, don't wait for it.
    fn items(&mut self, query: &str, env: &Env<'_>) -> Listing;

    /// Does what one of this provider's [`ItemAction::Local`] items asks,
    /// or the refresh button.
    fn act(&mut self, _action: LocalAction) {}

    /// Whether the palette shows a refresh button for this list, which
    /// calls [`Provider::act`] with [`LocalAction::Refresh`].
    fn can_refresh(&self) -> bool {
        false
    }

    /// While more items are still being fetched in the background, how many
    /// have arrived.
    fn fetching(&self) -> Option<usize> {
        None
    }

    /// What the line above the field says about the list, at its right.
    fn status(&self) -> Option<Status> {
        self.fetching()
            .filter(|&count| count > 0)
            .map(Status::Loading)
    }

    /// A command to run as the palette leaves this list, whether by going
    /// back, closing or choosing an item (whose command runs after it): to
    /// undo what items' previews did.
    fn leave(&mut self) -> Option<Command> {
        None
    }
}

/// Every command, as listed in command mode.
#[derive(Default)]
pub(crate) struct CommandProvider;

impl Provider for CommandProvider {
    fn title(&self) -> String {
        "Commands".to_owned()
    }

    fn hint(&self) -> String {
        "Type a command".to_owned()
    }

    fn items(&mut self, _query: &str, env: &Env<'_>) -> Listing {
        Listing::ready(
            Command::all()
                .into_iter()
                .map(|command| PaletteItem::command(command, env))
                .collect(),
        )
    }
}

/// How many sources open mode remembers.
const RECENT_LIMIT: usize = 20;

/// Open mode's list: a typed source, recent sources, and ways to open more.
#[derive(Default)]
pub(crate) struct SourceProvider {
    /// Most recent first, without repeats.
    recent: Vec<String>,
    /// The last query checked for being a file, and whether it was one, so
    /// the disk isn't asked again every frame.
    file_check: Option<(String, bool)>,
    /// An item browsing into each data source (connector).
    connectors: Vec<PaletteItem>,
}

impl SourceProvider {
    /// Puts `source` at the top of the recent sources.
    pub(crate) fn remember(&mut self, source: &str) {
        self.recent.retain(|recent| recent != source);
        self.recent.insert(0, source.to_owned());
        self.recent.truncate(RECENT_LIMIT);
    }

    fn is_file(&mut self, text: &str) -> bool {
        match &self.file_check {
            Some((checked, is_file)) if checked == text => *is_file,
            _ => {
                let is_file = Path::new(text).is_file();
                self.file_check = Some((text.to_owned(), is_file));
                is_file
            }
        }
    }
}

impl Provider for SourceProvider {
    fn title(&self) -> String {
        "Open".to_owned()
    }

    fn hint(&self) -> String {
        "Search recent sources, type a path or URL, or > for commands".to_owned()
    }

    fn items(&mut self, query: &str, env: &Env<'_>) -> Listing {
        let typed = clean_source(query).filter(|text| looks_like_url(text) || self.is_file(text));
        let recent = self.recent.iter().map(|source| {
            PaletteItem::new(
                source_name(source),
                ItemAction::Run(Command::OpenSource(source.clone())),
            )
            .with_detail(source.clone())
        });
        let more = [Command::OpenFileDialog, Command::OpenUrlDialog]
            .map(|command| PaletteItem::command(command, env));
        let mut items: Vec<_> = typed.map(open_typed_item).into_iter().collect();
        let listed = recent.chain(self.connectors.iter().cloned()).chain(more);
        items.extend(rank(listed.collect(), query));
        Listing {
            items,
            ranked: true,
            ..Listing::default()
        }
    }
}

/// The item that opens what was typed.
fn open_typed_item(source: String) -> PaletteItem {
    PaletteItem::new(
        format!("Open {source}"),
        ItemAction::Run(Command::OpenSource(source)),
    )
}

/// Whether `text` is shaped like a URL: a scheme, then `://`.
pub(crate) fn looks_like_url(text: &str) -> bool {
    text.split_once("://").is_some_and(|(scheme, rest)| {
        !scheme.is_empty()
            && !rest.is_empty()
            && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    })
}

/// The last part of a path or URL, for a recent source's title.
fn source_name(source: &str) -> String {
    source
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(source)
        .to_owned()
}

/// A fixed list of items, such as a STAC item's point clouds.
pub(crate) struct ListProvider {
    pub(crate) title: String,
    pub(crate) items: Vec<PaletteItem>,
}

impl Provider for ListProvider {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn items(&mut self, _query: &str, _env: &Env<'_>) -> Listing {
        Listing::ready(self.items.clone())
    }
}

/// How well `query` matches `text`, higher being better, or `None` if it
/// doesn't. Each word of the query must appear in `text`, ignoring case: as
/// a substring, best at the start of a word, or failing that as letters in
/// order, best when they're together and start words. An empty query
/// matches everything equally.
pub(crate) fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let text: Vec<char> = text.to_lowercase().chars().collect();
    query
        .to_lowercase()
        .split_whitespace()
        .map(|word| word_score(&word.chars().collect::<Vec<_>>(), &text))
        .sum()
}

fn is_word_start(text: &[char], at: usize) -> bool {
    at == 0 || !text[at - 1].is_alphanumeric()
}

fn word_score(word: &[char], text: &[char]) -> Option<i32> {
    let starts = (0..=text.len().saturating_sub(word.len()))
        .filter(|&at| text[at..].starts_with(word))
        .map(|at| 100 + if is_word_start(text, at) { 50 } else { 0 } - at.min(50) as i32);
    if let Some(best) = starts.max() {
        return Some(best);
    }
    // Letters in order, taken as early as possible.
    let mut score = 0;
    let mut from = 0;
    let mut last = None;
    for &letter in word {
        let at = from + text[from..].iter().position(|&c| c == letter)?;
        score += 1;
        if is_word_start(text, at) {
            score += 8;
        }
        if last.is_some_and(|last| last + 1 == at) {
            score += 5;
        }
        last = Some(at);
        from = at + 1;
    }
    Some(score)
}

/// `items` matching `query`, best first; enabled items before disabled
/// ones. A title match counts for more than a detail match. With nothing
/// typed, every item, in the order given.
pub(crate) fn rank(items: Vec<PaletteItem>, query: &str) -> Vec<PaletteItem> {
    let mut scored: Vec<_> = items
        .into_iter()
        .filter_map(|item| {
            let title = fuzzy_score(query, &item.title);
            let detail = fuzzy_score(query, &item.detail).map(|score| score / 2);
            let score = title.max(detail)?;
            Some((score, item))
        })
        .collect();
    // Stable, so equal scores keep the provider's order.
    scored.sort_by_key(|(score, item)| (!item.enabled, -score));
    scored.into_iter().map(|(_, item)| item).collect()
}

/// The first item that can be chosen.
pub(crate) fn first_enabled(items: &[PaletteItem]) -> Option<usize> {
    items.iter().position(|item| item.enabled)
}

/// The selection after moving `delta` items (up is negative) from `current`,
/// skipping disabled items and wrapping round the ends.
pub(crate) fn step_selection(
    items: &[PaletteItem],
    current: Option<usize>,
    delta: isize,
) -> Option<usize> {
    let first = first_enabled(items)?;
    let Some(mut at) = current.filter(|&at| at < items.len()) else {
        return Some(first);
    };
    let len = items.len() as isize;
    for _ in 0..delta.unsigned_abs() {
        // Ends at an enabled item: there's at least one.
        loop {
            at = (at as isize + delta.signum()).rem_euclid(len) as usize;
            if items[at].enabled {
                break;
            }
        }
    }
    Some(at)
}

/// Which list the palette shows at its root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PaletteMode {
    /// Every command.
    Commands,
    /// Sources to open.
    Open,
}

/// The mode for the text in the field at the root, and the text to filter
/// by: a leading `>` means commands.
pub(crate) fn split_mode(query: &str) -> (PaletteMode, &str) {
    match query.strip_prefix('>') {
        Some(rest) => (PaletteMode::Commands, rest.trim_start()),
        None => (PaletteMode::Open, query),
    }
}

/// A level above the root: a sub-list or a prompt, and the text that was in
/// the field before, to put back when going back up.
enum Level {
    Browse(Box<dyn Provider>),
    Prompt(ArgPrompt),
}

/// The list the palette shows this frame.
#[derive(Debug, Default)]
pub(crate) struct View {
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

/// The palette's state. See the module docs.
#[derive(Default)]
pub(crate) struct Palette {
    open: bool,
    /// The field's text.
    pub(crate) query: String,
    /// The query the selection was made for: a new query resets it.
    selected_for: Option<String>,
    pub(crate) selected: Option<usize>,
    /// The selection as of the last frame and its item's title, so it can
    /// follow the item when a list fetched again moves it.
    selected_item: Option<(usize, String)>,
    /// Sub-lists and prompts above the root, and the query to restore on
    /// leaving each.
    levels: Vec<(Level, String)>,
    commands: CommandProvider,
    sources: SourceProvider,
    /// Scroll to the selection next frame: it was moved by key.
    scroll_to_selected: bool,
    /// Put the field's cursor at the end of its text next frame: the text
    /// was set by the palette, not typed, so the cursor may be anywhere.
    cursor_to_end: bool,
    thumbnails: Thumbnails,
    /// Previews, and providers' commands on leaving, to run in order before
    /// any command chosen.
    effects: Vec<Command>,
    /// The preview last run, so it runs once each time the selection lands
    /// on its item.
    previewed: Option<Command>,
}

impl Palette {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Shows the palette at its root, in `mode`, with an empty filter.
    pub(crate) fn open(&mut self, mode: PaletteMode) {
        self.open = true;
        self.leave_levels();
        self.query = match mode {
            PaletteMode::Commands => ">".to_owned(),
            PaletteMode::Open => String::new(),
        };
        self.selected_for = None;
        self.cursor_to_end = true;
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.leave_levels();
        self.query.clear();
        self.selected_for = None;
    }

    /// Leaves every level above the root, top first.
    fn leave_levels(&mut self) {
        while let Some((level, _)) = self.levels.pop() {
            self.leave(level);
        }
    }

    /// Queues what `level`'s provider asks to run on leaving it.
    fn leave(&mut self, level: Level) {
        if let Level::Browse(mut provider) = level {
            self.effects.extend(provider.leave());
        }
        self.previewed = None;
    }

    /// The previews and leaving commands queued since last asked, in order.
    pub(crate) fn take_effects(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.effects)
    }

    /// Queues the selected item's preview, if the selection has just
    /// landed on it.
    fn preview_selected(&mut self, items: &[PaletteItem]) {
        let preview = self
            .selected
            .and_then(|at| items.get(at))
            .and_then(|item| item.preview.clone());
        if preview.is_some() && preview != self.previewed {
            self.effects.extend(preview.clone());
        }
        self.previewed = preview;
    }

    /// Keeps thumbnails in `cache` between sessions, if given.
    pub(crate) fn set_thumbnail_cache(&mut self, cache: Option<DiskCache>) {
        self.thumbnails.set_cache(cache);
    }

    /// Offers `source` first among the recent sources.
    pub(crate) fn remember(&mut self, source: &str) {
        self.sources.remember(source);
    }

    /// Lists `items`, each browsing into a data source, in open mode.
    pub(crate) fn set_connectors(&mut self, items: Vec<PaletteItem>) {
        self.sources.connectors = items;
    }

    /// Shows `provider`'s list above the current level, as choosing an item
    /// that browses into it would.
    pub(crate) fn browse(&mut self, provider: Box<dyn Provider>) {
        self.open = true;
        self.levels
            .push((Level::Browse(provider), std::mem::take(&mut self.query)));
        self.selected_for = None;
        self.cursor_to_end = true;
    }

    /// Whether a sub-list or prompt is showing.
    pub(crate) fn is_drilled_in(&self) -> bool {
        !self.levels.is_empty()
    }

    /// What's above the field: the sub-list's or prompt's title, once
    /// drilled in.
    pub(crate) fn breadcrumb(&self) -> Option<String> {
        self.levels.last().map(|(level, _)| match level {
            Level::Browse(provider) => provider.title(),
            Level::Prompt(prompt) => prompt.title.to_owned(),
        })
    }

    /// The sub-list showing, if one is.
    fn browsing(&self) -> Option<&dyn Provider> {
        match self.levels.last() {
            Some((Level::Browse(provider), _)) => Some(provider.as_ref()),
            _ => None,
        }
    }

    /// Fetches the sub-list showing again, as its refresh button does.
    pub(crate) fn refresh(&mut self) {
        if let Some((Level::Browse(provider), _)) = self.levels.last_mut() {
            provider.act(LocalAction::Refresh);
        }
    }

    /// The field's placeholder.
    pub(crate) fn hint(&self) -> String {
        match self.levels.last() {
            Some((Level::Browse(provider), _)) => provider.hint(),
            Some((Level::Prompt(prompt), _)) => prompt.hint.to_owned(),
            None => match split_mode(&self.query).0 {
                PaletteMode::Commands => self.commands.hint(),
                PaletteMode::Open => self.sources.hint(),
            },
        }
    }

    /// The items for the current level and query, ranked, with the selection
    /// kept valid.
    pub(crate) fn view(&mut self, env: &Env<'_>) -> View {
        let view = match self.levels.last_mut() {
            None => {
                let (mode, filter) = split_mode(&self.query);
                let provider: &mut dyn Provider = match mode {
                    PaletteMode::Commands => &mut self.commands,
                    PaletteMode::Open => &mut self.sources,
                };
                listing_view(provider, filter, env)
            }
            Some((Level::Browse(provider), _)) => listing_view(provider.as_mut(), &self.query, env),
            Some((Level::Prompt(prompt), _)) => View {
                items: (prompt.finish)(&self.query)
                    .map(|command| {
                        PaletteItem::new(
                            format!("{}: {}", prompt.title, self.query.trim()),
                            ItemAction::Run(command),
                        )
                    })
                    .into_iter()
                    .collect(),
                ..View::default()
            },
        };
        let stale = self.selected_for.as_deref() != Some(self.query.as_str());
        if !stale
            && let Some((at, title)) = &self.selected_item
            && self.selected == Some(*at)
            && view.items.get(*at).is_none_or(|item| &item.title != title)
            && let Some(moved) = view
                .items
                .iter()
                .position(|item| &item.title == title && item.enabled)
        {
            self.selected = Some(moved);
        }
        let invalid = self
            .selected
            .is_none_or(|at| !view.items.get(at).is_some_and(|item| item.enabled));
        if stale || invalid {
            // The current item until something's typed, then the best match.
            let current = view
                .items
                .iter()
                .position(|item| item.current && item.enabled)
                .filter(|_| self.query.trim().is_empty());
            self.selected = current.or_else(|| first_enabled(&view.items));
            self.selected_for = Some(self.query.clone());
        }
        self.selected_item = self
            .selected
            .and_then(|at| Some((at, view.items.get(at)?.title.clone())));
        self.preview_selected(&view.items);
        view
    }

    /// Moves the selection by `delta` rows through `items`.
    pub(crate) fn move_selection(&mut self, items: &[PaletteItem], delta: isize) {
        self.selected = step_selection(items, self.selected, delta);
        self.scroll_to_selected = true;
        self.preview_selected(items);
    }

    /// Chooses `item`: returns the command to run, if it runs one (closing
    /// the palette), or goes into its prompt or sub-list.
    pub(crate) fn choose(&mut self, item: PaletteItem) -> Option<Command> {
        if !item.enabled {
            return None;
        }
        let level = match item.action {
            ItemAction::Run(command) => {
                self.close();
                return Some(command);
            }
            ItemAction::Prompt(prompt) => Level::Prompt(prompt),
            ItemAction::Browse(Browse(make)) => Level::Browse(make()),
            ItemAction::Local(action) => {
                if let Some((Level::Browse(provider), _)) = self.levels.last_mut() {
                    provider.act(action);
                }
                return None;
            }
        };
        self.levels.push((level, std::mem::take(&mut self.query)));
        self.selected_for = None;
        self.cursor_to_end = true;
        None
    }

    /// Esc: up a level, or closed at the root.
    pub(crate) fn back(&mut self) {
        match self.levels.pop() {
            Some((level, query)) => {
                self.leave(level);
                self.query = query;
                self.selected_for = None;
                self.cursor_to_end = true;
            }
            None => self.close(),
        }
    }
}

fn listing_view(provider: &mut dyn Provider, filter: &str, env: &Env<'_>) -> View {
    let listing = provider.items(filter, env);
    let items = if listing.ranked {
        listing.items
    } else {
        rank(listing.items, filter)
    };
    View {
        items,
        loading: listing.loading,
        error: listing.error,
    }
}

/// The keys the palette handles itself, taken before its field sees them.
#[derive(Default)]
struct Keys {
    up: bool,
    down: bool,
    page_up: bool,
    page_down: bool,
    enter: bool,
    escape: bool,
    back: bool,
}

/// How many rows Page Up and Page Down move.
const PAGE: isize = 8;

impl ViewerApp {
    /// Draws the palette, if it's open, and queues the command chosen in it.
    pub(super) fn palette_ui(&mut self, ctx: &egui::Context) {
        if !self.palette.is_open() {
            // Closed by a command since: put back what it previewed.
            self.pending.extend(self.palette.take_effects());
            return;
        }
        let mut palette = std::mem::take(&mut self.palette);
        let can_run = |command: &Command| self.can_run(command);
        let env = Env {
            can_run: &can_run,
            keymap: &self.keymap,
        };
        let chosen = palette.show(ctx, &self.theme, &env);
        // Previews and undoing them first, so a theme chosen wins over the
        // one put back on leaving the list.
        self.pending.extend(palette.take_effects());
        self.palette = palette;
        self.pending.extend(chosen);
    }
}

impl Palette {
    fn show(&mut self, ctx: &egui::Context, theme: &Theme, env: &Env<'_>) -> Option<Command> {
        // The palette's own shortcuts switch it between modes, as in VS
        // Code; other shortcuts don't fire while it's open. Ctrl+Shift+P
        // first, as Ctrl+P would match it too.
        let switch_to = [
            ("palette.commands", PaletteMode::Commands),
            ("palette.open", PaletteMode::Open),
        ]
        .into_iter()
        .find(|(id, _)| {
            env.keymap
                .shortcut(id)
                .is_some_and(|shortcut| ctx.input_mut(|input| input.consume_shortcut(&shortcut)))
        });
        if let Some((_, mode)) = switch_to {
            self.open(mode);
        }

        let back_on_backspace = self.query.is_empty() && self.is_drilled_in();
        let keys = ctx.input_mut(|input| {
            let mut take = |key| input.consume_key(Modifiers::NONE, key);
            Keys {
                up: take(Key::ArrowUp),
                down: take(Key::ArrowDown),
                page_up: take(Key::PageUp),
                page_down: take(Key::PageDown),
                enter: take(Key::Enter),
                escape: take(Key::Escape),
                back: back_on_backspace && take(Key::Backspace),
            }
        });
        if keys.escape || keys.back {
            self.back();
            ctx.request_repaint();
            return None;
        }

        let colors = &theme.colors;
        let width = (ctx.content_rect().width() - 32.0).clamp(200.0, 600.0);
        let frame = theme
            .panel_frame()
            .outer_margin(0)
            .stroke(egui::Stroke::new(1.0, colors.strong_stroke))
            .shadow(theme.shape.popup_shadow);
        let mut clicked = None;
        let mut refresh = false;
        let mut view = View::default();
        let area = egui::Area::new(egui::Id::new("command_palette"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
            .show(ctx, |ui| {
                frame.show(ui, |ui| {
                    ui.set_width(width);
                    if let Some(breadcrumb) = self.breadcrumb() {
                        refresh = self.header_ui(ui, theme, breadcrumb);
                    }
                    let hint = self.hint();
                    let mut output = egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::new("command_palette_input"))
                        .hint_text(hint)
                        .desired_width(f32::INFINITY)
                        .show(ui);
                    if std::mem::take(&mut self.cursor_to_end) {
                        let end = egui::text::CCursor::new(self.query.chars().count());
                        output
                            .state
                            .cursor
                            .set_char_range(Some(egui::text_selection::CCursorRange::one(end)));
                        output.state.store(ui.ctx(), output.response.id);
                    }
                    // Only when it's lost: asking again each frame would
                    // drop the field's hold on the arrow keys, and egui
                    // would move the focus off it for a frame at each press.
                    if !output.response.has_focus() {
                        output.response.request_focus();
                    }

                    // After the field, so the list follows what was just typed.
                    view = self.view(env);
                    for (pressed, delta) in [
                        (keys.up, -1),
                        (keys.down, 1),
                        (keys.page_up, -PAGE),
                        (keys.page_down, PAGE),
                    ] {
                        if pressed {
                            self.move_selection(&view.items, delta);
                        }
                    }
                    clicked = self.list_ui(ui, theme, &view);
                });
            });

        // A click outside closes the palette, as in VS Code.
        let clicked_outside = ctx.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !area.response.rect.contains(pos))
        });
        if refresh {
            self.refresh();
            ctx.request_repaint();
        }
        let chosen = clicked.or(self.selected.filter(|_| keys.enter));
        let command = match chosen {
            Some(at) if at < view.items.len() => self.choose(view.items.swap_remove(at)),
            _ => None,
        };
        if clicked_outside && command.is_none() {
            self.close();
        }
        if chosen.is_some() || clicked_outside {
            // A new level, or none, shows next frame.
            ctx.request_repaint();
        }
        command
    }

    /// The line above the field: where the palette has browsed to, and for a
    /// list fetched in the background, how far it's got and a refresh
    /// button. Returns whether the button was clicked.
    fn header_ui(&self, ui: &mut egui::Ui, theme: &Theme, breadcrumb: String) -> bool {
        let colors = &theme.colors;
        let provider = self.browsing();
        let can_refresh = provider.is_some_and(|provider| provider.can_refresh());
        let status = provider.and_then(|provider| provider.status());
        let label = ui.label(egui::RichText::new(breadcrumb).color(colors.text_secondary));
        if !can_refresh && status.is_none() {
            return false;
        }
        // On the label's line, at the right, without making it taller.
        let line = egui::Rect::from_x_y_ranges(ui.max_rect().x_range(), label.rect.y_range());
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(line)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        let small = |text: &str| {
            egui::RichText::new(text)
                .small()
                .color(colors.text_secondary)
        };
        let mut clicked = false;
        if can_refresh {
            let button = egui::Button::new(small("Refresh")).frame(false).small();
            clicked = right
                .add(button)
                .on_hover_text("Fetch this list again")
                .clicked();
        }
        let spinner = |right: &mut egui::Ui| {
            let size = right.text_style_height(&egui::TextStyle::Small);
            right.add(egui::Spinner::new().size(size));
        };
        match status {
            Some(Status::Loading(count)) => {
                right.label(small(&format!("Loading… {count} so far")));
                spinner(&mut right);
            }
            Some(Status::Updating) => {
                right.label(small("Updating…"));
                spinner(&mut right);
            }
            Some(Status::Note { text, hover }) => {
                right.label(small(&text)).on_hover_text(hover);
            }
            None => {}
        }
        clicked
    }

    /// The rows, returning the one clicked.
    fn list_ui(&mut self, ui: &mut egui::Ui, theme: &Theme, view: &View) -> Option<usize> {
        let colors = &theme.colors;
        let mut clicked = None;
        let scroll_to_selected = std::mem::take(&mut self.scroll_to_selected);
        // Once some items are in, the header says more are coming, so the
        // rows don't move down for a spinner here.
        if view.loading && view.items.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(egui::RichText::new("Loading…").color(colors.text_secondary));
            });
        }
        if let Some(error) = &view.error {
            ui.colored_label(colors.error, error);
        }
        if view.items.is_empty() && !view.loading && view.error.is_none() {
            let empty = match self.levels.last() {
                Some((Level::Prompt(_), _)) => "Type to continue",
                _ => "No matches",
            };
            ui.label(egui::RichText::new(empty).color(colors.text_secondary));
            return None;
        }
        egui::ScrollArea::vertical()
            .max_height(360.0)
            // Else it starts as small as the area it's in was last frame.
            .min_scrolled_height(360.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                same_size_rows(ui);
                // Every row gets a thumbnail's slot if any has one, so the
                // titles line up.
                let thumbnail_slot = (thumbnails::ENABLED
                    && view.items.iter().any(|item| item.thumbnail.is_some()))
                .then(|| thumbnails::slot_size(ui));
                if thumbnail_slot.is_some() {
                    self.thumbnails.poll(ui.ctx());
                }
                for (at, item) in view.items.iter().enumerate() {
                    let selected = self.selected == Some(at);
                    // One line, the detail fainter after the title, cut short
                    // to fit before the shortcut.
                    let mut text = egui::text::LayoutJob::default();
                    let font = egui::TextStyle::Button.resolve(ui.style());
                    let format = |color| egui::TextFormat::simple(font.clone(), color);
                    text.append(&item.title, 0.0, format(colors.text));
                    if !item.detail.is_empty() && item.detail != item.title {
                        text.append(" ", 0.0, format(colors.text));
                        text.append(&item.detail, 8.0, format(colors.text_secondary));
                    }
                    // Always framed (see `same_size_rows`); the selected row
                    // keeps the outline it had at rest.
                    let slot_id = ui.id().with(("thumbnail", at));
                    let mut atoms = egui::Atoms::new(text);
                    if let Some(size) = thumbnail_slot {
                        atoms.push_left(egui::Atom::custom(slot_id, size));
                    }
                    let mut button = egui::Button::new(atoms)
                        .selected(selected)
                        .frame(true)
                        .frame_when_inactive(true)
                        .truncate()
                        .min_size(egui::vec2(ui.available_width(), 0.0));
                    if selected {
                        button = button.stroke(egui::Stroke::new(ROW_STROKE, colors.strong_stroke));
                    }
                    if let Some(shortcut) = &item.shortcut {
                        button = button.shortcut_text(ui.ctx().format_shortcut(shortcut));
                    }
                    let drawn = ui
                        .add_enabled_ui(item.enabled, |ui| button.atom_ui(ui))
                        .inner;
                    // Fetched only for rows on screen: the slot has a rect
                    // only when the row is visible.
                    if let (Some(url), Some(slot)) = (&item.thumbnail, drawn.rect(slot_id))
                        && let Some(texture) = self.thumbnails.get(ui.ctx(), url)
                    {
                        thumbnails::paint(ui, slot, texture);
                    }
                    let row = drawn.response;
                    if selected && scroll_to_selected {
                        row.scroll_to_me(None);
                    }
                    if row.clicked() {
                        clicked = Some(at);
                    }
                }
            });
        clicked
    }
}

/// The outline width of every palette row, highlighted or not.
const ROW_STROKE: f32 = 1.0;

/// Makes every row in `ui` the same size in every state. egui frames a
/// button only once it's hovered, pressed or selected, and its padding
/// allows for the outline only when it's framed, so a plain row was smaller
/// by its outline width on each side and the rows below moved as the
/// highlight did. Here every state has the same outline width and no
/// expansion, and plain rows are framed in transparent colours. The padding
/// gives back the outline's height, so rows stay the height plain ones were.
fn same_size_rows(ui: &mut egui::Ui) {
    ui.spacing_mut().button_padding.y -= ROW_STROKE;
    let widgets = &mut ui.visuals_mut().widgets;
    for state in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
    ] {
        state.expansion = 0.0;
        state.bg_stroke.width = ROW_STROKE;
    }
    widgets.inactive.bg_stroke.color = egui::Color32::TRANSPARENT;
    widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Mode;
    use crate::app::settings::ColorMode;
    use crate::camera::ViewPreset;

    fn env_with<'a>(can_run: &'a dyn Fn(&Command) -> bool, keymap: &'a Keymap) -> Env<'a> {
        Env { can_run, keymap }
    }

    fn item(title: &str) -> PaletteItem {
        PaletteItem::new(title, ItemAction::Run(Command::ResetView))
    }

    fn disabled(title: &str) -> PaletteItem {
        PaletteItem {
            enabled: false,
            ..item(title)
        }
    }

    fn titles(items: &[PaletteItem]) -> Vec<&str> {
        items.iter().map(|item| item.title.as_str()).collect()
    }

    /// The palette's items for what's typed, with every command runnable.
    fn view_of(palette: &mut Palette) -> View {
        let keymap = Keymap::default();
        palette.view(&env_with(&|_| true, &keymap))
    }

    fn type_text(palette: &mut Palette, text: &str) -> View {
        palette.query.push_str(text);
        view_of(palette)
    }

    /// Chooses the selected item, as Enter does.
    fn enter(palette: &mut Palette) -> Option<Command> {
        let mut view = view_of(palette);
        let at = palette.selected?;
        palette.choose(view.items.swap_remove(at))
    }

    #[test]
    fn fuzzy_score_should_match_a_substring_ignoring_case() {
        assert!(fuzzy_score("TOP", "View from top").is_some());
    }

    #[test]
    fn fuzzy_score_should_match_letters_in_order() {
        assert!(fuzzy_score("vft", "View from top").is_some());
        assert!(fuzzy_score("tfv", "View from top").is_none());
    }

    #[test]
    fn fuzzy_score_should_need_every_word() {
        assert!(fuzzy_score("view top", "View from top").is_some());
        assert!(fuzzy_score("view left", "View from top").is_none());
    }

    #[test]
    fn fuzzy_score_should_prefer_a_match_at_a_word_start() {
        let word_start = fuzzy_score("class", "Colour by classification").unwrap();
        let inside = fuzzy_score("ass", "Colour by classification").unwrap();
        assert!(word_start > inside);
    }

    #[test]
    fn fuzzy_score_should_prefer_a_substring_to_scattered_letters() {
        let substring = fuzzy_score("top", "View from top").unwrap();
        let scattered = fuzzy_score("vfp", "View from top").unwrap();
        assert!(substring > scattered);
    }

    #[test]
    fn rank_should_keep_the_order_given_for_an_empty_query() {
        let ranked = rank(vec![item("b"), item("a"), item("c")], "  ");
        assert_eq!(titles(&ranked), ["b", "a", "c"]);
    }

    #[test]
    fn rank_should_drop_items_that_dont_match() {
        let ranked = rank(vec![item("View from top"), item("Reset view")], "top");
        assert_eq!(titles(&ranked), ["View from top"]);
    }

    #[test]
    fn rank_should_put_better_matches_first() {
        let ranked = rank(
            vec![
                item("Toggle perspective / orthographic"),
                item("View from top"),
            ],
            "top",
        );
        assert_eq!(
            titles(&ranked),
            ["View from top", "Toggle perspective / orthographic"]
        );
    }

    #[test]
    fn rank_should_put_disabled_items_last() {
        let ranked = rank(vec![disabled("a"), item("b")], "");
        assert_eq!(titles(&ranked), ["b", "a"]);
    }

    #[test]
    fn rank_should_match_the_detail_too() {
        let source = item("site.copc.laz").with_detail("https://example.com/lidar/site.copc.laz");
        assert_eq!(rank(vec![source], "lidar").len(), 1);
    }

    #[test]
    fn step_selection_should_skip_disabled_items() {
        let items = [item("a"), disabled("b"), item("c")];
        assert_eq!(step_selection(&items, Some(0), 1), Some(2));
        assert_eq!(step_selection(&items, Some(2), -1), Some(0));
    }

    #[test]
    fn step_selection_should_wrap_round() {
        let items = [item("a"), item("b"), item("c")];
        assert_eq!(step_selection(&items, Some(2), 1), Some(0));
        assert_eq!(step_selection(&items, Some(0), -1), Some(2));
    }

    #[test]
    fn step_selection_should_take_several_steps_at_once() {
        let items = [item("a"), item("b"), disabled("c"), item("d")];
        assert_eq!(step_selection(&items, Some(0), 2), Some(3));
    }

    #[test]
    fn step_selection_should_find_nothing_when_everything_is_disabled() {
        assert_eq!(step_selection(&[disabled("a")], Some(0), 1), None);
        assert_eq!(step_selection(&[], None, 1), None);
    }

    #[test]
    fn split_mode_should_treat_a_leading_chevron_as_command_mode() {
        assert_eq!(split_mode("> top"), (PaletteMode::Commands, "top"));
        assert_eq!(split_mode("top"), (PaletteMode::Open, "top"));
    }

    #[test]
    fn looks_like_url_should_want_a_scheme() {
        assert!(looks_like_url("https://example.com/a.copc.laz"));
        assert!(looks_like_url("s3://bucket/a.copc.laz"));
        assert!(!looks_like_url("example.com/a.copc.laz"));
        assert!(!looks_like_url("://nothing"));
        assert!(!looks_like_url("https://"));
    }

    #[test]
    fn source_name_should_be_the_last_part() {
        assert_eq!(
            source_name("https://example.com/a/site.copc.laz"),
            "site.copc.laz"
        );
        assert_eq!(source_name(r"C:\data\site.copc.laz"), "site.copc.laz");
        assert_eq!(source_name("https://example.com/dir/"), "dir");
    }

    #[test]
    fn command_mode_should_list_every_command() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        assert_eq!(view_of(&mut palette).items.len(), Command::all().len());
    }

    #[test]
    fn command_mode_should_grey_out_commands_that_cant_run() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        palette.query.push_str("view from top");
        let keymap = Keymap::default();
        let can_run = |command: &Command| !matches!(command, Command::SetView(_));
        let view = palette.view(&env_with(&can_run, &keymap));
        assert_eq!(titles(&view.items)[0], "View from top");
        assert!(!view.items[0].enabled);
        assert_eq!(palette.selected, None);
    }

    #[test]
    fn command_items_should_show_their_shortcut() {
        let keymap = Keymap::default();
        let top = PaletteItem::command(
            Command::SetView(ViewPreset::Top),
            &env_with(&|_| true, &keymap),
        );
        assert_eq!(top.shortcut, keymap.shortcut("view.top"));
    }

    #[test]
    fn enter_should_run_the_best_match_and_close() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        type_text(&mut palette, "diagnostics");
        assert_eq!(enter(&mut palette), Some(Command::ToggleDiagnostics));
        assert!(!palette.is_open());
    }

    #[test]
    fn arrow_keys_should_move_the_selection() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        let view = type_text(&mut palette, "view from");
        assert_eq!(palette.selected, Some(0));
        palette.move_selection(&view.items, 1);
        assert_eq!(palette.selected, Some(1));
        let chosen = view.items[1].title.clone();
        assert_eq!(
            view_of(&mut palette).items[palette.selected.unwrap()].title,
            chosen
        );
    }

    #[test]
    fn typing_should_reset_the_selection_to_the_best_match() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        let view = view_of(&mut palette);
        palette.move_selection(&view.items, 3);
        type_text(&mut palette, "return to the subcloud");
        assert_eq!(enter(&mut palette), Some(Command::SetMode(Mode::Subcloud)));
    }

    #[test]
    fn open_mode_should_offer_to_open_a_typed_url_first() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        let view = type_text(&mut palette, " https://example.com/a.copc.laz ");
        assert_eq!(view.items[0].title, "Open https://example.com/a.copc.laz");
        assert_eq!(
            enter(&mut palette),
            Some(Command::OpenSource(
                "https://example.com/a.copc.laz".to_owned()
            ))
        );
    }

    #[test]
    fn open_mode_should_offer_to_open_an_existing_file() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/fixture.copc.laz");
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        type_text(&mut palette, path);
        assert_eq!(
            enter(&mut palette),
            Some(Command::OpenSource(path.to_owned()))
        );
    }

    #[test]
    fn open_mode_should_not_offer_to_open_a_missing_file() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        let view = type_text(&mut palette, "no/such/file.copc.laz");
        assert!(
            view.items
                .iter()
                .all(|item| !item.title.starts_with("Open no/"))
        );
    }

    #[test]
    fn open_mode_should_list_recent_sources_newest_first() {
        let mut palette = Palette::default();
        palette.remember("https://example.com/a.copc.laz");
        palette.remember("https://example.com/b.copc.laz");
        palette.remember("https://example.com/a.copc.laz");
        palette.open(PaletteMode::Open);
        let view = view_of(&mut palette);
        assert_eq!(
            titles(&view.items),
            ["a.copc.laz", "b.copc.laz", "Open file…", "Open URL…"]
        );
    }

    #[test]
    fn open_mode_should_filter_recent_sources() {
        let mut palette = Palette::default();
        palette.remember("https://example.com/a.copc.laz");
        palette.remember("https://example.com/b.copc.laz");
        palette.open(PaletteMode::Open);
        let view = type_text(&mut palette, "b.copc");
        assert_eq!(titles(&view.items), ["b.copc.laz"]);
    }

    #[test]
    fn recent_sources_should_be_capped() {
        let mut palette = Palette::default();
        for at in 0..RECENT_LIMIT + 5 {
            palette.remember(&format!("https://example.com/{at}.copc.laz"));
        }
        palette.open(PaletteMode::Open);
        assert_eq!(view_of(&mut palette).items.len(), RECENT_LIMIT + 2);
    }

    #[test]
    fn typing_a_chevron_should_switch_open_mode_to_commands() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        let view = type_text(&mut palette, ">");
        assert_eq!(view.items.len(), Command::all().len());
        assert_eq!(palette.hint(), "Type a command");
    }

    #[test]
    fn open_url_should_prompt_in_the_palette() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        type_text(&mut palette, "open url");
        assert_eq!(enter(&mut palette), None);
        assert!(palette.is_open());
        assert_eq!(palette.breadcrumb().as_deref(), Some("Open URL"));
        assert_eq!(palette.query, "");
        assert!(view_of(&mut palette).items.is_empty());

        let view = type_text(&mut palette, "https://example.com/a.copc.laz");
        assert_eq!(
            view.items[0].title,
            "Open URL: https://example.com/a.copc.laz"
        );
        assert_eq!(
            enter(&mut palette),
            Some(Command::OpenSource(
                "https://example.com/a.copc.laz".to_owned()
            ))
        );
        assert!(!palette.is_open());
    }

    #[test]
    fn back_should_leave_a_prompt_for_the_list_it_came_from() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        type_text(&mut palette, "open url");
        enter(&mut palette);
        palette.back();
        assert!(palette.is_open());
        assert_eq!(palette.query, ">open url");
        palette.back();
        assert!(!palette.is_open());
    }

    /// A connector: folders browse into sub-lists of sources.
    fn folder(name: &'static str, files: &'static [&'static str]) -> PaletteItem {
        PaletteItem::new(
            name,
            ItemAction::Browse(Browse::new(move || {
                Box::new(ListProvider {
                    title: name.to_owned(),
                    items: files
                        .iter()
                        .map(|file| {
                            PaletteItem::new(
                                *file,
                                ItemAction::Run(Command::OpenSource(format!("{name}/{file}"))),
                            )
                        })
                        .collect(),
                })
            })),
        )
    }

    #[test]
    fn browse_should_drill_into_a_sub_list_and_back() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        palette.query.push_str("sit");
        let chosen = palette.choose(folder("sites", &["north.copc.laz", "south.copc.laz"]));
        assert_eq!(chosen, None);
        assert_eq!(palette.breadcrumb().as_deref(), Some("sites"));
        let view = view_of(&mut palette);
        assert_eq!(titles(&view.items), ["north.copc.laz", "south.copc.laz"]);

        let view = type_text(&mut palette, "south");
        assert_eq!(titles(&view.items), ["south.copc.laz"]);
        palette.query.clear();
        palette.back();
        assert_eq!(palette.query, "sit");
        assert_eq!(palette.breadcrumb(), None);
    }

    #[test]
    fn browse_should_open_a_source_from_a_sub_list() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        palette.choose(folder("sites", &["north.copc.laz"]));
        assert_eq!(
            enter(&mut palette),
            Some(Command::OpenSource("sites/north.copc.laz".to_owned()))
        );
    }

    /// Counts the items it's asked to add, listing one item per request.
    struct Growing(usize);

    impl Provider for Growing {
        fn title(&self) -> String {
            "Growing".to_owned()
        }

        fn items(&mut self, _query: &str, _env: &Env<'_>) -> Listing {
            let mut items: Vec<_> = (0..self.0).map(|at| item(&at.to_string())).collect();
            items.push(PaletteItem::new(
                "Load more",
                ItemAction::Local(LocalAction::LoadMore),
            ));
            Listing::ready(items)
        }

        fn act(&mut self, action: LocalAction) {
            if action == LocalAction::LoadMore {
                self.0 += 1;
            }
        }
    }

    #[test]
    fn local_items_should_act_on_their_provider_and_stay_open() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        palette.browse(Box::new(Growing(1)));
        assert_eq!(palette.breadcrumb().as_deref(), Some("Growing"));
        let view = type_text(&mut palette, "load");
        palette.choose(view.items[0].clone());
        assert!(palette.is_open());
        assert_eq!(palette.query, "load", "the filter stays");
        palette.query.clear();
        assert_eq!(
            titles(&view_of(&mut palette).items),
            ["0", "1", "Load more"]
        );
    }

    #[test]
    fn open_mode_should_list_connectors_by_name() {
        let mut palette = Palette::default();
        palette.set_connectors(vec![
            folder("Survey data", &["a.copc.laz"]).with_detail("Folder source"),
        ]);
        palette.open(PaletteMode::Open);
        let view = type_text(&mut palette, "survey");
        assert_eq!(titles(&view.items), ["Survey data"]);
        assert_eq!(enter(&mut palette), None);
        assert_eq!(palette.breadcrumb().as_deref(), Some("Survey data"));
    }

    /// The rows' rects as `list_ui` lays them out with `theme`, with `selected`
    /// selected and the pointer at `hover`.
    fn row_rects(theme: &Theme, selected: usize, hover: Option<egui::Pos2>) -> Vec<egui::Rect> {
        use egui_kittest::kittest::Queryable;
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        let view = view_of(&mut palette);
        palette.selected = Some(selected);
        let theme = theme.clone();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(600.0, 800.0))
            .build_ui(move |ui| {
                theme.apply(ui.ctx());
                palette.list_ui(ui, &theme, &view);
            });
        harness.run();
        if let Some(pos) = hover {
            harness.hover_at(pos);
            harness.run();
        }
        harness
            .query_all_by_role(egui::accesskit::Role::Button)
            .map(|node| node.rect())
            .collect()
    }

    #[test]
    fn rows_should_stay_put_as_the_selection_and_hover_move() {
        // Large enough text that rows are taller than the minimum height,
        // where a border on highlighted rows only once made them bigger.
        for button_size in [Theme::default().fonts.button_size, 20.0] {
            let mut theme = Theme::default();
            theme.fonts.button_size = button_size;
            let first = row_rects(&theme, 0, None);
            assert!(first.len() > 3);
            let height = first[0].height();
            assert!(
                first.iter().all(|rect| rect.height() == height),
                "rows of different heights at {button_size} pt: {first:?}"
            );
            assert_eq!(row_rects(&theme, 2, None), first, "{button_size} pt");
            let hover = Some(first[1].center());
            assert_eq!(row_rects(&theme, 2, hover), first, "{button_size} pt");
        }
    }

    /// Items that preview a colour mode each, the second current, putting
    /// back RGB on leaving.
    struct Previews;

    impl Provider for Previews {
        fn title(&self) -> String {
            "Previews".to_owned()
        }

        fn items(&mut self, _query: &str, _env: &Env<'_>) -> Listing {
            let modes = [ColorMode::Height, ColorMode::Rgb, ColorMode::Classification];
            Listing::ready(
                modes
                    .into_iter()
                    .map(|mode| {
                        PaletteItem::new(format!("{mode:?}"), ItemAction::Run(Command::ResetView))
                            .with_preview(Command::SetColorMode(mode))
                            .with_current(mode == ColorMode::Rgb)
                    })
                    .collect(),
            )
        }

        fn leave(&mut self) -> Option<Command> {
            Some(Command::SetColorMode(ColorMode::Rgb))
        }
    }

    fn previews() -> Palette {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Open);
        palette.browse(Box::new(Previews));
        palette
    }

    #[test]
    fn the_current_item_should_be_selected_and_previewed_once() {
        let mut palette = previews();
        view_of(&mut palette);
        assert_eq!(palette.selected, Some(1));
        view_of(&mut palette);
        assert_eq!(
            palette.take_effects(),
            [Command::SetColorMode(ColorMode::Rgb)]
        );
    }

    #[test]
    fn moving_the_selection_should_preview_each_item_it_reaches() {
        let mut palette = previews();
        let view = view_of(&mut palette);
        palette.take_effects();
        palette.move_selection(&view.items, 1);
        view_of(&mut palette);
        palette.move_selection(&view.items, -2);
        assert_eq!(
            palette.take_effects(),
            [
                Command::SetColorMode(ColorMode::Classification),
                Command::SetColorMode(ColorMode::Height),
            ]
        );
    }

    #[test]
    fn leaving_a_list_should_run_its_leave_command() {
        for leave in [Palette::back, Palette::close] {
            let mut palette = previews();
            view_of(&mut palette);
            palette.take_effects();
            leave(&mut palette);
            assert_eq!(
                palette.take_effects(),
                [Command::SetColorMode(ColorMode::Rgb)]
            );
        }
        // Reopening the palette leaves the list too.
        let mut palette = previews();
        palette.open(PaletteMode::Commands);
        assert_eq!(
            palette.take_effects(),
            [Command::SetColorMode(ColorMode::Rgb)]
        );
    }

    #[test]
    fn choosing_should_leave_the_list_before_its_command_runs() {
        let mut palette = previews();
        let view = view_of(&mut palette);
        palette.move_selection(&view.items, 1);
        palette.take_effects();
        assert_eq!(enter(&mut palette), Some(Command::ResetView));
        assert_eq!(
            palette.take_effects(),
            [Command::SetColorMode(ColorMode::Rgb)]
        );
        assert!(!palette.is_open());
    }

    #[test]
    fn typing_should_select_the_best_match_not_the_current_item() {
        let mut palette = previews();
        let view = type_text(&mut palette, "class");
        assert_eq!(titles(&view.items), ["Classification"]);
        assert_eq!(palette.selected, Some(0));
    }

    #[test]
    fn disabled_items_should_not_be_chosen() {
        let mut palette = Palette::default();
        palette.open(PaletteMode::Commands);
        assert_eq!(palette.choose(disabled("Reset view")), None);
        assert!(palette.is_open());
    }
}
