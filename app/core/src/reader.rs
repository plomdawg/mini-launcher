//! Reader abstraction, tag UIDs and the `Simulated` reader.
//!
//! A [`Reader`] reports the raw state of the field on every poll: the tag that
//! is present right now, or nothing. Turning that into insert/remove events is
//! the job of [`crate::debounce`].

use std::fmt;
use std::io::{BufRead, Read};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::payload::Payload;

/// A tag UID: 4, 7 or 10 bytes (single, double, triple size). NTAG213 is 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Uid {
    bytes: [u8; Uid::MAX_LEN],
    len: u8,
}

impl Uid {
    /// Longest UID defined by ISO/IEC 14443-3.
    pub const MAX_LEN: usize = 10;

    /// Builds a UID from raw bytes; the length must be 4, 7 or 10.
    pub fn new(bytes: &[u8]) -> Result<Uid, UidError> {
        if !matches!(bytes.len(), 4 | 7 | 10) {
            return Err(UidError::BadLength(bytes.len()));
        }
        let mut buf = [0; Uid::MAX_LEN];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(Uid {
            bytes: buf,
            len: bytes.len() as u8,
        })
    }

    /// Parses hex digits (either case, no separators), e.g. `04A1B2C3D4E5F6`.
    pub fn from_hex(hex: &str) -> Result<Uid, UidError> {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(UidError::BadHex);
        }
        if !hex.len().is_multiple_of(2) {
            return Err(UidError::OddLength);
        }
        if !matches!(hex.len(), 8 | 14 | 20) {
            return Err(UidError::BadLength(hex.len() / 2));
        }
        // All ASCII hex digits, so byte-slicing on pairs is safe.
        let mut bytes = [0; Uid::MAX_LEN];
        for (i, b) in bytes.iter_mut().take(hex.len() / 2).enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("validated hex");
        }
        Uid::new(&bytes[..hex.len() / 2])
    }

    /// The UID bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

/// Uppercase hex without separators, e.g. `04A1B2C3D4E5F6`.
impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_bytes()
            .iter()
            .try_for_each(|b| write!(f, "{b:02X}"))
    }
}

/// Why a UID was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UidError {
    /// Contains a character that is not a hex digit.
    BadHex,
    /// Odd number of hex digits.
    OddLength,
    /// Byte length other than 4, 7 or 10.
    BadLength(usize),
}

impl fmt::Display for UidError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UidError::BadHex => f.write_str("UID is not hex"),
            UidError::OddLength => f.write_str("UID has an odd number of hex digits"),
            UidError::BadLength(n) => write!(f, "UID is {n} bytes; expected 4, 7 or 10"),
        }
    }
}

impl std::error::Error for UidError {}

/// One poll's view of the field: the tag present now and its parsed payload.
pub type Reading = (Uid, Payload);

/// A source of raw tag readings, polled at a fixed interval by the caller.
pub trait Reader {
    /// Error type for a failed poll (I/O, framing, bad input).
    type Error: std::error::Error + Send + Sync + 'static;

    /// Reports what is on the reader right now. Must not block for long.
    fn poll(&mut self) -> Result<Option<Reading>, Self::Error>;

    /// True once the reader can never report anything new (e.g. simulated
    /// input reached EOF and every line was applied). Hardware never closes.
    fn is_closed(&self) -> bool {
        false
    }
}

/// A command for the [`Simulated`] reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `insert <uid-hex> [payload]`: place a tag on the reader.
    Insert(Uid, Payload),
    /// `remove`: take the tag off the reader.
    Remove,
}

