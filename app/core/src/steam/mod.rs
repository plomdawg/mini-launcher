//! Installed Steam games: find Steam, read `steamapps/libraryfolders.vdf`,
//! then each library's `appmanifest_<appid>.acf`. The files are untrusted:
//! sizes are bounded, bad files are skipped with a warning, the appid must be
//! a `u32`, and names are display-only (control characters stripped).
//!
//! Non-Steam games added to Steam ("shortcuts") come from each user's
//! binary `userdata/<id>/config/shortcuts.vdf`, read under the same rules.

pub mod binvdf;
pub mod vdf;

use std::collections::HashSet;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use vdf::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamGame {
    pub appid: u32,
    pub name: String,
    /// Folder name under `steamapps/common`; for a shortcut, its target
    /// program (display only).
    pub installdir: String,
    /// The library folder holding the game; for a shortcut, the
    /// `userdata/<id>` folder it was found in.
    pub library: PathBuf,
    pub kind: SteamKind,
}

/// Where a [`SteamGame`] came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SteamKind {
    /// A store app: launched with `steam://rungameid/<appid>`.
    #[default]
    App,
    /// A non-Steam game added to Steam: `appid` is the shortcut's 32-bit
    /// id, launched through Steam with
    /// [`crate::payload::steam_shortcut_uri`].
    Shortcut,
}

#[derive(Debug, thiserror::Error)]
pub enum SteamError {
    #[error("Steam is not installed (no Steam folder with a steamapps directory found)")]
    NotFound,
}

/// Apps that are not games: shared redistributables and Linux compatibility
/// tools. Names are matched too, as each Proton/runtime version is a new app.
const NOT_GAMES: &[u32] = &[
    228980,  // Steamworks Common Redistributables
    1070560, // Steam Linux Runtime 1.0 (scout)
    1391110, // Steam Linux Runtime 2.0 (soldier)
    1628350, // Steam Linux Runtime 3.0 (sniper)
    1493710, // Proton Experimental
];
const NOT_GAME_PREFIXES: &[&str] = &["Proton ", "Steam Linux Runtime", "Steamworks "];
const MAX_LIBRARIES: usize = 64;
const MAX_MANIFESTS: usize = 20_000;
/// `userdata` folders looked at (one per Steam account used on this PC).
const MAX_USERS: usize = 64;
/// Shortcuts taken from one `shortcuts.vdf`.
const MAX_SHORTCUTS: usize = 10_000;

/// Installed games across all libraries, then the non-Steam shortcuts of
/// every user, all sorted by name.
pub fn installed_games() -> Result<Vec<SteamGame>, SteamError> {
    let root = steam_root().ok_or(SteamError::NotFound)?;
    let mut games = games_in(&libraries(&root));
    games.extend(shortcuts_in(&root));
    games.sort_by_cached_key(|g| {
        (
            g.name.to_lowercase(),
            g.kind == SteamKind::Shortcut,
            g.appid,
        )
    });
    Ok(games)
}

/// The first Steam folder candidate that has `steamapps`.
pub fn steam_root() -> Option<PathBuf> {
    #[allow(unused_mut)]
    let mut candidates = crate::platform::steam_paths();
    #[cfg(target_os = "linux")]
    if let Some(dirs) = directories::BaseDirs::new() {
        let flatpak = ".var/app/com.valvesoftware.Steam/.local/share/Steam";
        for rel in [".steam/steam", ".local/share/Steam", flatpak] {
            candidates.push(dirs.home_dir().join(rel));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.join("steamapps").is_dir())
}

/// `root` plus the folders listed in its `libraryfolders.vdf`, which is either
/// `"1" { "path" "D:\\Lib" ... }` or (older) `"1" "D:\\Lib"`.
pub fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![root.to_path_buf()];
    let file = root.join("steamapps/libraryfolders.vdf");
    let doc = match read_vdf(&file) {
        Ok(doc) => doc,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return libs,
        Err(e) => {
            tracing::warn!("ignoring {}: {e}", file.display());
            return libs;
        }
    };
    let folders = doc
        .get("libraryfolders")
        .map(Value::entries)
        .unwrap_or_default();
    for (key, v) in folders {
        let path = v.as_str().or_else(|| v.get("path")?.as_str());
        if let Some(p) = path.filter(|p| !p.is_empty() && key.parse::<u32>().is_ok()) {
            if libs.len() < MAX_LIBRARIES && !libs.iter().any(|l| l == Path::new(p)) {
                libs.push(p.into());
            }
        }
    }
    libs
}

