//! Keeps a reader alive across unplugs: a poll error drops it and it is
//! reopened with capped exponential backoff.
//!
//! [`Reconnector`] is a pure state machine; the caller passes the time in and
//! does the sleeping, so tests drive it with a fake [`Clock`].

use std::fmt::Display;
use std::time::{Duration, Instant};

use crate::reader::{Reader, Reading};

/// Time source for poll loops; tests substitute a fake.
pub trait Clock {
    fn now(&self) -> Instant;
    fn sleep(&mut self, d: Duration);
}

/// The real clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Capped exponential backoff between reopen attempts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backoff {
    first: Duration,
    max: Duration,
    next: Duration,
}

impl Backoff {
    /// Delay before the first reopen attempt.
    pub const FIRST: Duration = Duration::from_millis(500);
    /// Longest delay between attempts.
    pub const MAX: Duration = Duration::from_secs(5);

    pub fn new(first: Duration, max: Duration) -> Backoff {
        Backoff {
            first,
            max,
            next: first.min(max),
        }
    }

    /// The delay to wait now; each call doubles the next one, up to the cap.
    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = d.saturating_mul(2).min(self.max);
        d
    }

    /// Back to the first delay (after a successful reopen).
    pub fn reset(&mut self) {
        self.next = self.first.min(self.max);
    }
}

impl Default for Backoff {
    fn default() -> Backoff {
        Backoff::new(Backoff::FIRST, Backoff::MAX)
    }
}

/// Reader connection state, for status displays (e.g. a tray icon).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReaderState {
    /// Polling the reader on `port`.
    Connected { port: String },
    /// The reader on `last_port` went away; `attempts` reopens have failed.
    Reconnecting { last_port: String, attempts: u32 },
}

/// What one [`Reconnector::tick`] did.
#[derive(Debug)]
pub enum Tick<E> {
    /// The reader was polled.
    Reading(Option<Reading>),
    /// The reader failed and was dropped (port closed). Reported once per
    /// outage; reopening starts after the first backoff delay.
    Lost { port: String, error: E },
    /// The reader was reopened on `port` (possibly a different one).
    Reconnected { port: String },
    /// Disconnected, waiting for the next reopen attempt.
    Waiting,
}

/// A reader that is reopened by `open` whenever polling it fails. `open`
/// picks the port (re-running auto-detection if wanted), opens and
/// initializes the reader; its errors are logged at debug level and retried.
pub struct Reconnector<R, F> {
    open: F,
    reader: Option<R>,
    state: ReaderState,
    backoff: Backoff,
    retry_at: Option<Instant>,
}

