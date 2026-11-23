//! Comment-preserving edits of `config.toml`, atomic saves, and
//! export/import (see `docs/config.md`).
//!
//! Every edit is validated with [`Config::parse`], the same code path as
//! loading, so a [`ConfigDocument`] always holds a loadable config. Keys
//! this build does not know (e.g. written by a newer version) are kept.

use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, Value};

use super::location::{ConfigLocation, LocationError};
use super::{
    format_uid, read_text_or_empty, BootCart, Config, ConfigError, Emulator, Game, Settings,
    Sounds, TAG_WRITTEN,
};
use crate::launch::LaunchPlan;
use crate::payload::Slug;

/// The `export_version` this build writes and reads.
pub const EXPORT_VERSION: i64 = 1;
const EXPORT_TABLE: &str = "mini_launcher";
/// Larger files are not read as an import.
pub const MAX_IMPORT_BYTES: u64 = 1024 * 1024;

/// Why an edit, save, export or import failed.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Location(#[from] LocationError),
    #[error("no game with id \"{0}\"")]
    NoSuchGame(Slug),
    #[error("no emulator with id \"{0}\"")]
    NoSuchEmulator(Slug),
    #[error("emulator \"{id}\" is still used by {}", quoted(games))]
    EmulatorInUse { id: Slug, games: Vec<Slug> },
    #[error("{0}")]
    Unsupported(String),
    #[error("not a Mini Launcher export: {0}")]
    BadExport(String),
    #[error("cannot write {}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// `config.toml` as an editable document that keeps comments and formatting.
#[derive(Debug, Clone)]
pub struct ConfigDocument {
    doc: DocumentMut,
    config: Config,
    /// Whether the source text used CRLF line endings, kept on output.
    crlf: bool,
}

impl ConfigDocument {
    /// Parses and validates TOML text.
    pub fn parse(text: &str) -> Result<Self, EditError> {
        let config = Config::parse(text)?;
        let doc = text
            .parse()
            .map_err(|e| EditError::Unsupported(format!("invalid config.toml: {e}")))?;
        Ok(ConfigDocument {
            doc,
            config,
            crlf: uses_crlf(text),
        })
    }

    /// Loads `path`; a missing file is an empty document.
    pub fn load_or_empty(path: &Path) -> Result<Self, EditError> {
        ConfigDocument::parse(&read_text_or_empty(path)?)
    }

    /// The validated library this document describes.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Adds `game`, or updates the entry with the same id in place (keeping
    /// its comments and key order).
    pub fn upsert_game(&mut self, game: &Game) -> Result<(), EditError> {
        self.edit(|doc| write_game(entry_mut(doc, GAMES, &game.id)?, game))
    }

    /// Removes the entry with this id (and the comments above it).
    pub fn remove_game(&mut self, id: &Slug) -> Result<(), EditError> {
        self.edit(|doc| {
            let games = section_mut(doc, GAMES)?;
            let i = find(games, id).ok_or_else(|| EditError::NoSuchGame(id.clone()))?;
            games.remove(i);
            Ok(())
        })
    }

    /// Adds `emulator`, or updates the `[[emulators]]` entry with the same
    /// id in place. Games using it pick up the new exe, args and cwd.
    pub fn upsert_emulator(&mut self, emulator: &Emulator) -> Result<(), EditError> {
        self.edit(|doc| {
            let above_games = (!doc.contains_key(EMULATORS))
                .then(|| games_after_emulators(doc))
                .flatten();
            let table = entry_mut(doc, EMULATORS, &emulator.id)?;
            if let Some((position, header)) = above_games {
                table.set_position(position);
                table.decor_mut().set_prefix(header);
            }
            write_emulator(table, emulator)
        })
    }

    /// Removes the `[[emulators]]` entry with this id (and the comments above
    /// it, so removing the first entry also drops a section header comment
    /// written above it); fails with [`EditError::EmulatorInUse`] while any
    /// game still uses it.
    pub fn remove_emulator(&mut self, id: &Slug) -> Result<(), EditError> {
        let games: Vec<Slug> = self
            .config
            .games
            .iter()
            .filter(|g| matches!(&g.plan, LaunchPlan::Emulator { emulator, .. } if emulator == id))
            .map(|g| g.id.clone())
            .collect();
        if !games.is_empty() {
            return Err(EditError::EmulatorInUse {
                id: id.clone(),
                games,
            });
        }
        self.edit(|doc| {
            let emulators = section_mut(doc, EMULATORS)?;
            let i = find(emulators, id).ok_or_else(|| EditError::NoSuchEmulator(id.clone()))?;
            emulators.remove(i);
            Ok(())
        })
    }

    /// Binds a tag UID to the entry with this id, or unbinds it (`None`).
    pub fn set_uid(&mut self, id: &Slug, uid: Option<&[u8]>) -> Result<(), EditError> {
        self.edit(|doc| {
            let games = section_mut(doc, GAMES)?;
            let i = find(games, id).ok_or_else(|| EditError::NoSuchGame(id.clone()))?;
            let table = games.get_mut(i).expect("index from find");
            set(table, "uid", uid.map(|u| format_uid(u).into()));
            Ok(())
        })
    }

    /// Sets `tag = "written"` on the entry with this id, or removes it. A
    /// `tag` value this build does not know (which loads as not written) is
    /// only replaced when setting `written`.
    pub fn set_tag_written(&mut self, id: &Slug, written: bool) -> Result<(), EditError> {
        self.edit(|doc| {
            let games = section_mut(doc, GAMES)?;
            let i = find(games, id).ok_or_else(|| EditError::NoSuchGame(id.clone()))?;
            let table = games.get_mut(i).expect("index from find");
            write_tag(table, written);
            Ok(())
        })
    }

    /// Writes `settings` to `[settings]` / `[settings.sounds]`. A key already
    /// in the file is updated in place (even to its default value); a missing
    /// key is added only when its value differs from the default, so an
    /// all-default config gets no `[settings]` table. Unknown keys are kept.
    pub fn set_settings(&mut self, settings: &Settings) -> Result<(), EditError> {
        self.edit(|doc| write_settings(doc, settings))
    }

    /// Validates and atomically writes the document to `path`, keeping the
    /// previous file as `<path>.bak`.
    pub fn save(&self, path: &Path) -> Result<(), EditError> {
        self.save_at(path, true)
    }

    /// [`ConfigDocument::save`], creating the folder only if `create_dir`.
    fn save_at(&self, path: &Path, create_dir: bool) -> Result<(), EditError> {
        let text = self.to_string();
        validate(&text)?;
        write_atomic(path, &text, true, create_dir)?;
        remember_saved(&text);
        Ok(())
    }

    /// Saves to the config file of `location`; fails (writing nothing) when
    /// the config folder is unavailable. Returns the path written.
    pub fn save_to(&self, location: &ConfigLocation) -> Result<PathBuf, EditError> {
        let path = location.writable_file()?;
        self.save_at(&path, location.may_create_dir())?;
        Ok(path)
    }

    /// Writes a portable export: this config with a `[mini_launcher]`
    /// header carrying `export_version`.
    pub fn export(&self, path: &Path) -> Result<(), EditError> {
        let mut doc = self.doc.clone();
        let mut header = Table::new();
        header.insert("export_version", toml_edit::value(EXPORT_VERSION));
        header.set_position(Some(isize::MIN));
        header.decor_mut().set_prefix(
            "# Mini Launcher config export. Import it from the app; to use it as\n\
             # config.toml directly, delete this [mini_launcher] table.\n",
        );
        doc.insert(EXPORT_TABLE, Item::Table(header));
        write_atomic(path, &render(&doc, self.crlf, &self.config), false, true)
    }

