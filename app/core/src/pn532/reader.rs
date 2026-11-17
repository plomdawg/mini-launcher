//! [`Pn532Hsu`]: the hardware [`Reader`] on top of the PN532 driver.
//!
//! Each poll lists one ISO/IEC 14443-A target, reads its user memory (at most
//! the NTAG213's pages 4..=39, stopping as soon as the NDEF data is complete,
//! so smaller Type 2 tags are never read past their end), decodes it into a
//! [`Payload`] and releases the target.
//! Flaky radio or serial traffic (timeouts, stray frames, a tag pulled
//! mid-read) counts as a missed poll; the debouncer absorbs it. A failed read
//! is retried on every poll while the tag stays present, so a cart that is
//! still sliding in is reported once a read succeeds. A tag whose reads keep
//! failing ([`UNREADABLE_AFTER`] polls in a row) is reported as present with
//! an unusable payload, so it can still be mapped by UID; that fallback is
//! not cached, so reads keep being retried. Only port I/O errors (adapter
//! unplugged) are returned; `run` then drops the reader and reopens it
//! (see [`crate::reconnect`]).

use std::time::Duration;

use super::driver::{Pn532, Pn532Error, Target, Transport};
use crate::ndef::{self, NdefError, NdefRecord};
use crate::payload::Payload;
use crate::reader::{Reader, Reading, Uid};

/// First NTAG213 user page.
pub const FIRST_USER_PAGE: u8 = 4;
/// Last NTAG213 user page.
pub const LAST_USER_PAGE: u8 = 39;
/// NTAG213 user memory size in bytes (36 pages of 4 bytes).
pub const USER_MEMORY_LEN: usize = 144;
/// `MxRtyPassiveActivation`: few retries, so "no tag" comes back in a few
/// milliseconds, far inside the command timeout.
pub const PASSIVE_RETRIES: u8 = 0x05;
/// Per-command timeout while polling.
pub const COMMAND_TIMEOUT: Duration = Duration::from_millis(500);
/// SAK of an NFC Forum Type 2 tag (NTAG21x, MIFARE Ultralight). Any other
/// target (e.g. MIFARE Classic, DESFire) is reported with an unusable payload
/// without trying to read it, so it can still be mapped by UID.
pub const TYPE2_SAK: u8 = 0x00;
/// Consecutive failed reads of the same tag before it is reported with an
/// unusable payload (about 2 s at the default poll interval; longer in
/// practice, since a failed READ waits for its timeout). Long enough that a
/// cart still sliding in is read properly first.
pub const UNREADABLE_AFTER: u32 = 10;
/// Consecutive polls without a tag before a failing tag's count is forgotten;
/// matches the default debounce removal threshold.
const FORGET_AFTER_MISSES: u32 = 5;

/// True for errors that mean the reader is gone (port I/O), as opposed to a
/// transient hiccup that should count as a missed poll.
pub fn is_fatal(e: &Pn532Error) -> bool {
    matches!(e, Pn532Error::Io(_))
}

/// Decodes NTAG user memory into the first usable NDEF record.
pub fn decode(mem: &[u8]) -> Result<NdefRecord, NdefError> {
    ndef::parse_user_memory(mem)
}

/// Turns NTAG user memory into a payload; anything unreadable is unusable.
pub fn payload_from_memory(mem: &[u8]) -> Payload {
    match decode(mem) {
        Ok(record) => Payload::parse(record.as_str()),
        Err(e) => {
            tracing::debug!(error = %e, "no usable NDEF record");
            Payload::Unusable
        }
    }
}

/// A PN532 in HSU mode, polled as a [`Reader`].
pub struct Pn532Hsu<T> {
    dev: Pn532<T>,
    /// The tag read on the previous poll; a tag sitting on the reader is not
    /// re-read every poll. Only successful readings are cached.
    cached: Option<Reading>,
    /// The tag whose reads are failing, and how many polls in a row failed.
    failing: Option<(Uid, u32)>,
    /// Consecutive polls without any tag.
    misses: u32,
}

