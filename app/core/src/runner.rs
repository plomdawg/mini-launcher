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
    debouncer: Debouncer,
}

impl Runner {
    pub fn new(config: Config, dry_run: bool) -> Runner {
        Runner {
            config,
            dry_run,
            paused: false,
            debouncer: Debouncer::new(DebounceConfig::default()),
        }
    }

    /// While paused, inserted tags are tracked (so resuming does not launch
    /// the tag already on the reader) but nothing is launched.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Replaces the library, e.g. after the config file changed.
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Handles one tick taken at `now`, launching as needed.
    pub fn step<E: Display>(&mut self, tick: Tick<E>, now: Instant) -> Vec<Notice> {
        let events = match tick {
            Tick::Reading(reading) => self.debouncer.update(reading, now),
            Tick::Lost { port, error } => {
                tracing::warn!(
                    "lost the reader on {port} ({error}); was the adapter unplugged? \
                     Reconnecting when it is back"
                );
                let error = error.to_string();
                let mut notices = vec![Notice::ReaderLost { port, error }];
                notices.extend(self.debouncer.disconnect().map(|e| self.handle(e)));
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
            Tick::Waiting => Vec::new(),
        };
        events.into_iter().map(|e| self.handle(e)).collect()
    }

    fn handle(&self, event: Event) -> Notice {
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
        if self.paused {
            tracing::info!(%uid, "inserted while paused; not launching");
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
}
