//! `copcat`: the viewer's command line.
//!
//! It's a separate program from `copcat-viewer` because on Windows a program is
//! either a console or a GUI program: a GUI one can't print to the terminal
//! that ran it, and a console one opens a terminal window from a shortcut.

use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about = "Command line for the Copcat point cloud viewer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Open a COPC point cloud (a URL or a file path) in a new viewer window.
    Open { source: String },
    /// Render a view of a COPC point cloud to a PNG, without opening a window.
    ///
    /// The viewer's Save image section has a Copy as command button that
    /// writes this command for the view on screen.
    Render(Box<copcat::RenderArgs>),
    /// Update Copcat to the latest release.
    Update,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Commands::Open { source } => open(&source),
        Commands::Render(args) => copcat::render(&args),
        Commands::Update => update(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("copcat: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Starts `copcat-viewer` on `source` and returns without waiting for it.
fn open(source: &str) -> Result<(), String> {
    let viewer = sibling_path("copcat-viewer")?;
    let mut command = Command::new(&viewer);
    command.arg(source);
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
    println!("Opened {source} in the viewer (pid {}).", child.id());
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
