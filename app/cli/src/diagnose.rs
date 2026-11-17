//! `nfc-launcher diagnose`: ports, config location and a reader self-test.

use anyhow::Result;
use nfc_launcher_core::config::Config;
use nfc_launcher_core::pn532::FirmwareVersion;
use nfc_launcher_core::ports;

use crate::port;

pub fn run(explicit: Option<&str>) -> Result<()> {
    println!("Serial ports:");
    let ports = match ports::available() {
        Ok(p) if p.is_empty() => {
            println!("  (none)");
            p
        }
        Ok(p) => {
            for info in &p {
                println!("  {info}");
            }
            p
        }
        Err(e) => {
            println!("  cannot list serial ports: {e}");
            Vec::new()
        }
    };

    println!();
    match Config::default_path() {
        Some(path) => {
            let state = if path.exists() {
                "exists"
            } else {
                "not created yet; steam:// tags work without it"
            };
            println!("Config: {} ({state})", path.display());
        }
        None => println!("Config: no config directory available on this system"),
    }

    println!();
    let name = port::choose(explicit, &ports)?;
    println!("Reader on {name}:");
    let (_reader, fw) = port::open(&name)?;
    println!("  {}", describe(&fw));
    println!("  supports: {}", supported(fw.support));
    println!("OK: the reader is answering.");
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
