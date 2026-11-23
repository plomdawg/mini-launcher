//! Tag payload grammar and strict validation (see `docs/tag-format.md`).
//!
//! Tag contents are untrusted. This module turns the decoded record text into a
//! [`Payload`] and never hands raw tag text onward: a Steam appid comes out as a
//! parsed `u32`, an `id:` slug as a validated [`Slug`]. Anything that fails
//! validation is [`Payload::Unusable`], so the caller falls back to the tag UID.

use std::fmt;

/// What a tag's payload string asks the launcher to do.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Payload {
    /// `steam://<appid>` or `steam:<appid>`; the appid is non-zero.
    Steam(u32),
    /// `id:<slug>`: a key into the local library in `config.toml`.
    Id(Slug),
    /// Nothing usable; identify the tag by its UID only.
    Unusable,
}

impl Payload {
    /// Parses a decoded Text/URI record string. Never panics; invalid input
    /// yields [`Payload::Unusable`].
    pub fn parse(text: &str) -> Payload {
        let text = trim_spec_whitespace(text);
        // Check the long form first: `steam://440` also starts with `steam:`.
        let digits = text
            .strip_prefix("steam://")
            .or_else(|| text.strip_prefix("steam:"));
        if let Some(digits) = digits {
            return parse_appid(digits).map_or(Payload::Unusable, Payload::Steam);
        }
        if let Some(slug) = text.strip_prefix("id:") {
            return Slug::new(slug).map_or(Payload::Unusable, Payload::Id);
        }
        Payload::Unusable
    }
}

/// Builds the Steam launch URI from a parsed appid. This is the only way the
/// launcher should produce a `steam://` URI; tag text is never reused.
pub fn steam_launch_uri(appid: u32) -> String {
    format!("steam://rungameid/{appid}")
}

/// Builds the launch URI for a non-Steam game added to Steam (a shortcut)
/// from its parsed 32-bit appid: Steam's 64-bit game id, the appid in the
/// high half and the shortcut type (`0x0200_0000`) in the low half. Never
/// built from tag text; tags name shortcuts by `id:<slug>` only.
pub fn steam_shortcut_uri(appid: u32) -> String {
    let game_id = (u64::from(appid) << 32) | 0x0200_0000;
    format!("steam://rungameid/{game_id}")
}

/// A validated library slug matching `^[a-z0-9][a-z0-9-]{0,63}$`.
///
/// The only constructor is [`Slug::new`], so holding a `Slug` means the string
/// was validated. Slugs are config lookup keys only, never paths or commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slug(String);

/// Error returned when a string is not a valid [`Slug`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSlug;

impl fmt::Display for InvalidSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid slug: expected 1-64 chars of [a-z0-9-], not starting with '-'")
    }
}

impl std::error::Error for InvalidSlug {}

impl Slug {
    /// Maximum slug length in bytes (all valid slugs are ASCII).
    pub const MAX_LEN: usize = 64;

    /// Validates `s` against `^[a-z0-9][a-z0-9-]{0,63}$`.
    pub fn new(s: &str) -> Result<Slug, InvalidSlug> {
        let bytes = s.as_bytes();
        let first_ok = bytes
            .first()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
        let rest_ok = bytes
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if first_ok && rest_ok && bytes.len() <= Self::MAX_LEN {
            Ok(Slug(s.to_owned()))
        } else {
            Err(InvalidSlug)
        }
    }

