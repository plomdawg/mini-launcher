//! The settings window's model: the library as loaded from `config.toml`,
//! and the edit form with its validation. Pure (no egui), so it is tested
//! on every OS.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mini_launcher_core::config::edit::ConfigDocument;
use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_core::config::{Config, Game};
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

    /// Adds or updates `game` and saves.
    pub fn save_game(&mut self, game: &Game) -> Result<(), String> {
        self.change(&game.id, |doc| doc.upsert_game(game))
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
        let location = self
            .location
            .clone()
            .ok_or("this system has no config folder")?;
        let path = location.writable_file().map_err(|e| e.to_string())?;
        let stamp = modified(&path);
        let mut doc = ConfigDocument::load_or_empty(&path).map_err(|e| chain(&e))?;
        let shown = self.config().and_then(|c| c.by_id(id));
        if let Some(e) = conflict(id, shown, doc.config().by_id(id)) {
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

/// How game `id` on disk differs from the one `shown` (`None`: a new game
/// or already gone), if it does.
fn conflict(id: &Slug, shown: Option<&Game>, on_disk: Option<&Game>) -> Option<String> {
    match (shown, on_disk) {
        (None, Some(g)) => Some(format!("\"{}\" (id {id}) was added", g.name)),
        (Some(g), None) => Some(format!("\"{}\" was removed", g.name)),
        (Some(g), Some(d)) if g != d => Some(format!("\"{}\" was changed", g.name)),
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
    Exe,
    Args,
    Cwd,
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
        let plan = if self.steam {
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

/// Trims spaces and the quotes Explorer's "Copy as path" adds.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
}

/// An id for a new entry: the name as a slug, made unique in `config`.
pub fn new_id(name: &str, config: &Config) -> Slug {
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
        "" => "game",
        b => b,
    };
    (1..)
        .map(|n| match n {
            1 => base.to_owned(),
            n => format!("{base}-{n}"),
        })
        .filter_map(|s| Slug::new(&s).ok())
        .find(|id| config.by_id(id).is_none())
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

    fn slug(s: &str) -> Slug {
        Slug::new(s).unwrap()
    }
}