/// Games from the manifests in each library's `steamapps`, deduplicated by
/// appid and sorted by name. Unreadable libraries and manifests are skipped.
pub fn games_in(libs: &[PathBuf]) -> Vec<SteamGame> {
    let (mut games, mut seen) = (Vec::new(), HashSet::new());
    for lib in libs {
        let Ok(entries) = std::fs::read_dir(lib.join("steamapps")) else {
            tracing::debug!("skipping unreadable library {}", lib.display());
            continue;
        };
        for entry in entries.flatten().take(MAX_MANIFESTS) {
            let name = entry.file_name();
            let id = name
                .to_str()
                .and_then(|n| n.strip_prefix("appmanifest_")?.strip_suffix(".acf"));
            if !id.is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())) {
                continue;
            }
            match read_manifest(&entry.path(), lib) {
                Ok(Some(game)) if seen.insert(game.appid) => games.push(game),
                Ok(_) => {}
                Err(e) => tracing::warn!("skipping {}: {e}", entry.path().display()),
            }
        }
    }
    games.sort_by_cached_key(|g| (g.name.to_lowercase(), g.appid));
    games
}

/// `Ok(None)` for apps that are not games.
fn read_manifest(path: &Path, library: &Path) -> io::Result<Option<SteamGame>> {
    let doc = read_vdf(path)?;
    let state = doc.get("AppState");
    let field = |key| state.and_then(|s| s.get(key)?.as_str()).unwrap_or_default();
    let appid = Some(field("appid"))
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|&n| n != 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing or invalid appid"))?;
    let name = match display_text(field("name")) {
        name if name.is_empty() => format!("App {appid}"),
        name => name,
    };
    if NOT_GAMES.contains(&appid) || NOT_GAME_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return Ok(None);
    }
    let installdir = display_text(field("installdir"));
    let library = library.to_path_buf();
    Ok(Some(SteamGame {
        appid,
        name,
        installdir,
        library,
        kind: SteamKind::App,
    }))
}

/// The visible shortcuts in `root/userdata/<digits>/config/shortcuts.vdf`,
/// deduplicated by appid across users and sorted by name. Unreadable or
/// malformed files are skipped.
pub fn shortcuts_in(root: &Path) -> Vec<SteamGame> {
    let (mut games, mut seen) = (Vec::new(), HashSet::new());
    let Ok(users) = std::fs::read_dir(root.join("userdata")) else {
        return games;
    };
    let users = users.flatten().filter(|e| {
        let name = e.file_name();
        name.to_str()
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
    });
    for user in users.take(MAX_USERS) {
        let file = user.path().join("config/shortcuts.vdf");
        let doc = match read_binvdf(&file) {
            Ok(doc) => doc,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!("ignoring {}: {e}", file.display());
                continue;
            }
        };
        let entries = doc.get("shortcuts").map(binvdf::Value::entries);
        for (_, entry) in entries.unwrap_or_default().iter().take(MAX_SHORTCUTS) {
            match shortcut(entry, &user.path()) {
                Some(game) if seen.insert(game.appid) => games.push(game),
                _ => {}
            }
        }
    }
    games.sort_by_cached_key(|g| (g.name.to_lowercase(), g.appid));
    games
}

