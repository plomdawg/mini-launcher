//! Battle.net games: a catalog of Blizzard launch codes, detection of the
//! installed ones, and the code type a `battlenet = "..."` entry holds.
//!
//! Detection reads Battle.net's own install list (`product.db`, a protobuf
//! file), then Blizzard's uninstall entries (not every game has one: WoW
//! often doesn't), then probes the default install folders for each catalog
//! game's executable. World of Warcraft keeps every flavor in one folder, each
//! in its own subfolder (`_retail_`, `_classic_`, `_forever_`) whose
//! `.flavor.info` names the product (`wowf`); a flavor counts only when its
//! subfolder holds an executable.
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
    /// Battle.net product codes / uids (`product.db`, `--uid=` in uninstall
    /// entries, `.flavor.info`) that mean this game, with or without a
    /// locale suffix like `_enus`. Betas and test realms are other products.
    pub products: &'static [&'static str],
    /// The usual install folder name, probed under Program Files.
    pub folder: &'static str,
    /// For a WoW flavor: its usual subfolder of the WoW folder (found by its
    /// `.flavor.info` first). Any `.exe` in it counts, as the executable
    /// names change between patches.
    pub flavor: Option<&'static str>,
    /// Executables in the install folder that mean it is installed.
    pub exes: &'static [&'static str],
}

const WOW: &str = "World of Warcraft";

const fn game(
    code: &'static str,
    name: &'static str,
    products: &'static [&'static str],
    folder: &'static str,
    exes: &'static [&'static str],
) -> Known {
    Known {
        code,
        name,
        products,
        folder,
        flavor: None,
        exes,
    }
}

