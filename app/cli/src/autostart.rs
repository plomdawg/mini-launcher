//! `mini-launcher autostart enable|disable|status`: start `run` at login.

use std::io::Write;

use anyhow::{bail, Context, Result};
use mini_launcher_core::autostart::{self, Status};
use mini_launcher_core::ports;

pub enum Action {
    /// Optional fixed reader port to pass to `run`.
    Enable(Option<String>),
    Disable,
    Status,
}

pub fn run(out: &mut impl Write, action: Action) -> Result<()> {
    let exe = std::env::current_exe().context("cannot find this program's path")?;
    match action {
        Action::Enable(port) => {
            let mut args = vec!["run"];
            if let Some(p) = &port {
                // Fail now rather than at the next login.
                if !ports::is_valid_port_name(p) {
                    let want = if cfg!(windows) {
                        "COM<n>, e.g. COM7"
                    } else {
                        "a /dev/... path, e.g. /dev/ttyUSB0"
                    };
                    bail!("{p:?} is not a serial port name (expected {want})");
                }
                args.extend(["--port", p]);
            }
            let command = autostart::enable(&exe, &args)?;
            writeln!(out, "Start at login enabled: {command}")?;
        }
        Action::Disable => {
            autostart::disable()?;
            writeln!(out, "Start at login disabled.")?;
        }
        Action::Status => match autostart::status(&exe)? {
            Status::Disabled => writeln!(out, "Start at login: disabled")?,
            Status::Enabled { command, this_exe } => {
                writeln!(out, "Start at login: enabled\n  {command}")?;
                if !this_exe {
                    writeln!(
                        out,
                        "  note: this starts a different copy than {}; \
                         run `autostart enable` to point it here",
                        exe.display()
                    )?;
                }
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_rejects_a_bad_port_before_writing() {
        for bad in ["-x", "COM7 ", ""] {
            let err = run(&mut Vec::new(), Action::Enable(Some(bad.into()))).unwrap_err();
            assert!(err.to_string().contains("not a serial port name"), "{err}");
        }
    }
}
