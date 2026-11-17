use super::*;
use crate::payload::Slug;
use crate::pn532::frame::{ACK, NACK};
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::time::Duration;

const UID: [u8; 7] = [0x04, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0x80];
const NTAG213: [u8; 8] = [0x00, 0x04, 0x04, 0x02, 0x01, 0x00, 0x0F, 0x03];
const NTAG215: [u8; 8] = [0x00, 0x04, 0x04, 0x02, 0x01, 0x00, 0x11, 0x03];
const MF0UL11: [u8; 8] = [0x00, 0x04, 0x03, 0x01, 0x01, 0x00, 0x0B, 0x03];

/// A PN532 with an emulated NTAG213 in its field: decodes each request
/// frame and answers like the hardware would.
struct Tag {
    mem: Vec<u8>,
    /// Last user page: a WRITE past it fails the test.
    last_user: u8,
    /// `GET_VERSION` reply; `None` NAKs it (and drops the tag to IDLE).
    version: Option<[u8; 8]>,
    present: bool,
    /// Selected by the last `InListPassiveTarget` (not IDLE after a NAK).
    selected: bool,
    sak: u8,
    /// Every WRITE the tag received, in order.
    writes: Vec<(u8, [u8; 4])>,
    /// Remove the tag after this many more WRITEs.
    remove_after: Option<usize>,
    /// A memory byte that silently keeps its old value.
    stuck: Option<usize>,
    out: VecDeque<u8>,
}

impl Tag {
    fn ntag213() -> Tag {
        Tag::new(45, 39, 0x12, Some(NTAG213))
    }

    /// `pages` in total, user pages 4..=`last_user`, CC data area size `cc`.
    fn new(pages: usize, last_user: u8, cc: u8, version: Option<[u8; 8]>) -> Tag {
        let mut mem = vec![0; pages * 4];
        mem[..7].copy_from_slice(&UID[..7]);
        mem[8..16].copy_from_slice(&[0x80, 0x48, 0x00, 0x00, 0xE1, 0x10, cc, 0x00]);
        // Dynamic lock (unlocked) + config pages: must stay as-is.
        let cfg = (usize::from(last_user) + 1) * 4;
        mem[cfg..].fill(0xBD);
        if version.is_some_and(|v| v[6] != 0x0B) {
            mem[cfg..cfg + 3].fill(0);
        }
        Tag {
            mem,
            last_user,
            version,
            present: true,
            selected: true,
            sak: 0x00,
            writes: Vec::new(),
            remove_after: None,
            stuck: None,
            out: VecDeque::new(),
        }
    }

    fn respond(&mut self, data: &[u8]) -> Vec<u8> {
        let list = |tag: &Tag| {
            if tag.present {
                [&[0x4B, 1, 1, 0x00, 0x44, tag.sak, 7][..], &UID].concat()
            } else {
                vec![0x4B, 0]
            }
        };
        match *data {
            [0x4A, ..] => {
                self.selected = self.present;
                list(self)
            }
            [0x52, ..] => vec![0x53, 0x00],
            [0x40, _, ..] if !self.present || !self.selected => vec![0x41, 0x01],
            [0x40, _, 0x60] => match self.version {
                Some(v) => [&[0x41, 0x00][..], &v].concat(),
                None => {
                    self.selected = false;
                    vec![0x41, 0x01]
                }
            },
            [0x40, _, 0x30, page] => {
                let start = usize::from(page) * 4;
                let bytes = (0..16).map(|i| self.mem[(start + i) % self.mem.len()]);
                [0x41, 0x00].into_iter().chain(bytes).collect()
            }
            [0x40, _, 0xA2, page, a, b, c, d] => {
                assert!((4..=self.last_user).contains(&page), "WRITE page {page}");
                self.writes.push((page, [a, b, c, d]));
                let start = usize::from(page) * 4;
                for (i, v) in [a, b, c, d].into_iter().enumerate() {
                    if self.stuck != Some(start + i) {
                        self.mem[start + i] = v;
                    }
                }
                if let Some(n) = self.remove_after.as_mut() {
                    *n -= 1;
                    self.present = *n > 0;
                }
                vec![0x41, 0x00]
            }
            _ => panic!("unexpected command {data:02X?}"),
        }
    }

    fn user(&self) -> &[u8] {
        &self.mem[16..(usize::from(self.last_user) + 1) * 4]
    }
}

