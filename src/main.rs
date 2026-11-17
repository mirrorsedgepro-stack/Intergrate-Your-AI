//! Nexus: one terminal for Claude Code, the Antigravity CLI, OpenClaw and shells, on this computer and on any
//! machine you can SSH into.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
// The code favours dense one-liners and callback types spelled out where they are used.
#![allow(clippy::type_complexity, clippy::too_many_arguments, clippy::suspicious_else_formatting, clippy::possible_missing_else, clippy::unnecessary_sort_by, clippy::sliced_string_as_bytes)]

mod backend;
mod collab;
mod config;
mod local;
mod mcp;
mod platform;
mod remote;
#[cfg(test)]
mod remote_tests;
mod sshconfig;
mod stats;
mod tailscale;
mod term;
mod ui;
mod util;

use eframe::egui;

fn main() -> eframe::Result {
    if std::env::args().any(|a| a == "--version" || a == "-V") { println!("Nexus {}", backend::VERSION); return Ok(()); }
    // One Nexus per user: a second start just exits (sessions belong to the window that is already open).
    let _ = std::fs::create_dir_all(config::dir());
    let lock_file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(config::dir().join("nexus.lock"));
    let mut lock = lock_file.ok().map(fd_lock::RwLock::new);
    let _guard = match lock.as_mut().map(|l| l.try_write()) {
        Some(Err(_)) => { eprintln!("Nexus is already running."); return Ok(()); }
        Some(Ok(g)) => Some(g),
        None => None,
    };

    // Before any thread exists: this changes the process environment.
    platform::load_login_shell_path();
    let mut cfg = config::load();
    if backend::Backend::add_wsl_machines(&mut cfg) { config::save(&cfg); }

    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("nexus-io").build().expect("runtime");
    let w = cfg.window.clone().unwrap_or_default();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Nexus").with_app_id("com.nexus.terminal")
        .with_inner_size([w.width.unwrap_or(1480.0).max(860.0), w.height.unwrap_or(920.0).max(520.0)])
        .with_min_inner_size([860.0, 520.0])
        .with_maximized(w.maximized);
    if let (Some(x), Some(y)) = (w.x, w.y) { viewport = viewport.with_position([x, y]); }
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")) { viewport = viewport.with_icon(icon); }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Nexus", options, Box::new(move |cc| Ok(Box::new(ui::App::new(cc, rt, cfg)))))
}