/// Why a simulated input line was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The first word is not `insert` or `remove`.
    UnknownCommand(String),
    /// `insert` without a UID.
    MissingUid,
    /// The UID is malformed.
    BadUid(UidError),
    /// `remove` followed by more text.
    TrailingInput,
    /// Longer than [`MAX_LINE`] bytes.
    LineTooLong,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Debug-quote the user's word so control characters stay visible.
            ParseError::UnknownCommand(c) => write!(f, "unknown command {c:?}"),
            ParseError::MissingUid => f.write_str("usage: insert <uid-hex> [payload]"),
            ParseError::BadUid(e) => write!(f, "bad UID: {e}"),
            ParseError::TrailingInput => f.write_str("usage: remove"),
            ParseError::LineTooLong => write!(f, "line longer than {MAX_LINE} bytes"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Longest accepted simulated input line, in bytes.
pub const MAX_LINE: usize = 1024;

/// Parses one input line. Blank lines and `#` comments yield `Ok(None)`.
///
/// The payload is everything after the UID and goes through
/// [`Payload::parse`], exactly like text read from a real tag.
pub fn parse_line(line: &str) -> Result<Option<Command>, ParseError> {
    if line.len() > MAX_LINE {
        return Err(ParseError::LineTooLong);
    }
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let (cmd, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let rest = rest.trim_start();
    match cmd {
        "insert" => {
            let (uid, payload) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            if uid.is_empty() {
                return Err(ParseError::MissingUid);
            }
            let uid = Uid::from_hex(uid).map_err(ParseError::BadUid)?;
            Ok(Some(Command::Insert(uid, Payload::parse(payload))))
        }
        "remove" if rest.is_empty() => Ok(Some(Command::Remove)),
        "remove" => Err(ParseError::TrailingInput),
        _ => Err(ParseError::UnknownCommand(cmd.to_owned())),
    }
}

/// A reader driven by text commands (see [`parse_line`]) instead of hardware.
///
/// Lines arrive over a channel so [`Reader::poll`] never blocks. Each poll
/// applies at most one command (blank lines and comments are skipped), so a
/// script plays out one tick per line. The tag stays "present" until `remove`.
pub struct Simulated {
    lines: Receiver<String>,
    current: Option<Reading>,
    closed: bool,
}

impl Simulated {
    /// Reads lines from `input` (e.g. stdin) on a background thread. Invalid
    /// UTF-8 and overlong lines become parse errors; only EOF or an I/O error
    /// ends the input.
    pub fn spawn<R: BufRead + Send + 'static>(mut input: R) -> Simulated {
        let (tx, rx) = mpsc::sync_channel(64);
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            loop {
                buf.clear();
                // Keep at most MAX_LINE + 1 bytes so parse_line can reject it.
                let cap = (MAX_LINE + 1) as u64;
                match input.by_ref().take(cap).read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                if buf.len() > MAX_LINE && buf.last() != Some(&b'\n') {
                    // Discard the rest of the overlong line.
                    let mut rest = Vec::new();
                    while input
                        .by_ref()
                        .take(4096)
                        .read_until(b'\n', &mut rest)
                        .is_ok_and(|n| n > 0)
                        && rest.last() != Some(&b'\n')
                    {
                        rest.clear();
                    }
                }
                if tx.send(String::from_utf8_lossy(&buf).into_owned()).is_err() {
                    break;
                }
            }
        });
        Simulated::from_channel(rx)
    }

    /// Uses lines from an existing channel (deterministic for tests).
    pub fn from_channel(lines: Receiver<String>) -> Simulated {
        Simulated {
            lines,
            current: None,
            closed: false,
        }
    }
}

impl Reader for Simulated {
    type Error = ParseError;

    /// Applies the next command, if any, then reports the current tag. A
    /// malformed line is returned once as an error and skipped.
    fn poll(&mut self) -> Result<Option<Reading>, ParseError> {
        loop {
            match self.lines.try_recv() {
                Ok(line) => match parse_line(&line)? {
                    Some(Command::Insert(uid, payload)) => self.current = Some((uid, payload)),
                    Some(Command::Remove) => self.current = None,
                    None => continue,
                },
                Err(TryRecvError::Disconnected) => self.closed = true,
                Err(TryRecvError::Empty) => {}
            }
            return Ok(self.current.clone());
        }
    }

