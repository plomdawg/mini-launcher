//! The add-game wizard's model: pick a game, tap a tag, write it (or bind
//! its UID when it can't be written), test. Pure (no egui), so every branch
//! is tested on every OS; the screens are in [`crate::wizard_view`].
//!
//! The reader belongs to the tray's run loop. The wizard reaches it through
//! a [`Link`] (a [`SessionClient`] in the app): launches stay suspended
//! while the wizard holds one, and dropping it (Done, Cancel, closing the
//! window) gives the reader back.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use mini_launcher_core::config::{emulator_plan, format_uid, Config, Emulator, Game};
use mini_launcher_core::emulators::{
    display_name, scan_roms, suggest_slug, EmulatorPreset, RomScan, ScanOptions, PRESETS,
};
use mini_launcher_core::launch::LaunchPlan;
use mini_launcher_core::payload::{Payload, Slug};
use mini_launcher_core::reader::Uid;
use mini_launcher_core::session::{SessionClient, Update, WriteFailure};
use mini_launcher_core::steam::SteamGame;

#[cfg(test)]
use crate::library::Field;
use crate::library::{free_id, new_id, unquote, Form, Library};

/// The wizard's line to the reader (see [`mini_launcher_core::session`]).
pub trait Link {
    /// Report the next tag seen, ignoring `skip` until it is lifted off.
    fn watch(&self, skip: Option<Uid>);
    /// Write `payload` to tag `uid`.
    fn write(&self, uid: Uid, payload: Payload);
    fn try_recv(&mut self) -> Option<Update>;
    /// Nothing more will arrive (the run loop ended the session).
    fn is_closed(&self) -> bool;
}

impl Link for SessionClient {
    fn watch(&self, skip: Option<Uid>) {
        SessionClient::watch(self, skip);
    }
    fn write(&self, uid: Uid, payload: Payload) {
        SessionClient::write(self, uid, payload);
    }
    fn try_recv(&mut self) -> Option<Update> {
        SessionClient::try_recv(self)
    }
    fn is_closed(&self) -> bool {
        SessionClient::is_closed(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Choose an installed Steam game, Program... or Emulator game...
    Pick,
    /// The program form (exe, args, working dir, id).
    Program,
    /// An emulator game: the emulator and the game file.
    Rom,
    /// An emulator game's name and tag ID.
    Name,
    /// Waiting for a tag on the reader.
    Tap,
    /// The tag holds another game or is bound to one: ask first.
    Overwrite { uid: Uid, question: String },
    /// The run loop is writing the tag.
    Writing { uid: Uid },
    /// Saved; "Launch now" and Done.
    Test { outcome: Outcome },
}

impl Step {
    /// Position in the step bar: pick, tag, write, test.
    pub fn number(&self) -> usize {
        match self {
            Step::Pick | Step::Program | Step::Rom | Step::Name => 0,
            Step::Tap | Step::Overwrite { .. } => 1,
            Step::Writing { .. } => 2,
            Step::Test { .. } => 3,
        }
    }
}

/// How the game ended up on the tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The payload was written and verified.
    Written { uid: Uid },
    /// The tag already held it.
    AlreadyThere { uid: Uid },
    /// The tag can't be written; its UID is bound instead.
    Bound { uid: Uid, failure: WriteFailure },
    /// Saved without a tag.
    Skipped,
}

pub struct Wizard {
    pub step: Step,
    /// Started from the edit form's "Write tag...": Cancel goes back there.
    pub from_edit: bool,
    /// Installed Steam games, or why they can't be listed.
    pub steam: Result<Vec<SteamGame>, String>,
    pub search: String,
    /// The chosen Steam appid on the pick step.
    pub selected: Option<u32>,
    /// The program's id (`id:<slug>` on its tag), as typed.
    pub slug: String,
    /// Whether `slug` still follows the name.
    pub auto_slug: bool,
    pub slug_error: Option<String>,
    /// The game being added or tagged.
    pub game: Option<Game>,
    /// The emulator-game steps' choices.
    pub emu: EmuPick,
    /// A warning for the current step (failed write, reader gone, ...).
    pub problem: Option<String>,
    /// Whether the reader is connected; `None` until the run loop says.
    pub reader: Option<bool>,
    /// The result of the last "Launch now".
    pub launched: Option<Result<(), String>>,
    link: Option<Box<dyn Link>>,
    /// The tag an Overwrite question is about, with what it holds.
    pending: Option<(Uid, Payload)>,
    /// The other games the user agreed to take this tag from.
    unbind: Vec<Slug>,
    /// Asked again at save time (see [`Wizard::finish`]): the outcome to
    /// save on "yes".
    resume: Option<Outcome>,
    /// Set when the config was saved; see [`Wizard::take_saved`].
    saved: bool,
}

impl Wizard {
    /// Starts on the pick step.
    pub fn new(steam: Result<Vec<SteamGame>, String>) -> Wizard {
        Wizard {
            step: Step::Pick,
            from_edit: false,
            steam,
            search: String::new(),
            selected: None,
            slug: String::new(),
            auto_slug: true,
            slug_error: None,
            game: None,
            emu: EmuPick::default(),
            problem: None,
            reader: None,
            launched: None,
            link: None,
            pending: None,
            unbind: Vec::new(),
            resume: None,
            saved: false,
        }
    }

    /// Writes a tag for `game` (from the edit form): straight to the tap
    /// step once [`Wizard::begin`] is called.
    pub fn for_game(game: Game) -> Wizard {
        Wizard {
            from_edit: true,
            game: Some(game),
            ..Wizard::new(Ok(Vec::new()))
        }
    }

    /// The Steam games matching the search, case-insensitively.
    pub fn filtered(&self) -> Vec<&SteamGame> {
        let needle = self.search.trim().to_lowercase();
        let games = self.steam.as_deref().unwrap_or_default();
        games
            .iter()
            .filter(|g| g.name.to_lowercase().contains(&needle) || g.appid.to_string() == needle)
            .collect()
    }