impl<T: Transport> Pn532Hsu<T> {
    /// Wakes the PN532 and configures it for polling.
    pub fn init(mut dev: Pn532<T>) -> Result<Self, Pn532Error> {
        dev.set_timeout(COMMAND_TIMEOUT);
        dev.wakeup()?;
        dev.sam_configuration()?;
        dev.rf_configuration_max_retries(PASSIVE_RETRIES)?;
        Ok(Pn532Hsu {
            dev,
            cached: None,
            failing: None,
            misses: 0,
        })
    }

    /// Looks for a tag. Transient errors are logged and reported as no tag.
    pub fn detect(&mut self) -> Result<Option<Target>, Pn532Error> {
        self.dev
            .in_list_passive_target_106a()
            .or_else(|e| transient(e, "InListPassiveTarget").map(|()| None))
    }

    /// Reads the user memory of the listed tag, four pages per READ, from
    /// page 4 until the NDEF data is complete (see
    /// [`ndef::tlv_area_complete`]) or page 39 has been read. Stopping early
    /// keeps tags with fewer user pages (MIFARE Ultralight, NTAG210) from
    /// failing on a READ past their end.
    pub fn read_user_memory(&mut self) -> Result<Vec<u8>, Pn532Error> {
        let mut mem = Vec::with_capacity(USER_MEMORY_LEN);
        for page in (FIRST_USER_PAGE..=LAST_USER_PAGE).step_by(4) {
            mem.extend_from_slice(&self.dev.ntag_read(page)?);
            if ndef::tlv_area_complete(&mem) {
                break;
            }
        }
        Ok(mem)
    }

    /// Releases the listed tag. Transient errors are logged and ignored.
    pub fn release(&mut self) -> Result<(), Pn532Error> {
        self.dev.in_release().or_else(|e| transient(e, "InRelease"))
    }

    /// Returns the driver.
    pub fn into_inner(self) -> Pn532<T> {
        self.dev
    }
}

impl<T: Transport> Reader for Pn532Hsu<T> {
    /// Always fatal (see [`is_fatal`]); transient errors are missed polls.
    type Error = Pn532Error;

    fn poll(&mut self) -> Result<Option<Reading>, Pn532Error> {
        let Some(target) = self.detect()? else {
            self.cached = None;
            self.misses = self.misses.saturating_add(1);
            if self.misses >= FORGET_AFTER_MISSES {
                self.failing = None;
            }
            return Ok(None);
        };
        self.misses = 0;
        let reading = match Uid::new(&target.uid) {
            Err(e) => {
                tracing::debug!(error = %e, "ignoring target");
                None
            }
            Ok(uid) => match &self.cached {
                Some(cached) if cached.0 == uid => Some(cached.clone()),
                _ if target.sak != TYPE2_SAK => Some((uid, Payload::Unusable)),
                _ => self.read_payload(uid)?,
            },
        };
        self.release()?;
        // A failing tag (including the unreadable fallback) is not cached, so
        // its reads keep being retried.
        let failing =
            matches!((&self.failing, &reading), (Some((f, _)), Some((uid, _))) if f == uid);
        self.cached = if failing { None } else { reading.clone() };
        Ok(reading)
    }
}

impl<T: Transport> Pn532Hsu<T> {
    /// A failed read is a missed poll and is retried on the next one, until
    /// the same tag has failed [`UNREADABLE_AFTER`] polls in a row; from then
    /// on each failed read reports it with an unusable payload, so it stays
    /// present for the debouncer (one `Inserted`, then `Removed` once gone).
    fn read_payload(&mut self, uid: Uid) -> Result<Option<Reading>, Pn532Error> {
        let e = match self.read_user_memory() {
            Ok(mem) => {
                self.failing = None;
                return Ok(Some((uid, payload_from_memory(&mem))));
            }
            Err(e) => e,
        };
        transient(e, "NTAG READ")?;
        let failures = match self.failing {
            Some((failing, n)) if failing == uid => n.saturating_add(1),
            _ => 1,
        };
        self.failing = Some((uid, failures));
        if failures < UNREADABLE_AFTER {
            return Ok(None);
        }
        if failures == UNREADABLE_AFTER {
            tracing::warn!(%uid, "tag detected but its pages cannot be read; treating it as unusable");
        }
        Ok(Some((uid, Payload::Unusable)))
    }
}

