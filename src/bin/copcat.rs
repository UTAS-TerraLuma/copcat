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
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Commands::Open { source } => open(&source),
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
    let viewer = viewer_path()?;
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

/// `copcat-viewer` is installed next to `copcat`.
fn viewer_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("couldn't find this program: {e}"))?;
    let viewer = exe.with_file_name(format!("copcat-viewer{}", std::env::consts::EXE_SUFFIX));
    if viewer.is_file() {
        Ok(viewer)
    } else {
        Err(format!(
            "copcat-viewer isn't next to copcat (looked for {})",
            viewer.display()
        ))
    }
}
