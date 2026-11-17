//! `mini-launcher run`: reader → debounce → resolve via config → launch.

use std::fmt::Display;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use mini_launcher_core::config::Config;
use mini_launcher_core::debounce::{DebounceConfig, Debouncer, Event, DEFAULT_POLL_INTERVAL};
use mini_launcher_core::instance;
use mini_launcher_core::launch::{self, Outcome, Resolution, Via};
use mini_launcher_core::reader::{Reader, Simulated, Uid};
use mini_launcher_core::reconnect::{Backoff, Clock, Reconnector, SystemClock, Tick};

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
        let reopen = || port::open_poller(explicit);
        let mut reader = Reconnector::new(name, reader, reopen, Backoff::default());
        poll_loop(out, &ctx, stop, &mut SystemClock, |now| {
            Some(reader.tick(now))
        })
    }
}

/// Feeds `next` ticks to the debouncer once per poll interval until `stop`
/// is set or `next` returns `None` (the source is closed).
fn poll_loop<E: Display>(
    out: &mut impl Write,
    ctx: &Ctx,
    stop: &AtomicBool,
    clock: &mut impl Clock,
    mut next: impl FnMut(Instant) -> Option<Tick<E>>,
) -> Result<()> {
    let mut debouncer = Debouncer::new(DebounceConfig::default());
    while !stop.load(Ordering::Relaxed) {
        let now = clock.now();
        let Some(tick) = next(now) else { break };
        let events = match tick {
            Tick::Reading(reading) => debouncer.update(reading, now),
            Tick::Lost { port, error } => {
                tracing::warn!(
                    "lost the reader on {port} ({error}); was the adapter unplugged? \
                     Reconnecting when it is back"
                );
                debouncer.disconnect().into_iter().collect()
            }
            Tick::Reconnected { port } => {
                tracing::info!(%port, "reader reconnected");
                writeln!(out, "reader reconnected on {port}")?;
                out.flush()?;
                Vec::new()
            }
            Tick::Waiting => Vec::new(),
        };
        for event in events {
            handle(out, ctx, event)?;
            out.flush()?;
        }
        clock.sleep(DEFAULT_POLL_INTERVAL);
    }
    Ok(())
}

fn handle(out: &mut impl Write, ctx: &Ctx, event: Event) -> Result<()> {
    let (uid, payload) = match event {
        Event::Inserted { uid, payload } => (uid, payload),
        Event::Reseated { uid, .. } => {
            tracing::info!(%uid, "reseated; not relaunching");
            return Ok(writeln!(out, "reseated {uid}: not relaunching")?);
        }
        Event::Removed { uid } => {
            tracing::info!(%uid, "removed");
            return Ok(writeln!(out, "removed {uid}")?);
        }
    };
    match launch::resolve(&ctx.config, uid.as_bytes(), &payload) {
        Resolution::Launch { plan, via } => {
            let via = match via {
                Via::Uid(slug) => format!("uid mapped to \"{slug}\""),
                Via::TagSteam => "tag steam:".to_owned(),
                Via::TagId(slug) => format!("tag id:{slug}"),
            };
            let line = format!("launch {uid}: {plan} ({via})");
            match launch::execute(&plan, ctx.dry_run) {
                Ok(Outcome::DryRun) => {
                    tracing::info!("{line} [dry run]");
                    writeln!(out, "{line} [dry run]")?;
                }
                Ok(Outcome::Started) => {
                    tracing::info!("{line}");
                    writeln!(out, "{line}")?;
                }
                Err(e) => {
                    tracing::warn!("{line}: failed to start: {e}");
                    writeln!(out, "{line}: failed to start: {e}")?;
                }
            }
        }
        Resolution::UnknownId(slug) => {
            tracing::warn!(%uid, "tag says id:{slug}, which is not in the config");
            writeln!(
                out,
                "unknown tag {uid}: it says id:{slug}, but the config has no game with that id; \
                 not launching. Add {}:",
                config_name(ctx.path.as_deref())
            )?;
            snippet(out, &slug.to_string(), None)?;
        }
        Resolution::Unmapped => {
            tracing::warn!(%uid, "unknown tag: nothing usable on it and its UID is not mapped");
            writeln!(
                out,
                "unknown tag {uid}: nothing usable on it and its UID is not mapped; \
                 not launching. To map it, add {}:",
                config_name(ctx.path.as_deref())
            )?;
            snippet(out, "my-game", Some(uid))?;
        }
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
    /// `failed_opens` reopens fail and then it comes back with the tag still
    /// on it. Returns the output of `ticks` polls.
    fn unplug_and_replug(failed_opens: u32, ticks: u32) -> String {
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
        let open = || {
            opens += 1;
            if opens <= failed_opens {
                return Err("adapter not found");
            }
            Ok(("COM9".to_owned(), Fake(VecDeque::new())))
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
        let out = unplug_and_replug(1, 50);
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
    fn long_outage_relaunches_the_tag_like_a_fresh_insert() {
        // Five failed reopens: 0.5+1+2+4+5+5 s of backoff, past the window.
        let out = unplug_and_replug(5, 150);
        assert_eq!(out.matches("launch 04A1B2C3").count(), 2, "{out}");
        assert!(out.contains("reader reconnected on COM9"), "{out}");
        assert!(!out.contains("reseated"), "{out}");
    }
}
