//! `copcat`: the viewer's command line.
//!
//! It's a separate program from `copcat-viewer` because on Windows a program is
//! either a console or a GUI program: a GUI one can't print to the terminal
//! that ran it, and a console one opens a terminal window from a shortcut.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "Command line for the Copcat point cloud viewer",
    after_help = "With no command, opens an empty viewer window, like `copcat open`."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Open a COPC point cloud (a URL or a file path) in a new viewer window,
    /// or an empty window to open one from.
    Open {
        source: Option<String>,
        #[command(flatten)]
        heights: copcat::HeightSettings,
    },
    /// Render a view of a COPC point cloud to a PNG, without opening a window.
    ///
    /// The viewer's Save image section has a Copy as command button that
    /// writes this command for the view on screen.
    Render(Box<copcat::RenderArgs>),
    /// Update Copcat to the latest release.
    Update,
    /// Add the viewer to the Start Menu (Windows) or the app launcher
    /// (Linux), or take it out.
    ///
    /// Copcat does this once on its own the first time it runs. Run this to
    /// add it again (for example after moving the programs); it replaces
    /// what's there.
    Integrate {
        /// Take it out instead. Copcat won't add it again on its own.
        #[arg(long)]
        remove: bool,
    },
}

fn main() -> ExitCode {
    let command = Cli::parse().command;
    // Not for `render`, which scripts run, nor `integrate`, which does it
    // anyway (or is undoing it).
    if !matches!(
        command,
        Some(Commands::Render(_) | Commands::Integrate { .. })
    ) {
        copcat::integrate::register_on_first_run();
    }
    let result = match command {
        None => open(None, &copcat::HeightSettings::default()),
        Some(Commands::Open { source, heights }) => open(source.as_deref(), &heights),
        Some(Commands::Render(args)) => copcat::render(&args),
        Some(Commands::Update) => update(),
        Some(Commands::Integrate { remove }) => integrate(remove),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("copcat: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Starts `copcat-viewer`, on `source` if there is one, and returns without
/// waiting for it.
fn open(source: Option<&str>, heights: &copcat::HeightSettings) -> Result<(), String> {
    let viewer = sibling_path("copcat-viewer")?;
    let mut command = Command::new(&viewer);
    if let Some(source) = source {
        command.arg(source);
    }
    // `=` so a dimension name can't be read as a flag.
    command.arg(format!("--elevation={}", heights.elevation));
    command.arg(format!("--hag={}", heights.hag));
    // Detach from our stdio: a caller that reads our output to the end (an
    // agent, a script) would otherwise wait until the window is closed.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        // Its own process group, so closing the terminal doesn't close the window.
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        // No console, so a debug build doesn't share (or keep open) ours.
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        command.creation_flags(DETACHED_PROCESS);
    }

    let child = command
        .spawn()
        .map_err(|e| format!("couldn't start {}: {e}", viewer.display()))?;
    match source {
        Some(source) => println!("Opened {source} in the viewer (pid {}).", child.id()),
        None => println!("Opened the viewer (pid {}).", child.id()),
    }
    Ok(())
}

/// Runs `copcat-update`, the updater the installer puts next to `copcat`,
/// and waits for it.
fn update() -> Result<(), String> {
    let updater = sibling_path("copcat-update")?;
    // The Windows installer copies over copcat.exe, which fails while it's
    // running (and it is: we're waiting on the updater). A running exe can be
    // renamed, though, so move ours out of the way first.
    #[cfg(windows)]
    let moved = move_self_aside()?;

    let status = Command::new(&updater)
        .status()
        .map_err(|e| format!("couldn't start {}: {e}", updater.display()));

    // If nothing was installed in our place (no newer release, or the update
    // failed), put ourselves back.
    #[cfg(windows)]
    {
        let (exe, aside) = &moved;
        if !exe.exists() {
            std::fs::rename(aside, exe)
                .map_err(|e| format!("couldn't move {} back: {e}", aside.display()))?;
        }
    }

    let status = status?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("copcat-update failed ({status})"))
    }
}

/// Adds the viewer to the launcher, or with `remove` takes it out.
fn integrate(remove: bool) -> Result<(), String> {
    use copcat::integrate::{Integration, LAUNCHER, SUPPORTED};

    if !SUPPORTED {
        println!(
            "Adding Copcat to the app launcher isn't supported on {} yet. Nothing was changed.",
            std::env::consts::OS
        );
        return Ok(());
    }
    let integration = Integration::from_env()?;
    if remove {
        let removed = integration.remove()?;
        if removed.is_empty() {
            println!("Copcat wasn't in {LAUNCHER}. Nothing to remove; looked for:");
            print_paths(&integration.files());
        } else {
            println!("Removed Copcat from {LAUNCHER}:");
            print_paths(&removed);
        }
    } else {
        let written = integration.install()?;
        println!(
            "Added Copcat to {LAUNCHER}, starting {}:",
            integration.viewer.display()
        );
        print_paths(&written);
    }
    Ok(())
}

fn print_paths(paths: &[PathBuf]) {
    for path in paths {
        println!("  {}", path.display());
    }
}

/// Renames this program to `copcat.exe.old`, returning both paths. The
/// `.old` from the last update can't be removed while it runs, so it's
/// removed here instead.
#[cfg(windows)]
fn move_self_aside() -> Result<(PathBuf, PathBuf), String> {
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find this program: {e}"))?;
    let aside = exe.with_extension("exe.old");
    let _ = std::fs::remove_file(&aside);
    std::fs::rename(&exe, &aside)
        .map_err(|e| format!("couldn't move {} aside: {e}", exe.display()))?;
    Ok((exe, aside))
}

/// The other Copcat programs are installed next to `copcat`.
fn sibling_path(name: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find this program: {e}"))?;
    let path = exe.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "{name} isn't next to copcat (looked for {})",
            path.display()
        ))
    }
}
