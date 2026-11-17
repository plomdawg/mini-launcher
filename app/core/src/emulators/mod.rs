//! Emulator presets and ROM folder scanning.
//!
//! A preset knows an emulator's default launch args, its ROM file extensions
//! and where its exe usually lives. Detection and scanning are best-effort:
//! anything missing or unreadable is skipped quietly. File names are
//! untrusted: names are display-only and suggested slugs are validated.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::payload::Slug;

/// A known emulator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmulatorPreset {
    pub name: &'static str,
    /// Default launch args; `{rom}` stands for the ROM path.
    pub default_args: &'static [&'static str],
    /// Lowercase ROM extensions without the dot.
    pub rom_extensions: &'static [&'static str],
    /// Exe locations relative to `%LOCALAPPDATA%`, most likely first.
    pub local_app_data_exes: &'static [&'static str],
}

/// Eden, a Switch emulator: fullscreen (`-f`), then the game (`-g`).
pub const EDEN: EmulatorPreset = EmulatorPreset {
    name: "Eden",
    default_args: &["-f", "-g", "{rom}"],
    rom_extensions: &["nsp", "xci", "nca", "nro", "nso"],
    local_app_data_exes: &["Programs/eden/eden.exe", "eden/eden.exe"],
};

/// All presets, for pickers.
pub const PRESETS: &[EmulatorPreset] = &[EDEN];

/// Default cap on [`scan_roms`] results.
pub const MAX_ROMS: usize = 2000;
/// Directory entries looked at per folder, so a huge folder can't stall a scan.
const MAX_ENTRIES: usize = 50_000;

impl EmulatorPreset {
    /// Whether `path` has one of this preset's ROM extensions (any case).
    pub fn is_rom(&self, path: &Path) -> bool {
        path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
            self.rom_extensions
                .iter()
                .any(|e| e.eq_ignore_ascii_case(ext))
        })
    }

    /// The installed exe, if found in a usual location. `None` when not
    /// installed or on platforms without `%LOCALAPPDATA%`.
    pub fn detect_exe(&self) -> Option<PathBuf> {
        let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
        self.detect_exe_in(Path::new(&local))
    }

    /// [`detect_exe`](Self::detect_exe) with an explicit `%LOCALAPPDATA%`.
    pub fn detect_exe_in(&self, local_app_data: &Path) -> Option<PathBuf> {
        self.local_app_data_exes
            .iter()
            .map(|rel| local_app_data.join(rel))
            .find(|p| p.is_file())
    }
}

/// A ROM found by [`scan_roms`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomFile {
    pub path: PathBuf,
    /// File stem without `[TitleID]`/`(USA)`-style tags (display only).
    pub name: String,
    /// A library id derived from `name`; `None` if nothing usable is left
    /// (e.g. a name with no ASCII letters or digits).
    pub slug: Option<Slug>,
}

/// Options for [`scan_roms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    /// Also look one level into subfolders.
    pub recurse_one_level: bool,
    /// Stop after this many ROMs.
    pub limit: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            recurse_one_level: false,
            limit: MAX_ROMS,
        }
    }
}

/// ROM files in `dir`, sorted by name then path. Symlinks pointing outside
/// `dir` are skipped and symlinked folders are never entered; unreadable
/// entries are ignored. Returns an empty list if `dir` can't be read.
pub fn scan_roms(dir: &Path, preset: &EmulatorPreset, opts: ScanOptions) -> Vec<RomFile> {
    let Ok(root) = fs::canonicalize(dir) else {
        tracing::debug!("cannot read ROM folder {}", dir.display());
        return Vec::new();
    };
    let mut roms = Vec::new();
    let mut subdirs = Vec::new();
    // Paths stay under `dir` as given; `root` is only for the symlink check
    // (canonical Windows paths are `\\?\` paths, which some emulators reject).
    scan_dir(
        &root,
        dir,
        preset,
        opts.limit,
        &mut roms,
        Some(&mut subdirs),
    );
    if opts.recurse_one_level {
        subdirs.sort();
        for sub in subdirs {
            scan_dir(&root, &sub, preset, opts.limit, &mut roms, None);
        }
    }
    roms.sort_by_cached_key(|r| (r.name.to_lowercase(), r.path.clone()));
    roms
}

