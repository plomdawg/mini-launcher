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
    /// The reader on `last_port` went away; `attempts` reopens have failed,
    /// the latest with `last_error` (`None` before the first attempt).
    Reconnecting {
        last_port: String,
        attempts: u32,
        last_error: Option<String>,
    },
}

/// When a reconnect counts as stuck: after this many failed reopens or this
/// long since the loss, whichever comes first.
pub const STUCK_AFTER_ATTEMPTS: u32 = 5;
/// See [`STUCK_AFTER_ATTEMPTS`].
pub const STUCK_AFTER: Duration = Duration::from_secs(30);

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
    /// Reopening keeps failing (see [`STUCK_AFTER_ATTEMPTS`]); `error` is
    /// the latest reason. Reported once per outage, so status output can say
    /// why instead of looking hung. Retrying continues.
    Stuck {
        port: String,
        attempts: u32,
        error: String,
    },
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
    /// When the current outage started.
    lost_at: Option<Instant>,
    /// Whether [`Tick::Stuck`] was reported for the current outage.
    stuck_reported: bool,
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
            lost_at: None,
            stuck_reported: false,
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
                        last_error: None,
                    };
                    self.retry_at = Some(now + self.backoff.next_delay());
                    self.lost_at = Some(now);
                    self.stuck_reported = false;
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
                self.lost_at = None;
                self.state = ReaderState::Connected { port: port.clone() };
                Tick::Reconnected { port }
            }
            Err(e) => {
                let delay = self.backoff.next_delay();
                self.retry_at = Some(now + delay);
                let ReaderState::Reconnecting {
                    last_port,
                    attempts,
                    last_error,
                } = &mut self.state
                else {
                    unreachable!("reopening only while disconnected");
                };
                *attempts = attempts.saturating_add(1);
                let error = e.to_string();
                tracing::debug!(
                    retry_in = ?delay,
                    "reopen attempt {attempts} on {last_port} failed: {error}"
                );
                *last_error = Some(error.clone());
                let down_for = self
                    .lost_at
                    .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
                if self.stuck_reported
                    || (*attempts < STUCK_AFTER_ATTEMPTS && down_for < STUCK_AFTER)
                {
                    return Tick::Waiting;
                }
                self.stuck_reported = true;
                Tick::Stuck {
                    port: last_port.clone(),
                    attempts: *attempts,
                    error,
                }
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
            last_error: (attempts > 0).then(|| "no reader".to_owned()),
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

    /// Reopens fail with `errors` in turn, then forever with the last one.
    fn failing(errors: &[&'static str]) -> impl FnMut() -> Result<(String, Fake), &'static str> {
        let mut errors = errors.to_vec();
        move || {
            Err(if errors.len() > 1 {
                errors.remove(0)
            } else {
                errors[0]
            })
        }
    }

    #[test]
    fn a_stuck_reconnect_is_reported_once_with_the_latest_reason() {
        let open = failing(&["port busy", "port busy", "no reply from the PN532"]);
        let first = Fake([gone()].into());
        let mut r = Reconnector::new("COM7".into(), first, open, Backoff::default());
        let t0 = Instant::now();
        let mut now = t0;
        assert!(matches!(r.tick(now), Tick::Lost { .. }));
        // Attempts at 0.5, 1.5, 3.5, 7.5 and 12.5 s; the fifth is stuck.
        let mut ticks = Vec::new();
        while now < t0 + Duration::from_secs(60) {
            now += Duration::from_millis(200);
            ticks.push((now - t0, r.tick(now)));
        }
        let stuck: Vec<_> = ticks
            .iter()
            .filter_map(|(at, t)| match t {
                Tick::Stuck {
                    port,
                    attempts,
                    error,
                } => Some((*at, port.as_str(), *attempts, error.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            stuck,
            [(
                Duration::from_millis(12_600),
                "COM7",
                5,
                "no reply from the PN532"
            )]
        );
        assert!(matches!(
            r.state(),
            ReaderState::Reconnecting { attempts, last_error: Some(e), .. }
                if *attempts > 5 && e == "no reply from the PN532"
        ));
    }

    #[test]
    fn a_slow_backoff_is_reported_as_stuck_after_30_s() {
        // A long first delay: the 30 s limit trips before 5 attempts.
        let backoff = Backoff::new(Duration::from_secs(20), Duration::from_secs(20));
        let mut r = Reconnector::new(
            "COM7".into(),
            Fake([gone()].into()),
            failing(&["port busy"]),
            backoff,
        );
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        assert!(matches!(r.tick(t0), Tick::Lost { .. }));
        assert!(matches!(r.tick(at(20)), Tick::Waiting)); // attempt 1, 20 s
        assert!(matches!(r.tick(at(40)), Tick::Stuck { attempts: 2, .. }));
        assert!(matches!(r.tick(at(60)), Tick::Waiting)); // only once
    }

    #[test]
    fn stuck_is_reported_again_after_the_next_outage() {
        let mut opens = 0;
        let open = || {
            opens += 1;
            match opens {
                6 => Ok(("COM7".to_owned(), Fake([gone()].into()))),
                _ => Err("port busy"),
            }
        };
        let mut r = Reconnector::new(
            "COM7".into(),
            Fake([gone()].into()),
            open,
            Backoff::new(Duration::ZERO, Duration::ZERO),
        );
        let t0 = Instant::now();
        let ticks: Vec<_> = (0..14).map(|_| r.tick(t0)).collect();
        let kinds: String = ticks
            .iter()
            .map(|t| match t {
                Tick::Lost { .. } => 'L',
                Tick::Stuck { .. } => 'S',
                Tick::Reconnected { .. } => 'R',
                Tick::Waiting => 'w',
                Tick::Reading(_) => '.',
            })
            .collect();
        // Five failed opens (the fifth stuck), a reconnect, then a new loss.
        assert_eq!(kinds, "LwwwwSRLwwwwSw");
    }
}
