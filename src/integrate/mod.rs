//! Puts the viewer in the operating system's app launcher, so Start search,
//! PowerToys Run, Raycast, GNOME, KDE or rofi can start it: a Start Menu
//! shortcut on Windows, a desktop entry and an icon on Linux. Per user, no
//! admin rights. Nothing yet on macOS.
//!
//! `copcat integrate` does it (or `--remove` undoes it) whenever it's asked.
//! [`register_on_first_run`] does it once, the first time the programs run,
//! and leaves a marker file in the user's settings directory so it never
//! does it again: a shortcut the user deleted stays deleted.

#[cfg(any(test, all(unix, not(target_os = "macos"))))]
mod freedesktop;
#[cfg(windows)]
mod shortcut;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Copcat's icon, 256 pixels square (`assets/icon.png`).
pub const ICON_PNG: &[u8] = include_bytes!("../../assets/icon.png");

/// The viewer window's app id (Wayland) and class (X11) on Linux, which
/// docks match to the desktop entry's name and `StartupWMClass`.
pub const APP_ID: &str = "copcat";

/// The marker [`register_on_first_run`] leaves in the user's settings
/// directory.
pub const MARKER_FILE: &str = "launcher-registered.txt";

/// Set (to anything) to stop [`register_on_first_run`] from doing anything.
pub const DISABLE_ENV: &str = "COPCAT_NO_AUTO_INTEGRATE";

/// Whether this platform has a launcher Copcat knows how to join.
pub const SUPPORTED: bool = cfg!(any(windows, all(unix, not(target_os = "macos"))));

/// The launcher's name, for messages.
pub const LAUNCHER: &str = if cfg!(windows) {
    "the Start Menu"
} else {
    "the app launcher"
};

/// An environment lookup, so tests can give their own.
type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// Where the launcher entry goes and what it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integration {
    /// The `copcat-viewer` program the entry starts.
    pub viewer: PathBuf,
    /// The folder the viewer starts in.
    pub home: PathBuf,
    /// The Start Menu's Programs folder on Windows; the XDG data directory
    /// (`~/.local/share`) on Linux.
    pub dir: PathBuf,
}

impl Integration {
    /// The `copcat-viewer` next to this program, the home folder, and the
    /// launcher's folder from the environment.
    pub fn from_env() -> Result<Self, String> {
        let exe =
            std::env::current_exe().map_err(|e| format!("couldn't find this program: {e}"))?;
        let viewer = viewer_beside(&exe);
        if !viewer.is_file() {
            return Err(format!(
                "copcat-viewer isn't next to this program (looked for {})",
                viewer.display()
            ));
        }
        let home = crate::config::home_dir().ok_or("couldn't find your home folder")?;
        let dir = launcher_dir(&env_var, &home)
            .ok_or_else(|| format!("couldn't find {LAUNCHER}'s folder"))?;
        Ok(Self { viewer, home, dir })
    }

    /// Every file [`Self::install`] writes. Empty where it isn't
    /// [`SUPPORTED`].
    pub fn files(&self) -> Vec<PathBuf> {
        #[cfg(windows)]
        {
            vec![shortcut::path(&self.dir)]
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            freedesktop::files(&self.dir).to_vec()
        }
        #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
        {
            Vec::new()
        }
    }

    /// Writes the launcher entry, over any that's there, and returns the
    /// files written. Writes nothing where it isn't [`SUPPORTED`].
    pub fn install(&self) -> Result<Vec<PathBuf>, String> {
        #[cfg(windows)]
        {
            let path = shortcut::path(&self.dir);
            std::fs::create_dir_all(&self.dir)
                .map_err(|e| format!("couldn't create {}: {e}", self.dir.display()))?;
            shortcut::write(&path, &self.viewer, &self.home)?;
            Ok(vec![path])
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            freedesktop::install(&self.dir, &self.viewer, &self.home)
        }
        #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
        {
            Ok(Vec::new())
        }
    }

    /// Deletes what [`Self::install`] writes, and returns the files that
    /// were there. Nothing there is fine.
    pub fn remove(&self) -> Result<Vec<PathBuf>, String> {
        remove_files(&self.files())
    }
}

/// `copcat-viewer` in the same folder as `exe` (which may be the viewer).
fn viewer_beside(exe: &Path) -> PathBuf {
    exe.with_file_name(format!("copcat-viewer{}", std::env::consts::EXE_SUFFIX))
}