/// One `shortcuts` entry, or `None` if it is hidden or has no usable name.
fn shortcut(entry: &binvdf::Value, user: &Path) -> Option<SteamGame> {
    let text = |key| entry.get(key).and_then(binvdf::Value::as_str);
    let int = |key| entry.get(key).and_then(binvdf::Value::as_int);
    if int("IsHidden").is_some_and(|n| n != 0) {
        return None;
    }
    let raw_name = text("AppName")?;
    let name = display_text(raw_name);
    if name.is_empty() {
        return None;
    }
    let exe = text("Exe").unwrap_or_default();
    let appid = int("appid")
        // The file stores the u32 id as a signed int32.
        .map(|n| n as u32)
        .filter(|&n| n != 0)
        .unwrap_or_else(|| shortcut_appid(exe, raw_name));
    Some(SteamGame {
        appid,
        name,
        installdir: display_text(exe.trim().trim_matches('"')),
        library: user.to_path_buf(),
        kind: SteamKind::Shortcut,
    })
}

/// The id Steam gives a shortcut whose file has no `appid` (older files):
/// CRC-32 of the quoted exe followed by the name, with the top bit set.
pub fn shortcut_appid(exe: &str, name: &str) -> u32 {
    let quoted = if exe.starts_with('"') {
        exe.to_owned()
    } else {
        format!("\"{exe}\"")
    };
    let mut crc = crc32fast::Hasher::new();
    crc.update(quoted.as_bytes());
    crc.update(name.as_bytes());
    crc.finalize() | 0x8000_0000
}

/// Control characters (e.g. terminal escapes) removed, at most 256 chars.
fn display_text(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(256).collect();
    s.trim().to_owned()
}

