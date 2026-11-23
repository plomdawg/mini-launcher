//! Minimal reader for Valve's binary KeyValues ("binary VDF"), as used by
//! `userdata/<id>/config/shortcuts.vdf`. Input is untrusted: size, nesting
//! and the number of entries are bounded, and truncated or garbage input is
//! an error, never a panic.
//!
//! The format is a sequence of entries, each a type byte, a NUL-terminated
//! key, then the value: `0x00` a nested map (entries up to `0x08`), `0x01` a
//! NUL-terminated string, `0x02` an int32 (little-endian). Fixed-size types
//! that shortcuts don't use (`0x03` float, `0x04` pointer, `0x06` color,
//! `0x07` uint64, `0x0A` int64) are skipped; any other type byte rejects the
//! file. Strings are decoded as UTF-8, lossily.

/// Largest input accepted, in bytes. Real files are a few KiB.
pub const MAX_LEN: usize = 4 << 20;
const MAX_DEPTH: usize = 16;
/// Entries in the whole file, nested ones included.
const MAX_ENTRIES: usize = 100_000;

const MAP: u8 = 0x00;
const STR: u8 = 0x01;
const INT: u8 = 0x02;
const END: u8 = 0x08;

/// A string, an int32 or a map of key/value pairs (file order; keys may
/// repeat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Int(i32),
    Map(Vec<(String, Value)>),
}

impl Value {
    /// First entry named `key`, ASCII case-insensitive like Steam
    /// (`AppName` is `appname` in some files).
    pub fn get(&self, key: &str) -> Option<&Value> {
        let mut entries = self.entries().iter();
        entries
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    /// Entries of a map; empty for anything else.
    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Map(entries) => entries,
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("larger than {MAX_LEN} bytes")]
    TooLarge,
    #[error("byte {0}: unexpected end of file")]
    Truncated(usize),
    #[error("byte {0}: unknown value type 0x{1:02x}")]
    BadType(usize, u8),
    #[error("nested deeper than {MAX_DEPTH}")]
    TooDeep,
    #[error("more than {MAX_ENTRIES} entries")]
    TooManyEntries,
}

/// Parses a whole file into a map of its top-level entries. The top level
/// ends at its `0x08` (anything after it is ignored) or at the end of input.
pub fn parse(bytes: &[u8]) -> Result<Value, ParseError> {
    if bytes.len() > MAX_LEN {
        return Err(ParseError::TooLarge);
    }
    let mut p = Parser {
        b: bytes,
        pos: 0,
        entries: 0,
    };
    p.map(0).map(Value::Map)
}

struct Parser<'a> {
    b: &'a [u8],
    pos: usize,
    entries: usize,
}

impl Parser<'_> {
    /// Entries up to `0x08` (or the end of input at depth 0).
    fn map(&mut self, depth: usize) -> Result<Vec<(String, Value)>, ParseError> {
        if depth > MAX_DEPTH {
            return Err(ParseError::TooDeep);
        }
        let mut out = Vec::new();
        loop {
            let at = self.pos;
            let ty = match self.b.get(at) {
                Some(&ty) => ty,
                None if depth == 0 => return Ok(out),
                None => return Err(ParseError::Truncated(at)),
            };
            self.pos += 1;
            if ty == END {
                return Ok(out);
            }
            if !matches!(ty, MAP | STR | INT | 0x03 | 0x04 | 0x06 | 0x07 | 0x0A) {
                return Err(ParseError::BadType(at, ty));
            }
            self.entries += 1;
            if self.entries > MAX_ENTRIES {
                return Err(ParseError::TooManyEntries);
            }
            let key = self.string()?;
            let value = match ty {
                MAP => Value::Map(self.map(depth + 1)?),
                STR => Value::Str(self.string()?),
                INT => Value::Int(i32::from_le_bytes(self.take()?)),
                // float, pointer, color: 4 bytes; uint64, int64: 8 bytes.
                0x03 | 0x04 | 0x06 => {
                    self.take::<4>()?;
                    continue;
                }
                // 0x07, 0x0A (checked above).
                _ => {
                    self.take::<8>()?;
                    continue;
                }
            };
            out.push((key, value));
        }
    }

    /// A NUL-terminated string.
    fn string(&mut self) -> Result<String, ParseError> {
        let rest = &self.b[self.pos..];
        let len = rest
            .iter()
            .position(|&c| c == 0)
            .ok_or(ParseError::Truncated(self.b.len()))?;
        self.pos += len + 1;
        Ok(String::from_utf8_lossy(&rest[..len]).into_owned())
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let bytes = self
            .b
            .get(self.pos..self.pos + N)
            .ok_or(ParseError::Truncated(self.b.len()))?;
        let mut out = [0; N];
        out.copy_from_slice(bytes);
        self.pos += N;
        Ok(out)
    }
}

/// Builds binary VDF bytes, for tests (here and in [`super`]).
#[cfg(test)]
pub(crate) mod build {
    pub fn str(out: &mut Vec<u8>, key: &str, value: &str) {
        out.push(super::STR);
        cstr(out, key);
        cstr(out, value);
    }

    pub fn int(out: &mut Vec<u8>, key: &str, value: i32) {
        out.push(super::INT);
        cstr(out, key);
        out.extend(value.to_le_bytes());
    }

    /// A nested map whose entries `body` writes.
    pub fn map(out: &mut Vec<u8>, key: &str, body: impl FnOnce(&mut Vec<u8>)) {
        out.push(super::MAP);
        cstr(out, key);
        body(out);
        out.push(super::END);
    }

