//! Tag sessions: another thread (the settings window's add-game wizard)
//! borrows the reader from the run loop without ever opening the port.
//!
//! The run loop owns the reader. A [`SessionClient`] asks it, over a
//! channel, to suspend launches, report the next tag it sees, write a
//! payload to that tag (where the reader lives) and report the outcome.
//! Dropping the client (cancel, window closed, panic) ends the session and
//! the run loop goes back to normal: launches resume, with the usual
//! post-resume silence so the cart left on the reader is not launched.
//!
//! [`SessionServer::tick`] is the run loop's side. It is a plain state
//! machine over a [`Reconnector`] and a [`Runner`], so it is tested without
//! hardware or threads.

use std::fmt::Display;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::Instant;

use crate::payload::Payload;
use crate::pn532::write::WriteError;
use crate::reader::{Reader, TagWriter, Uid};
use crate::reconnect::{ReaderState, Reconnector, Tick};
use crate::runner::Runner;

/// Polls in a row without a tag after which a skipped tag (see
/// [`SessionClient::watch`]) counts as lifted off, so it is reported again
/// when it comes back. More than one, so one flaky read doesn't count.
pub const LIFTED_AFTER: u32 = 3;

/// From the client to the run loop.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    /// Report the next tag seen, except `skip` until it has been lifted off.
    Watch { skip: Option<Uid> },
    /// Write `payload` to the tag `uid` (which must still be on the reader).
    Write { uid: Uid, payload: Payload },
}

/// Why a write did not happen, as the wizard needs it (cloneable, unlike
/// [`WriteError`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteFailure {
    /// The tag is read-only.
    Locked,
    /// The payload does not fit.
    TooSmall { need: usize, have: usize },
    /// Not an NDEF-formatted Type 2 tag.
    UnknownTag,
    /// The tag left the reader (or another one took its place).
    TagRemoved,
    /// The tag did not read back what was written.
    VerifyFailed,
    /// The payload is not writable (a bug in the caller).
    Unusable,
    /// The reader failed or is not connected.
    Reader(String),
}

impl WriteFailure {
    /// Whether the tag can't hold a payload at all, so binding its UID is
    /// the way to use it. The other failures are worth another try.
    pub fn needs_uid_binding(&self) -> bool {
        matches!(
            self,
            WriteFailure::Locked | WriteFailure::TooSmall { .. } | WriteFailure::UnknownTag
        )
    }
}

impl From<WriteError> for WriteFailure {
    fn from(e: WriteError) -> WriteFailure {
        match e {
            WriteError::Unusable => WriteFailure::Unusable,
            WriteError::UnknownTag => WriteFailure::UnknownTag,
            WriteError::Locked => WriteFailure::Locked,
            WriteError::TooSmall { need, have } => WriteFailure::TooSmall { need, have },
            WriteError::TagRemoved => WriteFailure::TagRemoved,
            WriteError::VerifyFailed => WriteFailure::VerifyFailed,
            WriteError::Transport(e) => WriteFailure::Reader(e.to_string()),
        }
    }
}

/// From the run loop to the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    /// Sent when the session starts and whenever it changes.
    Reader { connected: bool },
    /// The tag asked for by [`SessionClient::watch`] and what it holds now.
    Tag { uid: Uid, payload: Payload },
    /// The outcome of [`SessionClient::write`].
    Written {
        uid: Uid,
        result: Result<(), WriteFailure>,
    },
}

/// A new session for the run loop.
struct Open {
    requests: Receiver<Request>,
    updates: Sender<Update>,
}

/// Starts sessions; cheap to share between threads.
#[derive(Clone)]
pub struct Sessions(Sender<Open>);

impl Sessions {
    /// Starts a session: launches stop until the client is dropped. A
    /// session started while another is running replaces it (the old
    /// client's channel closes). If the run loop has ended, the client
    /// reports [`SessionClient::is_closed`].
    pub fn open(&self) -> SessionClient {
        let (requests_tx, requests) = mpsc::channel();
        let (updates, updates_rx) = mpsc::channel();
        let _ = self.0.send(Open { requests, updates });
        SessionClient {
            requests: requests_tx,
            updates: updates_rx,
            closed: false,
        }
    }
}

/// The wizard's end of a session. Dropping it ends the session.
pub struct SessionClient {
    requests: Sender<Request>,
    updates: Receiver<Update>,
    closed: bool,
}

