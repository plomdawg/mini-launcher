//! Single instance: only one process may own the reader at a time.
//!
//! On Windows this is the named mutex `Local\mini-launcher` (per login
//! session). Other OSes have no lock yet, so every acquire succeeds.

use std::io;

use crate::platform::NamedMutex;

/// Mutex name shared by `run` and the tray app.
pub const NAME: &str = r"Local\mini-launcher";

/// Held for as long as this process is the running instance.
pub struct InstanceGuard {
    _mutex: NamedMutex,
}

/// Claims the instance; `Ok(None)` if another process already holds it.
pub fn acquire() -> io::Result<Option<InstanceGuard>> {
    acquire_named(NAME)
}

fn acquire_named(name: &str) -> io::Result<Option<InstanceGuard>> {
    Ok(NamedMutex::acquire(name)?.map(|m| InstanceGuard { _mutex: m }))
}

/// Whether another process (or this one) currently holds the instance.
/// Probing never blocks a later [`acquire`].
pub fn is_running() -> bool {
    matches!(acquire(), Ok(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_first_is_dropped() {
        // A test-only name, so a real instance on the machine does not interfere.
        let name = r"Local\mini-launcher-test-instance";
        {
            let _first = acquire_named(name).unwrap().expect("first acquire");
            let second = acquire_named(name).unwrap();
            if cfg!(windows) {
                assert!(second.is_none(), "second acquire must fail on Windows");
            }
        }
        assert!(acquire_named(name).unwrap().is_some());
    }
}
