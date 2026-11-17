//! Optional config folder override (e.g. a synced folder): a small pointer
//! file, `location.toml`, in the default config directory:
//!
//! ```toml
//! dir = 'D:\OneDrive\Mini Launcher'
//! ```
//!
//! `config.toml` (and its `.bak`) then live in that folder. The folder must be
//! an absolute path that exists.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The pointer file's name, inside the default config directory.
pub const POINTER_FILE: &str = "location.toml";
/// The config file's name, inside the effective config directory.
pub const CONFIG_FILE: &str = "config.toml";

/// Why the pointer file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum LocationError {
    #[error("{}: cannot read the file", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {message}", path.display())]
    Invalid { path: PathBuf, message: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pointer {
    dir: PathBuf,
}

/// The `config.toml` to use, given the default config directory: the one in
/// the folder named by `location.toml` if that file exists, else the default.
pub fn resolve(default_dir: &Path) -> Result<PathBuf, LocationError> {
    let path = default_dir.join(POINTER_FILE);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(default_dir.join(CONFIG_FILE));
        }
        Err(source) => return Err(LocationError::Io { path, source }),
        Ok(text) => text,
    };
    let invalid = |message: String| LocationError::Invalid {
        path: path.clone(),
        message,
    };
    let pointer: Pointer = toml::from_str(&text).map_err(|e| invalid(e.to_string()))?;
    let dir = pointer.dir;
    if !dir.is_absolute() {
        return Err(invalid(format!(
            "dir \"{}\" must be an absolute path",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(invalid(format!(
            "dir \"{}\" is not an existing folder",
            dir.display()
        )));
    }
    Ok(dir.join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mini-launcher-location-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pointer(default: &Path, dir: &Path) {
        let text = format!("dir = '{}'\n", dir.display());
        std::fs::write(default.join(POINTER_FILE), text).unwrap();
    }

    #[test]
    fn no_pointer_means_default_dir() {
        let default = temp("none");
        assert_eq!(resolve(&default).unwrap(), default.join(CONFIG_FILE));
    }

    #[test]
    fn pointer_to_existing_absolute_folder() {
        let (default, synced) = (temp("ok"), temp("ok-synced"));
        pointer(&default, &synced);
        assert_eq!(resolve(&default).unwrap(), synced.join(CONFIG_FILE));
    }

    #[test]
    fn rejects_bad_pointers() {
        let default = temp("bad");
        let missing = default.join("does-not-exist");
        let file = default.join("a-file");
        std::fs::write(&file, "").unwrap();
        let cases = [
            (
                format!("dir = '{}'", missing.display()),
                "not an existing folder",
            ),
            (
                format!("dir = '{}'", file.display()),
                "not an existing folder",
            ),
            ("dir = 'relative/folder'".into(), "must be an absolute path"),
            ("folder = '/x'".into(), "unknown field"),
            ("dir = ".into(), ""),
        ];
        for (text, want) in cases {
            std::fs::write(default.join(POINTER_FILE), &text).unwrap();
            let got = resolve(&default).unwrap_err().to_string();
            assert!(got.contains(want), "{text:?}: got {got:?}");
            assert!(got.contains(POINTER_FILE), "{got:?}");
        }
    }
}
