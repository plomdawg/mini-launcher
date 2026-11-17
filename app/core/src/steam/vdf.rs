//! Minimal reader for Valve's text KeyValues ("VDF") format, as used by
//! `libraryfolders.vdf` and `appmanifest_*.acf`. Input is untrusted: size and
//! nesting are bounded and malformed input is an error, never a panic.
//! Escapes `\\ \" \n \t` are decoded; any other backslash is kept, so an
//! unescaped Windows path survives. `//` comments are skipped.

/// Largest input accepted, in bytes. Real Steam files are a few KiB.
pub const MAX_LEN: usize = 4 << 20;
const MAX_DEPTH: usize = 32;

/// A string or a block of key/value pairs (file order; keys may repeat).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// First entry named `key`, ASCII case-insensitive like Steam.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let mut entries = self.entries().iter();
        entries
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Obj(_) => None,
        }
    }

    /// Entries of a block; empty for a string.
    pub fn entries(&self) -> &[(String, Value)] {
        match self {
            Value::Obj(entries) => entries,
            Value::Str(_) => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    #[error("larger than {MAX_LEN} bytes")]
    TooLarge,
    #[error("line {0}: {1}")]
    Syntax(usize, &'static str),
}

/// Parses a whole file into a block of its top-level entries.
pub fn parse(text: &str) -> Result<Value, ParseError> {
    if text.len() > MAX_LEN {
        return Err(ParseError::TooLarge);
    }
    let s = text.strip_prefix('\u{feff}').unwrap_or(text);
    block(&mut Lexer { s, pos: 0, line: 1 }, 0).map(Value::Obj)
}

/// Entries up to the closing `}` (or the end of input at depth 0).
fn block(lx: &mut Lexer, depth: usize) -> Result<Vec<(String, Value)>, ParseError> {
    if depth > MAX_DEPTH {
        return Err(lx.err("nested too deep"));
    }
    let mut out = Vec::new();
    loop {
        let key = match lx.next()? {
            Tok::Str(key) => key,
            Tok::Close if depth > 0 => return Ok(out),
            Tok::Eof if depth == 0 => return Ok(out),
            _ => return Err(lx.err("expected a key")),
        };
        let value = match lx.next()? {
            Tok::Str(v) => Value::Str(v),
            Tok::Open => Value::Obj(block(lx, depth + 1)?),
            _ => return Err(lx.err("expected a value")),
        };
        out.push((key, value));
    }
}

enum Tok {
    Str(String),
    Open,
    Close,
    Eof,
}

struct Lexer<'a> {
    s: &'a str,
    /// Byte offset, only advanced past ASCII bytes or whole chars.
    pos: usize,
    line: usize,
}

impl Lexer<'_> {
    fn err(&self, msg: &'static str) -> ParseError {
        ParseError::Syntax(self.line, msg)
    }

    fn next(&mut self) -> Result<Tok, ParseError> {
        let b = self.s.as_bytes();
        let is_delim = |c: &u8| c.is_ascii_whitespace() || matches!(c, b'{' | b'}' | b'"');
        loop {
            let Some(&c) = b.get(self.pos) else {
                return Ok(Tok::Eof);
            };
            self.pos += 1;
            match c {
                b'\n' => self.line += 1,
                b'{' => return Ok(Tok::Open),
                b'}' => return Ok(Tok::Close),
                b'"' => return self.quoted(),
                b'/' if b.get(self.pos) == Some(&b'/') => {
                    while b.get(self.pos).is_some_and(|&c| c != b'\n') {
                        self.pos += 1;
                    }
                }
                _ if c.is_ascii_whitespace() => {}
                _ => {
                    let start = self.pos - 1;
                    while b.get(self.pos).is_some_and(|c| !is_delim(c)) {
                        self.pos += 1;
                    }
                    return Ok(Tok::Str(self.s[start..self.pos].to_owned()));
                }
            }
        }
    }

    fn quoted(&mut self) -> Result<Tok, ParseError> {
        let mut out = String::new();
        let mut chars = self.s[self.pos..].char_indices();
        while let Some((i, c)) = chars.next() {
            match (c, c == '\\') {
                ('"', _) => {
                    self.pos += i + 1;
                    return Ok(Tok::Str(out));
                }
                (_, true) => match chars.next().map(|(_, e)| e) {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(e @ ('\\' | '"')) => out.push(e),
                    Some(e) => out.extend(['\\', e]),
                    None => break,
                },
                _ => {
                    self.line += usize::from(c == '\n');
                    out.push(c);
                }
            }
        }
        Err(self.err("unterminated string"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_comments_and_escapes() {
        let text = "\u{feff}// c\n\"Root\"\n{\n \"a\" \"1\"\n b unquoted\n \"Sub\" { \"x\" \"\" }\n \
                    \"p\" \"C:\\\\Games\\\\\" \"q\" \"say \\\"hi\\\"\\tok\\n\" \"raw\" \"D:\\Lib\"\n}";
        let doc = parse(text).unwrap();
        let root = doc.get("root").unwrap();
        let get = |k| root.get(k).and_then(Value::as_str);
        assert_eq!(get("A"), Some("1"));
        assert_eq!(get("b"), Some("unquoted"));
        assert_eq!(
            root.get("sub").unwrap().get("x"),
            Some(&Value::Str(String::new()))
        );
        assert_eq!(get("p"), Some(r"C:\Games\"));
        assert_eq!(get("q"), Some("say \"hi\"\tok\n"));
        assert_eq!(get("raw"), Some(r"D:\Lib"));
        assert_eq!(
            (get("nope"), get("sub"), root.get("a").unwrap().get("a")),
            (None, None, None)
        );
    }

    #[test]
    fn rejects_malformed_input() {
        for (text, line) in [
            ("\"a\" {\n\"b\" \"c\"\n", 3),
            ("\"a\"", 1),
            ("}", 1),
            ("a }", 1),
        ] {
            assert!(
                matches!(parse(text), Err(ParseError::Syntax(l, _)) if l == line),
                "{text}"
            );
        }
        assert!(parse(r#""raw" "D:\Lib\""#).is_err()); // `\"` is a quote
        assert!(parse(&"a{".repeat(100)).is_err());
        let big = format!("\"a\" \"{}\"", "x".repeat(MAX_LEN));
        assert_eq!(parse(&big), Err(ParseError::TooLarge));
        assert_eq!(parse(""), Ok(Value::Obj(vec![])));
    }

    /// Random bytes and token soup must never panic (cheap fuzzing).
    #[test]
    fn random_input_never_panics() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut rand = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as usize
        };
        let alphabet: Vec<char> = "{}\"\\/ \n\tab1é".chars().collect();
        for _ in 0..5000 {
            let len = rand() % 64;
            let bytes: Vec<u8> = (0..len).map(|_| rand() as u8).collect();
            let _ = parse(&String::from_utf8_lossy(&bytes));
            let soup: String = (0..len)
                .map(|_| alphabet[rand() % alphabet.len()])
                .collect();
            let _ = parse(&soup);
        }
    }
}