    /// Reads and fully validates an export (or a plain `config.toml`).
    pub fn read_export(path: &Path) -> Result<Self, EditError> {
        let bad = |m: String| EditError::BadExport(m);
        let mut text = String::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(MAX_IMPORT_BYTES + 1).read_to_string(&mut text))
            .map_err(|source| ConfigError::Io {
                path: path.to_owned(),
                source,
            })?;
        if text.len() as u64 > MAX_IMPORT_BYTES {
            return Err(bad(format!("larger than {} KiB", MAX_IMPORT_BYTES / 1024)));
        }
        let mut doc: DocumentMut = text.parse().map_err(|e| bad(format!("{e}")))?;
        if let Some(item) = doc.remove(EXPORT_TABLE) {
            let header = item
                .as_table()
                .ok_or_else(|| bad(format!("[{EXPORT_TABLE}] must be a table")))?;
            match header.get("export_version").map(Item::as_integer) {
                Some(Some(EXPORT_VERSION)) => {}
                Some(Some(n)) => {
                    return Err(bad(format!(
                        "export_version {n} is not supported (expected {EXPORT_VERSION})"
                    )))
                }
                Some(None) => return Err(bad("export_version must be an integer".into())),
                None => return Err(bad("missing export_version".into())),
            }
            if header.len() != 1 {
                return Err(bad(format!("unknown keys in [{EXPORT_TABLE}]")));
            }
        }
        let config = validate(&doc.to_string())?;
        ConfigDocument::parse(&render(&doc, uses_crlf(&text), &config))
    }

    /// Validates the export at `from`, then saves it as the config at `to`
    /// (the current config is kept as `<to>.bak`).
    pub fn import(from: &Path, to: &Path) -> Result<Self, EditError> {
        let doc = ConfigDocument::read_export(from)?;
        doc.save(to)?;
        Ok(doc)
    }

    /// [`ConfigDocument::import`] into the config file of `location`; fails
    /// when the config folder is unavailable.
    pub fn import_into(from: &Path, location: &ConfigLocation) -> Result<Self, EditError> {
        let to = location.writable_file()?;
        let doc = ConfigDocument::read_export(from)?;
        doc.save_at(&to, location.may_create_dir())?;
        Ok(doc)
    }

    /// Applies `change` to a copy and keeps it only if it still validates.
    fn edit(
        &mut self,
        change: impl FnOnce(&mut DocumentMut) -> Result<(), EditError>,
    ) -> Result<(), EditError> {
        let mut doc = self.doc.clone();
        change(&mut doc)?;
        self.config = validate(&doc.to_string())?;
        self.doc = doc;
        Ok(())
    }
}

impl std::fmt::Display for ConfigDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&render(&self.doc, self.crlf, &self.config))
    }
}

/// Whether `text` uses CRLF line endings. Only the first line ending is
/// looked at: a file whose first line ends in LF is written all LF.
fn uses_crlf(text: &str) -> bool {
    text.find('\n').is_some_and(|i| text[..i].ends_with('\r'))
}

/// `doc` as text, which must describe `config`. toml_edit drops `\r` from
/// comments, whitespace and the line breaks it writes (multi-line string
/// values keep theirs), so for a CRLF source each bare `\n` is turned back
/// into `\r\n`. If that would change a value (an LF inside a multi-line
/// string, or a new value containing a newline) or break the TOML, the
/// plain LF output is used instead: losing CRLF beats changing data.
fn render(doc: &DocumentMut, crlf: bool, config: &Config) -> String {
    let lf = doc.to_string();
    if !crlf {
        return lf;
    }
    let mut out = String::with_capacity(lf.len() + lf.len() / 16);
    let mut prev = '\0';
    for c in lf.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    if describes(&out, config) {
        out
    } else {
        lf
    }
}

/// Whether `text` parses to exactly `config` (ignoring an export header).
fn describes(text: &str, config: &Config) -> bool {
    let Ok(mut doc) = text.parse::<DocumentMut>() else {
        return false;
    };
    let parsed = match doc.remove(EXPORT_TABLE) {
        Some(_) => validate(&doc.to_string()),
        None => validate(text),
    };
    parsed.is_ok_and(|c| c == *config)
}

/// [`Config::parse`] without the unknown-key warnings, which were already
/// logged when the document was loaded.
fn validate(text: &str) -> Result<Config, ConfigError> {
    Config::parse_with_unknown(text).map(|(config, _)| config)
}

const GAMES: &str = "games";
const SETTINGS: &str = "settings";
const SOUNDS: &str = "sounds";
const TAG: &str = "tag";
const EMULATORS: &str = "emulators";

/// `"a", "b"` for error messages.
fn quoted(ids: &[Slug]) -> String {
    let ids: Vec<String> = ids.iter().map(|id| format!("\"{id}\"")).collect();
    ids.join(", ")
}

/// The `[[name]]` array, created if missing.
fn section_mut<'a>(
    doc: &'a mut DocumentMut,
    name: &str,
) -> Result<&'a mut ArrayOfTables, EditError> {
    doc.entry(name)
        .or_insert_with(|| Item::ArrayOfTables(ArrayOfTables::new()))
        .as_array_of_tables_mut()
        .ok_or_else(|| {
            EditError::Unsupported(format!(
                "`{name}` must be written as [[{name}]] blocks to be edited by the app"
            ))
        })
}

/// Moves `games` after a new `emulators` key, so a new first
/// `[[emulators]]` entry is written above the games. Returns the position
/// of the first `[[games]]` table and the file-header part of its comments
/// (up to the last blank line), both for the new entry to take.
fn games_after_emulators(doc: &mut DocumentMut) -> Option<(Option<isize>, String)> {
    let (key, mut games) = doc.as_table_mut().remove_entry(GAMES)?;
    let first = games.as_array_of_tables_mut().and_then(|g| g.get_mut(0));
    let mut above = (None, String::new());
    if let Some(first) = first {
        let prefix = first.decor().prefix().and_then(|p| p.as_str());
        let prefix = prefix.unwrap_or_default().to_owned();
        let (header, own) = prefix.split_at(after_last_blank_line(&prefix).unwrap_or(0));
        above = (first.position(), header.to_owned());
        first.decor_mut().set_prefix(format!("\n{own}"));
    }
    doc.insert(EMULATORS, Item::ArrayOfTables(ArrayOfTables::new()));
    doc.as_table_mut().insert_formatted(&key, games);
    Some(above)
}

/// The byte offset just past the last blank line in `text`, which may end
/// in `\n` or `\r\n` (toml_edit keeps CRLF in comment decor, and a file can
/// mix the two).
fn after_last_blank_line(text: &str) -> Option<usize> {
    let b = text.as_bytes();
    (1..b.len()).rev().find_map(|i| {
        if b[i] != b'\n' {
            return None;
        }
        let start = if b[i - 1] == b'\r' { i - 1 } else { i };
        (start > 0 && b[start - 1] == b'\n').then_some(i + 1)
    })
}

/// The `[[name]]` entry with this id, appended if there is none.
fn entry_mut<'a>(
    doc: &'a mut DocumentMut,
    name: &str,
    id: &Slug,
) -> Result<&'a mut Table, EditError> {
    let tables = section_mut(doc, name)?;
    let i = match find(tables, id) {
        Some(i) => i,
        None => {
            tables.push(Table::new());
            tables.len() - 1
        }
    };
    Ok(tables.get_mut(i).expect("index in range"))
}

