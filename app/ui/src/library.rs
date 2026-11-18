//! The settings window's model: the library as loaded from `config.toml`,
//! and the edit form with its validation. Pure (no egui), so it is tested
//! on every OS.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mini_launcher_core::config::edit::ConfigDocument;
use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_core::config::{check_rom_arg, emulator_plan, Config, Emulator, Game};
use mini_launcher_core::emulators::EmulatorPreset;
use mini_launcher_core::launch::LaunchPlan;
use mini_launcher_core::payload::Slug;

/// The library as last read from (or written to) the config file.
pub struct Library {
    location: Option<ConfigLocation>,
    /// The document, or why it can't be shown (folder unavailable, invalid
    /// file).
    doc: Result<ConfigDocument, String>,
    /// The file's mtime when read, to notice outside edits.
    stamp: Option<SystemTime>,
}

impl Library {
    pub fn load(location: Option<ConfigLocation>) -> Library {
        let path = file(location.as_ref());
        let stamp = path.as_deref().ok().and_then(modified);
        let doc = path.and_then(|p| {
            ConfigDocument::load_or_empty(&p).map_err(|e| format!("{}: {}", p.display(), chain(&e)))
        });
        Library {
            location,
            doc,
            stamp,
        }
    }

    /// Whether `location` differs from the one loaded, or the file changed
    /// since.
    pub fn is_stale(&self, location: Option<&ConfigLocation>) -> bool {
        self.location.as_ref() != location
            || file(location).is_ok_and(|p| modified(&p) != self.stamp)
    }

    pub fn location(&self) -> Option<&ConfigLocation> {
        self.location.as_ref()
    }

    /// Why the library can't be shown or edited, if it can't.
    pub fn problem(&self) -> Option<&str> {
        self.doc.as_ref().err().map(String::as_str)
    }

    pub fn config(&self) -> Option<&Config> {
        self.doc.as_ref().ok().map(ConfigDocument::config)
    }

    pub fn games(&self) -> &[Game] {
        self.config().map_or(&[], |c| &c.games)
    }

    pub fn emulators(&self) -> &[Emulator] {
        self.config().map_or(&[], |c| &c.emulators)
    }

    /// Adds or updates `emulator` and saves.
    pub fn save_emulator(&mut self, emulator: &Emulator) -> Result<(), String> {
        self.change_emulator(&emulator.id, |doc| doc.upsert_emulator(emulator))
    }

    /// Why emulator `id` can't be removed now (games still use it), if so.
    pub fn removal_problem(&self, id: &Slug) -> Option<String> {
        let mut doc = self.doc.as_ref().ok()?.clone();
        doc.remove_emulator(id).err().map(|e| chain(&e))
    }

    /// Removes the emulator with this id and saves.
    pub fn remove_emulator(&mut self, id: &Slug) -> Result<(), String> {
        self.change_emulator(id, |doc| doc.remove_emulator(id))
    }

    /// Adds or updates `game` and saves.
    pub fn save_game(&mut self, game: &Game) -> Result<(), String> {
        self.change(&game.id, |doc| doc.upsert_game(game))
    }

    /// Adds or updates `game` and saves; `take_uid` is first unbound from
    /// every other entry (a tag UID maps to one game, and a UID mapping
    /// would override what was just written to the tag).
    pub fn save_game_taking_uid(&mut self, game: &Game, take_uid: &[u8]) -> Result<(), String> {
        self.change(&game.id, |doc| {
            let others: Vec<Slug> = doc
                .config()
                .games
                .iter()
                .filter(|g| g.id != game.id && g.uid.as_deref() == Some(take_uid))
                .map(|g| g.id.clone())
                .collect();
            for id in &others {
                doc.set_uid(id, None)?;
            }
            doc.upsert_game(game)
        })
    }

    /// Removes the game with this id and saves.
    pub fn remove(&mut self, id: &Slug) -> Result<(), String> {
        self.change(id, |doc| doc.remove_game(id))
    }

    /// Applies `edit` to the file as it is now on disk (so an outside edit
    /// made meanwhile is kept), then saves it through [`ConfigDocument`]
    /// (validated, atomic, comments kept). If game `id` on disk is no longer
    /// what this library shows (added, changed or removed outside the app),
    /// nothing is saved: the library is reloaded and the error says why.
    fn change(
        &mut self,
        id: &Slug,
        edit: impl FnOnce(
            &mut ConfigDocument,
        ) -> Result<(), mini_launcher_core::config::edit::EditError>,
    ) -> Result<(), String> {
        self.change_where(id, |c| c.by_id(id), |g| &g.name, edit)
    }