fn scan_dir(
    root: &Path,
    dir: &Path,
    preset: &EmulatorPreset,
    limit: usize,
    roms: &mut Vec<RomFile>,
    mut subdirs: Option<&mut Vec<PathBuf>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        tracing::debug!("skipping unreadable folder {}", dir.display());
        return;
    };
    for entry in entries.flatten().take(MAX_ENTRIES) {
        if roms.len() >= limit {
            return;
        }
        // `file_type` does not follow symlinks.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            if let Some(subdirs) = subdirs.as_deref_mut() {
                subdirs.push(path);
            }
            continue;
        }
        if !preset.is_rom(&path) {
            continue;
        }
        if kind.is_symlink() {
            let inside = fs::canonicalize(&path)
                .is_ok_and(|target| target.starts_with(root) && target.is_file());
            if !inside {
                continue;
            }
        } else if !kind.is_file() {
            continue;
        }
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let name = display_name(&stem);
        let slug = suggest_slug(&name);
        roms.push(RomFile { path, name, slug });
    }
}

/// `stem` without `[...]`/`(...)` tags, control characters and extra spaces;
/// the cleaned stem itself if stripping would leave nothing.
pub fn display_name(stem: &str) -> String {
    let (mut out, mut depth) = (String::new(), 0usize);
    for c in stem.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            c if depth == 0 => out.push(if c.is_control() { ' ' } else { c }),
            _ => {}
        }
    }
    let tidy = |s: &str| {
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        let s = s.trim_matches(|c: char| c == '-' || c == '_' || c.is_whitespace());
        s.chars().take(256).collect::<String>()
    };
    match tidy(&out) {
        name if name.is_empty() => tidy(&stem.replace(char::is_control, " ")),
        name => name,
    }
}

/// A slug from a display name: lowercase ASCII letters and digits, with runs
/// of anything else turned into one `-`. Accented Latin letters lose their
/// accent; other non-ASCII letters are dropped.
pub fn suggest_slug(name: &str) -> Option<Slug> {
    let mut s = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        let c = fold_accent(c);
        if c.is_ascii_alphanumeric() {
            s.push(c);
        } else if !c.is_alphanumeric() && !s.is_empty() && !s.ends_with('-') {
            s.push('-');
        }
    }
    s.truncate(Slug::MAX_LEN);
    Slug::new(s.trim_end_matches('-')).ok()
}

