//! `mini-launcher-tray`: the background tray app. No console window; runs
//! the same loop as `mini-launcher run` (see `mini_launcher_core::runner`).
//!
//! `--settings` also opens the settings window (a static mock for now, #64)
//! once the tray icon is up (repeat it to reopen after each close); the tray
//! menu entry for it comes later.
//!
//! Windows-only for now. Elsewhere it only points at `mini-launcher run`.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg_attr(not(windows), allow(dead_code))]
mod model;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    app::main()
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("mini-launcher-tray is Windows-only for now; use `mini-launcher run` instead.");
    std::process::ExitCode::FAILURE
}