fn path_value(p: &Path) -> Result<Value, EditError> {
    p.to_str()
        .map(Value::from)
        .ok_or_else(|| EditError::Unsupported(format!("path {} is not valid Unicode", p.display())))
}

fn write_emulator(table: &mut Table, emulator: &Emulator) -> Result<(), EditError> {
    set(table, "id", Some(emulator.id.as_str().into()));
    set(table, "name", Some(emulator.name.as_str().into()));
    set(table, "exe", Some(path_value(&emulator.exe)?));
    set(table, "args", Some(emulator.args.iter().collect()));
    let cwd = emulator.cwd.as_deref().map(path_value).transpose()?;
    set(table, "cwd", cwd);
    Ok(())
}

fn find(games: &ArrayOfTables, id: &Slug) -> Option<usize> {
    games
        .iter()
        .position(|t| t.get("id").and_then(Item::as_str) == Some(id.as_str()))
}

fn write_game(table: &mut Table, game: &Game) -> Result<(), EditError> {
    set(table, "id", Some(game.id.as_str().into()));
    set(table, "name", Some(game.name.as_str().into()));
    set(
        table,
        "uid",
        game.uid.as_deref().map(|u| format_uid(u).into()),
    );
    let (mut appid, mut battlenet, mut exe, mut args, mut cwd, mut emulator, mut rom) =
        (None, None, None, None, None, None, None);
    let mut shortcut = None;
    match &game.plan {
        LaunchPlan::Steam { appid: n } => appid = Some(i64::from(*n).into()),
        LaunchPlan::SteamShortcut { appid: n } => shortcut = Some(i64::from(*n).into()),
        LaunchPlan::BattleNet { code } => battlenet = Some(code.as_str().into()),
        LaunchPlan::Exe {
            path,
            args: a,
            cwd: c,
        } => {
            exe = Some(path_value(path)?);
            args = (!a.is_empty()).then(|| Value::Array(a.iter().collect()));
            cwd = c.as_deref().map(path_value).transpose()?;
        }
        // Exe, args and cwd come from the [[emulators]] entry.
        LaunchPlan::Emulator {
            emulator: id,
            rom: r,
            ..
        } => {
            emulator = Some(id.as_str().into());
            rom = Some(path_value(r)?);
        }
    }
    set(table, "steam_appid", appid);
    set(table, "steam_shortcut", shortcut);
    set(table, "battlenet", battlenet);
    set(table, "exe", exe);
    set(table, "args", args);
    set(table, "cwd", cwd);
    set(table, "emulator", emulator);
    set(table, "rom", rom);
    write_tag(table, game.tag_written);
    Ok(())
}

/// Sets `tag = "written"`, or removes it; an unknown `tag` value already
/// means "not written" to this build, so that case keeps it.
fn write_tag(table: &mut Table, written: bool) {
    let current = table.get(TAG).and_then(Item::as_str);
    if written || current == Some(TAG_WRITTEN) {
        set(table, TAG, written.then(|| TAG_WRITTEN.into()));
    }
}

/// A table key of `parent` (created implicit if missing), or an error if it
/// is something else, e.g. an inline table, which toml_edit keeps separate.
fn table_mut<'a>(
    parent: &'a mut Table,
    name: &str,
    path: &str,
) -> Result<&'a mut Table, EditError> {
    let item = parent.entry(name).or_insert_with(|| {
        let mut table = Table::new();
        table.set_implicit(true);
        Item::Table(table)
    });
    item.as_table_mut().ok_or_else(|| {
        EditError::Unsupported(format!(
            "`{name}` must be written as a [{path}] table to be edited by the app"
        ))
    })
}

fn write_settings(doc: &mut DocumentMut, settings: &Settings) -> Result<(), EditError> {
    let (d, ds) = (Settings::default(), Sounds::default());
    // toml_edit places a table without a position after the table before
    // it in key order, which moves when `[[emulators]]` is first added; pin
    // a new `[settings]` below everything instead.
    let last = max_position(doc.as_table());
    let root = table_mut(doc.as_table_mut(), SETTINGS, SETTINGS)?;
    if root.position().is_none() {
        root.set_position(Some(last.map_or(0, |p| p.saturating_add(1))));
    }
    let boot = settings.boot_cart;
    // An unknown value loads as the default; keep it unless that changed.
    let unknown_boot = root
        .get("boot_cart")
        .and_then(Item::as_str)
        .is_some_and(|v| BootCart::from_value(v).is_none());
    if !(unknown_boot && boot == d.boot_cart) {
        put(root, "boot_cart", boot.as_str().into(), boot == d.boot_cart);
    }
    let scale = settings.font_scale;
    put(
        root,
        "font_scale",
        // Via the shortest decimal for the f32, so 1.1 is written as 1.1.
        scale.to_string().parse::<f64>().unwrap_or(1.0).into(),
        scale == d.font_scale,
    );
    let sounds_table = table_mut(root, SOUNDS, "settings.sounds")?;
    let s = settings.sounds;
    put(
        sounds_table,
        "enabled",
        s.enabled.into(),
        s.enabled == ds.enabled,
    );
    put(
        sounds_table,
        "volume",
        i64::from(s.volume).into(),
        s.volume == ds.volume,
    );
    for (key, value, default) in [
        ("waiting", s.waiting, ds.waiting),
        ("writing", s.writing, ds.writing),
        ("success", s.success, ds.success),
        ("failure", s.failure, ds.failure),
        ("launch", s.launch, ds.launch),
    ] {
        put(sounds_table, key, value.into(), value == default);
    }
    if sounds_table.is_empty() && sounds_table.is_implicit() {
        root.remove(SOUNDS);
    }
    // A new `[settings]` stays implicit (only `[settings.sounds]` shows)
    // until it has keys of its own; a new one with nothing in it goes away.
    let own_keys = root.iter().any(|(_, item)| !item.is_table());
    if own_keys {
        root.set_implicit(false);
    }
    if root.is_empty() && root.is_implicit() {
        doc.remove(SETTINGS);
    }
    Ok(())
}

/// The largest position of any table in `table`, nested ones included.
fn max_position(table: &Table) -> Option<isize> {
    let nested = table.iter().filter_map(|(_, item)| match item {
        Item::Table(t) => t.position().max(max_position(t)),
        Item::ArrayOfTables(a) => a
            .iter()
            .filter_map(|t| t.position().max(max_position(t)))
            .max(),
        _ => None,
    });
    nested.max()
}

/// Updates `key` if present; adds it only if `value` is not the default.
fn put(table: &mut Table, key: &str, value: Value, is_default: bool) {
    if table.contains_key(key) || !is_default {
        set(table, key, Some(value));
    }
}

/// Sets or removes `key`, keeping an existing value's comments and spacing.
fn set(table: &mut Table, key: &str, value: Option<Value>) {
    let Some(mut value) = value else {
        table.remove(key);
        return;
    };
    if let Some(old) = table.get(key).and_then(Item::as_value) {
        if same_value(old, &value) {
            return;
        }
        *value.decor_mut() = old.decor().clone();
    }
    table.insert(key, Item::Value(value));
}

/// Whether `a` and `b` hold the same data, ignoring quoting, layout and
/// comments, so a no-op edit leaves the user's formatting alone.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(a), Value::String(b)) => a.value() == b.value(),
        (Value::Integer(a), Value::Integer(b)) => a.value() == b.value(),
        (Value::Boolean(a), Value::Boolean(b)) => a.value() == b.value(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(a, b)| same_value(a, b))
        }
        _ => a.to_string().trim() == b.to_string().trim(),
    }
}

