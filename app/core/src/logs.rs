//! Where the log files live.

use std::path::{Path, PathBuf};

/// The per-user log directory, if the OS has a local data dir: on Windows
/// `%LOCALAPPDATA%\mini-launcher\logs`, on Linux
/// `$XDG_DATA_HOME/mini-launcher/logs` (usually `~/.local/share/...`).
pub fn default_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "mini-launcher")
        .map(|dirs| dir_in(dirs.data_local_dir(), cfg!(windows)))
}

/// The log dir for a project's local data dir. On Windows `directories` puts
/// that in a `data` subfolder of the app folder; logs go next to it instead.
fn dir_in(data_local_dir: &Path, windows: bool) -> PathBuf {
    let base = match data_local_dir.parent() {
        Some(app) if windows && data_local_dir.ends_with("data") => app,
        _ => data_local_dir,
    };
    base.join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_logs_sit_beside_the_data_folder() {
        let local = Path::new("C:/Users/me/AppData/Local/mini-launcher/data");
        assert_eq!(
            dir_in(local, true),
            Path::new("C:/Users/me/AppData/Local/mini-launcher/logs")
        );
    }

    #[test]
    fn other_systems_put_logs_inside_the_data_dir() {
        let local = Path::new("/home/me/.local/share/mini-launcher");
        assert_eq!(
            dir_in(local, false),
            Path::new("/home/me/.local/share/mini-launcher/logs")
        );
    }

    #[test]
    fn default_dir_is_named_after_the_app() {
        // CI runners always have a home dir; skip gracefully elsewhere.
        if let Some(dir) = default_dir() {
            assert!(dir.ends_with("logs"));
            assert!(dir.iter().any(|c| c == "mini-launcher"));
        }
    }
}
