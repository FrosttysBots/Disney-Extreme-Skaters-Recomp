// No console window in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Disney's Extreme Skate Adventure on its own: the Map Viewer without its
//! panel, debug keys or markers, booting through the opening movies into
//! the game's main menu.

#[allow(unused_attributes)]
#[path = "../map_viewer/main.rs"]
mod map_viewer;

fn main() -> anyhow::Result<()> {
    map_viewer::run(true)
}
