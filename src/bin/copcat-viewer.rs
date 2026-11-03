// Release builds are Windows GUI programs, so launching one doesn't open a
// console window. Debug builds keep the console for logs and panics.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

/// The Copcat point cloud viewer.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// A COPC file path or URL to load on startup.
    source: Option<String>,
    #[command(flatten)]
    heights: copcat::HeightSettings,
}

fn main() -> eframe::Result {
    // `cargo run -- <url-or-path>` loads a cloud on startup.
    let args = Args::parse();
    copcat::run_viewer(args.source, args.heights)
}