    /// [`Library::change`] for emulator `id`.
    fn change_emulator(
        &mut self,
        id: &Slug,
        edit: impl FnOnce(
            &mut ConfigDocument,
        ) -> Result<(), mini_launcher_core::config::edit::EditError>,
    ) -> Result<(), String> {
        self.change_where(
            id,
            |c| c.emulators.iter().find(|e| &e.id == id),
            |e| &e.name,
            edit,
        )
    }

    /// `find` looks up entry `id` in a config; `name` names it.
    fn change_where<T: PartialEq>(
        &mut self,
        id: &Slug,
        find: impl Fn(&Config) -> Option<&T>,
        name: fn(&T) -> &String,
        edit: impl FnOnce(
            &mut ConfigDocument,
        ) -> Result<(), mini_launcher_core::config::edit::EditError>,
    ) -> Result<(), String> {
        let location = self
            .location
            .clone()
            .ok_or("this system has no config folder")?;
        let path = location.writable_file().map_err(|e| e.to_string())?;
        let stamp = modified(&path);
        let mut doc = ConfigDocument::load_or_empty(&path).map_err(|e| chain(&e))?;
        let shown = self.config().and_then(&find);
        if let Some(e) = conflict(id, shown, find(doc.config()), name) {
            (self.doc, self.stamp) = (Ok(doc), stamp);
            return Err(format!(
                "{e} outside the app. The library was reloaded: check it and try again."
            ));
        }
        edit(&mut doc).map_err(|e| chain(&e))?;
        doc.save_to(&location).map_err(|e| chain(&e))?;
        self.stamp = modified(&path);
        self.doc = Ok(doc);
        Ok(())
    }
}

/// How entry `id` on disk differs from the one `shown` (`None`: a new entry
/// or already gone), if it does.
fn conflict<T: PartialEq>(
    id: &Slug,
    shown: Option<&T>,
    on_disk: Option<&T>,
    name: fn(&T) -> &String,
) -> Option<String> {
    match (shown, on_disk) {
        (None, Some(g)) => Some(format!("\"{}\" (id {id}) was added", name(g))),
        (Some(g), None) => Some(format!("\"{}\" was removed", name(g))),
        (Some(g), Some(d)) if g != d => Some(format!("\"{}\" was changed", name(g))),
        _ => None,
    }
}

fn file(location: Option<&ConfigLocation>) -> Result<PathBuf, String> {
    location
        .ok_or_else(|| "this system has no config folder".to_owned())?
        .config_file()
        .map_err(|e| e.to_string())
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

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

/// The form fields that can have an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    AppId,
    /// The id a new program gets (`id:<slug>` on its tag).
    Id,
    Exe,
    Args,
    Cwd,
    /// An emulator game's ROM file.
    Rom,
}

/// The edit form: text as typed, turned into a [`Game`] on save.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Form {
    /// The entry being edited; `None` for a new one (an id is made from the
    /// name on save).
    pub id: Option<Slug>,
    pub name: String,
    pub steam: bool,
    pub app_id: String,
    pub exe: String,
    pub args: String,
    pub cwd: String,
    /// The emulator an emulator game runs on (then `steam` is ignored).
    pub emulator: Option<Slug>,
    pub rom: String,
    /// The bound tag UID; cleared by Unbind.
    pub uid: Option<Vec<u8>>,
    /// Errors from the last save attempt, shown next to their fields.
    pub errors: Vec<(Field, String)>,
    /// A save error not tied to a field (e.g. the file could not be written).
    pub problem: Option<String>,
}

impl Form {
    pub fn new() -> Form {
        Form {
            steam: true,
            ..Form::default()
        }
    }

    pub fn from_game(game: &Game) -> Form {
        let mut form = Form {
            id: Some(game.id.clone()),
            name: game.name.clone(),
            uid: game.uid.clone(),
            ..Form::default()
        };
        match &game.plan {
            LaunchPlan::Steam { appid } => {
                form.steam = true;
                form.app_id = appid.to_string();
            }
            LaunchPlan::Exe { path, args, cwd } => {
                form.exe = path.display().to_string();
                form.args = join_args(args);
                form.cwd = cwd
                    .as_ref()
                    .map(|c| c.display().to_string())
                    .unwrap_or_default();
            }
            LaunchPlan::Emulator { emulator, rom, .. } => {
                form.emulator = Some(emulator.clone());
                form.rom = rom.display().to_string();
            }
        }
        form
    }