    /// The library entry that launches Steam app `appid`, if any.
    pub fn in_library(config: Option<&Config>, appid: u32) -> Option<&Game> {
        config?
            .games
            .iter()
            .find(|g| g.plan == LaunchPlan::Steam { appid })
    }

    /// Takes the selected Steam game: its library entry if it has one, else
    /// a new one. Returns whether the wizard moved on (then call
    /// [`Wizard::begin`]).
    pub fn pick_steam(&mut self, config: &Config) -> bool {
        let Some(appid) = self.selected else {
            return false;
        };
        let Some(steam) = self.filtered().into_iter().find(|g| g.appid == appid) else {
            return false;
        };
        let game = match Wizard::in_library(Some(config), appid) {
            Some(game) => game.clone(),
            None => Game {
                id: new_id(&steam.name, config),
                name: steam.name.clone(),
                uid: None,
                plan: LaunchPlan::Steam { appid },
            },
        };
        self.game = Some(game);
        true
    }

    /// Opens the program form (on `form`, which the Browse buttons fill).
    pub fn program(&mut self, form: &mut Form, config: &Config) {
        *form = Form::new();
        form.steam = false;
        self.auto_slug = true;
        self.slug_error = None;
        self.follow_name(form, config);
        self.step = Step::Program;
    }

    /// Keeps the id in step with the name until the user edits it.
    pub fn follow_name(&mut self, form: &Form, config: &Config) {
        if self.auto_slug {
            let name = form.name.trim();
            self.slug = if name.is_empty() {
                String::new()
            } else {
                new_id(name, config).as_str().to_owned()
            };
        }
    }

    /// Validates the program form; returns whether the wizard moved on
    /// (then call [`Wizard::begin`]).
    pub fn program_next(&mut self, form: &mut Form, config: &Config) -> bool {
        form.problem = None;
        let slug = self.checked_slug(config);
        form.id = slug.clone();
        let game = form.to_game(config);
        form.id = None;
        match (game, slug) {
            (Ok(game), Some(_)) => {
                form.errors.clear();
                self.game = Some(game);
                true
            }
            (Ok(_), None) => {
                form.errors.clear();
                false
            }
            (Err(errors), _) => {
                form.errors = errors;
                false
            }
        }
    }

    /// The typed tag ID if it is valid and no other game has it; else
    /// `None`, with `slug_error` saying why.
    fn checked_slug(&mut self, config: &Config) -> Option<Slug> {
        let error = match Slug::new(self.slug.trim()) {
            Ok(slug) if config.by_id(&slug).is_none() => {
                self.slug_error = None;
                return Some(slug);
            }
            Ok(_) => "Another game already uses this ID.",
            Err(_) => "Use 1-64 lowercase letters, digits and dashes, e.g. my-game.",
        };
        self.slug_error = Some(error.to_owned());
        None
    }

    /// Opens the emulator-game steps, on the first configured emulator.
    pub fn emulator_game(&mut self, config: &Config) {
        let first = config.emulators.first().map(|e| e.id.clone());
        self.emu = EmuPick {
            emulator: first,
            ..EmuPick::default()
        };
        self.step = Step::Rom;
    }