    fn cstr(out: &mut Vec<u8>, s: &str) {
        out.extend(s.as_bytes());
        out.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    fn shortcut_file() -> Vec<u8> {
        let mut b = Vec::new();
        map(&mut b, "shortcuts", |b| {
            map(b, "0", |b| {
                int(b, "appid", -1_234_567_890);
                str(b, "AppName", "Doom");
                str(b, "Exe", "\"C:\\Games\\doom.exe\"");
                // A float and a uint64, skipped.
                b.push(0x03);
                b.extend(b"f\0\0\0\x80\x3f");
                b.push(0x07);
                b.extend(b"u\0\x01\x02\x03\x04\x05\x06\x07\x08");
                map(b, "tags", |b| str(b, "0", "fps"));
                int(b, "IsHidden", 0);
            });
        });
        b.push(END);
        b
    }

    #[test]
    fn parses_a_shortcuts_file() {
        let doc = parse(&shortcut_file()).unwrap();
        let entry = doc.get("Shortcuts").unwrap().get("0").unwrap();
        assert_eq!(
            entry.get("APPID").and_then(Value::as_int),
            Some(-1_234_567_890)
        );
        assert_eq!(entry.get("appname").and_then(Value::as_str), Some("Doom"));
        assert_eq!(
            entry.get("exe").and_then(Value::as_str),
            Some("\"C:\\Games\\doom.exe\"")
        );
        assert_eq!(entry.get("f"), None);
        assert_eq!(entry.get("u"), None);
        assert_eq!(
            entry.get("tags").unwrap().get("0").and_then(Value::as_str),
            Some("fps")
        );
        assert_eq!(entry.get("IsHidden").and_then(Value::as_int), Some(0));
        // Wrong-type lookups are None, not a panic.
        assert_eq!(entry.get("appid").and_then(Value::as_str), None);
        assert_eq!(entry.get("exe").unwrap().entries(), []);
    }

    #[test]
    fn table() {
        let full = shortcut_file();
        let mut trailing = full.clone();
        trailing.extend(b"junk");
        let mut no_root_end = full.clone();
        no_root_end.pop();
        let ok = |b: &[u8]| parse(b).is_ok();
        let cases: Vec<(&str, Vec<u8>, Result<(), ParseError>)> = vec![
            ("empty", vec![], Ok(())),
            ("only the root end", vec![END], Ok(())),
            ("trailing bytes after the root", trailing, Ok(())),
            ("no root end", no_root_end, Ok(())),
            (
                "missing inner end",
                full[..full.len() - 2].to_vec(),
                Err(ParseError::Truncated(0)),
            ),
            (
                "key without NUL",
                b"\x01abc".to_vec(),
                Err(ParseError::Truncated(0)),
            ),
            (
                "string without NUL",
                b"\x01k\0abc".to_vec(),
                Err(ParseError::Truncated(0)),
            ),
            (
                "short int",
                b"\x02k\0\x01\x02".to_vec(),
                Err(ParseError::Truncated(0)),
            ),
            (
                "short uint64",
                b"\x07k\0\x01".to_vec(),
                Err(ParseError::Truncated(0)),
            ),
            (
                "unknown type",
                b"\x01a\0b\0\x05k\0".to_vec(),
                Err(ParseError::BadType(5, 5)),
            ),
            ("type 0xff", vec![0xff], Err(ParseError::BadType(0, 0xff))),
            ("too deep", b"\0k\0".repeat(100), Err(ParseError::TooDeep)),
            (
                "too many entries",
                b"\x02k\0\0\0\0\0".repeat(MAX_ENTRIES + 1),
                Err(ParseError::TooManyEntries),
            ),
            (
                "too large",
                vec![END; MAX_LEN + 1],
                Err(ParseError::TooLarge),
            ),
        ];
        for (name, bytes, want) in cases {
            let got = parse(&bytes).map(drop);
            match (&got, &want) {
                // Where a truncation is noticed is not part of the contract.
                (Err(ParseError::Truncated(_)), Err(ParseError::Truncated(_))) => {}
                _ => assert_eq!(got, want, "{name}"),
            }
        }
        assert!(ok(&full));
        // Every truncation of a valid file fails cleanly (or parses as a
        // shorter file when cut right after an entry at the top level).
        for n in 0..full.len() {
            let _ = parse(&full[..n]);
        }
        // Invalid UTF-8 is replaced, not rejected.
        let doc = parse(b"\x01k\0\xff\xfe\0").unwrap();
        assert_eq!(
            doc.get("k").and_then(Value::as_str),
            Some("\u{fffd}\u{fffd}")
        );
    }

    /// Random bytes must never panic (cheap fuzzing).
    #[test]
    fn random_input_never_panics() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut rand = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as usize
        };
        let alphabet = [0x00, 0x01, 0x02, 0x03, 0x07, 0x08, b'a', 0xff];
        let full = shortcut_file();
        for _ in 0..5000 {
            let len = rand() % 64;
            let bytes: Vec<u8> = (0..len).map(|_| rand() as u8).collect();
            let _ = parse(&bytes);
            let soup: Vec<u8> = (0..len)
                .map(|_| alphabet[rand() % alphabet.len()])
                .collect();
            let _ = parse(&soup);
            // A valid file with one byte flipped.
            let mut flipped = full.clone();
            let i = rand() % flipped.len();
            flipped[i] = rand() as u8;
            let _ = parse(&flipped);
        }
    }
}
