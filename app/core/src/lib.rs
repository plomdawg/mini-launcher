//! Core library for Mini Launcher.
//!
//! Reader access, NDEF parsing, tag payload handling, config, launching and
//! insert/remove debounce will live here. Everything above the reader is meant
//! to be testable without hardware.

pub mod config;
pub mod debounce;
pub mod launch;
pub mod ndef;
pub mod payload;
pub mod pn532;
pub mod ports;
pub mod reader;

/// Version of the core library, taken from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the core library version string.
pub fn version() -> &'static str {
    VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_package() {
        assert_eq!(version(), env!("CARGO_PKG_VERSION"));
        assert!(!version().is_empty());
    }
}
