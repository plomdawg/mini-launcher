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
}
