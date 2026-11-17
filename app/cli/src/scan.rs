//! `nfc-launcher scan`: dump what the reader sees on each newly placed tag.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use nfc_launcher_core::debounce::{DebounceConfig, DEFAULT_POLL_INTERVAL};
use nfc_launcher_core::payload::{steam_launch_uri, Payload};
use nfc_launcher_core::pn532::reader::{self, decode, is_fatal, FIRST_USER_PAGE};
use nfc_launcher_core::pn532::Target;
use nfc_launcher_core::reader::Uid;

use crate::port;

/// Polls until `stop` is set, printing each new tag once.
pub fn run(out: &mut impl Write, explicit: Option<&str>, stop: &AtomicBool) -> Result<()> {
    let (name, mut reader) = port::open_poller(explicit)?;
    writeln!(
        out,
        "Scanning on {name}. Place a tag on the reader (Ctrl-C to stop)."
    )?;
    out.flush()?;
    let miss_threshold = DebounceConfig::default().miss_threshold;
    let (mut seen, mut misses) = (None::<Vec<u8>>, 0);
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(DEFAULT_POLL_INTERVAL);
        let Some(t) = reader.detect().map_err(|e| port::lost(&name, e))? else {
            misses += 1;
            if misses == miss_threshold && seen.take().is_some() {
                writeln!(out, "tag removed")?;
            }
            continue;
        };
        misses = 0;
        if seen.as_ref() != Some(&t.uid) {
            let mem = match reader.read_user_memory() {
                Err(e) if is_fatal(&e) => return Err(port::lost(&name, e)),
                mem => mem.map_err(|e| e.to_string()),
            };
            report(out, &t, mem)?;
            out.flush()?;
            seen = Some(t.uid);
        }
        reader.release().map_err(|e| port::lost(&name, e))?;
    }
    Ok(())
}

/// Prints one tag: UID, user pages, the NDEF record and the parsed payload.
fn report(out: &mut impl Write, t: &Target, mem: Result<Vec<u8>, String>) -> Result<()> {
    let uid = match Uid::new(&t.uid) {
        Ok(uid) => uid.to_string(),
        Err(e) => format!("{} ({e})", hex(&t.uid)),
    };
    writeln!(
        out,
        "\ntag {uid}  (ATQA {:02X}{:02X}, SAK {:02X})",
        t.atqa[0], t.atqa[1], t.sak
    )?;
    let mem = match mem {
        Ok(mem) => mem,
        Err(e) => {
            writeln!(out, "  cannot read pages: {e} (not an NTAG21x tag?)")?;
            return Ok(());
        }
    };
    for (i, page) in mem.chunks(4).enumerate() {
        let ascii: String = page
            .iter()
            .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
            .collect();
        let n = usize::from(FIRST_USER_PAGE) + i;
        writeln!(out, "  page {n:2} (0x{n:02X}): {}  {ascii}", hex(page))?;
    }
    match decode(&mem) {
        Ok(record) => writeln!(out, "  NDEF: {record:?}")?,
        Err(e) => writeln!(out, "  NDEF: none ({e})")?,
    }
    let payload = match reader::payload_from_memory(&mem) {
        Payload::Steam(appid) => format!("Steam app {appid} ({})", steam_launch_uri(appid)),
        Payload::Id(slug) => format!("library id \"{slug}\" (looked up in config.toml)"),
        Payload::Unusable => "unusable; the tag UID is used to look it up in config.toml".into(),
    };
    writeln!(out, "  payload: {payload}")?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
