//! What the tray shows, as pure functions of the reader state and the run
//! loop's notices, so it is tested on every OS.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_core::reconnect::ReaderState;
use mini_launcher_core::runner::Notice;

pub const APP: &str = "Mini Launcher";

/// Windows keeps 128 UTF-16 units of tooltip, including the terminator.
const TOOLTIP_MAX: usize = 127;

/// Longest menu status line, in characters: the menu is as wide as its
/// widest item, so the full error goes to the tooltip and the log instead.
pub const STATUS_MAX: usize = 48;

/// The tooltip and menu labels for one state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub tooltip: String,
    /// The (disabled) status line at the top of the menu: one short line,
    /// at most [`STATUS_MAX`] characters, `&` escaped for the menu.
    pub status: String,
    pub pause_label: &'static str,
}

/// `reader` is `None` until the first open attempt has run; `config_problem`
/// is why the config folder is unavailable, if it is.
pub fn view(reader: Option<&ReaderState>, paused: bool, config_problem: Option<&str>) -> View {
    let (reader, short) = match reader {
        None => ("starting…".to_owned(), None),
        Some(ReaderState::Connected { port }) => (format!("ready ({port})"), None),
        Some(ReaderState::Reconnecting { last_error, .. }) => match last_error {
            Some(e) => (
                format!("reader not found, retrying… ({e})"),
                Some(format!("reader not found: {}", shorten(e, STATUS_MAX))),
            ),
            None => ("reader not found, retrying…".to_owned(), None),
        },
    };
    let short = short.unwrap_or_else(|| reader.clone());
    let (mut tooltip, status) = if paused {
        ("paused".to_owned(), format!("Paused — {short}"))
    } else {
        (reader, capitalize(&short))
    };
    // The config problem always fits: only the reader part is cut.
    let suffix = if config_problem.is_some() {
        CONFIG_SUFFIX
    } else {
        ""
    };
    let status = shorten(&status, STATUS_MAX - suffix.chars().count()) + suffix;
    if let Some(problem) = config_problem {
        // First, so a long reader error cannot cut it off.
        tooltip = format!("{problem}; {tooltip}");
    }
    View {
        tooltip: truncate_utf16(&format!("{APP} — {tooltip}"), TOOLTIP_MAX),
        status: status.replace('&', "&&"),
        pause_label: if paused { "Resume" } else { "Pause" },
    }
}

/// Ends the status line while the config folder is unavailable (the
/// tooltip and the balloon say why).
const CONFIG_SUFFIX: &str = "; config unavailable";

/// The last error opening the reader, if it is not connected.
pub fn reader_error(reader: Option<&ReaderState>) -> Option<&String> {
    match reader {
        Some(ReaderState::Reconnecting { last_error, .. }) => last_error.as_ref(),
        _ => None,
    }
}

/// `s` as one short line: its first non-blank line, whitespace collapsed,
/// cut to at most `max` characters (never inside one), ending in `…` when
/// cut.
pub fn shorten(s: &str, max: usize) -> String {
    let line = s
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut out: String = line.chars().take(max.saturating_sub(1)).collect();
    out.truncate(out.trim_end().len());
    out.push('…');
    out
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Cuts `s` to at most `max` UTF-16 units, ending in `…` when cut.
fn truncate_utf16(s: &str, max: usize) -> String {
    if s.encode_utf16().count() <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        // Leave room for the ellipsis (one unit).
        if units + c.len_utf16() > max - 1 {
            break;
        }
        units += c.len_utf16();
        out.push(c);
    }
    out.push('…');
    out
}

/// The notification title while the config folder is unavailable.
pub const CONFIG_UNAVAILABLE: &str = "Config folder unavailable";

/// How a notification is shown: an info or a warning icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
}

