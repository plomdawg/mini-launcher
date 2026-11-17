//! NDEF parsing from raw NFC Forum Type 2 (NTAG213) user memory.
//!
//! Input is the tag's user memory starting at page 4. The TLV area is walked to
//! the NDEF Message TLV, its records are parsed, and the first *usable* Well-Known
//! Text (`T`) or URI (`U`) record is decoded to a `String` (see
//! `docs/tag-format.md`). Interpreting that string (`steam://`, `id:`) is not
//! done here.
//!
//! Tag contents are untrusted: every length is bounds-checked and parsing never
//! panics. Malformed structure is an error; a Text/URI record that is UTF-16,
//! not valid UTF-8, or uses a reserved URI prefix is skipped in favour of the
//! next record.
//!
//! [`encode_payload`] is the inverse for the payloads the launcher writes.

use std::fmt;

use crate::payload::Payload;

/// A decoded Text or URI record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NdefRecord {
    /// Well-Known Text record: the text, without status byte or language code.
    Text(String),
    /// Well-Known URI record: the URI with its identifier prefix expanded.
    Uri(String),
}

impl NdefRecord {
    /// The decoded payload string, whichever record type it came from.
    pub fn as_str(&self) -> &str {
        match self {
            NdefRecord::Text(s) | NdefRecord::Uri(s) => s,
        }
    }
}

/// Why no usable record could be read from a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdefError {
    /// The TLV area ended (or hit a terminator) before an NDEF Message TLV.
    NoNdefMessage,
    /// A TLV with a tag value that is not defined for Type 2 tags.
    UnknownTlv(u8),
    /// A TLV length field or value runs past the end of the buffer.
    TruncatedTlv,
    /// A record header, type, ID or payload runs past the end of the message.
    TruncatedRecord,
    /// The message has no usable Text or URI record.
    NoUsableRecord,
}

impl fmt::Display for NdefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NdefError::NoNdefMessage => f.write_str("no NDEF message TLV on tag"),
            NdefError::UnknownTlv(t) => write!(f, "unknown TLV tag 0x{t:02X}"),
            NdefError::TruncatedTlv => f.write_str("TLV runs past end of tag memory"),
            NdefError::TruncatedRecord => f.write_str("NDEF record runs past end of message"),
            NdefError::NoUsableRecord => f.write_str("no usable Text or URI record"),
        }
    }
}

impl std::error::Error for NdefError {}

const TLV_NULL: u8 = 0x00;
const TLV_LOCK_CONTROL: u8 = 0x01;
const TLV_MEMORY_CONTROL: u8 = 0x02;
const TLV_NDEF_MESSAGE: u8 = 0x03;
const TLV_PROPRIETARY: u8 = 0xFD;
const TLV_TERMINATOR: u8 = 0xFE;

const FLAG_ME: u8 = 0x40;
const FLAG_CF: u8 = 0x20;
const FLAG_SR: u8 = 0x10;
const FLAG_IL: u8 = 0x08;
const TNF_MASK: u8 = 0x07;
const TNF_WELL_KNOWN: u8 = 0x01;

/// NFC Forum URI Record Type Definition identifier codes `0x00`–`0x23`.
const URI_PREFIXES: [&str; 0x24] = [
    "",
    "http://www.",
    "https://www.",
    "http://",
    "https://",
    "tel:",
    "mailto:",
    "ftp://anonymous:anonymous@",
    "ftp://ftp.",
    "ftps://",
    "sftp://",
    "smb://",
    "nfs://",
    "ftp://",
    "dav://",
    "news:",
    "telnet://",
    "imap:",
    "rtsp://",
    "urn:",
    "pop:",
    "sip:",
    "sips:",
    "tftp:",
    "btspp://",
    "btl2cap://",
    "btgoep://",
    "tcpobex://",
    "irdaobex://",
    "file://",
    "urn:epc:id:",
    "urn:epc:tag:",
    "urn:epc:pat:",
    "urn:epc:raw:",
    "urn:epc:",
    "urn:nfc:",
];

/// Reads the first usable Text or URI record from Type 2 tag user memory
/// (bytes starting at page 4).
pub fn parse_user_memory(mem: &[u8]) -> Result<NdefRecord, NdefError> {
    first_usable_record(find_ndef_message(mem)?)
}

/// Walks the TLV area and returns the value of the first NDEF Message TLV.
pub fn find_ndef_message(mem: &[u8]) -> Result<&[u8], NdefError> {
    match walk_tlvs(mem) {
        Walk::Message(msg) => Ok(msg),
        Walk::Terminator | Walk::OutOfBytes => Err(NdefError::NoNdefMessage),
        Walk::Error(e) => Err(e),
    }
}

/// True when reading more memory after `mem` cannot change what
/// [`parse_user_memory`] returns: the TLV area holds a whole NDEF Message
/// TLV, hits a terminator, or is already malformed. False while it is all
/// padding so far or a TLV runs past the end. Lets a reader stop early, so a
/// tag with fewer user pages than an NTAG213 is never read past its end.
pub fn tlv_area_complete(mem: &[u8]) -> bool {
    match walk_tlvs(mem) {
        Walk::Message(_) | Walk::Terminator => true,
        Walk::OutOfBytes | Walk::Error(NdefError::TruncatedTlv) => false,
        Walk::Error(_) => true,
    }
}

/// Where a walk over the TLV area stopped.
enum Walk<'a> {
    /// The value of the first NDEF Message TLV.
    Message(&'a [u8]),
    /// A Terminator TLV before any NDEF Message TLV.
    Terminator,
    /// The buffer ended between TLVs (e.g. in padding).
    OutOfBytes,
    /// Malformed or truncated structure.
    Error(NdefError),
}