impl SessionClient {
    /// Asks for the next tag seen. With `skip`, that tag is ignored until it
    /// has been lifted off the reader (e.g. after the user said "not this
    /// one"). Replaces an earlier watch.
    pub fn watch(&self, skip: Option<Uid>) {
        let _ = self.requests.send(Request::Watch { skip });
    }

    /// Asks the run loop to write `payload` to tag `uid`; the outcome comes
    /// back as [`Update::Written`].
    pub fn write(&self, uid: Uid, payload: Payload) {
        let _ = self.requests.send(Request::Write { uid, payload });
    }

    /// The next update, if one arrived.
    pub fn try_recv(&mut self) -> Option<Update> {
        match self.updates.try_recv() {
            Ok(update) => Some(update),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.closed = true;
                None
            }
        }
    }

    /// True once the run loop ended the session (it quit, or a newer
    /// session replaced this one): nothing more will arrive.
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

/// A session the run loop is serving.
struct Active {
    requests: Receiver<Request>,
    updates: Sender<Update>,
    /// `Some` while a [`Request::Watch`] is pending: the tag to skip, and
    /// how many polls in a row have seen no tag.
    watch: Option<(Option<Uid>, u32)>,
    /// The reader state last reported.
    connected: Option<bool>,
}

impl Active {
    /// Sends `update`; false once the client is gone.
    fn send(&self, update: Update) -> bool {
        self.updates.send(update).is_ok()
    }
}

/// The run loop's end: owns at most one active session.
pub struct SessionServer {
    opens: Receiver<Open>,
    active: Option<Active>,
}

/// A connected [`Sessions`] / [`SessionServer`] pair.
pub fn channel() -> (Sessions, SessionServer) {
    let (tx, rx) = mpsc::channel();
    (
        Sessions(tx),
        SessionServer {
            opens: rx,
            active: None,
        },
    )
}

impl SessionServer {
    /// Whether a session is running (launches are suspended).
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// One run-loop tick: takes new sessions and requests, sets the pause
    /// state of `runner` and `reader` (`paused` is the user's own pause),
    /// ticks the reader and reports what it saw to the session. Returns the
    /// tick for the runner.
    ///
    /// While a session runs, the reader keeps polling (even if the user
    /// paused) and the runner launches nothing. When it ends, the user's
    /// pause state applies again and the runner's resume silence keeps the
    /// cart on the reader from launching.
    pub fn tick<R, F, E>(
        &mut self,
        reader: &mut Reconnector<R, F>,
        runner: &mut Runner,
        paused: bool,
        now: Instant,
    ) -> Tick<R::Error>
    where
        R: Reader + TagWriter,
        F: FnMut() -> Result<(String, R), E>,
        E: Display,
    {
        self.accept();
        self.report(reader.state());
        self.serve(reader);
        let active = self.active.is_some();
        runner.set_paused(paused || active);
        reader.set_paused(paused && !active);
        let tick = reader.tick(now);
        self.observe(&tick, reader.state());
        tick
    }

    /// Takes the newest session; an older one ends (its channel closes).
    fn accept(&mut self) {
        while let Ok(open) = self.opens.try_recv() {
            if self.active.is_some() {
                tracing::info!("tag session replaced by a new one");
            } else {
                tracing::info!("tag session started; launches suspended");
            }
            self.active = Some(Active {
                requests: open.requests,
                updates: open.updates,
                watch: None,
                connected: None,
            });
        }
    }

    fn end(&mut self) {
        if self.active.take().is_some() {
            tracing::info!("tag session ended; launches resume");
        }
    }

    /// Handles the session's pending requests.
    fn serve<R, F, E>(&mut self, reader: &mut Reconnector<R, F>)
    where
        R: Reader + TagWriter,
        F: FnMut() -> Result<(String, R), E>,
        E: Display,
    {
        loop {
            let Some(active) = &mut self.active else {
                return;
            };
            let request = match active.requests.try_recv() {
                Ok(request) => request,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => return self.end(),
            };
            let alive = match request {
                Request::Watch { skip } => {
                    active.watch = Some((skip, 0));
                    true
                }
                Request::Write { uid, payload } => {
                    active.watch = None;
                    let result = match reader.reader_mut() {
                        Some(r) => r.write_tag(uid, &payload).map(drop).map_err(Into::into),
                        None => Err(WriteFailure::Reader("the reader is not connected".into())),
                    };
                    match &result {
                        Ok(()) => tracing::info!(%uid, "wrote the tag from the wizard"),
                        Err(e) => tracing::info!(%uid, "tag not written from the wizard: {e:?}"),
                    }
                    active.send(Update::Written { uid, result })
                }
            };
            if !alive {
                return self.end();
            }
        }
    }

