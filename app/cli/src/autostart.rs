//! `mini-launcher autostart enable|disable|status`: start `run` at login.

use std::io::Write;

use anyhow::{Context, Result};
use mini_launcher_core::autostart::{self, Status};

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