fn walk_tlvs(mem: &[u8]) -> Walk<'_> {
    let mut pos = 0;
    while let Some(&tag) = mem.get(pos) {
        pos += 1;
        match tag {
            TLV_NULL => continue,
            TLV_TERMINATOR => return Walk::Terminator,
            TLV_LOCK_CONTROL | TLV_MEMORY_CONTROL | TLV_NDEF_MESSAGE | TLV_PROPRIETARY => {}
            other => return Walk::Error(NdefError::UnknownTlv(other)),
        }
        let len = match mem.get(pos) {
            None => return Walk::Error(NdefError::TruncatedTlv),
            Some(0xFF) => {
                let Some(bytes) = mem.get(pos + 1..pos + 3) else {
                    return Walk::Error(NdefError::TruncatedTlv);
                };
                pos += 3;
                usize::from(u16::from_be_bytes([bytes[0], bytes[1]]))
            }
            Some(&n) => {
                pos += 1;
                usize::from(n)
            }
        };
        let Some(value) = pos.checked_add(len).and_then(|end| mem.get(pos..end)) else {
            return Walk::Error(NdefError::TruncatedTlv);
        };
        if tag == TLV_NDEF_MESSAGE {
            return Walk::Message(value);
        }
        pos += len;
    }
    Walk::OutOfBytes
}

/// Parses the records of an NDEF message and decodes the first usable one.
pub fn first_usable_record(msg: &[u8]) -> Result<NdefRecord, NdefError> {
    let mut rest = msg;
    while !rest.is_empty() {
        let (record, tail) = split_record(rest)?;
        let header = record.header;
        if header & TNF_MASK == TNF_WELL_KNOWN && header & FLAG_CF == 0 {
            let decoded = match record.record_type {
                b"T" => decode_text(record.payload).map(NdefRecord::Text),
                b"U" => decode_uri(record.payload).map(NdefRecord::Uri),
                _ => None,
            };
            if let Some(record) = decoded {
                return Ok(record);
            }
        }
        if header & FLAG_ME != 0 {
            break;
        }
        rest = tail;
    }
    Err(NdefError::NoUsableRecord)
}

/// One raw NDEF record, borrowed from the message.
struct RawRecord<'a> {
    header: u8,
    record_type: &'a [u8],
    payload: &'a [u8],
}

/// Splits one record off the front of `buf`, returning it and the remaining bytes.
fn split_record(buf: &[u8]) -> Result<(RawRecord<'_>, &[u8]), NdefError> {
    let mut r = Reader(buf);
    let header = r.byte()?;
    let type_len = usize::from(r.byte()?);
    let payload_len = if header & FLAG_SR != 0 {
        usize::from(r.byte()?)
    } else {
        let b = r.take(4)?;
        usize::try_from(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
            .map_err(|_| NdefError::TruncatedRecord)?
    };
    let id_len = if header & FLAG_IL != 0 {
        usize::from(r.byte()?)
    } else {
        0
    };
    let record_type = r.take(type_len)?;
    r.take(id_len)?;
    let payload = r.take(payload_len)?;
    let record = RawRecord {
        header,
        record_type,
        payload,
    };
    Ok((record, r.0))
}

/// Bounds-checked cursor over a record.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, NdefError> {
        Ok(self.take(1)?[0])
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], NdefError> {
        if n > self.0.len() {
            return Err(NdefError::TruncatedRecord);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
}

/// Decodes a Text record payload; `None` if it is UTF-16, truncated or not UTF-8.
fn decode_text(payload: &[u8]) -> Option<String> {
    let (&status, rest) = payload.split_first()?;
    if status & 0x80 != 0 {
        return None; // UTF-16 is not supported (docs/tag-format.md).
    }
    let lang_len = usize::from(status & 0x3F);
    let text = rest.get(lang_len..)?;
    String::from_utf8(text.to_vec()).ok()
}

/// Decodes a URI record payload; `None` for a reserved prefix code or bad UTF-8.
fn decode_uri(payload: &[u8]) -> Option<String> {
    let (&code, rest) = payload.split_first()?;
    let prefix = URI_PREFIXES.get(usize::from(code))?;
    let rest = std::str::from_utf8(rest).ok()?;
    Some(format!("{prefix}{rest}"))
}

/// Encodes a payload as Type 2 tag user memory: an NDEF Message TLV with one
/// record, then a Terminator TLV. `steam://<appid>` becomes a URI record
/// (prefix code `0x00`, no abbreviation exists), `id:<slug>` a UTF-8 Text
/// record with language `en`. Built only from the parsed appid / validated
/// slug, in canonical form; `None` for [`Payload::Unusable`].
pub fn encode_payload(payload: &Payload) -> Option<Vec<u8>> {
    let (record_type, body) = match payload {
        Payload::Steam(appid) => (
            b'U',
            [&[0x00][..], format!("steam://{appid}").as_bytes()].concat(),
        ),
        Payload::Id(slug) => (b'T', [b"\x02en", format!("id:{slug}").as_bytes()].concat()),
        Payload::Unusable => return None,
    };
    // A 64-char slug gives a 70-byte payload: short records and a 1-byte TLV
    // length always suffice.
    let payload_len = u8::try_from(body.len()).ok()?;
    let header = 0x80 | FLAG_ME | FLAG_SR | TNF_WELL_KNOWN; // MB|ME|SR, TNF 1
    let mut msg = vec![header, 0x01, payload_len, record_type];
    msg.extend_from_slice(&body);
    let msg_len = u8::try_from(msg.len()).ok().filter(|&n| n < 0xFF)?;
    let mut out = vec![TLV_NDEF_MESSAGE, msg_len];
    out.extend_from_slice(&msg);
    out.push(TLV_TERMINATOR);
    Some(out)
}

#[cfg(test)]
mod tests;