impl<R, F, E> Reconnector<R, F>
where
    R: Reader,
    F: FnMut() -> Result<(String, R), E>,
    E: Display,
{
    /// Wraps a reader already open on `port`.
    pub fn new(port: String, reader: R, open: F, backoff: Backoff) -> Self {
        Reconnector {
            open,
            reader: Some(reader),
            state: ReaderState::Connected { port },
            backoff,
            retry_at: None,
        }
    }

    pub fn state(&self) -> &ReaderState {
        &self.state
    }

    /// Polls the reader, or tries to reopen it once the backoff delay has
    /// passed. Call once per poll interval.
    pub fn tick(&mut self, now: Instant) -> Tick<R::Error> {
        if let Some(reader) = &mut self.reader {
            return match reader.poll() {
                Ok(reading) => Tick::Reading(reading),
                Err(error) => {
                    self.reader = None;
                    let port = match &self.state {
                        ReaderState::Connected { port } => port.clone(),
                        ReaderState::Reconnecting { last_port, .. } => last_port.clone(),
                    };
                    self.state = ReaderState::Reconnecting {
                        last_port: port.clone(),
                        attempts: 0,
                    };
                    self.retry_at = Some(now + self.backoff.next_delay());
                    Tick::Lost { port, error }
                }
            };
        }
        if self.retry_at.is_some_and(|at| now < at) {
            return Tick::Waiting;
        }
        match (self.open)() {
            Ok((port, reader)) => {
                self.reader = Some(reader);
                self.backoff.reset();
                self.retry_at = None;
                self.state = ReaderState::Connected { port: port.clone() };
                Tick::Reconnected { port }
            }
            Err(e) => {
                let delay = self.backoff.next_delay();
                if let ReaderState::Reconnecting { attempts, .. } = &mut self.state {
                    *attempts = attempts.saturating_add(1);
                }
                tracing::debug!(error = %e, retry_in = ?delay, "reader not back yet");
                self.retry_at = Some(now + delay);
                Tick::Waiting
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::Payload;
    use crate::reader::Uid;
    use std::collections::VecDeque;
    use std::io;

    /// Replays polls; `Err` is a port I/O error. Runs dry → no tag.
    struct Fake(VecDeque<io::Result<Option<Reading>>>);

    impl Reader for Fake {
        type Error = io::Error;
        fn poll(&mut self) -> io::Result<Option<Reading>> {
            self.0.pop_front().unwrap_or(Ok(None))
        }
    }

    fn tag() -> io::Result<Option<Reading>> {
        Ok(Some((
            Uid::from_hex("04a1b2c3").unwrap(),
            Payload::Steam(440),
        )))
    }

    fn gone() -> io::Result<Option<Reading>> {
        Err(io::ErrorKind::BrokenPipe.into())
    }

    #[test]
    fn backoff_doubles_up_to_the_cap_and_resets() {
        let mut b = Backoff::default();
        let ms: Vec<_> = (0..6).map(|_| b.next_delay().as_millis()).collect();
        assert_eq!(ms, [500, 1000, 2000, 4000, 5000, 5000]);
        b.reset();
        assert_eq!(b.next_delay(), Backoff::FIRST);
    }

    #[test]
    fn reopens_with_backoff_after_the_reader_fails() {
        let mut opens = 0;
        let open = || {
            opens += 1;
            if opens < 3 {
                Err("no reader")
            } else {
                Ok(("COM9".to_owned(), Fake([tag()].into())))
            }
        };
        let first = Fake([tag(), gone()].into());
        let mut r = Reconnector::new("COM7".into(), first, open, Backoff::default());
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!(matches!(r.tick(at(0)), Tick::Reading(Some(_))));
        assert!(matches!(r.tick(at(200)), Tick::Lost { port, .. } if port == "COM7"));
        let reconnecting = |attempts| ReaderState::Reconnecting {
            last_port: "COM7".into(),
            attempts,
        };
        assert_eq!(r.state(), &reconnecting(0));
        // First attempt 500 ms after the loss, then 1 s, then 2 s later.
        assert!(matches!(r.tick(at(600)), Tick::Waiting));
        assert!(matches!(r.tick(at(700)), Tick::Waiting)); // attempt 1 fails
        assert_eq!(r.state(), &reconnecting(1));
        assert!(matches!(r.tick(at(1600)), Tick::Waiting));
        assert!(matches!(r.tick(at(1700)), Tick::Waiting)); // attempt 2 fails
        assert_eq!(r.state(), &reconnecting(2));
        assert!(matches!(r.tick(at(3600)), Tick::Waiting));
        assert!(matches!(r.tick(at(3700)), Tick::Reconnected { port } if port == "COM9"));
        assert_eq!(
            r.state(),
            &ReaderState::Connected {
                port: "COM9".into()
            }
        );
        assert!(matches!(r.tick(at(3900)), Tick::Reading(Some(_))));
        drop(r);
        assert_eq!(opens, 3);
    }

    #[test]
    fn backoff_restarts_after_a_successful_reopen() {
        let open = || Ok::<_, &str>(("COM7".to_owned(), Fake([gone()].into())));
        let mut r = Reconnector::new(
            "COM7".into(),
            Fake([gone()].into()),
            open,
            Backoff::default(),
        );
        let t0 = Instant::now();
        for _ in 0..5 {
            assert!(matches!(r.tick(t0), Tick::Lost { .. }));
            assert!(matches!(
                r.tick(t0 + Duration::from_millis(499)),
                Tick::Waiting
            ));
            let back = r.tick(t0 + Backoff::FIRST);
            assert!(matches!(back, Tick::Reconnected { .. }));
        }
    }
}
