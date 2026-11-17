//! What the tray shows, as pure functions of the reader state and the run
//! loop's notices, so it is tested on every OS.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use mini_launcher_core::reconnect::ReaderState;
use mini_launcher_core::runner::Notice;

pub const APP: &str = "Mini Launcher";

/// Windows keeps 128 UTF-16 units of tooltip, including the terminator.
const TOOLTIP_MAX: usize = 127;

/// The tooltip and menu labels for one state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub tooltip: String,
    /// The (disabled) status line at the top of the menu.
    pub status: String,
    pub pause_label: &'static str,
}

/// `reader` is `None` until the first open attempt has run.
pub fn view(reader: Option<&ReaderState>, paused: bool) -> View {
    let reader = match reader {
        None => "starting…".to_owned(),
        Some(ReaderState::Connected { port }) => format!("ready ({port})"),
        Some(ReaderState::Reconnecting { last_error, .. }) => match last_error {
            Some(e) => format!("reader not found, retrying… ({e})"),
            None => "reader not found, retrying…".to_owned(),
        },
    };
    let (tooltip, status) = if paused {
        ("paused".to_owned(), format!("Paused — {reader}"))
    } else {
        (reader.clone(), capitalize(&reader))
    };
    View {
        tooltip: truncate_utf16(&format!("{APP} — {tooltip}"), TOOLTIP_MAX),
        status,
        pause_label: if paused { "Resume" } else { "Pause" },
    }
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

/// How a notification is shown: an info or a warning icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warning,
}

/// The balloon (level, title, text) for notices the user should see;
/// everything else only goes to the log. A failed launch is a warning; an
/// unknown cart is the user's to map, so it is info.
pub fn notification(notice: &Notice) -> Option<(Level, &'static str, String)> {
    match notice {
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
# id = "my-game"
# name = "My Game"
# exe = 'C:\Games\MyGame\game.exe'
# args = ["--fullscreen"]         # optional
# cwd = 'C:\Games\MyGame'         # optional
"#;

/// Creates `path` (and its folder) from [`CONFIG_TEMPLATE`] unless it
/// exists. Returns whether it was created.
pub fn ensure_config(path: &Path) -> io::Result<bool> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            io::Write::write_all(&mut file, CONFIG_TEMPLATE.as_bytes())?;
            Ok(true)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
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
        let v = view(Some(&connected()), false);
        assert_eq!(v.tooltip, "Mini Launcher — ready (COM7)");
        assert_eq!(v.status, "Ready (COM7)");
        assert_eq!(v.pause_label, "Pause");

        let v = view(Some(&missing(Some("no CH340 adapter found"))), false);
        assert_eq!(
            v.tooltip,
            "Mini Launcher — reader not found, retrying… (no CH340 adapter found)"
        );
        assert_eq!(
            v.status,
            "Reader not found, retrying… (no CH340 adapter found)"
        );
        let v = view(Some(&missing(None)), false);
        assert_eq!(v.tooltip, "Mini Launcher — reader not found, retrying…");

        assert_eq!(view(None, false).status, "Starting…");
    }

    #[test]
    fn paused_wins_the_tooltip_and_flips_the_menu_label() {
        let v = view(Some(&connected()), true);
        assert_eq!(v.tooltip, "Mini Launcher — paused");
        assert_eq!(v.status, "Paused — ready (COM7)");
        assert_eq!(v.pause_label, "Resume");
        let v = view(Some(&missing(None)), true);
        assert_eq!(v.tooltip, "Mini Launcher — paused");
        assert_eq!(v.status, "Paused — reader not found, retrying…");
    }

    #[test]
    fn long_tooltips_are_cut_to_the_windows_limit() {
        let v = view(Some(&missing(Some(&"é".repeat(300)))), false);
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
        let (level, title, text) = notification(&failed).unwrap();
        assert_eq!(level, Level::Warning);
        assert_eq!(title, "Could not start the game");
        assert_eq!(text, "open steam://rungameid/440: no handler");
        let (level, title, text) = notification(&Notice::Unmapped { uid }).unwrap();
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
            assert_eq!(notification(&quiet), None);
        }
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

    #[test]
    fn config_template_is_a_valid_empty_config_and_never_overwrites() {
        assert_eq!(Config::parse(CONFIG_TEMPLATE).unwrap(), Config::default());
        let dir = std::env::temp_dir().join(format!(
            "mini-launcher-test-tray-config-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub").join("config.toml");
        assert!(ensure_config(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CONFIG_TEMPLATE);
        std::fs::write(&path, "# mine\n").unwrap();
        assert!(!ensure_config(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# mine\n");
        std::fs::remove_dir_all(dir).unwrap();
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
}
