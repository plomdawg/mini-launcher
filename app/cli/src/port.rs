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
    let mut last = None;
    // The first command after power-up is sometimes lost; try twice.
    for attempt in 1..=2 {
        let result = reader.wakeup().and_then(|()| reader.get_firmware_version());
        match result {
            Ok(fw) => return Ok((reader, fw)),
            Err(e) => {
                tracing::debug!(attempt, error = %e, "no firmware reply");
                last = Some(e);
            }
        }
    }
    let e = last.expect("loop ran");
    Err(match e {
        Pn532Error::Timeout | Pn532Error::NoAck => {
            anyhow!("no reply from the PN532 on {name} ({e}); {NO_REPLY_HINT}")
        }
        Pn532Error::Io(io) => anyhow!("I/O error on {name}: {io}; was the adapter unplugged?"),
        other => anyhow!("garbled reply from the PN532 on {name} ({other}); {NO_REPLY_HINT}"),
    })
}

fn open_error(name: &str, e: &serialport::Error) -> anyhow::Error {
    use serialport::ErrorKind;
    use std::io::ErrorKind as Io;
    let hint = match e.kind() {
        ErrorKind::Io(Io::NotFound) => {
            "no such port; run `nfc-launcher diagnose` to list ports".to_owned()
        }
        ErrorKind::NoDevice | ErrorKind::Io(Io::PermissionDenied) => {
            let mut h = "the port may be busy: close other programs using it (serial \
                         monitors, Arduino IDE, another nfc-launcher)"
                .to_owned();
            if cfg!(target_os = "linux") {
                h.push_str(
                    ", or you lack permission: add yourself to the `dialout` group \
                     (then log out and in) or install the udev rule granting `uaccess`",
                );
            }
            h
        }
        _ => "check the port name and that the adapter is plugged in".to_owned(),
    };
    anyhow!("cannot open {name}: {e}; {hint}")
}
