//! Finding the COPC files under a folder, for a folder connector. Knows
//! nothing about egui; it blocks, so run it on a thread.
//!
//! The walk is bounded so pointing it at a huge tree can't hang or flood
//! the list: hidden folders and build or package folders are skipped,
//! symbolic links to folders aren't followed (so a link loop can't trap
//! it), and depth, files listed and entries looked at are capped.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// How far a scan may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Limits {
    /// How many folders down from the root are searched.
    pub(crate) max_depth: usize,
    /// How many files are listed; the rest are counted.
    pub(crate) max_files: usize,
    /// How many directory entries are looked at before the scan stops.
    pub(crate) max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 16,
            max_files: 2_000,
            max_entries: 200_000,
        }
    }
}

/// What a scan found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scan {
    /// Relative to the root, sorted.
    pub(crate) files: Vec<PathBuf>,
    /// COPC files found past [`Limits::max_files`].
    pub(crate) not_listed: usize,
    /// The scan stopped before looking everywhere: it hit
    /// [`Limits::max_entries`], or folders were deeper than
    /// [`Limits::max_depth`]. There may be more files.
    pub(crate) incomplete: bool,
}

/// Folders never searched, besides hidden ones (named starting with `.`).
const SKIPPED_FOLDERS: [&str; 3] = ["target", "node_modules", "__pycache__"];

/// Whether `name` is a COPC file's name.
pub(crate) fn is_copc_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".copc.laz")
}

fn skip_folder(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_FOLDERS.contains(&name)
}

/// The COPC files under `root`. Folders that can't be read are skipped;
/// only `root` itself not being a readable folder is an error.
pub(crate) fn scan(root: &Path, limits: Limits) -> io::Result<Scan> {
    let mut found = Vec::new();
    let mut scan = Scan::default();
    let mut entries_seen = 0;
    // Depth-first, with the folders to visit and how deep each is.
    let mut folders = vec![(root.to_path_buf(), 0)];
    let mut first = true;
    while let Some((folder, depth)) = folders.pop() {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if first => return Err(error),
            Err(_) => continue,
        };
        first = false;
        for entry in entries.flatten() {
            entries_seen += 1;
            if entries_seen > limits.max_entries {
                scan.incomplete = true;
                folders.clear();
                break;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let path = entry.path();
            // Not followed if it's a link, so a loop of links can't trap
            // the scan.
            if file_type.is_dir() {
                if skip_folder(&name) {
                    continue;
                }
                if depth + 1 > limits.max_depth {
                    scan.incomplete = true;
                    continue;
                }
                folders.push((path, depth + 1));
                continue;
            }
            // A link to a file counts; a link to a folder doesn't.
            let is_file = file_type.is_file()
                || (file_type.is_symlink() && fs::metadata(&path).is_ok_and(|m| m.is_file()));
            if is_file && is_copc_name(&name) {
                found.push(path);
            }
        }
    }
    found.sort();
    if found.len() > limits.max_files {
        scan.not_listed = found.len() - limits.max_files;
        found.truncate(limits.max_files);
    }
    scan.files = found
        .into_iter()
        .map(|path| {
            path.strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or(path)
        })
        .collect();
    Ok(scan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn names(scan: &Scan) -> Vec<String> {
        scan.files
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn scan_should_find_copc_files_recursively_and_sorted() {
        let root = tempfile::tempdir().unwrap();
        touch(root.path(), "b.copc.laz");
        touch(root.path(), "site/a.COPC.LAZ");
        touch(root.path(), "site/deeper/c.copc.laz");
        touch(root.path(), "site/not-copc.laz");
        touch(root.path(), "site/readme.txt");
        let scan = scan(root.path(), Limits::default()).unwrap();
        assert_eq!(
            names(&scan),
            ["b.copc.laz", "site/a.COPC.LAZ", "site/deeper/c.copc.laz"]
        );
        assert_eq!(scan.not_listed, 0);
        assert!(!scan.incomplete);
    }

    #[test]
    fn scan_should_skip_hidden_and_build_folders() {
        let root = tempfile::tempdir().unwrap();
        touch(root.path(), ".git/x.copc.laz");
        touch(root.path(), ".cache/x.copc.laz");
        touch(root.path(), "target/x.copc.laz");
        touch(root.path(), "web/node_modules/x.copc.laz");
        touch(root.path(), "keep/x.copc.laz");
        let scan = scan(root.path(), Limits::default()).unwrap();
        assert_eq!(names(&scan), ["keep/x.copc.laz"]);
    }

    #[test]
    fn scan_should_cap_files_and_count_the_rest() {
        let root = tempfile::tempdir().unwrap();
        for at in 0..7 {
            touch(root.path(), &format!("{at}.copc.laz"));
        }
        let limits = Limits {
            max_files: 5,
            ..Limits::default()
        };
        let scan = scan(root.path(), limits).unwrap();
        assert_eq!(scan.files.len(), 5);
        assert_eq!(scan.not_listed, 2);
    }

    #[test]
    fn scan_should_stop_at_the_depth_limit() {
        let root = tempfile::tempdir().unwrap();
        touch(root.path(), "1/a.copc.laz");
        touch(root.path(), "1/2/3/b.copc.laz");
        let limits = Limits {
            max_depth: 2,
            ..Limits::default()
        };
        let scan = scan(root.path(), limits).unwrap();
        assert_eq!(names(&scan), ["1/a.copc.laz"]);
        assert!(scan.incomplete);
    }

    #[test]
    fn scan_should_stop_after_looking_at_enough_entries() {
        let root = tempfile::tempdir().unwrap();
        for at in 0..10 {
            touch(root.path(), &format!("{at}.txt"));
        }
        let limits = Limits {
            max_entries: 4,
            ..Limits::default()
        };
        assert!(scan(root.path(), limits).unwrap().incomplete);
    }

    #[test]
    fn scan_should_fail_for_a_missing_root() {
        let root = tempfile::tempdir().unwrap();
        assert!(scan(&root.path().join("missing"), Limits::default()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn scan_should_not_follow_a_link_loop() {
        let root = tempfile::tempdir().unwrap();
        touch(root.path(), "a/x.copc.laz");
        std::os::unix::fs::symlink(root.path(), root.path().join("a/loop")).unwrap();
        let scan = scan(root.path(), Limits::default()).unwrap();
        assert_eq!(names(&scan), ["a/x.copc.laz"]);
    }
}
