//! Emulator presets and ROM folder scanning.
//!
//! A preset knows an emulator's default launch args, its ROM file extensions
//! and where its exe usually lives. Detection and scanning are best-effort:
//! anything missing or unreadable is skipped quietly. File names are
//! untrusted: names are display-only and suggested slugs are validated.

use std::collections::HashMap;
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
    /// Exe locations relative to `%LOCALAPPDATA%` as path components (joined
    /// one by one so the result has native separators), most likely first.
    pub local_app_data_exes: &'static [&'static [&'static str]],
}

/// Eden, a Switch emulator: fullscreen (`-f`), then the game (`-g`).
pub const EDEN: EmulatorPreset = EmulatorPreset {
    name: "Eden",
    default_args: &["-f", "-g", "{rom}"],
    rom_extensions: &["nsp", "xci", "nca", "nro", "nso"],
    local_app_data_exes: &[&["Programs", "eden", "eden.exe"], &["eden", "eden.exe"]],
};

/// All presets, for pickers.
pub const PRESETS: &[EmulatorPreset] = &[EDEN];

/// Default cap on [`scan_roms`] results.
pub const MAX_ROMS: usize = 2000;
/// Directory entries looked at in one scan (all folders together), so huge
/// folders can't stall it.
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
            .map(|parts| {
                parts
                    .iter()
                    .fold(local_app_data.to_path_buf(), |p, part| p.join(part))
            })
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

/// The result of [`scan_roms`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RomScan {
    /// Sorted by name then path.
    pub roms: Vec<RomFile>,
    /// The scan stopped early (the ROM limit or the entry budget was hit), so
    /// `roms` is only part of the folder, in directory-listing order rather
    /// than the alphabetically first ones. UIs can say "showing first N".
    pub truncated: bool,
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

/// ROM files in `dir`, sorted by name then path. Dotfiles (e.g. macOS
/// `._Game.nsp`) are skipped, symlinks pointing outside `dir` are skipped,
/// symlinked folders are never entered, and a file reachable through several
/// names (a symlink to a ROM in the same folder) is listed once. Unreadable
/// entries are ignored; an unreadable `dir` gives an empty scan.
pub fn scan_roms(dir: &Path, preset: &EmulatorPreset, opts: ScanOptions) -> RomScan {
    scan_roms_with_budget(dir, preset, opts, MAX_ENTRIES)
}

fn scan_roms_with_budget(
    dir: &Path,
    preset: &EmulatorPreset,
    opts: ScanOptions,
    max_entries: usize,
) -> RomScan {
    let Ok(root) = fs::canonicalize(dir) else {
        tracing::debug!("cannot read ROM folder {}", dir.display());
        return RomScan::default();
    };
    // Paths stay under `dir` as given; `root` is only for the symlink check
    // (canonical Windows paths are `\\?\` paths, which some emulators reject).
    let mut scan = Scan {
        root,
        preset,
        limit: opts.limit,
        budget: max_entries,
        found: RomScan::default(),
        seen: HashMap::new(),
    };
    let mut subdirs = Vec::new();
    scan.dir(dir, Some(&mut subdirs));
    if opts.recurse_one_level {
        subdirs.sort();
        for sub in subdirs {
            if scan.found.truncated {
                break;
            }
            scan.dir(&sub, None);
        }
    }
    let mut found = scan.found;
    found
        .roms
        .sort_by_cached_key(|r| (r.name.to_lowercase(), r.path.clone()));
    found
}

struct Scan<'a> {
    root: PathBuf,
    preset: &'a EmulatorPreset,
    limit: usize,
    /// Directory entries left to look at, shared by every folder.
    budget: usize,
    found: RomScan,
    /// Canonical target -> (index in `found.roms`, listed via a symlink).
    seen: HashMap<PathBuf, (usize, bool)>,
}

impl Scan<'_> {
    fn dir(&mut self, dir: &Path, mut subdirs: Option<&mut Vec<PathBuf>>) {
        let Ok(entries) = fs::read_dir(dir) else {
            tracing::debug!("skipping unreadable folder {}", dir.display());
            return;
        };
        for entry in entries.flatten() {
            if self.budget == 0 {
                self.found.truncated = true;
                return;
            }
            self.budget -= 1;
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
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
            if !self.preset.is_rom(&path) || !(kind.is_file() || kind.is_symlink()) {
                continue;
            }
            // The canonical target, to list a file reached by several names once.
            let target = if kind.is_symlink() {
                match fs::canonicalize(&path) {
                    Ok(t) if t.starts_with(&self.root) && t.is_file() => t,
                    _ => continue,
                }
            } else {
                fs::canonicalize(&path).unwrap_or_else(|_| path.clone())
            };
            if !self.add(path, target, kind.is_symlink()) {
                return;
            }
        }
    }

    /// Records a ROM; `false` once the limit is hit and the scan should stop.
    fn add(&mut self, path: PathBuf, target: PathBuf, via_link: bool) -> bool {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let name = display_name(&stem);
        let rom = RomFile {
            slug: suggest_slug(&name),
            name,
            path,
        };
        if let Some(&(i, linked)) = self.seen.get(&target) {
            // Same file twice: prefer the real file over a symlink to it.
            if linked && !via_link {
                self.found.roms[i] = rom;
                self.seen.insert(target, (i, false));
            }
            return true;
        }
        if self.found.roms.len() >= self.limit {
            self.found.truncated = true;
            return false;
        }
        self.seen.insert(target, (self.found.roms.len(), via_link));
        self.found.roms.push(rom);
        true
    }
}

