//! [`Pn532Hsu`]: the hardware [`Reader`] on top of the PN532 driver.
//!
//! Each poll lists one ISO/IEC 14443-A target, reads the NTAG213 user memory
//! (pages 4..=39), decodes it into a [`Payload`] and releases the target.
//! Flaky radio or serial traffic (timeouts, stray frames, a tag pulled
//! mid-read) counts as a missed poll; the debouncer absorbs it. A failed read
//! is retried on every poll while the tag stays present, so a cart that is
//! still sliding in is reported once a read succeeds. Only port I/O
//! errors (adapter unplugged) are returned, and those are fatal.

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
}

impl<T: Transport> Pn532Hsu<T> {
    /// Wakes the PN532 and configures it for polling.
    pub fn init(mut dev: Pn532<T>) -> Result<Self, Pn532Error> {
        dev.set_timeout(COMMAND_TIMEOUT);
        dev.wakeup()?;
        dev.sam_configuration()?;
        dev.rf_configuration_max_retries(PASSIVE_RETRIES)?;
        Ok(Pn532Hsu { dev, cached: None })
    }

    /// Looks for a tag. Transient errors are logged and reported as no tag.
    pub fn detect(&mut self) -> Result<Option<Target>, Pn532Error> {
        self.dev
            .in_list_passive_target_106a()
            .or_else(|e| transient(e, "InListPassiveTarget").map(|()| None))
    }

    /// Reads the user memory (pages 4..=39) of the listed tag.
    pub fn read_user_memory(&mut self) -> Result<Vec<u8>, Pn532Error> {
        let mut mem = Vec::with_capacity(USER_MEMORY_LEN);
        for page in (FIRST_USER_PAGE..=LAST_USER_PAGE).step_by(4) {
            mem.extend_from_slice(&self.dev.ntag_read(page)?);
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
            return Ok(None);
        };
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
        self.cached.clone_from(&reading);
        Ok(reading)
    }
}

impl<T: Transport> Pn532Hsu<T> {
    /// A failed read is a missed poll; it is retried on the next one.
    fn read_payload(&mut self, uid: Uid) -> Result<Option<Reading>, Pn532Error> {
        match self.read_user_memory() {
            Ok(mem) => Ok(Some((uid, payload_from_memory(&mem)))),
            Err(e) => transient(e, "NTAG READ").map(|()| None),
        }
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
        let text = b"steam:440";
        let rec = [
            &[0xD1, 0x01, 3 + text.len() as u8, b'T', 0x02, b'e', b'n'],
            &text[..],
        ]
        .concat();
        let mut mem = [&[0x03, rec.len() as u8][..], &rec, &[0xFE]].concat();
        mem.resize(USER_MEMORY_LEN, 0);
        mem
    }

    fn read_replies(mem: &[u8]) -> Vec<Option<Vec<u8>>> {
        mem.chunks(16)
            .flat_map(|c| reply(&[&[0x41, 0x00], c].concat()))
            .collect()
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
            polls.extend(list_tag());
            polls.extend(reply(&[0x41, 0x01])); // READ status: timeout
            polls.extend(reply(&[0x53, 0x00]));
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
