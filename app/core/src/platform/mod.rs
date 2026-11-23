//! Thin OS layer. Windows code lives in `windows.rs`; every other OS gets the
//! stubs below until Linux support lands.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(not(windows))]
pub(crate) use other::*;

#[cfg(not(windows))]
mod other {
    use std::io;

    /// No cross-process lock here yet: every acquire succeeds.
    pub(crate) struct NamedMutex;

    impl NamedMutex {
        /// `Ok(None)` would mean another process holds `name`.
        pub(crate) fn acquire(_name: &str) -> io::Result<Option<Self>> {
            Ok(Some(Self))
        }

        /// Never held, since nothing is ever locked.
        pub(crate) fn exists(_name: &str) -> io::Result<bool> {
            Ok(false)
        }
    }

    /// `None`: start at login is not available on this OS.
    pub(crate) fn run_value(_name: &str) -> Option<io::Result<Option<String>>> {
        None
    }

    pub(crate) fn set_run_value(_name: &str, _value: &str) -> Option<io::Result<()>> {
        None
    }

    pub(crate) fn delete_run_value(_name: &str) -> Option<io::Result<()>> {
        None
    }

    /// No registry: Steam's default paths are probed by `steam` instead.
    pub(crate) fn steam_paths() -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    /// Battle.net is Windows-only.
    pub(crate) fn blizzard_uninstalls() -> Vec<crate::battlenet::Uninstall> {
        Vec::new()
    }

    pub(crate) fn program_files() -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    pub(crate) fn battlenet_dirs() -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    /// Process listing is only needed for Battle.net, which is Windows-only.
    pub(crate) fn process_ids(_exe_name: &str) -> Vec<u32> {
        Vec::new()
    }

    pub(crate) fn has_visible_window(_pids: &[u32]) -> bool {
        false
    }
}
