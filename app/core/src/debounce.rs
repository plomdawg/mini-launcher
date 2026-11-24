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
    /// The tag that was just removed came back within the re-seat window,
    /// or was still on the reader when it came back after a disconnect.
    /// Not a new launch, even if the tag was rewritten meanwhile. Consumers
    /// that close the game on `Removed` (opt-in) may relaunch from `payload`.
    Reseated { uid: Uid, payload: Payload },
    /// The tag has been gone for the configured number of polls.
    Removed { uid: Uid },
}

/// Poll interval the [`DebounceConfig`] defaults are tuned for.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(200);

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
    /// The tag seated when the reader went away. Found again on the first
    /// sighting after the reader is back, it is re-seated however long the
    /// outage was; cleared once a tag is seen or after a removal's worth of
    /// misses (the tag was lifted).
    held: Option<Uid>,
}

impl Debouncer {
    pub fn new(config: DebounceConfig) -> Debouncer {
        Debouncer {
            config,
            seated: None,
            misses: 0,
            last_seen: None,
            held: None,
        }
    }

    /// Whether the tag seated when the reader went away is being waited for
    /// (see [`Debouncer::disconnect`]): a `Reseated` for it now means it was
    /// never lifted.
    pub fn holding(&self) -> bool {
        self.held.is_some()
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
                    let held = self.held == Some(uid);
                    let reseat = held
                        || matches!(self.last_seen, Some((prev, seen))
                            if prev == uid
                                && now.saturating_duration_since(seen)
                                    <= self.config.reseat_window);
                    events.push(if reseat {
                        Event::Reseated { uid, payload }
                    } else {
                        Event::Inserted { uid, payload }
                    });
                }
                self.misses = 0;
                self.last_seen = Some((uid, now));
                self.held = None;
            }
            None if self.held.is_some() => {
                self.misses += 1;
                if self.misses >= self.config.miss_threshold.max(1) {
                    self.held = None;
                    self.misses = 0;
                }
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

    /// The reader went away: the seated tag (if any) is removed now, without
    /// waiting for missed polls. If that same tag is on the reader when it
    /// comes back, it is [`Event::Reseated`], not a new launch, however long
    /// the reader was gone: the tag was never lifted (re-enumerating a
    /// replugged adapter can take several seconds). Otherwise the usual
    /// re-seat window applies.
    pub fn disconnect(&mut self) -> Option<Event> {
        self.misses = 0;
        if let Some(uid) = self.seated {
            self.held = Some(uid);
        }
        self.seated.take().map(|uid| Event::Removed { uid })
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
        let payload = Payload::Steam(440);
        assert_eq!(h.poll(tag(A)), vec![Event::Reseated { uid, payload }]);
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
    fn disconnect_removes_now_and_keeps_the_reseat_window() {
        let mut h = Harness::new();
        assert_eq!(h.poll(tag("04a1b2c3")), [inserted("04a1b2c3")]);
        assert_eq!(h.debouncer.disconnect(), Some(removed("04a1b2c3")));
        assert_eq!(h.debouncer.disconnect(), None);
        assert_eq!(h.polls(3, None), []);
        let reseated = Event::Reseated {
            uid: Uid::from_hex("04a1b2c3").unwrap(),
            payload: Payload::Steam(440),
        };
        assert_eq!(h.poll(tag("04a1b2c3")), [reseated]);
    }

    #[test]
    fn zero_threshold_behaves_as_one() {
        let mut h = Harness::new();
        h.debouncer.config.miss_threshold = 0;
        h.poll(tag(A));
        assert_eq!(h.poll(None), vec![removed(A)]);
    }

    fn reseated(hex: &str) -> Event {
        Event::Reseated {
            uid: Uid::from_hex(hex).unwrap(),
            payload: Payload::Steam(440),
        }
    }

    #[test]
    fn same_tag_after_a_long_disconnect_is_reseated() {
        let mut h = Harness::new();
        h.poll(tag(A));
        assert_eq!(h.debouncer.disconnect(), Some(removed(A)));
        h.now += Duration::from_secs(60); // reader gone, no polls
                                          // A couple of misses while the reader settles do not drop the hold.
        assert_eq!(h.polls(2, None), []);
        assert_eq!(h.poll(tag(A)), [reseated(A)]);
        // The hold is used up: a real removal and slow re-insert launches.
        assert_eq!(h.polls(5, None), [removed(A)]);
        h.now += Duration::from_secs(10);
        assert_eq!(h.poll(tag(A)), [inserted(A)]);
    }

    #[test]
    fn different_tag_after_a_disconnect_launches() {
        let mut h = Harness::new();
        h.poll(tag(A));
        h.debouncer.disconnect();
        h.now += Duration::from_secs(60);
        assert_eq!(h.poll(tag(B)), [inserted(B)]);
        // A is no longer held either.
        assert_eq!(h.polls(5, None), [removed(B)]);
        h.now += Duration::from_secs(10);
        assert_eq!(h.poll(tag(A)), [inserted(A)]);
    }

    #[test]
    fn tag_lifted_after_reconnect_then_replaced_launches() {
        let mut h = Harness::new();
        h.poll(tag(A));
        h.debouncer.disconnect();
        h.now += Duration::from_secs(60);
        assert_eq!(h.polls(5, None), []); // lifted: hold dropped
        assert_eq!(h.poll(tag(A)), [inserted(A)]);
    }

    #[test]
    fn disconnect_without_a_tag_holds_nothing() {
        let mut h = Harness::new();
        h.poll(tag(A));
        h.polls(5, None);
        h.debouncer.disconnect();
        h.now += Duration::from_secs(60);
        assert_eq!(h.poll(tag(A)), [inserted(A)]);
    }
}