const fn wow(
    code: &'static str,
    name: &'static str,
    products: &'static [&'static str],
    flavor: &'static str,
) -> Known {
    Known {
        code,
        name,
        products,
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
    wow("WoW", "World of Warcraft", &["wow"], "_retail_"),
    wow(
        "WoWC",
        "World of Warcraft Classic",
        &["wow_classic"],
        "_classic_",
    ),
    wow("WoWF", "World of Warcraft: Forever", &["wowf"], "_forever_"),
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

/// An install Battle.net knows about: a product code or uid and its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    pub product: String,
    pub location: PathBuf,
    /// Where this came from (`product.db`, `registry`), for diagnostics.
    pub source: &'static str,
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

/// What detection looked at, and what it found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// From `product.db` and the uninstall entries.
    pub installs: Vec<Install>,
    /// Folders probed for executables and WoW flavors.
    pub dirs: Vec<PathBuf>,
    /// A problem reading `product.db`, if any.
    pub product_db_error: Option<String>,
    pub games: Vec<BattleNetGame>,
}

/// Battle.net's own list of installs; Blizzard writes it on every change.
const PRODUCT_DB: &str = r"Battle.net\Agent\product.db";
/// `product.db` is a few KiB; anything huge is not it.
const MAX_PRODUCT_DB: u64 = 16 << 20;

/// Installed Battle.net games, sorted by name. Empty when Battle.net or
/// its games are not installed (and on other OSes).
pub fn installed_games() -> Vec<BattleNetGame> {
    scan().games
}

/// Looks for installed games in `product.db`, the uninstall entries and
/// the default install folders.
pub fn scan() -> Scan {
    let mut installs = Vec::new();
    let mut product_db_error = None;
    if let Some(data) = std::env::var_os("ProgramData").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(data).join(PRODUCT_DB);
        match read_bounded(&path) {
            Ok(Some(bytes)) => installs.extend(parse_product_db(&bytes)),
            Ok(None) => {}
            Err(e) => product_db_error = Some(format!("{}: {e}", path.display())),
        }
    }
    for entry in crate::platform::blizzard_uninstalls() {
        if let Some(uid) = entry.uid {
            installs.push(Install {
                product: uid,
                location: entry.location,
                source: "registry",
            });
        }
    }
    let mut dirs = Vec::new();
    for base in crate::platform::program_files() {
        for k in CATALOG {
            let dir = base.join(k.folder);
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    let games = games_in(&installs, &dirs);
    Scan {
        installs,
        dirs,
        product_db_error,
        games,
    }
}

/// The file's bytes, or `None` if it does not exist.
fn read_bounded(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        file => file?,
    };
    let mut bytes = Vec::new();
    file.take(MAX_PRODUCT_DB).read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// The catalog games installed according to `installs` or found in `dirs`,
/// one per code, sorted by name.
///
/// A game counts when an install names its product and its folder exists,
/// or when one of its executables is in a folder from `dirs`. A WoW flavor
/// counts when a WoW folder (any install's folder, or one of `dirs`) has its
/// subfolder with an `.exe` in it: the subfolder whose `.flavor.info` names
/// the flavor's product, or else the usual subfolder name. WoW keeps every
/// flavor under one folder, and the subfolders of flavors that aren't
/// installed linger empty.
pub fn games_in(installs: &[Install], dirs: &[PathBuf]) -> Vec<BattleNetGame> {
    let mut roots: Vec<&Path> = Vec::new();
    for dir in installs
        .iter()
        .map(|i| i.location.as_path())
        .chain(dirs.iter().map(PathBuf::as_path))
    {
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    }
    let flavors: Vec<Flavor> = roots.iter().flat_map(|r| flavors_in(r)).collect();
    let mut games: Vec<BattleNetGame> = Vec::new();
    for k in CATALOG {
        let location = match k.flavor {
            Some(folder) => flavors
                .iter()
                .find(|f| f.product.as_deref().is_some_and(|p| is_product(k, p)))
                .or_else(|| {
                    flavors.iter().find(|f| {
                        f.product.is_none()
                            && f.dir
                                .file_name()
                                .is_some_and(|n| n.eq_ignore_ascii_case(folder))
                    })
                })
                .map(|f| f.dir.clone()),
            None => installs
                .iter()
                .find(|i| is_product(k, &i.product) && i.location.is_dir())
                .map(|i| i.location.clone())
                .or_else(|| {
                    let has = |d: &&PathBuf| k.exes.iter().any(|e| d.join(e).is_file());
                    dirs.iter().find(has).cloned()
                }),
        };
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

/// Locales Battle.net appends to uids (`wow_enus`).
const LOCALES: &[&str] = &[
    "enus", "engb", "dede", "eses", "esmx", "frfr", "itit", "kokr", "plpl", "ptbr", "ptpt", "ruru",
    "zhcn", "zhtw", "jajp", "thth", "trtr", "arsa",
];

/// `product` is one of `k`'s products, alone or with a locale suffix.
fn is_product(k: &Known, product: &str) -> bool {
    let product = product.to_ascii_lowercase();
    k.products
        .iter()
        .any(|known| match product.strip_prefix(known) {
            Some("") => true,
            Some(rest) => rest
                .strip_prefix('_')
                .is_some_and(|locale| LOCALES.contains(&locale)),
            None => false,
        })
}

/// A WoW flavor folder with an executable in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flavor {
    pub dir: PathBuf,
    /// From its `.flavor.info`, if it has one.
    pub product: Option<String>,
}

/// The flavor folders in WoW folder `root` (or `root` itself, when it is
/// one) that hold an `.exe`.
pub fn flavors_in(root: &Path) -> Vec<Flavor> {
    let is_flavor_dir = |d: &Path| {
        d.join(".flavor.info").is_file()
            || d.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.len() > 2 && n.starts_with('_') && n.ends_with('_'))
    };
    let mut dirs = Vec::new();
    if is_flavor_dir(root) {
        dirs.push(root.to_path_buf());
    } else if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten().take(256) {
            let path = e.path();
            if path.is_dir() && is_flavor_dir(&path) {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    dirs.into_iter()
        .filter(|d| has_exe(d))
        .map(|dir| Flavor {
            product: flavor_product(&dir),
            dir,
        })
        .collect()
}

/// The product in a `.flavor.info`: a `Product Flavor!STRING:0` header
/// line, then the value (`wowf`).
fn flavor_product(dir: &Path) -> Option<String> {
    let text = std::fs::read(dir.join(".flavor.info")).ok()?;
    let text = String::from_utf8_lossy(&text[..text.len().min(4096)]).into_owned();
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.contains('!'))
        .filter(|l| l.len() <= 64 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .map(str::to_owned)
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

/// The installs in `product.db` (protobuf: `Database.productInstall = 1`,
/// each with `uid = 1`, `productCode = 2` and `settings = 3` holding
/// `installPath = 1`). Both the uid and the product code are kept, as
/// either may name the game. Malformed data ends the parse early.
pub fn parse_product_db(bytes: &[u8]) -> Vec<Install> {
    let mut installs = Vec::new();
    for (field, install) in proto_fields(bytes) {
        if field != 1 {
            continue;
        }
        let (mut uid, mut code, mut path) = (None, None, None);
        for (field, value) in proto_fields(install) {
            match field {
                1 => uid = proto_str(value),
                2 => code = proto_str(value),
                3 => {
                    path = proto_fields(value)
                        .into_iter()
                        .find(|(f, _)| *f == 1)
                        .and_then(|(_, v)| proto_str(v))
                }
                _ => {}
            }
        }
        let Some(path) = path.filter(|p| !p.trim().is_empty()) else {
            continue;
        };
        let location = native_path(path.trim());
        for product in [code, uid].into_iter().flatten() {
            if installs
                .iter()
                .any(|i: &Install| i.product == product && i.location == location)
            {
                continue;
            }
            installs.push(Install {
                product,
                location: location.clone(),
                source: "product.db",
            });
        }
    }
    installs
}

/// `product.db` writes `D:/Games/...`; show Windows paths the Windows way.
fn native_path(path: &str) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(path.replace('/', "\\"))
    } else {
        PathBuf::from(path)
    }
}

fn proto_str(bytes: &[u8]) -> Option<String> {
    std::str::from_utf8(bytes).ok().map(str::to_owned)
}

/// The length-delimited fields of one protobuf message, in order; other
/// wire types are skipped. Stops at the first malformed field.
fn proto_fields(mut buf: &[u8]) -> Vec<(u64, &[u8])> {
    let mut out = Vec::new();
    while !buf.is_empty() {
        let Some(key) = varint(&mut buf) else { break };
        let (field, wire) = (key >> 3, key & 7);
        let skip = match wire {
            0 => varint(&mut buf).map(|_| 0),
            1 => Some(8),
            5 => Some(4),
            2 => match varint(&mut buf).and_then(|n| usize::try_from(n).ok()) {
                Some(n) if n <= buf.len() => {
                    out.push((field, &buf[..n]));
                    Some(n)
                }
                _ => None,
            },
            // Groups (3, 4) are not used by product.db.
            _ => None,
        };
        match skip {
            Some(n) if n <= buf.len() => buf = &buf[n..],
            _ => break,
        }
    }
    out
}

fn varint(buf: &mut &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for i in 0..10 {
        let (&b, rest) = buf.split_first()?;
        *buf = rest;
        value |= u64::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The `--uid=` value in a Blizzard uninstall command line, such as
/// `"...\Blizzard Uninstaller.exe" --lang=enUS --uid=wow_enus --displayname="..."`.
pub fn uninstall_uid(command: &str) -> Option<String> {
    let rest = command.split("--uid=").nth(1)?;
    let uid: String = rest
        .trim_start_matches('"')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
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
            assert!(!k.products.is_empty(), "{}", k.code);
        }
    }

    #[test]
    fn uninstall_strings_and_products() {
        let cmd = r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS --uid=fenris_beta --displayname="Diablo IV - Server Slam""#;
        assert!(is_blizzard_uninstaller(cmd));
        assert_eq!(uninstall_uid(cmd).as_deref(), Some("fenris_beta"));
        assert_eq!(
            uninstall_uid("x --uid=battle.net y").as_deref(),
            Some("battle.net")
        );
        assert_eq!(
            uninstall_uid("x --uid=\"prometheus\"").as_deref(),
            Some("prometheus")
        );
        assert_eq!(uninstall_uid("x --uid= y"), None);
        assert_eq!(uninstall_uid("x"), None);
        assert!(!is_blizzard_uninstaller(r#""C:\Steam\uninstall.exe""#));
        let k = |c| known(&code(c)).unwrap();
        assert!(is_product(k("Pro"), "prometheus"));
        assert!(is_product(k("WTCG"), "Hs_Beta_enUS"));
        assert!(is_product(k("WoW"), "wow_enus"));
        // Betas, test realms and other flavors are other products.
        assert!(!is_product(k("Fen"), "fenris_beta"));
        assert!(!is_product(k("WoW"), "wow_beta"));
        assert!(!is_product(k("WoW"), "wowf"));
        assert!(!is_product(k("WoW"), "wow_classic"));
        assert!(!is_product(k("WoWC"), "wow_classic_era"));
        assert!(!is_product(k("S2"), "s2x"));
    }

    /// Protobuf encoding helpers for a fake `product.db`.
    fn len_field(field: u64, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for mut n in [field << 3 | 2, body.len() as u64] {
            loop {
                let b = (n & 0x7f) as u8;
                n >>= 7;
                if n == 0 {
                    out.push(b);
                    break;
                }
                out.push(b | 0x80);
            }
        }
        out.extend_from_slice(body);
        out
    }

    fn product(uid: &str, code: &str, path: &str) -> Vec<u8> {
        let mut settings = len_field(1, path.as_bytes());
        settings.extend(len_field(2, b"US"));
        // A varint field and a fixed32 field, skipped.
        settings.extend([3 << 3, 1, 4 << 3 | 5, 0, 0, 0, 0]);
        let mut body = len_field(1, uid.as_bytes());
        body.extend(len_field(2, code.as_bytes()));
        body.extend(len_field(3, &settings));
        len_field(1, &body)
    }

    #[test]
    fn parses_product_db() {
        let mut db = product("battle.net", "bna", "C:/Program Files (x86)/Battle.net");
        db.extend(product(
            "wowf",
            "wowf",
            "D:/Games/Battle.net/World of Warcraft",
        ));
        db.extend(product(
            "fenris_beta",
            "fenrisb",
            "D:/Games/Battle.net/Diablo IV - Server Slam",
        ));
        db.extend(len_field(4, b"product config, ignored"));
        let got: Vec<_> = parse_product_db(&db)
            .into_iter()
            .map(|i| (i.product, i.location))
            .collect();
        let wow = native_path("D:/Games/Battle.net/World of Warcraft");
        let slam = native_path("D:/Games/Battle.net/Diablo IV - Server Slam");
        let bnet = native_path("C:/Program Files (x86)/Battle.net");
        assert_eq!(
            got,
            [
                ("bna".into(), bnet.clone()),
                ("battle.net".into(), bnet),
                ("wowf".into(), wow),
                ("fenrisb".into(), slam.clone()),
                ("fenris_beta".into(), slam),
            ]
        );
        // Truncated or garbage data never panics.
        for n in 0..db.len() {
            parse_product_db(&db[..n]);
        }
        assert_eq!(parse_product_db(&[0xff; 32]), []);
        assert_eq!(parse_product_db(b""), []);
    }

    fn touch(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn install(product: &str, location: &Path) -> Install {
        Install {
            product: product.into(),
            location: location.to_path_buf(),
            source: "test",
        }
    }

    #[test]
    fn detects_games() {
        let root = std::env::temp_dir().join(format!("ml-bnet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // The layout from a real PC: WoW only in product.db, Forever
        // installed, the other flavors' folders left empty.
        let wow = root.join("Battle.net/World of Warcraft");
        touch(&wow.join("_forever_/Wow.exe"), "");
        touch(
            &wow.join("_forever_/.flavor.info"),
            "Product Flavor!STRING:0\nwowf\n",
        );
        for empty in ["_retail_", "_classic_", "_classic_beta_", "Data"] {
            std::fs::create_dir_all(wow.join(empty)).unwrap();
        }
        let slam = root.join("Battle.net/Diablo IV - Server Slam");
        touch(&slam.join("Diablo IV.exe"), "");
        let ow = root.join("Games/OW");
        std::fs::create_dir_all(&ow).unwrap();
        let d3 = root.join("Diablo III");
        touch(&d3.join("Diablo III.exe"), "");

        let installs = [
            install("wowf", &wow),
            install("fenris_beta", &slam),
            // Found by product alone: Overwatch's folder has no known exe.
            install("prometheus", &ow),
            // Listed but gone: not installed.
            install("s2", &root.join("gone")),
        ];
        let dirs = [d3.clone(), root.join("missing")];
        let got: Vec<_> = games_in(&installs, &dirs)
            .into_iter()
            .map(|g| (g.code.to_string(), g.location))
            .collect();
        assert_eq!(
            got,
            [
                ("D3".into(), d3),
                ("Pro".into(), ow),
                ("WoWF".into(), wow.join("_forever_")),
            ]
        );

        // A flavor folder renamed: .flavor.info still finds it.
        let renamed = root.join("WoW2");
        touch(&renamed.join("_forever2_/WowF.exe"), "");
        touch(
            &renamed.join("_forever2_/.flavor.info"),
            "Product Flavor!STRING:0\nwowf\n",
        );
        let got = games_in(&[], std::slice::from_ref(&renamed));
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].code.as_str(), &got[0].location),
            ("WoWF", &renamed.join("_forever2_"))
        );
        // No .flavor.info: the usual folder name; a root that is the flavor.
        let plain = root.join("WoW3/_retail_");
        touch(&plain.join("Wow.exe"), "");
        let got = games_in(&[], std::slice::from_ref(&plain));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].code.as_str(), &got[0].location), ("WoW", &plain));
        assert_eq!(games_in(&[], &[]), []);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