/// Reads at most [`binvdf::MAX_LEN`] bytes; parse errors are `InvalidData`.
fn read_binvdf(path: &Path) -> io::Result<binvdf::Value> {
    let mut bytes = Vec::new();
    let limit = binvdf::MAX_LEN as u64 + 1;
    std::fs::File::open(path)?
        .take(limit)
        .read_to_end(&mut bytes)?;
    binvdf::parse(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Reads at most [`vdf::MAX_LEN`] bytes; parse errors are `InvalidData`.
fn read_vdf(path: &Path) -> io::Result<Value> {
    let mut bytes = Vec::new();
    let limit = vdf::MAX_LEN as u64 + 1;
    std::fs::File::open(path)?
        .take(limit)
        .read_to_end(&mut bytes)?;
    // An oversized file is read as MAX_LEN + 1 bytes, so parse says TooLarge.
    vdf::parse(&String::from_utf8_lossy(&bytes))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/steam")
            .join(rel)
    }

    #[test]
    fn lists_games_across_libraries() {
        let libs = [fixture("root"), fixture("library2"), fixture("missing")];
        let games = games_in(&libs);
        let got: Vec<_> = games.iter().map(|g| (g.appid, g.name.as_str())).collect();
        // Skipped: Proton + redistributables, the malformed and non-u32
        // manifests, and library2's duplicate of 220.
        let want = [
            (400, "App 400"),
            (220, "Half-Life 2"),
            (70, "Half-Life: \"Source\""),
            (620, "Portal 2"),
        ];
        assert_eq!(got, want);
        assert_eq!(
            (&games[1].library, games[1].installdir.as_str()),
            (&libs[0], "Half-Life 2")
        );
        assert_eq!(games[2].installdir, r"Half-Life\Source");
        assert_eq!(games[3].library, libs[1]);
    }

    #[test]
    fn reads_libraryfolders_and_tolerates_missing_or_bad_files() {
        let root = fixture("root");
        assert_eq!(libraries(&root), std::slice::from_ref(&root));
        let dir = TempDir::new("steam");
        let file = dir.join("steamapps/libraryfolders.vdf");
        std::fs::create_dir_all(dir.join("steamapps")).unwrap();
        std::fs::copy(fixture("libraryfolders.vdf"), &file).unwrap();
        let want = [
            &*dir,
            Path::new(r"C:\Program Files (x86)\Steam"),
            Path::new(r"D:\SteamLibrary"),
        ];
        assert_eq!(libraries(&dir), want);
        let lib2 = fixture("library2")
            .display()
            .to_string()
            .replace('\\', "\\\\");
        std::fs::write(
            &file,
            format!("libraryfolders {{ x y 1 \"{lib2}\" 2 \"\" }}"),
        )
        .unwrap();
        assert_eq!(libraries(&dir), [dir.to_path_buf(), fixture("library2")]);
        std::fs::write(&file, "\"libraryfolders\" {").unwrap();
        assert_eq!(libraries(&dir), [dir.to_path_buf()]);
    }

    #[test]
    fn lists_visible_shortcuts_of_every_user() {
        let root = fixture("shortcuts");
        let games = shortcuts_in(&root);
        let got: Vec<_> = games
            .iter()
            .map(|g| (g.appid, g.name.as_str(), g.installdir.as_str()))
            .collect();
        // Skipped: the hidden and blank-named entries, user 2002's copy of
        // Doom (same appid), user 3003's truncated file and the folder that
        // is not a user id.
        let want = [
            (0xFFFF_FFFA, "[31mRetro", r"C:\Retro\retro.exe"),
            (0xC0FF_EE01, "Doom", r"C:\Games\Doom\gzdoom.exe"),
            (0xFFFF_FFF8, "Lower Keys", r"C:\lower.exe"),
            // No appid (older files) or 0: computed like Steam does.
            (0xF73A_352C, "Old Game", r"C:\Old\old.exe"),
            (0xEB6C_6CD4, "Zero Id", r"C:\Zero\zero.exe"),
        ];
        assert_eq!(got, want);
        assert!(games.iter().all(|g| g.kind == SteamKind::Shortcut));
        assert_eq!(games[1].library, root.join("userdata").join("1001"));
        assert_eq!(games[3].library, root.join("userdata").join("2002"));
        // No userdata folder at all.
        assert_eq!(shortcuts_in(&fixture("root")), []);
    }

    #[test]
    fn shortcut_appid_matches_steam() {
        // Quoted or not, the exe is hashed with its quotes.
        assert_eq!(
            shortcut_appid(r#""C:\Old\old.exe""#, "Old Game"),
            0xF73A_352C
        );
        assert_eq!(shortcut_appid(r"C:\Zero\zero.exe", "Zero Id"), 0xEB6C_6CD4);
        assert!(shortcut_appid("", "") >= 0x8000_0000);
    }

    #[test]
    fn shortcuts_tolerate_hostile_files() {
        use binvdf::build;
        let dir = TempDir::new("shortcuts");
        let file = dir.join("userdata/7/config/shortcuts.vdf");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mut ok = Vec::new();
        build::map(&mut ok, "shortcuts", |b| {
            build::map(b, "0", |b| build::str(b, "AppName", "Fine"));
            // Wrong types: a map as the name, a string as the id.
            build::map(b, "1", |b| build::map(b, "AppName", |_| {}));
            build::map(b, "2", |b| {
                build::str(b, "appid", "123");
                build::str(b, "AppName", "Str Id");
                build::int(b, "IsHidden", 2);
            });
            build::str(b, "3", "not a map");
        });
        std::fs::write(&file, &ok).unwrap();
        let names: Vec<_> = shortcuts_in(&dir).into_iter().map(|g| g.name).collect();
        assert_eq!(names, ["Fine"]);
        for bad in [
            &b"\xff\xff"[..],
            b"\x00shortcuts\x00\x00",
            &ok[..ok.len() / 2],
        ] {
            std::fs::write(&file, bad).unwrap();
            assert_eq!(shortcuts_in(&dir), [], "{bad:?}");
        }
        std::fs::write(&file, vec![0x08; binvdf::MAX_LEN + 10]).unwrap();
        assert_eq!(shortcuts_in(&dir), []);
    }

    #[test]
    fn display_text_strips_control_characters() {
        assert_eq!(display_text(" a\u{1b}[31mb\n "), "a[31mb");
        assert_eq!(display_text(&"x".repeat(1000)).len(), 256);
    }
}
