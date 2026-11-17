//! Export, import and moving the config folder: the model behind the
//! settings window's Export / Import buttons and its Config folder dialog.
//! Pure (no egui), so it is tested on every OS.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mini_launcher_core::config::edit::ConfigDocument;
use mini_launcher_core::config::location::{set_pointer, ConfigLocation, CONFIG_FILE};

/// An error with its sources, `a: b: c`.
fn chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        msg.push_str(&format!(": {s}"));
        source = s.source();
    }
    msg
}

/// `(year, month, day, seconds into the day)` of `t`, in UTC.
fn civil(t: SystemTime) -> (i64, u32, u32, u64) {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    // Howard Hinnant's days-to-civil.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day, secs % 86_400)
}

/// The export file name the save dialog suggests.
pub fn export_name(now: SystemTime) -> String {
    let (y, m, d, _) = civil(now);
    format!("mini-launcher-backup-{y:04}-{m:02}-{d:02}.toml")
}

/// Writes the config at `location` (empty if it has none yet) as an export.
pub fn export(location: &ConfigLocation, to: &Path) -> Result<(), String> {
    let file = location.config_file().map_err(|e| e.to_string())?;
    let doc = ConfigDocument::load_or_empty(&file).map_err(|e| chain(&e))?;
    doc.export(to).map_err(|e| chain(&e))
}

/// A validated import waiting for the user's confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    pub from: PathBuf,
    pub games: usize,
    /// Games bound to a tag UID.
    pub bound: usize,
}

impl ImportPlan {
    /// Reads and validates `from` without changing anything.
    pub fn read(from: &Path) -> Result<ImportPlan, String> {
        let doc = ConfigDocument::read_export(from).map_err(|e| chain(&e))?;
        let games = &doc.config().games;
        Ok(ImportPlan {
            from: from.to_owned(),
            games: games.len(),
            bound: games.iter().filter(|g| g.uid.is_some()).count(),
        })
    }

    /// Backs up the current `config.toml` (if any) as
    /// `config.toml.import-<date>-<time>.bak`, then imports. Returns the
    /// backup's name.
    pub fn apply(
        &self,
        location: &ConfigLocation,
        now: SystemTime,
    ) -> Result<Option<String>, String> {
        let file = location.writable_file().map_err(|e| e.to_string())?;
        let backup = backup(&file, "import", now)?;
        ConfigDocument::import_into(&self.from, location).map_err(|e| chain(&e))?;
        Ok(backup)
    }
}

/// Copies `file` (if it exists) to `config.toml.<kind>-<date>-<time>.bak`
/// next to it, a name later saves never replace. Returns that name.
fn backup(file: &Path, kind: &str, now: SystemTime) -> Result<Option<String>, String> {
    if !file.exists() {
        return Ok(None);
    }
    let (y, m, d, s) = civil(now);
    let (h, min, sec) = (s / 3600, s / 60 % 60, s % 60);
    let name = format!("{CONFIG_FILE}.{kind}-{y:04}{m:02}{d:02}-{h:02}{min:02}{sec:02}.bak");
    std::fs::copy(file, file.with_file_name(&name))
        .map_err(|e| format!("cannot back up {}: {e}", file.display()))?;
    Ok(Some(name))
}

/// The current `config.toml`, if the folder is available and it exists
/// (so there is a library to take along).
pub fn mine(current: Option<&ConfigLocation>) -> Option<PathBuf> {
    current?.config_file().ok().filter(|f| f.is_file())
}

/// What is already in a folder the config is moving to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Existing {
    /// No `config.toml` there.
    Nothing,
    /// A loadable `config.toml` with this many games.
    Config(usize),
    /// A `config.toml` that can't be loaded, and why.
    Broken(String),
}

/// A config folder change waiting for the user's choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    /// The new folder.
    pub dir: PathBuf,
    /// Moving back to the default folder (removes `location.toml`).
    pub to_default: bool,
    pub existing: Existing,
}

