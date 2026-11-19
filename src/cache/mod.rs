//! What the viewer keeps on disk between runs so it needn't fetch it again:
//! data sources' listings ([`stac`]) and their thumbnails, under `cache/` in
//! the user's settings directory (`~/.copcat/cache`, or in
//! `COPCAT_CONFIG_DIR`). Knows nothing about egui.
//!
//! Each kind of thing is a [`DiskCache`]: a folder of small files, written
//! whole (to a temporary file, then renamed over the old one) so a crash or
//! a second copy of Copcat writing at the same time can't leave half a file.
//! A file's modified time is when it was last written or read, so the least
//! recently used are removed first when a folder is over its size cap, and
//! files not used for [`Limits::max_age`] are removed too. Anything that
//! can't be read is a miss, never an error: the cache is only ever a
//! shortcut.

pub(crate) mod stac;

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

pub(crate) use stac::StacCache;

/// The time now. Injected so tests can say when "now" is.
pub(crate) type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The wall clock.
pub(crate) fn system_clock() -> Clock {
    Arc::new(SystemTime::now)
}

const MB: u64 = 1024 * 1024;
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Thumbnails, already shrunk to a row's size and saved as PNG: a few KB
/// each.
pub(crate) const THUMBNAIL_LIMITS: Limits = Limits {
    total: 10 * MB,
    file: 256 * 1024,
    max_age: Duration::from_secs(30 * DAY.as_secs()),
};

/// How big a cache folder may get, and how long a file is kept unused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// All its files together, in bytes.
    pub(crate) total: u64,
    /// One file, in bytes. A bigger one isn't written.
    pub(crate) file: u64,
    pub(crate) max_age: Duration,
}

/// Every cache, in one settings directory.
#[derive(Clone)]
pub(crate) struct Caches {
    pub(crate) stac: StacCache,
    pub(crate) thumbnails: DiskCache,
}

impl Caches {
    /// The caches under `cache/` in the settings directory `config_dir`.
    /// Nothing is created until something is written.
    pub(crate) fn in_config_dir(config_dir: &Path, clock: Clock) -> Self {
        let root = config_dir.join("cache");
        Self {
            stac: StacCache::new(root.join("stac"), clock.clone()),
            thumbnails: DiskCache::new(root.join("thumbnails"), THUMBNAIL_LIMITS, clock),
        }
    }

    /// Removes old and excess files from every cache.
    pub(crate) fn prune(&self) {
        self.stac.prune();
        self.thumbnails.prune();
    }
}

/// What a removal removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Removed {
    pub(crate) files: usize,
    pub(crate) bytes: u64,
}

impl Removed {
    fn add(&mut self, bytes: u64) {
        self.files += 1;
        self.bytes += bytes;
    }
}

/// A folder of small files by name. See the module docs.
#[derive(Clone)]
pub(crate) struct DiskCache {
    dir: PathBuf,
    limits: Limits,
    clock: Clock,
}

/// Makes temporary files' names unique within this process; the process id
/// makes them unique between processes.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temporary file older than this was left by a crash.
const ABANDONED: Duration = Duration::from_secs(60 * 60);

impl DiskCache {
    pub(crate) fn new(dir: PathBuf, limits: Limits, clock: Clock) -> Self {
        Self { dir, limits, clock }
    }

    #[cfg(test)]
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// The file called `name`, marking it used, or `None` if it's missing,
    /// too big or can't be read.
    pub(crate) fn read(&self, name: &str) -> Option<Vec<u8>> {
        let path = self.dir.join(name);
        if fs::metadata(&path).ok()?.len() > self.limits.file {
            return None;
        }
        let bytes = fs::read(&path).ok()?;
        self.touch(&path);
        Some(bytes)
    }

    /// Writes `bytes` as `name`, replacing it whole, then removes old and
    /// excess files. Returns whether it was written: not if it's over the
    /// per-file cap (any old copy is removed then, as out of date) or the
    /// disk refused.
    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> bool {
        let path = self.dir.join(name);
        if bytes.len() as u64 > self.limits.file {
            let _ = fs::remove_file(&path);
            return false;
        }
        let written = self.write_atomically(&path, bytes);
        self.prune();
        written
    }