/// Passes fatal errors through; logs the rest at debug level.
fn transient(e: Pn532Error, what: &str) -> Result<(), Pn532Error> {
    if is_fatal(&e) {
        return Err(e);
    }
    tracing::debug!(error = %e, "{what} failed; treating as a missed poll");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::debounce::{DebounceConfig, Debouncer, Event};
    use crate::pn532::frame::ACK;
    use std::collections::VecDeque;
    use std::io::{self, ErrorKind, Read, Write};
    use std::time::Instant;

    /// Replays canned replies; `None` is a port I/O error. Runs dry → timeouts.
    struct Mock(VecDeque<Option<Vec<u8>>>);

    impl Read for Mock {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.pop_front() {
                Some(Some(chunk)) => {
                    buf[..chunk.len()].copy_from_slice(&chunk);
                    Ok(chunk.len())
                }
                Some(None) => Err(ErrorKind::BrokenPipe.into()),
                None => Err(ErrorKind::TimedOut.into()),
            }
        }
    }

    impl Write for Mock {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Transport for Mock {}

    /// ACK + response frame with `data` (response code first).
    fn reply(data: &[u8]) -> [Option<Vec<u8>>; 2] {
        let len = data.len() as u8 + 1;
        let sum = data.iter().fold(0xD5u8, |a, b| a.wrapping_add(*b));
        let mut f = vec![0x00, 0x00, 0xFF, len, len.wrapping_neg(), 0xD5];
        f.extend_from_slice(data);
        f.extend([sum.wrapping_neg(), 0x00]);
        [Some(ACK.to_vec()), Some(f)]
    }

    const UID: [u8; 7] = [0x04, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0x80];

    fn list_tag() -> [Option<Vec<u8>>; 2] {
        reply(&[[0x4B, 1, 1, 0x00, 0x44, 0x00, 7].as_slice(), &UID].concat())
    }

    /// Init replies, then `polls`.
    fn reader(polls: Vec<Option<Vec<u8>>>) -> Pn532Hsu<Mock> {
        let mut script: Vec<_> = [reply(&[0x15]), reply(&[0x33])].concat();
        script.extend(polls);
        let mut dev = Pn532::new(Mock(script.into()));
        dev.set_timeout(Duration::from_millis(20));
        let mut r = Pn532Hsu::init(dev).unwrap();
        r.dev.set_timeout(Duration::from_millis(20));
        r
    }

    /// Text record "steam:440" in an NDEF TLV, padded to 144 bytes.
    fn memory() -> Vec<u8> {
        text_memory("steam:440")
    }

    /// Text record `text` in an NDEF TLV, padded to 144 bytes.
    fn text_memory(text: &str) -> Vec<u8> {
        let text = text.as_bytes();
        let rec = [
            &[0xD1, 0x01, 3 + text.len() as u8, b'T', 0x02, b'e', b'n'],
            text,
        ]
        .concat();
        let mut mem = [&[0x03, rec.len() as u8][..], &rec, &[0xFE]].concat();
        mem.resize(USER_MEMORY_LEN, 0);
        mem
    }

    /// READ replies for the 4-page chunks the reader asks for: up to the
    /// one that completes the NDEF data.
    fn read_replies(mem: &[u8]) -> Vec<Option<Vec<u8>>> {
        let chunks = (1..=mem.len().div_ceil(16))
            .find(|&n| ndef::tlv_area_complete(&mem[..(n * 16).min(mem.len())]))
            .unwrap_or(mem.len().div_ceil(16));
        mem.chunks(16)
            .take(chunks)
            .flat_map(|c| reply(&[&[0x41, 0x00], c].concat()))
            .collect()
    }

    /// One poll of a present tag whose first READ fails.
    fn failed_read_poll() -> Vec<Option<Vec<u8>>> {
        let mut poll = list_tag().to_vec();
        poll.extend(reply(&[0x41, 0x01])); // READ status: timeout
        poll.extend(reply(&[0x53, 0x00]));
        poll
    }

    /// Every scripted reply was consumed: no command more or less was sent.
    fn assert_drained(r: Pn532Hsu<Mock>) {
        assert!(
            r.into_inner().into_inner().0.is_empty(),
            "replies left over"
        );
    }

    #[test]
    fn reads_tag_then_uses_cache_then_sees_removal() {
        let mut polls = list_tag().to_vec();
        polls.extend(read_replies(&memory()));
        polls.extend(reply(&[0x53, 0x00]));
        polls.extend(list_tag()); // same tag: no re-read
        polls.extend(reply(&[0x53, 0x00]));
        polls.extend(reply(&[0x4B, 0x00])); // gone
        let mut r = reader(polls);
        let want = Some((Uid::new(&UID).unwrap(), Payload::Steam(440)));
        assert_eq!(r.poll().unwrap(), want);
        assert_eq!(r.poll().unwrap(), want);
        assert_eq!(r.poll().unwrap(), None);
        assert_drained(r);
    }

    #[test]
    fn stops_reading_once_the_ndef_data_is_complete() {
        // "steam:440" ends in the second chunk: two READs, not nine.
        let mut polls = list_tag().to_vec();
        polls.extend(read_replies(&memory()));
        polls.extend(reply(&[0x53, 0x00]));
        assert_eq!(read_replies(&memory()).len(), 2 * 2);
        let mut r = reader(polls);
        assert_eq!(r.poll().unwrap().unwrap().1, Payload::Steam(440));
        assert_drained(r);
    }

    #[test]
    fn small_tag_launches_when_reads_past_its_end_would_fail() {
        // MIFARE Ultralight: 12 user pages (4..=15). The message fills them,
        // so a READ of page 16 would NAK; it must never be sent.
        // 9 bytes of TLV/record framing + 39 of text = exactly 48 bytes.
        let text = "id:a-slug-long-enough-to-fill-12-pages1";
        assert_eq!(text.len(), 39);
        let mem = text_memory(text);
        let mem = &mem[..48];
        assert!(ndef::tlv_area_complete(mem));
        assert!(!ndef::tlv_area_complete(&mem[..32]));
        let mut polls = list_tag().to_vec();
        polls.extend(read_replies(mem));
        polls.extend(reply(&[0x53, 0x00]));
        let mut r = reader(polls);
        let (_, payload) = r.poll().unwrap().unwrap();
        assert!(matches!(payload, Payload::Id(_)), "{payload:?}");
        assert_drained(r);
    }

    #[test]
    fn unreadable_tag_is_reported_once_and_still_removed() {
        // An NTAG whose pages can never be read (read-protected): after
        // UNREADABLE_AFTER failed polls it is present with an unusable
        // payload, every later poll still tries a READ (nothing is cached),
        // and it debounces as removed once gone.
        const POLLS: u32 = UNREADABLE_AFTER + 20;
        let mut polls = Vec::new();
        for _ in 0..POLLS {
            polls.extend(failed_read_poll());
        }
        let config = DebounceConfig::default();
        for _ in 0..config.miss_threshold {
            polls.extend(reply(&[0x4B, 0x00])); // gone
        }
        let mut r = reader(polls);
        let mut debouncer = Debouncer::new(config);
        let now = Instant::now();
        let uid = Uid::new(&UID).unwrap();
        let mut events = Vec::new();
        for i in 0..POLLS {
            let reading = r.poll().unwrap();
            let want = (i + 1 >= UNREADABLE_AFTER).then_some((uid, Payload::Unusable));
            assert_eq!(reading, want, "poll {i}");
            events.extend(debouncer.update(reading, now));
        }
        for _ in 0..config.miss_threshold {
            events.extend(debouncer.update(r.poll().unwrap(), now));
        }
        let payload = Payload::Unusable;
        assert_eq!(
            events,
            [Event::Inserted { uid, payload }, Event::Removed { uid }]
        );
        assert_drained(r);
    }

    #[test]
    fn unreadable_fallback_is_not_cached() {
        // Reported as unusable, then a read finally succeeds: the real
        // payload is returned (and cached from then on).
        let mut polls = Vec::new();
        for _ in 0..UNREADABLE_AFTER {
            polls.extend(failed_read_poll());
        }
        polls.extend(list_tag());
        polls.extend(read_replies(&memory()));
        polls.extend(reply(&[0x53, 0x00]));
        polls.extend(list_tag()); // cached: no READ
        polls.extend(reply(&[0x53, 0x00]));
        let mut r = reader(polls);
        for _ in 1..UNREADABLE_AFTER {
            assert_eq!(r.poll().unwrap(), None);
        }
        assert_eq!(r.poll().unwrap().unwrap().1, Payload::Unusable);
        assert_eq!(r.poll().unwrap().unwrap().1, Payload::Steam(440));
        assert_eq!(r.poll().unwrap().unwrap().1, Payload::Steam(440));
        assert_drained(r);
    }

    #[test]
    fn blank_tag_is_unusable() {
        let mut polls = list_tag().to_vec();
        polls.extend(read_replies(&[0; USER_MEMORY_LEN]));
        polls.extend(reply(&[0x53, 0x00]));
        let mut r = reader(polls);
        assert_eq!(r.poll().unwrap().unwrap().1, Payload::Unusable);
    }

    #[test]
    fn transient_errors_are_missed_polls() {
        // Silence: NoAck on list.
        let mut r = reader(Vec::new());
        assert_eq!(r.poll().unwrap(), None);
    }

    #[test]
    fn slow_insert_retries_reads_until_one_succeeds() {
        // A cart still sliding in: several READs fail, then one succeeds.
        const FAILED_READS: usize = 6;
        let mut polls = Vec::new();
        for _ in 0..FAILED_READS {
            polls.extend(failed_read_poll());
        }
        polls.extend(list_tag());
        polls.extend(read_replies(&memory()));
        polls.extend(reply(&[0x53, 0x00]));
        polls.extend(list_tag()); // cached from here on
        polls.extend(reply(&[0x53, 0x00]));
        let mut r = reader(polls);
        let mut debouncer = Debouncer::new(DebounceConfig::default());
        let now = Instant::now();
        let mut events = Vec::new();
        for i in 0..FAILED_READS + 2 {
            let reading = r.poll().unwrap();
            assert_eq!(reading.is_some(), i >= FAILED_READS, "poll {i}");
            events.extend(debouncer.update(reading, now));
        }
        let uid = Uid::new(&UID).unwrap();
        let payload = Payload::Steam(440);
        assert_eq!(events, [Event::Inserted { uid, payload }]);
    }

    #[test]
    fn non_type2_target_is_unusable_without_reading() {
        // MIFARE Classic 1K (SAK 08): no READ is sent, only InRelease.
        let mut polls =
            reply(&[[0x4B, 1, 1, 0x00, 0x04, 0x08, 7].as_slice(), &UID].concat()).to_vec();
        polls.extend(reply(&[0x53, 0x00]));
        let mut r = reader(polls);
        let (uid, payload) = r.poll().unwrap().unwrap();
        assert_eq!((uid.as_bytes(), payload), (&UID[..], Payload::Unusable));
    }

    #[test]
    fn port_io_errors_are_fatal() {
        let mut r = reader(vec![None]);
        assert!(matches!(r.poll(), Err(Pn532Error::Io(_))));
    }
}
