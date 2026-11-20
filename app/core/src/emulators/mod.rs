//! Emulator presets and ROM folder scanning.
//!
//! A preset knows an emulator's default launch args, its ROM file extensions
//! and where its exe usually lives. Detection and scanning are best-effort:
//! anything missing or unreadable is skipped quietly. File names are
//! untrusted: names are display-only and suggested slugs are validated.

use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Usual exe locations: a base folder and the path below it as
    /// components (joined one by one so the result has native separators),
    /// most likely first. The last component is the exe's file name.
    pub install_paths: &'static [(KnownDir, &'static [&'static str])],
    /// How to get updates and DLC loaded with a base game, for the note on
    /// the files a scan left out. `None`: the emulator has no such files, so
    /// a scan doesn't leave any out.
    pub updates_hint: Option<&'static str>,
}

/// A per-user or system folder an emulator is usually installed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KnownDir {
    /// `%LOCALAPPDATA%`
    LocalAppData,
    /// `%ProgramFiles%`
    ProgramFiles,
    /// `%USERPROFILE%` (e.g. Scoop's `scoop\apps`)
    UserProfile,
}

impl KnownDir {
    /// The folder from the environment; `None` when unset (e.g. on Linux).
    pub fn path(self) -> Option<PathBuf> {
        let var = match self {
            KnownDir::LocalAppData => "LOCALAPPDATA",
            KnownDir::ProgramFiles => "ProgramFiles",
            KnownDir::UserProfile => "USERPROFILE",
        };
        std::env::var_os(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
}

/// Eden, a Switch emulator: fullscreen (`-f`), then the game (`-g`).
pub const EDEN: EmulatorPreset = EmulatorPreset {
    name: "Eden",
    default_args: &["-f", "-g", "{rom}"],
    rom_extensions: &["nsp", "xci", "nca", "nro", "nso"],
    install_paths: &[
        (KnownDir::LocalAppData, &["Programs", "eden", "eden.exe"]),
        (KnownDir::LocalAppData, &["eden", "eden.exe"]),
    ],
    updates_hint: Some("Install updates and DLC in Eden once (File > Install Files to NAND) and they load with the base game."),
};

/// PCSX2, a PS2 emulator (2.x, the Qt build): no main window and quit when
/// the game closes (`-nogui`), fullscreen, then the game after `--` so a
/// file name starting with `-` isn't read as an option. CD games ripped as
/// `.bin` + `.cue` are listed by their `.cue`.
pub const PCSX2: EmulatorPreset = EmulatorPreset {
    name: "PCSX2",
    default_args: &["-nogui", "-fullscreen", "--", "{rom}"],
    rom_extensions: &["iso", "chd", "cso", "zso", "gz", "cue", "elf"],
    install_paths: &[
        (KnownDir::ProgramFiles, &["PCSX2", "pcsx2-qt.exe"]),
        (
            KnownDir::LocalAppData,
            &["Programs", "PCSX2", "pcsx2-qt.exe"],
        ),
        (
            KnownDir::UserProfile,
            &["scoop", "apps", "pcsx2", "current", "pcsx2-qt.exe"],
        ),
        (
            KnownDir::UserProfile,
            &["scoop", "apps", "pcsx2-dev", "current", "pcsx2-qt.exe"],
        ),
    ],
    updates_hint: None,
};

/// All presets, for pickers.
pub const PRESETS: &[EmulatorPreset] = &[EDEN, PCSX2];

/// Default cap on [`scan_roms`] results.
pub const MAX_ROMS: usize = 2000;
/// Default for [`ScanOptions::max_depth`].
pub const MAX_DEPTH: usize = 3;
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
    /// installed or on platforms without those folders.
    pub fn detect_exe(&self) -> Option<PathBuf> {
        self.detect_exe_in(KnownDir::path)
    }

    /// [`detect_exe`](Self::detect_exe) with the known folders from `dir`.
    pub fn detect_exe_in(&self, dir: impl Fn(KnownDir) -> Option<PathBuf>) -> Option<PathBuf> {
        self.install_paths
            .iter()
            .filter_map(|(base, parts)| {
                let base = dir(*base)?;
                Some(parts.iter().fold(base, |p, part| p.join(part)))
            })
            .find(|p| p.is_file())
    }

    /// The exe file names this preset installs as (any case on match).
    pub fn exe_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.install_paths
            .iter()
            .filter_map(|(_, parts)| parts.last().copied())
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
    /// `roms` is only part of the folder: the shallowest folders first, each
    /// in directory-listing order, rather than the alphabetically first
    /// ones. UIs can say there are more.
    pub truncated: bool,
    /// Update and DLC files left out of `roms` (see [`is_update_or_dlc`]).
    pub skipped_updates: usize,
}

/// Options for [`scan_roms`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanOptions {
    /// How many levels of subfolders to look into: 0 is `dir` only.
    pub max_depth: usize,
    /// Stop after this many ROMs.
    pub limit: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            max_depth: MAX_DEPTH,
            limit: MAX_ROMS,
        }
    }
}

