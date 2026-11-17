//! The Windows tray: a notification-area icon with a menu on the UI thread,
//! and the shared run loop on a worker thread.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use mini_launcher_cli::{logging, port};
use mini_launcher_core::autostart::{self, Status};
use mini_launcher_core::config::edit::saved_by_this_process;
use mini_launcher_core::config::{self, Config};
use mini_launcher_core::reconnect::{Backoff, ReaderState, Reconnector, SystemClock};
use mini_launcher_core::runner::{self, Notice, Runner};
use mini_launcher_core::{instance, logs};
use muda::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconAttributes};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use crate::model::{self, Level, RateLimiter, APP};

/// At most one balloon per this long, so a bad tag left on the reader
/// cannot flood the notification center.
const NOTIFY_GAP: Duration = Duration::from_secs(10);
/// How often the worker checks whether `config.toml` changed (in polls).
const CONFIG_CHECK_POLLS: u32 = 10;
/// How long the UI thread waits for window messages between other checks.
const UI_WAIT_MS: u32 = 50;
/// Placeholder port name until auto-detection finds the reader.
const AUTO_PORT: &str = "auto-detected port";

/// From the worker to the UI thread.
enum Update {
    Reader(ReaderState),
    Notice(Notice),
    ConfigError(String),
}

pub fn main() -> ExitCode {
    let log_dir = logs::dir();
    // Held until exit so buffered file lines get flushed.
    let _log_guard = logging::init(0, log_dir.as_deref());
    tracing::info!("mini-launcher-tray {}", mini_launcher_core::version());
    // There is no console: send panics to the log file too.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));
    let _instance = match instance::acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            tracing::info!("another instance is running; exiting");
            message_box("Mini Launcher is already running.", false);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            tracing::error!("cannot check for another running instance: {e}");
            message_box(&format!("Mini Launcher cannot start: {e}"), true);
            return ExitCode::FAILURE;
        }
    };
    match run(log_dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            message_box(&format!("Mini Launcher stopped: {e:#}"), true);
            ExitCode::FAILURE
        }
    }
}

fn run(log_dir: Option<PathBuf>) -> Result<()> {
    let config_path = Config::default_path();
    if config_path.is_none() {
        tracing::warn!("no config directory on this system; only steam:// tags will work");
    }
    let exe = std::env::current_exe().context("cannot find this program's path")?;
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("run-loop".into())
        .spawn({
            let (path, stop, paused) = (config_path.clone(), stop.clone(), paused.clone());
            move || work(path, &stop, &paused, &tx)
        })
        .context("cannot start the run loop")?;

    let status = MenuItem::new("", false, None);
    let pause = MenuItem::new("", true, None);
    let open_config = MenuItem::new("Open config", config_path.is_some(), None);
    let open_logs = MenuItem::new("Open logs folder", log_dir.is_some(), None);
    let start_at_login = CheckMenuItem::new("Start at login", true, starts_at_login(&exe), None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &status,
        &PredefinedMenuItem::separator(),
        &pause,
        &open_config,
        &open_logs,
        &start_at_login,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .context("cannot build the tray menu")?;
    // The id is fixed so the icon's Win32 identity is known (see `balloon`).
    let tray = TrayIcon::with_id(
        "mini-launcher",
        TrayIconAttributes {
            icon: Some(tray_icon()),
            menu: Some(Box::new(menu)),
            ..Default::default()
        },
    )
    .context("cannot create the tray icon")?;
    if tray_window().is_none() {
        tracing::error!(
            "tray-icon's window (class tray_icon_app) not found: notifications will not \
             show; re-check balloon() against the tray-icon version in use"
        );
    }

    let mut reader: Option<ReaderState> = None;
    let mut limiter = RateLimiter::new(NOTIFY_GAP);
    let refresh = |reader: Option<&ReaderState>| {
        let view = model::view(reader, paused.load(Ordering::Relaxed));
        if let Err(e) = tray.set_tooltip(Some(&view.tooltip)) {
            tracing::warn!("cannot set the tray tooltip: {e}");
        }
        status.set_text(&view.status);
        pause.set_text(view.pause_label);
    };
    refresh(None);

    'ui: loop {
        if !pump_messages() {
            break;
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == quit.id() {
                break 'ui;
            } else if id == pause.id() {
                let now_paused = !paused.load(Ordering::Relaxed);
                paused.store(now_paused, Ordering::Relaxed);
                tracing::info!(paused = now_paused, "pause toggled from the tray");
                refresh(reader.as_ref());
            } else if id == open_config.id() {
                if let Some(path) = &config_path {
                    if let Err(e) = edit_config(path) {
                        tracing::warn!("cannot open {}: {e:#}", path.display());
                        balloon(Level::Warning, "Cannot open the config", &format!("{e:#}"));
                    }
                }
            } else if id == open_logs.id() {
                if let Some(dir) = &log_dir {
                    let result =
                        std::fs::create_dir_all(dir).and_then(|()| open::that_detached(dir));
                    if let Err(e) = result {
                        tracing::warn!("cannot open {}: {e}", dir.display());
                    }
                }
            } else if id == start_at_login.id() {
                toggle_start_at_login(&exe);
                start_at_login.set_checked(starts_at_login(&exe));
            }
        }
        loop {
            match rx.try_recv() {
                Ok(Update::Reader(state)) => {
                    reader = Some(state);
                    refresh(reader.as_ref());
                }
                Ok(Update::Notice(notice)) => {
                    if let Some((level, title, text)) = model::notification(&notice) {
                        if limiter.allow(Instant::now()) {
                            balloon(level, title, &text);
                        }
                    }
                }
                Ok(Update::ConfigError(e)) => balloon(Level::Warning, "Config not loaded", &e),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    stop.store(true, Ordering::Relaxed);
                    let _ = worker.join();
                    anyhow::bail!("the run loop ended unexpectedly; see the log file");
                }
            }
        }
    }
    tracing::info!("quitting");
    stop.store(true, Ordering::Relaxed);
    drop(tray);
    // The worker notices `stop` within one poll (or one reopen attempt).
    let _ = worker.join();
    Ok(())
}