/// The balloon (level, title, text) for notices the user should see;
/// everything else only goes to the log. A failed launch is a warning; an
/// unknown cart is the user's to map, so it is info. While the config folder
/// is unavailable (`config_problem`), a cart that needs the config says so.
pub fn notification(
    notice: &Notice,
    config_problem: Option<&str>,
) -> Option<(Level, &'static str, String)> {
    match notice {
        Notice::UnknownId { uid, .. } | Notice::Unmapped { uid } if config_problem.is_some() => {
            Some((
                Level::Warning,
                CONFIG_UNAVAILABLE,
                format!(
                    "Tag {uid} needs config.toml, but {}.",
                    config_problem.unwrap_or_default()
                ),
            ))
        }
        Notice::LaunchFailed { plan, error, .. } => Some((
            Level::Warning,
            "Could not start the game",
            format!("{plan}: {error}"),
        )),
        Notice::UnknownId { uid, slug } => Some((
            Level::Info,
            "Unknown cart",
            format!("Tag {uid} says id:{slug}, but config.toml has no game with that id."),
        )),
        Notice::Unmapped { uid } => Some((
            Level::Info,
            "Unknown cart",
            format!("Tag {uid} has no game on it and is not mapped in config.toml."),
        )),
        _ => None,
    }
}

/// Lets at most one notification through per `gap`.
#[derive(Debug)]
pub struct RateLimiter {
    gap: Duration,
    last: Option<Instant>,
}

impl RateLimiter {
    pub fn new(gap: Duration) -> RateLimiter {
        RateLimiter { gap, last: None }
    }

    pub fn allow(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < self.gap)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Mode {
    /// Run the tray app; open the settings window this many times
    /// (`--settings`, repeatable: each opens it again after it closes).
    Run { settings: usize },
    /// `--quit`: ask the running tray app to quit, then exit.
    Quit,
}

pub fn mode<A: AsRef<std::ffi::OsStr>>(args: impl IntoIterator<Item = A>) -> Mode {
    let mut settings = 0;
    for arg in args {
        match arg.as_ref().to_str() {
            Some("--quit") => return Mode::Quit,
            Some("--settings") => settings += 1,
            _ => {}
        }
    }
    Mode::Run { settings }
}

/// What a quit request from outside (setup, `--quit`) does.
#[derive(Debug, PartialEq, Eq)]
pub enum QuitAction {
    /// Same as the Quit menu item.
    Quit,
    /// The settings window is open and may hold unsaved edits: keep
    /// running, bring it to the front and say why.
    CloseSettingsFirst,
    /// Already asked recently (setup repeats its request while it waits).
    Ignore,
}

pub const CLOSE_SETTINGS_TITLE: &str = "Update waiting";
pub const CLOSE_SETTINGS_TEXT: &str = "Close Settings to finish updating Mini Launcher.";

/// Decides [`QuitAction`]s; asks at most once per gap to close Settings.
#[derive(Debug)]
pub struct QuitRequests(RateLimiter);

impl Default for QuitRequests {
    fn default() -> Self {
        Self(RateLimiter::new(Self::GAP))
    }
}

impl QuitRequests {
    pub const GAP: Duration = Duration::from_secs(30);

    pub fn handle(&mut self, settings_open: bool, now: Instant) -> QuitAction {
        if !settings_open {
            QuitAction::Quit
        } else if self.0.allow(now) {
            QuitAction::CloseSettingsFirst
        } else {
            QuitAction::Ignore
        }
    }
}

/// Written by "Open config" when there is no config file yet.
pub const CONFIG_TEMPLATE: &str = r#"# Mini Launcher library. Changes are picked up automatically.
#
# Tags that say steam://<appid> work without any entry here. Add an entry to
# give a tag that says id:<id> a game, or to map a tag by its UID (shown in
# the log when an unknown tag is inserted).
#
# [[games]]
# id = "half-life-2"              # lowercase letters, digits and dashes
# name = "Half-Life 2"
# uid = "04:A1:B2:C3:D4:E5:F6"    # optional: this tag always launches this game
# steam_appid = 220
#
# [[games]]
# id = "wow-forever"
# name = "World of Warcraft: Forever"
# battlenet = "WoWF"              # Battle.net launch code
#
# [[games]]
# id = "my-game"
# name = "My Game"
# exe = 'C:\Games\MyGame\game.exe'
# args = ["--fullscreen"]         # optional
# cwd = 'C:\Games\MyGame'         # optional
"#;

/// What "Open config" found.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenConfig {
    /// The file to open; `created` if it was just made from the template.
    Ready { path: PathBuf, created: bool },
    /// The config folder is unavailable (why); nothing was created.
    Unavailable(String),
}