    /// The chosen emulator, if it is (still) in `config`.
    pub fn emulator<'c>(&self, config: &'c Config) -> Option<&'c Emulator> {
        let id = self.emu.emulator.as_ref()?;
        config.emulators.iter().find(|e| &e.id == id)
    }

    /// Lists the chosen emulator's game files in `dir` and the folders in
    /// it; nothing when its preset (and so its file types) is unknown.
    pub fn scan(&mut self, dir: PathBuf, config: &Config) {
        let Some(preset) = self.emulator(config).and_then(preset_of) else {
            return;
        };
        let opts = ScanOptions {
            recurse_one_level: true,
            ..ScanOptions::default()
        };
        let found = scan_roms(&dir, preset, opts);
        self.emu.scan = Some((dir, found));
    }

    /// The Rom step's Next. A file the library already has on this
    /// emulator reuses that entry and moves on to the tag (returns `true`:
    /// then call [`Wizard::begin`]); else on to the Name step, with a name
    /// and tag ID made from the file name.
    pub fn rom_next(&mut self, config: &Config) -> bool {
        let rom = unquote(&self.emu.rom).to_owned();
        let emulator = self.emulator(config).map(|e| e.id.clone());
        self.emu.rom_error = match (&emulator, rom.is_empty()) {
            (None, _) => Some("Choose an emulator, or add Eden.".to_owned()),
            (_, true) => Some("Choose the game file.".to_owned()),
            _ => None,
        };
        let Some(emulator) = emulator.filter(|_| !rom.is_empty()) else {
            return false;
        };
        let same = |g: &&Game| {
            matches!(&g.plan, LaunchPlan::Emulator { emulator: e, rom: r, .. }
                if *e == emulator && r == Path::new(&rom))
        };
        if let Some(game) = config.games.iter().find(same) {
            self.game = Some(game.clone());
            return true;
        }
        if self.emu.named != rom {
            // The file name, from a path typed for either OS.
            let file = rom.rsplit(['/', '\\']).next().unwrap_or_default();
            let stem = Path::new(file).file_stem().unwrap_or_default();
            self.emu.name = display_name(&stem.to_string_lossy());
            self.emu.named = rom;
            self.auto_slug = true;
        }
        self.emu.name_error = None;
        self.slug_error = None;
        self.follow_rom_name(config);
        self.step = Step::Name;
        false
    }

    /// Keeps the tag ID in step with the emulator game's name until the
    /// user edits it: the name's slug, made unique; empty (so one must be
    /// typed) when the name has nothing to make one from.
    pub fn follow_rom_name(&mut self, config: &Config) {
        if self.auto_slug {
            let taken = |id: &Slug| config.by_id(id).is_some();
            let slug = suggest_slug(&self.emu.name).map(|s| free_id(s.as_str(), "game", taken));
            self.slug = slug.map(|s| s.as_str().to_owned()).unwrap_or_default();
        }
    }

    /// The Name step's Next; returns whether the wizard moved on (then call
    /// [`Wizard::begin`]).
    pub fn name_next(&mut self, config: &Config) -> bool {
        let name = self.emu.name.trim().to_owned();
        self.emu.name_error = name.is_empty().then(|| "Enter a name.".to_owned());
        let slug = self.checked_slug(config);
        let Some(emulator) = self.emulator(config) else {
            // Removed outside the app meanwhile.
            self.step = Step::Rom;
            return self.rom_next(config);
        };
        let (Some(id), false) = (slug, name.is_empty()) else {
            return false;
        };
        let rom = PathBuf::from(unquote(&self.emu.rom));
        self.game = Some(Game {
            id,
            name,
            uid: None,
            plan: emulator_plan(emulator, rom),
        });
        true
    }

    /// Goes to the tap step with `link` to the reader (`None`: this app has
    /// no reader, only Skip works) and asks for a tag.
    pub fn begin(&mut self, link: Option<Box<dyn Link>>) {
        self.link = link;
        self.problem = None;
        self.reader = None;
        self.step = Step::Tap;
        match &self.link {
            Some(link) => link.watch(None),
            None => self.problem = Some(NO_READER.to_owned()),
        }
    }

    /// What goes on the tag: `steam://<appid>`, or `id:<slug>` for programs.
    pub fn payload(&self) -> Option<Payload> {
        let game = self.game.as_ref()?;
        Some(match &game.plan {
            LaunchPlan::Steam { appid } => Payload::Steam(*appid),
            LaunchPlan::Exe { .. } | LaunchPlan::Emulator { .. } => Payload::Id(game.id.clone()),
        })
    }

    /// Handles what the run loop sent.
    pub fn poll(&mut self, library: &mut Library) {
        while let Some(update) = self.link.as_mut().and_then(|l| l.try_recv()) {
            self.on_update(update, library);
        }
        if self.link.as_ref().is_some_and(|l| l.is_closed()) {
            self.link = None;
            if matches!(
                self.step,
                Step::Tap | Step::Overwrite { .. } | Step::Writing { .. }
            ) {
                self.step = Step::Tap;
                self.problem = Some(LINK_CLOSED.to_owned());
            }
        }
    }

    /// One update from the run loop; stale ones (for an earlier step) are
    /// ignored.
    pub fn on_update(&mut self, update: Update, library: &mut Library) {
        match update {
            Update::Reader { connected } => self.reader = Some(connected),
            // Only once the run loop confirmed the session (its first
            // `Reader` update): before that, launches may not be off yet.
            Update::Tag { uid, payload } if self.step == Step::Tap && self.armed() => {
                self.tag(uid, payload, library)
            }
            Update::Written { uid, result } if self.step == (Step::Writing { uid }) => match result
            {
                Ok(()) => self.finish(Outcome::Written { uid }, library),
                Err(failure) if failure.needs_uid_binding() => {
                    self.finish(Outcome::Bound { uid, failure }, library)
                }
                Err(failure) => {
                    self.problem = Some(format!(
                        "{} Lift the cart off and put it back to try again.",
                        failure_text(&failure)
                    ));
                    self.step = Step::Tap;
                    self.watch(Some(uid));
                }
            },
            _ => {}
        }
    }

    /// A tag arrived on the tap step.
    fn tag(&mut self, uid: Uid, payload: Payload, library: &mut Library) {
        let (Some(game), Some(want)) = (&self.game, self.payload()) else {
            return;
        };
        self.problem = None;
        let bound_to = library
            .games()
            .iter()
            .find(|g| g.id != game.id && g.uid.as_deref() == Some(uid.as_bytes()));
        self.unbind = bound_to.map(|g| g.id.clone()).into_iter().collect();
        let holds_other = payload != Payload::Unusable && payload != want;
        if !holds_other && bound_to.is_none() {
            return self.bind(uid, payload, library);
        }
        let mut question = String::new();
        if holds_other {
            let held = describe(&payload, library.config(), &self.steam);
            question.push_str(&format!("This tag holds {held}. "));
        }
        if let Some(other) = bound_to {
            question.push_str(&format!(
                "It is bound to \"{}\" in your library; that game loses this tag. ",
                other.name
            ));
        }
        question.push_str(&format!("Use it for \"{}\"?", game.name));
        self.pending = Some((uid, payload));
        self.step = Step::Overwrite { uid, question };
    }

    /// Writes the tag, or (when it already holds the game) just saves.
    fn bind(&mut self, uid: Uid, payload: Payload, library: &mut Library) {
        let Some(want) = self.payload() else {
            return;
        };
        if payload == want {
            return self.finish(Outcome::AlreadyThere { uid }, library);
        }
        match &self.link {
            Some(link) => {
                link.write(uid, want);
                self.step = Step::Writing { uid };
            }
            None => self.problem = Some(LINK_CLOSED.to_owned()),
        }
    }

    /// The answer to the Overwrite question.
    pub fn answer(&mut self, yes: bool, library: &mut Library) {
        let Step::Overwrite { .. } = self.step else {
            return;
        };
        let Some((uid, payload)) = self.pending.take() else {
            return;
        };
        match (yes, self.resume.take()) {
            (true, Some(outcome)) => self.finish(outcome, library),
            (true, None) => self.bind(uid, payload, library),
            (false, resumed) => {
                if resumed.is_some() {
                    self.problem =
                        Some("Not saved. Put a cart on the reader to try again, or Skip.".into());
                }
                self.step = Step::Tap;
                self.watch(Some(uid));
            }
        }
    }

    /// Back from the program form or the emulator-game steps.
    pub fn back(&mut self, form: &mut Form) {
        self.step = match self.step {
            Step::Program => {
                form.errors.clear();
                Step::Pick
            }
            Step::Rom => Step::Pick,
            Step::Name => Step::Rom,
            _ => return,
        };
        self.slug_error = None;
    }

    /// The test step's "Save again", after saving failed.
    pub fn retry_save(&mut self, library: &mut Library) {
        if let (Step::Test { outcome }, Some(_)) = (&self.step, &self.problem) {
            let outcome = outcome.clone();
            self.finish(outcome, library);
        }
    }

    /// Saves the game without a tag.
    pub fn skip(&mut self, library: &mut Library) {
        if matches!(self.step, Step::Tap) {
            self.finish(Outcome::Skipped, library);
        }
    }

    fn watch(&self, skip: Option<Uid>) {
        if let Some(link) = &self.link {
            link.watch(skip);
        }
    }

    /// Saves the game, gives the reader back and shows the test step.
    fn finish(&mut self, outcome: Outcome, library: &mut Library) {
        let Some(mut game) = self.game.clone() else {
            return;
        };
        // The tag is taken from other games as the file is now: if that
        // set grew since the user confirmed (an outside edit bound this tag
        // to another game meanwhile), ask again rather than unbind it
        // silently.
        if let Outcome::Bound { uid, .. }
        | Outcome::Written { uid }
        | Outcome::AlreadyThere { uid } = &outcome
        {
            let fresh = Library::load(library.location().cloned());
            let now: Vec<&Game> = fresh
                .games()
                .iter()
                .filter(|g| g.id != game.id && g.uid.as_deref() == Some(uid.as_bytes()))
                .collect();
            if let Some(other) = now.iter().find(|g| !self.unbind.contains(&g.id)) {
                self.unbind = now.iter().map(|g| g.id.clone()).collect();
                let question = format!(
                    "The library changed meanwhile: this tag is now bound to \"{}\"; \
                     that game loses this tag. Use it for \"{}\"?",
                    other.name, game.name
                );
                self.pending = Some((*uid, Payload::Unusable));
                self.resume = Some(outcome.clone());
                self.step = Step::Overwrite {
                    uid: *uid,
                    question,
                };
                return;
            }
        }
        let result = match &outcome {
            Outcome::Bound { uid, .. } => {
                game.uid = Some(uid.as_bytes().to_vec());
                library.save_game_taking_uid(&game, uid.as_bytes())
            }
            Outcome::Written { uid } | Outcome::AlreadyThere { uid } => {
                library.save_game_taking_uid(&game, uid.as_bytes())
            }
            Outcome::Skipped => library.save_game(&game),
        };
        self.link = None;
        self.saved |= result.is_ok();
        self.problem = result.err().map(|e| format!("Not saved: {e}"));
        self.game = Some(game);
        self.step = Step::Test { outcome };
    }

    /// Whether the config was saved since the last call.
    pub fn take_saved(&mut self) -> bool {
        std::mem::take(&mut self.saved)
    }

    /// Whether the wizard holds the reader (launches are suspended).
    pub fn has_reader(&self) -> bool {
        self.link.is_some()
    }

    /// Whether the run loop has taken up the session (it sent its first
    /// reader state), so launches are off and a tag may go on: only then
    /// does the tap step ask for the cart.
    pub fn armed(&self) -> bool {
        self.has_reader() && self.reader.is_some()
    }
}