fn fold_accent(c: char) -> char {
    match c {
        'à'..='å' => 'a',
        'ç' => 'c',
        'è'..='ë' => 'e',
        'ì'..='ï' => 'i',
        'ñ' => 'n',
        'ò'..='ö' | 'ø' => 'o',
        'ù'..='ü' => 'u',
        'ý' | 'ÿ' => 'y',
        c => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir()
                .join(format!("mini-launcher-roms-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }

        fn touch(&self, rel: &str) -> PathBuf {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"").unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(roms: &[RomFile]) -> Vec<(&str, Option<&str>)> {
        roms.iter()
            .map(|r| (r.name.as_str(), r.slug.as_ref().map(Slug::as_str)))
            .collect()
    }

    #[test]
    fn eden_preset() {
        assert_eq!(EDEN.name, "Eden");
        assert_eq!(EDEN.default_args, ["-f", "-g", "{rom}"]);
        assert!(EDEN.is_rom(Path::new("a.NSP")));
        assert!(EDEN.is_rom(Path::new("dir/a.Xci")));
        assert!(!EDEN.is_rom(Path::new("a.nsp.txt")));
        assert!(!EDEN.is_rom(Path::new("nsp")));
        assert_eq!(PRESETS, [EDEN]);
    }

    #[test]
    fn detects_the_exe_in_usual_locations_only() {
        let dir = TempDir::new("detect");
        assert_eq!(EDEN.detect_exe_in(&dir.0), None);
        fs::create_dir_all(dir.0.join("Programs/eden/eden.exe")).unwrap(); // a folder
        assert_eq!(EDEN.detect_exe_in(&dir.0), None);
        let fallback = dir.touch("eden/eden.exe");
        assert_eq!(EDEN.detect_exe_in(&dir.0), Some(fallback));
        fs::remove_dir(dir.0.join("Programs/eden/eden.exe")).unwrap();
        let installed = dir.touch("Programs/eden/eden.exe");
        assert_eq!(EDEN.detect_exe_in(&dir.0), Some(installed));
        assert_eq!(EDEN.detect_exe_in(&dir.0.join("missing")), None);
    }

    #[test]
    fn scans_roms_with_clean_names_and_slugs() {
        let dir = TempDir::new("scan");
        let mario = dir.touch("Super Mario Odyssey [0100000000010000][v0].NSP");
        dir.touch("Pokémon Legends (USA) [v65536].xci");
        dir.touch("homebrew.Nro");
        dir.touch("[0100ABCD00000000].nca");
        dir.touch("readme.txt");
        dir.touch("save.nsp.bak");
        dir.touch("noext");
        dir.touch("sub/Zelda [01007EF00011E000].nsp");
        fs::create_dir(dir.0.join("folder.nsp")).unwrap();

        let roms = scan_roms(&dir.0, &EDEN, ScanOptions::default());
        let want = [
            ("[0100ABCD00000000]", Some("0100abcd00000000")),
            ("homebrew", Some("homebrew")),
            ("Pokémon Legends", Some("pokemon-legends")),
            ("Super Mario Odyssey", Some("super-mario-odyssey")),
        ];
        assert_eq!(names(&roms), want);
        assert_eq!(roms[3].path, mario);

        let opts = ScanOptions {
            recurse_one_level: true,
            ..ScanOptions::default()
        };
        let roms = scan_roms(&dir.0, &EDEN, opts);
        assert_eq!(roms.len(), 5);
        assert_eq!(roms[4].name, "Zelda");
        assert!(scan_roms(&dir.0.join("missing"), &EDEN, opts).is_empty());
    }

    #[test]
    fn recursion_stops_after_one_level() {
        let dir = TempDir::new("depth");
        dir.touch("a/one.nsp");
        dir.touch("a/b/two.nsp");
        let opts = ScanOptions {
            recurse_one_level: true,
            ..ScanOptions::default()
        };
        assert_eq!(
            names(&scan_roms(&dir.0, &EDEN, opts)),
            [("one", Some("one"))]
        );
    }

    #[test]
    fn honours_the_limit() {
        let dir = TempDir::new("cap");
        for i in 0..10 {
            dir.touch(&format!("game {i}.nsp"));
            dir.touch(&format!("sub/more {i}.xci"));
        }
        let opts = |limit| ScanOptions {
            recurse_one_level: true,
            limit,
        };
        assert_eq!(scan_roms(&dir.0, &EDEN, opts(4)).len(), 4);
        assert_eq!(scan_roms(&dir.0, &EDEN, opts(15)).len(), 15);
        assert_eq!(scan_roms(&dir.0, &EDEN, opts(0)).len(), 0);
        assert_eq!(scan_roms(&dir.0, &EDEN, opts(MAX_ROMS)).len(), 20);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks_out_of_the_folder() {
        use std::os::unix::fs::symlink;
        let outside = TempDir::new("outside");
        let target = outside.touch("secret.nsp");
        outside.touch("dir/inner.nsp");
        let dir = TempDir::new("links");
        let real = dir.touch("real.nsp");
        symlink(&target, dir.0.join("escape.nsp")).unwrap();
        symlink(&real, dir.0.join("alias.nsp")).unwrap();
        symlink(outside.0.join("dir"), dir.0.join("linked")).unwrap();
        let opts = ScanOptions {
            recurse_one_level: true,
            ..ScanOptions::default()
        };
        let roms = scan_roms(&dir.0, &EDEN, opts);
        assert_eq!(
            names(&roms),
            [("alias", Some("alias")), ("real", Some("real"))]
        );
    }

    #[test]
    fn display_names_strip_tags() {
        assert_eq!(display_name("Game (USA) (En,Fr) [v0]"), "Game");
        assert_eq!(display_name("Game [Update [nested]] - DLC"), "Game - DLC");
        assert_eq!(display_name("  Odd]  Name ( "), "Odd] Name");
        assert_eq!(display_name("A\u{1b}[31mB"), "A");
        assert_eq!(display_name("(USA)"), "(USA)");
        assert_eq!(display_name(&"x".repeat(1000)).len(), 256);
    }

    #[test]
    fn slugs_are_valid_or_none() {
        fn slug(s: &str) -> Option<String> {
            suggest_slug(s).map(|s| s.as_str().to_owned())
        }
        assert_eq!(
            slug("Mario Kart 8 Deluxe"),
            Some("mario-kart-8-deluxe".into())
        );
        assert_eq!(slug("  --Ñandú: Ünïcode!! "), Some("nandu-unicode".into()));
        assert_eq!(slug("ゼルダの伝説"), None);
        assert_eq!(slug("!!!"), None);
        let long = slug(&format!("{} b", "a".repeat(63))).unwrap();
        assert_eq!(long, "a".repeat(63));
        assert!(Slug::new(&slug(&"ab ".repeat(40)).unwrap()).is_ok());
    }
}
