//! The run loop shared by `mini-launcher run` and the tray app: reader ticks
//! → debounce → resolve via config → launch.
//!
//! [`Runner`] turns each [`Tick`] into [`Notice`]s (launching as a side
//! effect) and logs them; front ends only decide how to show a notice.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::debounce::{DebounceConfig, Debouncer, Event, DEFAULT_POLL_INTERVAL};
use crate::launch::{self, LaunchPlan, Outcome, Resolution, Via};
use crate::payload::Slug;
use crate::reader::Uid;
use crate::reconnect::{Clock, Tick};

/// Something the user may want to hear about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// `plan` was started (or, with `dry_run`, would have been).
    Launched {
        uid: Uid,
        plan: LaunchPlan,
        via: Via,
        dry_run: bool,
    },
    /// Starting `plan` failed.
    LaunchFailed {
        uid: Uid,
        plan: LaunchPlan,
        via: Via,
        error: String,
    },
    /// A tag was inserted while paused, or was already on the reader at
    /// start-up or on resume: not launched.
    Paused {
        uid: Uid,
    },
    /// The tag came back within the re-seat window: not relaunched.
    Reseated {
        uid: Uid,
    },
    Removed {
        uid: Uid,
    },
    /// The tag says `id:<slug>`, but the config has no such game.
    UnknownId {
        uid: Uid,
        slug: Slug,
    },
    /// Nothing usable on the tag and its UID is not mapped.
    Unmapped {
        uid: Uid,
    },
    /// The reader failed; reconnecting.
    ReaderLost {
        port: String,
        error: String,
    },
    ReaderReconnected {
        port: String,
    },
    /// Reconnecting keeps failing; still retrying.
    ReaderStuck {
        port: String,
        attempts: u32,
        error: String,
    },
}

/// How long after resuming tags on the reader are only recorded, not
/// launched: two poll intervals, so one missed poll (a flaky first read of a
/// cart that sat in the field while it was off) cannot end the window.
pub const RESUME_SILENCE: Duration = Duration::from_millis(400);

/// The silent window after a resume (see [`Runner::set_paused`]) or at
/// start-up (see [`Runner::wait_for_reseat`]).
#[derive(Debug, Clone, Copy, Default)]
struct Resume {
    /// The first reading since the window opened; `set_paused` has no time,
    /// and a reader that is still being opened sends none.
    since: Option<Instant>,
}

/// Debounce + resolve + launch state for one run.
pub struct Runner {
    config: Config,
    dry_run: bool,
    paused: bool,
    /// Set while the post-resume silent window is open.
    resuming: Option<Resume>,
    /// The tag last recorded without launching (paused, or in a silent
    /// window), until a launch. Taking it out and putting it back launches
    /// it even within the re-seat window: it never started a game.
    unlaunched: Option<Uid>,
    debouncer: Debouncer,
}

impl Runner {
    /// A runner that launches a cart already on the reader when it starts
    /// (`boot_cart = "launch"`).
    pub fn new(config: Config, dry_run: bool) -> Runner {
        Runner {
            config,
            dry_run,
            paused: false,
            resuming: None,
            unlaunched: None,
            debouncer: Debouncer::new(DebounceConfig::default()),
        }
    }

    /// A runner that starts the way it is after a resume
    /// (`boot_cart = "wait"`): see [`Runner::wait_for_reseat`].
    pub fn new_waiting_for_reseat(config: Config, dry_run: bool) -> Runner {
        let mut runner = Runner::new(config, dry_run);
        runner.wait_for_reseat();
        runner
    }

    /// Tags on the reader now (or found by the first readings, see
    /// [`RESUME_SILENCE`]) are only recorded, not launched: only taking one
    /// out and putting it back (or a different cart) launches. For start-up,
    /// before the first reading; [`Runner::set_paused`] does the same on
    /// resume.
    pub fn wait_for_reseat(&mut self) {
        self.resuming = Some(Resume::default());
    }

