// Release builds are Windows GUI programs, so launching one doesn't open a
// console window. Debug builds keep the console for logs and panics.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    // `cargo run -- <url-or-path>` loads a cloud on startup.
    let initial_source = std::env::args().nth(1);
    copcat::run_viewer(initial_source)
}
