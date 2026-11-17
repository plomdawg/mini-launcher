//! `mini-launcher run`: reader → debounce → resolve via config → launch.

use std::fmt::Display;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context, Result};
use mini_launcher_core::config::Config;
use mini_launcher_core::instance;
use mini_launcher_core::reader::{Reader, Simulated, Uid};
use mini_launcher_core::reconnect::{Backoff, Clock, Reconnector, SystemClock, Tick};
use mini_launcher_core::runner::{self, describe, Notice, Runner};

use crate::port;

/// Where tags come from and what to do with them.
pub struct Options {
    pub port: Option<String>,
    pub simulate: bool,
    pub dry_run: bool,
    /// Overrides the default config location.
    pub config: Option<PathBuf>,
}

struct Ctx {
    config: Config,
    path: Option<PathBuf>,
    dry_run: bool,
}

/// Runs until `stop` is set or simulated input ends.
pub fn run(out: &mut impl Write, opts: Options, stop: &AtomicBool) -> Result<()> {
    // Simulated runs touch no reader, so they may run alongside a real one.
    let _instance = if opts.simulate {
        None
    } else {
        let guard = instance::acquire().context("cannot check for another running instance")?;
        Some(guard.context(
            "mini-launcher is already running (only one copy can use the reader); \
             quit the other one first",
        )?)
    };
    // An explicit --config must exist; a missing default config is empty.
    let (path, load): (_, fn(&Path) -> _) = match opts.config {
        Some(p) => (Some(p), Config::load),
        None => (Config::default_path(), Config::load_or_empty),
    };
    let config = match &path {
        Some(p) => load(p).with_context(|| format!("cannot load {}", p.display()))?,
        None => {
            tracing::warn!("no config directory on this system; only steam:// tags will work");
            Config::default()
        }
    };
    if let Some(p) = &path {
        writeln!(
            out,
            "config: {} ({} games)",
            p.display(),
            config.games.len()
        )?;
    }
    let ctx = Ctx {
        config,
        path,
        dry_run: opts.dry_run,
    };
    if opts.simulate {
        writeln!(
            out,
            "simulating: one command per line, `insert <uid-hex> [payload]` or `remove`"
        )?;
        let mut sim = Simulated::spawn(BufReader::new(std::io::stdin()));
        poll_loop(out, &ctx, stop, &mut SystemClock, |_| {
            if sim.is_closed() {
                return None;
            }
            Some(sim.poll().map_or_else(
                |e| {
                    tracing::warn!("ignoring input line: {e}");
                    Tick::<std::convert::Infallible>::Waiting
                },
                Tick::Reading,
            ))
        })
    } else {
        let explicit = opts.port.as_deref();
        let (name, reader) = port::open_poller(explicit)?;
        tracing::info!(port = %name, "watching for tags");
        writeln!(out, "watching {name} for tags (Ctrl-C to stop)")?;
        // Without --port, auto-detection runs again on every attempt: the
        // adapter may come back under another name.
        let reopen = || port::reopen_poller(explicit);
        let mut reader = Reconnector::new(name, reader, reopen, Backoff::default());
        poll_loop(out, &ctx, stop, &mut SystemClock, |now| {
            Some(reader.tick(now))
        })
    }
}

/// Runs the shared loop, printing each notice to `out`.
fn poll_loop<E: Display>(
    out: &mut impl Write,
    ctx: &Ctx,
    stop: &AtomicBool,
    clock: &mut impl Clock,
    mut next: impl FnMut(Instant) -> Option<Tick<E>>,
) -> Result<()> {
    let mut runner = Runner::new(ctx.config.clone(), ctx.dry_run);
    runner::poll_loop(
        &mut runner,
        stop,
        clock,
        |_, now| next(now),
        |notice| {
            print(out, ctx.path.as_deref(), notice)?;
            Ok(out.flush()?)
        },
    )
}

fn print(out: &mut impl Write, path: Option<&Path>, notice: Notice) -> Result<()> {
    match notice {
        Notice::Launched {
            uid,
            plan,
            via,
            dry_run,
        } => {
            let suffix = if dry_run { " [dry run]" } else { "" };
            writeln!(out, "launch {uid}: {plan} ({}){suffix}", describe(&via))?;
        }
        Notice::LaunchFailed {
            uid,
            plan,
            via,
            error,
        } => {
            let via = describe(&via);
            writeln!(
                out,
                "launch {uid}: {plan} ({via}): failed to start: {error}"
            )?;
        }
        Notice::Paused { uid } => writeln!(out, "paused: not launching {uid}")?,
        Notice::Reseated { uid } => writeln!(out, "reseated {uid}: not relaunching")?,
        Notice::Removed { uid } => writeln!(out, "removed {uid}")?,
        Notice::UnknownId { uid, slug } => {
            writeln!(
                out,
                "unknown tag {uid}: it says id:{slug}, but the config has no game with that id; \
                 not launching. Add {}:",
                config_name(path)
            )?;
            snippet(out, &slug.to_string(), None)?;
        }
        Notice::Unmapped { uid } => {
            writeln!(
                out,
                "unknown tag {uid}: nothing usable on it and its UID is not mapped; \
                 not launching. To map it, add {}:",
                config_name(path)
            )?;
            snippet(out, "my-game", Some(uid))?;
        }
        // Logged by the runner; the user sees the removal it causes.
        Notice::ReaderLost { .. } => {}
        Notice::ReaderReconnected { port } => writeln!(out, "reader reconnected on {port}")?,
        Notice::ReaderStuck {
            attempts, error, ..
        } => writeln!(
            out,
            "reader still not back after {attempts} attempts: {error}"
        )?,
    }
    Ok(())
}

