//! Battle.net games: a catalog of Blizzard launch codes, detection of the
//! installed ones, and the code type a `battlenet = "..."` entry holds.
//!
//! Battle.net has no public manifest of installed games, so detection looks
//! in the folders Blizzard's uninstall entries name (and the default install
//! folders) for each catalog game's executable. World of Warcraft keeps every
//! flavor in one folder, each in its own subfolder (`_retail_`, `_classic_`,
//! `_forever_`), so those are found by subfolder.
//!
//! Games are launched with `Battle.net.exe --exec="launch <code>"`, which
//! starts Battle.net if needed. Codes come from the catalog or the user's
//! config, never from a tag, and must match `^[A-Za-z0-9_]{1,32}$`.

use std::fmt;
use std::path::{Path, PathBuf};

/// A Battle.net launch code such as `WoW`, `WoWF` or `Pro`: 1-32 ASCII
/// letters, digits or underscores, so it is safe on a command line.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Code(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid Battle.net code (expected 1-32 letters, digits or underscores, e.g. WoW)")]
pub struct InvalidCode;

impl Code {
    pub fn new(s: &str) -> Result<Code, InvalidCode> {
        let ok = (1..=32).contains(&s.len())
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if ok {
            Ok(Code(s.to_owned()))
        } else {
            Err(InvalidCode)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The `--exec` argument that launches `code`, exactly as Blizzard's own
/// shortcuts write it.
pub fn exec_arg(code: &Code) -> String {
    format!("--exec=\"launch {code}\"")
}

/// A game Battle.net can launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub code: &'static str,
    pub name: &'static str,
    /// Uninstall-entry uids (`--uid=<uid>`, with or without a locale suffix
    /// like `_enus`) that mean this game is installed.
    pub uids: &'static [&'static str],
    /// The usual install folder name, probed under Program Files.
    pub folder: &'static str,
    /// For a WoW flavor: its subfolder of the WoW folder. Then any `.exe`
    /// in it counts, as the executable names change between patches.
    pub flavor: Option<&'static str>,
    /// Executables in the install folder that mean it is installed.
    pub exes: &'static [&'static str],
}

const WOW: &str = "World of Warcraft";

const fn game(
    code: &'static str,
    name: &'static str,
    uids: &'static [&'static str],
    folder: &'static str,
    exes: &'static [&'static str],
) -> Known {
    Known {
        code,
        name,
        uids,
        folder,
        flavor: None,
        exes,
    }
}

const fn wow(code: &'static str, name: &'static str, flavor: &'static str) -> Known {
    Known {
        code,
        name,
        uids: &[],
        folder: WOW,
        flavor: Some(flavor),
        exes: &[],
    }
}

/// Launch codes from Blizzard's shortcuts and community launchers (e.g.
/// ShizCalev/Battlenet-Launcher). WoW Classic Era has no launch code of its
/// own (`WoWC` starts whichever Classic was last chosen), so it is left out;
/// any other code can still be entered by hand.
pub const CATALOG: &[Known] = &[
    wow("WoW", "World of Warcraft", "_retail_"),
    wow("WoWC", "World of Warcraft Classic", "_classic_"),
    wow("WoWF", "World of Warcraft: Forever", "_forever_"),
    game(
        "Pro",
        "Overwatch",
        &["prometheus"],
        "Overwatch",
        &["Overwatch.exe", "Overwatch Launcher.exe"],
    ),
    game(
        "Fen",
        "Diablo IV",
        &["fenris"],
        "Diablo IV",
        &["Diablo IV.exe", "Diablo IV Launcher.exe"],
    ),
    game(
        "D3",
        "Diablo III",
        &["diablo3"],
        "Diablo III",
        &["Diablo III.exe", "Diablo III Launcher.exe"],
    ),
    game(
        "OSI",
        "Diablo II: Resurrected",
        &["osi"],
        "Diablo II Resurrected",
        &["D2R.exe", "Diablo II Resurrected Launcher.exe"],
    ),
    game(
        "ANBS",
        "Diablo Immortal",
        &["anbs"],
        "Diablo Immortal",
        &["DiabloImmortal.exe", "Diablo Immortal Launcher.exe"],
    ),
    game("D1", "Diablo", &["drtl"], "Diablo", &["Diablo.exe"]),
    game(
        "WTCG",
        "Hearthstone",
        &["hs_beta"],
        "Hearthstone",
        &["Hearthstone.exe", "Hearthstone Beta Launcher.exe"],
    ),
    game(
        "Hero",
        "Heroes of the Storm",
        &["heroes"],
        "Heroes of the Storm",
        &["Heroes of the Storm.exe"],
    ),
    game(
        "S2",
        "StarCraft II",
        &["s2"],
        "StarCraft II",
        &["StarCraft II.exe", "SC2Switcher.exe"],
    ),
    game(
        "S1",
        "StarCraft: Remastered",
        &["s1"],
        "StarCraft",
        &["StarCraft.exe", "StarCraft Launcher.exe"],
    ),
    game(
        "W3",
        "Warcraft III: Reforged",
        &["w3"],
        "Warcraft III",
        &["Warcraft III.exe", "Warcraft III Launcher.exe"],
    ),
    game(
        "W1R",
        "Warcraft I: Remastered",
        &["w1r"],
        "Warcraft I Remastered",
        &["Warcraft.exe"],
    ),
    game(
        "W2R",
        "Warcraft II: Remastered",
        &["w2r"],
        "Warcraft II Remastered",
        &["Warcraft II.exe"],
    ),
    game(
        "RTRO",
        "Blizzard Arcade Collection",
        &["rtro"],
        "Blizzard Arcade Collection",
        &["client.exe"],
    ),
    game(
        "WLBY",
        "Crash Bandicoot 4: It's About Time",
        &["wlby"],
        "Crash Bandicoot 4",
        &["CrashBandicoot4.exe"],
    ),
];

