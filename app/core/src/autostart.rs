//! Start at login: a per-user `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
//! value that starts this exe with `run`. No admin rights, no shell.
//!
//! The command-line building/parsing here is pure and tested on every OS; only
//! the registry access is Windows-specific.

use std::io;
use std::path::Path;

use crate::platform;

/// Name of the value under the `Run` key.
pub const VALUE_NAME: &str = "Mini Launcher";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("start at login is not supported on this OS yet")]
    Unsupported,
    #[error("cannot start {0} at login: the path contains a double quote or is not valid Unicode")]
    BadPath(String),
    #[error("cannot access the Run registry key: {0}")]
    Os(#[from] io::Error),
}

/// What the `Run` value currently says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Disabled,
    /// `command` is the raw value; `this_exe` tells whether it starts the exe
    /// asking (a moved or older copy may have registered itself).
    Enabled {
        command: String,
        this_exe: bool,
    },
}

/// Quotes one argument so `CommandLineToArgvW` / the MSVC runtime read it back
/// unchanged.
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
        return arg.to_owned();
    }
    let mut out = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // Double the backslashes already written, then escape the quote.
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
        out.push(c);
    }
    // Backslashes before the closing quote must be doubled too.
    out.extend(std::iter::repeat_n('\\', backslashes));
    out.push('"');
    out
}

/// The `Run` value: the quoted absolute exe path followed by quoted `args`.
/// The program name is always quoted and never escaped (Windows reads it up to
/// the next quote), so a path containing `"` is rejected.
pub fn command_line(exe: &Path, args: &[&str]) -> Result<String, Error> {
    let exe = exe
        .to_str()
        .filter(|s| !s.contains('"'))
        .ok_or_else(|| Error::BadPath(exe.display().to_string()))?;
    let mut line = format!("\"{exe}\"");
    for arg in args {
        line.push(' ');
        line.push_str(&quote_arg(arg));
    }
    Ok(line)
}

/// The program path of a `Run` value: up to the closing quote if quoted,
/// otherwise up to the first space.
pub fn program(command: &str) -> &str {
    let command = command.trim_start();
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split([' ', '\t']).next().unwrap_or(command),
    }
}

/// Registers `exe` (normally `std::env::current_exe()`) to start with `args`.
pub fn enable(exe: &Path, args: &[&str]) -> Result<String, Error> {
    let command = command_line(exe, args)?;
    platform::set_run_value(VALUE_NAME, &command).ok_or(Error::Unsupported)??;
    Ok(command)
}

/// Removes the `Run` value; succeeds if it was already absent.
pub fn disable() -> Result<(), Error> {
    Ok(platform::delete_run_value(VALUE_NAME).ok_or(Error::Unsupported)??)
}

/// Reads the `Run` value and compares it with `exe`.
pub fn status(exe: &Path) -> Result<Status, Error> {
    let value = platform::run_value(VALUE_NAME).ok_or(Error::Unsupported)??;
    Ok(match value {
        None => Status::Disabled,
        Some(command) => Status::Enabled {
            // Windows paths are case-insensitive.
            this_exe: exe
                .to_str()
                .is_some_and(|e| program(&command).eq_ignore_ascii_case(e)),
            command,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_arg_follows_msvc_rules() {
        let cases = [
            ("run", "run"),
            ("COM7", "COM7"),
            (r"C:\x\", r"C:\x\"),
            ("", r#""""#),
            ("a b", r#""a b""#),
            (r"C:\My Games\", r#""C:\My Games\\""#),
            (r#"say "hi""#, r#""say \"hi\"""#),
            (r#"a\"b"#, r#""a\\\"b""#),
            ("tab\there", "\"tab\there\""),
        ];
        for (arg, want) in cases {
            assert_eq!(quote_arg(arg), want, "{arg:?}");
        }
    }

    #[test]
    fn command_line_quotes_exe_and_args() {
        let exe = Path::new(r"C:\Program Files\Mini Launcher\mini-launcher.exe");
        assert_eq!(
            command_line(exe, &["run"]).unwrap(),
            r#""C:\Program Files\Mini Launcher\mini-launcher.exe" run"#
        );
        assert_eq!(
            command_line(exe, &["run", "--port", "COM 7"]).unwrap(),
            r#""C:\Program Files\Mini Launcher\mini-launcher.exe" run --port "COM 7""#
        );
        assert!(matches!(
            command_line(Path::new(r#"C:\a"b.exe"#), &["run"]),
            Err(Error::BadPath(_))
        ));
    }

    #[test]
    fn program_round_trips_and_handles_unquoted_values() {
        let exe = r"C:\Users\Me\AppData\Local\Mini Launcher\mini-launcher.exe";
        let line = command_line(Path::new(exe), &["run", "-p", "COM7"]).unwrap();
        assert_eq!(program(&line), exe);
        assert_eq!(program(r"C:\tools\ml.exe run"), r"C:\tools\ml.exe");
        assert_eq!(program(r#""C:\no close quote"#), r"C:\no close quote");
        assert_eq!(program(""), "");
    }

    #[cfg(not(windows))]
    #[test]
    fn unsupported_off_windows() {
        let exe = Path::new("/usr/bin/mini-launcher");
        assert!(matches!(enable(exe, &["run"]), Err(Error::Unsupported)));
        assert!(matches!(disable(), Err(Error::Unsupported)));
        assert!(matches!(status(exe), Err(Error::Unsupported)));
    }
}