/// Creates the config file from [`CONFIG_TEMPLATE`] unless it exists. Only
/// the default folder is created if missing: an override folder that went
/// away is reported, never recreated (that would split the library in two).
pub fn ensure_config(location: &ConfigLocation) -> io::Result<OpenConfig> {
    let path = match location.writable_file() {
        Ok(path) => path,
        Err(e) => return Ok(OpenConfig::Unavailable(e.to_string())),
    };
    if location.may_create_dir() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let created = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            io::Write::write_all(&mut file, CONFIG_TEMPLATE.as_bytes())?;
            true
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e),
    };
    Ok(OpenConfig::Ready { path, created })
}

/// For an available config folder: the location to switch to if the folder
/// itself went away (whether or not `config.toml` was ever there), resolved
/// again from `default_dir`.
pub fn vanished(location: &ConfigLocation, default_dir: &Path) -> Option<ConfigLocation> {
    if !location.is_available() || location.writable_file().is_ok() {
        return None;
    }
    Some(ConfigLocation::resolve(default_dir)).filter(|l| !l.is_available())
}

/// The tray's view of the config folder: the worker's location, plus a
/// problem "Open config" found before the worker noticed. That one is only
/// provisional: the worker is asked to check again (see
/// [`ConfigStatus::open_config_unavailable`]) and its answer replaces it, so
/// the state always converges, whether or not the folder came back.
#[derive(Debug, Default)]
pub struct ConfigStatus {
    location: Option<ConfigLocation>,
    problem: Option<String>,
}

impl ConfigStatus {
    /// Why the config folder is unavailable, if it is.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The location "Open config" should try: the worker's, if it is
    /// available, even when "Open config" itself found it gone before (so a
    /// folder that came back opens at once). `None`: just warn.
    pub fn to_open(&self) -> Option<&ConfigLocation> {
        self.location.as_ref().filter(|l| l.is_available())
    }

    /// The worker resolved `location`: the balloon to show, if the state
    /// changed.
    pub fn set_location(
        &mut self,
        location: ConfigLocation,
    ) -> Option<(Level, &'static str, String)> {
        let problem = location.config_file().err().map(|e| e.to_string());
        let balloon = match (&problem, &self.problem) {
            (None, Some(_)) => {
                let path = location.config_file().unwrap_or_default();
                let text = format!("Loaded {}", path.display());
                Some((Level::Info, "Config folder available", text))
            }
            (Some(p), old) if old.as_ref() != Some(p) => Some(unavailable_balloon(p)),
            _ => None,
        };
        self.location = Some(location);
        self.problem = problem;
        balloon
    }

    /// "Open config" found the folder gone (`problem`) before the worker
    /// did. Returns the balloon; the caller asks the worker to re-check.
    pub fn open_config_unavailable(&mut self, problem: String) -> (Level, &'static str, String) {
        let balloon = unavailable_balloon(&problem);
        self.problem = Some(problem);
        balloon
    }

    /// "Open config" worked: whatever it found missing before is back.
    /// Returns whether the state changed.
    pub fn open_config_ready(&mut self) -> bool {
        self.to_open().is_some() && self.problem.take().is_some()
    }
}

/// The warning for an unavailable config folder, the same from wherever it
/// was found.
pub fn unavailable_balloon(problem: &str) -> (Level, &'static str, String) {
    let text = format!("{problem}. Only steam:// tags work until it is.");
    (Level::Warning, CONFIG_UNAVAILABLE, text)
}

/// What the worker does with its config retry thread's channel.
#[derive(Debug, PartialEq, Eq)]
pub enum Retry {
    /// Nothing found yet.
    Wait,
    /// The location changed: switch to it.
    Switch(Option<ConfigLocation>),
    /// The thread ended without a result (it panicked): start it again.
    Respawn,
}