/// The catalog entry for `code`, matched case-sensitively like Battle.net.
pub fn known(code: &Code) -> Option<&'static Known> {
    CATALOG.iter().find(|k| k.code == code.as_str())
}

/// An installed Battle.net game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BattleNetGame {
    pub code: Code,
    pub name: String,
    /// Where it was found (display only).
    pub location: PathBuf,
}

/// A Blizzard uninstall entry: its `InstallLocation` and the `--uid=` from
/// its `UninstallString`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uninstall {
    pub location: PathBuf,
    pub uid: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum BattleNetError {
    #[error("Battle.net is not installed (Battle.net.exe not found)")]
    NotFound,
    #[error("Battle.net games can only be launched on Windows")]
    Unsupported,
}

/// Installed Battle.net games, sorted by name. Empty when Battle.net or
/// its games are not installed (and on other OSes).
pub fn installed_games() -> Vec<BattleNetGame> {
    let entries = crate::platform::blizzard_uninstalls();
    let mut dirs: Vec<PathBuf> = entries.iter().map(|e| e.location.clone()).collect();
    for base in crate::platform::program_files() {
        for k in CATALOG {
            dirs.push(base.join(k.folder));
        }
    }
    games_in(&entries, &dirs)
}

/// The catalog games installed according to `entries` (matched by uid) or
/// found in `dirs` (by executable or WoW subfolder), one per code.
pub fn games_in(entries: &[Uninstall], dirs: &[PathBuf]) -> Vec<BattleNetGame> {
    let mut games: Vec<BattleNetGame> = Vec::new();
    for k in CATALOG {
        let by_uid = entries.iter().find(|e| {
            e.uid
                .as_deref()
                .is_some_and(|uid| k.uids.iter().any(|known| uid_matches(uid, known)))
        });
        let location = by_uid
            .map(|e| e.location.clone())
            .or_else(|| dirs.iter().find_map(|d| found_in(k, d)));
        if let Some(location) = location {
            games.push(BattleNetGame {
                // The catalog codes are valid.
                code: Code(k.code.to_owned()),
                name: k.name.to_owned(),
                location,
            });
        }
    }
    games.sort_by_cached_key(|g| g.name.to_lowercase());
    games
}

/// `uid` is `known`, or `known` plus a locale suffix (`wow_enus`).
fn uid_matches(uid: &str, known: &str) -> bool {
    let uid = uid.to_ascii_lowercase();
    match uid.strip_prefix(known) {
        Some("") => true,
        Some(rest) => {
            let locale = rest.strip_prefix('_').unwrap_or_default();
            locale.len() == 4 && locale.bytes().all(|b| b.is_ascii_alphabetic())
        }
        None => false,
    }
}

/// Where `k` is installed if `dir` holds it: for a WoW flavor its subfolder
/// of `dir` (or `dir` itself, when it is that subfolder) with an `.exe`
/// in it; else `dir` when one of `k`'s executables is in it.
fn found_in(k: &Known, dir: &Path) -> Option<PathBuf> {
    match k.flavor {
        Some(flavor) => {
            let is_flavor = dir
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(flavor));
            let sub = if is_flavor {
                dir.to_path_buf()
            } else {
                dir.join(flavor)
            };
            has_exe(&sub).then_some(sub)
        }
        None => k
            .exes
            .iter()
            .any(|exe| dir.join(exe).is_file())
            .then(|| dir.to_path_buf()),
    }
}

fn has_exe(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().take(10_000).any(|e| {
        let name = e.file_name();
        let exe = Path::new(&name)
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("exe"));
        exe && e.path().is_file()
    })
}

