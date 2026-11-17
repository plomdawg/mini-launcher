//! The user's local library: `config.toml` in the OS config directory.
//!
//! ```toml
//! [[games]]
//! id = "half-life-2"
//! name = "Half-Life 2"
//! uid = "04:A1:B2:C3:D4:E5:F6"   # optional: this tag UID always launches this entry
//! steam_appid = 220
//!
//! [[games]]
//! id = "my-game"
//! name = "My Game"
//! exe = 'C:\Games\MyGame\game.exe'
//! args = ["--fullscreen"]
//! cwd = 'C:\Games\MyGame'
//! ```
//!
//! Each entry has exactly one of `steam_appid` or `exe`; `args` and `cwd` are
//! only allowed with `exe`. Unknown keys are rejected so typos surface early.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::launch::LaunchPlan;
use crate::payload::Slug;

/// A validated library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub games: Vec<Game>,
}

/// One validated `[[games]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub id: Slug,
    pub name: String,
    /// Tag UID bytes that map to this entry, overriding whatever the tag says.
    pub uid: Option<Vec<u8>>,
    pub plan: LaunchPlan,
}

/// Why a config could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config.toml: {0}")]
    Parse(#[from] toml::de::Error),
    /// Entry `index` (0-based position in `[[games]]`) failed validation.
    #[error("games[{index}]: {message}")]
    Invalid { index: usize, message: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    games: Vec<RawGame>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGame {
    id: String,
    name: String,
    uid: Option<String>,
    steam_appid: Option<i64>,
    exe: Option<PathBuf>,
    args: Option<Vec<String>>,
    cwd: Option<PathBuf>,
}

impl Config {
    /// `<OS config dir>/nfc-launcher/config.toml`, if the OS has a config dir.
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "nfc-launcher")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    /// Loads and validates `path`. A missing file is an empty library, so
    /// `steam://` tags work with zero configuration.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(source) => Err(ConfigError::Io {
                path: path.to_owned(),
                source,
            }),
        }
    }

    /// Parses and validates TOML text.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text)?;
        let mut ids = HashSet::new();
        let mut uids = HashSet::new();
        let mut games = Vec::with_capacity(raw.games.len());
        for (index, raw) in raw.games.into_iter().enumerate() {
            let invalid = |message: String| ConfigError::Invalid { index, message };
            let game = validate(raw).map_err(invalid)?;
            if !ids.insert(game.id.clone()) {
                return Err(invalid(format!("duplicate id \"{}\"", game.id)));
            }
            if let Some(uid) = &game.uid {
                if !uids.insert(uid.clone()) {
                    return Err(invalid(format!("duplicate uid {}", format_uid(uid))));
                }
            }
            games.push(game);
        }
        Ok(Config { games })
    }

    /// The entry with this id.
    pub fn by_id(&self, id: &Slug) -> Option<&Game> {
        self.games.iter().find(|g| &g.id == id)
    }

    /// The entry mapped to this tag UID.
    pub fn by_uid(&self, uid: &[u8]) -> Option<&Game> {
        self.games.iter().find(|g| g.uid.as_deref() == Some(uid))
    }
}

fn validate(raw: RawGame) -> Result<Game, String> {
    let id = Slug::new(&raw.id).map_err(|e| format!("id \"{}\": {e}", raw.id))?;
    let ctx = |msg: &str| format!("\"{id}\": {msg}");
    if raw.name.trim().is_empty() {
        return Err(ctx("name must not be empty"));
    }
    let uid = raw
        .uid
        .as_deref()
        .map(|s| {
            parse_uid(s).ok_or_else(|| {
                ctx(&format!(
                    "uid \"{s}\": expected 4, 7 or 10 hex bytes, e.g. \"04:A1:B2:C3:D4:E5:F6\""
                ))
            })
        })
        .transpose()?;
    let plan = match (raw.steam_appid, raw.exe) {
        (Some(_), Some(_)) => return Err(ctx("set steam_appid or exe, not both")),
        (None, None) => return Err(ctx("set one of steam_appid or exe")),
        (Some(n), None) => {
            if raw.args.is_some() || raw.cwd.is_some() {
                return Err(ctx("args and cwd are only allowed with exe"));
            }
            let appid = u32::try_from(n)
                .ok()
                .filter(|&n| n != 0)
                .ok_or_else(|| ctx(&format!("steam_appid {n}: must be 1 to {}", u32::MAX)))?;
            LaunchPlan::Steam { appid }
        }
        (None, Some(path)) => {
            if path.as_os_str().is_empty() {
                return Err(ctx("exe must not be empty"));
            }
            if raw.cwd.as_ref().is_some_and(|c| c.as_os_str().is_empty()) {
                return Err(ctx("cwd must not be empty"));
            }
            LaunchPlan::Exe {
                path,
                args: raw.args.unwrap_or_default(),
                cwd: raw.cwd,
            }
        }
    };
    Ok(Game {
        id,
        name: raw.name,
        uid,
        plan,
    })
}

