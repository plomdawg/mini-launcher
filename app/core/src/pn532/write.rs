//! Writing a payload to an NTAG21x (NFC Forum Type 2) tag, with read-back
//! verification.
//!
//! Only user pages are written: from page 4 up to the end of the data area
//! the capability container (page 3) declares, and never past NTAG213's last
//! user page (39), whatever the CC claims. The UID, lock bytes, CC, dynamic
//! lock and config pages are never touched, and nothing is ever locked.
//!
//! Torn writes: page 4 (which holds the NDEF TLV length) is first written
//! with length 0, then the rest of the record, then page 4 again with the
//! real length. A tag pulled mid-write therefore reads as an empty NDEF
//! message (nothing usable, so UID fallback) instead of a half-written one.

use thiserror::Error;

use super::driver::{Pn532, Pn532Error, Target, Transport, FIRST_WRITABLE_PAGE};
use super::reader::{is_fatal, LAST_USER_PAGE, TYPE2_SAK};
use crate::ndef;
use crate::payload::Payload;

/// CC byte 0: NDEF magic number.
const CC_MAGIC: u8 = 0xE1;

/// Why a payload was not written. The GUI falls back to UID binding on
/// [`Locked`](WriteError::Locked), [`TooSmall`](WriteError::TooSmall) and
/// [`UnknownTag`](WriteError::UnknownTag).
#[derive(Debug, Error)]
pub enum WriteError {
    #[error("nothing to write: the payload is not usable")]
    Unusable,
    #[error("not an NDEF-formatted NFC Forum Type 2 tag (e.g. NTAG213)")]
    UnknownTag,
    #[error("the tag is read-only (locked)")]
    Locked,
    #[error("the payload needs {need} bytes but the tag only holds {have}")]
    TooSmall { need: usize, have: usize },
    #[error("the tag was removed before writing finished")]
    TagRemoved,
    #[error("the tag did not read back what was written")]
    VerifyFailed,
    #[error("reader error: {0}")]
    Transport(#[source] Pn532Error),
}

/// A written and verified payload: user pages `first_page..=last_page`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOutcome {
    pub first_page: u8,
    pub last_page: u8,
}

/// The last page that may be written, from pages 2..=5 as one READ returns
/// them (page 2 holds the static lock bytes, page 3 the CC).
pub fn writable_end(head: &[u8; 16]) -> Result<u8, WriteError> {
    let (lock, cc) = ([head[2], head[3]], &head[4..8]);
    if cc[0] != CC_MAGIC || cc[1] >> 4 != 1 {
        return Err(WriteError::UnknownTag);
    }
    // CC byte 3: read access (high nibble) and write access (low nibble);
    // anything but 0x00 is not plain read/write. Static lock bits for pages
    // 4-15 (lock byte 0 bits 4-7, lock byte 1) are refused conservatively.
    if cc[3] != 0x00 || lock[0] & 0xF0 != 0 || lock[1] != 0 {
        return Err(WriteError::Locked);
    }
    // Data area = CC[2] * 8 bytes = CC[2] * 2 pages after page 3.
    let end = 3 + u16::from(cc[2]) * 2;
    Ok(end.min(u16::from(LAST_USER_PAGE)) as u8)
}

/// Splits encoded user memory into 4-byte pages starting at page 4, padded
/// with zeros, or fails if they don't fit up to `last_page`.
pub fn plan(mem: &[u8], last_page: u8) -> Result<Vec<[u8; 4]>, WriteError> {
    let have = usize::from(last_page.saturating_sub(FIRST_WRITABLE_PAGE - 1)) * 4;
    if mem.is_empty() || mem.len() > have {
        return Err(WriteError::TooSmall {
            need: mem.len(),
            have,
        });
    }
    Ok(mem
        .chunks(4)
        .map(|c| {
            let mut page = [0; 4];
            page[..c.len()].copy_from_slice(c);
            page
        })
        .collect())
}

/// Writes `payload` to `target`, which must be the tag just listed by
/// `InListPassiveTarget`, then re-activates it and reads the pages back.
pub fn write_payload<T: Transport>(
    dev: &mut Pn532<T>,
    target: &Target,
    payload: &Payload,
) -> Result<WriteOutcome, WriteError> {
    let mem = ndef::encode_payload(payload).ok_or(WriteError::Unusable)?;
    if target.sak != TYPE2_SAK {
        return Err(WriteError::UnknownTag);
    }
    let head = dev.ntag_read(2).map_err(|e| failed(dev, target, e))?;
    let pages = plan(&mem, writable_end(&head)?)?;
    let last_page = FIRST_WRITABLE_PAGE + (pages.len() - 1) as u8;

    let mut empty_first = pages[0];
    empty_first[1] = 0; // NDEF TLV length 0 until everything else is written
    let order = std::iter::once((FIRST_WRITABLE_PAGE, empty_first))
        .chain((FIRST_WRITABLE_PAGE..).zip(pages.iter().copied()).skip(1))
        .chain([(FIRST_WRITABLE_PAGE, pages[0])]);
    for (page, data) in order {
        dev.ntag_write(page, data)
            .map_err(|e| failed(dev, target, e))?;
    }

    // Verify from a fresh activation of the same tag, not cached state.
    if let Err(e) = dev.in_release() {
        return Err(failed(dev, target, e));
    }
    match dev.in_list_passive_target_106a() {
        Ok(Some(t)) if t.uid == target.uid => {}
        Ok(_) => return Err(WriteError::TagRemoved),
        Err(e) => return Err(failed(dev, target, e)),
    }
    let mut back = Vec::with_capacity(pages.len() * 4 + 16);
    for page in (FIRST_WRITABLE_PAGE..=last_page).step_by(4) {
        let chunk = dev.ntag_read(page).map_err(|e| failed(dev, target, e))?;
        back.extend_from_slice(&chunk);
    }
    let read_payload = ndef::parse_user_memory(&back).map(|r| Payload::parse(r.as_str()));
    if back[..pages.len() * 4] != *pages.concat() || read_payload.as_ref() != Ok(payload) {
        return Err(WriteError::VerifyFailed);
    }
    Ok(WriteOutcome {
        first_page: FIRST_WRITABLE_PAGE,
        last_page,
    })
}

/// Classifies a failed command: port errors and failures with the tag still
/// present are transport errors; a tag that no longer answers was removed.
fn failed<T: Transport>(dev: &mut Pn532<T>, target: &Target, e: Pn532Error) -> WriteError {
    if is_fatal(&e) {
        return WriteError::Transport(e);
    }
    match dev.in_list_passive_target_106a() {
        Ok(Some(t)) if t.uid == target.uid => WriteError::Transport(e),
        Ok(_) => WriteError::TagRemoved,
        Err(_) => WriteError::Transport(e),
    }
}

#[cfg(test)]
mod tests;