impl Move {
    /// Plans moving the config to `dir`; `None` if it is already there.
    pub fn plan(
        default_dir: &Path,
        current: Option<&ConfigLocation>,
        dir: &Path,
    ) -> Result<Option<Move>, String> {
        let to_default = same(dir, default_dir);
        if !dir.is_absolute() || !(dir.is_dir() || to_default) {
            return Err(format!("\"{}\" is not a folder.", dir.display()));
        }
        let here = match current {
            Some(ConfigLocation::Default(d) | ConfigLocation::Override(d)) => same(dir, d),
            _ => false,
        };
        if here && (to_default == matches!(current, Some(ConfigLocation::Default(_)))) {
            return Ok(None);
        }
        let file = dir.join(CONFIG_FILE);
        let existing = if !file.exists() {
            Existing::Nothing
        } else {
            match ConfigDocument::load_or_empty(&file) {
                Ok(doc) => Existing::Config(doc.config().games.len()),
                Err(e) => Existing::Broken(chain(&e)),
            }
        };
        Ok(Some(Move {
            dir: dir.to_owned(),
            to_default,
            existing,
        }))
    }

    /// Whether the user has to choose between the config already there and
    /// their own.
    pub fn needs_choice(&self) -> bool {
        self.existing != Existing::Nothing
    }

    /// Switches to the new folder: with `copy_mine`, the current config
    /// (see [`mine`]) is first saved there, after the one there (if any) is
    /// copied to `config.toml.move-<date>-<time>.bak`, whose name is
    /// returned. Then `location.toml` is written (or removed for the
    /// default folder, which is created if missing).
    pub fn apply(
        &self,
        default_dir: &Path,
        current: Option<&ConfigLocation>,
        copy_mine: bool,
        now: SystemTime,
    ) -> Result<Option<String>, String> {
        let target = self.dir.join(CONFIG_FILE);
        let mut kept = None;
        if copy_mine {
            let file = mine(current).ok_or("you have no config to copy")?;
            let doc = ConfigDocument::load_or_empty(&file).map_err(|e| chain(&e))?;
            kept = backup(&target, "move", now)?;
            doc.save(&target).map_err(|e| chain(&e))?;
        } else if let Existing::Broken(e) = &self.existing {
            return Err(format!("The config there can't be loaded: {e}"));
        }
        if self.to_default {
            std::fs::create_dir_all(default_dir)
                .map_err(|e| format!("cannot create {}: {e}", default_dir.display()))?;
        }
        let pointer = (!self.to_default).then_some(self.dir.as_path());
        set_pointer(default_dir, pointer).map_err(|e| chain(&e))?;
        Ok(kept)
    }
}