/// The emulator-game steps' choices, as typed or picked.
#[derive(Debug, Default)]
pub struct EmuPick {
    pub emulator: Option<Slug>,
    /// The game file.
    pub rom: String,
    pub rom_error: Option<String>,
    /// The folder scanned last, and the game files found in it.
    pub scan: Option<(PathBuf, RomScan)>,
    /// The game's name (Name step).
    pub name: String,
    pub name_error: Option<String>,
    /// The file `name` was made from, so Back then Next keeps an edit.
    named: String,
}

/// The preset `emulator` was set up from (same name or exe name), which
/// knows its game file types.
pub fn preset_of(emulator: &Emulator) -> Option<&'static EmulatorPreset> {
    let exe = emulator.exe.file_stem().and_then(OsStr::to_str);
    PRESETS.iter().find(|p| {
        p.name.eq_ignore_ascii_case(emulator.name.trim())
            || exe.is_some_and(|x| p.name.eq_ignore_ascii_case(x))
    })
}

const NO_READER: &str =
    "This app has no reader to write with. Use Skip to add the game without a tag.";
const LINK_CLOSED: &str =
    "The reader is no longer available to the wizard. Cancel and start again, or Skip.";

/// A payload as written on a tag.
pub fn payload_text(payload: &Payload) -> String {
    match payload {
        Payload::Steam(appid) => format!("steam://{appid}"),
        Payload::Id(slug) => format!("id:{slug}"),
        Payload::Unusable => "nothing usable".to_owned(),
    }
}

/// What a tag holds, with the game's name when it is known.
fn describe(
    payload: &Payload,
    config: Option<&Config>,
    steam: &Result<Vec<SteamGame>, String>,
) -> String {
    let name = match payload {
        Payload::Steam(appid) => Wizard::in_library(config, *appid)
            .map(|g| g.name.clone())
            .or_else(|| {
                let games = steam.as_deref().unwrap_or_default();
                games
                    .iter()
                    .find(|g| g.appid == *appid)
                    .map(|g| g.name.clone())
            }),
        Payload::Id(slug) => config.and_then(|c| c.by_id(slug)).map(|g| g.name.clone()),
        Payload::Unusable => None,
    };
    match name {
        Some(name) => format!("\"{name}\" ({})", payload_text(payload)),
        None => payload_text(payload),
    }
}

/// Why a write failed, in plain words.
pub fn failure_text(failure: &WriteFailure) -> String {
    match failure {
        WriteFailure::Locked => "This tag is read-only (locked), so it can't be written.".into(),
        WriteFailure::TooSmall { need, have } => {
            format!("This tag is too small: the game needs {need} bytes and the tag holds {have}.")
        }
        WriteFailure::UnknownTag => {
            "This tag isn't a writable NFC tag of the kind Mini Launcher uses (NTAG213).".into()
        }
        WriteFailure::TagRemoved => "The cart left the reader before writing finished.".into(),
        WriteFailure::VerifyFailed => "The tag didn't read back what was written.".into(),
        WriteFailure::Unusable => "There is nothing to write for this game.".into(),
        WriteFailure::Reader(e) => format!("The reader stopped responding ({e})."),
    }
}