    pub fn error(&self, field: Field) -> Option<&str> {
        self.errors
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, e)| e.as_str())
    }

    /// The entry this form describes, or the errors per field. `config` is
    /// the current library (a new entry gets an id not used there).
    pub fn to_game(&self, config: &Config) -> Result<Game, Vec<(Field, String)>> {
        let mut errors = Vec::new();
        let name = self.name.trim();
        if name.is_empty() {
            errors.push((Field::Name, "Enter a name.".to_owned()));
        }
        let plan = if let Some(id) = &self.emulator {
            let rom = unquote(&self.rom);
            let emulator = config.emulators.iter().find(|e| &e.id == id);
            match (emulator, rom.is_empty()) {
                (None, _) => {
                    let e = "This emulator is no longer in the library.";
                    errors.push((Field::Rom, e.to_owned()));
                    None
                }
                (_, true) => {
                    errors.push((Field::Rom, "Choose the game file.".to_owned()));
                    None
                }
                (Some(e), false) => Some(emulator_plan(e, PathBuf::from(rom))),
            }
        } else if self.steam {
            match self.app_id.trim().parse::<u32>() {
                Ok(appid) if appid > 0 => Some(LaunchPlan::Steam { appid }),
                _ => {
                    let e = "Enter the game's Steam app ID (a number, e.g. 220).";
                    errors.push((Field::AppId, e.to_owned()));
                    None
                }
            }
        } else {
            let exe = unquote(&self.exe);
            if exe.is_empty() {
                errors.push((Field::Exe, "Choose the program to start.".to_owned()));
            }
            let args = split_args(&self.args)
                .map_err(|e| errors.push((Field::Args, e.to_owned())))
                .ok();
            let cwd = unquote(&self.cwd);
            args.filter(|_| !exe.is_empty())
                .map(|args| LaunchPlan::Exe {
                    path: PathBuf::from(exe),
                    args,
                    cwd: (!cwd.is_empty()).then(|| PathBuf::from(cwd)),
                })
        };
        match plan {
            Some(plan) if errors.is_empty() => Ok(Game {
                id: self.id.clone().unwrap_or_else(|| new_id(name, config)),
                name: name.to_owned(),
                uid: self.uid.clone(),
                plan,
            }),
            _ => Err(errors),
        }
    }
}

/// The emulator form: text as typed, turned into an [`Emulator`] on save.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmuForm {
    /// The entry being edited; `None` for a new one (an id is made from the
    /// name on save, and never changes after).
    pub id: Option<Slug>,
    pub name: String,
    pub exe: String,
    pub args: String,
    /// Kept as it is: not edited in the window.
    pub cwd: Option<PathBuf>,
    pub errors: Vec<(Field, String)>,
    pub problem: Option<String>,
}

impl EmuForm {
    /// A new entry from `preset`, with its `exe` if it was detected.
    pub fn from_preset(preset: &EmulatorPreset, exe: Option<PathBuf>) -> EmuForm {
        let args: Vec<String> = preset.default_args.iter().map(|&a| a.into()).collect();
        EmuForm {
            name: preset.name.into(),
            exe: exe.map(|p| p.display().to_string()).unwrap_or_default(),
            args: join_args(&args),
            ..EmuForm::default()
        }
    }

    pub fn from_emulator(e: &Emulator) -> EmuForm {
        EmuForm {
            id: Some(e.id.clone()),
            name: e.name.clone(),
            exe: e.exe.display().to_string(),
            args: join_args(&e.args),
            cwd: e.cwd.clone(),
            ..EmuForm::default()
        }
    }