/// The `--uid=` value in a Blizzard uninstall command line, such as
/// `"...\Blizzard Uninstaller.exe" --lang=enUS --uid=wow_enus --displayname="..."`.
pub fn uninstall_uid(command: &str) -> Option<String> {
    let rest = command.split("--uid=").nth(1)?;
    let uid: String = rest
        .trim_start_matches('"')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .collect();
    (!uid.is_empty()).then_some(uid)
}

/// Whether an uninstall command line belongs to a Battle.net game.
pub fn is_blizzard_uninstaller(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("blizzard uninstaller") || lower.contains("battle.net")
}

/// `Battle.net.exe`, if Battle.net is installed.
pub fn client_exe() -> Result<PathBuf, BattleNetError> {
    if !cfg!(windows) {
        return Err(BattleNetError::Unsupported);
    }
    crate::platform::battlenet_dirs()
        .into_iter()
        .map(|d| d.join("Battle.net.exe"))
        .find(|p| p.is_file())
        .ok_or(BattleNetError::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(s: &str) -> Code {
        Code::new(s).unwrap()
    }

    #[test]
    fn codes() {
        for ok in ["WoW", "WoWF", "Pro", "wow_classic_era", &"a".repeat(32)] {
            assert_eq!(Code::new(ok).unwrap().as_str(), ok);
        }
        for bad in [
            "",
            "WoW F",
            "WoW\"",
            "launch;calc",
            "Wo-W",
            &"a".repeat(33),
            "é",
        ] {
            assert_eq!(Code::new(bad), Err(InvalidCode), "{bad:?}");
        }
        assert_eq!(exec_arg(&code("WoWF")), r#"--exec="launch WoWF""#);
        assert_eq!(
            known(&code("WoWF")).unwrap().name,
            "World of Warcraft: Forever"
        );
        assert_eq!(known(&code("wowf")), None);
    }

    #[test]
    fn catalog_is_valid() {
        let mut seen = std::collections::HashSet::new();
        for k in CATALOG {
            assert!(Code::new(k.code).is_ok(), "{}", k.code);
            assert!(seen.insert(k.code), "duplicate {}", k.code);
            assert!(k.flavor.is_some() != !k.exes.is_empty(), "{}", k.code);
        }
    }

    #[test]
    fn uninstall_strings() {
        let cmd = r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS --uid=wow_enus --displayname="World of Warcraft""#;
        assert!(is_blizzard_uninstaller(cmd));
        assert_eq!(uninstall_uid(cmd).as_deref(), Some("wow_enus"));
        assert_eq!(
            uninstall_uid("x --uid=\"prometheus\"").as_deref(),
            Some("prometheus")
        );
        assert_eq!(uninstall_uid("x --uid= y"), None);
        assert_eq!(uninstall_uid("x"), None);
        assert!(!is_blizzard_uninstaller(r#""C:\Steam\uninstall.exe""#));
        assert!(uid_matches("prometheus", "prometheus"));
        assert!(uid_matches("Hs_Beta_enUS", "hs_beta"));
        assert!(!uid_matches("s2x", "s2"));
        assert!(!uid_matches("s2_classic", "s2"));
        assert!(!uid_matches("s1", "s2"));
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    #[test]
    fn detects_games() {
        let root = std::env::temp_dir().join(format!("ml-bnet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let wow = root.join("World of Warcraft");
        touch(&wow.join("_retail_/Wow.exe"));
        touch(&wow.join("_forever_/WowForever.exe"));
        // A flavor folder without an executable is left over, not installed.
        std::fs::create_dir_all(wow.join("_classic_")).unwrap();
        let d4 = root.join("Diablo IV");
        touch(&d4.join("Diablo IV.exe"));
        let ow = root.join("Games/OW");
        std::fs::create_dir_all(&ow).unwrap();

        let entries = [
            Uninstall {
                location: wow.clone(),
                uid: Some("wow_enus".into()),
            },
            // Found by uid alone: Overwatch's folder has no known exe.
            Uninstall {
                location: ow.clone(),
                uid: Some("prometheus".into()),
            },
        ];
        let dirs = [wow.clone(), d4.clone(), root.join("missing")];
        let games = games_in(&entries, &dirs);
        let got: Vec<_> = games
            .iter()
            .map(|g| (g.code.as_str(), g.location.clone()))
            .collect();
        assert_eq!(
            got,
            [
                ("Fen", d4),
                ("Pro", ow),
                ("WoW", wow.join("_retail_")),
                ("WoWF", wow.join("_forever_")),
            ]
        );
        // An uninstall entry pointing at the flavor folder itself.
        let flavor = wow.join("_forever_");
        let games = games_in(&[], std::slice::from_ref(&flavor));
        assert_eq!(games.len(), 1);
        assert_eq!(
            (games[0].code.as_str(), &games[0].location),
            ("WoWF", &flavor)
        );
        assert_eq!(games_in(&[], &[]), []);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
