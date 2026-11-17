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
//!
//! [[emulators]]
//! id = "eden"
//! name = "Eden"
//! exe = 'C:\Emulators\Eden\eden.exe'
//! args = ["-f", "-g", "{rom}"]
//!
//! [[games]]
//! id = "mario-kart-8"
//! name = "Mario Kart 8 Deluxe"
//! emulator = "eden"
//! rom = 'D:\Switch\mk8.nsp'
//! ```
//!
//! Each game has exactly one of `steam_appid`, `exe` or `emulator` + `rom`;
//! `args` and `cwd` are only allowed with `exe`. An emulator's `args` must
//! contain `{rom}` as one whole argument exactly once; it is replaced by the
//! ROM path as a single argument when the config is loaded. Unknown keys are
//! rejected so typos surface early.
//! [`edit`] changes the file without losing comments; [`location`] handles
//! the optional config folder override.

pub mod edit;
pub mod location;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::launch::LaunchPlan;
use crate::payload::Slug;

/// A validated library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub emulators: Vec<Emulator>,
    pub games: Vec<Game>,
}

/// One validated `[[emulators]]` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emulator {
    pub id: Slug,
    pub name: String,
    pub exe: PathBuf,
    /// Contains [`ROM_PLACEHOLDER`] exactly once, as a whole argument.
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
}

