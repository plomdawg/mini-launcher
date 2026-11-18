//! `mini-launcher-tray`: the background tray app. No console window; runs
//! the same loop as `mini-launcher run` (see `mini_launcher_core::runner`).
//!
//! The menu's "Settings…" opens the settings window on its own thread (see
//! `mini_launcher_ui::window`). `--settings` opens it at startup (repeat it to
//! reopen after each close; CI uses that).
//!
//! `--quit` asks the running tray app to quit (as setup does before an
//! upgrade), waits up to 10 s, and fails if it is still running: it never
//! quits while the settings window is open.
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
