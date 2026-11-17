//! Optional config folder override (e.g. a synced folder): a small pointer
//! file, `location.toml`, in the default config directory:
//!
//! ```toml
//! dir = 'D:\OneDrive\Mini Launcher'
//! ```
//!
//! `config.toml` (and its `.bak`) then live in that folder. The folder must be
//! an absolute path that exists. A pointer that cannot be used (invalid, or
//! its folder missing, e.g. a drive not mounted yet) is never replaced by the
//! default folder: see [`ConfigLocation::OverrideUnavailable`].

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
    /// The config folder override cannot be used right now.
    #[error("config folder {}unavailable: {reason}", dir.as_ref().map(|d| format!("\"{}\" ", d.display())).unwrap_or_default())]
    Unavailable {
        dir: Option<PathBuf>,
        reason: String,
    },
}

/// Where the config lives: the effective config folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigLocation {
    /// No `location.toml`: the default config folder.
    Default(PathBuf),
    /// The existing folder `location.toml` names.
    Override(PathBuf),
    /// `location.toml` exists but cannot be used (yet). There is no config:
    /// `steam://` tags still work, `id:` tags and UID mappings do not, and
    /// every save fails, so nothing is written to a different folder.
    OverrideUnavailable {
        /// The folder named, if the pointer could be read.
        dir: Option<PathBuf>,
        reason: String,
    },
}

impl ConfigLocation {
    /// Reads `location.toml` in `default_dir`. Checking an override folder
    /// may block while a network share times out.
    pub fn resolve(default_dir: &Path) -> ConfigLocation {
        match read_pointer(default_dir) {
            Ok(None) => ConfigLocation::Default(default_dir.to_owned()),
            Ok(Some(dir)) if dir.is_dir() => ConfigLocation::Override(dir),
            Ok(Some(dir)) => ConfigLocation::OverrideUnavailable {
                dir: Some(dir),
                reason: "the folder does not exist or is not reachable (not mounted yet?)".into(),
            },
            Err(e) => ConfigLocation::OverrideUnavailable {
                dir: None,
                reason: e.to_string(),
            },
        }
    }

    /// The `config.toml` to load and watch, or why there is none.
    pub fn config_file(&self) -> Result<PathBuf, LocationError> {
        match self {
            ConfigLocation::Default(dir) | ConfigLocation::Override(dir) => {
                Ok(dir.join(CONFIG_FILE))
            }
            ConfigLocation::OverrideUnavailable { dir, reason } => {
                Err(LocationError::Unavailable {
                    dir: dir.clone(),
                    reason: reason.clone(),
                })
            }
        }
    }

    /// Like [`ConfigLocation::config_file`], but first checks again that an
    /// override folder still exists, so a save never recreates a folder on a
    /// drive that went away.
    pub fn writable_file(&self) -> Result<PathBuf, LocationError> {
        match self {
            ConfigLocation::Override(dir) if !dir.is_dir() => Err(LocationError::Unavailable {
                dir: Some(dir.clone()),
                reason: "the folder is gone (drive removed or share offline?)".into(),
            }),
            _ => self.config_file(),
        }
    }

    pub fn is_available(&self) -> bool {
        !matches!(self, ConfigLocation::OverrideUnavailable { .. })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pointer {
    dir: PathBuf,
}

/// The absolute folder `location.toml` names, or `None` without one.
fn read_pointer(default_dir: &Path) -> Result<Option<PathBuf>, LocationError> {
    let path = default_dir.join(POINTER_FILE);
    let text = match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
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
    Ok(Some(dir))
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

    fn reason(loc: &ConfigLocation) -> String {
        loc.config_file().unwrap_err().to_string()
    }

    #[test]
    fn no_pointer_means_default_dir() {
        let default = temp("none");
        let loc = ConfigLocation::resolve(&default);
        assert_eq!(loc, ConfigLocation::Default(default.clone()));
        assert_eq!(loc.writable_file().unwrap(), default.join(CONFIG_FILE));
    }

    #[test]
    fn pointer_to_existing_absolute_folder() {
        let (default, synced) = (temp("ok"), temp("ok-synced"));
        pointer(&default, &synced);
        let loc = ConfigLocation::resolve(&default);
        assert_eq!(loc, ConfigLocation::Override(synced.clone()));
        assert_eq!(loc.config_file().unwrap(), synced.join(CONFIG_FILE));
        // The folder goes away after resolving: saves refuse to recreate it.
        std::fs::remove_dir_all(&synced).unwrap();
        let err = loc.writable_file().unwrap_err().to_string();
        assert!(err.contains("unavailable: the folder is gone"), "{err}");
    }

    #[test]
    fn missing_folder_is_unavailable_not_the_default() {
        let default = temp("missing");
        let missing = default.join("not-mounted");
        pointer(&default, &missing);
        let loc = ConfigLocation::resolve(&default);
        assert!(!loc.is_available());
        assert!(loc.writable_file().is_err());
        let msg = reason(&loc);
        assert!(
            msg.starts_with(&format!(
                "config folder \"{}\" unavailable",
                missing.display()
            )),
            "{msg}"
        );
        // It comes back (e.g. the drive is mounted): resolving again finds it.
        std::fs::create_dir(&missing).unwrap();
        assert_eq!(
            ConfigLocation::resolve(&default),
            ConfigLocation::Override(missing)
        );
    }

    #[test]
    fn rejects_bad_pointers() {
        let default = temp("bad");
        let missing = default.join("does-not-exist");
        let file = default.join("a-file");
        std::fs::write(&file, "").unwrap();
        let cases = [
            (format!("dir = '{}'", missing.display()), "does not exist"),
            (format!("dir = '{}'", file.display()), "does not exist"),
            ("dir = 'relative/folder'".into(), "must be an absolute path"),
            ("folder = '/x'".into(), "unknown field"),
            ("dir = ".into(), POINTER_FILE),
        ];
        for (text, want) in cases {
            std::fs::write(default.join(POINTER_FILE), &text).unwrap();
            let loc = ConfigLocation::resolve(&default);
            let got = reason(&loc);
            assert!(got.contains(want), "{text:?}: got {got:?}");
            assert!(got.starts_with("config folder "), "{got:?}");
        }
    }
}