/// The worker: the shared run loop, with the reader opened (and reopened)
/// in the background so a missing reader never stops the app.
fn work(
    config_path: Option<PathBuf>,
    stop: &AtomicBool,
    paused: &AtomicBool,
    tx: &mpsc::Sender<Update>,
) {
    let mut stamp = config_path.as_deref().and_then(modified);
    // The text behind the running library, to skip no-op reloads.
    let mut loaded = config_path
        .as_deref()
        .and_then(|p| config::read_text_or_empty(p).ok());
    let config = match config_path.as_deref().map(Config::load_or_empty) {
        Some(Ok(config)) => config,
        Some(Err(e)) => {
            let e = config_error(config_path.as_deref(), &e);
            tracing::warn!("{e}; starting with an empty library");
            let _ = tx.send(Update::ConfigError(e));
            Config::default()
        }
        None => Config::default(),
    };
    let mut runner = Runner::new(config, false);
    let mut reader = Reconnector::unopened(
        AUTO_PORT.to_owned(),
        || port::reopen_poller(None),
        Backoff::default(),
    );
    let mut last_state = None;
    let mut polls = 0u32;
    let next = |runner: &mut Runner, now| {
        runner.set_paused(paused.load(Ordering::Relaxed));
        polls = polls.wrapping_add(1);
        if let Some(path) = config_path
            .as_deref()
            .filter(|_| polls.is_multiple_of(CONFIG_CHECK_POLLS))
        {
            let current = modified(path);
            if current != stamp {
                stamp = current;
                let text = config::read_text_or_empty(path);
                // Touched but unchanged (e.g. re-saved as is): nothing to do.
                if text.as_ref().ok() != loaded.as_ref() {
                    match text.and_then(|t| Config::parse(&t).map(|c| (c, t))) {
                        Ok((config, text)) => {
                            // This app's own saves are expected; log outside edits.
                            if !saved_by_this_process(&text) {
                                tracing::info!("config reloaded ({} games)", config.games.len());
                            }
                            runner.set_config(config);
                            loaded = Some(text);
                        }
                        Err(e) => {
                            let e = config_error(Some(path), &e);
                            tracing::warn!("{e}; keeping the previous library");
                            tx.send(Update::ConfigError(e)).ok()?;
                        }
                    }
                }
            }
        }
        let tick = reader.tick(now);
        if last_state.as_ref() != Some(reader.state()) {
            last_state = Some(reader.state().clone());
            tx.send(Update::Reader(reader.state().clone())).ok()?;
        }
        Some(tick)
    };
    let notify = |notice| tx.send(Update::Notice(notice)).map_err(drop);
    // A send error means the UI is gone: stop quietly.
    let _ = runner::poll_loop(&mut runner, stop, &mut SystemClock, next, notify);
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn config_error(path: Option<&Path>, e: &dyn std::error::Error) -> String {
    let mut msg = match path {
        Some(p) => format!("{}: {e}", p.display()),
        None => e.to_string(),
    };
    let mut source = e.source();
    while let Some(s) = source {
        msg.push_str(&format!(": {s}"));
        source = s.source();
    }
    msg
}

/// Opens `config.toml` in the default editor, creating a commented template
/// first if it is missing. Falls back to Notepad when `.toml` has no
/// associated program.
fn edit_config(path: &Path) -> Result<()> {
    if model::ensure_config(path).context("cannot create the config file")? {
        tracing::info!("created {}", path.display());
    }
    if let Err(e) = open::that_detached(path) {
        tracing::info!("no program for .toml files ({e}); using Notepad");
        std::process::Command::new("notepad.exe")
            .arg(path)
            .spawn()
            .context("cannot start Notepad")?;
    }
    Ok(())
}

fn starts_at_login(exe: &Path) -> bool {
    matches!(
        autostart::status(exe),
        Ok(Status::Enabled { this_exe: true, .. })
    )
}

/// Points start-at-login at this exe (no arguments), or turns it off when it
/// already does.
fn toggle_start_at_login(exe: &Path) {
    let result = if starts_at_login(exe) {
        autostart::disable().map(|()| "disabled".to_owned())
    } else {
        autostart::enable(exe, &[]).map(|cmd| format!("enabled: {cmd}"))
    };
    match result {
        Ok(msg) => tracing::info!("start at login {msg}"),
        Err(e) => {
            tracing::warn!("cannot change start at login: {e}");
            balloon(Level::Warning, "Start at login not changed", &e.to_string());
        }
    }
}

/// The exe's embedded icon (resource 1, see `build.rs`) at the small-icon
/// size for the current DPI.
fn tray_icon() -> Icon {
    let size = unsafe { wm::GetSystemMetrics(wm::SM_CXSMICON) }.max(16) as u32;
    Icon::from_resource(1, Some((size, size))).unwrap_or_else(|e| {
        tracing::warn!("no embedded icon ({e}); using a plain one");
        let rgba = [0x2b, 0x6c, 0xb0, 0xff].repeat(16 * 16);
        Icon::from_rgba(rgba, 16, 16).expect("16x16 RGBA buffer")
    })
}

/// Dispatches pending window messages, then waits briefly for more.
/// Returns `false` once `WM_QUIT` arrives.
fn pump_messages() -> bool {
    // SAFETY: plain Win32 message loop on the thread that owns the tray window.
    unsafe {
        let mut msg: wm::MSG = std::mem::zeroed();
        while wm::PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, wm::PM_REMOVE) != 0 {
            if msg.message == wm::WM_QUIT {
                return false;
            }
            wm::TranslateMessage(&msg);
            wm::DispatchMessageW(&msg);
        }
        wm::MsgWaitForMultipleObjects(0, std::ptr::null(), 0, UI_WAIT_MS, wm::QS_ALLINPUT);
    }
    true
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn message_box(text: &str, error: bool) {
    let icon = if error {
        wm::MB_ICONERROR
    } else {
        wm::MB_ICONINFORMATION
    };
    let (text, caption) = (wide(text), wide(APP));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        wm::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            wm::MB_OK | icon,
        );
    }
}

