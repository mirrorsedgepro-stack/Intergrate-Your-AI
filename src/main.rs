//! The desktop program (Windows, macOS, Linux). The app itself lives in the library so Android can load it too.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> eframe::Result { nexus_app::desktop_main() }
