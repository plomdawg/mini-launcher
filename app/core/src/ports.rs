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
        match self {
            SelectError::NoReader(ports) => {
                let (shown, hidden) = visible(ports, false);
                if shown.is_empty() {
                    write!(f, "no USB serial ports found")?;
                } else {
                    writeln!(f, "no CH340 reader adapter found among these ports:")?;
                    for p in shown {
                        writeln!(f, "  {p}")?;
                    }
                }
                if hidden > 0 {
                    write!(
                        f,
                        "\n{}",
                        hidden_note(hidden, "`mini-launcher diagnose --all`")
                    )?;
                }
                write!(
                    f,
                    "\nis the reader plugged in and its USB-serial driver installed? \
                     If your adapter is not a CH340 (e.g. a CP2102), pass --port <name>"
                )
            }
            SelectError::Ambiguous(ports) => {
                writeln!(f, "more than one CH340 adapter found:")?;
                for p in ports {
                    writeln!(f, "  {p}")?;
                }
                write!(f, "pick one with --port <name>")
            }
        }
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

/// Whether `name` looks like a serial port this OS can open: `COM1`..`COM999`
/// (any case, optionally `\\.\COMn`) on Windows, an absolute `/dev/...` path
/// elsewhere. Catches typos before they are stored for later (e.g. autostart).
pub fn is_valid_port_name(name: &str) -> bool {
    is_valid_port_name_for(name, cfg!(windows))
}

fn is_valid_port_name_for(name: &str, windows: bool) -> bool {
    if windows {
        let com = name.strip_prefix(r"\\.\").unwrap_or(name);
        let digits = match com.get(..3) {
            Some(p) if p.eq_ignore_ascii_case("COM") => &com[3..],
            _ => return false,
        };
        (1..=3).contains(&digits.len())
            && !digits.starts_with('0')
            && digits.bytes().all(|b| b.is_ascii_digit())
    } else {
        name.strip_prefix("/dev/").is_some_and(|rest| {
            !rest.is_empty()
                && !rest.ends_with('/')
                && !rest.chars().any(|c| c.is_control() || c.is_whitespace())
                && rest.split('/').all(|part| !part.is_empty() && part != "..")
        })
    }
}

/// Ports worth showing: USB ones only, unless `all`. Also returns how many
/// non-USB ports (legacy `/dev/ttyS*`, motherboard `COM1`, ...) were hidden.
pub fn visible(ports: &[PortInfo], all: bool) -> (Vec<&PortInfo>, usize) {
    let shown: Vec<&PortInfo> = ports.iter().filter(|p| all || p.usb.is_some()).collect();
    let hidden = ports.len() - shown.len();
    (shown, hidden)
}

/// "(N non-USB port(s) hidden; `how` shows them)".
pub fn hidden_note(hidden: usize, how: &str) -> String {
    let s = if hidden == 1 { "" } else { "s" };
    format!("({hidden} non-USB port{s} hidden; {how} shows them)")
}

/// Sort key so `COM3 < COM10` and `ttyS2 < ttyS10`: the name with its
/// trailing digits split off as a number.
fn natural_key(name: &str) -> (&str, Option<u64>, &str) {
    let prefix = name.trim_end_matches(|c: char| c.is_ascii_digit());
    (prefix, name[prefix.len()..].parse().ok(), name)
}

/// Sorts ports by name, numerically on any trailing number.
pub fn sort_ports(ports: &mut [PortInfo]) {
    ports.sort_by(|a, b| natural_key(&a.name).cmp(&natural_key(&b.name)));
}

/// Lists the serial ports the OS knows about, naturally sorted by name.
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
    sort_ports(&mut ports);
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
    fn port_names_are_validated_per_os() {
        for ok in ["COM1", "COM7", "com12", "Com999", r"\\.\COM10", r"\\.\com3"] {
            assert!(is_valid_port_name_for(ok, true), "{ok:?}");
        }
        for bad in [
            "",
            "COM",
            "COM0",
            "COM07",
            "COM1000",
            "COM7 ",
            " COM7",
            "COM-1",
            "-x",
            "COM7x",
            "LPT1",
            "/dev/ttyUSB0",
            r"\\.\",
            r"\\?\COM7",
            "ÇOM7",
        ] {
            assert!(!is_valid_port_name_for(bad, true), "{bad:?}");
        }
        for ok in [
            "/dev/ttyUSB0",
            "/dev/ttyACM1",
            "/dev/serial/by-id/usb-1a86_USB_Serial-if00",
        ] {
            assert!(is_valid_port_name_for(ok, false), "{ok:?}");
        }
        for bad in [
            "",
            "COM7",
            "ttyUSB0",
            "/dev/",
            "/dev",
            "/dev/tty USB0",
            "/dev/../etc/passwd",
            "/dev//ttyUSB0",
            "/dev/ttyUSB0/",
            "/dev/tty\nUSB0",
            "/tmp/ttyUSB0",
        ] {
            assert!(!is_valid_port_name_for(bad, false), "{bad:?}");
        }
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
        assert!(!msg.contains("COM1"), "non-USB ports are hidden: {msg}");
        assert!(msg.contains("(1 non-USB port hidden;"), "{msg}");
        assert!(msg.contains("CP2102), pass --port"), "{msg}");
    }

    #[test]
    fn no_ports_at_all() {
        let err = select_port(None, &[]).unwrap_err();
        assert_eq!(err, SelectError::NoReader(vec![]));
        let msg = err.to_string();
        assert!(msg.contains("no USB serial ports found"), "{msg}");
        assert!(!msg.contains("hidden"), "{msg}");
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

    #[test]
    fn visible_hides_non_usb_unless_all() {
        let ports = others();
        let (shown, hidden) = visible(&ports, false);
        assert_eq!(shown, vec![&ports[1], &ports[2]]);
        assert_eq!(hidden, 1);
        let (shown, hidden) = visible(&ports, true);
        assert_eq!(shown.len(), 3);
        assert_eq!(hidden, 0);
        assert_eq!(
            hidden_note(32, "--all"),
            "(32 non-USB ports hidden; --all shows them)"
        );
    }

    #[test]
    fn natural_sort() {
        let mut ports: Vec<PortInfo> = [
            "COM10",
            "COM3",
            "/dev/ttyS10",
            "/dev/ttyS2",
            "/dev/ttyACM0",
            "COM",
        ]
        .into_iter()
        .map(plain)
        .collect();
        sort_ports(&mut ports);
        let names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "/dev/ttyACM0",
                "/dev/ttyS2",
                "/dev/ttyS10",
                "COM",
                "COM3",
                "COM10"
            ]
        );
    }
}
