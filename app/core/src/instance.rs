//! Single instance: only one process may own the reader at a time.
//!
//! On Windows this is the named mutex `Local\mini-launcher` (per login
//! session). Other OSes have no lock yet, so every acquire succeeds.
//!
//! On Windows the tray app also listens on the auto-reset event
//! [`QUIT_EVENT`]: setup signals it to close the app before an upgrade.

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
/// Only opens the name, never creates it, so it cannot make a concurrent
/// [`acquire`] fail.
pub fn is_running() -> bool {
    NamedMutex::exists(NAME).unwrap_or(false)
}

/// Named event the tray app watches for quit requests (setup, `--quit`).
/// Must match `QuitEvent` in `installer/mini-launcher.iss`.
#[cfg(windows)]
pub const QUIT_EVENT: &str = r"Local\mini-launcher-quit";

/// The tray app's end of [`QUIT_EVENT`]; the event exists while it lives.
#[cfg(windows)]
pub struct QuitListener(crate::platform::NamedEvent);

#[cfg(windows)]
impl QuitListener {
    pub fn create() -> io::Result<Self> {
        crate::platform::NamedEvent::create(QUIT_EVENT).map(Self)
    }

    /// Whether a quit was requested since the last call.
    pub fn requested(&self) -> bool {
        self.0.take()
    }
}

/// Asks the running tray app to quit; `Ok(false)` if none is listening.
#[cfg(windows)]
pub fn request_quit() -> io::Result<bool> {
    crate::platform::NamedEvent::signal(QUIT_EVENT)
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
        assert!(!NamedMutex::exists(name).unwrap());
        assert!(acquire_named(name).unwrap().is_some());
    }

    #[test]
    fn exists_sees_a_held_name_without_taking_it() {
        let name = r"Local\mini-launcher-test-exists";
        assert!(!NamedMutex::exists(name).unwrap());
        let _held = acquire_named(name).unwrap().expect("acquire");
        assert_eq!(NamedMutex::exists(name).unwrap(), cfg!(windows));
    }

    #[cfg(windows)]
    #[test]
    fn quit_event_is_auto_reset_and_signalable_by_name() {
        use crate::platform::NamedEvent;
        let name = r"Local\mini-launcher-test-quit";
        assert!(!NamedEvent::signal(name).unwrap(), "nobody listens yet");
        let listener = NamedEvent::create(name).unwrap();
        assert!(!listener.take());
        assert!(NamedEvent::signal(name).unwrap());
        assert!(listener.take());
        assert!(!listener.take(), "taking the request resets it");
        drop(listener);
        assert!(!NamedEvent::signal(name).unwrap());
    }
}