    /// The validated slug text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Trims exactly the spec's whitespace set: space, tab, CR, LF. (Not
/// `trim_ascii`, which also trims form feed.)
fn trim_spec_whitespace(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\r', '\n'])
}

/// ASCII decimal digits only, parsed as a non-zero `u32`.
fn parse_appid(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Only digits remain, so `parse` cannot see a sign; it rejects overflow.
    digits.parse::<u32>().ok().filter(|&n| n != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Payload {
        Payload::Id(Slug::new(s).unwrap())
    }

    #[test]
    fn valid_examples() {
        let slug64 = "a".repeat(64);
        let cases: &[(&str, Payload)] = &[
            // docs/tag-format.md "Valid"
            ("steam://440", Payload::Steam(440)),
            ("steam:440", Payload::Steam(440)),
            ("steam://4294967295", Payload::Steam(u32::MAX)),
            ("id:my-game", id("my-game")),
            ("id:0", id("0")),
            ("id:half-life-2", id("half-life-2")),
            ("steam://440 ", Payload::Steam(440)),
            ("steam://440\n", Payload::Steam(440)),
            // Whitespace set: space, tab, CR, LF on both sides.
            (" \t\r\nsteam://440\r\n\t ", Payload::Steam(440)),
            ("\tid:my-game\r\n", id("my-game")),
            // Leading zeros tolerated (issue #5 review).
            ("steam://0440", Payload::Steam(440)),
            ("steam:0001", Payload::Steam(1)),
            ("steam://1", Payload::Steam(1)),
            ("id:a", id("a")),
            ("id:a-", id("a-")),
            ("id:9-9", id("9-9")),
            (&format!("id:{slug64}"), id(&slug64)),
        ];
        for (input, want) in cases {
            assert_eq!(&Payload::parse(input), want, "input {input:?}");
        }
    }

    #[test]
    fn rejected_examples() {
        let slug65 = format!("id:{}", "a".repeat(65));
        let cases: &[&str] = &[
            // docs/tag-format.md "Rejected"
            "steam://0",
            "steam://",
            "steam://4294967296",
            "steam://+440",
            "steam://-1",
            "steam://0x1b8",
            "steam://٤٤٠",
            "steam://440 && calc",
            "steam://440/",
            "steam://440?x=1",
            "steam://rungameid/440",
            "steam://run/440",
            "steam://440;calc.exe",
            "STEAM://440",
            "https://store.steampowered.com/app/440/",
            "https://steam://440",
            "id:",
            "id:My-Game",
            "id:-game",
            "id:my_game",
            "id:../x",
            "id:../games/x",
            &slug65,
            "C:\\Games\\game.exe",
            // Issue #5 hostile list and review comments.
            "",
            "   \r\n\t",
            "steam://00",
            "steam:0",
            "steam:",
            "steam://440/foo",
            "steam://440?x",
            "steam://440;calc",
            "steam://１２３", // fullwidth digits
            "steam://99999999999999999999999",
            "id:A",
            "id:-x",
            "steam://440\0",
            "steam://\u{0}440",
            "id:my-game\0",
            "\0steam://440",
            // Whitespace outside the spec's set is not trimmed.
            "steam://440\u{c}",
            "\u{b}steam://440",
            "steam://440\u{a0}",
            "\u{feff}steam://440",
            // Internal whitespace.
            "steam:// 440",
            "steam: 440",
            "id: my-game",
            "id:my game",
            // Other near-misses.
            "Steam://440",
            "steam:/440",
            "steam:///440",
            "steam://440.0",
            "steam://4_40",
            "ID:my-game",
            "id:my-game/",
            "id:é",
            "id:my-gamé",
            "440",
            "id",
            "steam",
        ];
        for input in cases {
            assert_eq!(Payload::parse(input), Payload::Unusable, "input {input:?}");
        }
    }

    #[test]
    fn slug_constructor_validates() {
        assert_eq!(Slug::new("my-game").unwrap().as_str(), "my-game");
        assert_eq!(Slug::new("x").unwrap().to_string(), "x");
        for bad in ["", "-", "A", "a.b", "a/b", "a b", "a\0", "\u{0130}"] {
            assert_eq!(Slug::new(bad), Err(InvalidSlug), "input {bad:?}");
        }
        assert!(Slug::new(&"a".repeat(64)).is_ok());
        assert!(Slug::new(&"a".repeat(65)).is_err());
    }

    #[test]
    fn shortcut_uri_is_built_from_the_u32() {
        let uri = |n| steam_shortcut_uri(n);
        assert_eq!(uri(0xC0FF_EE01), "steam://rungameid/13907095862439313408");
        assert_eq!(uri(u32::MAX), "steam://rungameid/18446744069448138752");
        assert_eq!(uri(1), "steam://rungameid/4328521728");
        // A shortcut appid on a tag is just a (store) Steam appid: the tag
        // grammar has no shortcut form.
        assert_eq!(
            Payload::parse("steam://3237998081"),
            Payload::Steam(3_237_998_081)
        );
        assert_eq!(
            Payload::parse("steam://13907095862439313408"),
            Payload::Unusable
        );
    }

    #[test]
    fn launch_uri_is_rebuilt_from_number() {
        assert_eq!(steam_launch_uri(440), "steam://rungameid/440");
        assert_eq!(steam_launch_uri(u32::MAX), "steam://rungameid/4294967295");
        // Leading zeros on the tag never reach the URI.
        let Payload::Steam(n) = Payload::parse("steam://000440") else {
            panic!("expected Steam");
        };
        assert_eq!(steam_launch_uri(n), "steam://rungameid/440");
    }

    /// Independent reference check of the grammar, used by the brute-force test.
    fn check_invariants(input: &str) {
        let trimmed = input.trim_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'));
        match Payload::parse(input) {
            Payload::Steam(n) => {
                assert_ne!(n, 0, "input {input:?}");
                let digits = trimmed
                    .strip_prefix("steam://")
                    .or_else(|| trimmed.strip_prefix("steam:"))
                    .expect("steam prefix");
                assert!(digits.chars().all(|c| c.is_ascii_digit()), "{input:?}");
                assert_eq!(digits.trim_start_matches('0'), n.to_string(), "{input:?}");
            }
            Payload::Id(slug) => {
                assert_eq!(trimmed, format!("id:{slug}"), "input {input:?}");
                let s = slug.as_str();
                assert!((1..=64).contains(&s.len()), "{input:?}");
                assert!(!s.starts_with('-'), "{input:?}");
                assert!(
                    s.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                    "{input:?}"
                );
            }
            Payload::Unusable => {}
        }
    }

    #[test]
    fn brute_force_never_panics_and_respects_grammar() {
        // Exhaustive short suffixes over an alphabet of interesting characters.
        let alphabet = [
            '0', '1', '4', '9', 'a', 'z', 'A', '-', '_', '/', ':', '.', '+', ' ', '\t', '\n', '\0',
            '\u{c}', '٤', 'é', '\u{feff}',
        ];
        let prefixes = ["", "steam://", "steam:", "id:", " steam://", "id:a"];
        // Every string of length 0..=3 over the alphabet.
        let mut layer = vec![String::new()];
        let mut suffixes = layer.clone();
        for _ in 0..3 {
            layer = layer
                .iter()
                .flat_map(|s| alphabet.iter().map(move |c| format!("{s}{c}")))
                .collect();
            suffixes.extend(layer.iter().cloned());
        }
        for prefix in prefixes {
            for suffix in &suffixes {
                check_invariants(&format!("{prefix}{suffix}"));
            }
        }

        // Pseudo-random byte strings (xorshift, fixed seed), lossily decoded.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let len = (next() % 80) as usize;
            let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let s = String::from_utf8_lossy(&bytes);
            check_invariants(&s);
            check_invariants(&format!("steam://{s}"));
            check_invariants(&format!("id:{s}"));
        }
    }
}