fn config_name(path: Option<&Path>) -> String {
    match path {
        Some(p) => format!("to {}", p.display()),
        None => "to your config.toml".to_owned(),
    }
}

/// A `[[games]]` entry to copy into the config.
fn snippet(out: &mut impl Write, id: &str, uid: Option<Uid>) -> Result<()> {
    writeln!(out, "    [[games]]")?;
    writeln!(out, "    id = \"{id}\"")?;
    writeln!(out, "    name = \"My Game\"")?;
    if let Some(uid) = uid {
        writeln!(out, "    uid = \"{uid}\"")?;
    }
    writeln!(
        out,
        "    steam_appid = 440   # or: exe = 'C:\\Games\\MyGame\\game.exe'"
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_launcher_core::payload::Payload;
    use std::collections::VecDeque;
    use std::io;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// Advances on `sleep`; sets `stop` after `ticks` sleeps.
    struct FakeClock<'a> {
        now: Instant,
        ticks: u32,
        stop: &'a AtomicBool,
    }

    impl Clock for FakeClock<'_> {
        fn now(&self) -> Instant {
            self.now
        }
        fn sleep(&mut self, d: Duration) {
            self.now += d;
            self.ticks -= 1;
            if self.ticks == 0 {
                self.stop.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Replays polls; runs dry → the tag stays on the reader.
    struct Fake(VecDeque<io::Result<Option<(Uid, Payload)>>>);

    impl Reader for Fake {
        type Error = io::Error;
        fn poll(&mut self) -> io::Result<Option<(Uid, Payload)>> {
            self.0.pop_front().unwrap_or_else(tag)
        }
    }

    fn tag() -> io::Result<Option<(Uid, Payload)>> {
        Ok(Some((
            Uid::from_hex("04a1b2c3").unwrap(),
            Payload::Steam(440),
        )))
    }

    /// The tag sits on the reader, the reader is unplugged, the first
    /// `failed_opens` reopens fail and then it comes back, first polling
    /// `after` and then the tag. Returns the output of `ticks` polls.
    fn unplug_and_replug(
        failed_opens: u32,
        ticks: u32,
        after: Vec<io::Result<Option<(Uid, Payload)>>>,
    ) -> String {
        let ctx = Ctx {
            config: Config::default(),
            path: None,
            dry_run: true,
        };
        let stop = AtomicBool::new(false);
        let mut clock = FakeClock {
            now: Instant::now(),
            ticks,
            stop: &stop,
        };
        let mut opens = 0;
        let mut after = after;
        let open = || {
            opens += 1;
            if opens <= failed_opens {
                return Err("adapter not found");
            }
            Ok(("COM9".to_owned(), Fake(after.drain(..).collect())))
        };
        let first = Fake([tag(), tag(), Err(io::ErrorKind::BrokenPipe.into())].into());
        let mut reader = Reconnector::new("COM7".into(), first, open, Backoff::default());
        let mut out = Vec::new();
        poll_loop(&mut out, &ctx, &stop, &mut clock, |now| {
            Some(reader.tick(now))
        })
        .unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn quick_replug_keeps_running_without_relaunching() {
        // Lost at 400 ms; reopens at 1 s (fails), then ~2 s: inside the 5 s
        // re-seat window.
        let out = unplug_and_replug(1, 50, Vec::new());
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "launch 04A1B2C3: open steam://rungameid/440 (tag steam:) [dry run]",
                "removed 04A1B2C3",
                "reader reconnected on COM9",
                "reseated 04A1B2C3: not relaunching",
            ]
        );
    }

    #[test]
    fn long_outage_with_the_tag_left_on_does_not_relaunch() {
        // Six failed reopens: 0.5+1+2+4+5+5+5 s of backoff, past the window.
        let out = unplug_and_replug(6, 150, Vec::new());
        let lines: Vec<_> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "launch 04A1B2C3: open steam://rungameid/440 (tag steam:) [dry run]",
                "removed 04A1B2C3",
                "reader still not back after 5 attempts: adapter not found",
                "reader reconnected on COM9",
                "reseated 04A1B2C3: not relaunching",
            ]
        );
    }

    #[test]
    fn tag_lifted_after_a_long_outage_relaunches_when_put_back() {
        let lifted = (0..5).map(|_| Ok(None)).collect();
        let out = unplug_and_replug(6, 150, lifted);
        assert_eq!(out.matches("launch 04A1B2C3").count(), 2, "{out}");
        assert!(!out.contains("reseated"), "{out}");
    }

    #[test]
    fn different_tag_after_a_long_outage_launches() {
        let other = Ok(Some((
            Uid::from_hex("deadbeef").unwrap(),
            Payload::Steam(70),
        )));
        let out = unplug_and_replug(6, 150, vec![other, Ok(None), Ok(None)]);
        assert!(out.contains("launch DEADBEEF"), "{out}");
    }
}