    /// The entry this form describes, or the errors per field.
    pub fn to_emulator(&self, config: &Config) -> Result<Emulator, Vec<(Field, String)>> {
        let mut errors = Vec::new();
        let name = self.name.trim();
        if name.is_empty() {
            errors.push((Field::Name, "Enter a name.".to_owned()));
        }
        let exe = unquote(&self.exe);
        if exe.is_empty() {
            errors.push((Field::Exe, "Choose the emulator's program.".to_owned()));
        }
        let args = split_args(&self.args)
            .map_err(str::to_owned)
            .and_then(|a| check_rom_arg(&a).map(|()| a))
            .map_err(|e| errors.push((Field::Args, e)));
        match args {
            Ok(args) if errors.is_empty() => Ok(Emulator {
                id: self.id.clone().unwrap_or_else(|| {
                    free_id(name, "emulator", |id| {
                        config.emulators.iter().any(|e| &e.id == id)
                    })
                }),
                name: name.to_owned(),
                exe: PathBuf::from(exe),
                args,
                cwd: self.cwd.clone(),
            }),
            _ => Err(errors),
        }
    }
}

/// Trims spaces and the quotes Explorer's "Copy as path" adds.
pub(crate) fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
}

/// An id for a new entry: the name as a slug, made unique in `config`.
pub fn new_id(name: &str, config: &Config) -> Slug {
    free_id(name, "game", |id| config.by_id(id).is_some())
}

/// `name` as a slug (`fallback` if nothing is left), made unique: not
/// `taken`.
pub(crate) fn free_id(name: &str, fallback: &str, taken: impl Fn(&Slug) -> bool) -> Slug {
    let mut base = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            base.push(c);
        } else if !base.is_empty() && !base.ends_with('-') {
            base.push('-');
        }
    }
    base.truncate(Slug::MAX_LEN - 4);
    let base = match base.trim_end_matches('-') {
        "" => fallback,
        b => b,
    };
    (1..)
        .map(|n| match n {
            1 => base.to_owned(),
            n => format!("{base}-{n}"),
        })
        .filter_map(|s| Slug::new(&s).ok())
        .find(|id| !taken(id))
        .expect("a free id")
}

/// Splits an argument line: spaces separate arguments, `"..."` groups
/// (with `""` for a literal quote inside). Backslashes are plain characters,
/// so Windows paths need no escaping.
pub fn split_args(line: &str) -> Result<Vec<String>, &'static str> {
    let mut args = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let arg = current.get_or_insert_with(String::new);
                loop {
                    match chars.next() {
                        None => return Err("A quote is not closed."),
                        Some('"') if chars.peek() == Some(&'"') => {
                            chars.next();
                            arg.push('"');
                        }
                        Some('"') => break,
                        Some(c) => arg.push(c),
                    }
                }
            }
            c if c.is_whitespace() => args.extend(current.take()),
            c => current.get_or_insert_with(String::new).push(c),
        }
    }
    args.extend(current);
    Ok(args)
}