/// Makes temp file names unique within this process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `text` to a temp file next to `path`, flushes it to disk, then
/// renames it over `path` (`rename` replaces an existing file on Windows and
/// Unix alike). With `backup`, an existing `path` is first copied to
/// `<path>.bak`. The folder is created only with `create_dir`. On Unix a symlinked `path` is written through (the link
/// stays) and an existing file's permissions are kept.
pub fn write_atomic(
    path: &Path,
    text: &str,
    backup: bool,
    create_dir: bool,
) -> Result<(), EditError> {
    let err = |source| EditError::Write {
        path: path.to_owned(),
        source,
    };
    #[cfg(unix)]
    let resolved = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).map_err(err)?,
        _ => path.to_owned(),
    };
    #[cfg(unix)]
    let path = resolved.as_path();
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .ok_or_else(|| err(std::io::ErrorKind::InvalidInput.into()))?;
    if create_dir {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    tmp_name.push(format!(".{}.{n}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        #[cfg(unix)]
        if let Ok(m) = std::fs::metadata(path) {
            file.set_permissions(m.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        if backup && path.exists() {
            let mut bak = path.as_os_str().to_owned();
            bak.push(".bak");
            std::fs::copy(path, PathBuf::from(bak))?;
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(err)?;
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Hashes of the last few configs saved by this process, so a file watcher
/// can tell its own app's saves from outside edits.
static SAVED: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn hash(text: &str) -> u64 {
    let mut h = std::hash::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn remember_saved(text: &str) {
    let mut saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    if saved.len() >= 8 {
        saved.remove(0);
    }
    saved.push(hash(text));
}

/// Whether this process recently saved exactly `text` (e.g. from the
/// settings window), so a watcher reloading it need not announce it.
pub fn saved_by_this_process(text: &str) -> bool {
    let saved = SAVED.lock().unwrap_or_else(|e| e.into_inner());
    saved.contains(&hash(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    const COMMENTED: &str = r#"# My library
# (hand-written)

# Steam
[[games]]
id = "hl2"   # short id
name = "Half-Life 2"
steam_appid = 220 # the appid

# Non-Steam
[[games]]
id = "my-game"
name = "My Game"
exe = 'C:\Games\MyGame\game.exe'
args = ["--fullscreen"]
"#;

    fn slug(s: &str) -> Slug {
        Slug::new(s).unwrap()
    }

    #[test]
    fn unchanged_round_trip_is_byte_identical() {
        let doc = ConfigDocument::parse(COMMENTED).unwrap();
        assert_eq!(doc.to_string(), COMMENTED);
        assert_eq!(doc.config(), &Config::parse(COMMENTED).unwrap());
    }

    #[test]
    fn edits_keep_comments() {
        let mut doc = ConfigDocument::parse(COMMENTED).unwrap();
        let mut hl2 = doc.config().games[0].clone();
        hl2.name = "Half-Life 2: Episode One".into();
        hl2.plan = LaunchPlan::Steam { appid: 380 };
        doc.upsert_game(&hl2).unwrap();
        doc.set_uid(&slug("my-game"), Some(&[1, 2, 3, 4])).unwrap();
        doc.upsert_game(&Game {
            id: slug("new"),
            name: "New".into(),
            uid: None,
            tag_written: false,
            plan: LaunchPlan::Exe {
                path: r"D:\New Game\new.exe".into(),
                args: vec![],
                cwd: Some(r"D:\New Game".into()),
            },
        })
        .unwrap();
        let text = doc.to_string();
        for kept in [
            "# My library\n# (hand-written)\n",
            "# short id",
            "# the appid",
        ] {
            assert!(text.contains(kept), "{kept:?} lost:\n{text}");
        }
        assert!(text.contains("steam_appid = 380 # the appid"), "{text}");
        assert!(text.contains("uid = \"01:02:03:04\""), "{text}");
        let cfg = Config::parse(&text).unwrap();
        assert_eq!(cfg.games.len(), 3);
        assert_eq!(&cfg, doc.config());
        assert_eq!(cfg.by_uid(&[1, 2, 3, 4]).unwrap().id, slug("my-game"));

        // Switching an entry from exe to Steam drops exe-only keys.
        let mut game = cfg.games[1].clone();
        game.plan = LaunchPlan::Steam { appid: 70 };
        doc.upsert_game(&game).unwrap();
        doc.set_uid(&slug("my-game"), None).unwrap();
        doc.remove_game(&slug("hl2")).unwrap();
        let text = doc.to_string();
        assert!(
            !text.contains("# Steam\n") && !text.contains("args"),
            "{text}"
        );
        assert!(text.contains("# Non-Steam\n[[games]]"), "{text}");
        assert_eq!(doc.config().games.len(), 2);
    }

    const FUTURE: &str = r#"future = "top" # from a newer version

[settings]
theme = "dark"

[settings.sounds]
chime = "bell"

[future_table]
x = 1

[[emulators]]
id = "eden"
name = "Eden"
exe = "eden"
args = ["{rom}"]
portable = true

[[games]]
id = "hl2"
name = "Half-Life 2"
steam_appid = 220
cover = { path = "hl2.png" } # keep me
[games.extra]
y = 2
"#;

    #[test]
    fn edits_keep_unknown_keys_and_tables() {
        let mut doc = ConfigDocument::parse(FUTURE).unwrap();
        assert_eq!(doc.to_string(), FUTURE);
        let mut hl2 = doc.config().games[0].clone();
        hl2.name = "HL2".into();
        hl2.plan = LaunchPlan::Exe {
            path: "hl2.exe".into(),
            args: vec!["-novid".into()],
            cwd: None,
        };
        doc.upsert_game(&hl2).unwrap();
        doc.set_uid(&slug("hl2"), Some(&[1, 2, 3, 4])).unwrap();
        doc.set_tag_written(&slug("hl2"), true).unwrap();
        let mut eden = doc.config().emulators[0].clone();
        eden.args = vec!["-f".into(), "{rom}".into()];
        doc.upsert_emulator(&eden).unwrap();
        let settings = Settings {
            boot_cart: BootCart::Launch,
            font_scale: 1.25,
            sounds: Sounds {
                volume: 30,
                ..Sounds::default()
            },
        };
        doc.set_settings(&settings).unwrap();
        let text = doc.to_string();
        for kept in [
            "future = \"top\" # from a newer version\n",
            "theme = \"dark\"",
            "chime = \"bell\"",
            "[future_table]\nx = 1\n",
            "portable = true",
            "cover = { path = \"hl2.png\" } # keep me",
            "[games.extra]\ny = 2\n",
        ] {
            assert!(text.contains(kept), "{kept:?} lost:\n{text}");
        }
        let (cfg, unknown) = Config::parse_with_unknown(&text).unwrap();
        assert_eq!(unknown.len(), 7, "{unknown:?}");
        assert_eq!(&cfg, doc.config());
        assert_eq!(cfg.settings, settings);
        assert_eq!(cfg.games[0].name, "HL2");
        assert!(cfg.games[0].tag_written);
    }

    #[test]
    fn settings_write_only_what_differs_unless_already_present() {
        let mut doc = ConfigDocument::parse(COMMENTED).unwrap();
        doc.set_settings(&Settings::default()).unwrap();
        assert_eq!(doc.to_string(), COMMENTED);

        let mut settings = Settings::default();
        settings.sounds.waiting = false;
        doc.set_settings(&settings).unwrap();
        let text = doc.to_string();
        assert!(text.starts_with("# My library\n"), "{text}");
        assert!(
            text.ends_with("\n[settings.sounds]\nwaiting = false\n"),
            "{text}"
        );
        assert!(!text.contains("[settings]\n"), "{text}");
        assert_eq!(doc.config().settings, settings);

        settings.font_scale = 1.1;
        settings.boot_cart = BootCart::Launch;
        doc.set_settings(&settings).unwrap();
        let text = doc.to_string();
        assert!(
            text.contains("[settings]\nboot_cart = \"launch\"\nfont_scale = 1.1\n"),
            "{text}"
        );
        assert_eq!(Config::parse(&text).unwrap().settings, settings);

        // Back to defaults: keys already in the file stay, set to the default.
        doc.set_settings(&Settings::default()).unwrap();
        let text = doc.to_string();
        assert!(text.contains("boot_cart = \"wait\""), "{text}");
        assert!(text.contains("font_scale = 1.0"), "{text}");
        assert!(text.contains("waiting = true"), "{text}");
        assert!(!text.contains("volume"), "{text}");
        assert_eq!(doc.config().settings, Settings::default());

        // Comments on existing settings keys are kept.
        let commented = "[settings]\nfont_scale = 1.0 # bigger later\n";
        let mut doc = ConfigDocument::parse(commented).unwrap();
        doc.set_settings(&Settings {
            font_scale: 2.0,
            ..Settings::default()
        })
        .unwrap();
        assert_eq!(
            doc.to_string(),
            "[settings]\nfont_scale = 2.0 # bigger later\n"
        );
    }

    #[test]
    fn new_settings_stay_below_the_library() {
        let mut doc = ConfigDocument::parse(
            "# header\n\n[[games]]\nid = \"a\"\nname = \"A\"\nsteam_appid = 1\n",
        )
        .unwrap();
        doc.set_settings(&Settings {
            font_scale: 1.5,
            ..Settings::default()
        })
        .unwrap();
        // Adding the first emulator reorders the sections; settings stay last.
        doc.upsert_emulator(&emulator("eden", &["{rom}"])).unwrap();
        let text = doc.to_string();
        assert!(text.starts_with("# header\n\n[[emulators]]"), "{text}");
        assert!(text.ends_with("\n[settings]\nfont_scale = 1.5\n"), "{text}");
    }

    #[test]
    fn invalid_settings_are_rejected() {
        let mut doc = ConfigDocument::parse(COMMENTED).unwrap();
        for bad in [
            Settings {
                font_scale: 3.0,
                ..Settings::default()
            },
            Settings {
                sounds: Sounds {
                    volume: 101,
                    ..Sounds::default()
                },
                ..Settings::default()
            },
        ] {
            assert!(doc.set_settings(&bad).is_err());
            assert_eq!(doc.to_string(), COMMENTED);
        }
        let mut doc = ConfigDocument::parse("settings = { font_scale = 1.5 }\n").unwrap();
        let err = doc.set_settings(&Settings::default()).unwrap_err();
        assert!(err.to_string().contains("[settings] table"), "{err}");
        let mut doc = ConfigDocument::parse("[settings]\nsounds = { volume = 5 }\n").unwrap();
        let err = doc.set_settings(&Settings::default()).unwrap_err();
        assert!(
            err.to_string()
                .contains("`sounds` must be written as a [settings.sounds] table"),
            "{err}"
        );
    }

    #[test]
    fn tag_marker_round_trips_through_edits() {
        let mut doc = ConfigDocument::parse(COMMENTED).unwrap();
        doc.set_tag_written(&slug("my-game"), true).unwrap();
        assert!(doc
            .to_string()
            .contains("args = [\"--fullscreen\"]\ntag = \"written\"\n"));
        // Re-saving the entry (as the edit form does) keeps the marker.
        let mut game = doc.config().games[1].clone();
        assert!(game.tag_written);
        game.name = "Renamed".into();
        doc.upsert_game(&game).unwrap();
        assert!(doc.to_string().contains("tag = \"written\""));
        assert!(doc.config().games[1].tag_written);
        doc.set_tag_written(&slug("my-game"), false).unwrap();
        assert!(!doc.to_string().contains("tag"));
        assert!(matches!(
            doc.set_tag_written(&slug("nope"), true),
            Err(EditError::NoSuchGame(_))
        ));
    }

    #[test]
    fn unknown_enum_values_survive_edits_that_do_not_change_them() {
        let text = "[settings]\nboot_cart = \"ask\"\n\n\
                    [[games]]\nid = \"a\"\nname = \"A\"\nsteam_appid = 1\ntag = \"verified\"\n";
        let mut doc = ConfigDocument::parse(text).unwrap();
        let mut a = doc.config().games[0].clone();
        assert!(!a.tag_written);
        a.name = "A2".into();
        doc.upsert_game(&a).unwrap();
        doc.set_tag_written(&slug("a"), false).unwrap();
        let mut settings = doc.config().settings;
        settings.font_scale = 1.5;
        doc.set_settings(&settings).unwrap();
        let text = doc.to_string();
        assert!(text.contains("tag = \"verified\""), "{text}");
        assert!(text.contains("boot_cart = \"ask\""), "{text}");

        // Actually changing them replaces the unknown value.
        doc.set_tag_written(&slug("a"), true).unwrap();
        settings.boot_cart = BootCart::Launch;
        doc.set_settings(&settings).unwrap();
        let text = doc.to_string();
        assert!(text.contains("tag = \"written\""), "{text}");
        assert!(text.contains("boot_cart = \"launch\""), "{text}");
        doc.set_tag_written(&slug("a"), false).unwrap();
        assert!(!doc.to_string().contains("tag ="));
    }

    #[test]
    fn steam_shortcut_games_keep_the_tag_marker() {
        let mut doc = ConfigDocument::parse(
            "[[games]]\nid = \"doom\"\nname = \"Doom\"\nsteam_shortcut = 3237998081\n",
        )
        .unwrap();
        doc.set_tag_written(&slug("doom"), true).unwrap();
        let mut game = doc.config().games[0].clone();
        assert!(game.tag_written);
        game.name = "DOOM".into();
        doc.upsert_game(&game).unwrap();
        let text = doc.to_string();
        assert!(
            text.contains("steam_shortcut = 3237998081\ntag = \"written\"\n"),
            "{text}"
        );
        assert_eq!(Config::parse(&text).unwrap().games[0], game);
    }

    #[test]
    fn invalid_edits_are_rejected_and_leave_the_document_unchanged() {
        let mut doc = ConfigDocument::parse(COMMENTED).unwrap();
        doc.set_uid(&slug("hl2"), Some(&[9, 9, 9, 9])).unwrap();
        let before = doc.to_string();
        let err = doc
            .set_uid(&slug("my-game"), Some(&[9, 9, 9, 9]))
            .unwrap_err();
        assert!(err.to_string().contains("duplicate uid"), "{err}");
        assert!(matches!(
            doc.remove_game(&slug("nope")),
            Err(EditError::NoSuchGame(_))
        ));
        assert_eq!(doc.to_string(), before);
        let mut inline = ConfigDocument::parse("games = []").unwrap();
        let game = doc.config().games[0].clone();
        assert!(matches!(
            inline.upsert_game(&game),
            Err(EditError::Unsupported(_))
        ));
    }

    const EMULATED: &str = r#"# Emulators
[[emulators]] # mine
id = "eden"
name = "Eden" # Switch
exe = 'C:\Emulators\Eden\eden.exe'
args = ["-f", "-g", "{rom}"]

# Games
[[games]]
id = "mk8"
name = "MK8"
emulator = "eden" # the emulator
rom = 'D:\Switch\mk8.nsp'

[[games]]
id = "botw"
name = "BotW"
emulator = "eden"
rom = 'D:\Switch\botw.nsp'
"#;

    fn emulator(id: &str, args: &[&str]) -> Emulator {
        Emulator {
            id: slug(id),
            name: id.to_uppercase(),
            exe: format!(r"C:\Emulators\{id}.exe").into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: None,
        }
    }

    fn emulator_game(id: &str, emulator: &str, rom: &str) -> Game {
        Game {
            id: slug(id),
            name: id.into(),
            uid: None,
            tag_written: false,
            plan: LaunchPlan::Emulator {
                emulator: slug(emulator),
                rom: rom.into(),
                // Derived from the emulator entry; never written.
                path: "ignored".into(),
                args: vec![],
                cwd: None,
            },
        }
    }

    #[test]
    fn emulator_edits_keep_comments() {
        let mut doc = ConfigDocument::parse(EMULATED).unwrap();
        doc.set_uid(&slug("mk8"), Some(&[1, 2, 3, 4])).unwrap();
        doc.set_uid(&slug("mk8"), None).unwrap();
        assert_eq!(doc.to_string(), EMULATED);
        // Saving an unchanged emulator game or emulator is a no-op.
        doc.upsert_game(&doc.config().games[0].clone()).unwrap();
        doc.upsert_emulator(&doc.config().emulators[0].clone())
            .unwrap();
        assert_eq!(doc.to_string(), EMULATED);

        let mut eden = doc.config().emulators[0].clone();
        eden.args = vec!["{rom}".into()];
        eden.cwd = Some(r"C:\Emulators\Eden".into());
        doc.upsert_emulator(&eden).unwrap();
        doc.upsert_emulator(&emulator("ryu", &["--fullscreen", "{rom}"]))
            .unwrap();
        doc.upsert_game(&emulator_game("mk8", "ryu", r"E:\mk8.xci"))
            .unwrap();
        doc.upsert_game(&emulator_game("smo", "eden", r"D:\smo.nsp"))
            .unwrap();
        let text = doc.to_string();
        for kept in [
            "# Emulators\n",
            "[[emulators]] # mine",
            "# Switch",
            "# the emulator",
        ] {
            assert!(text.contains(kept), "{kept:?} lost:\n{text}");
        }
        assert!(text.contains("emulator = \"ryu\" # the emulator"), "{text}");
        let cfg = Config::parse(&text).unwrap();
        assert_eq!(&cfg, doc.config());
        assert_eq!(cfg.emulators.len(), 2);
        assert_eq!(cfg.emulators[0], eden);
        assert_eq!(cfg.games.len(), 3);
        // Resolved against the edited emulator, not the plan that was passed.
        let LaunchPlan::Emulator { path, args, .. } = &cfg.games[2].plan else {
            panic!("{:?}", cfg.games[2].plan)
        };
        assert_eq!(
            (path.to_str().unwrap(), &args[..]),
            (
                r"C:\Emulators\Eden\eden.exe",
                &[r"D:\smo.nsp".to_owned()][..]
            )
        );
    }

    /// The keys of `[[games]]` entry `i` in `doc`'s text.
    fn game_keys(doc: &ConfigDocument, i: usize) -> Vec<String> {
        let parsed: DocumentMut = doc.to_string().parse().unwrap();
        let games = parsed["games"].as_array_of_tables().unwrap();
        games
            .get(i)
            .unwrap()
            .iter()
            .map(|(k, _)| k.to_owned())
            .collect()
    }

    #[test]
    fn switching_launch_type_drops_the_old_keys() {
        let mut doc = ConfigDocument::parse(EMULATED).unwrap();
        let mut mk8 = doc.config().games[0].clone();
        mk8.plan = LaunchPlan::Steam { appid: 70 };
        doc.upsert_game(&mk8).unwrap();
        assert_eq!(game_keys(&doc, 0), ["id", "name", "steam_appid"]);
        mk8.plan = LaunchPlan::BattleNet {
            code: crate::battlenet::Code::new("WoWF").unwrap(),
        };
        doc.upsert_game(&mk8).unwrap();
        assert_eq!(game_keys(&doc, 0), ["id", "name", "battlenet"]);
        assert!(doc.to_string().contains("battlenet = \"WoWF\""));
        assert_eq!(doc.config().games[0].plan, mk8.plan);
        mk8.plan = LaunchPlan::SteamShortcut { appid: u32::MAX };
        doc.upsert_game(&mk8).unwrap();
        assert_eq!(game_keys(&doc, 0), ["id", "name", "steam_shortcut"]);
        assert!(doc.to_string().contains("steam_shortcut = 4294967295"));
        assert_eq!(doc.config().games[0].plan, mk8.plan);

        let mut botw = doc.config().games[1].clone();
        botw.plan = LaunchPlan::Exe {
            path: "b.exe".into(),
            args: vec!["-x".into()],
            cwd: Some("c".into()),
        };
        doc.upsert_game(&botw).unwrap();
        assert_eq!(game_keys(&doc, 1), ["id", "name", "exe", "args", "cwd"]);

        doc.upsert_game(&emulator_game("botw", "eden", "botw.xci"))
            .unwrap();
        assert_eq!(game_keys(&doc, 1), ["id", "name", "emulator", "rom"]);
        assert_eq!(&Config::parse(&doc.to_string()).unwrap(), doc.config());
    }

    #[test]
    fn unchanged_values_keep_their_formatting() {
        let text = r#"[[emulators]]
id = "eden"
name = "Eden"
exe = 'C:\Eden\eden.exe'
args = [
  "-f", # fullscreen
  "{rom}", # the game
]

[[games]]
id = "mk8"
name = "MK8"
emulator = "eden"
rom = 'D:\mk8.nsp'

[[games]]
id = "tool"
name = 'Tool'
exe = 'C:\Tool\tool.exe'
args = [
  "--big", # bigger window
]
"#;
        let mut doc = ConfigDocument::parse(text).unwrap();
        let cfg = doc.config().clone();
        doc.upsert_emulator(&cfg.emulators[0]).unwrap();
        for game in &cfg.games {
            doc.upsert_game(game).unwrap();
        }
        assert_eq!(doc.to_string(), text);

        // Changing one key leaves the others alone.
        let mut eden = cfg.emulators[0].clone();
        eden.name = "Eden 2".into();
        doc.upsert_emulator(&eden).unwrap();
        let mut tool = cfg.games[1].clone();
        tool.name = "Tool 2".into();
        doc.upsert_game(&tool).unwrap();
        let want = text
            .replace("name = \"Eden\"", "name = \"Eden 2\"")
            .replace("name = 'Tool'", "name = \"Tool 2\"");
        assert_eq!(doc.to_string(), want);
    }

    #[test]
    fn a_new_emulators_section_goes_above_the_games() {
        let mut doc = ConfigDocument::parse(COMMENTED).unwrap();
        doc.upsert_emulator(&emulator("eden", &["{rom}"])).unwrap();
        doc.upsert_emulator(&emulator("ryu", &["{rom}"])).unwrap();
        let text = doc.to_string();
        let first_game = text.find("[[games]]").unwrap();
        assert!(text.rfind("[[emulators]]").unwrap() < first_game, "{text}");
        assert!(
            text.starts_with("# My library\n# (hand-written)\n\n[[emulators]]"),
            "{text}"
        );
        assert!(
            text.contains("args = [\"{rom}\"]\n\n# Steam\n[[games]]"),
            "{text}"
        );
        let cfg = Config::parse(&text).unwrap();
        assert_eq!(&cfg, doc.config());
        assert_eq!((cfg.emulators.len(), cfg.games.len()), (2, 2));
    }

    fn crlf(text: &str) -> String {
        text.replace('\n', "\r\n")
    }

    /// No `\n` without a `\r` before it.
    fn all_crlf(text: &str) -> bool {
        text.match_indices('\n')
            .all(|(i, _)| text[..i].ends_with('\r'))
    }

    #[test]
    fn crlf_files_keep_their_line_endings() {
        for lf in [COMMENTED, EMULATED] {
            let text = crlf(lf);
            let mut doc = ConfigDocument::parse(&text).unwrap();
            assert_eq!(doc.to_string(), text);
            // No-op saves leave the file alone.
            let cfg = doc.config().clone();
            for game in &cfg.games {
                doc.upsert_game(game).unwrap();
            }
            for emulator in &cfg.emulators {
                doc.upsert_emulator(emulator).unwrap();
            }
            assert_eq!(doc.to_string(), text);
        }

        // Real edits match the LF result, with CRLF line endings.
        let edit = |doc: &mut ConfigDocument| {
            let mut hl2 = doc.config().games[0].clone();
            hl2.plan = LaunchPlan::Steam { appid: 380 };
            doc.upsert_game(&hl2).unwrap();
            doc.set_uid(&slug("my-game"), Some(&[1, 2, 3, 4])).unwrap();
            doc.upsert_game(&emulator_game("smo", "eden", r"D:\smo.nsp"))
                .unwrap();
            doc.remove_game(&slug("hl2")).unwrap();
        };
        let mut lf = ConfigDocument::parse(COMMENTED).unwrap();
        lf.upsert_emulator(&emulator("eden", &["{rom}"])).unwrap();
        edit(&mut lf);
        let mut doc = ConfigDocument::parse(&crlf(COMMENTED)).unwrap();
        doc.upsert_emulator(&emulator("eden", &["{rom}"])).unwrap();
        edit(&mut doc);
        let text = doc.to_string();
        assert_eq!(text, crlf(&lf.to_string()));
        assert!(all_crlf(&text), "{text:?}");
        assert_eq!(&Config::parse(&text).unwrap(), doc.config());
    }

    /// Saves `doc`, then checks the file is valid and reloads to the same
    /// config, also through export and import. Returns the saved text.
    fn saved(doc: &ConfigDocument, name: &str) -> String {
        let dir = TempDir::new(name);
        let (path, export) = (dir.join("config.toml"), dir.join("export.toml"));
        doc.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(ConfigDocument::parse(&text).unwrap().config(), doc.config());
        doc.export(&export).unwrap();
        let imported = ConfigDocument::import(&export, &path).unwrap();
        assert_eq!(imported.config(), doc.config());
        text
    }

    #[test]
    fn crlf_multi_line_strings_keep_their_values() {
        let text = "[[games]]\r\nid = \"a\"\r\nname = '''x\r\ny'''\r\nsteam_appid = 1\r\n\
                    \r\n[[games]]\r\nid = \"b\"\r\nname = \"\"\"p\r\nq\"\"\"\r\nsteam_appid = 2\r\n";
        let mut doc = ConfigDocument::parse(text).unwrap();
        let names = |doc: &ConfigDocument| -> Vec<String> {
            doc.config().games.iter().map(|g| g.name.clone()).collect()
        };
        assert_eq!(names(&doc), ["x\r\ny", "p\r\nq"]);
        assert_eq!(doc.to_string(), text);
        for game in doc.config().games.clone() {
            doc.upsert_game(&game).unwrap();
        }
        assert_eq!(saved(&doc, "crlf-multi-noop"), text);

        let mut a = doc.config().games[0].clone();
        a.plan = LaunchPlan::Steam { appid: 10 };
        doc.upsert_game(&a).unwrap();
        let want = text.replace("steam_appid = 1\r", "steam_appid = 10\r");
        assert_eq!(saved(&doc, "crlf-multi-edit"), want);
        assert_eq!(names(&doc), ["x\r\ny", "p\r\nq"]);
    }

    #[test]
    fn mixed_line_endings_inside_a_string_fall_back_to_lf() {
        // CRLF by its first line, but the multi-line name holds an LF:
        // CRLF everywhere would change the name, so the file is written LF.
        let text = "[[games]]\r\nid = \"a\"\r\nname = '''x\ny'''\r\nsteam_appid = 1\r\n";
        let doc = ConfigDocument::parse(text).unwrap();
        assert_eq!(doc.config().games[0].name, "x\ny");
        let want = "[[games]]\nid = \"a\"\nname = '''x\ny'''\nsteam_appid = 1\n";
        assert_eq!(doc.to_string(), want);
        assert_eq!(saved(&doc, "mixed-multi"), want);
    }

    #[test]
    fn a_new_value_with_a_newline_keeps_its_value_in_crlf_files() {
        let mut doc = ConfigDocument::parse(&crlf(COMMENTED)).unwrap();
        doc.upsert_game(&Game {
            id: slug("two"),
            name: "line1\nline2".into(),
            uid: None,
            tag_written: false,
            plan: LaunchPlan::Steam { appid: 5 },
        })
        .unwrap();
        let text = saved(&doc, "crlf-new-newline");
        let cfg = Config::parse(&text).unwrap();
        assert_eq!(cfg.games[2].name, "line1\nline2");
        assert_eq!(&cfg, doc.config());
        // CRLF would turn the new name's line break into `\r\n`, so the
        // file is written LF.
        let want = format!(
            "{COMMENTED}\n[[games]]\nid = \"two\"\nname = \"\"\"\nline1\nline2\"\"\"\nsteam_appid = 5\n"
        );
        assert_eq!(text, want);
    }

    #[test]
    fn a_new_emulators_section_goes_above_the_games_in_crlf_files() {
        let want = crlf(
            r#"# My library
# (hand-written)

[[emulators]]
id = "eden"
name = "EDEN"
exe = 'C:\Emulators\eden.exe'
args = ["{rom}"]

# Steam
[[games]]
id = "hl2"   # short id
name = "Half-Life 2"
steam_appid = 220 # the appid

# Non-Steam
[[games]]
id = "my-game"
name = "My Game"
exe = 'C:\Games\MyGame\game.exe'
args = ["--fullscreen"]
"#,
        );
        // Also a file whose blank line mixes `\r\n` and `\n`.
        let mixed = crlf(COMMENTED).replacen("\r\n\r\n", "\r\n\n", 1);
        for text in [crlf(COMMENTED), mixed] {
            let mut doc = ConfigDocument::parse(&text).unwrap();
            doc.upsert_emulator(&emulator("eden", &["{rom}"])).unwrap();
            assert_eq!(doc.to_string(), want);
            assert_eq!(&Config::parse(&want).unwrap(), doc.config());
        }
    }

    #[test]
    fn blank_lines_are_found_with_either_line_ending() {
        for (text, want) in [
            ("# a\n\n# b\n", Some(5)),
            ("# a\r\n\r\n# b\r\n", Some(7)),
            ("# a\r\n\n# b\r\n", Some(6)),
            ("# a\n\r\n# b\n", Some(6)),
            ("\n\n# a\n", Some(2)),
            ("# a\r\n# b\r\n", None),
            ("\r\n", None),
            ("", None),
        ] {
            assert_eq!(after_last_blank_line(text), want, "{text:?}");
        }
    }

    #[test]
    fn removing_an_emulator_in_use_names_its_games() {
        let mut doc = ConfigDocument::parse(EMULATED).unwrap();
        let err = doc.remove_emulator(&slug("eden")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "emulator \"eden\" is still used by \"mk8\", \"botw\""
        );
        assert!(matches!(
            doc.remove_emulator(&slug("nope")),
            Err(EditError::NoSuchEmulator(_))
        ));
        // Unknown emulators and invalid args fail validation.
        let err = doc
            .upsert_game(&emulator_game("x", "nope", "r"))
            .unwrap_err();
        assert!(err.to_string().contains("\"nope\" is not defined"), "{err}");
        let err = doc.upsert_emulator(&emulator("eden", &["-f"])).unwrap_err();
        assert!(err.to_string().contains("exactly once"), "{err}");
        assert_eq!(doc.to_string(), EMULATED);

        doc.remove_game(&slug("mk8")).unwrap();
        doc.remove_game(&slug("botw")).unwrap();
        doc.remove_emulator(&slug("eden")).unwrap();
        assert!(doc.config().emulators.is_empty());
        assert!(!doc.to_string().contains("# mine"), "{doc}");

        // Into an empty file.
        let mut doc = ConfigDocument::parse("").unwrap();
        doc.upsert_emulator(&emulator("eden", &["{rom}"])).unwrap();
        doc.upsert_game(&emulator_game("mk8", "eden", "mk8.nsp"))
            .unwrap();
        assert_eq!(&Config::parse(&doc.to_string()).unwrap(), doc.config());
        let mut inline = ConfigDocument::parse("emulators = []").unwrap();
        assert!(matches!(
            inline.upsert_emulator(&emulator("eden", &["{rom}"])),
            Err(EditError::Unsupported(_))
        ));
    }

    #[test]
    fn atomic_save_replaces_and_keeps_one_backup() {
        let dir = TempDir::new("save");
        let path = dir.join("config.toml");
        let bak = dir.join("config.toml.bak");
        let mut doc = ConfigDocument::load_or_empty(&path).unwrap();
        doc.upsert_game(&Config::parse(COMMENTED).unwrap().games[0])
            .unwrap();
        doc.save(&path).unwrap();
        assert!(!bak.exists());
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(saved_by_this_process(&first));

        doc.remove_game(&slug("hl2")).unwrap();
        doc.save(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), first);
        assert_eq!(Config::load(&path).unwrap(), Config::default());
        doc.save(&path).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "temp file left behind: {names:?}");
        assert!(!saved_by_this_process("something else"));
    }

    #[test]
    fn export_import_round_trip() {
        let dir = TempDir::new("export");
        let (export, config) = (dir.join("backup.toml"), dir.join("config.toml"));
        let doc = ConfigDocument::parse(COMMENTED).unwrap();
        doc.export(&export).unwrap();
        let text = std::fs::read_to_string(&export).unwrap();
        assert!(text.starts_with("# Mini Launcher config export"), "{text}");
        assert!(
            text.contains("[mini_launcher]\nexport_version = 1\n"),
            "{text}"
        );

        std::fs::write(&config, "# old\n").unwrap();
        let imported = ConfigDocument::import(&export, &config).unwrap();
        assert_eq!(imported.config(), doc.config());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), COMMENTED);
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml.bak")).unwrap(),
            "# old\n"
        );

        // CRLF files stay CRLF through export and import.
        let doc = ConfigDocument::parse(&crlf(COMMENTED)).unwrap();
        doc.export(&export).unwrap();
        let text = std::fs::read_to_string(&export).unwrap();
        assert!(all_crlf(&text), "{text:?}");
        ConfigDocument::import(&export, &config).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), crlf(COMMENTED));
    }

    #[test]
    fn corrupted_imports_are_rejected_before_touching_the_config() {
        let dir = TempDir::new("import");
        let (export, config) = (dir.join("backup.toml"), dir.join("config.toml"));
        std::fs::write(&config, COMMENTED).unwrap();
        let body = "[[games]]\nid = \"a\"\nname = \"A\"\nsteam_appid = 1\n";
        let cases = [
            ("[[games]\nid =".to_owned(), "not a Mini Launcher export"),
            (format!("[mini_launcher]\nexport_version = 2\n{body}"), "not supported"),
            (format!("[mini_launcher]\n{body}"), "missing export_version"),
            (
                format!("[mini_launcher]\nexport_version = \"1\"\n{body}"),
                "export_version must be an integer",
            ),
            (
                format!("{body}# {}\n", "x".repeat(MAX_IMPORT_BYTES as usize)),
                "larger than 1024 KiB",
            ),
            (format!("mini_launcher = 1\n{body}"), "must be a table"),
            (
                format!("[mini_launcher]\nexport_version = 1\nx = 1\n{body}"),
                "unknown keys",
            ),
            (
                "[mini_launcher]\nexport_version = 1\n[[games]]\nid = \"../x\"\nname = \"X\"\nsteam_appid = 1\n"
                    .to_owned(),
                "invalid slug",
            ),
        ];
        for (text, want) in cases {
            std::fs::write(&export, &text).unwrap();
            let err = ConfigDocument::import(&export, &config)
                .unwrap_err()
                .to_string();
            assert!(err.contains(want), "{text:?}: got {err:?}, want {want:?}");
        }
        assert_eq!(std::fs::read_to_string(&config).unwrap(), COMMENTED);
        assert!(!dir.join("config.toml.bak").exists());
    }

    #[test]
    fn unavailable_location_refuses_saves_and_imports() {
        let dir = TempDir::new("unavailable");
        let missing = dir.join("not-mounted");
        let export = dir.join("backup.toml");
        let doc = ConfigDocument::parse(COMMENTED).unwrap();
        doc.export(&export).unwrap();
        let locations = [
            ConfigLocation::OverrideUnavailable {
                dir: Some(missing.clone()),
                reason: "not mounted".into(),
            },
            // Resolved while present, then gone.
            ConfigLocation::Override(missing.clone()),
        ];
        for loc in locations {
            let err = doc.save_to(&loc).unwrap_err();
            assert!(matches!(err, EditError::Location(_)), "{err:?}");
            let err = ConfigDocument::import_into(&export, &loc).unwrap_err();
            assert!(err.to_string().contains("unavailable"), "{err}");
        }
        assert!(!missing.exists());
        // Past the availability check, the write itself never recreates it.
        let toctou = doc.save_at(&missing.join("config.toml"), false);
        assert!(matches!(toctou, Err(EditError::Write { .. })), "{toctou:?}");
        assert!(!missing.exists());
        let ok = ConfigLocation::Default(dir.join("fresh"));
        assert_eq!(doc.save_to(&ok).unwrap(), dir.join("fresh/config.toml"));
    }

    #[test]
    fn temp_names_are_unique_within_the_process() {
        let dir = TempDir::new("concurrent");
        let path = dir.join("config.toml");
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let mut errors = Vec::new();
                    for j in 0..20 {
                        if let Err(e) = write_atomic(&path, &format!("# {i} {j}\n"), false, false) {
                            errors.push(e);
                        }
                    }
                    errors
                })
            })
            .collect();
        for t in threads {
            for e in t.join().unwrap() {
                // Windows may refuse a rename while another one is in
                // flight; a temp name collision would be AlreadyExists.
                let EditError::Write { source, .. } = &e else {
                    panic!("{e:?}")
                };
                assert_ne!(source.kind(), std::io::ErrorKind::AlreadyExists, "{e:?}");
                if !cfg!(windows) {
                    panic!("{e:?}");
                }
            }
        }
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(names.len(), 1, "temp files left behind: {names:?}");
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_permissions_and_symlinks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("unix");
        let (real, link) = (dir.join("real.toml"), dir.join("config.toml"));
        std::fs::write(&real, "").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        ConfigDocument::parse(COMMENTED)
            .unwrap()
            .save(&link)
            .unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&real).unwrap(), COMMENTED);
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
