//! `nfc-launcher run`: reader → debounce → resolve via config → launch.

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use nfc_launcher_core::config::Config;
use nfc_launcher_core::debounce::{DebounceConfig, Debouncer, Event, DEFAULT_POLL_INTERVAL};
use nfc_launcher_core::launch::{self, Outcome, Resolution, Via};
use nfc_launcher_core::reader::{Reader, Simulated, Uid};

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
        poll_loop(out, &mut sim, &ctx, stop, |e| {
            tracing::warn!("ignoring input line: {e}");
            None
        })
    } else {
        let (name, mut reader) = port::open_poller(opts.port.as_deref())?;
        writeln!(out, "watching {name} for tags (Ctrl-C to stop)")?;
        poll_loop(out, &mut reader, &ctx, stop, |e| Some(port::lost(&name, e)))
    }
}

/// Polls `reader`, feeding the debouncer. `on_error` turns a poll error into
/// a fatal error, or `None` to skip the tick.
fn poll_loop<R: Reader>(
    out: &mut impl Write,
    reader: &mut R,
    ctx: &Ctx,
    stop: &AtomicBool,
    on_error: impl Fn(R::Error) -> Option<anyhow::Error>,
) -> Result<()> {
    let mut debouncer = Debouncer::new(DebounceConfig::default());
    while !stop.load(Ordering::Relaxed) && !reader.is_closed() {
        match reader.poll() {
            Ok(reading) => {
                for event in debouncer.update(reading, Instant::now()) {
                    handle(out, ctx, event)?;
                    out.flush()?;
                }
            }
            Err(e) => {
                if let Some(fatal) = on_error(e) {
                    return Err(fatal);
                }
            }
        }
        std::thread::sleep(DEFAULT_POLL_INTERVAL);
    }
    Ok(())
}

fn handle(out: &mut impl Write, ctx: &Ctx, event: Event) -> Result<()> {
    let (uid, payload) = match event {
        Event::Inserted { uid, payload } => (uid, payload),
        Event::Reseated { uid, .. } => {
            return Ok(writeln!(out, "reseated {uid}: not relaunching")?);
        }
        Event::Removed { uid } => return Ok(writeln!(out, "removed {uid}")?),
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
                Ok(Outcome::DryRun) => writeln!(out, "{line} [dry run]")?,
                Ok(Outcome::Started) => writeln!(out, "{line}")?,
                Err(e) => writeln!(out, "{line}: failed to start: {e}")?,
            }
        }
        Resolution::UnknownId(slug) => {
            writeln!(
                out,
                "unknown tag {uid}: it says id:{slug}, but the config has no game with that id; \
                 not launching. Add {}:",
                config_name(ctx.path.as_deref())
            )?;
            snippet(out, &slug.to_string(), None)?;
        }
        Resolution::Unmapped => {
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