    /// Tells the session the reader state, if it changed.
    fn report(&mut self, state: &ReaderState) {
        let Some(active) = &mut self.active else {
            return;
        };
        let connected = matches!(state, ReaderState::Connected { .. });
        if active.connected != Some(connected) {
            active.connected = Some(connected);
            if !active.send(Update::Reader { connected }) {
                self.end();
            }
        }
    }

    /// Reports the reader state and a watched tag.
    fn observe<E>(&mut self, tick: &Tick<E>, state: &ReaderState) {
        self.report(state);
        let Some(active) = &mut self.active else {
            return;
        };
        let mut alive = true;
        if let (Tick::Reading(reading), Some((skip, misses))) = (tick, &mut active.watch) {
            match reading {
                None => {
                    *misses = misses.saturating_add(1);
                    if *misses >= LIFTED_AFTER {
                        *skip = None;
                    }
                }
                Some((uid, _)) if Some(*uid) == *skip => *misses = 0,
                Some((uid, payload)) => {
                    let (uid, payload) = (*uid, payload.clone());
                    active.watch = None;
                    alive = alive && active.send(Update::Tag { uid, payload });
                }
            }
        }
        if !alive {
            self.end();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::pn532::write::WriteOutcome;
    use crate::reconnect::Backoff;
    use crate::runner::{Notice, RESUME_SILENCE};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io;
    use std::rc::Rc;
    use std::time::Duration;

    /// What the fake reader sees: the tag on it, or a port failure.
    #[derive(Default)]
    struct Field {
        tag: Option<(Uid, Payload)>,
        fail: bool,
        /// Scripted write results; empty = success.
        writes: VecDeque<WriteError>,
        /// Every call, for asserting on pause/resume traffic.
        log: String,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Field>>);

    impl Reader for Fake {
        type Error = io::Error;
        fn poll(&mut self) -> io::Result<Option<(Uid, Payload)>> {
            let mut f = self.0.borrow_mut();
            f.log.push('.');
            if f.fail {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            Ok(f.tag.clone())
        }
        fn pause(&mut self) -> io::Result<()> {
            self.0.borrow_mut().log.push('P');
            Ok(())
        }
        fn resume(&mut self) -> io::Result<()> {
            self.0.borrow_mut().log.push('R');
            Ok(())
        }
    }

    impl TagWriter for Fake {
        fn write_tag(&mut self, uid: Uid, payload: &Payload) -> Result<WriteOutcome, WriteError> {
            let mut f = self.0.borrow_mut();
            f.log.push('W');
            if let Some(e) = f.writes.pop_front() {
                return Err(e);
            }
            match &mut f.tag {
                Some((on, p)) if *on == uid => *p = payload.clone(),
                _ => return Err(WriteError::TagRemoved),
            }
            Ok(WriteOutcome {
                first_page: 4,
                last_page: 7,
            })
        }
    }

    type Open = Box<dyn FnMut() -> Result<(String, Fake), &'static str>>;

    struct Rig {
        field: Fake,
        reader: Reconnector<Fake, Open>,
        runner: Runner,
        server: SessionServer,
        sessions: Sessions,
        now: Instant,
        paused: bool,
        notices: Vec<Notice>,
    }

    impl Rig {
        fn new() -> Rig {
            let field = Fake::default();
            let reopen = field.clone();
            let open: Open = Box::new(move || {
                if reopen.0.borrow().fail {
                    Err("no reader")
                } else {
                    Ok(("COM7".to_owned(), reopen.clone()))
                }
            });
            let reader = Reconnector::new("COM7".into(), field.clone(), open, Backoff::default());
            let (sessions, server) = channel();
            Rig {
                field,
                reader,
                runner: Runner::new(Config::default(), true),
                server,
                sessions,
                now: Instant::now(),
                paused: false,
                notices: Vec::new(),
            }
        }

        fn put(&self, hex: &str, payload: Payload) {
            self.field.0.borrow_mut().tag = Some((uid(hex), payload));
        }

        fn lift(&self) {
            self.field.0.borrow_mut().tag = None;
        }

        fn step(&mut self) {
            self.now += Duration::from_millis(200);
            let tick = self
                .server
                .tick(&mut self.reader, &mut self.runner, self.paused, self.now);
            self.notices.extend(self.runner.step(tick, self.now));
        }

        fn steps(&mut self, n: usize) {
            (0..n).for_each(|_| self.step());
        }

        fn launches(&mut self) -> usize {
            let n = self
                .notices
                .iter()
                .filter(|n| matches!(n, Notice::Launched { .. }))
                .count();
            self.notices.clear();
            n
        }
    }

    fn uid(hex: &str) -> Uid {
        Uid::from_hex(hex).unwrap()
    }

    fn drain(client: &mut SessionClient) -> Vec<Update> {
        std::iter::from_fn(|| client.try_recv()).collect()
    }

    const CONNECTED: Update = Update::Reader { connected: true };

    #[test]
    fn suspends_launches_reports_the_tag_writes_it_and_resumes() {
        let mut rig = Rig::new();
        let mut client = rig.sessions.open();
        rig.step();
        assert!(rig.server.is_active());
        assert_eq!(drain(&mut client), [CONNECTED]);
        // A cart placed now is not launched, and nothing is reported
        // until the wizard asks.
        rig.put("04a1b2c3", Payload::Steam(440));
        rig.steps(3);
        assert_eq!(rig.launches(), 0);
        assert_eq!(drain(&mut client), []);
        client.watch(None);
        rig.step();
        let tag = Update::Tag {
            uid: uid("04a1b2c3"),
            payload: Payload::Steam(440),
        };
        assert_eq!(drain(&mut client), [tag]);
        // One report per watch.
        rig.step();
        assert_eq!(drain(&mut client), []);
        client.write(uid("04a1b2c3"), Payload::Steam(620));
        rig.step();
        let written = Update::Written {
            uid: uid("04a1b2c3"),
            result: Ok(()),
        };
        assert_eq!(drain(&mut client), [written]);
        assert_eq!(
            rig.field.0.borrow().tag.as_ref().unwrap().1,
            Payload::Steam(620)
        );
        assert_eq!(rig.launches(), 0);
        // The wizard is done: the cart still on the reader doesn't launch...
        drop(client);
        rig.steps(5);
        assert!(!rig.server.is_active());
        assert_eq!(rig.launches(), 0);
        // ...but the next one does.
        rig.lift();
        rig.steps(6);
        rig.put("04d4e5f6", Payload::Steam(570));
        rig.step();
        assert_eq!(rig.launches(), 1);
    }

    #[test]
    fn every_write_failure_is_reported_with_its_cause() {
        let mut rig = Rig::new();
        let mut client = rig.sessions.open();
        rig.put("04a1b2c3", Payload::Unusable);
        let cases = [
            (WriteError::Locked, WriteFailure::Locked),
            (
                WriteError::TooSmall { need: 80, have: 48 },
                WriteFailure::TooSmall { need: 80, have: 48 },
            ),
            (WriteError::UnknownTag, WriteFailure::UnknownTag),
            (WriteError::TagRemoved, WriteFailure::TagRemoved),
            (WriteError::VerifyFailed, WriteFailure::VerifyFailed),
            (WriteError::Unusable, WriteFailure::Unusable),
        ];
        for (error, want) in cases {
            let fallback = want.needs_uid_binding();
            rig.field.0.borrow_mut().writes.push_back(error);
            client.write(uid("04a1b2c3"), Payload::Steam(440));
            rig.step();
            let got = drain(&mut client);
            assert_eq!(
                got.last(),
                Some(&Update::Written {
                    uid: uid("04a1b2c3"),
                    result: Err(want.clone()),
                }),
                "{want:?}"
            );
            let binds = matches!(
                want,
                WriteFailure::Locked | WriteFailure::TooSmall { .. } | WriteFailure::UnknownTag
            );
            assert_eq!(fallback, binds, "{want:?}");
        }
        // A different tag in its place counts as removed.
        client.write(uid("04ffffff"), Payload::Steam(440));
        rig.step();
        assert_eq!(
            drain(&mut client),
            [Update::Written {
                uid: uid("04ffffff"),
                result: Err(WriteFailure::TagRemoved),
            }]
        );
        assert_eq!(rig.launches(), 0);
    }

    #[test]
    fn a_skipped_tag_is_reported_again_only_after_it_was_lifted() {
        let mut rig = Rig::new();
        let mut client = rig.sessions.open();
        rig.put("04a1b2c3", Payload::Steam(440));
        client.watch(Some(uid("04a1b2c3")));
        rig.steps(5);
        assert_eq!(drain(&mut client), [CONNECTED]);
        // Another tag is reported at once.
        rig.put("04d4e5f6", Payload::Unusable);
        rig.step();
        assert!(
            matches!(&drain(&mut client)[..], [Update::Tag { uid: u, .. }] if *u == uid("04d4e5f6"))
        );
        // The skipped one: a single missed read is not a lift...
        client.watch(Some(uid("04a1b2c3")));
        rig.put("04a1b2c3", Payload::Steam(440));
        rig.step();
        rig.lift();
        rig.step();
        rig.put("04a1b2c3", Payload::Steam(440));
        rig.step();
        assert_eq!(drain(&mut client), []);
        // ...lifting it off is.
        rig.lift();
        rig.steps(LIFTED_AFTER as usize);
        rig.put("04a1b2c3", Payload::Steam(440));
        rig.step();
        assert!(matches!(&drain(&mut client)[..], [Update::Tag { .. }]));
    }

    #[test]
    fn reader_lost_mid_session_is_reported_and_writes_fail_until_it_is_back() {
        let mut rig = Rig::new();
        let mut client = rig.sessions.open();
        rig.put("04a1b2c3", Payload::Steam(440));
        client.watch(None);
        rig.step();
        assert_eq!(drain(&mut client).len(), 2);
        rig.field.0.borrow_mut().fail = true;
        rig.step();
        assert_eq!(drain(&mut client), [Update::Reader { connected: false }]);
        client.write(uid("04a1b2c3"), Payload::Steam(620));
        rig.step();
        assert!(matches!(
            &drain(&mut client)[..],
            [Update::Written {
                result: Err(WriteFailure::Reader(_)),
                ..
            }]
        ));
        client.watch(None);
        rig.field.0.borrow_mut().fail = false;
        rig.steps(5);
        let got = drain(&mut client);
        assert_eq!(got[0], CONNECTED);
        assert!(matches!(got[1], Update::Tag { .. }), "{got:?}");
        assert_eq!(rig.launches(), 0);
    }

    #[test]
    fn a_session_overrides_the_user_pause_and_hands_it_back() {
        let mut rig = Rig::new();
        rig.paused = true;
        rig.steps(2);
        assert_eq!(rig.field.0.borrow().log, "P");
        let mut client = rig.sessions.open();
        rig.put("04a1b2c3", Payload::Steam(440));
        client.watch(None);
        rig.step();
        assert!(matches!(
            drain(&mut client).last(),
            Some(Update::Tag { .. })
        ));
        assert_eq!(rig.field.0.borrow().log, "PR.");
        drop(client);
        rig.steps(2);
        assert_eq!(rig.field.0.borrow().log, "PR.P");
        assert_eq!(rig.launches(), 0);
    }

    #[test]
    fn cancelling_at_any_step_restores_normal_operation() {
        // Cancel before anything, while watching, and with a write pending.
        for stage in 0..3 {
            let mut rig = Rig::new();
            let client = rig.sessions.open();
            rig.put("04a1b2c3", Payload::Steam(440));
            rig.step();
            if stage >= 1 {
                client.watch(None);
            }
            if stage >= 2 {
                client.write(uid("04a1b2c3"), Payload::Steam(620));
            }
            drop(client);
            rig.step();
            assert!(!rig.server.is_active(), "stage {stage}");
            // A write sent just before the cancel may still have run (it
            // was asked for); nothing launches and the reader is polled.
            rig.lift();
            rig.steps(6);
            rig.now += RESUME_SILENCE;
            rig.put("04d4e5f6", Payload::Steam(570));
            rig.step();
            assert_eq!(rig.launches(), 1, "stage {stage}");
        }
    }

    #[test]
    fn a_new_session_replaces_the_old_one_and_a_dead_loop_closes_clients() {
        let mut rig = Rig::new();
        let mut old = rig.sessions.open();
        rig.step();
        let mut new = rig.sessions.open();
        rig.step();
        drain(&mut old);
        assert!(old.is_closed());
        assert_eq!(drain(&mut new), [CONNECTED]);
        assert!(!new.is_closed());
        drop(new);
        rig.step();
        assert!(!rig.server.is_active());

        let Rig {
            server, sessions, ..
        } = rig;
        drop(server);
        let mut orphan = sessions.open();
        assert_eq!(orphan.try_recv(), None);
        assert!(orphan.is_closed());
    }
}
