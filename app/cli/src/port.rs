//! Reader port selection and opening, with actionable error messages.
//! Shared by every command that talks to the reader.

use anyhow::{anyhow, Context, Result};
use mini_launcher_core::pn532::{serial, FirmwareVersion, Pn532, Pn532Error, Pn532Hsu};
use mini_launcher_core::ports::{self, PortInfo};
use serialport::SerialPort;
use std::time::Duration;

/// The open reader.
pub type Reader = Pn532<Box<dyn SerialPort>>;
/// The open reader, configured for polling.
pub type Poller = Pn532Hsu<Box<dyn SerialPort>>;

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
pub fn resolve(explicit: Option<&str>) -> Result<String> {
    if explicit.is_some() {
        return choose(explicit, &[]);
    }
    let ports = ports::available().context("cannot list serial ports")?;
    choose(None, &ports)
}

/// Firmware-reply timeout when reopening a lost reader. With two tries a
/// silent PN532 costs ~0.8 s, so Ctrl-C is honoured within about a second
/// while reconnecting (the first open keeps the default, longer timeout).
pub const REOPEN_TIMEOUT: Duration = Duration::from_millis(400);

/// Picks and opens the reader port and configures the PN532 for polling.
pub fn open_poller(explicit: Option<&str>) -> Result<(String, Poller)> {
    open_poller_with(explicit, None)
}

/// [`open_poller`] for reconnecting: gives up on a silent PN532 sooner.
pub fn reopen_poller(explicit: Option<&str>) -> Result<(String, Poller)> {
    open_poller_with(explicit, Some(REOPEN_TIMEOUT))
}

fn open_poller_with(explicit: Option<&str>, timeout: Option<Duration>) -> Result<(String, Poller)> {
    let name = resolve(explicit)?;
    let (reader, fw) = open_with(&name, false, timeout)?;
    tracing::info!(port = %name, "PN532 firmware {}.{}", fw.ver, fw.rev);
    let poller = Pn532Hsu::init(reader).map_err(|e| match e {
        Pn532Error::Io(io) => {
            anyhow!("I/O error configuring the PN532 on {name}: {io}; was the adapter unplugged?")
        }
        e => anyhow!("cannot configure the PN532 on {name} ({e}); {NO_REPLY_HINT}"),
    })?;
    Ok((name, poller))
}

/// The error for a reader that stopped working mid-run.
pub fn lost(name: &str, e: Pn532Error) -> anyhow::Error {
    anyhow!("lost the reader on {name} ({e}); was the adapter unplugged?")
}

/// Opens `name`, wakes the PN532 and checks it answers `GetFirmwareVersion`.
/// `in_diagnose` drops hints that would just say "run diagnose".
pub fn open(name: &str, in_diagnose: bool) -> Result<(Reader, FirmwareVersion)> {
    open_with(name, in_diagnose, None)
}

/// [`open`] with a firmware-reply timeout other than the driver default.
fn open_with(
    name: &str,
    in_diagnose: bool,
    timeout: Option<Duration>,
) -> Result<(Reader, FirmwareVersion)> {
    let mut reader = serial::open(name).map_err(|e| open_error(name, &e, in_diagnose))?;
    if let Some(t) = timeout {
        reader.set_timeout(t);
    }
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

fn open_error(name: &str, e: &serialport::Error, in_diagnose: bool) -> anyhow::Error {
    anyhow!(
        "cannot open {name}: {e}; {}",
        open_hint(e.kind(), in_diagnose)
    )
}

/// What to try when opening a port fails. `serialport` reports errors
/// differently per OS: on Windows a missing COM port and one held by another
/// program are both `NoDevice`; on Unix `NoDevice` means busy (EBUSY/flock).
fn open_hint(kind: serialport::ErrorKind, in_diagnose: bool) -> String {
    use serialport::ErrorKind;
    use std::io::ErrorKind as Io;
    let list = if in_diagnose {
        "see the serial ports listed above"
    } else {
        "run `mini-launcher diagnose` to list ports"
    };
    let hint = match kind {
        ErrorKind::NoDevice if cfg!(windows) => {
            return format!(
                "COM port not found or in use by another program: {list} \
                 (or check Device Manager for the number) and close other serial apps"
            )
        }
        ErrorKind::NoDevice => {
            "the port is busy: close other programs using it (serial monitors, Arduino IDE, \
             another mini-launcher)"
        }
        ErrorKind::Io(Io::NotFound) => return format!("no such port; {list}"),
        ErrorKind::Io(Io::PermissionDenied) if cfg!(target_os = "linux") => {
            "add yourself to the `dialout` group (then log out and in) \
             or install the udev rule granting `uaccess`"
        }
        ErrorKind::Io(Io::PermissionDenied) => "check that your user may access the port",
        _ => "check the port name and that the adapter is plugged in",
    };
    hint.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serialport::ErrorKind;
    use std::io::ErrorKind as Io;

    #[test]
    fn hints_match_the_os_error_mapping() {
        let no_device = open_hint(ErrorKind::NoDevice, false);
        if cfg!(windows) {
            assert!(no_device.contains("not found or in use"));
        } else {
            assert!(no_device.contains("busy"));
            assert!(!no_device.contains("dialout"));
        }
        let denied = open_hint(ErrorKind::Io(Io::PermissionDenied), false);
        assert_eq!(denied.contains("dialout"), cfg!(target_os = "linux"));
        assert!(open_hint(ErrorKind::Io(Io::NotFound), false).contains("diagnose"));
        assert!(!open_hint(ErrorKind::Io(Io::NotFound), true).contains("diagnose"));
    }
}