/// Shows a balloon (a toast on Windows 10/11) from the tray icon.
///
/// `tray-icon` has no notification API, so this modifies its icon directly.
/// That needs the icon's Win32 identity: the hidden window `tray-icon`
/// creates on this thread (class `tray_icon_app`) and the per-process
/// counter it uses as `uID`, which is 1 for the first icon when the id is
/// given explicitly (`TrayIcon::with_id`). Both are internals of the pinned
/// `tray-icon` version; a failure here is only logged.
fn balloon(level: Level, title: &str, text: &str) {
    use windows_sys::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_INFO, NIIF_INFO, NIIF_RESPECT_QUIET_TIME, NIIF_WARNING, NIM_MODIFY,
        NOTIFYICONDATAW,
    };
    const TRAY_UID: u32 = 1;
    let Some(hwnd) = tray_window() else {
        tracing::warn!("cannot show a notification: tray window not found");
        return;
    };
    // SAFETY: NOTIFYICONDATAW is plain data; all-zero is a valid value.
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = TRAY_UID;
    nid.uFlags = NIF_INFO;
    let icon = match level {
        Level::Info => NIIF_INFO,
        Level::Warning => NIIF_WARNING,
    };
    // Quiet time: nothing pops up during the first hour after install or
    // while the user has notifications silenced.
    nid.dwInfoFlags = icon | NIIF_RESPECT_QUIET_TIME;
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, text);
    // SAFETY: `nid` is fully initialized and lives across the call.
    if unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) } == 0 {
        tracing::warn!(
            "cannot show a notification: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Copies `s` into a fixed UTF-16 buffer, cut to fit with a terminator.
fn copy_wide(buf: &mut [u16], s: &str) {
    let units: Vec<u16> = s.encode_utf16().take(buf.len() - 1).collect();
    buf[..units.len()].copy_from_slice(&units);
    buf[units.len()] = 0;
}

/// The hidden window `tray-icon` created on this thread.
fn tray_window() -> Option<windows_sys::Win32::Foundation::HWND> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;

    unsafe extern "system" fn find(hwnd: HWND, found: LPARAM) -> windows_sys::core::BOOL {
        let mut class = [0u16; 32];
        // SAFETY: the buffer length is passed along.
        let len = unsafe { wm::GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
        if String::from_utf16_lossy(&class[..len.max(0) as usize]) == "tray_icon_app" {
            // SAFETY: `found` points at the `Option<HWND>` below.
            unsafe { *(found as *mut Option<HWND>) = Some(hwnd) };
            return 0; // stop enumerating
        }
        1
    }

    let mut found: Option<HWND> = None;
    // SAFETY: `find` only writes through the pointer while `found` is alive.
    unsafe {
        wm::EnumThreadWindows(
            GetCurrentThreadId(),
            Some(find),
            &mut found as *mut Option<HWND> as LPARAM,
        );
    }
    found
}