    fn is_closed(&self) -> bool {
        self.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn uid(hex: &str) -> Uid {
        Uid::from_hex(hex).unwrap()
    }

    #[test]
    fn uid_hex_round_trip_and_lengths() {
        assert_eq!(uid("04a1b2c3d4e5f6").to_string(), "04A1B2C3D4E5F6");
        assert_eq!(uid("DEADBEEF").as_bytes(), &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(uid(&"00".repeat(10)).as_bytes().len(), 10);
        assert_eq!(Uid::new(&[1, 2, 3]), Err(UidError::BadLength(3)));
        let cases = [
            ("", UidError::BadLength(0)),
            ("0011223344", UidError::BadLength(5)),
            ("0011223344556677889900", UidError::BadLength(11)),
            ("04a1b2c", UidError::OddLength),
            ("04a1b2cg", UidError::BadHex),
            ("+4a1b2c3", UidError::BadHex),
            ("04:a1:b2:c3", UidError::BadHex),
            ("0x04a1b2", UidError::BadHex),
            ("٤٤٤٤٤٤٤٤", UidError::BadHex),
            ("deadbeé", UidError::BadHex),
        ];
        for (hex, want) in cases {
            assert_eq!(Uid::from_hex(hex), Err(want), "{hex:?}");
        }
    }

    #[test]
    fn parses_commands() {
        // (line, uid, payload text); payloads are untrusted, so bad ones come
        // out unusable rather than as errors.
        let inserts = [
            (
                "insert 04a1b2c3d4e5f6 steam://440",
                "04a1b2c3d4e5f6",
                "steam://440",
            ),
            (
                "  insert\tdeadbeef   id:my-game \r\n",
                "deadbeef",
                "id:my-game",
            ),
            ("insert deadbeef", "deadbeef", ""),
            ("insert deadbeef steam://440 && calc", "deadbeef", "x"),
            ("insert deadbeef id:../x", "deadbeef", "x"),
        ];
        for (line, hex, payload) in inserts {
            let want = Command::Insert(uid(hex), Payload::parse(payload));
            assert_eq!(parse_line(line), Ok(Some(want)), "{line:?}");
        }
        assert_eq!(parse_line("  remove \n"), Ok(Some(Command::Remove)));
        for blank in ["", "   ", "\n", "# comment", "  # insert deadbeef"] {
            assert_eq!(parse_line(blank), Ok(None), "{blank:?}");
        }
    }

    #[test]
    fn rejects_malformed_lines() {
        use ParseError::*;
        let unknown = |s: &str| UnknownCommand(s.into());
        let cases = [
            ("insert", MissingUid),
            ("insert   ", MissingUid),
            ("insert zzzzzzzz", BadUid(UidError::BadHex)),
            ("insert 04a1b2c", BadUid(UidError::OddLength)),
            ("insert 04a1b2", BadUid(UidError::BadLength(3))),
            ("remove now", TrailingInput),
            ("eject", unknown("eject")),
            ("INSERT deadbeef", unknown("INSERT")),
            ("insert-deadbeef", unknown("insert-deadbeef")),
            ("\0", unknown("\0")),
        ];
        for (line, want) in cases {
            assert_eq!(parse_line(line), Err(want), "{line:?}");
        }
    }

    #[test]
    fn simulated_applies_one_command_per_poll() {
        let (tx, rx) = mpsc::sync_channel(16);
        let mut sim = Simulated::from_channel(rx);
        assert_eq!(sim.poll(), Ok(None));
        for line in ["insert deadbeef steam:440", "", "# c", "remove", "bogus"] {
            tx.send(line.into()).unwrap();
        }
        tx.send("insert 04a1b2c3d4e5f6".into()).unwrap();
        drop(tx);
        let a = Some((uid("deadbeef"), Payload::Steam(440)));
        let b = Some((uid("04a1b2c3d4e5f6"), Payload::parse("")));
        assert_eq!(sim.poll(), Ok(a));
        assert_eq!(sim.poll(), Ok(None), "blank + comment skipped, then remove");
        assert_eq!(sim.poll(), Err(ParseError::UnknownCommand("bogus".into())));
        assert_eq!(sim.poll(), Ok(b.clone()), "line after the bad one applies");
        assert!(!sim.is_closed());
        assert_eq!(sim.poll(), Ok(b), "state kept after EOF");
        assert!(sim.is_closed());
    }

    #[test]
    fn simulated_spawn_survives_bad_input() {
        let mut input = b"insert deadbeef steam:440\n\xff\xfe\nremove\n".to_vec();
        input.extend(vec![b'x'; 5000]);
        input.extend(b"\ninsert 04a1b2c3d4e5f6 id:x".as_slice()); // no final newline
        let mut sim = Simulated::spawn(std::io::Cursor::new(input));
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while !sim.is_closed() && Instant::now() < deadline {
            let r = sim.poll();
            if seen.last() != Some(&r) {
                seen.push(r);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        let a = Some((uid("deadbeef"), Payload::Steam(440)));
        let b = Some((uid("04a1b2c3d4e5f6"), Payload::parse("id:x")));
        let bad_utf8 = ParseError::UnknownCommand("\u{fffd}\u{fffd}".into());
        let want = [
            Ok(a),
            Err(bad_utf8),
            Ok(None),
            Err(ParseError::LineTooLong),
            Ok(b),
        ];
        // A leading Ok(None) only shows up if a poll beat the thread.
        let seen: Vec<_> = seen.into_iter().skip_while(|r| r == &Ok(None)).collect();
        assert_eq!(seen, want);
    }
}