impl Write for Tag {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf != ACK && buf != NACK {
            let len = usize::from(buf[3]);
            assert_eq!(buf[5], 0xD4);
            let resp = self.respond(&buf[6..5 + len]);
            let n = resp.len() as u8 + 1;
            let sum = resp.iter().fold(0xD5u8, |a, b| a.wrapping_add(*b));
            self.out.extend(ACK);
            self.out
                .extend([0x00, 0x00, 0xFF, n, n.wrapping_neg(), 0xD5]);
            self.out.extend(resp);
            self.out.extend([sum.wrapping_neg(), 0x00]);
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Read for Tag {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.out.is_empty() {
            return Err(ErrorKind::TimedOut.into());
        }
        let n = buf.len().min(self.out.len());
        for (b, v) in buf.iter_mut().zip(self.out.drain(..n)) {
            *b = v;
        }
        Ok(n)
    }
}

impl Transport for Tag {}

/// Lists the tag and writes `payload`; returns the result and the tag.
fn write(tag: Tag, payload: &Payload) -> (Result<WriteOutcome, WriteError>, Tag) {
    let mut dev = Pn532::new(tag);
    dev.set_timeout(Duration::from_millis(20));
    let target = dev.in_list_passive_target_106a().unwrap().unwrap();
    let result = write_payload(&mut dev, &target, payload);
    (result, dev.into_inner())
}

fn id(s: &str) -> Payload {
    Payload::Id(Slug::new(s).unwrap())
}

#[test]
fn writes_then_verifies() {
    for payload in [Payload::Steam(440), id(&"a".repeat(64))] {
        let (result, tag) = write(Tag::ntag213(), &payload);
        let mem = ndef::encode_payload(&payload).unwrap();
        let last = 3 + mem.len().div_ceil(4) as u8;
        assert_eq!(
            result.unwrap(),
            WriteOutcome {
                first_page: 4,
                last_page: last
            }
        );
        assert_eq!(tag.user()[..mem.len()], mem[..]);
        let read = ndef::parse_user_memory(tag.user()).unwrap();
        assert_eq!(Payload::parse(read.as_str()), payload);
        // Page 4 with TLV length 0 first, the rest in order, page 4 last.
        let pages: Vec<u8> = tag.writes.iter().map(|w| w.0).collect();
        let want: Vec<u8> = [4].into_iter().chain(5..=last).chain([4]).collect();
        assert_eq!(pages, want);
        assert_eq!(tag.writes[0].1, [0x03, 0x00, mem[2], mem[3]]);
        assert_eq!(tag.writes.last().unwrap().1, mem[..4]);
        assert!(tag.mem[40 * 4 + 3..].iter().all(|&b| b == 0xBD));
    }
}

#[test]
fn overwrites_a_longer_payload() {
    let (result, tag) = write(Tag::ntag213(), &id(&"x".repeat(60)));
    result.unwrap();
    let (result, tag) = write(tag, &Payload::Steam(7));
    result.unwrap();
    let read = ndef::parse_user_memory(tag.user()).unwrap();
    assert_eq!(Payload::parse(read.as_str()), Payload::Steam(7));
}

#[test]
fn refuses_locked_unknown_and_small_tags_without_writing() {
    let mut read_only = Tag::ntag213();
    read_only.mem[15] = 0x0F;
    let mut lock_bits = Tag::ntag213();
    lock_bits.mem[10] = 0x10; // static lock bit for page 4
    let mut no_ndef = Tag::ntag213();
    no_ndef.mem[12] = 0x00;
    let mut classic = Tag::ntag213();
    classic.sak = 0x08;
    let mut small = Tag::ntag213();
    small.mem[14] = 0x02; // 16-byte data area: pages 4..=7
    let cases = [read_only, lock_bits, no_ndef, classic, small];
    let results = cases.map(|t| write(t, &Payload::Steam(440)));
    for (i, (result, tag)) in results.iter().enumerate() {
        let ok = match i {
            0 | 1 => matches!(result, Err(WriteError::Locked)),
            2 | 3 => matches!(result, Err(WriteError::UnknownTag)),
            _ => matches!(result, Err(WriteError::TooSmall { need: 19, have: 16 })),
        };
        assert!(ok, "case {i}: {result:?}");
        assert!(tag.writes.is_empty(), "case {i} wrote");
    }
    let (result, tag) = write(Tag::ntag213(), &Payload::Unusable);
    assert!(matches!(result, Err(WriteError::Unusable)));
    assert!(tag.writes.is_empty());
}

#[test]
fn removal_mid_write_is_reported_and_leaves_no_payload() {
    for after in 1..=4 {
        let mut tag = Tag::ntag213();
        tag.remove_after = Some(after);
        let (result, tag) = write(tag, &Payload::Steam(440));
        assert!(matches!(result, Err(WriteError::TagRemoved)), "{result:?}");
        assert_eq!(tag.writes.len(), after);
        // Torn: the TLV length is still 0, so the tag reads as unusable.
        assert!(ndef::parse_user_memory(tag.user()).is_err());
    }
}

#[test]
fn read_back_mismatch_fails_verify() {
    let mut tag = Tag::ntag213();
    tag.stuck = Some(16 + 10); // a byte inside the URI
    let (result, _) = write(tag, &Payload::Steam(440));
    assert!(
        matches!(result, Err(WriteError::VerifyFailed)),
        "{result:?}"
    );
}

#[test]
fn cc_size_is_capped_by_the_product() {
    let head = |size: u8| {
        let mut h = [0; 16];
        h[4..8].copy_from_slice(&[0xE1, 0x10, size, 0x00]);
        h
    };
    let end = |size, version| writable_end(&head(size), product(&version).last_user_page);
    assert_eq!(end(0x12, NTAG213).unwrap(), 39);
    assert_eq!(end(0xFF, NTAG213).unwrap(), 39);
    assert_eq!(end(0x16, MF0UL11).unwrap(), 15); // OTP-inflated CC
    assert_eq!(end(0x3E, NTAG215).unwrap(), 127);
    assert_eq!(end(0xFF, NTAG215).unwrap(), 129);
    assert_eq!(end(0xFF, [0; 8]).unwrap(), 15); // unknown product
    assert!(matches!(
        end(0x00, NTAG213).and_then(|end| plan(&[1], end)),
        Err(WriteError::TooSmall { need: 1, have: 0 })
    ));
    let mut h = head(0x12);
    h[7] = 0x80; // read access restricted
    assert!(matches!(writable_end(&h, 39), Err(WriteError::UnknownTag)));
    assert_eq!(
        plan(&[1, 2, 3, 4, 5], 39).unwrap(),
        [[1, 2, 3, 4], [5, 0, 0, 0]]
    );
}

#[test]
fn inflated_cc_on_a_small_tag_never_reaches_config_pages() {
    // Ultralight EV1 (20 pages, CFG/PWD at 16-19) whose OTP CC was bumped
    // to 0x16 (47 pages). A long slug would reach page 16: refused instead,
    // with or without GET_VERSION.
    let slug = id(&"a".repeat(64));
    for version in [Some(MF0UL11), None] {
        let (result, tag) = write(Tag::new(20, 15, 0x16, version), &slug);
        assert!(
            matches!(result, Err(WriteError::TooSmall { need: 77, have: 48 })),
            "{version:?}: {result:?}"
        );
        assert!(tag.writes.is_empty());
        // A payload that fits is written, also after re-selecting on a NAK.
        let (result, tag) = write(tag, &Payload::Steam(440));
        assert_eq!(result.unwrap().last_page, 8, "{version:?}");
        assert!(tag.mem[64..].iter().all(|&b| b == 0xBD));
    }
}

#[test]
fn ntag215_uses_its_larger_memory() {
    let (result, tag) = write(
        Tag::new(135, 129, 0x3E, Some(NTAG215)),
        &id(&"b".repeat(64)),
    );
    assert_eq!(result.unwrap().last_page, 23);
    assert!(tag.writes.iter().all(|w| w.0 <= 23));
}

#[test]
fn dynamic_lock_bits_refuse_before_writing() {
    let slug = id(&"c".repeat(64)); // pages 4-23
                                    // Lock bit for pages 16-17: refused, old contents kept.
    let mut tag = Tag::ntag213();
    tag.mem[40 * 4] = 0x01;
    let (result, tag) = write(tag, &slug);
    assert!(matches!(result, Err(WriteError::Locked)), "{result:?}");
    assert!(tag.writes.is_empty());
    // The same tag still takes a payload that stays below page 16.
    let (result, mut tag) = write(tag, &Payload::Steam(440));
    result.unwrap();
    // Only pages 38-39 locked: not in the way.
    tag.mem[40 * 4] = 0x00;
    tag.mem[40 * 4 + 1] = 0x08;
    let (result, _) = write(tag, &slug);
    result.unwrap();
    // Unknown granularity: any bit counts once we reach page 16.
    assert!(dynamically_locked([0x00, 0x80], None, 4, 16));
    assert!(!dynamically_locked([0xFF, 0xFF], None, 4, 15));
    assert!(dynamically_locked([0x80, 0x00], Some(2), 4, 30));
    assert!(!dynamically_locked([0x80, 0x00], Some(2), 4, 29));
    assert!(dynamically_locked([0x02, 0x00], Some(16), 4, 32));
}