impl From<Result<Option<ConfigLocation>, std::sync::mpsc::TryRecvError>> for Retry {
    fn from(received: Result<Option<ConfigLocation>, std::sync::mpsc::TryRecvError>) -> Retry {
        match received {
            Ok(location) => Retry::Switch(location),
            Err(std::sync::mpsc::TryRecvError::Empty) => Retry::Wait,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Retry::Respawn,
        }
    }
}

/// Backoff for restarting a config retry thread that died (it panicked),
/// so one that dies at once every time can't flood the log: 1 s, doubling
/// up to [`Respawn::MAX`].
#[derive(Debug, Default)]
pub struct Respawn {
    failures: u32,
    due: Option<Instant>,
}

/// What to do about a dead retry thread now.
#[derive(Debug, PartialEq, Eq)]
pub enum RespawnStep {
    /// Just noticed: restart it after this delay (log it once).
    Scheduled(Duration),
    /// Not yet.
    Wait,
    /// Restart it now.
    Now,
}

impl Respawn {
    pub const MAX: Duration = Duration::from_secs(60);

    /// The thread is dead at `now`.
    pub fn step(&mut self, now: Instant) -> RespawnStep {
        match self.due {
            None => {
                self.failures = self.failures.saturating_add(1);
                let delay = Duration::from_secs(1u64 << (self.failures - 1).min(6)).min(Self::MAX);
                self.due = Some(now + delay);
                RespawnStep::Scheduled(delay)
            }
            Some(due) if now < due => RespawnStep::Wait,
            Some(_) => {
                self.due = None;
                RespawnStep::Now
            }
        }
    }
}

/// Whether the config file must be read again: its mtime changed since the
/// last check (`None` before the first), or a reload was asked for.
pub fn config_changed(
    last: Option<Option<SystemTime>>,
    current: Option<SystemTime>,
    force: bool,
) -> bool {
    force || last != Some(current)
}