    fn write_atomically(&self, path: &Path, bytes: &[u8]) -> bool {
        if fs::create_dir_all(&self.dir).is_err() {
            return false;
        }
        let temp = self.dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let result = File::create(&temp).and_then(|mut file| {
            file.write_all(bytes)?;
            file.set_modified((self.clock)())?;
            file.sync_all()
        });
        // Renaming over the old file replaces it in one step, whoever else
        // is reading or writing it.
        if result.and_then(|()| fs::rename(&temp, path)).is_err() {
            let _ = fs::remove_file(&temp);
            return false;
        }
        true
    }

    /// Marks `path` used now, for least-recently-used eviction.
    fn touch(&self, path: &Path) {
        if let Ok(file) = File::options().write(true).open(path) {
            let _ = file.set_modified((self.clock)());
        }
    }

    /// The cache's files (not temporary ones): name, size and when last
    /// used.
    fn files(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let Ok(dir) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        dir.flatten()
            .filter_map(|entry| {
                let metadata = entry.metadata().ok()?;
                metadata.is_file().then(|| {
                    let used = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    (entry.path(), metadata.len(), used)
                })
            })
            .collect()
    }

    /// Removes files not used for [`Limits::max_age`], then the least
    /// recently used until the rest fit in [`Limits::total`].
    pub(crate) fn prune(&self) -> Removed {
        let now = (self.clock)();
        let mut removed = Removed::default();
        let mut kept = Vec::new();
        for (path, size, used) in self.files() {
            let age = now.duration_since(used).unwrap_or_default();
            let temporary = is_temporary(&path);
            let expired = if temporary {
                age > ABANDONED
            } else {
                age > self.limits.max_age
            };
            if expired {
                if fs::remove_file(&path).is_ok() {
                    removed.add(size);
                }
            } else if !temporary {
                kept.push((path, size, used));
            }
        }
        let mut total: u64 = kept.iter().map(|(_, size, _)| size).sum();
        kept.sort_by_key(|(_, _, used)| *used);
        for (path, size, _) in kept {
            if total <= self.limits.total {
                break;
            }
            total -= size;
            if fs::remove_file(&path).is_ok() {
                removed.add(size);
            }
        }
        removed
    }

    /// Removes the files whose names start with `prefix`.
    pub(crate) fn remove_prefixed(&self, prefix: &str) -> Removed {
        let mut removed = Removed::default();
        for (path, size, _) in self.files() {
            let matches = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(prefix));
            if matches && fs::remove_file(&path).is_ok() {
                removed.add(size);
            }
        }
        removed
    }

    /// Removes every file.
    pub(crate) fn clear(&self) -> Removed {
        self.remove_prefixed("")
    }
}

fn is_temporary(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') && name.ends_with(".tmp"))
}