/// Hex UID, case-insensitive, optionally separated by `:`, `-` or spaces.
/// ISO 14443-A UIDs are 4, 7 (NTAG213) or 10 bytes.
pub fn parse_uid(s: &str) -> Option<Vec<u8>> {
    let hex: Vec<u8> = s.bytes().filter(|b| !b" :-".contains(b)).collect();
    if !matches!(hex.len(), 8 | 14 | 20) || !hex.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
    Some(
        hex.chunks(2)
            .map(|p| digit(p[0]) << 4 | digit(p[1]))
            .collect(),
    )
}

/// Canonical `04:A1:...` form for messages.
pub fn format_uid(uid: &[u8]) -> String {
    uid.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_config() {
        let cfg = Config::parse(
            r#"
            [[games]]
            id = "hl2"
            name = "Half-Life 2"
            uid = "04:a1:B2:c3:D4:e5:F6"
            steam_appid = 220
            [[games]]
            id = "my-game"
            name = "My Game"
            exe = "/opt/game/run"
            args = ["--fullscreen", "a b; rm -rf ~"]
            cwd = "/opt/game"
            [[games]]
            id = "bare"
            name = "Bare"
            exe = "game"
            "#,
        )
        .unwrap();
        let plans: Vec<_> = cfg.games.iter().map(|g| g.plan.clone()).collect();
        let (args, cwd) = (vec!["--fullscreen".into(), "a b; rm -rf ~".into()], None);
        assert_eq!(
            plans,
            [
                LaunchPlan::Steam { appid: 220 },
                LaunchPlan::Exe {
                    path: "/opt/game/run".into(),
                    args,
                    cwd: Some("/opt/game".into())
                },
                LaunchPlan::Exe {
                    path: "game".into(),
                    args: vec![],
                    cwd
                },
            ]
        );
        assert_eq!(
            cfg.games[0].uid,
            Some(vec![4, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6])
        );
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn rejects_invalid_configs() {
        let g = |body: &str| format!("[[games]]\nid = \"x\"\nname = \"X\"\n{body}");
        let h = |body: &str| format!("[[games]]\nsteam_appid = 1\n{body}");
        let two = |a: &str, b: &str| format!("{}\n{}", h(a), h(b));
        let cases = [
            (g("steam_appid = 1\nexe = \"a\""), "not both"),
            (g(""), "set one of"),
            (g("steam_appid = 0"), "must be 1 to"),
            (g("steam_appid = -1"), "must be 1 to"),
            (g("steam_appid = 4294967296"), "must be 1 to"),
            (g("steam_appid = \"440\""), "invalid type"),
            (g("steam_appid = 1\nargs = []"), "only allowed with exe"),
            (g("steam_appid = 1\ncwd = \"/\""), "only allowed with exe"),
            (g("exe = \"\""), "exe must not be empty"),
            (g("exe = \"a\"\ncwd = \"\""), "cwd must not be empty"),
            (g("steam_appid = 1\nuid = \"zz\""), "uid \"zz\""),
            (g("steam_appid = 1\nuid = \"04a1b2c3d4e5\""), "uid"),
            (g("steam_appid = 1\nexec = \"a\""), "unknown field `exec`"),
            (h("id = \"x\""), "missing field `name`"),
            (h("id = \"x\"\nname = \" \""), "name must not be empty"),
            (
                h("id = \"../x\"\nname = \"X\""),
                "id \"../x\": invalid slug",
            ),
            (h("id = \"My\"\nname = \"X\""), "invalid slug"),
            ("game = []".into(), "unknown field `game`"),
            ("[[games]\n".into(), "invalid config.toml"),
            (
                two("id = \"x\"\nname = \"X\"", "id = \"x\"\nname = \"Y\""),
                "games[1]: duplicate id",
            ),
            (
                two(
                    "id = \"a\"\nname = \"A\"\nuid = \"01020304\"",
                    "id = \"b\"\nname = \"B\"\nuid = \"01:02:03:04\"",
                ),
                "games[1]: duplicate uid 01:02:03:04",
            ),
        ];
        for (text, want) in cases {
            let got = Config::parse(&text).unwrap_err().to_string();
            assert!(got.contains(want), "{text:?}: got {got:?}, want {want:?}");
        }
    }

    #[test]
    fn uid_parsing() {
        assert_eq!(parse_uid("DEADBEEF"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(parse_uid("de-ad be:ef"), Some(vec![0xde, 0xad, 0xbe, 0xef]));
        assert_eq!(parse_uid(&"ab".repeat(10)).map(|u| u.len()), Some(10));
        for bad in [
            "",
            "dead",
            "deadbee",
            "deadbeeg",
            "+eadbeef",
            "abababababababab",
        ] {
            assert_eq!(parse_uid(bad), None, "input {bad:?}");
        }
    }

    #[test]
    fn missing_file_is_empty_library() {
        let path = std::env::temp_dir().join("nfc-launcher-test-does-not-exist/config.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }
}
