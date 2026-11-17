//! `mini-launcher write`: write a payload to the next tag placed on the reader.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use mini_launcher_core::debounce::DEFAULT_POLL_INTERVAL;
use mini_launcher_core::instance;
use mini_launcher_core::payload::Payload;
use mini_launcher_core::pn532::write::{write_payload, WriteError};
use mini_launcher_core::reader::Uid;

use crate::port;

/// Validates `text` with the same rules as a tag payload. Only the parsed
/// appid / slug is written, in canonical form.
pub fn parse(text: &str) -> Result<Payload> {
    match Payload::parse(text) {
        Payload::Unusable => bail!(
            "{text:?} is not a valid payload; expected steam://<appid> or id:<slug> \
             (see docs/tag-format.md)"
        ),
        p => Ok(p),
    }
}

/// Waits up to `timeout` for a tag, writes `payload` to it and verifies it.
pub fn run(
    out: &mut impl Write,
    payload: &Payload,
    explicit: Option<&str>,
    timeout: Duration,
    stop: &AtomicBool,
) -> Result<()> {
    if instance::is_running() {
        bail!("Mini Launcher is running (tray app or `mini-launcher run`) and holds the reader; quit it first");
    }
    let (name, mut reader) = port::open_poller(explicit)?;
    let shown = match payload {
        Payload::Steam(appid) => format!("steam://{appid}"),
        Payload::Id(slug) => format!("id:{slug}"),
        Payload::Unusable => bail!("nothing to write"),
    };
    writeln!(
        out,
        "Writing {shown} on {name}. Place a tag on the reader (Ctrl-C to cancel)."
    )?;
    out.flush()?;
    let deadline = Instant::now() + timeout;
    while !stop.load(Ordering::Relaxed) {
        if Instant::now() >= deadline {
            bail!("no tag within {} s", timeout.as_secs());
        }
        std::thread::sleep(DEFAULT_POLL_INTERVAL);
        let Some(target) = reader.detect().map_err(|e| port::lost(&name, e))? else {
            continue;
        };
        let uid = Uid::new(&target.uid).map_or_else(|_| "?".into(), |u| u.to_string());
        let result = write_payload(reader.driver(), &target, payload);
        // Best effort: the outcome is already known.
        let _ = reader.release();
        return match result {
            Ok(w) => {
                writeln!(
                    out,
                    "Wrote and verified {shown} on tag {uid} (pages {}-{}).",
                    w.first_page, w.last_page
                )?;
                Ok(())
            }
            Err(WriteError::Transport(e)) => Err(port::lost(&name, e)),
            Err(
                e @ (WriteError::Locked | WriteError::TooSmall { .. } | WriteError::UnknownTag),
            ) => {
                bail!("cannot write tag {uid}: {e}; map its UID in config.toml instead")
            }
            Err(e) => {
                bail!("writing tag {uid} failed: {e}; place it flat on the reader and try again")
            }
        };
    }
    bail!("cancelled; nothing was written")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_uses_the_tag_payload_rules() {
        assert_eq!(parse("steam://440").unwrap(), Payload::Steam(440));
        assert_eq!(parse("steam:0440").unwrap(), Payload::Steam(440));
        assert!(matches!(parse("id:my-game").unwrap(), Payload::Id(_)));
        for bad in ["steam://0", "steam://440;calc", "id:../x", "C:\\x.exe", ""] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