/// The settings window's status-bar reader cell: its text, and whether
/// the reader is ready (lights the mark).
pub fn reader_cell(reader: Option<&ReaderState>, paused: bool) -> (String, bool) {
    match (reader, paused) {
        (_, true) => ("Reader: paused".to_owned(), false),
        (None, _) => ("Reader: starting".to_owned(), false),
        (Some(ReaderState::Connected { port }), _) => (format!("Reader {port}: ready"), true),
        (Some(ReaderState::Reconnecting { .. }), _) => ("Reader: not found".to_owned(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_launcher_core::config::Config;
    use mini_launcher_core::launch::{LaunchPlan, Via};
    use mini_launcher_core::reader::Uid;

    fn connected() -> ReaderState {
        ReaderState::Connected {
            port: "COM7".into(),
        }
    }

    fn missing(error: Option<&str>) -> ReaderState {
        ReaderState::Reconnecting {
            last_port: "COM7".into(),
            attempts: 3,
            last_error: error.map(str::to_owned),
        }
    }

    #[test]
    fn tooltip_and_status_follow_the_reader() {
        let v = view(Some(&connected()), false, None);
        assert_eq!(v.tooltip, "Mini Launcher — ready (COM7)");
        assert_eq!(v.status, "Ready (COM7)");
        assert_eq!(v.pause_label, "Pause");

        let v = view(Some(&missing(Some("no CH340 adapter found"))), false, None);
        assert_eq!(
            v.tooltip,
            "Mini Launcher — reader not found, retrying… (no CH340 adapter found)"
        );
        assert_eq!(v.status, "Reader not found: no CH340 adapter found");
        let v = view(Some(&missing(None)), false, None);
        assert_eq!(v.tooltip, "Mini Launcher — reader not found, retrying…");

        assert_eq!(view(None, false, None).status, "Starting…");
    }

    #[test]
    fn paused_wins_the_tooltip_and_flips_the_menu_label() {
        let v = view(Some(&connected()), true, None);
        assert_eq!(v.tooltip, "Mini Launcher — paused");
        assert_eq!(v.status, "Paused — ready (COM7)");
        assert_eq!(v.pause_label, "Resume");
        let v = view(Some(&missing(None)), true, None);
        assert_eq!(v.tooltip, "Mini Launcher — paused");
        assert_eq!(v.status, "Paused — reader not found, retrying…");
    }

    /// Issue #106: the menu is as wide as its widest item.
    #[test]
    fn long_reader_errors_keep_the_status_line_short() {
        let error = "no USB serial ports found\nis the reader plugged in and its USB-serial \
                     driver installed? If your adapter is not a CH340 (e.g. a CP2102), pass \
                     --port <name>";
        let v = view(Some(&missing(Some(error))), false, None);
        assert_eq!(v.status, "Reader not found: no USB serial ports found");
        // The tooltip still has as much of it as fits.
        assert!(v.tooltip.contains("is the reader plugged in"), "{v:?}");
        // The tray logs the whole error when it changes.
        assert_eq!(
            reader_error(Some(&missing(Some(error)))).map(String::as_str),
            Some(error)
        );
        assert_eq!(reader_error(Some(&connected())), None);
        assert_eq!(reader_error(None), None);

        let v = view(Some(&missing(Some(&"x".repeat(500)))), true, None);
        assert_eq!(v.status.chars().count(), STATUS_MAX);
        assert!(
            v.status.starts_with("Paused — reader not found: xxx"),
            "{v:?}"
        );
        assert!(v.status.ends_with('…'), "{v:?}");

        // A config problem is never cut off, in the menu or the tooltip,
        // however long the reader error is.
        let problem = "config folder \"D:\\Sync\" unavailable: not mounted";
        for paused in [false, true] {
            for error in [error.to_owned(), "x".repeat(500)] {
                let v = view(Some(&missing(Some(&error))), paused, Some(problem));
                assert!(v.status.chars().count() <= STATUS_MAX, "{v:?}");
                assert!(v.status.ends_with("…; config unavailable"), "{v:?}");
                assert!(v.tooltip.contains(problem), "{v:?}");
            }
        }
        let v = view(Some(&missing(Some(error))), false, Some(problem));
        assert_eq!(v.status, "Reader not found: no USB se…; config unavailable");

        // `&` would be a mnemonic in a menu label.
        let v = view(Some(&missing(Some("Tom & Jerry"))), false, None);
        assert_eq!(v.status, "Reader not found: Tom && Jerry");
    }

    #[test]
    fn shorten_makes_one_short_line() {
        assert_eq!(shorten("short", 10), "short");
        assert_eq!(shorten("exactly 10", 10), "exactly 10");
        assert_eq!(shorten("exactly 11!", 10), "exactly 1…");
        // A space before the cut is dropped, not left before the `…`.
        assert_eq!(shorten("abcd efghij", 6), "abcd…");
        // First non-blank line only, whitespace collapsed.
        assert_eq!(
            shorten("\n  \r\n first   line \r\nsecond", 40),
            "first line"
        );
        assert_eq!(shorten("tab\tand  spaces", 40), "tab and spaces");
        assert_eq!(shorten("", 10), "");
        // Multibyte characters count as one and are never split.
        assert_eq!(shorten(&"é".repeat(20), 5), "éééé…");
        assert_eq!(shorten(&"😀".repeat(20), 3), "😀😀…");
        assert_eq!(shorten("日本語のエラーメッセージ", 6), "日本語のエ…");
        for max in 1..30 {
            let s = shorten(&"aé😀 ".repeat(20), max);
            assert!(s.chars().count() <= max, "{max}: {s}");
        }
    }

    #[test]
    fn long_tooltips_are_cut_to_the_windows_limit() {
        let v = view(Some(&missing(Some(&"é".repeat(300)))), false, None);
        assert_eq!(v.tooltip.encode_utf16().count(), TOOLTIP_MAX);
        assert!(v.tooltip.ends_with('…'));
        // Astral characters are two units and are never split.
        let s = truncate_utf16(&"😀".repeat(100), 10);
        assert_eq!(s, format!("{}…", "😀".repeat(4)));
    }

    #[test]
    fn only_problems_with_a_tag_notify() {
        let uid = Uid::from_hex("04a1b2c3").unwrap();
        let failed = Notice::LaunchFailed {
            uid,
            plan: LaunchPlan::Steam { appid: 440 },
            via: Via::TagSteam,
            error: "no handler".into(),
        };
        let (level, title, text) = notification(&failed, None).unwrap();
        assert_eq!(level, Level::Warning);
        assert_eq!(title, "Could not start the game");
        assert_eq!(text, "open steam://rungameid/440: no handler");
        let (level, title, text) = notification(&Notice::Unmapped { uid }, None).unwrap();
        assert_eq!(level, Level::Info);
        assert_eq!(title, "Unknown cart");
        assert!(text.contains("04A1B2C3"), "{text}");
        for quiet in [
            Notice::Removed { uid },
            Notice::Paused { uid },
            Notice::ReaderReconnected {
                port: "COM7".into(),
            },
        ] {
            assert_eq!(notification(&quiet, None), None);
        }
    }

    #[test]
    fn unavailable_config_folder_shows_in_tooltip_and_cart_notices() {
        let problem = "config folder \"D:\\Sync\" unavailable: not mounted";
        let v = view(Some(&connected()), false, Some(problem));
        assert_eq!(
            v.tooltip,
            format!("Mini Launcher — {problem}; ready (COM7)")
        );
        assert_eq!(v.status, "Ready (COM7); config unavailable");
        let v = view(Some(&connected()), true, Some(problem));
        assert!(
            v.tooltip
                .starts_with(&format!("Mini Launcher — {problem}; paused")),
            "{v:?}"
        );

        let uid = Uid::from_hex("04a1b2c3").unwrap();
        let slug = mini_launcher_core::payload::Slug::new("my-game").unwrap();
        for notice in [Notice::Unmapped { uid }, Notice::UnknownId { uid, slug }] {
            let (level, title, text) = notification(&notice, Some(problem)).unwrap();
            assert_eq!((level, title), (Level::Warning, CONFIG_UNAVAILABLE));
            assert!(text.ends_with(&format!("but {problem}.")), "{text}");
        }
        // Launch failures (e.g. steam://) are reported as usual.
        let failed = Notice::LaunchFailed {
            uid,
            plan: LaunchPlan::Steam { appid: 440 },
            via: Via::TagSteam,
            error: "no handler".into(),
        };
        assert_eq!(
            notification(&failed, Some(problem)).unwrap().1,
            "Could not start the game"
        );
    }

    #[test]
    fn mode_parses_quit_and_counts_settings() {
        assert_eq!(mode::<&str>([]), Mode::Run { settings: 0 });
        assert_eq!(
            mode(["--settings", "--other", "--settings"]),
            Mode::Run { settings: 2 }
        );
        assert_eq!(mode(["--quit"]), Mode::Quit);
        assert_eq!(mode(["--settings", "--quit"]), Mode::Quit);
        assert_eq!(mode(["quit", "-quit"]), Mode::Run { settings: 0 });
    }

    #[test]
    fn quit_requests_quit_unless_settings_is_open() {
        let mut q = QuitRequests::default();
        let t0 = Instant::now();
        assert_eq!(q.handle(false, t0), QuitAction::Quit);
        // Settings open: ask once, then stay quiet while setup repeats it.
        assert_eq!(q.handle(true, t0), QuitAction::CloseSettingsFirst);
        let soon = t0 + Duration::from_secs(1);
        assert_eq!(q.handle(true, soon), QuitAction::Ignore);
        // Closing Settings lets the next request through at once.
        assert_eq!(q.handle(false, soon), QuitAction::Quit);
        let later = t0 + QuitRequests::GAP;
        assert_eq!(q.handle(true, later), QuitAction::CloseSettingsFirst);
    }

    #[test]
    fn rate_limiter_lets_one_through_per_gap() {
        let mut r = RateLimiter::new(Duration::from_secs(10));
        let t0 = Instant::now();
        assert!(r.allow(t0));
        assert!(!r.allow(t0 + Duration::from_secs(9)));
        assert!(r.allow(t0 + Duration::from_secs(10)));
        assert!(!r.allow(t0 + Duration::from_secs(11)));
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mini-launcher-test-tray-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn config_template_is_a_valid_empty_config_and_never_overwrites() {
        assert_eq!(Config::parse(CONFIG_TEMPLATE).unwrap(), Config::default());
        let dir = temp("config");
        let default = ConfigLocation::Default(dir.join("sub"));
        let path = dir.join("sub").join("config.toml");
        let ready = |created| OpenConfig::Ready {
            path: path.clone(),
            created,
        };
        assert_eq!(ensure_config(&default).unwrap(), ready(true));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CONFIG_TEMPLATE);
        std::fs::write(&path, "# mine\n").unwrap();
        assert_eq!(ensure_config(&default).unwrap(), ready(false));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_config_never_recreates_a_vanished_override_folder() {
        let dir = temp("override");
        let synced = dir.join("synced");
        std::fs::create_dir(&synced).unwrap();
        let location = ConfigLocation::Override(synced.clone());
        assert!(matches!(
            ensure_config(&location).unwrap(),
            OpenConfig::Ready { created: true, .. }
        ));
        std::fs::remove_dir_all(&synced).unwrap();
        let got = ensure_config(&location).unwrap();
        assert!(
            matches!(&got, OpenConfig::Unavailable(e) if e.contains("unavailable")),
            "{got:?}"
        );
        assert!(!synced.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn vanished_override_folder_is_detected_without_a_config_file() {
        let (default, synced) = (temp("vanish-default"), temp("vanish-synced"));
        let text = format!("dir = '{}'\n", synced.display());
        std::fs::write(default.join("location.toml"), text).unwrap();
        let location = ConfigLocation::resolve(&default);
        assert_eq!(location, ConfigLocation::Override(synced.clone()));
        // No config.toml was ever created in it.
        assert_eq!(vanished(&location, &default), None);
        std::fs::remove_dir_all(&synced).unwrap();
        let now = vanished(&location, &default).unwrap();
        assert!(!now.is_available(), "{now:?}");
        assert_eq!(vanished(&now, &default), None);
        assert_eq!(
            vanished(&ConfigLocation::Default(default.clone()), &default),
            None
        );
        std::fs::remove_dir_all(default).unwrap();
    }

    #[test]
    fn open_config_problem_converges_when_the_folder_comes_back() {
        let dir = temp("converge");
        let synced = dir.join("synced");
        std::fs::create_dir(&synced).unwrap();
        let location = ConfigLocation::Override(synced.clone());
        let mut status = ConfigStatus::default();
        assert!(status.set_location(location.clone()).is_none());
        // Open config finds the folder gone before the worker does.
        std::fs::remove_dir(&synced).unwrap();
        let OpenConfig::Unavailable(problem) = ensure_config(&location).unwrap() else {
            panic!("folder is gone");
        };
        let (_, _, text) = status.open_config_unavailable(problem.clone());
        assert_eq!(status.problem(), Some(problem.as_str()));
        // Same text as the worker's balloon would show.
        assert!(
            text.ends_with("Only steam:// tags work until it is."),
            "{text}"
        );
        // Open config still tries the folder rather than only warning...
        assert_eq!(status.to_open(), Some(&location));
        std::fs::create_dir(&synced).unwrap();
        // ...and the worker's re-check (same location) clears the problem.
        let (level, title, _) = status.set_location(location.clone()).unwrap();
        assert_eq!((level, title), (Level::Info, "Config folder available"));
        assert_eq!(status.problem(), None);
        // Or Open config itself succeeding clears it.
        status.open_config_unavailable(problem);
        assert!(matches!(
            ensure_config(&location).unwrap(),
            OpenConfig::Ready { .. }
        ));
        assert!(status.open_config_ready());
        assert_eq!(status.problem(), None);
        assert!(!status.open_config_ready());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn worker_confirming_an_open_config_problem_does_not_repeat_the_balloon() {
        let dir = temp("confirm");
        let synced = dir.join("synced");
        let text = format!("dir = '{}'\n", synced.display());
        std::fs::write(dir.join("location.toml"), text).unwrap();
        let mut status = ConfigStatus::default();
        status.set_location(ConfigLocation::Override(synced.clone()));
        let OpenConfig::Unavailable(problem) =
            ensure_config(&ConfigLocation::Override(synced)).unwrap()
        else {
            panic!("folder is missing");
        };
        let (_, _, ui_text) = status.open_config_unavailable(problem);
        // What the worker resolves when it re-checks.
        let gone = ConfigLocation::resolve(&dir);
        assert!(!gone.is_available(), "{gone:?}");
        assert!(status.set_location(gone).is_none());
        assert_eq!(status.to_open(), None);
        assert_eq!(unavailable_balloon(status.problem().unwrap()).2, ui_text);
        // A different problem is news.
        let other = ConfigLocation::OverrideUnavailable {
            dir: None,
            reason: "bad pointer".into(),
        };
        assert!(status.set_location(other).is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retry_thread_that_died_is_respawned() {
        use std::sync::mpsc::TryRecvError;
        assert_eq!(Retry::from(Err(TryRecvError::Empty)), Retry::Wait);
        assert_eq!(Retry::from(Err(TryRecvError::Disconnected)), Retry::Respawn);
        assert_eq!(Retry::from(Ok(None)), Retry::Switch(None));
        let (tx, rx) = std::sync::mpsc::channel::<Option<ConfigLocation>>();
        let _ = std::thread::spawn(move || {
            let _tx = tx;
            panic!("retry thread panics");
        })
        .join();
        assert_eq!(Retry::from(rx.try_recv()), Retry::Respawn);
    }

    #[test]
    fn dead_retry_thread_respawns_with_backoff() {
        let mut r = Respawn::default();
        let t0 = Instant::now();
        let mut now = t0;
        let mut delays = Vec::new();
        for _ in 0..9 {
            let RespawnStep::Scheduled(d) = r.step(now) else {
                panic!("expected a new delay");
            };
            delays.push(d.as_secs());
            assert_eq!(r.step(now + d / 2), RespawnStep::Wait);
            now += d;
            assert_eq!(r.step(now), RespawnStep::Now);
        }
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 60, 60, 60]);
        // At most one restart (and one log line) per minute from here on.
        assert!(now - t0 >= Duration::from_secs(1 + 2 + 4 + 8 + 16 + 32 + 180));
    }

    #[test]
    fn config_reload_forced_after_a_save() {
        let t = Some(SystemTime::UNIX_EPOCH);
        assert!(config_changed(None, t, false));
        assert!(config_changed(Some(None), t, false));
        assert!(!config_changed(Some(t), t, false));
        assert!(config_changed(Some(t), t, true));
    }

    /// `app.rs` `balloon()` reaches into tray-icon internals (its window
    /// class and icon uID), verified against this exact version only.
    #[test]
    fn tray_icon_is_still_the_version_balloon_was_checked_against() {
        let lock = include_str!("../../../Cargo.lock");
        let version = lock
            .split("[[package]]")
            // `lines()` also strips the `\r` of a CRLF checkout.
            .find(|p| p.lines().any(|l| l == "name = \"tray-icon\""))
            .and_then(|p| p.lines().find_map(|l| l.strip_prefix("version = ")))
            .map(|v| v.trim_matches('"'))
            .expect("tray-icon in Cargo.lock");
        assert_eq!(
            version, "0.26.1",
            "tray-icon changed: re-verify balloon() and tray_window() in \
             cli/src/tray/app.rs against the new tray-icon source (window class \
             `tray_icon_app`, icon uID 1 with `TrayIcon::with_id`, no GUID), then \
             update this test"
        );
    }

    #[test]
    fn reader_cell_for_the_settings_window() {
        let connected = ReaderState::Connected {
            port: "COM3".into(),
        };
        assert_eq!(
            reader_cell(Some(&connected), false),
            ("Reader COM3: ready".into(), true)
        );
        assert_eq!(
            reader_cell(Some(&connected), true),
            ("Reader: paused".into(), false)
        );
        assert_eq!(reader_cell(None, false), ("Reader: starting".into(), false));
    }
}
