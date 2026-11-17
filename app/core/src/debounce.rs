//! Insert/remove debounce: turns raw per-poll readings into tag events.
//!
//! Pure state machine; the caller passes the poll time in, so tests drive it
//! with a fake clock. Removal needs several consecutive missed polls, and a
//! tag re-seated soon after removal does not launch again.

use std::time::{Duration, Instant};

use crate::payload::Payload;
use crate::reader::{Reading, Uid};

/// An event derived from the raw readings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A tag arrived. This is the only event that should trigger a launch.
    Inserted { uid: Uid, payload: Payload },
    /// The tag that was just removed came back within the re-seat window.
    /// Not a new launch.
    Reseated { uid: Uid },
    /// The tag has been gone for the configured number of polls.
    Removed { uid: Uid },
}

/// Debounce tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebounceConfig {
    /// Consecutive missed polls before a tag counts as removed (at least 1).
    pub miss_threshold: u32,
    /// A tag re-seated within this long of its last sighting is not relaunched.
    pub reseat_window: Duration,
}

impl Default for DebounceConfig {
    /// About 1 s of misses at a 200 ms poll interval; 5 s re-seat window.
    /// Callers polling at another rate should scale `miss_threshold`.
    fn default() -> Self {
        DebounceConfig {
            miss_threshold: 5,
            reseat_window: Duration::from_secs(5),
        }
    }
}

/// The debounce state machine. Feed it one reading per poll.
#[derive(Debug, Clone)]
pub struct Debouncer {
    config: DebounceConfig,
    /// The tag currently considered present.
    seated: Option<Uid>,
    /// Consecutive polls without the seated tag.
    misses: u32,
    /// The most recently seen tag and when; drives the re-seat window.
    last_seen: Option<(Uid, Instant)>,
}

impl Debouncer {
    pub fn new(config: DebounceConfig) -> Debouncer {
        Debouncer {
            config,
            seated: None,
            misses: 0,
            last_seen: None,
        }
    }

    /// Processes one poll result taken at `now` and returns the events it
    /// causes (a swap to a different tag yields `Removed` then `Inserted`).
    pub fn update(&mut self, reading: Option<Reading>, now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        match reading {
            Some((uid, payload)) => {
                if self.seated != Some(uid) {
                    if let Some(old) = self.seated.replace(uid) {
                        events.push(Event::Removed { uid: old });
                    }
                    let reseat = matches!(self.last_seen, Some((prev, seen))
                        if prev == uid
                            && now.saturating_duration_since(seen) <= self.config.reseat_window);
                    events.push(if reseat {
                        Event::Reseated { uid }
                    } else {
                        Event::Inserted { uid, payload }
                    });
                }
                self.misses = 0;
                self.last_seen = Some((uid, now));
            }
            None => {
                if let Some(uid) = self.seated {
                    self.misses += 1;
                    if self.misses >= self.config.miss_threshold.max(1) {
                        self.seated = None;
                        events.push(Event::Removed { uid });
                    }
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: Duration = Duration::from_millis(200);

    /// Fake clock: advances by one poll interval per `poll`.
    struct Harness {
        debouncer: Debouncer,
        now: Instant,
    }

    impl Harness {
        fn new() -> Harness {
            let config = DebounceConfig {
                miss_threshold: 5,
                reseat_window: Duration::from_secs(3),
            };
            Harness {
                debouncer: Debouncer::new(config),
                now: Instant::now(),
            }
        }

        fn poll(&mut self, reading: Option<Reading>) -> Vec<Event> {
            self.now += TICK;
            self.debouncer.update(reading, self.now)
        }

        /// Polls `n` times with the same reading and collects all events.
        fn polls(&mut self, n: usize, reading: Option<Reading>) -> Vec<Event> {
            (0..n).flat_map(|_| self.poll(reading.clone())).collect()
        }
    }

    fn tag(hex: &str) -> Option<Reading> {
        Some((Uid::from_hex(hex).unwrap(), Payload::Steam(440)))
    }

    fn inserted(hex: &str) -> Event {
        Event::Inserted {
            uid: Uid::from_hex(hex).unwrap(),
            payload: Payload::Steam(440),
        }
    }

    fn removed(hex: &str) -> Event {
        Event::Removed {
            uid: Uid::from_hex(hex).unwrap(),
        }
    }

    const A: &str = "04a1b2c3d4e5f6";
    const B: &str = "deadbeef";

    #[test]
    fn insert_then_steady() {
        let mut h = Harness::new();
        assert_eq!(h.polls(3, None), vec![]);
        assert_eq!(h.poll(tag(A)), vec![inserted(A)]);
        assert_eq!(h.polls(20, tag(A)), vec![]);
    }

    #[test]
    fn flicker_is_ignored() {
        let mut h = Harness::new();
        h.poll(tag(A));
        for misses in [1, 2, 4] {
            assert_eq!(h.polls(misses, None), vec![], "{misses} misses");
            assert_eq!(h.poll(tag(A)), vec![], "back after {misses} misses");
        }
    }

    #[test]
    fn removal_after_threshold_misses() {
        let mut h = Harness::new();
        h.poll(tag(A));
        assert_eq!(h.polls(4, None), vec![]);
        assert_eq!(h.poll(None), vec![removed(A)]);
        assert_eq!(h.polls(10, None), vec![]);
    }

    #[test]
    fn quick_reseat_does_not_relaunch() {
        let mut h = Harness::new();
        h.poll(tag(A));
        h.polls(5, None); // removed, 1 s after last sighting
        let uid = Uid::from_hex(A).unwrap();
        assert_eq!(h.poll(tag(A)), vec![Event::Reseated { uid }]);
        assert_eq!(h.polls(3, tag(A)), vec![]);
        // Removing again still reports removal.
        assert_eq!(h.polls(5, None), vec![removed(A)]);
    }

    #[test]
    fn slow_reseat_relaunches() {
        let mut h = Harness::new();
        h.poll(tag(A));
        h.polls(5, None);
        h.polls(20, None); // 5 s since last sighting > 3 s window
        assert_eq!(h.poll(tag(A)), vec![inserted(A)]);
    }

    #[test]
    fn swap_to_new_uid_launches() {
        let mut h = Harness::new();
        h.poll(tag(A));
        h.polls(5, None);
        assert_eq!(h.poll(tag(B)), vec![inserted(B)], "after removal");
        assert_eq!(h.poll(tag(A)), vec![removed(B), inserted(A)], "direct swap");
        // Re-seat only applies to the tag seen last, so swapping straight
        // back to B is a deliberate change and launches.
        assert_eq!(h.poll(tag(B)), vec![removed(A), inserted(B)]);
    }

    #[test]
    fn zero_threshold_behaves_as_one() {
        let mut h = Harness::new();
        h.debouncer.config.miss_threshold = 0;
        h.poll(tag(A));
        assert_eq!(h.poll(None), vec![removed(A)]);
    }
}