/// `stem` without `[...]`/`(...)` tags, control characters and extra spaces;
/// the cleaned stem itself if stripping would leave nothing.
pub fn display_name(stem: &str) -> String {
    // `open` holds the text since the outermost unclosed bracket; it is kept
    // if that bracket never closes.
    let (mut out, mut open, mut depth) = (String::new(), String::new(), 0usize);
    for c in stem.chars() {
        let c = if c.is_control() { ' ' } else { c };
        match c {
            '[' | '(' => {
                depth += 1;
                open.push(c);
            }
            ']' | ')' if depth > 0 => {
                depth -= 1;
                open.push(c);
                if depth == 0 {
                    open.clear();
                }
            }
            c if depth == 0 => out.push(c),
            c => open.push(c),
        }
    }
    out.push_str(&open);
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
        dir.touch("eden/eden.exe");
        let fallback = dir.0.join("eden").join("eden.exe");
        assert_eq!(EDEN.detect_exe_in(&dir.0), Some(fallback));
        fs::remove_dir(dir.0.join("Programs/eden/eden.exe")).unwrap();
        dir.touch("Programs/eden/eden.exe");
        // Joined per component, so the separators are native on Windows.
        let installed = dir.0.join("Programs").join("eden").join("eden.exe");
        let found = EDEN.detect_exe_in(&dir.0).unwrap();
        assert_eq!(found.as_os_str(), installed.as_os_str());
        if cfg!(windows) {
            assert!(!found.to_string_lossy().contains('/'), "{found:?}");
        }
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

        let roms = scan_roms(&dir.0, &EDEN, ScanOptions::default()).roms;
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
        let roms = scan_roms(&dir.0, &EDEN, opts).roms;
        assert_eq!(roms.len(), 5);
        assert_eq!(roms[4].name, "Zelda");
        assert_eq!(
            scan_roms(&dir.0.join("missing"), &EDEN, opts),
            RomScan::default()
        );
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
            names(&scan_roms(&dir.0, &EDEN, opts).roms),
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
        let scan = |limit| {
            let s = scan_roms(&dir.0, &EDEN, opts(limit));
            (s.roms.len(), s.truncated)
        };
        assert_eq!(scan(4), (4, true));
        assert_eq!(scan(15), (15, true));
        assert_eq!(scan(0), (0, true));
        assert_eq!(scan(20), (20, false));
        assert_eq!(scan(MAX_ROMS), (20, false));
    }

    #[test]
    fn one_entry_budget_covers_the_whole_scan() {
        let dir = TempDir::new("budget");
        for i in 0..3 {
            dir.touch(&format!("{i}/x.nsp"));
            dir.touch(&format!("{i}/y.nsp"));
        }
        let opts = ScanOptions {
            recurse_one_level: true,
            ..ScanOptions::default()
        };
        // 3 top-level folders + 6 files = 9 entries in total.
        let full = scan_roms_with_budget(&dir.0, &EDEN, opts, 9);
        assert_eq!((full.roms.len(), full.truncated), (6, false));
        // A per-folder cap of 3 would still find all 6.
        let cut = scan_roms_with_budget(&dir.0, &EDEN, opts, 6);
        assert_eq!((cut.roms.len(), cut.truncated), (3, true));
        let none = scan_roms_with_budget(&dir.0, &EDEN, opts, 0);
        assert_eq!((none.roms.len(), none.truncated), (0, true));
    }

    #[test]
    fn skips_dotfiles() {
        let dir = TempDir::new("dot");
        dir.touch("Game.nsp");
        dir.touch("._Game.nsp");
        dir.touch(".hidden.xci");
        dir.touch(".Trashes/Old.nsp");
        let opts = ScanOptions {
            recurse_one_level: true,
            ..ScanOptions::default()
        };
        let roms = scan_roms(&dir.0, &EDEN, opts).roms;
        assert_eq!(names(&roms), [("Game", Some("game"))]);
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
        let roms = scan_roms(&dir.0, &EDEN, opts).roms;
        // `alias` is the same file as `real`, so it is listed once, as `real`.
        assert_eq!(names(&roms), [("real", Some("real"))]);
        assert_eq!(roms[0].path, real);

        // A link to a ROM in a subfolder counts once too, as the real file.
        let other = TempDir::new("links-sub");
        let deep = other.touch("sub/deep.nsp");
        symlink(&deep, other.0.join("shortcut.nsp")).unwrap();
        let roms = scan_roms(&other.0, &EDEN, opts).roms;
        assert_eq!(names(&roms), [("deep", Some("deep"))]);
        assert_eq!(roms[0].path, deep);
    }

    #[test]
    fn display_names_strip_tags() {
        assert_eq!(display_name("Game (USA) (En,Fr) [v0]"), "Game");
        assert_eq!(display_name("Game [Update [nested]] - DLC"), "Game - DLC");
        assert_eq!(display_name("  Odd]  Name ( "), "Odd] Name (");
        // A bracket that never closes keeps the rest of the name.
        assert_eq!(display_name("A\u{1b}[31mB"), "A [31mB");
        assert_eq!(display_name("Game (Disc [USA] 2"), "Game (Disc [USA] 2");
        assert_eq!(display_name("Game [v0] (Unfinished"), "Game (Unfinished");
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