/// The emulator argument that is replaced by a game's ROM path.
pub const ROM_PLACEHOLDER: &str = "{rom}";

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
    /// The file could not be read. The display omits `path` and `source`
    /// (reported by the error chain) so callers can add context once.
    #[error("cannot read the file")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config.toml: {0}")]
    Parse(#[from] toml::de::Error),
    /// Entry `index` (0-based position in `[[section]]`, `games` or
    /// `emulators`) failed validation.
    #[error("{section}[{index}]: {message}")]
    Invalid {
        section: &'static str,
        index: usize,
        message: String,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    emulators: Vec<RawEmulator>,
    #[serde(default)]
    games: Vec<RawGame>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEmulator {
    id: String,
    name: String,
    exe: PathBuf,
    args: Vec<String>,
    cwd: Option<PathBuf>,
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
    emulator: Option<String>,
    rom: Option<PathBuf>,
}

impl Config {
    /// The per-user config directory, if the OS has one: on Windows
    /// `%APPDATA%\mini-launcher\config`, on Linux
    /// `$XDG_CONFIG_HOME/mini-launcher` (usually `~/.config/...`).
    pub fn default_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "mini-launcher")
            .map(|dirs| dirs.config_dir().to_owned())
    }

    /// Where the config lives: [`Config::default_dir`] or the folder its
    /// `location.toml` names. `None` if the OS has no config directory.
    pub fn location() -> Option<location::ConfigLocation> {
        Config::default_dir().map(|dir| location::ConfigLocation::resolve(&dir))
    }

    /// Loads and validates `path`, which must exist (e.g. a path the user
    /// gave explicitly, where a typo must not silently drop every mapping).
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        })?;
        Config::parse(&text)
    }

    /// Like [`Config::load`], but a missing file is an empty library, so
    /// `steam://` tags work with zero configuration. For the default location.
    pub fn load_or_empty(path: &Path) -> Result<Config, ConfigError> {
        Config::parse(&read_text_or_empty(path)?)
    }

    /// Parses and validates TOML text.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let raw: RawConfig = toml::from_str(text)?;
        let mut emulators = Vec::with_capacity(raw.emulators.len());
        for (index, raw) in raw.emulators.into_iter().enumerate() {
            let invalid = |message: String| ConfigError::Invalid {
                section: "emulators",
                index,
                message,
            };
            let emulator = validate_emulator(raw).map_err(invalid)?;
            if emulators.iter().any(|e: &Emulator| e.id == emulator.id) {
                return Err(invalid(format!("duplicate id \"{}\"", emulator.id)));
            }
            emulators.push(emulator);
        }
        let mut ids = HashSet::new();
        let mut uids = HashSet::new();
        let mut games = Vec::with_capacity(raw.games.len());
        for (index, raw) in raw.games.into_iter().enumerate() {
            let invalid = |message: String| ConfigError::Invalid {
                section: "games",
                index,
                message,
            };
            let game = validate(raw, &emulators).map_err(invalid)?;
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
        Ok(Config { emulators, games })
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

/// The text of `path`, or `""` if it does not exist.
pub fn read_text_or_empty(path: &Path) -> Result<String, ConfigError> {
    match std::fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        result => result.map_err(|source| ConfigError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn validate_emulator(raw: RawEmulator) -> Result<Emulator, String> {
    let id = Slug::new(&raw.id).map_err(|e| format!("id \"{}\": {e}", raw.id))?;
    let ctx = |msg: &str| format!("\"{id}\": {msg}");
    if raw.name.trim().is_empty() {
        return Err(ctx("name must not be empty"));
    }
    if raw.exe.as_os_str().is_empty() {
        return Err(ctx("exe must not be empty"));
    }
    if raw.cwd.as_ref().is_some_and(|c| c.as_os_str().is_empty()) {
        return Err(ctx("cwd must not be empty"));
    }
    check_rom_arg(&raw.args).map_err(|e| ctx(&e))?;
    Ok(Emulator {
        id,
        name: raw.name,
        exe: raw.exe,
        args: raw.args,
        cwd: raw.cwd,
    })
}

/// The `{rom}` rule for an emulator's args: [`ROM_PLACEHOLDER`] exactly
/// once, as a whole argument (no interpolation inside others).
pub fn check_rom_arg(args: &[String]) -> Result<(), String> {
    if let Some(arg) = args
        .iter()
        .find(|a| *a != ROM_PLACEHOLDER && a.contains(ROM_PLACEHOLDER))
    {
        return Err(format!(
            "args: {ROM_PLACEHOLDER} must be a whole argument, not part of {arg:?}"
        ));
    }
    if args.iter().filter(|a| *a == ROM_PLACEHOLDER).count() != 1 {
        return Err(format!(
            "args must contain {ROM_PLACEHOLDER:?} exactly once"
        ));
    }
    Ok(())
}

fn validate(raw: RawGame, emulators: &[Emulator]) -> Result<Game, String> {
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
    if raw.rom.is_some() && raw.emulator.is_none() {
        return Err(ctx("rom is only allowed with emulator"));
    }
    let plan = match (raw.steam_appid, raw.exe, raw.emulator) {
        (None, None, None) => return Err(ctx("set one of steam_appid, exe or emulator")),
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) | (_, Some(_), Some(_)) => {
            return Err(ctx("set only one of steam_appid, exe or emulator"))
        }
        (None, None, Some(emulator)) => {
            if raw.args.is_some() || raw.cwd.is_some() {
                return Err(ctx("args and cwd are only allowed with exe"));
            }
            let emu = emulators
                .iter()
                .find(|e| e.id.as_str() == emulator)
                .ok_or_else(|| {
                    ctx(&format!(
                        "emulator \"{emulator}\" is not defined in [[emulators]]"
                    ))
                })?;
            let rom = raw.rom.ok_or_else(|| ctx("emulator needs a rom"))?;
            if rom.as_os_str().is_empty() {
                return Err(ctx("rom must not be empty"));
            }
            emulator_plan(emu, rom)
        }
        (Some(n), None, None) => {
            if raw.args.is_some() || raw.cwd.is_some() {
                return Err(ctx("args and cwd are only allowed with exe"));
            }
            let appid = u32::try_from(n)
                .ok()
                .filter(|&n| n != 0)
                .ok_or_else(|| ctx(&format!("steam_appid {n}: must be 1 to {}", u32::MAX)))?;
            LaunchPlan::Steam { appid }
        }
        (None, Some(path), None) => {
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

/// The plan for `rom` on `emulator`: its args with the `{rom}` argument
/// replaced by the ROM path as one argument (never split or interpolated).
pub fn emulator_plan(emulator: &Emulator, rom: PathBuf) -> LaunchPlan {
    let args = emulator
        .args
        .iter()
        .map(|a| {
            if a == ROM_PLACEHOLDER {
                // Lossless: TOML strings are UTF-8.
                rom.to_string_lossy().into_owned()
            } else {
                a.clone()
            }
        })
        .collect();
    LaunchPlan::Emulator {
        emulator: emulator.id.clone(),
        rom,
        path: emulator.exe.clone(),
        args,
        cwd: emulator.cwd.clone(),
    }
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
            (g("steam_appid = 1\nexe = \"a\""), "only one of"),
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

    const EMU: &str = r#"
        [[emulators]]
        id = "eden"
        name = "Eden"
        exe = 'C:\Emu\eden.exe'
        args = ["-f", "-g", "{rom}"]
        cwd = 'C:\Emu'
    "#;

    #[test]
    fn emulator_games_resolve_at_load() {
        let cfg = Config::parse(&format!(
            "{EMU}\n[[games]]\nid = \"mk8\"\nname = \"MK8\"\nemulator = \"eden\"\n\
             rom = 'D:\\My Games\\mk8; rm -rf ~ {{rom}}.nsp'\n"
        ))
        .unwrap();
        assert_eq!(cfg.emulators[0].args, ["-f", "-g", ROM_PLACEHOLDER]);
        let rom = r"D:\My Games\mk8; rm -rf ~ {rom}.nsp";
        // The ROM path is one argv element, spaces and all, never re-expanded.
        assert_eq!(
            cfg.games[0].plan,
            LaunchPlan::Emulator {
                emulator: Slug::new("eden").unwrap(),
                rom: rom.into(),
                path: r"C:\Emu\eden.exe".into(),
                args: vec!["-f".into(), "-g".into(), rom.into()],
                cwd: Some(r"C:\Emu".into()),
            }
        );
        // An emulator section with no games, and no section at all, both load.
        assert_eq!(Config::parse(EMU).unwrap().games, []);
        assert_eq!(
            Config::parse("[[games]]\nid = \"a\"\nname = \"A\"\nsteam_appid = 1")
                .unwrap()
                .emulators,
            []
        );
    }

    #[test]
    fn rejects_invalid_emulator_configs() {
        let e = |body: &str| format!("[[emulators]]\nid = \"eden\"\nname = \"Eden\"\n{body}");
        let a = |args: &str| e(&format!("exe = \"eden\"\nargs = {args}"));
        let ok = a("[\"{rom}\"]");
        let g = |body: &str| format!("{ok}\n[[games]]\nid = \"x\"\nname = \"X\"\n{body}");
        let m = |body: &str| g(&format!("emulator = \"eden\"\nrom = \"r\"\n{body}"));
        let cases = [
            (
                g("emulator = \"nope\"\nrom = \"r\""),
                "games[0]: \"x\": emulator \"nope\" is not defined",
            ),
            (g("rom = \"r\""), "rom is only allowed with emulator"),
            (
                g("exe = \"a\"\nrom = \"r\""),
                "rom is only allowed with emulator",
            ),
            (g("emulator = \"eden\""), "emulator needs a rom"),
            (
                g("emulator = \"eden\"\nrom = \"\""),
                "rom must not be empty",
            ),
            (m("steam_appid = 1"), "only one of"),
            (m("exe = \"a\""), "only one of"),
            (m("args = []"), "only allowed with exe"),
            (m("cwd = \"/\""), "only allowed with exe"),
            (
                a("[]"),
                "emulators[0]: \"eden\": args must contain \"{rom}\" exactly once",
            ),
            (a("[\"-g\"]"), "exactly once"),
            (a("[\"{rom}\", \"{rom}\"]"), "exactly once"),
            (a("[\"{rom}\", \"--rom={rom}\"]"), "whole argument"),
            (a("[\"-g {rom}\"]"), "whole argument"),
            (e("exe = \"eden\""), "missing field `args`"),
            (e("exe = \"\"\nargs = [\"{rom}\"]"), "exe must not be empty"),
            (
                e("exe = \"a\"\nargs = [\"{rom}\"]\ncwd = \"\""),
                "cwd must not be empty",
            ),
            (
                e("exe = \"a\"\nargs = [\"{rom}\"]\nrom = \"r\""),
                "unknown field `rom`",
            ),
            (format!("{ok}\n{ok}"), "emulators[1]: duplicate id \"eden\""),
            (
                "[[emulators]]\nid = \"Eden\"\nname = \"E\"\nexe = \"e\"\nargs = [\"{rom}\"]"
                    .into(),
                "invalid slug",
            ),
            (
                "[[emulators]]\nid = \"e\"\nname = \"\"\nexe = \"e\"\nargs = [\"{rom}\"]".into(),
                "name must not be empty",
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
    fn missing_file_is_empty_library_only_by_default() {
        let path = std::env::temp_dir().join("mini-launcher-test-does-not-exist/config.toml");
        assert_eq!(Config::load_or_empty(&path).unwrap(), Config::default());
        let err = Config::load(&path).unwrap_err();
        assert!(
            matches!(&err, ConfigError::Io { source, .. } if source.kind() == std::io::ErrorKind::NotFound),
            "{err:?}"
        );
    }
}
