//! Serial port discovery and reader port selection.
//!
//! [`select_port`] is a pure function over a list of [`PortInfo`], so the
//! auto-detect rules are testable without hardware. [`available`] is the only
//! part that touches the OS.

use std::fmt;

/// USB vendor/product ID of the CH340 USB-UART bridge shipped with the reader.
pub const CH340: (u16, u16) = (0x1A86, 0x7523);

/// A serial port as seen by the OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// Name to open, e.g. `COM7` or `/dev/ttyUSB0`.
    pub name: String,
    /// USB details, if the port is a USB device.
    pub usb: Option<UsbInfo>,
}

/// USB details of a serial port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbInfo {
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
}

impl PortInfo {
    /// Whether this looks like the reader's USB-UART adapter (a CH340).
    pub fn is_likely_reader(&self) -> bool {
        self.usb.as_ref().is_some_and(|u| (u.vid, u.pid) == CH340)
    }
}

impl fmt::Display for PortInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        match &self.usb {
            Some(u) => {
                write!(f, "  USB {:04X}:{:04X}", u.vid, u.pid)?;
                let desc: Vec<&str> = [&u.manufacturer, &u.product]
                    .into_iter()
                    .flatten()
                    .map(String::as_str)
                    .collect();
                if !desc.is_empty() {
                    write!(f, "  {}", desc.join(" / "))?;
                }
            }
            None => write!(f, "  (not USB)")?,
        }
        if self.is_likely_reader() {
            write!(f, "  <- likely reader (CH340)")?;
        }
        Ok(())
    }
}

/// Why no port could be chosen automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    /// No CH340 adapter found. Holds every port seen.
    NoReader(Vec<PortInfo>),
    /// More than one CH340 adapter found. Holds the candidates.
    Ambiguous(Vec<PortInfo>),
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (msg, ports) = match self {
            SelectError::NoReader(p) if p.is_empty() => {
                return write!(
                    f,
                    "no serial ports found; is the reader plugged in and its \
                     USB-serial driver installed?"
                );
            }
            SelectError::NoReader(p) => ("no CH340 reader adapter found among these ports", p),
            SelectError::Ambiguous(p) => ("more than one CH340 adapter found", p),
        };
        writeln!(f, "{msg}:")?;
        for p in ports {
            writeln!(f, "  {p}")?;
        }
        write!(f, "pick one with --port <name>")
    }
}

impl std::error::Error for SelectError {}

/// Chooses the reader port: an explicit name always wins; otherwise exactly
/// one CH340 adapter must be present.
pub fn select_port(explicit: Option<&str>, ports: &[PortInfo]) -> Result<String, SelectError> {
    if let Some(name) = explicit {
        return Ok(name.to_owned());
    }
    let readers: Vec<&PortInfo> = ports.iter().filter(|p| p.is_likely_reader()).collect();
    match readers[..] {
        [one] => Ok(one.name.clone()),
        [] => Err(SelectError::NoReader(ports.to_vec())),
        _ => Err(SelectError::Ambiguous(
            readers.into_iter().cloned().collect(),
        )),
    }
}

/// Lists the serial ports the OS knows about, sorted by name.
pub fn available() -> serialport::Result<Vec<PortInfo>> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()?
        .into_iter()
        .map(|p| PortInfo {
            name: p.port_name,
            usb: match p.port_type {
                serialport::SerialPortType::UsbPort(u) => Some(UsbInfo {
                    vid: u.vid,
                    pid: u.pid,
                    manufacturer: u.manufacturer,
                    product: u.product,
                }),
                _ => None,
            },
        })
        .collect();
    ports.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usb(name: &str, vid: u16, pid: u16) -> PortInfo {
        PortInfo {
            name: name.into(),
            usb: Some(UsbInfo {
                vid,
                pid,
                manufacturer: None,
                product: None,
            }),
        }
    }

    fn plain(name: &str) -> PortInfo {
        PortInfo {
            name: name.into(),
            usb: None,
        }
    }

    fn ch340(name: &str) -> PortInfo {
        usb(name, CH340.0, CH340.1)
    }

    /// CP2102 and FTDI: other USB-serial chips, not auto-selected.
    fn others() -> Vec<PortInfo> {
        vec![
            plain("COM1"),
            usb("COM3", 0x10C4, 0xEA60),
            usb("/dev/ttyUSB1", 0x0403, 0x6001),
        ]
    }

    #[test]
    fn explicit_port_wins() {
        let ports = vec![ch340("COM4"), ch340("COM5")];
        assert_eq!(select_port(Some("COM9"), &ports).unwrap(), "COM9");
        assert_eq!(select_port(Some("COM9"), &[]).unwrap(), "COM9");
    }

    #[test]
    fn single_ch340_is_chosen_among_others() {
        let mut ports = others();
        ports.push(ch340("COM7"));
        assert_eq!(select_port(None, &ports).unwrap(), "COM7");
    }

    #[test]
    fn no_ch340_lists_all_ports() {
        let err = select_port(None, &others()).unwrap_err();
        assert_eq!(err, SelectError::NoReader(others()));
        let msg = err.to_string();
        assert!(msg.contains("COM3  USB 10C4:EA60"), "{msg}");
        assert!(msg.contains("COM1  (not USB)"), "{msg}");
        assert!(msg.contains("--port"), "{msg}");
    }

    #[test]
    fn no_ports_at_all() {
        let err = select_port(None, &[]).unwrap_err();
        assert_eq!(err, SelectError::NoReader(vec![]));
        assert!(err.to_string().contains("no serial ports found"));
    }

    #[test]
    fn two_ch340_is_ambiguous() {
        let mut ports = others();
        ports.extend([ch340("/dev/ttyUSB0"), ch340("/dev/ttyUSB2")]);
        let err = select_port(None, &ports).unwrap_err();
        assert_eq!(
            err,
            SelectError::Ambiguous(vec![ch340("/dev/ttyUSB0"), ch340("/dev/ttyUSB2")])
        );
        assert!(err.to_string().contains("likely reader"));
    }

    #[test]
    fn display_includes_usb_strings() {
        let mut p = ch340("COM7");
        p.usb.as_mut().unwrap().manufacturer = Some("wch.cn".into());
        p.usb.as_mut().unwrap().product = Some("USB-SERIAL CH340".into());
        assert_eq!(
            p.to_string(),
            "COM7  USB 1A86:7523  wch.cn / USB-SERIAL CH340  <- likely reader (CH340)"
        );
    }
}
