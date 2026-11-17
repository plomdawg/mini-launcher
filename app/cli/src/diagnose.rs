//! `mini-launcher diagnose`: ports, config location and a reader self-test.

use std::io::Write;

use anyhow::Result;
use std::path::Path;

use mini_launcher_core::config::location::{ConfigLocation, POINTER_FILE};
use mini_launcher_core::config::Config;
use mini_launcher_core::logs;
use mini_launcher_core::pn532::FirmwareVersion;
use mini_launcher_core::ports;

use crate::port;

/// Runs the diagnosis, writing the report to `out`. `all` also lists non-USB ports.
pub fn run(out: &mut impl Write, explicit: Option<&str>, all: bool) -> Result<()> {
    writeln!(out, "Serial ports:")?;
    let ports = match ports::available() {
        Ok(p) => p,
        Err(e) => {
            writeln!(out, "  cannot list serial ports: {e}")?;
            Vec::new()
        }
    };
    let (shown, hidden) = ports::visible(&ports, all);
    if shown.is_empty() {
        writeln!(out, "  (none)")?;
    }
    for info in shown {
        writeln!(out, "  {info}")?;
    }
    if hidden > 0 {
        writeln!(out, "  {}", ports::hidden_note(hidden, "--all"))?;
    }

    writeln!(out)?;
    config_report(out, Config::default_dir().as_deref())?;
    match logs::dir() {
        Some(dir) => writeln!(out, "Logs: {} (written by `run`)", dir.display())?,
        None => writeln!(out, "Logs: no data directory available on this system")?,
    }

    writeln!(out)?;
    let name = port::choose(explicit, &ports)?;
    writeln!(out, "Reader on {name}:")?;
    out.flush()?;
    let (_reader, fw) = port::open(&name, true)?;
    writeln!(out, "  {}", describe(&fw))?;
    writeln!(out, "  supports: {}", supported(fw.support))?;
    writeln!(out, "OK: the reader is answering.")?;
    Ok(())
}

/// The `Config:` lines, given the default config folder.
fn config_report(out: &mut impl Write, default_dir: Option<&Path>) -> Result<()> {
    let Some(default_dir) = default_dir else {
        writeln!(out, "Config: no config directory available on this system")?;
        return Ok(());
    };
    let pointer = default_dir.join(POINTER_FILE);
    let location = ConfigLocation::resolve(default_dir);
    if location != ConfigLocation::Default(default_dir.to_owned()) {
        writeln!(out, "Config folder override: {}", pointer.display())?;
    }
    match location.config_file() {
        Ok(path) => {
            let state = if path.exists() {
                "exists"
            } else {
                "not created yet; steam:// tags work without it"
            };
            writeln!(out, "Config: {} ({state})", path.display())?;
        }
        Err(e) => {
            writeln!(out, "Config: {e}")?;
            writeln!(
                out,
                "  Until it is available, steam:// tags still launch, but id: tags and UID \
                 mappings do not, and nothing can be saved. Make the folder available, or \
                 fix or delete {} to use the default folder.",
                pointer.display()
            )?;
        }
    }
    Ok(())
}

fn describe(fw: &FirmwareVersion) -> String {
    let chip = if fw.ic == 0x32 {
        "PN532"
    } else {
        "unknown chip"
    };
    format!("{chip} (IC {:#04x}), firmware {}.{}", fw.ic, fw.ver, fw.rev)
}

/// Decodes the `Support` byte of `GetFirmwareVersion` (see the PN532 user manual).
fn supported(flags: u8) -> String {
    let names: Vec<&str> = [
        (0x01, "ISO/IEC 14443 Type A"),
        (0x02, "ISO/IEC 14443 Type B"),
        (0x04, "ISO 18092"),
    ]
    .into_iter()
    .filter(|(bit, _)| flags & bit != 0)
    .map(|(_, name)| name)
    .collect();
    if names.is_empty() {
        format!("none reported ({flags:#04x})")
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_an_unavailable_config_folder() {
        let dir = std::env::temp_dir().join(format!(
            "mini-launcher-test-diagnose-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let report = |dir: &Path| {
            let mut out = Vec::new();
            config_report(&mut out, Some(dir)).unwrap();
            String::from_utf8(out).unwrap()
        };
        assert!(report(&dir).starts_with("Config: "), "{}", report(&dir));
        let missing = dir.join("not-mounted");
        let text = format!("dir = '{}'\n", missing.display());
        std::fs::write(dir.join(POINTER_FILE), text).unwrap();
        let out = report(&dir);
        assert!(out.starts_with("Config folder override: "), "{out}");
        assert!(out.contains("unavailable"), "{out}");
        assert!(out.contains("steam:// tags still launch"), "{out}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn describes_firmware() {
        let fw = FirmwareVersion {
            ic: 0x32,
            ver: 1,
            rev: 6,
            support: 0x07,
        };
        assert_eq!(describe(&fw), "PN532 (IC 0x32), firmware 1.6");
        assert_eq!(
            supported(fw.support),
            "ISO/IEC 14443 Type A, ISO/IEC 14443 Type B, ISO 18092"
        );
        assert_eq!(supported(0), "none reported (0x00)");
    }
}