    /// The library in use.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// While paused nothing is launched. Readings taken while paused (a
    /// reader that keeps polling) are tracked; a reader that idles (see
    /// [`crate::reconnect::Reconnector::set_paused`]) sends none. Either way
    /// after resuming, readings only record the tags already on the reader
    /// until [`RESUME_SILENCE`] has passed since the first reading, so
    /// resuming never launches the cart left in it, even if the first read
    /// after switching the field on misses.
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused && !paused {
            self.resuming = Some(Resume::default());
        }
        self.paused = paused;
    }

    /// Replaces the library, e.g. after the config file changed.
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Handles one tick taken at `now`, launching as needed.
    pub fn step<E: Display>(&mut self, tick: Tick<E>, now: Instant) -> Vec<Notice> {
        if let Some(Resume { since: Some(since) }) = self.resuming {
            if now.saturating_duration_since(since) >= RESUME_SILENCE {
                self.resuming = None;
            }
        }
        let silent = self.paused || self.resuming.is_some();
        let events = match tick {
            Tick::Reading(reading) => {
                if let Some(resume) = &mut self.resuming {
                    resume.since.get_or_insert(now);
                }
                // A tag still on the reader after a reconnect was never
                // lifted: its `Reseated` is not a re-insert.
                if self.debouncer.holding() {
                    let events = self.debouncer.update(reading, now);
                    return events
                        .into_iter()
                        .map(|e| match e {
                            Event::Reseated { uid, .. } => {
                                tracing::info!(%uid, "still on the reader after reconnecting");
                                Notice::Reseated { uid }
                            }
                            e => self.handle(e, silent),
                        })
                        .collect();
                }
                self.debouncer.update(reading, now)
            }
            Tick::Lost { port, error } => {
                tracing::warn!(
                    "lost the reader on {port} ({error}); was the adapter unplugged? \
                     Reconnecting when it is back"
                );
                let error = error.to_string();
                let mut notices = vec![Notice::ReaderLost { port, error }];
                let removed = self.debouncer.disconnect();
                notices.extend(removed.map(|e| self.handle(e, true)));
                return notices;
            }
            Tick::Reconnected { port } => {
                tracing::info!(%port, "reader reconnected");
                return vec![Notice::ReaderReconnected { port }];
            }
            Tick::Stuck {
                port,
                attempts,
                error,
            } => {
                tracing::warn!(
                    "the reader on {port} is still not back after {attempts} attempts: {error}; \
                     still retrying"
                );
                return vec![Notice::ReaderStuck {
                    port,
                    attempts,
                    error,
                }];
            }
            Tick::Waiting | Tick::Paused => Vec::new(),
        };
        events.into_iter().map(|e| self.handle(e, silent)).collect()
    }

    /// Turns `event` into a notice, launching unless `silent`.
    fn handle(&mut self, event: Event, silent: bool) -> Notice {
        let (uid, payload) = match event {
            Event::Inserted { uid, payload } => (uid, payload),
            // Re-inserted after it was only recorded: a launch.
            Event::Reseated { uid, payload } if self.unlaunched == Some(uid) && !silent => {
                tracing::info!(%uid, "re-inserted after it was recorded without launching");
                (uid, payload)
            }
            Event::Reseated { uid, .. } => {
                tracing::info!(%uid, "reseated; not relaunching");
                return Notice::Reseated { uid };
            }
            Event::Removed { uid } => {
                tracing::info!(%uid, "removed");
                return Notice::Removed { uid };
            }
        };
        if silent {
            if self.paused {
                tracing::info!(%uid, "on the reader while paused; not launching");
            } else {
                tracing::info!(%uid, "already on the reader; launches when re-inserted");
            }
            self.unlaunched = Some(uid);
            return Notice::Paused { uid };
        }
        self.unlaunched = None;
        match launch::resolve(&self.config, uid.as_bytes(), &payload) {
            Resolution::Launch { plan, via } => match launch::execute(&plan, self.dry_run) {
                Ok(outcome) => {
                    let dry_run = outcome == Outcome::DryRun;
                    let suffix = if dry_run { " [dry run]" } else { "" };
                    tracing::info!("launch {uid}: {plan} ({}){suffix}", describe(&via));
                    Notice::Launched {
                        uid,
                        plan,
                        via,
                        dry_run,
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "launch {uid}: {plan} ({}): failed to start: {e}",
                        describe(&via)
                    );
                    Notice::LaunchFailed {
                        uid,
                        plan,
                        via,
                        error: e.to_string(),
                    }
                }
            },
            Resolution::UnknownId(slug) => {
                tracing::warn!(%uid, "tag says id:{slug}, which is not in the config");
                Notice::UnknownId { uid, slug }
            }
            Resolution::Unmapped => {
                tracing::warn!(%uid, "unknown tag: nothing usable on it and its UID is not mapped");
                Notice::Unmapped { uid }
            }
        }
    }
}