/// A 64-bit FNV-1a hash of `text`, as 16 hex digits: the same in every
/// build and on every machine, unlike `std`'s hasher, so cache files keep
/// their names between versions.
pub(crate) fn stable_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// "just now", "5 minutes ago", "3 days ago": how long ago `then` was.
pub(crate) fn ago(now: SystemTime, then: SystemTime) -> String {
    let seconds = now.duration_since(then).unwrap_or_default().as_secs();
    let plural = |count: u64, unit: &str| match count {
        1 => format!("1 {unit} ago"),
        _ => format!("{count} {unit}s ago"),
    };
    match seconds {
        0..60 => "just now".to_owned(),
        60..3600 => plural(seconds / 60, "minute"),
        3600..86_400 => plural(seconds / 3600, "hour"),
        _ => plural(seconds / 86_400, "day"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A clock that says what it's told.
    pub(crate) struct FakeClock(Mutex<SystemTime>);

    impl FakeClock {
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )))
        }

        pub(crate) fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }

        pub(crate) fn now(&self) -> SystemTime {
            *self.0.lock().unwrap()
        }

        pub(crate) fn clock(self: &Arc<Self>) -> Clock {
            let fake = Arc::clone(self);
            Arc::new(move || fake.now())
        }
    }

    fn cache(dir: &Path, limits: Limits, clock: &Arc<FakeClock>) -> DiskCache {
        DiskCache::new(dir.join("c"), limits, clock.clock())
    }

    const SMALL: Limits = Limits {
        total: 250,
        file: 100,
        max_age: Duration::from_secs(1000),
    };

    fn names(cache: &DiskCache) -> Vec<String> {
        let mut names: Vec<_> = cache
            .files()
            .into_iter()
            .map(|(path, ..)| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_should_read_back_as_written_and_a_missing_one_should_miss() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), SMALL, &clock);
        assert_eq!(cache.read("a"), None);
        assert!(cache.write("a", b"hello"));
        assert_eq!(cache.read("a").as_deref(), Some(&b"hello"[..]));
        assert!(cache.write("a", b"again"));
        assert_eq!(cache.read("a").as_deref(), Some(&b"again"[..]));
    }

    #[test]
    fn a_write_should_leave_no_temporary_file_and_replace_the_old_one_whole() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), SMALL, &clock);
        assert!(cache.write("a", &[1; 50]));
        assert!(cache.write("a", &[2; 20]));
        let in_dir: Vec<_> = fs::read_dir(cache.dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(in_dir, ["a"]);
        assert_eq!(cache.read("a").unwrap(), [2; 20]);
    }

    #[test]
    fn a_crashed_writes_temporary_file_should_be_ignored_then_removed() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), SMALL, &clock);
        fs::create_dir_all(cache.dir()).unwrap();
        let leftover = cache.dir().join(".a.1.0.tmp");
        let file = File::create(&leftover).unwrap();
        file.set_modified(clock.now()).unwrap();
        drop(file);
        assert_eq!(cache.read("a"), None);
        cache.prune();
        assert!(leftover.exists(), "it may still be being written");
        clock.advance(ABANDONED * 2);
        cache.prune();
        assert!(!leftover.exists());
    }

    #[test]
    fn a_file_over_the_cap_should_not_be_written_and_should_remove_the_old_copy() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), SMALL, &clock);
        assert!(cache.write("a", &[0; 100]));
        assert!(!cache.write("a", &[0; 101]));
        assert_eq!(cache.read("a"), None);
    }

    #[test]
    fn the_least_recently_used_should_go_first_when_over_the_total() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), SMALL, &clock);
        for name in ["a", "b", "c"] {
            assert!(cache.write(name, &[0; 80]));
            clock.advance(Duration::from_secs(1));
        }
        // Reading "a" makes "b" the least recently used.
        assert!(cache.read("a").is_some());
        clock.advance(Duration::from_secs(1));
        assert!(cache.write("d", &[0; 80]));
        assert_eq!(names(&cache), ["a", "c", "d"]);
        let total: u64 = cache.files().iter().map(|(_, size, _)| size).sum();
        assert!(total <= SMALL.total);
    }

    #[test]
    fn files_unused_for_too_long_should_be_removed() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), SMALL, &clock);
        assert!(cache.write("old", b"x"));
        clock.advance(Duration::from_secs(600));
        assert!(cache.write("new", b"y"));
        clock.advance(Duration::from_secs(600));
        assert_eq!(cache.prune(), Removed { files: 1, bytes: 1 });
        assert_eq!(names(&cache), ["new"]);
    }

    #[test]
    fn removing_by_prefix_and_clearing_should_say_what_went() {
        let temp = tempfile::tempdir().unwrap();
        let clock = FakeClock::new();
        let cache = cache(temp.path(), SMALL, &clock);
        for name in ["s1-a", "s1-b", "s2-a"] {
            assert!(cache.write(name, b"12345"));
        }
        assert_eq!(
            cache.remove_prefixed("s1-"),
            Removed {
                files: 2,
                bytes: 10
            }
        );
        assert_eq!(names(&cache), ["s2-a"]);
        assert_eq!(cache.clear().files, 1);
        assert!(names(&cache).is_empty());
        // A cache that was never written has nothing to clear.
        let never = DiskCache::new(temp.path().join("none"), SMALL, clock.clock());
        assert_eq!(never.clear(), Removed::default());
    }

    #[test]
    fn hashes_should_never_change() {
        // File names depend on these: a change would orphan every cache.
        assert_eq!(stable_hash(""), "cbf29ce484222325");
        assert_eq!(stable_hash("a"), "af63dc4c8601ec8c");
        assert_eq!(
            stable_hash("https://stacapi.naturescan.earth/"),
            stable_hash("https://stacapi.naturescan.earth/")
        );
        assert_ne!(stable_hash("ab"), stable_hash("ba"));
    }

    #[test]
    fn ago_should_say_how_long_ago_in_words() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let before = |secs| now - Duration::from_secs(secs);
        assert_eq!(ago(now, before(5)), "just now");
        assert_eq!(ago(now, before(60)), "1 minute ago");
        assert_eq!(ago(now, before(3 * 3600 + 5)), "3 hours ago");
        assert_eq!(ago(now, before(3 * 86_400)), "3 days ago");
        assert_eq!(ago(now, now + Duration::from_secs(9)), "just now");
    }
}