/// The test step's summary of `outcome` for `game`.
pub fn outcome_text(outcome: &Outcome, payload: Option<&Payload>) -> String {
    let written = payload.map(payload_text).unwrap_or_default();
    match outcome {
        Outcome::Written { uid } => {
            format!("Wrote {written} to tag {}.", format_uid(uid.as_bytes()))
        }
        Outcome::AlreadyThere { uid } => format!(
            "Tag {} already holds {written}; nothing to write.",
            format_uid(uid.as_bytes())
        ),
        Outcome::Bound { uid, failure } => format!(
            "{} Mini Launcher remembers tag {} instead, so it launches this game on this PC.",
            failure_text(failure),
            format_uid(uid.as_bytes())
        ),
        Outcome::Skipped => "Saved without a tag.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mini_launcher_core::config::location::ConfigLocation;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    const LIBRARY: &str = "[[games]]\nid = \"portal-2\"\nname = \"Portal 2\"\nuid = \"04:E1:5C:93:27:81:00\"\nsteam_appid = 620\n\n[[games]]\nid = \"hades\"\nname = \"Hades\"\nsteam_appid = 1145360\n";

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Sent {
        Watch(Option<Uid>),
        Write(Uid, Payload),
    }

    #[derive(Default)]
    struct Line {
        sent: Vec<Sent>,
        inbox: VecDeque<Update>,
        closed: bool,
        dropped: bool,
    }

    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Line>>);

    impl Link for Fake {
        fn watch(&self, skip: Option<Uid>) {
            self.0.borrow_mut().sent.push(Sent::Watch(skip));
        }
        fn write(&self, uid: Uid, payload: Payload) {
            self.0.borrow_mut().sent.push(Sent::Write(uid, payload));
        }
        fn try_recv(&mut self) -> Option<Update> {
            self.0.borrow_mut().inbox.pop_front()
        }
        fn is_closed(&self) -> bool {
            self.0.borrow().closed
        }
    }

    /// Records that the session was given back.
    struct Tracked(Fake);
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.0 .0.borrow_mut().dropped = true;
        }
    }
    impl Link for Tracked {
        fn watch(&self, skip: Option<Uid>) {
            self.0.watch(skip)
        }
        fn write(&self, uid: Uid, payload: Payload) {
            self.0.write(uid, payload)
        }
        fn try_recv(&mut self) -> Option<Update> {
            self.0.try_recv()
        }
        fn is_closed(&self) -> bool {
            self.0.is_closed()
        }
    }

    struct Rig {
        wizard: Wizard,
        library: Library,
        location: ConfigLocation,
        link: Fake,
    }

    impl Rig {
        fn send(&mut self, update: Update) {
            self.link.0.borrow_mut().inbox.push_back(update);
            self.wizard.poll(&mut self.library);
        }
        fn tag(&mut self, hex: &str, payload: Payload) {
            self.send(Update::Tag {
                uid: uid(hex),
                payload,
            });
        }
        fn written(&mut self, hex: &str, result: Result<(), WriteFailure>) {
            self.send(Update::Written {
                uid: uid(hex),
                result,
            });
        }
        fn sent(&self) -> Vec<Sent> {
            std::mem::take(&mut self.link.0.borrow_mut().sent)
        }
        fn config(&self) -> Config {
            Config::parse(&std::fs::read_to_string(self.location.config_file().unwrap()).unwrap())
                .unwrap()
        }
        fn released(&self) -> bool {
            self.link.0.borrow().dropped
        }
    }

    fn uid(hex: &str) -> Uid {
        Uid::from_hex(hex).unwrap()
    }

    fn steam_games() -> Vec<SteamGame> {
        [(620, "Portal 2"), (504230, "Celeste"), (1145360, "Hades")]
            .into_iter()
            .map(|(appid, name)| SteamGame {
                appid,
                name: name.into(),
                installdir: name.into(),
                library: "C:\\Steam".into(),
            })
            .collect()
    }

    /// A wizard at the tap step for Celeste (a new Steam game).
    fn rig(name: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!(
            "mini-launcher-wizard-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), LIBRARY).unwrap();
        let location = ConfigLocation::Default(dir);
        let library = Library::load(Some(location.clone()));
        let mut wizard = Wizard::new(Ok(steam_games()));
        wizard.search = "cel".into();
        assert_eq!(wizard.filtered().len(), 1);
        wizard.selected = Some(504230);
        assert!(wizard.pick_steam(library.config().unwrap()));
        let link = Fake::default();
        wizard.begin(Some(Box::new(Tracked(link.clone()))));
        let rig = Rig {
            wizard,
            library,
            location,
            link,
        };
        assert_eq!(rig.sent(), [Sent::Watch(None)]);
        let mut rig = rig;
        rig.send(Update::Reader { connected: true });
        rig
    }

    const A: &str = "04a1b2c3d4e5f6";
    const PORTAL_TAG: &str = "04e15c93278100";

    #[test]
    fn blank_tag_is_written_and_saved_and_the_reader_given_back() {
        let mut r = rig("written");
        r.send(Update::Reader { connected: true });
        assert_eq!(r.wizard.reader, Some(true));
        r.tag(A, Payload::Unusable);
        assert_eq!(r.wizard.step, Step::Writing { uid: uid(A) });
        assert_eq!(r.sent(), [Sent::Write(uid(A), Payload::Steam(504230))]);
        assert!(!r.released());
        r.written(A, Ok(()));
        assert_eq!(
            r.wizard.step,
            Step::Test {
                outcome: Outcome::Written { uid: uid(A) }
            }
        );
        assert!(r.released() && !r.wizard.has_reader());
        assert!(r.wizard.take_saved() && !r.wizard.take_saved());
        let celeste = r.config().games.into_iter().find(|g| g.name == "Celeste");
        let celeste = celeste.unwrap();
        assert_eq!(celeste.id.as_str(), "celeste");
        assert_eq!(celeste.uid, None, "a written tag carries the game itself");
    }

    #[test]
    fn unwritable_tags_fall_back_to_the_uid_with_the_reason() {
        let causes = [
            (WriteFailure::Locked, "read-only"),
            (WriteFailure::TooSmall { need: 80, have: 48 }, "too small"),
            (WriteFailure::UnknownTag, "NTAG213"),
        ];
        for (i, (failure, words)) in causes.into_iter().enumerate() {
            let mut r = rig(&format!("fallback-{i}"));
            r.tag(A, Payload::Unusable);
            r.written(A, Err(failure.clone()));
            let outcome = Outcome::Bound {
                uid: uid(A),
                failure,
            };
            let text = outcome_text(&outcome, r.wizard.payload().as_ref());
            assert!(text.contains(words) && text.contains("04:A1:B2"), "{text}");
            assert_eq!(r.wizard.step, Step::Test { outcome });
            let config = r.config();
            let game = config.by_uid(uid(A).as_bytes()).unwrap();
            assert_eq!(game.name, "Celeste");
            assert!(r.released());
        }
    }

    #[test]
    fn other_failures_ask_for_another_try_with_the_same_tag_lifted() {
        let causes = [
            WriteFailure::TagRemoved,
            WriteFailure::VerifyFailed,
            WriteFailure::Reader("I/O error".into()),
        ];
        for (i, failure) in causes.into_iter().enumerate() {
            let mut r = rig(&format!("retry-{i}"));
            r.tag(A, Payload::Unusable);
            r.sent();
            r.written(A, Err(failure.clone()));
            assert_eq!(r.wizard.step, Step::Tap, "{failure:?}");
            let problem = r.wizard.problem.clone().unwrap();
            assert!(problem.starts_with(&failure_text(&failure)), "{problem}");
            assert_eq!(r.sent(), [Sent::Watch(Some(uid(A)))]);
            assert!(!r.released());
            // The next tag goes through.
            r.tag(A, Payload::Unusable);
            r.written(A, Ok(()));
            assert!(matches!(r.wizard.step, Step::Test { .. }));
            assert_eq!(r.wizard.problem, None);
        }
    }

    #[test]
    fn a_tag_holding_another_game_asks_before_overwriting() {
        let mut r = rig("overwrite");
        r.tag(A, Payload::Steam(1145360));
        let Step::Overwrite { question, .. } = &r.wizard.step else {
            panic!("{:?}", r.wizard.step);
        };
        assert_eq!(
            question,
            "This tag holds \"Hades\" (steam://1145360). Use it for \"Celeste\"?"
        );
        assert_eq!(r.sent(), []);
        // No: wait for another tag, skipping this one until it's lifted.
        r.wizard.answer(false, &mut r.library);
        assert_eq!(r.wizard.step, Step::Tap);
        assert_eq!(r.sent(), [Sent::Watch(Some(uid(A)))]);
        // Yes: write over it.
        r.tag(A, Payload::Steam(1145360));
        r.wizard.answer(true, &mut r.library);
        assert_eq!(r.sent(), [Sent::Write(uid(A), Payload::Steam(504230))]);
        r.written(A, Ok(()));
        assert!(matches!(r.wizard.step, Step::Test { .. }));
    }

    #[test]
    fn a_tag_bound_to_another_game_is_taken_from_it_after_asking() {
        let mut r = rig("bound");
        r.tag(PORTAL_TAG, Payload::Steam(620));
        let Step::Overwrite { question, .. } = &r.wizard.step else {
            panic!("{:?}", r.wizard.step);
        };
        assert!(
            question.contains("\"Portal 2\" (steam://620)"),
            "{question}"
        );
        assert!(question.contains("bound to \"Portal 2\""), "{question}");
        r.wizard.answer(true, &mut r.library);
        r.written(PORTAL_TAG, Ok(()));
        let config = r.config();
        let portal = config.by_id(&Slug::new("portal-2").unwrap()).unwrap();
        assert_eq!(portal.uid, None, "the UID mapping would override the tag");
    }

    #[test]
    fn a_tag_that_already_holds_the_game_is_not_rewritten() {
        let mut r = rig("already");
        r.tag(A, Payload::Steam(504230));
        assert_eq!(
            r.wizard.step,
            Step::Test {
                outcome: Outcome::AlreadyThere { uid: uid(A) }
            }
        );
        assert_eq!(r.sent(), []);
        assert!(r.config().games.iter().any(|g| g.name == "Celeste"));
    }

    #[test]
    fn cancel_at_any_step_gives_the_reader_back_and_saves_nothing() {
        let before = LIBRARY;
        // Tap, overwrite question, writing.
        for stage in 0..3 {
            let mut r = rig(&format!("cancel-{stage}"));
            if stage >= 1 {
                r.tag(A, Payload::Steam(1145360));
            }
            if stage >= 2 {
                r.wizard.answer(true, &mut r.library);
                assert_eq!(r.wizard.step, Step::Writing { uid: uid(A) });
            }
            assert!(!r.released());
            drop(r.wizard);
            assert!(r.link.0.borrow().dropped, "stage {stage}");
            let text = std::fs::read_to_string(r.location.config_file().unwrap()).unwrap();
            assert_eq!(text, before);
        }
    }

    #[test]
    fn reader_lost_mid_wizard_is_shown_and_a_closed_session_stops_waiting() {
        let mut r = rig("lost");
        r.send(Update::Reader { connected: false });
        assert_eq!(r.wizard.reader, Some(false));
        r.send(Update::Reader { connected: true });
        r.tag(A, Payload::Unusable);
        // The run loop went away while writing.
        r.link.0.borrow_mut().closed = true;
        r.wizard.poll(&mut r.library);
        assert_eq!(r.wizard.step, Step::Tap);
        assert_eq!(r.wizard.problem.as_deref(), Some(LINK_CLOSED));
        assert!(!r.wizard.has_reader());
        // Skip still saves.
        r.wizard.skip(&mut r.library);
        assert_eq!(
            r.wizard.step,
            Step::Test {
                outcome: Outcome::Skipped
            }
        );
        assert!(r.config().games.iter().any(|g| g.name == "Celeste"));
    }

    #[test]
    fn a_tag_before_the_run_loop_took_up_the_session_is_ignored() {
        let mut r = rig("unarmed");
        // As if the run loop had not answered yet.
        r.wizard.reader = None;
        assert!(!r.wizard.armed());
        r.tag(A, Payload::Unusable);
        assert_eq!(r.wizard.step, Step::Tap);
        assert_eq!(r.sent(), []);
        r.send(Update::Reader { connected: false });
        assert!(
            r.wizard.armed(),
            "a disconnected reader still means launches are off"
        );
        r.tag(A, Payload::Unusable);
        assert_eq!(r.wizard.step, Step::Writing { uid: uid(A) });
    }

    #[test]
    fn a_binding_added_outside_meanwhile_is_asked_about_again_at_save() {
        let mut r = rig("rebound");
        r.tag(A, Payload::Unusable);
        // Edited by hand while the tag was being written: Hades gets it.
        let path = r.location.config_file().unwrap();
        let text = LIBRARY.replace(
            "name = \"Hades\"\n",
            "name = \"Hades\"\nuid = \"04:A1:B2:C3:D4:E5:F6\"\n",
        );
        std::fs::write(&path, text).unwrap();
        r.written(A, Ok(()));
        let Step::Overwrite { question, .. } = &r.wizard.step else {
            panic!("{:?}", r.wizard.step);
        };
        assert!(question.contains("now bound to \"Hades\""), "{question}");
        assert!(
            r.config().by_uid(uid(A).as_bytes()).is_some(),
            "nothing saved yet"
        );
        r.wizard.answer(true, &mut r.library);
        assert_eq!(
            r.wizard.step,
            Step::Test {
                outcome: Outcome::Written { uid: uid(A) }
            }
        );
        let config = r.config();
        assert_eq!(config.by_uid(uid(A).as_bytes()), None);
        assert!(config.games.iter().any(|g| g.name == "Celeste"));

        // "No" saves nothing and waits for another tag.
        let mut r = rig("rebound-no");
        r.tag(A, Payload::Unusable);
        std::fs::write(
            r.location.config_file().unwrap(),
            LIBRARY.replace(
                "name = \"Hades\"\n",
                "name = \"Hades\"\nuid = \"04:A1:B2:C3:D4:E5:F6\"\n",
            ),
        )
        .unwrap();
        r.written(A, Ok(()));
        r.sent();
        r.wizard.answer(false, &mut r.library);
        assert_eq!(r.wizard.step, Step::Tap);
        assert!(r
            .wizard
            .problem
            .as_deref()
            .unwrap()
            .starts_with("Not saved."));
        assert_eq!(r.sent(), [Sent::Watch(Some(uid(A)))]);
        assert!(!r.config().games.iter().any(|g| g.name == "Celeste"));
    }

    #[test]
    fn a_failed_save_can_be_retried_and_the_program_form_goes_back() {
        let mut r = rig("retry-save");
        r.tag(A, Payload::Unusable);
        // The folder is gone when saving.
        let dir = r.location.config_file().unwrap();
        let dir = dir.parent().unwrap().to_owned();
        std::fs::remove_dir_all(&dir).unwrap();
        let blocked = dir.clone();
        std::fs::write(&blocked, "a file where the folder was").unwrap();
        r.written(A, Ok(()));
        assert!(r
            .wizard
            .problem
            .as_deref()
            .unwrap()
            .starts_with("Not saved"));
        assert!(!r.wizard.take_saved());
        std::fs::remove_file(&blocked).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.toml"), LIBRARY).unwrap();
        r.wizard.retry_save(&mut r.library);
        assert_eq!(r.wizard.problem, None);
        assert!(r.wizard.take_saved());
        assert!(r.config().games.iter().any(|g| g.name == "Celeste"));

        let config = r.library.config().unwrap().clone();
        let mut w = Wizard::new(Ok(steam_games()));
        let mut form = Form::new();
        w.program(&mut form, &config);
        assert!(!w.program_next(&mut form, &config));
        w.back(&mut form);
        assert_eq!(w.step, Step::Pick);
        assert!(form.errors.is_empty() && w.slug_error.is_none());
    }

    #[test]
    fn stale_updates_are_ignored() {
        let mut r = rig("stale");
        r.written(A, Ok(()));
        assert_eq!(r.wizard.step, Step::Tap);
        r.tag(A, Payload::Unusable);
        r.tag("04d4e5f6", Payload::Unusable);
        r.written("04d4e5f6", Ok(()));
        assert_eq!(r.wizard.step, Step::Writing { uid: uid(A) });
    }

    #[test]
    fn program_form_makes_an_id_payload_with_an_editable_slug() {
        let r = rig("program");
        let config = r.library.config().unwrap().clone();
        let mut w = Wizard::new(Err("Steam is not installed".into()));
        assert!(w.filtered().is_empty());
        let mut form = Form::new();
        w.program(&mut form, &config);
        assert_eq!(w.step, Step::Program);
        assert!(!form.steam);
        assert!(!w.program_next(&mut form, &config));
        assert!(form.error(Field::Name).is_some() && form.error(Field::Exe).is_some());
        assert!(w.slug_error.is_some());
        form.name = "Hades".into();
        w.follow_name(&form, &config);
        assert_eq!(w.slug, "hades-2");
        form.exe = "C:\\Games\\hades.exe".into();
        w.slug = "hades".into();
        w.auto_slug = false;
        assert!(!w.program_next(&mut form, &config));
        assert_eq!(
            w.slug_error.as_deref(),
            Some("Another game already uses this ID.")
        );
        w.slug = "Hades!".into();
        assert!(!w.program_next(&mut form, &config));
        w.slug = "my-hades".into();
        w.follow_name(&form, &config);
        assert_eq!(
            w.slug, "my-hades",
            "an edited id no longer follows the name"
        );
        assert!(w.program_next(&mut form, &config));
        assert_eq!(
            w.payload(),
            Some(Payload::Id(Slug::new("my-hades").unwrap()))
        );
        // No reader in this host: only Skip.
        w.begin(None);
        assert_eq!(w.problem.as_deref(), Some(NO_READER));
    }

    const EDEN: &str = "[[emulators]]\nid = \"eden\"\nname = \"Eden\"\nexe = 'C:\\Eden\\eden.exe'\nargs = [\"-g\", \"{rom}\"]\n\n[[games]]\nid = \"tile-tumble\"\nname = \"Tile Tumble\"\nemulator = \"eden\"\nrom = '/roms/tile-tumble.nsp'\n\n";

    fn eden_config() -> Config {
        Config::parse(&format!("{EDEN}{LIBRARY}")).unwrap()
    }

    #[test]
    fn emulator_game_from_a_scanned_folder_gets_a_unique_suggested_id() {
        let config = eden_config();
        let dir =
            std::env::temp_dir().join(format!("mini-launcher-wizard-roms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("more")).unwrap();
        for f in [
            "Tile Tumble [0100AB][v0].nsp",
            "more/Sky Drifter (USA).xci",
            "notes.txt",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        let mut w = Wizard::new(Ok(steam_games()));
        w.emulator_game(&config);
        assert_eq!(w.step, Step::Rom);
        assert_eq!(w.emu.emulator.as_ref().map(Slug::as_str), Some("eden"));
        assert!(!w.rom_next(&config));
        assert_eq!(w.emu.rom_error.as_deref(), Some("Choose the game file."));
        w.scan(dir.clone(), &config);
        let found = w.emu.scan.clone().unwrap().1;
        let names: Vec<&str> = found.roms.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            (names, found.truncated),
            (vec!["Sky Drifter", "Tile Tumble"], false)
        );
        w.emu.rom = found.roms[1].path.display().to_string();
        assert!(!w.rom_next(&config));
        assert_eq!(w.step, Step::Name);
        assert_eq!(w.emu.name, "Tile Tumble");
        assert_eq!(w.slug, "tile-tumble-2", "the suggestion, made unique");
        // An edited name is kept across Back and Next; the id follows it.
        w.emu.name = "Tile Tumble Deluxe".into();
        w.follow_rom_name(&config);
        w.back(&mut Form::new());
        assert_eq!(w.step, Step::Rom);
        assert!(!w.rom_next(&config));
        assert_eq!(
            (w.emu.name.as_str(), w.slug.as_str()),
            ("Tile Tumble Deluxe", "tile-tumble-deluxe")
        );
        assert!(w.name_next(&config));
        let game = w.game.clone().unwrap();
        let LaunchPlan::Emulator {
            emulator,
            rom,
            args,
            ..
        } = &game.plan
        else {
            panic!("{game:?}");
        };
        assert_eq!((emulator.as_str(), rom), ("eden", &found.roms[1].path));
        assert_eq!(args, &["-g".to_owned(), rom.display().to_string()]);
        assert_eq!(
            w.payload(),
            Some(Payload::Id(Slug::new("tile-tumble-deluxe").unwrap()))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emulator_game_ids_are_required_when_nothing_can_be_suggested() {
        let config = eden_config();
        let mut w = Wizard::new(Ok(Vec::new()));
        w.emulator_game(&config);
        w.emu.rom = "\"D:\\Switch\\ゼルダ.nsp\"".into();
        assert!(!w.rom_next(&config));
        assert_eq!((w.emu.name.as_str(), w.slug.as_str()), ("ゼルダ", ""));
        w.emu.name.clear();
        assert!(!w.name_next(&config));
        assert!(w.emu.name_error.is_some() && w.slug_error.is_some());
        w.emu.name = "ゼルダ".into();
        (w.slug, w.auto_slug) = ("tile-tumble".into(), false);
        assert!(!w.name_next(&config));
        assert_eq!(
            w.slug_error.as_deref(),
            Some("Another game already uses this ID.")
        );
        w.slug = "my-switch-game".into();
        assert!(w.name_next(&config));
        let LaunchPlan::Emulator { rom, .. } = &w.game.unwrap().plan else {
            panic!();
        };
        assert_eq!(rom, Path::new("D:\\Switch\\ゼルダ.nsp"), "quotes trimmed");
    }

    #[test]
    fn a_game_file_already_in_the_library_reuses_its_entry() {
        let config = eden_config();
        let mut w = Wizard::new(Ok(Vec::new()));
        w.emulator_game(&config);
        w.emu.rom = "/roms/tile-tumble.nsp".into();
        assert!(w.rom_next(&config), "straight to the tag");
        assert_eq!(w.game.unwrap().id.as_str(), "tile-tumble");
        // No emulator set up: Next says so.
        let empty = Config::parse(LIBRARY).unwrap();
        let mut w = Wizard::new(Ok(Vec::new()));
        w.emulator_game(&empty);
        w.emu.rom = "/roms/x.nsp".into();
        assert!(!w.rom_next(&empty));
        assert!(w.emu.rom_error.as_deref().unwrap().contains("add Eden"));
        w.back(&mut Form::new());
        assert_eq!(w.step, Step::Pick);
    }

    #[test]
    fn presets_are_matched_by_name_or_exe() {
        let config = eden_config();
        let mut eden = config.emulators[0].clone();
        assert_eq!(preset_of(&eden).map(|p| p.name), Some("Eden"));
        eden.name = "Nightly".into();
        eden.exe = PathBuf::from("eden.exe");
        assert!(preset_of(&eden).is_some());
        eden.exe = PathBuf::from("other.exe");
        assert_eq!(preset_of(&eden), None);
    }

    #[test]
    fn picking_a_game_already_in_the_library_reuses_its_entry() {
        let r = rig("existing");
        let config = r.library.config().unwrap();
        let mut w = Wizard::new(Ok(steam_games()));
        w.selected = Some(620);
        assert!(w.pick_steam(config));
        let game = w.game.unwrap();
        assert_eq!(game.id.as_str(), "portal-2");
        assert!(game.uid.is_some());
        let mut w = Wizard::new(Ok(steam_games()));
        assert!(!w.pick_steam(config), "nothing selected");
        w.search = "620".into();
        assert_eq!(w.filtered().len(), 1);
    }
}