/// Whether two paths name the same folder.
fn same(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mini-launcher-transfer-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const TWO: &str = "# mine\n[[games]]\nid = \"a\"\nname = \"A\"\nsteam_appid = 1\nuid = \"04:01:02:03\"\n\n[[games]]\nid = \"b\"\nname = \"B\"\nsteam_appid = 2\n";

    #[test]
    fn dates() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_437_296);
        assert_eq!(export_name(t), "mini-launcher-backup-2026-11-23.toml");
        assert_eq!(
            civil(SystemTime::UNIX_EPOCH + Duration::from_secs(951_782_400)).1,
            2
        );
        assert_eq!(
            civil(SystemTime::UNIX_EPOCH + Duration::from_secs(951_868_800)).2,
            1
        );
    }

    #[test]
    fn export_then_import_with_summary_and_backup() {
        let dir = temp("roundtrip");
        let loc = ConfigLocation::Default(dir.clone());
        std::fs::write(dir.join(CONFIG_FILE), TWO).unwrap();
        let out = dir.join("export.toml");
        export(&loc, &out).unwrap();
        let plan = ImportPlan::read(&out).unwrap();
        assert_eq!((plan.games, plan.bound), (2, 1));

        std::fs::write(dir.join(CONFIG_FILE), "# other\n").unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_437_296);
        let backup = plan.apply(&loc, t).unwrap().unwrap();
        assert_eq!(backup, "config.toml.import-20261123-123456.bak");
        assert_eq!(
            std::fs::read_to_string(dir.join(&backup)).unwrap(),
            "# other\n"
        );
        let now = std::fs::read_to_string(dir.join(CONFIG_FILE)).unwrap();
        assert!(
            now.contains("# mine") && !now.contains("mini_launcher"),
            "{now}"
        );
    }

    #[test]
    fn bad_import_changes_nothing() {
        let dir = temp("bad");
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "[mini_launcher]\nexport_version = 9\n").unwrap();
        let err = ImportPlan::read(&bad).unwrap_err();
        assert!(err.contains("export_version 9"), "{err}");
        let gone = ConfigLocation::OverrideUnavailable {
            dir: None,
            reason: "x".into(),
        };
        std::fs::write(&bad, TWO).unwrap();
        let plan = ImportPlan::read(&bad).unwrap();
        assert!(plan.apply(&gone, SystemTime::now()).is_err());
    }

    fn t() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_437_296)
    }

    #[test]
    fn reset_from_an_unavailable_folder_to_an_empty_default() {
        let (default, synced) = (temp("unavail"), temp("unavail-synced"));
        set_pointer(&default, Some(&synced)).unwrap();
        std::fs::remove_dir_all(&synced).unwrap();
        let gone = ConfigLocation::resolve(&default);
        assert!(!gone.is_available() && mine(Some(&gone)).is_none());
        let mv = Move::plan(&default, Some(&gone), &default)
            .unwrap()
            .unwrap();
        assert!(!mv.needs_choice() && mv.to_default);
        assert!(mv.apply(&default, Some(&gone), true, t()).is_err());
        assert_eq!(mv.apply(&default, Some(&gone), false, t()), Ok(None));
        assert_eq!(
            ConfigLocation::resolve(&default),
            ConfigLocation::Default(default)
        );
    }

    #[test]
    fn move_copies_mine_or_uses_theirs_and_resets() {
        let (default, synced) = (temp("mv-default"), temp("mv-synced"));
        std::fs::write(default.join(CONFIG_FILE), TWO).unwrap();
        let home = ConfigLocation::Default(default.clone());
        assert_eq!(Move::plan(&default, Some(&home), &default).unwrap(), None);
        assert!(Move::plan(&default, Some(&home), Path::new("relative")).is_err());

        // Empty target: mine is copied there.
        let mv = Move::plan(&default, Some(&home), &synced).unwrap().unwrap();
        assert!(!mv.needs_choice() && !mv.to_default);
        mv.apply(&default, Some(&home), true, t()).unwrap();
        let there = ConfigLocation::resolve(&default);
        assert_eq!(there, ConfigLocation::Override(synced.clone()));
        assert_eq!(
            std::fs::read_to_string(synced.join(CONFIG_FILE)).unwrap(),
            TWO
        );

        // Back to the default, which has a config: use the one there.
        std::fs::write(default.join(CONFIG_FILE), "").unwrap();
        let back = Move::plan(&default, Some(&there), &default)
            .unwrap()
            .unwrap();
        assert_eq!(
            (back.to_default, back.existing.clone()),
            (true, Existing::Config(0))
        );
        back.apply(&default, Some(&there), false, t()).unwrap();
        assert_eq!(ConfigLocation::resolve(&default), home);
        assert_eq!(
            std::fs::read_to_string(default.join(CONFIG_FILE)).unwrap(),
            ""
        );

        // Copy mine over an existing one: theirs is kept under a dated name
        // that later saves don't replace.
        let mv = Move::plan(&default, Some(&home), &synced).unwrap().unwrap();
        assert_eq!(mv.existing, Existing::Config(2));
        let kept = mv.apply(&default, Some(&home), true, t()).unwrap().unwrap();
        assert_eq!(kept, "config.toml.move-20261123-123456.bak");
        let there = ConfigLocation::resolve(&default);
        ConfigDocument::load_or_empty(&synced.join(CONFIG_FILE))
            .unwrap()
            .save_to(&there)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(synced.join(CONFIG_FILE)).unwrap(),
            ""
        );
        assert_eq!(std::fs::read_to_string(synced.join(&kept)).unwrap(), TWO);
        assert_ne!(
            std::fs::read_to_string(synced.join("config.toml.bak")).unwrap(),
            TWO
        );

        // A broken config there can only be overwritten.
        std::fs::write(default.join(CONFIG_FILE), "not toml [").unwrap();
        let there = ConfigLocation::resolve(&default);
        let mv = Move::plan(&default, Some(&there), &default)
            .unwrap()
            .unwrap();
        assert!(matches!(mv.existing, Existing::Broken(_)));
        assert!(mv.apply(&default, Some(&there), false, t()).is_err());
        assert_eq!(ConfigLocation::resolve(&default), there);
    }
}
