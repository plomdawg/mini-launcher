//! Launching a game through the Battle.net client.
//!
//! `Battle.net.exe --exec="launch <code>"` only works when the client is
//! already running: a cold start goes through Battle.net's launcher, update
//! and login handoff and drops the `--exec`. So [`launch`] checks for a
//! running client first. If there is one, the `--exec` is sent once. If not,
//! it is sent anyway (that starts the client), and a background thread waits
//! for the client to show a window, gives it [`LOGIN_GRACE`] to log in, and
//! sends the `--exec` again. Nothing here ever blocks the caller.
//!
//! The decision logic sits behind [`Client`] so tests can drive it with a fake
//! clock. The code is always a validated [`Code`]; nothing from a tag reaches
//! the command line.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use super::Code;

/// The client's process name.
pub const CLIENT_PROCESS: &str = "Battle.net.exe";
/// How often a cold-started client is checked for a window.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long a cold-started client gets to show a window.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);
/// Extra time after the window appears for the login to finish.
pub const LOGIN_GRACE: Duration = Duration::from_secs(5);

/// Timings for [`resend_when_ready`]; [`Timings::DEFAULT`] in production.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    pub poll: Duration,
    pub timeout: Duration,
    pub grace: Duration,
}

impl Timings {
    pub const DEFAULT: Timings = Timings {
        poll: POLL_INTERVAL,
        timeout: STARTUP_TIMEOUT,
        grace: LOGIN_GRACE,
    };
}

/// The side effects of launching through Battle.net.
pub trait Client {
    /// Whether a `Battle.net.exe` process exists.
    fn is_running(&self) -> bool;
    /// Whether a `Battle.net.exe` process has a visible top-level window.
    fn has_window(&self) -> bool;
    /// Runs `Battle.net.exe --exec="launch <code>"`, without waiting for it.
    fn exec(&self, code: &Code) -> io::Result<()>;
    fn sleep(&self, d: Duration);
}

/// What [`start`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// The client was running; the `--exec` is all it takes.
    Running,
    /// The client was not running; the `--exec` started it but will likely
    /// be dropped, so it must be sent again once the client is up.
    Cold,
}

/// Sends the `--exec` once and reports whether the client was running.
pub fn start(client: &impl Client, code: &Code) -> io::Result<Start> {
    let start = if client.is_running() {
        tracing::info!("Battle.net is running; sending launch {code}");
        Start::Running
    } else {
        tracing::info!("Battle.net is not running; starting it with launch {code}");
        Start::Cold
    };
    client.exec(code)?;
    Ok(start)
}

/// What [`resend_when_ready`] did.
#[derive(Debug)]
pub enum Resend {
    /// The client came up and the `--exec` was sent again.
    Sent,
    /// The client showed no window within the timeout; nothing was sent.
    TimedOut,
    Failed(io::Error),
}

/// After a cold start: waits (polling every `t.poll`, up to `t.timeout`) for
/// the client to run with a visible window, waits `t.grace` more for the
/// login, then sends the `--exec` again.
pub fn resend_when_ready(client: &impl Client, code: &Code, t: Timings) -> Resend {
    let mut waited = Duration::ZERO;
    while !(client.is_running() && client.has_window()) {
        if waited >= t.timeout {
            tracing::warn!(
                "Battle.net showed no window within {}s; not resending launch {code}",
                t.timeout.as_secs()
            );
            return Resend::TimedOut;
        }
        client.sleep(t.poll);
        waited += t.poll;
    }
    tracing::info!(
        "Battle.net window is up after {}s; waiting {}s for login",
        waited.as_secs(),
        t.grace.as_secs()
    );
    client.sleep(t.grace);
    tracing::info!("sending launch {code} again");
    match client.exec(code) {
        Ok(()) => Resend::Sent,
        Err(e) => {
            tracing::warn!("could not resend launch {code} to Battle.net: {e}");
            Resend::Failed(e)
        }
    }
}

/// The real client: `exe` is `Battle.net.exe`.
struct System {
    exe: PathBuf,
}

impl Client for System {
    fn is_running(&self) -> bool {
        !crate::platform::process_ids(CLIENT_PROCESS).is_empty()
    }

    fn has_window(&self) -> bool {
        crate::platform::has_visible_window(&crate::platform::process_ids(CLIENT_PROCESS))
    }

