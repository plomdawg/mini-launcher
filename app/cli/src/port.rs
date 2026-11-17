//! Reader port selection and opening, with actionable error messages.
//! Shared by every command that talks to the reader.

use anyhow::{anyhow, Context, Result};
use nfc_launcher_core::pn532::{serial, FirmwareVersion, Pn532, Pn532Error};
use nfc_launcher_core::ports::{self, PortInfo};
use serialport::SerialPort;

/// The open reader.
pub type Reader = Pn532<Box<dyn SerialPort>>;

const NO_REPLY_HINT: &str = "check that the PN532 DIP switches are set to HSU (both OFF), \
     that TX/RX are crossed between the adapter and the board, and that the board is powered";

/// Picks the reader port from `ports`: `explicit` wins, else exactly one CH340.
pub fn choose(explicit: Option<&str>, ports: &[PortInfo]) -> Result<String> {
    let name = ports::select_port(explicit, ports)?;
    match explicit {
        Some(_) => tracing::info!(port = %name, "using port from --port"),
        None => tracing::info!(port = %name, "auto-detected CH340 reader port"),
    }
    Ok(name)
}

/// Lists ports (only when needed) and picks the reader port.
#[allow(dead_code)] // used by `scan`/`run`
pub fn resolve(explicit: Option<&str>) -> Result<String> {
    if explicit.is_some() {
        return choose(explicit, &[]);
    }
    let ports = ports::available().context("cannot list serial ports")?;
    choose(None, &ports)
}

/// Opens `name`, wakes the PN532 and checks it answers `GetFirmwareVersion`.
pub fn open(name: &str) -> Result<(Reader, FirmwareVersion)> {
    let mut reader = serial::open(name).map_err(|e| open_error(name, &e))?;
    tracing::debug!(port = name, "port open, waking PN532");
    let mut last = Pn532Error::Timeout;
    // The first command after power-up is sometimes lost; try twice.
    for attempt in 1..=2 {
        let result = reader.wakeup().and_then(|()| reader.get_firmware_version());
        match result {
            Ok(fw) => return Ok((reader, fw)),
            Err(e) => {
                tracing::debug!(attempt, error = %e, "no firmware reply");
                last = e;
            }
        }
    }
    let e = last;
    Err(match e {
        Pn532Error::Timeout | Pn532Error::NoAck => {
            anyhow!("no reply from the PN532 on {name} ({e}); {NO_REPLY_HINT}")
        }
        Pn532Error::Io(io) => anyhow!("I/O error on {name}: {io}; was the adapter unplugged?"),
        other => anyhow!("garbled reply from the PN532 on {name} ({other}); {NO_REPLY_HINT}"),
    })
}

fn open_error(name: &str, e: &serialport::Error) -> anyhow::Error {
    anyhow!("cannot open {name}: {e}; {}", open_hint(e.kind()))
}

/// What to try when opening a port fails. `serialport` reports errors
/// differently per OS: on Windows a missing COM port and one held by another
/// program are both `NoDevice`; on Unix `NoDevice` means busy (EBUSY/flock).
fn open_hint(kind: serialport::ErrorKind) -> &'static str {
    use serialport::ErrorKind;
    use std::io::ErrorKind as Io;
    match kind {
        ErrorKind::NoDevice if cfg!(windows) => {
            "COM port not found or in use by another program: run `nfc-launcher diagnose` \
             to list ports (or check Device Manager for the number) and close other serial apps"
        }
        ErrorKind::NoDevice => {
            "the port is busy: close other programs using it (serial monitors, Arduino IDE, \
             another nfc-launcher)"
        }
        ErrorKind::Io(Io::NotFound) => "no such port; run `nfc-launcher diagnose` to list ports",
        ErrorKind::Io(Io::PermissionDenied) if cfg!(target_os = "linux") => {
            "add yourself to the `dialout` group (then log out and in) \
             or install the udev rule granting `uaccess`"
        }
        ErrorKind::Io(Io::PermissionDenied) => "check that your user may access the port",
        _ => "check the port name and that the adapter is plugged in",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::ErrorKind;
    use std::io::ErrorKind as Io;

    #[test]
    fn hints_match_the_os_error_mapping() {
        let no_device = open_hint(ErrorKind::NoDevice);
        if cfg!(windows) {
            assert!(no_device.contains("not found or in use"));
        } else {
            assert!(no_device.contains("busy"));
            assert!(!no_device.contains("dialout"));
        }
        let denied = open_hint(ErrorKind::Io(Io::PermissionDenied));
        assert_eq!(denied.contains("dialout"), cfg!(target_os = "linux"));
        assert!(open_hint(ErrorKind::Io(Io::NotFound)).contains("diagnose"));
    }
}
