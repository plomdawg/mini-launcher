//! The Windows tray: a notification-area icon with a menu on the UI thread,
//! and the shared run loop on a worker thread.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use mini_launcher_cli::{logging, port};
use mini_launcher_core::autostart::{self, Status};
use mini_launcher_core::battlenet::{self, BattleNetGame};
use mini_launcher_core::config::edit::saved_by_this_process;
use mini_launcher_core::config::location::{ConfigLocation, POINTER_FILE};
use mini_launcher_core::config::{self, Config};
use mini_launcher_core::launch::{self, LaunchPlan};
use mini_launcher_core::reconnect::{Backoff, ReaderState, Reconnector, SystemClock};
use mini_launcher_core::runner::{self, Notice, Runner};
use mini_launcher_core::session::{self, SessionServer, Sessions};
use mini_launcher_core::steam::{self, SteamGame};
use mini_launcher_core::{instance, logs};
use mini_launcher_ui::settings::{Host, ReaderStatus};
use mini_launcher_ui::window::{Event as WindowEvent, SettingsWindow};
use mini_launcher_ui::wizard::Link;
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconAttributes};
use windows_sys::Win32::UI::WindowsAndMessaging as wm;

use crate::model::{self, Level, RateLimiter, APP};

/// At most one balloon per this long, so a bad tag left on the reader
/// cannot flood the notification center.
const NOTIFY_GAP: Duration = Duration::from_secs(10);
/// How often the worker checks whether `config.toml` changed (in polls).
const CONFIG_CHECK_POLLS: u32 = 10;
/// How often an unavailable config folder is checked again (it is also
/// checked when `location.toml` changes).
const CONFIG_RETRY: Duration = Duration::from_secs(30);
/// How long the UI thread waits for window messages between other checks.
const UI_WAIT_MS: u32 = 50;
/// How long `--quit` waits for the running app to exit.
const QUIT_WAIT: Duration = Duration::from_secs(10);
/// Placeholder port name until auto-detection finds the reader.
const AUTO_PORT: &str = "auto-detected port";

/// From the worker to the UI thread.
enum Update {
    Reader(ReaderState),
    Notice(Notice),
    ConfigError(String),
    /// The config location now in use.
    Location(ConfigLocation),
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
    let settings = match model::mode(std::env::args_os().skip(1)) {
        model::Mode::Quit => return quit_running(),
        model::Mode::Run { settings } => settings,
    };
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
    match run(log_dir, settings) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            message_box(&format!("Mini Launcher stopped: {e:#}"), true);
            ExitCode::FAILURE
        }
    }
}