/// The inverse of [`split_args`].
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.contains(|c: char| c.is_whitespace() || c == '"') {
                format!("\"{}\"", a.replace('"', "\"\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = "# my games\n\n[[games]]\nid = \"hades\" # keep me\nname = \"Hades\"\nuid = \"04:A2:3B:1C:5D:80:00\"\nsteam_appid = 1145360\n\n# the doom one\n[[games]]\nid = \"doom\"\nname = \"DOOM\"\nexe = 'C:\\Games\\gzdoom.exe'\nargs = [\"-iwad\", \"doom 2.wad\"]\n";

    fn temp_location(name: &str, text: Option<&str>) -> ConfigLocation {
        let dir =
            std::env::temp_dir().join(format!("mini-launcher-ui-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(text) = text {
            std::fs::write(dir.join("config.toml"), text).unwrap();
        }
        ConfigLocation::Default(dir)
    }

    fn read(location: &ConfigLocation) -> String {
        std::fs::read_to_string(location.config_file().unwrap()).unwrap()
    }

    #[test]
    fn args_round_trip() {
        let cases: &[&[&str]] = &[
            &[],
            &["-skipintro"],
            &["-iwad", "doom 2.wad", r"C:\a b\"],
            &["", "say \"hi\"", "x"],
        ];
        for &args in cases {
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            assert_eq!(split_args(&join_args(&args)).unwrap(), args);
        }
        assert_eq!(
            split_args(r#"  --path="C:\My Games\x"  -w  "#).unwrap(),
            [r"--path=C:\My Games\x", "-w"]
        );
        assert_eq!(split_args("\"abc"), Err("A quote is not closed."));
    }

    #[test]
    fn ids_are_slugs_and_unique() {
        let config = Config::parse(LIBRARY).unwrap();
        assert_eq!(new_id("Hades", &config).as_str(), "hades-2");
        assert_eq!(
            new_id("  Half-Life 2: Episode One!", &config).as_str(),
            "half-life-2-episode-one"
        );
        assert_eq!(new_id("ゲーム", &config).as_str(), "game");
        assert_eq!(new_id(&"x".repeat(100), &config).as_str().len(), 60);
    }

    #[test]
    fn form_validation_marks_fields() {
        let config = Config::default();
        let mut form = Form::new();
        form.app_id = "abc".into();
        let errors = form.to_game(&config).unwrap_err();
        let fields: Vec<_> = errors.iter().map(|e| e.0).collect();
        assert_eq!(fields, [Field::Name, Field::AppId]);

        form.steam = false;
        form.name = "Doom".into();
        form.args = "\"open".into();
        let fields: Vec<_> = form
            .to_game(&config)
            .unwrap_err()
            .iter()
            .map(|e| e.0)
            .collect();
        assert_eq!(fields, [Field::Exe, Field::Args]);

        form.exe = "\"C:\\Games\\gzdoom.exe\"".into();
        form.args = "-iwad \"doom 2.wad\"".into();
        let game = form.to_game(&config).unwrap();
        assert_eq!(game.id.as_str(), "doom");
        assert_eq!(
            game.plan,
            LaunchPlan::Exe {
                path: "C:\\Games\\gzdoom.exe".into(),
                args: vec!["-iwad".into(), "doom 2.wad".into()],
                cwd: None
            }
        );
        form.steam = true;
        form.app_id = "0".into();
        assert_eq!(form.to_game(&config).unwrap_err()[0].0, Field::AppId);
    }

    #[test]
    fn edit_save_keeps_comments_and_unbinds() {
        let location = temp_location("edit", Some(LIBRARY));
        let mut library = Library::load(Some(location.clone()));
        assert_eq!(library.games().len(), 2);
        let mut form = Form::from_game(&library.games()[0]);
        assert_eq!((form.steam, form.app_id.as_str()), (true, "1145360"));
        form.name = "Hades II".into();
        form.uid = None;
        let game = form.to_game(library.config().unwrap()).unwrap();
        library.save_game(&game).unwrap();
        let text = read(&location);
        assert!(
            text.contains("# my games") && text.contains("# keep me"),
            "{text}"
        );
        assert!(
            text.contains("name = \"Hades II\"") && !text.contains("uid"),
            "{text}"
        );
        assert!(!library.is_stale(Some(&location)));

        let doom = Form::from_game(&library.games()[1]);
        assert_eq!(doom.args, "-iwad \"doom 2.wad\"");
        assert_eq!(
            doom.to_game(library.config().unwrap()).unwrap(),
            library.games()[1]
        );
    }

    #[test]
    fn add_and_remove() {
        let location = temp_location("add", None);
        let mut library = Library::load(Some(location.clone()));
        assert!(library.games().is_empty() && library.problem().is_none());
        let mut form = Form::new();
        form.name = "Celeste".into();
        form.app_id = "504230".into();
        library
            .save_game(&form.to_game(library.config().unwrap()).unwrap())
            .unwrap();
        assert!(read(&location).contains("steam_appid = 504230"));
        let id = library.games()[0].id.clone();
        library.remove(&id).unwrap();
        assert!(library.games().is_empty());
        assert!(!read(&location).contains("celeste"));
    }

    #[test]
    fn save_merges_outside_edits_and_reports_problems() {
        let location = temp_location("merge", Some(LIBRARY));
        let mut library = Library::load(Some(location.clone()));
        let path = location.config_file().unwrap();
        // Edited by hand while the window was open.
        std::fs::write(
            &path,
            format!("{LIBRARY}\n[[games]]\nid = \"new\"\nname = \"New\"\nsteam_appid = 1\n"),
        )
        .unwrap();
        library.remove(&Slug::new("doom").unwrap()).unwrap();
        let ids: Vec<_> = library.games().iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, ["hades", "new"]);

        std::fs::write(&path, "[[games]\n").unwrap();
        assert!(library.is_stale(Some(&location)));
        let broken = Library::load(Some(location.clone()));
        assert!(broken.problem().unwrap().contains("config.toml"));
        assert!(library.remove(&Slug::new("hades").unwrap()).is_err());

        let gone = ConfigLocation::OverrideUnavailable {
            dir: None,
            reason: "not mounted".into(),
        };
        assert!(Library::load(Some(gone))
            .problem()
            .unwrap()
            .contains("not mounted"));
    }
    #[test]
    fn save_refuses_games_changed_outside() {
        let location = temp_location("conflict", Some(LIBRARY));
        let path = location.config_file().unwrap();
        let hand = "\n[[games]]\nid = \"celeste\"\nname = \"Hand\"\nsteam_appid = 1\n";

        // A new game whose id was hand-added meanwhile is not overwritten.
        let mut library = Library::load(Some(location.clone()));
        let mut form = Form::new();
        form.name = "Celeste".into();
        form.app_id = "504230".into();
        let game = form.to_game(library.config().unwrap()).unwrap();
        std::fs::write(&path, format!("{LIBRARY}{hand}")).unwrap();
        let e = library.save_game(&game).unwrap_err();
        assert!(
            e.contains("\"Hand\" (id celeste) was added outside the app"),
            "{e}"
        );
        assert!(read(&location).contains("name = \"Hand\""));
        // Reloaded, so trying again gets a free id.
        let game = form.to_game(library.config().unwrap()).unwrap();
        assert_eq!(game.id.as_str(), "celeste-2");
        library.save_game(&game).unwrap();
        assert_eq!(library.games().len(), 4);

        // Editing a game removed or changed outside is refused too.
        let doom = Form::from_game(library.config().unwrap().by_id(&slug("doom")).unwrap());
        let doom = doom.to_game(library.config().unwrap()).unwrap();
        std::fs::write(&path, LIBRARY.replace("DOOM", "DOOM II")).unwrap();
        let e = library.save_game(&doom).unwrap_err();
        assert!(e.contains("\"DOOM\" was changed"), "{e}");
        std::fs::write(&path, "").unwrap();
        let e = library.save_game(&doom).unwrap_err();
        assert!(e.contains("\"DOOM II\" was removed"), "{e}");
        assert_eq!(read(&location), "");
        assert!(library.remove(&slug("hades")).is_err());
    }

    #[test]
    fn emulator_form_checks_the_rom_rule_and_saves() {
        let location = temp_location("emulators", Some(LIBRARY));
        let mut library = Library::load(Some(location.clone()));
        let mut form = EmuForm::from_preset(&mini_launcher_core::emulators::EDEN, None);
        assert_eq!(
            (form.name.as_str(), form.args.as_str()),
            ("Eden", "-f -g {rom}")
        );
        form.args = "-g x{rom}".into();
        let errors = form.to_emulator(library.config().unwrap()).unwrap_err();
        let fields: Vec<_> = errors.iter().map(|e| e.0).collect();
        assert_eq!(fields, [Field::Exe, Field::Args]);
        assert!(
            errors[1].1.contains("must be a whole argument"),
            "{errors:?}"
        );

        form.exe = r"C:\Eden\eden.exe".into();
        form.args = "-g {rom}".into();
        let eden = form.to_emulator(library.config().unwrap()).unwrap();
        assert_eq!(eden.id.as_str(), "eden");
        library.save_emulator(&eden).unwrap();
        // A second one from the preset gets a free id.
        let again = form.to_emulator(library.config().unwrap()).unwrap();
        assert_eq!(again.id.as_str(), "eden-2");
        assert_eq!(
            EmuForm::from_emulator(&library.emulators()[0]).args,
            "-g {rom}"
        );

        let mut game = library.games()[0].clone();
        game.plan = LaunchPlan::Emulator {
            emulator: eden.id.clone(),
            rom: "mk8.nsp".into(),
            path: eden.exe.clone(),
            args: Vec::new(),
            cwd: None,
        };
        library.save_game(&game).unwrap();
        let e = library.removal_problem(&eden.id).unwrap();
        assert!(e.contains("still used by \"hades\""), "{e}");
        assert!(library.remove_emulator(&eden.id).is_err());
        library.remove(&game.id).unwrap();
        assert_eq!(library.removal_problem(&eden.id), None);
        library.remove_emulator(&eden.id).unwrap();
        assert!(!read(&location).contains("[[emulators]]"));
    }

    fn slug(s: &str) -> Slug {
        Slug::new(s).unwrap()
    }
}