/// Why a plan was chosen, for messages: `tag steam:`, `tag id:<slug>`, ...
pub fn describe(via: &Via) -> String {
    match via {
        Via::Uid(slug) => format!("uid mapped to \"{slug}\""),
        Via::TagSteam => "tag steam:".to_owned(),
        Via::TagId(slug) => format!("tag id:{slug}"),
    }
}

/// Feeds `next` ticks to `runner` once per poll interval and passes every
/// notice to `notify` until `stop` is set, `next` returns `None` (the source
/// is closed) or `notify` fails. `next` may also adjust the runner (pause,
/// config) before ticking.
pub fn poll_loop<E: Display, X>(
    runner: &mut Runner,
    stop: &AtomicBool,
    clock: &mut impl Clock,
    mut next: impl FnMut(&mut Runner, Instant) -> Option<Tick<E>>,
    mut notify: impl FnMut(Notice) -> Result<(), X>,
) -> Result<(), X> {
    while !stop.load(Ordering::Relaxed) {
        let now = clock.now();
        let Some(tick) = next(runner, now) else { break };
        for notice in runner.step(tick, now) {
            notify(notice)?;
        }
        clock.sleep(DEFAULT_POLL_INTERVAL);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::Payload;
    use std::convert::Infallible;

    fn uid() -> Uid {
        Uid::from_hex("04a1b2c3").unwrap()
    }

    #[test]
    fn insert_launches_and_a_lost_reader_removes_the_tag() {
        let mut runner = Runner::new(Config::default(), true);
        let now = Instant::now();
        let tick = Tick::<Infallible>::Reading(Some((uid(), Payload::Steam(440))));
        assert_eq!(
            runner.step(tick, now),
            [Notice::Launched {
                uid: uid(),
                plan: LaunchPlan::Steam { appid: 440 },
                via: Via::TagSteam,
                dry_run: true,
            }]
        );
        let lost = Tick::Lost {
            port: "COM7".into(),
            error: "gone",
        };
        assert_eq!(
            runner.step(lost, now),
            [
                Notice::ReaderLost {
                    port: "COM7".into(),
                    error: "gone".into()
                },
                Notice::Removed { uid: uid() },
            ]
        );
    }

    #[test]
    fn unusable_unmapped_tag_is_reported_not_launched() {
        let mut runner = Runner::new(Config::default(), true);
        let tick = Tick::<Infallible>::Reading(Some((uid(), Payload::Unusable)));
        assert_eq!(
            runner.step(tick, Instant::now()),
            [Notice::Unmapped { uid: uid() }]
        );
    }

    #[test]
    fn paused_inserts_do_not_launch_and_resuming_keeps_the_seated_tag() {
        let mut runner = Runner::new(Config::default(), true);
        let now = Instant::now();
        let tag = || Tick::<Infallible>::Reading(Some((uid(), Payload::Steam(440))));
        runner.set_paused(true);
        assert_eq!(runner.step(tag(), now), [Notice::Paused { uid: uid() }]);
        runner.set_paused(false);
        assert_eq!(runner.step(tag(), now), []);
    }

    fn read(hex: &str, appid: u32) -> Tick<Infallible> {
        let uid = Uid::from_hex(hex).unwrap();
        Tick::Reading(Some((uid, Payload::Steam(appid))))
    }

    const POLL: Duration = DEFAULT_POLL_INTERVAL;

    #[test]
    fn resuming_an_idle_reader_skips_the_tags_on_it_then_launches_new_ones() {
        let mut runner = Runner::new(Config::default(), true);
        let t0 = Instant::now();
        runner.set_paused(true);
        assert_eq!(runner.step(Tick::<Infallible>::Paused, t0), []);
        // A cart put in while paused is found by the first poll: no launch.
        runner.set_paused(false);
        assert_eq!(
            runner.step(read("04a1b2c3", 440), t0),
            [Notice::Paused { uid: uid() }]
        );
        assert_eq!(runner.step(read("04a1b2c3", 440), t0 + POLL), []);
        // A swap after the window launches.
        let other = Uid::from_hex("04d4e5f6").unwrap();
        let notices = runner.step(read("04d4e5f6", 570), t0 + RESUME_SILENCE);
        assert_eq!(notices[0], Notice::Removed { uid: uid() });
        assert!(matches!(&notices[1], Notice::Launched { uid, .. } if *uid == other));
        assert_eq!(notices.len(), 2);
    }

    #[test]
    fn a_missed_first_read_after_resuming_does_not_launch_the_seated_cart() {
        let mut runner = Runner::new(Config::default(), true);
        let t0 = Instant::now();
        runner.set_paused(true);
        runner.set_paused(false);
        // The first read of the cart fails; the next one finds it.
        assert_eq!(runner.step(Tick::<Infallible>::Reading(None), t0), []);
        assert_eq!(
            runner.step(read("04a1b2c3", 440), t0 + POLL),
            [Notice::Paused { uid: uid() }]
        );
        assert_eq!(runner.step(read("04a1b2c3", 440), t0 + 2 * POLL), []);
    }

    #[test]
    fn the_window_needs_a_reading_not_just_time() {
        let mut runner = Runner::new(Config::default(), true);
        let t0 = Instant::now();
        runner.set_paused(true);
        runner.set_paused(false);
        // A slow reconnect: no reading until long after the resume.
        assert_eq!(runner.step(Tick::<Infallible>::Waiting, t0), []);
        let late = t0 + 10 * RESUME_SILENCE;
        assert_eq!(
            runner.step(read("04a1b2c3", 440), late),
            [Notice::Paused { uid: uid() }]
        );
    }

    #[test]
    fn a_new_tag_after_the_window_launches() {
        let mut runner = Runner::new(Config::default(), true);
        let t0 = Instant::now();
        runner.set_paused(true);
        runner.set_paused(false);
        assert_eq!(runner.step(Tick::<Infallible>::Reading(None), t0), []);
        assert_eq!(
            runner.step(Tick::<Infallible>::Reading(None), t0 + POLL),
            []
        );
        assert!(matches!(
            &runner.step(read("04a1b2c3", 440), t0 + RESUME_SILENCE)[..],
            [Notice::Launched { .. }]
        ));
    }

    #[test]
    fn launch_at_boot_starts_a_cart_already_on_the_reader() {
        let mut runner = Runner::new(Config::default(), true);
        let t0 = Instant::now();
        // The reader is still being opened.
        assert_eq!(runner.step(Tick::<Infallible>::Waiting, t0), []);
        assert!(matches!(
            &runner.step(read("04a1b2c3", 440), t0 + POLL)[..],
            [Notice::Launched { .. }]
        ));
    }

    #[test]
    fn wait_at_boot_records_the_seated_cart_until_it_is_reinserted() {
        let mut runner = Runner::new_waiting_for_reseat(Config::default(), true);
        let t0 = Instant::now();
        // However long the reader takes to open, the window starts at the
        // first reading, and a missed first read is covered too.
        assert_eq!(runner.step(Tick::<Infallible>::Waiting, t0), []);
        let first = t0 + 10 * RESUME_SILENCE;
        assert_eq!(runner.step(Tick::<Infallible>::Reading(None), first), []);
        assert_eq!(
            runner.step(read("04a1b2c3", 440), first + POLL),
            [Notice::Paused { uid: uid() }]
        );
        // Still seated long after: never launched.
        for i in 2..20 {
            assert_eq!(runner.step(read("04a1b2c3", 440), first + i * POLL), []);
        }
        // Taken out and put back about 1.5 s later, well inside the re-seat
        // window: it never started a game, so it launches now.
        let now = first + 20 * POLL;
        let (removed, back) = lift(&mut runner, now);
        assert_eq!(removed, [Notice::Removed { uid: uid() }]);
        assert_eq!(back, Duration::from_millis(1400));
        let notices = runner.step(read("04a1b2c3", 440), now + back);
        assert!(
            matches!(&notices[..], [Notice::Launched { .. }]),
            "{notices:?}"
        );
        // Launched now: a quick bump no longer relaunches it.
        let now = now + back + POLL;
        let (_, back) = lift(&mut runner, now);
        assert_eq!(
            runner.step(read("04a1b2c3", 440), now + back),
            [Notice::Reseated { uid: uid() }]
        );
    }

    /// Lifts the seated tag at `now`: missed polls until it counts as
    /// removed, then two more. Returns the notices and the time taken.
    fn lift(runner: &mut Runner, now: Instant) -> (Vec<Notice>, Duration) {
        let mut notices = Vec::new();
        let mut at = now;
        for _ in 0..7 {
            at += POLL;
            notices.extend(runner.step(Tick::<Infallible>::Reading(None), at));
        }
        (notices, at - now)
    }

    #[test]
    fn a_cart_recorded_on_resume_launches_when_quickly_reinserted() {
        let mut runner = Runner::new(Config::default(), true);
        let t0 = Instant::now();
        runner.set_paused(true);
        assert_eq!(runner.step(Tick::<Infallible>::Paused, t0), []);
        runner.set_paused(false);
        assert_eq!(
            runner.step(read("04a1b2c3", 440), t0 + POLL),
            [Notice::Paused { uid: uid() }]
        );
        let now = t0 + 5 * POLL;
        assert_eq!(runner.step(read("04a1b2c3", 440), now), []);
        let (removed, back) = lift(&mut runner, now);
        assert_eq!(removed, [Notice::Removed { uid: uid() }]);
        assert!(matches!(
            &runner.step(read("04a1b2c3", 440), now + back)[..],
            [Notice::Launched { .. }]
        ));
    }

    #[test]
    fn a_launched_cart_bumped_during_play_does_not_relaunch() {
        let mut runner = Runner::new(Config::default(), true);
        let t0 = Instant::now();
        assert!(matches!(
            &runner.step(read("04a1b2c3", 440), t0)[..],
            [Notice::Launched { .. }]
        ));
        let (_, back) = lift(&mut runner, t0);
        assert_eq!(
            runner.step(read("04a1b2c3", 440), t0 + back),
            [Notice::Reseated { uid: uid() }]
        );
    }

    #[test]
    fn a_cart_recorded_at_boot_survives_a_replug_then_launches_when_reinserted() {
        let mut runner = Runner::new_waiting_for_reseat(Config::default(), true);
        let t0 = Instant::now();
        assert_eq!(
            runner.step(read("04a1b2c3", 440), t0),
            [Notice::Paused { uid: uid() }]
        );
        let now = t0 + RESUME_SILENCE;
        assert_eq!(runner.step(read("04a1b2c3", 440), now), []);
        let lost = Tick::Lost {
            port: "COM7".into(),
            error: "gone",
        };
        runner.step(lost, now);
        // Back with the cart never lifted: not a re-insert.
        let now = now + 2 * POLL;
        assert_eq!(
            runner.step(read("04a1b2c3", 440), now),
            [Notice::Reseated { uid: uid() }]
        );
        // Lifted and put back for real: launches.
        let (_, back) = lift(&mut runner, now);
        assert!(matches!(
            &runner.step(read("04a1b2c3", 440), now + back)[..],
            [Notice::Launched { .. }]
        ));
    }

    #[test]
    fn wait_at_boot_launches_a_cart_put_in_after_start() {
        let mut runner = Runner::new_waiting_for_reseat(Config::default(), true);
        let t0 = Instant::now();
        assert_eq!(runner.step(Tick::<Infallible>::Reading(None), t0), []);
        assert_eq!(
            runner.step(Tick::<Infallible>::Reading(None), t0 + POLL),
            []
        );
        assert!(matches!(
            &runner.step(read("04a1b2c3", 440), t0 + RESUME_SILENCE)[..],
            [Notice::Launched { .. }]
        ));
    }

    #[test]
    fn a_reconnect_does_not_relaunch_the_seated_cart_in_either_mode() {
        for mut runner in [
            Runner::new(Config::default(), true),
            Runner::new_waiting_for_reseat(Config::default(), true),
        ] {
            let t0 = Instant::now() + Duration::from_secs(1);
            // Seated (launched or recorded) well before the replug.
            runner.step(read("04a1b2c3", 440), t0);
            runner.step(read("04a1b2c3", 440), t0 + RESUME_SILENCE);
            let lost = Tick::Lost {
                port: "COM7".into(),
                error: "gone",
            };
            runner.step(lost, t0 + RESUME_SILENCE);
            let back = t0 + Duration::from_secs(30);
            runner.step(
                Tick::<Infallible>::Reconnected {
                    port: "COM7".into(),
                },
                back,
            );
            assert_eq!(
                runner.step(read("04a1b2c3", 440), back),
                [Notice::Reseated { uid: uid() }]
            );
        }
    }
}
