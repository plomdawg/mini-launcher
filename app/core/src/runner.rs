//! The run loop shared by `mini-launcher run` and the tray app: reader ticks
//! → debounce → resolve via config → launch.
//!
//! [`Runner`] turns each [`Tick`] into [`Notice`]s (launching as a side
//! effect) and logs them; front ends only decide how to show a notice.

use std::fmt::Display;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

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
    /// A tag was inserted while paused: not launched.
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

/// Debounce + resolve + launch state for one run.
pub struct Runner {
    config: Config,
    dry_run: bool,
    paused: bool,
    /// Resumed, but no reading has come in since: the first one only
    /// records what is on the reader (see [`Self::set_paused`]).
    resuming: bool,
    debouncer: Debouncer,
}

impl Runner {
    pub fn new(config: Config, dry_run: bool) -> Runner {
        Runner {
            config,
            dry_run,
            paused: false,
            resuming: false,
            debouncer: Debouncer::new(DebounceConfig::default()),
        }
    }

    /// While paused nothing is launched. Readings taken while paused (a
    /// reader that keeps polling) are tracked; a reader that idles (see
    /// [`crate::reconnect::Reconnector::set_paused`]) sends none. Either way
    /// the first reading after resuming only records the tags already on
    /// the reader, so resuming never launches the cart left in it.
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused && !paused {
            self.resuming = true;
        }
        self.paused = paused;
    }

    /// Replaces the library, e.g. after the config file changed.
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Handles one tick taken at `now`, launching as needed.
    pub fn step<E: Display>(&mut self, tick: Tick<E>, now: Instant) -> Vec<Notice> {
        let silent = self.paused || self.resuming;
        let events = match tick {
            Tick::Reading(reading) => {
                self.resuming = false;
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
    fn handle(&self, event: Event, silent: bool) -> Notice {
        let (uid, payload) = match event {
            Event::Inserted { uid, payload } => (uid, payload),
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
            tracing::info!(%uid, "on the reader while paused; not launching");
            return Notice::Paused { uid };
        }
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

    #[test]
    fn resuming_an_idle_reader_skips_the_tags_on_it_then_launches_new_ones() {
        let mut runner = Runner::new(Config::default(), true);
        let now = Instant::now();
        let read = |hex: &str, appid| {
            let uid = Uid::from_hex(hex).unwrap();
            Tick::<Infallible>::Reading(Some((uid, Payload::Steam(appid))))
        };
        runner.set_paused(true);
        assert_eq!(runner.step(Tick::<Infallible>::Paused, now), []);
        // A cart put in while paused is found by the first poll: no launch.
        runner.set_paused(false);
        assert_eq!(
            runner.step(read("04a1b2c3", 440), now),
            [Notice::Paused { uid: uid() }]
        );
        assert_eq!(runner.step(read("04a1b2c3", 440), now), []);
        // A swap after resuming launches.
        let other = Uid::from_hex("04d4e5f6").unwrap();
        let notices = runner.step(read("04d4e5f6", 570), now);
        assert_eq!(notices[0], Notice::Removed { uid: uid() });
        assert!(matches!(&notices[1], Notice::Launched { uid, .. } if *uid == other));
        assert_eq!(notices.len(), 2);
    }

    #[test]
    fn a_new_tag_after_resuming_with_an_empty_reader_launches() {
        let mut runner = Runner::new(Config::default(), true);
        let now = Instant::now();
        runner.set_paused(true);
        runner.set_paused(false);
        assert_eq!(runner.step(Tick::<Infallible>::Reading(None), now), []);
        let tick = Tick::<Infallible>::Reading(Some((uid(), Payload::Steam(440))));
        assert!(matches!(
            &runner.step(tick, now)[..],
            [Notice::Launched { .. }]
        ));
    }
}