/// ROM files in `dir` and its subfolders down to `opts.max_depth`, sorted by
/// name then path. Folders are visited breadth-first in sorted order, so a
/// truncated scan keeps the shallowest ones. For presets with an
/// [`updates_hint`](EmulatorPreset::updates_hint), update and DLC files are
/// counted in [`RomScan::skipped_updates`] instead of being listed. Dotfiles (e.g. macOS
/// `._Game.nsp`) are skipped, symlinks pointing outside `dir` are skipped,
/// symlinked folders are never entered, and a file reachable through several
/// names (a symlink to a ROM in the scanned folders) is listed once. Unreadable
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
        skipped: HashSet::new(),
    };
    // (folder, depth below `dir`, whether it is in an update/DLC folder)
    let mut queue = VecDeque::from([(dir.to_path_buf(), 0, false)]);
    while let Some((folder, depth, updates)) = queue.pop_front() {
        let mut subdirs = Vec::new();
        let want_subdirs = depth < opts.max_depth;
        scan.dir(&folder, updates, want_subdirs.then_some(&mut subdirs));
        if scan.found.truncated {
            break;
        }
        subdirs.sort();
        queue.extend(subdirs.into_iter().map(|sub| {
            let updates = updates || is_update_folder(&sub);
            (sub, depth + 1, updates)
        }));
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
    /// Canonical targets of the skipped update/DLC files.
    skipped: HashSet<PathBuf>,
}