fn env_var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// The folder launcher entries go in, from the environment.
fn launcher_dir(env: Env, home: &Path) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let _ = home;
        start_menu_dir(env)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Some(freedesktop::data_dir(env, home))
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = (env, home);
        None
    }
}

/// The current user's Start Menu Programs folder, under `%APPDATA%`.
#[cfg(any(test, windows))]
fn start_menu_dir(env: Env) -> Option<PathBuf> {
    let appdata = PathBuf::from(env("APPDATA")?);
    Some(
        appdata
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs"),
    )
}

fn remove_files(files: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let mut removed = Vec::new();
    for file in files {
        match std::fs::remove_file(file) {
            Ok(()) => removed.push(file.clone()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("couldn't remove {}: {e}", file.display())),
        }
    }
    Ok(removed)
}

/// What [`register_on_first_run`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum FirstRun {
    /// Not attempted: unsupported platform, a development build, turned off
    /// with [`DISABLE_ENV`], or nowhere to record it.
    Skipped,
    /// The marker is there: it was done (or tried) before.
    AlreadyDone,
    /// Registered now; these files were written.
    Registered(Vec<PathBuf>),
    /// Tried now and failed. It won't be tried again on its own.
    Failed(String),
}

/// The first time Copcat runs, adds the viewer to the launcher, once. Never
/// fails and never prints more than one line, to stderr.
///
/// Skipped on platforms without [`SUPPORTED`] launchers, when [`DISABLE_ENV`]
/// is set, when this program runs from a Cargo target directory (a
/// development build or a test), and when there's no user settings
/// directory to keep the marker in.
pub fn register_on_first_run() -> FirstRun {
    let outcome = first_run_from_env();
    match &outcome {
        FirstRun::Registered(files) => {
            if let Some(file) = files.first() {
                eprintln!(
                    "copcat: added Copcat to {LAUNCHER} ({}); `copcat integrate --remove` takes it out",
                    file.display()
                );
            }
        }
        FirstRun::Failed(message) => eprintln!(
            "copcat: couldn't add Copcat to {LAUNCHER}: {message} (`copcat integrate` tries again)"
        ),
        FirstRun::Skipped | FirstRun::AlreadyDone => {}
    }
    outcome
}

fn first_run_from_env() -> FirstRun {
    if !SUPPORTED || env_var(DISABLE_ENV).is_some() {
        return FirstRun::Skipped;
    }
    let Ok(exe) = std::env::current_exe() else {
        return FirstRun::Skipped;
    };
    if in_cargo_target_dir(&exe) {
        return FirstRun::Skipped;
    }
    let Some(user_dir) = crate::config::Locations::from_env().user_dir else {
        return FirstRun::Skipped;
    };
    first_run(&user_dir, || Integration::from_env()?.install())
}

/// Runs `register` unless `user_dir` has the marker, writing the marker
/// first. If the marker can't be written, `register` isn't run either, so
/// it isn't run again on every start.
fn first_run(user_dir: &Path, register: impl FnOnce() -> Result<Vec<PathBuf>, String>) -> FirstRun {
    let marker = user_dir.join(MARKER_FILE);
    if marker.exists() {
        return FirstRun::AlreadyDone;
    }
    if std::fs::create_dir_all(user_dir).is_err() {
        return FirstRun::Skipped;
    }
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return FirstRun::AlreadyDone,
        Err(_) => return FirstRun::Skipped,
    };
    let note = format!(
        "Copcat {} added itself to {LAUNCHER} the first time it ran, and left this\n\
         file so that it never does so on its own again. `copcat integrate` adds\n\
         it again, and `copcat integrate --remove` takes it out.\n",
        env!("CARGO_PKG_VERSION")
    );
    if file.write_all(note.as_bytes()).is_err() {
        return FirstRun::Skipped;
    }
    drop(file);
    match register() {
        Ok(files) => FirstRun::Registered(files),
        Err(message) => FirstRun::Failed(message),
    }
}

/// Whether `exe` is in a Cargo target directory, which Cargo marks with a
/// `CACHEDIR.TAG`: a development build, or a test running one.
fn in_cargo_target_dir(exe: &Path) -> bool {
    exe.ancestors()
        .skip(1)
        .any(|dir| dir.join("CACHEDIR.TAG").is_file())
}