    fn exec(&self, code: &Code) -> io::Result<()> {
        crate::launch::spawn_raw(&self.exe, &super::exec_arg(code))
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Launches `code` through `exe` (`Battle.net.exe`), retrying on a background
/// thread after a cold start. Returns once the first `--exec` is sent.
pub fn launch(exe: PathBuf, code: &Code) -> io::Result<()> {
    let client = System { exe };
    if start(&client, code)? == Start::Cold {
        let code = code.clone();
        std::thread::Builder::new()
            .name("battlenet-launch".into())
            .spawn(move || resend_when_ready(&client, &code, Timings::DEFAULT))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    /// A client on a fake clock: it starts running at `runs_at` and shows a
    /// window at `window_at` (seconds; `None` = never).
    struct Fake {
        now: Cell<Duration>,
        runs_at: Option<u64>,
        window_at: Option<u64>,
        execs: RefCell<Vec<(Duration, String)>>,
        fail_exec: bool,
    }

    impl Fake {
        fn new(runs_at: Option<u64>, window_at: Option<u64>) -> Fake {
            Fake {
                now: Cell::new(Duration::ZERO),
                runs_at,
                window_at,
                execs: RefCell::default(),
                fail_exec: false,
            }
        }

        fn reached(&self, at: Option<u64>) -> bool {
            at.is_some_and(|s| self.now.get() >= Duration::from_secs(s))
        }

        fn exec_times(&self) -> Vec<u64> {
            self.execs
                .borrow()
                .iter()
                .map(|(t, _)| t.as_secs())
                .collect()
        }
    }

    impl Client for Fake {
        fn is_running(&self) -> bool {
            self.reached(self.runs_at)
        }
        fn has_window(&self) -> bool {
            self.reached(self.window_at)
        }
        fn exec(&self, code: &Code) -> io::Result<()> {
            if self.fail_exec {
                return Err(io::Error::other("spawn failed"));
            }
            self.execs
                .borrow_mut()
                .push((self.now.get(), code.as_str().to_owned()));
            Ok(())
        }
        fn sleep(&self, d: Duration) {
            self.now.set(self.now.get() + d);
        }
    }

    fn code() -> Code {
        Code::new("WoWF").unwrap()
    }

    #[test]
    fn running_client_gets_one_exec() {
        let fake = Fake::new(Some(0), Some(0));
        assert_eq!(start(&fake, &code()).unwrap(), Start::Running);
        assert_eq!(*fake.execs.borrow(), vec![(Duration::ZERO, "WoWF".into())]);
    }

    #[test]
    fn cold_start_execs_then_resends_after_window_and_grace() {
        let fake = Fake::new(None, None);
        assert_eq!(start(&fake, &code()).unwrap(), Start::Cold);
        // The client starts at 3 s and shows its window at 12 s.
        let fake = Fake {
            runs_at: Some(3),
            window_at: Some(12),
            ..fake
        };
        assert!(matches!(
            resend_when_ready(&fake, &code(), Timings::DEFAULT),
            Resend::Sent
        ));
        assert_eq!(fake.exec_times(), vec![0, 12 + 5]);
    }

    #[test]
    fn window_needs_a_running_process() {
        // A stale "window" with no process never counts.
        let fake = Fake::new(None, Some(0));
        assert!(matches!(
            resend_when_ready(&fake, &code(), Timings::DEFAULT),
            Resend::TimedOut
        ));
        assert!(fake.execs.borrow().is_empty());
    }

    #[test]
    fn gives_up_after_the_timeout() {
        // Running but never shows a window.
        let fake = Fake::new(Some(0), None);
        assert!(matches!(
            resend_when_ready(&fake, &code(), Timings::DEFAULT),
            Resend::TimedOut
        ));
        assert!(fake.execs.borrow().is_empty());
        assert_eq!(fake.now.get(), STARTUP_TIMEOUT);
    }

    #[test]
    fn window_at_the_deadline_still_counts() {
        let fake = Fake::new(Some(0), Some(STARTUP_TIMEOUT.as_secs()));
        assert!(matches!(
            resend_when_ready(&fake, &code(), Timings::DEFAULT),
            Resend::Sent
        ));
        assert_eq!(fake.exec_times(), vec![STARTUP_TIMEOUT.as_secs() + 5]);
    }

    #[test]
    fn exec_errors_are_reported() {
        let mut fake = Fake::new(Some(0), Some(0));
        fake.fail_exec = true;
        assert!(start(&fake, &code()).is_err());
        assert!(matches!(
            resend_when_ready(&fake, &code(), Timings::DEFAULT),
            Resend::Failed(_)
        ));
    }
}