impl Scan<'_> {
    /// Looks at the files in `dir` (all of them updates/DLC if `updates`),
    /// collecting its folders into `subdirs` if given.
    fn dir(&mut self, dir: &Path, updates: bool, mut subdirs: Option<&mut Vec<PathBuf>>) {
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
            let skips = self.preset.updates_hint.is_some();
            if skips && (updates || is_update_or_dlc(&path)) {
                // Counted once however many names reach it.
                if self.skipped.insert(target) {
                    self.found.skipped_updates += 1;
                }
                continue;
            }
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

/// Whether a game file is an update or DLC rather than a base game, from its
/// name: an `[UPD]`, `[Update]` or `[DLC]` tag (any case), or a bracketed
/// title ID (16 hex digits starting with `01`) that isn't a base game's
/// (those end in `000`; updates end in `800`, anything else is DLC). Files
/// anywhere under an update/DLC folder are caught by the scan instead.
pub fn is_update_or_dlc(path: &Path) -> bool {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    stem.split('[')
        .skip(1)
        .filter_map(|s| s.split_once(']'))
        .any(|(tag, _)| {
            let tag = tag.trim();
            ["upd", "update", "dlc"]
                .iter()
                .any(|t| tag.eq_ignore_ascii_case(t))
                || (tag.len() == 16
                    && tag.starts_with("01")
                    && tag.bytes().all(|b| b.is_ascii_hexdigit())
                    && !tag.ends_with("000"))
        })
}

/// Whether a folder's name marks the files under it as updates or DLC.
fn is_update_folder(path: &Path) -> bool {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    ["update", "updates", "dlc", "add-on"]
        .iter()
        .any(|n| name.eq_ignore_ascii_case(n))
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
    use crate::test_util::TempDir;

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
        assert_eq!(PRESETS, [EDEN, PCSX2]);
        assert!(EDEN.exe_names().all(|n| n == "eden.exe"));
    }

    #[test]
    fn pcsx2_preset() {
        assert_eq!(PCSX2.default_args, ["-nogui", "-fullscreen", "--", "{rom}"]);
        for f in [
            "a.iso", "a.CHD", "a.cso", "a.zso", "a.iso.gz", "a.cue", "a.elf",
        ] {
            assert!(PCSX2.is_rom(Path::new(f)), "{f}");
        }
        // A `.cue`'s tracks would list the game twice.
        assert!(!PCSX2.is_rom(Path::new("a (Track 1).bin")));
        assert!(!PCSX2.is_rom(Path::new("a.nsp")));
        assert!(PCSX2.exe_names().all(|n| n == "pcsx2-qt.exe"));
        // Names are unique, so a configured emulator maps to one preset.
        assert!(PRESETS
            .iter()
            .all(|p| PRESETS.iter().filter(|q| q.name == p.name).count() == 1));
    }

    /// Known folders under `root`: `local`, `pf` and `home`.
    fn under(root: &Path) -> impl Fn(KnownDir) -> Option<PathBuf> + '_ {
        move |d| {
            Some(root.join(match d {
                KnownDir::LocalAppData => "local",
                KnownDir::ProgramFiles => "pf",
                KnownDir::UserProfile => "home",
            }))
        }
    }

    #[test]
    fn detects_the_exe_in_usual_locations_only() {
        let dir = TempDir::new("detect");
        let local = dir.join("local");
        assert_eq!(EDEN.detect_exe_in(under(&dir)), None);
        fs::create_dir_all(local.join("Programs/eden/eden.exe")).unwrap(); // a folder
        assert_eq!(EDEN.detect_exe_in(under(&dir)), None);
        dir.touch("local/eden/eden.exe");
        let fallback = local.join("eden").join("eden.exe");
        assert_eq!(EDEN.detect_exe_in(under(&dir)), Some(fallback));
        fs::remove_dir(local.join("Programs/eden/eden.exe")).unwrap();
        dir.touch("local/Programs/eden/eden.exe");
        // Joined per component, so the separators are native on Windows.
        let installed = local.join("Programs").join("eden").join("eden.exe");
        let found = EDEN.detect_exe_in(under(&dir)).unwrap();
        assert_eq!(found.as_os_str(), installed.as_os_str());
        if cfg!(windows) {
            assert!(!found.to_string_lossy().contains('/'), "{found:?}");
        }
        assert_eq!(EDEN.detect_exe_in(|_| None), None);
        // Another preset's exe isn't this one's.
        dir.touch("pf/PCSX2/eden.exe");
        assert_eq!(PCSX2.detect_exe_in(under(&dir)), None);
    }

    #[test]
    fn detects_pcsx2_in_program_files_then_scoop() {
        let dir = TempDir::new("detect-pcsx2");
        dir.touch("home/scoop/apps/pcsx2/current/pcsx2-qt.exe");
        let scoop = dir.join("home/scoop/apps/pcsx2/current/pcsx2-qt.exe");
        let found = PCSX2.detect_exe_in(under(&dir)).unwrap();
        assert_eq!(found, scoop);
        dir.touch("pf/PCSX2/pcsx2-qt.exe");
        let found = PCSX2.detect_exe_in(under(&dir)).unwrap();
        assert_eq!(found, dir.join("pf/PCSX2/pcsx2-qt.exe"));
        // An unset folder is skipped, not a reason to give up.
        let no_pf = |d| {
            (d != KnownDir::ProgramFiles)
                .then(|| under(&dir)(d))
                .flatten()
        };
        assert_eq!(PCSX2.detect_exe_in(no_pf), Some(scoop));
    }

    #[test]
    fn pcsx2_scans_keep_update_named_files() {
        let dir = TempDir::new("scan-pcsx2");
        dir.touch("Game (USA) [SLUS-20946].iso");
        dir.touch("Update/Disc Two.chd");
        dir.touch("Racer [Update].cue");
        dir.touch("Racer [Update] (Track 1).bin");
        let scan = scan_roms(&dir, &PCSX2, ScanOptions::default());
        let roms: Vec<_> = scan.roms.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(roms, ["Disc Two", "Game", "Racer"]);
        assert_eq!(scan.skipped_updates, 0);
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
        fs::create_dir(dir.join("folder.nsp")).unwrap();

        let top = ScanOptions {
            max_depth: 0,
            ..ScanOptions::default()
        };
        let roms = scan_roms(&dir, &EDEN, top).roms;
        let want = [
            ("[0100ABCD00000000]", Some("0100abcd00000000")),
            ("homebrew", Some("homebrew")),
            ("Pokémon Legends", Some("pokemon-legends")),
            ("Super Mario Odyssey", Some("super-mario-odyssey")),
        ];
        assert_eq!(names(&roms), want);
        assert_eq!(roms[3].path, mario);

        let opts = ScanOptions::default();
        let roms = scan_roms(&dir, &EDEN, opts).roms;
        assert_eq!(roms.len(), 5);
        assert_eq!(roms[4].name, "Zelda");
        assert_eq!(
            scan_roms(&dir.join("missing"), &EDEN, opts),
            RomScan::default()
        );
    }

    #[test]
    fn recursion_stops_at_the_depth_limit() {
        let dir = TempDir::new("depth");
        dir.touch("a/one.nsp");
        dir.touch("a/b/two.nsp");
        dir.touch("a/b/c/three.nsp");
        dir.touch("a/b/c/d/four.nsp");
        let found = |max_depth| {
            let opts = ScanOptions {
                max_depth,
                ..ScanOptions::default()
            };
            let roms = scan_roms(&dir, &EDEN, opts).roms;
            roms.into_iter().map(|r| r.name).collect::<Vec<_>>()
        };
        assert_eq!(ScanOptions::default().max_depth, 3);
        // Four levels down is too deep for the default.
        assert_eq!(found(MAX_DEPTH), ["one", "three", "two"]);
        assert_eq!(found(1), ["one"]);
        assert!(found(0).is_empty());
        assert_eq!(found(4).len(), 4);
    }

    #[test]
    fn finds_nested_games_and_skips_their_updates() {
        let dir = TempDir::new("nested");
        let base = dir.touch("Super Mario Party/Title/Super Mario Party.nsp");
        dir.touch("Super Mario Party/Update/Super Mario Party v1.0.1 [UPD].nsp");
        let scan = scan_roms(&dir, &EDEN, ScanOptions::default());
        assert_eq!(
            names(&scan.roms),
            [("Super Mario Party", Some("super-mario-party"))]
        );
        assert_eq!(scan.roms[0].path, base);
        assert_eq!((scan.skipped_updates, scan.truncated), (1, false));
    }

    #[test]
    fn tells_updates_and_dlc_from_base_games() {
        let dir = TempDir::new("updates");
        for f in [
            "Kart [0100152000022000][v0].nsp",
            "Kart [0100152000022800][v65536].nsp",
            "Kart Track Pack [0100152000023001].nsp",
            "Kart Bonus [dlc].xci",
            "Kart [ Update ].nsp",
            "Kart v2 [upd].nsp",
            "Kart [0100ABC].nsp",
            "Kart Updates.nsp",
            "dlc/Extra.nsp",
            "Updates/Patch.nsp",
            "ADD-ON/Pack.nsp",
            "add-ons/Not Skipped.nsp",
            "Update Disc/Also Kept.nsp",
            "Update/v1.0.1/Deep Patch.nsp",
            "DLC/Pack 1/Deep Pack.nsp",
            "Kart Hash [DEADBEEFCAFEBABE].nsp",
        ] {
            dir.touch(f);
        }
        let scan = scan_roms(&dir, &EDEN, ScanOptions::default());
        let roms: Vec<_> = scan.roms.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            roms,
            [
                "Also Kept",
                "Kart",
                "Kart",
                "Kart Hash",
                "Kart Updates",
                "Not Skipped"
            ]
        );
        // Including everything below an update/DLC folder, at any depth.
        assert_eq!(scan.skipped_updates, 10);

        let upd = |f: &str| is_update_or_dlc(Path::new(f));
        assert!(upd("Game [0100000000010800].nsp"));
        assert!(upd("Game [0100000000010001].nsp"));
        assert!(upd("Game [0100abcd0001a800].nsp"));
        assert!(upd("Game [v0] [UPDATE].nsp"));
        assert!(!upd("Game [0100000000010000].nsp"));
        assert!(!upd("Game [010000000001080].nsp")); // 15 digits
        assert!(!upd("Game [010000000001080G].nsp"));
        // Real title IDs start with `01`; other hex is just a hash.
        assert!(!upd("Game [DEADBEEFCAFEBABE].nsp"));
        assert!(!upd("Game [0200000000010800].nsp"));
        assert!(!upd("Game (DLC Edition).nsp"));
        assert!(!upd("DLC/Game.nsp"), "folders are the scan's job");
    }

    #[test]
    fn an_update_folder_as_the_scanned_folder_still_lists() {
        let dir = TempDir::new("pick-update");
        dir.touch("Update/Patch.nsp");
        let scan = scan_roms(&dir.join("Update"), &EDEN, ScanOptions::default());
        assert_eq!((scan.roms.len(), scan.skipped_updates), (1, 0));
    }

    #[test]
    fn honours_the_limit() {
        let dir = TempDir::new("cap");
        for i in 0..10 {
            dir.touch(&format!("game {i}.nsp"));
            dir.touch(&format!("sub/more {i}.xci"));
            dir.touch(&format!("sub/a/b/deep {i}.xci"));
            dir.touch(&format!("sub/a/Update/patch {i}.xci"));
        }
        let opts = |limit| ScanOptions {
            limit,
            ..ScanOptions::default()
        };
        let scan = |limit| {
            let s = scan_roms(&dir, &EDEN, opts(limit));
            (s.roms.len(), s.truncated)
        };
        assert_eq!(scan(4), (4, true));
        assert_eq!(scan(15), (15, true));
        assert_eq!(scan(0), (0, true));
        assert_eq!(scan(30), (30, false));
        assert_eq!(scan(MAX_ROMS), (30, false));
        // Breadth-first: the shallow files come before the deep ones.
        let s = scan_roms(&dir, &EDEN, opts(20));
        assert!(s.truncated && s.roms.iter().all(|r| !r.name.starts_with("deep")));
        // Skipped updates don't count towards the limit.
        let s = scan_roms(&dir, &EDEN, opts(30));
        assert_eq!((s.truncated, s.skipped_updates), (false, 10));
    }

    #[test]
    fn one_entry_budget_covers_the_whole_scan() {
        let dir = TempDir::new("budget");
        for i in 0..3 {
            dir.touch(&format!("{i}/x.nsp"));
            dir.touch(&format!("{i}/y.nsp"));
        }
        dir.touch("0/a/b/z.nsp");
        let opts = ScanOptions::default();
        // 3 top-level folders, 1 folder in each of `0` and `0/a`, 7 files =
        // 12 entries in total.
        let full = scan_roms_with_budget(&dir, &EDEN, opts, 12);
        assert_eq!((full.roms.len(), full.truncated), (7, false));
        // One short: the deepest file is the one left out.
        let short = scan_roms_with_budget(&dir, &EDEN, opts, 11);
        assert_eq!((short.roms.len(), short.truncated), (6, true));
        assert!(short.roms.iter().all(|r| r.name != "z"));
        // The top level and `0` use 6; one more entry reaches into `1`. A
        // per-folder cap of 4 would still find all 7.
        let cut = scan_roms_with_budget(&dir, &EDEN, opts, 7);
        assert_eq!((cut.roms.len(), cut.truncated), (3, true));
        let none = scan_roms_with_budget(&dir, &EDEN, opts, 0);
        assert_eq!((none.roms.len(), none.truncated), (0, true));
    }

    #[test]
    fn skips_dotfiles() {
        let dir = TempDir::new("dot");
        dir.touch("Game.nsp");
        dir.touch("._Game.nsp");
        dir.touch(".hidden.xci");
        dir.touch(".Trashes/Old.nsp");
        let opts = ScanOptions::default();
        let roms = scan_roms(&dir, &EDEN, opts).roms;
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
        symlink(&target, dir.join("escape.nsp")).unwrap();
        symlink(&real, dir.join("alias.nsp")).unwrap();
        symlink(outside.join("dir"), dir.join("linked")).unwrap();
        let opts = ScanOptions::default();
        let roms = scan_roms(&dir, &EDEN, opts).roms;
        // `alias` is the same file as `real`, so it is listed once, as `real`.
        assert_eq!(names(&roms), [("real", Some("real"))]);
        assert_eq!(roms[0].path, real);

        // A link to a ROM in a subfolder counts once too, as the real file.
        let other = TempDir::new("links-sub");
        let deep = other.touch("sub/deep.nsp");
        symlink(&deep, other.join("shortcut.nsp")).unwrap();
        let roms = scan_roms(&other, &EDEN, opts).roms;
        assert_eq!(names(&roms), [("deep", Some("deep"))]);
        assert_eq!(roms[0].path, deep);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_update_is_skipped_once() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new("links-upd");
        let patch = dir.touch("Game [UPD].nsp");
        symlink(&patch, dir.join("Alias [UPD].nsp")).unwrap();
        dir.touch("Game.nsp");
        let scan = scan_roms(&dir, &EDEN, ScanOptions::default());
        assert_eq!((scan.roms.len(), scan.skipped_updates), (1, 1));
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