fn run(log_dir: Option<PathBuf>, mut reopen: usize) -> Result<()> {
    let mut config = model::ConfigStatus::default();
    let exe = std::env::current_exe().context("cannot find this program's path")?;
    let stop = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    // Set when "Open config" finds the folder gone: the worker checks again.
    let recheck = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    // The settings window's wizard borrows the reader through these.
    let (sessions, server) = session::channel();
    let host = Arc::new(TrayHost {
        state: Mutex::default(),
        reload: AtomicBool::new(false),
        relocate: AtomicBool::new(false),
        sessions,
    });
    // Setup signals this to close the app before it upgrades it.
    let quit_listener = instance::QuitListener::create()
        .inspect_err(|e| tracing::warn!("cannot listen for quit requests from setup: {e}"))
        .ok();
    let mut quit_requests = model::QuitRequests::default();
    let worker = thread::Builder::new()
        .name("run-loop".into())
        .spawn({
            let (stop, paused, recheck) = (stop.clone(), paused.clone(), recheck.clone());
            let host = host.clone();
            move || work(&stop, &paused, &recheck, &host, server, &tx)
        })
        .context("cannot start the run loop")?;

    let settings = MenuItem::new("Settings…", true, None);
    let pause = MenuItem::new("", true, None);
    let open_config = MenuItem::new("Open config", Config::default_dir().is_some(), None);
    let open_logs = MenuItem::new("Open logs folder", log_dir.is_some(), None);
    let start_at_login = CheckMenuItem::new("Start at login", true, starts_at_login(&exe), None);
    let quit = MenuItem::new("Quit", true, None);
    let separator = PredefinedMenuItem::separator();
    let items: [&dyn IsMenuItem; 7] = [
        &settings,
        &pause,
        &open_config,
        &open_logs,
        &start_at_login,
        &separator,
        &quit,
    ];
    let mut shown_status = model::view(None, false, None).status;
    let menu = tray_menu(&shown_status, &items).context("cannot build the tray menu")?;
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

    // The settings window runs on its own thread, so this loop keeps
    // handling the menu and the worker while it is open.
    let (window_tx, window_rx) = mpsc::channel();
    let window = SettingsWindow::spawn(host.clone(), window_tx)
        .context("cannot start the settings window")?;
    let mut reader: Option<ReaderState> = None;
    let mut limiter = RateLimiter::new(NOTIFY_GAP);
    let mut refresh = |reader: Option<&ReaderState>, config_problem: Option<&str>| {
        let is_paused = paused.load(Ordering::Relaxed);
        let (text, ready) = model::reader_cell(reader, is_paused);
        lock(&host.state).1 = ReaderStatus { text, ready };
        let view = model::view(reader, is_paused, config_problem);
        if let Err(e) = tray.set_tooltip(Some(&view.tooltip)) {
            tracing::warn!("cannot set the tray tooltip: {e}");
        }
        pause.set_text(view.pause_label);
        // A menu can stay as wide as the widest text its status line ever
        // had, so a changed status gets a fresh menu (see `tray_menu`).
        if shown_status != view.status {
            match tray_menu(&view.status, &items) {
                Ok(menu) => {
                    tray.set_menu(Some(Box::new(menu)));
                    shown_status = view.status;
                }
                Err(e) => tracing::warn!("cannot rebuild the tray menu: {e}"),
            }
        }
        window.repaint();
    };
    refresh(None, None);

    // Each `--settings` opens the window once more after the previous one
    // closes (CI repeats it to check re-opening on the kept event loop).
    if reopen > 0 {
        reopen -= 1;
        window.show();
    }

    'ui: loop {
        if !pump_messages() {
            break;
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let id = event.id();
            if id == quit.id() {
                break 'ui;
            } else if id == settings.id() {
                // Opens it, or brings the open one to the front.
                window.show();
            } else if id == pause.id() {
                let now_paused = !paused.load(Ordering::Relaxed);
                paused.store(now_paused, Ordering::Relaxed);
                tracing::info!(paused = now_paused, "pause toggled from the tray");
                refresh(reader.as_ref(), config.problem());
            } else if id == open_config.id() {
                let Some(location) = config.to_open() else {
                    if let Some(problem) = config.problem() {
                        let (level, title, text) = model::unavailable_balloon(problem);
                        balloon(level, title, &text);
                    }
                    continue;
                };
                match edit_config(location) {
                    // The folder went away before the worker noticed: show
                    // it now and have the worker check (its answer wins).
                    Ok(Some(problem)) => {
                        let (level, title, text) = config.open_config_unavailable(problem);
                        balloon(level, title, &text);
                        recheck.store(true, Ordering::Relaxed);
                        refresh(reader.as_ref(), config.problem());
                    }
                    Ok(None) => {
                        if config.open_config_ready() {
                            refresh(reader.as_ref(), config.problem());
                        }
                    }
                    Err(e) => {
                        tracing::warn!("cannot open the config: {e:#}");
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
        if quit_listener.as_ref().is_some_and(|q| q.requested()) {
            match quit_requests.handle(window.is_open(), Instant::now()) {
                model::QuitAction::Quit => {
                    tracing::info!("quit requested by setup or --quit");
                    break 'ui;
                }
                model::QuitAction::CloseSettingsFirst => {
                    // It may hold unsaved edits: never close it from outside.
                    tracing::info!("quit requested while Settings is open; not quitting");
                    window.show();
                    balloon(
                        Level::Info,
                        model::CLOSE_SETTINGS_TITLE,
                        model::CLOSE_SETTINGS_TEXT,
                    );
                }
                model::QuitAction::Ignore => {}
            }
        }
        while let Ok(event) = window_rx.try_recv() {
            let closed = matches!(event, WindowEvent::Closed(_));
            match event {
                WindowEvent::Opened => tracing::info!("settings window opened"),
                WindowEvent::Closed(Ok(())) => tracing::info!("settings window closed"),
                WindowEvent::Closed(Err(e)) => {
                    tracing::error!("settings window failed: {e}");
                    balloon(Level::Warning, "Cannot open the settings", &e);
                }
            }
            if closed && reopen > 0 {
                reopen -= 1;
                window.show();
            }
        }
        loop {
            match rx.try_recv() {
                Ok(Update::Reader(state)) => {
                    // The menu only shows the first line of it.
                    if let ReaderState::Reconnecting {
                        last_error: Some(e),
                        ..
                    } = &state
                    {
                        if model::reader_error(reader.as_ref()) != Some(e) {
                            tracing::info!("reader not found: {e}");
                        }
                    }
                    reader = Some(state);
                    refresh(reader.as_ref(), config.problem());
                }
                Ok(Update::Notice(notice)) => {
                    let problem = config.problem();
                    if let Some((level, title, text)) = model::notification(&notice, problem) {
                        if limiter.allow(Instant::now()) {
                            balloon(level, title, &text);
                        }
                    }
                }
                Ok(Update::ConfigError(e)) => balloon(Level::Warning, "Config not loaded", &e),
                Ok(Update::Location(location)) => {
                    lock(&host.state).0 = Some(location.clone());
                    if let Some((level, title, text)) = config.set_location(location) {
                        balloon(level, title, &text);
                    }
                    refresh(reader.as_ref(), config.problem());
                }
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
    if window.is_open() {
        tracing::info!("closing the settings window");
    }
    if !window.shut_down(Duration::from_secs(3)) {
        tracing::warn!("the settings window did not close in time");
    }
    drop(tray);
    // The worker notices `stop` within one poll (or one reopen attempt).
    let _ = worker.join();
    Ok(())
}

/// `--quit`: asks the running tray app to quit and waits for it to exit.
/// Fails if it is still running (Settings open, or a `mini-launcher run` or
/// an older version that does not listen).
fn quit_running() -> ExitCode {
    match instance::request_quit() {
        Ok(true) => {
            tracing::info!("asked the running tray app to quit");
            let start = Instant::now();
            while instance::is_running() && start.elapsed() < QUIT_WAIT {
                thread::sleep(Duration::from_millis(100));
            }
        }
        Ok(false) => {}
        Err(e) => tracing::warn!("cannot ask the running tray app to quit: {e}"),
    }
    if instance::is_running() {
        tracing::info!("--quit: Mini Launcher is still running");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// The tray menu: the (disabled) `status` line, a separator, then `items`.
///
/// Win32 caches a popup menu's width, and a shorter status set with
/// `set_text` may leave it as wide as the longest text it ever had (#106).
/// A fresh menu has nothing cached. The other items are reused, so their
/// ids (and the event handling) stay the same; dropping the old menu (in
/// `TrayIcon::set_menu`) detaches them from it.
fn tray_menu(status: &str, items: &[&dyn IsMenuItem]) -> muda::Result<Menu> {
    let menu = Menu::new();
    menu.append(&MenuItem::new(status, false, None))?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append_items(items)?;
    Ok(menu)
}

/// What the settings window reads from the tray, and its way back.
struct TrayHost {
    /// The config location in use, and the reader cell.
    state: Mutex<(Option<ConfigLocation>, ReaderStatus)>,
    /// Set when the window saved; the worker reloads the library.
    reload: AtomicBool,
    /// Tag sessions with the worker, which owns the reader.
    sessions: Sessions,
    /// Set when the window changed `location.toml`; the worker switches.
    relocate: AtomicBool,
}

impl Host for TrayHost {
    fn location(&self) -> Option<ConfigLocation> {
        lock(&self.state).0.clone()
    }

    fn reader(&self) -> ReaderStatus {
        lock(&self.state).1.clone()
    }

    fn saved(&self) {
        self.reload.store(true, Ordering::Relaxed);
    }

    fn tag_session(&self) -> Option<Box<dyn Link>> {
        Some(Box::new(self.sessions.open()))
    }

    fn steam_games(&self) -> Result<Vec<SteamGame>, String> {
        steam::installed_games().map_err(|e| e.to_string())
    }

    fn battlenet_games(&self) -> Vec<BattleNetGame> {
        battlenet::installed_games()
    }

    fn launch(&self, plan: &LaunchPlan) -> Result<(), String> {
        tracing::info!("launch from the settings window: {plan}");
        launch::execute(plan, false).map(drop).map_err(|e| {
            tracing::warn!("launch from the settings window failed: {e}");
            e.to_string()
        })
    }

    fn moved(&self) {
        // The window shows the new folder at once; the worker follows on
        // its next poll (the same switch as when the folder comes back).
        // Resolving runs on the window thread and can briefly stall it
        // while a network share times out (see docs/config.md).
        lock(&self.state).0 = Config::location();
        self.relocate.store(true, Ordering::Relaxed);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The worker: the shared run loop, with the reader opened (and reopened)
/// in the background so a missing reader never stops the app.
fn work(
    stop: &AtomicBool,
    paused: &AtomicBool,
    recheck: &AtomicBool,
    host: &TrayHost,
    mut sessions: SessionServer,
    tx: &mpsc::Sender<Update>,
) {
    let mut runner = Runner::new(Config::default(), false);
    let mut watch = ConfigWatch::default();
    if watch.switch(Config::location(), &mut runner, tx).is_none() {
        return;
    }
    let mut reader = Reconnector::unopened(
        AUTO_PORT.to_owned(),
        || port::reopen_poller(None),
        Backoff::default(),
    );
    let mut last_state = None;
    let mut polls = 0u32;
    let next = |runner: &mut Runner, now| {
        // Paused: the reader idles with its RF field off (no polling),
        // unless the settings window's wizard has a tag session (then it
        // polls and nothing launches until the session ends).
        let is_paused = paused.load(Ordering::Relaxed);
        polls = polls.wrapping_add(1);
        // The settings window moved the config folder: follow it now.
        if host.relocate.swap(false, Ordering::Relaxed) {
            watch.switch(Config::location(), runner, tx)?;
        }
        // A save from the settings window is loaded at once (`force` makes
        // the check reread the file even if its mtime looks the same).
        let saved = host.reload.swap(false, Ordering::Relaxed);
        if saved {
            watch.force = true;
        }
        if saved || polls.is_multiple_of(CONFIG_CHECK_POLLS) {
            watch.check(runner, recheck.swap(false, Ordering::Relaxed), tx)?;
        }
        let tick = sessions.tick(&mut reader, runner, is_paused, now);
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

/// Follows the config: reloads `config.toml` when it changes. While the
/// config folder is unavailable the library is empty (never the default
/// folder's), and a helper thread checks the folder again (see
/// [`retry_location`]) so a slow share never stalls tag polling. Methods
/// return `None` once the UI is gone.
#[derive(Default)]
struct ConfigWatch {
    location: Option<ConfigLocation>,
    /// `config.toml`'s mtime at the last check; `None` before the first.
    stamp: Option<Option<SystemTime>>,
    /// The text behind the running library, to skip no-op reloads.
    loaded: Option<String>,
    /// While unavailable: the next location found by the helper thread.
    retry: Option<mpsc::Receiver<Option<ConfigLocation>>>,
    /// Backoff for restarting that thread if it dies; kept across switches.
    respawn: model::Respawn,
    /// Reread the file at the next check even if its mtime is the same.
    force: bool,
}

impl ConfigWatch {
    /// Starts following `location` with an empty library.
    fn switch(
        &mut self,
        location: Option<ConfigLocation>,
        runner: &mut Runner,
        tx: &mpsc::Sender<Update>,
    ) -> Option<()> {
        *self = ConfigWatch {
            respawn: std::mem::take(&mut self.respawn),
            ..ConfigWatch::default()
        };
        runner.set_config(Config::default());
        let Some(location) = location else {
            tracing::warn!("no config directory on this system; only steam:// tags will work");
            return Some(());
        };
        self.location = Some(location.clone());
        tx.send(Update::Location(location.clone())).ok()?;
        match location.config_file() {
            Ok(path) => self.check_file(&path, runner, tx)?,
            Err(e) => {
                tracing::warn!("{e}; only steam:// tags will work until it is available");
                let (retry_tx, retry_rx) = mpsc::channel();
                let spawned = thread::Builder::new()
                    .name("config-retry".into())
                    .spawn(move || retry_location(Some(location), &retry_tx));
                match spawned {
                    Ok(_) => self.retry = Some(retry_rx),
                    Err(e) => tracing::warn!("cannot start the config retry thread: {e}"),
                }
            }
        }
        Some(())
    }

    /// `recheck`: the UI saw the folder gone; always tell it the outcome.
    fn check(
        &mut self,
        runner: &mut Runner,
        recheck: bool,
        tx: &mpsc::Sender<Update>,
    ) -> Option<()> {
        if let Some(retry) = &self.retry {
            // Unavailable here too: the UI already has that location.
            return match model::Retry::from(retry.try_recv()) {
                model::Retry::Wait => Some(()),
                model::Retry::Switch(location) => {
                    self.respawn = model::Respawn::default();
                    if location.as_ref().is_some_and(ConfigLocation::is_available) {
                        tracing::info!("config folder available again");
                    }
                    self.switch(location, runner, tx)
                }
                model::Retry::Respawn => match self.respawn.step(Instant::now()) {
                    model::RespawnStep::Scheduled(delay) => {
                        tracing::error!(
                            "the config retry thread ended without a result; \
                             restarting it in {} s",
                            delay.as_secs()
                        );
                        Some(())
                    }
                    model::RespawnStep::Wait => Some(()),
                    model::RespawnStep::Now => self.switch(self.location.clone(), runner, tx),
                },
            };
        }
        let (Some(location), Some(dir)) = (&self.location, Config::default_dir()) else {
            return Some(());
        };
        // Checked on the folder itself, so it also works before a
        // config.toml was ever created in it.
        if let Some(gone) = model::vanished(location, &dir) {
            return self.switch(Some(gone), runner, tx);
        }
        if recheck {
            tx.send(Update::Location(location.clone())).ok()?;
        }
        match location.config_file() {
            Ok(path) => self.check_file(&path, runner, tx),
            Err(_) => Some(()),
        }
    }

    /// Reloads `path` if it changed since the last check.
    fn check_file(
        &mut self,
        path: &Path,
        runner: &mut Runner,
        tx: &mpsc::Sender<Update>,
    ) -> Option<()> {
        let current = modified(path);
        if !model::config_changed(self.stamp, current, std::mem::take(&mut self.force)) {
            return Some(());
        }
        let first = self.stamp.is_none();
        self.stamp = Some(current);
        let text = config::read_text_or_empty(path);
        // Touched but unchanged (e.g. re-saved as is): nothing to do.
        if text.as_ref().ok() == self.loaded.as_ref() {
            return Some(());
        }
        match text.and_then(|t| Config::parse(&t).map(|c| (c, t))) {
            Ok((config, text)) => {
                // This app's own saves are expected; log outside edits.
                if !first && !saved_by_this_process(&text) {
                    tracing::info!("config reloaded ({} games)", config.games.len());
                }
                runner.set_config(config);
                self.loaded = Some(text);
            }
            Err(e) => {
                let e = config_error(Some(path), &e);
                tracing::warn!("{e}; keeping the previous library");
                tx.send(Update::ConfigError(e)).ok()?;
            }
        }
        Some(())
    }
}

/// Resolves the config location again every [`CONFIG_RETRY`], or sooner
/// when `location.toml` changes, until it differs from `current`; sends it
/// and ends.
fn retry_location(current: Option<ConfigLocation>, tx: &mpsc::Sender<Option<ConfigLocation>>) {
    let pointer = Config::default_dir().map(|d| d.join(POINTER_FILE));
    let mut stamp = pointer.as_deref().and_then(modified);
    let mut last = Instant::now();
    loop {
        thread::sleep(Duration::from_secs(1));
        let now = pointer.as_deref().and_then(modified);
        if now == stamp && last.elapsed() < CONFIG_RETRY {
            continue;
        }
        (stamp, last) = (now, Instant::now());
        let location = Config::location();
        if location != current {
            let _ = tx.send(location);
            return;
        }
    }
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
/// associated program. Returns why the config folder is unavailable, if it
/// is (nothing is created then).
fn edit_config(location: &ConfigLocation) -> Result<Option<String>> {
    let path = match model::ensure_config(location).context("cannot create the config file")? {
        model::OpenConfig::Unavailable(problem) => return Ok(Some(problem)),
        model::OpenConfig::Ready { path, created } => {
            if created {
                tracing::info!("created {}", path.display());
            }
            path
        }
    };
    let path = path.as_path();
    if let Err(e) = open::that_detached(path) {
        tracing::info!("no program for .toml files ({e}); using Notepad");
        std::process::Command::new("notepad.exe")
            .arg(path)
            .spawn()
            .context("cannot start Notepad")?;
    }
    Ok(None)
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
