//! The settings window's screens: the library list and the edit form, on
//! the real config (see [`crate::library`]). Plain egui, so they are
//! tested headlessly; the native window is in `window`.

pub mod backup;
pub mod emulators;

use std::path::PathBuf;
use std::sync::Arc;

use egui::{pos2, vec2, Align, Layout, Rect, Sense, Ui};
use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_core::config::{format_uid, Config};
use mini_launcher_core::emulators::EmulatorPreset;
use mini_launcher_core::launch::LaunchPlan;
use mini_launcher_core::payload::Slug;
use mini_launcher_core::steam::SteamGame;

use crate::library::{Field, Form, Library};
use crate::theme::{self, BIG, BLUE, FACE, INK, PAPER, ROW, SHADE, SIGNAL, TEXT, U};
use crate::wizard::{Link, Wizard};

/// What the window needs from the app hosting it (the tray).
pub trait Host: Send + Sync {
    /// Where the config lives now.
    fn location(&self) -> Option<ConfigLocation>;
    /// The live reader state for the status bar.
    fn reader(&self) -> ReaderStatus;
    /// The window saved `config.toml`: load the new library now.
    fn saved(&self);
    /// Borrows the reader from the run loop (launches stop until the
    /// session is dropped); `None` if this host has no reader.
    fn tag_session(&self) -> Option<Box<dyn Link>>;
    /// The installed Steam games, or why they can't be listed.
    fn steam_games(&self) -> Result<Vec<SteamGame>, String>;
    /// Starts `plan` now (the wizard's "Launch now").
    fn launch(&self, plan: &LaunchPlan) -> Result<(), String>;
    /// The default config folder, where `location.toml` lives.
    fn default_dir(&self) -> Option<PathBuf> {
        Config::default_dir()
    }
    /// The window changed `location.toml`: switch to the new folder now
    /// (so [`Host::location`] returns it from this call on).
    fn moved(&self) {
        self.saved();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaderStatus {
    pub text: String,
    pub ready: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Library,
    Edit,
    /// The add-game wizard (see [`crate::wizard`]).
    Wizard,
    /// The `[[emulators]]` list, and its edit form.
    Emulators,
    EditEmulator,
}

/// Which path a Browse button fills in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    Exe,
    Cwd,
    /// The emulator form's exe.
    EmulatorExe,
    /// An emulator game's ROM file.
    Rom,
    /// The wizard's emulator game file, and a folder of them to scan.
    GameFile,
    GameFolder,
}

pub struct Settings {
    host: Arc<dyn Host>,
    pub screen: Screen,
    /// Index into the library's games.
    pub selected: Option<usize>,
    pub library: Library,
    pub form: Form,
    /// The game a Remove is waiting for confirmation on (by id, so an
    /// outside edit that reorders the file can't swap it for another).
    pub confirm_remove: Option<Slug>,
    /// Why the last change on the library screen failed.
    pub problem: Option<String>,
    /// The add-game wizard, while it is open. Dropping it gives the reader
    /// back to the run loop.
    pub wizard: Option<Wizard>,
    pub(crate) browse: Option<Pick>,
    /// Export / import / config folder dialogs.
    pub transfer: backup::Transfer,
    pub emulators: emulators::Emulators,
}

impl Settings {
    pub fn new(host: Arc<dyn Host>) -> Self {
        let library = Library::load(host.location());
        let selected = (!library.games().is_empty()).then_some(0);
        Settings {
            host,
            screen: Screen::Library,
            selected,
            library,
            form: Form::new(),
            confirm_remove: None,
            problem: None,
            wizard: None,
            browse: None,
            transfer: backup::Transfer::default(),
            emulators: emulators::Emulators::default(),
        }
    }

    /// Opens the form on game `i`.
    pub fn edit(&mut self, i: usize) {
        if let Some(game) = self.library.games().get(i) {
            self.selected = Some(i);
            self.form = Form::from_game(game);
            self.screen = Screen::Edit;
        }
    }

    /// Opens the add-game wizard on its pick step.
    pub fn add(&mut self) {
        self.form = Form::new();
        self.wizard = Some(Wizard::new(self.host.steam_games()));
        self.screen = Screen::Wizard;
    }

    /// The new-cart notification was clicked: opens the add-game wizard,
    /// unless that would throw something away (a wizard already running,
    /// or an edit form), in which case the window is only focused.
    pub fn from_tray_add(&mut self) {
        if self.wizard.is_none() && matches!(self.screen, Screen::Library | Screen::Emulators) {
            self.add();
        }
    }

    /// The edit form's "Write tag...": validates the form, then opens the
    /// wizard on its tap step for that game.
    pub fn write_tag(&mut self) {
        let Some(config) = self.library.config() else {
            return;
        };
        self.form.problem = None;
        match self.form.to_game(config) {
            Ok(game) => {
                self.form.errors.clear();
                let mut wizard = Wizard::for_game(game);
                wizard.begin(self.host.tag_session());
                self.wizard = Some(wizard);
                self.screen = Screen::Wizard;
            }
            Err(errors) => self.form.errors = errors,
        }
    }

    /// The wizard's Next on the pick and program steps.
    pub fn wizard_next(&mut self) {
        let (Some(w), Some(config)) = (&mut self.wizard, self.library.config()) else {
            return;
        };
        let next = match w.step {
            crate::wizard::Step::Pick => w.pick_steam(config),
            crate::wizard::Step::Program => w.program_next(&mut self.form, config),
            crate::wizard::Step::Rom => w.rom_next(config),
            crate::wizard::Step::Name => w.name_next(config),
            _ => false,
        };
        if next {
            w.begin(self.host.tag_session());
        }
    }

    /// The wizard's "Program...": the program form.
    pub fn wizard_program(&mut self) {
        if let (Some(w), Some(config)) = (&mut self.wizard, self.library.config()) {
            w.program(&mut self.form, config);
        }
    }

    /// The wizard's "Emulator game...".
    pub fn wizard_emulator(&mut self) {
        if let (Some(w), Some(config)) = (&mut self.wizard, self.library.config()) {
            w.emulator_game(config);
        }
    }

    /// The wizard's "Add Eden..." and the like: the emulator form, then
    /// back to the wizard (see [`Settings::save_emulator`]).
    pub fn wizard_add_emulator(&mut self, preset: &'static EmulatorPreset) {
        self.add_emulator(preset);
        self.emulators.for_wizard = true;
    }

    /// The file types a game file Browse offers: those of the emulator
    /// chosen in the wizard, or in the edit form.
    pub fn game_file_types(&self) -> &'static [&'static str] {
        let id = match (&self.wizard, self.screen) {
            (Some(w), Screen::Wizard) => w.emu.emulator.as_ref(),
            _ => self.form.emulator.as_ref(),
        };
        let emulator = self.library.emulators().iter().find(|e| Some(&e.id) == id);
        let preset = emulator.and_then(crate::wizard::preset_of);
        preset.map_or(&[], |p| p.rom_extensions)
    }

    /// Closes the wizard (giving the reader back): to the edit form if it
    /// came from there and nothing was saved, else to the library.
    pub fn wizard_close(&mut self) {
        let Some(w) = self.wizard.take() else {
            return;
        };
        let done = matches!(w.step, crate::wizard::Step::Test { .. });
        if let (true, Some(game)) = (done, &w.game) {
            self.selected = self.library.games().iter().position(|g| g.id == game.id);
        }
        self.screen = if w.from_edit && !done {
            Screen::Edit
        } else {
            Screen::Library
        };
    }

    /// The test step's "Launch now".
    pub fn launch_now(&mut self) {
        if let Some(w) = &mut self.wizard {
            if let Some(game) = &w.game {
                w.launched = Some(self.host.launch(&game.plan));
            }
        }
    }

    /// Lets the wizard handle what the run loop sent.
    fn poll_wizard(&mut self) {
        if let Some(w) = &mut self.wizard {
            w.poll_scan();
            w.poll(&mut self.library);
            if w.take_saved() {
                self.host.saved();
            }
        }
    }

    /// Validates and saves the form; back to the library on success.
    pub fn save(&mut self) {
        let Some(config) = self.library.config() else {
            return;
        };
        self.form.problem = None;
        let game = match self.form.to_game(config) {
            Ok(game) => game,
            Err(errors) => {
                self.form.errors = errors;
                return;
            }
        };
        self.form.errors.clear();
        match self.library.save_game(&game) {
            Ok(()) => {
                self.host.saved();
                self.selected = self.library.games().iter().position(|g| g.id == game.id);
                self.screen = Screen::Library;
            }
            Err(e) => self.form.problem = Some(format!("Not saved: {e}")),
        }
    }

    /// Asks to remove game `i`.
    pub fn ask_remove(&mut self, i: usize) {
        self.confirm_remove = self.library.games().get(i).map(|g| g.id.clone());
    }

    /// Removes the game with this id (after the user confirmed).
    pub fn remove(&mut self, id: &Slug) {
        let Some(i) = self.library.games().iter().position(|g| &g.id == id) else {
            self.problem = Some(format!(
                "Not removed: \"{id}\" is no longer in the library (changed outside the app)."
            ));
            return;
        };
        match self.library.remove(id) {
            Ok(()) => {
                self.host.saved();
                self.problem = None;
                let n = self.library.games().len();
                self.selected = (n > 0).then(|| i.min(n - 1));
            }
            Err(e) => self.problem = Some(format!("Not removed: {e}")),
        }
    }

    /// A Browse button was clicked: which field, and the folder to start in.
    pub fn take_browse(&mut self) -> Option<(Pick, Option<PathBuf>)> {
        let pick = self.browse.take()?;
        let current = match pick {
            Pick::Exe => PathBuf::from(&self.form.exe).parent().map(PathBuf::from),
            Pick::Cwd => Some(PathBuf::from(&self.form.cwd)),
            Pick::EmulatorExe => PathBuf::from(&self.emulators.form.exe)
                .parent()
                .map(PathBuf::from),
            Pick::Rom => PathBuf::from(&self.form.rom).parent().map(PathBuf::from),
            Pick::GameFile | Pick::GameFolder => self.wizard.as_ref().and_then(|w| {
                let last = w.emu.scan.as_ref().map(|(dir, _)| dir.as_path());
                let scanned = w.scanning().or(last).map(PathBuf::from);
                let rom = PathBuf::from(&w.emu.rom).parent().map(PathBuf::from);
                rom.filter(|_| pick == Pick::GameFile).or(scanned)
            }),
        };
        Some((pick, current.filter(|p| p.is_dir())))
    }

    /// Fills in the path chosen in the file dialog.
    pub fn picked(&mut self, pick: Pick, path: PathBuf) {
        let text = path.display().to_string();
        match pick {
            Pick::Exe => {
                if self.form.cwd.trim().is_empty() {
                    if let Some(dir) = path.parent() {
                        self.form.cwd = dir.display().to_string();
                    }
                }
                self.form.exe = text;
            }
            Pick::Cwd => self.form.cwd = text,
            Pick::EmulatorExe => self.emulators.form.exe = text,
            Pick::Rom => self.form.rom = text,
            Pick::GameFile | Pick::GameFolder => {
                let (Some(w), Some(config)) = (&mut self.wizard, self.library.config()) else {
                    return;
                };
                match pick {
                    Pick::GameFile => w.emu.rom = text,
                    _ => w.scan(path, config),
                }
            }
        }
    }

    /// Reloads the library if the config file or its folder changed.
    fn refresh(&mut self) {
        let location = self.host.location();
        if self.library.is_stale(location.as_ref()) {
            self.library = Library::load(location);
            let n = self.library.games().len();
            self.selected = self.selected.filter(|&i| i < n).or((n > 0).then_some(0));
            let (m, e) = (self.library.emulators().len(), &mut self.emulators);
            e.selected = e.selected.filter(|&i| i < m).or((m > 0).then_some(0));
        }
    }
}

/// The whole window: masthead, screen, status bar.
pub fn show(ui: &mut Ui, s: &mut Settings) {
    if matches!(s.screen, Screen::Library | Screen::Emulators) {
        s.refresh();
    }
    if s.screen == Screen::Wizard {
        s.poll_wizard();
        // Updates from the run loop and the folder scan don't wake the
        // window: look again soon.
        let waiting = |w: &Wizard| w.has_reader() || w.scanning().is_some();
        if s.wizard.as_ref().is_some_and(waiting) {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    let title = match (s.screen, &s.form.id) {
        (Screen::Library, _) => "LIBRARY",
        (Screen::Edit, Some(_)) => "EDIT GAME",
        (Screen::Edit, None) => "ADD GAME",
        (Screen::Wizard, _) if s.wizard.as_ref().is_some_and(|w| w.from_edit) => "WRITE TAG",
        (Screen::Wizard, _) => "ADD GAME",
        (Screen::Emulators, _) => "EMULATORS",
        (Screen::EditEmulator, _) if s.emulators.form.id.is_some() => "EDIT EMULATOR",
        (Screen::EditEmulator, _) => "ADD EMULATOR",
    };
    egui::Panel::top("masthead")
        .frame(egui::Frame::NONE.fill(BLUE))
        .exact_size(10.0 * U)
        .show_separator_line(false)
        .show(ui, |ui| masthead(ui, title));
    egui::Panel::bottom("status")
        .frame(
            egui::Frame::NONE
                .fill(FACE)
                .inner_margin(egui::Margin::same(U as i8)),
        )
        .show_separator_line(false)
        .show(ui, |ui| status_bar(ui, s));
    egui::CentralPanel::no_frame()
        .frame(
            egui::Frame::NONE
                .fill(FACE)
                .inner_margin(egui::Margin::same((3.0 * U) as i8)),
        )
        .show(ui, |ui| match s.screen {
            Screen::Library => library(ui, s),
            Screen::Edit => edit(ui, s),
            Screen::Wizard => crate::wizard_view::show(ui, s),
            Screen::Emulators => emulators::list(ui, s),
            Screen::EditEmulator => emulators::edit(ui, s),
        });
}

fn masthead(ui: &mut Ui, title: &str) {
    let r = theme::snap(ui.max_rect());
    let p = ui.painter();
    // The 16 px app icon (app/assets/icon/build.py), white on the band:
    // o = disc, k = wave, . = see-through.
    const MARK: [&str; 16] = [
        ".....oooooo.....",
        "...oooooooooo...",
        "..oooooooooooo..",
        ".oookooooookooo.",
        ".ookkooooookkoo.",
        "ookkookookookkoo",
        "ookkokooookokkoo",
        "ookkoko..okokkoo",
        "ookkoko..okokkoo",
        "ookkokooookokkoo",
        "ookkookookookkoo",
        ".ookkooooookkoo.",
        ".oookooooookooo.",
        "..oooooooooooo..",
        "...oooooooooo...",
        ".....oooooo.....",
    ];
    let origin = pos2(r.min.x + 3.0 * U, r.center().y - 16.0).round();
    for (y, row) in MARK.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            let color = match c {
                b'o' => PAPER,
                b'k' => INK,
                _ => continue,
            };
            let at = origin + vec2(x as f32 * 2.0, y as f32 * 2.0);
            p.rect_filled(Rect::from_min_size(at, vec2(2.0, 2.0)), 0.0, color);
        }
    }
    let name = theme::galley(ui, "MINI LAUNCHER", BIG, PAPER);
    let x = origin.x + 32.0 + 3.0 * U;
    let name_w = name.size().x;
    p.galley(
        pos2(x, r.center().y - name.size().y / 2.0).round(),
        name,
        PAPER,
    );
    let screen = theme::galley(ui, title, TEXT, INK);
    let tab = Rect::from_min_size(
        pos2(x + name_w + 4.0 * U, r.center().y - ROW / 2.0),
        vec2(screen.size().x + 4.0 * U, ROW),
    );
    let tab = theme::snap(tab);
    p.rect_filled(tab, 0.0, SIGNAL);
    theme::outline(p, tab, INK);
    p.galley((tab.center() - screen.size() / 2.0).round(), screen, INK);
    // A hard black rule under the masthead.
    p.rect_filled(
        Rect::from_min_max(pos2(r.min.x, r.max.y - 1.0), r.max),
        0.0,
        INK,
    );
}

/// Bottom-right corner of every screen.
const CREDIT: &str = "<3 plomdawg";

fn status_bar(ui: &mut Ui, s: &Settings) {
    let games = s.library.games();
    let bound = games.iter().filter(|g| g.uid.is_some()).count();
    let reader = s.host.reader();
    let cells = [
        format!("{} games", games.len()),
        format!("{bound} tagged"),
        reader.text,
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = U;
        for (i, cell) in cells.iter().enumerate() {
            let last = i + 1 == cells.len();
            let width = if last { ui.available_width() } else { 24.0 * U };
            let (rect, _) = ui.allocate_exact_size(vec2(width, ROW - U), Sense::hover());
            let rect = theme::snap(rect);
            theme::bevel(ui.painter(), rect, SHADE, PAPER);
            let mut x = rect.min.x + 2.0 * U;
            if last {
                x = theme::mark(ui.painter(), rect, x, reader.ready, INK);
            }
            theme::label_at(ui, rect, x, cell, INK);
            if last {
                let credit = theme::galley(ui, CREDIT, TEXT, SHADE);
                let x = rect.max.x - 2.0 * U - credit.size().x;
                theme::label_at(ui, rect, x, CREDIT, SHADE);
            }
        }
    });
}

fn library(ui: &mut Ui, s: &mut Settings) {
    let editable = s.library.problem().is_none();
    let has = editable && s.selected.is_some();
    ui.horizontal(|ui| {
        if theme::button(ui, "Add game", editable, false).clicked() {
            s.add();
        }
        if theme::button(ui, "Edit", has, false).clicked() {
            s.edit(s.selected.unwrap_or_default());
        }
        if theme::button(ui, "Remove", has, false).clicked() {
            s.ask_remove(s.selected.unwrap_or_default());
        }
        if theme::button(ui, "Emulators", editable, false).clicked() {
            s.open_emulators();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            backup::toolbar(ui, s);
        });
    });
    ui.add_space(U);
    backup::dialogs(ui, s);
    if let Some(problem) = s.library.problem() {
        let problem = format!("The library can't be shown: {problem}");
        theme::warning(ui, 0.0, &problem);
        return;
    }
    if let Some(problem) = &s.problem {
        theme::warning(ui, 0.0, problem);
    }
    let games = s.library.games();
    let mut clicked = None;
    let mut open = None;
    let body = theme::list(
        ui,
        &[0.0, 0.40, 0.66],
        &["NAME", "LAUNCH", "TAG"],
        games.len(),
        |ui, i, row, cols| {
            let game = &games[i];
            let on = s.selected == Some(i);
            let (resp, fg, weak) = theme::list_row(ui, row, ("game", i), on, &game.name);
            if resp.clicked() {
                clicked = Some(i);
            }
            if resp.double_clicked() {
                open = Some(i);
            }
            theme::label_at(ui, row, cols[0] + 2.0 * U, &game.name, fg);
            theme::label_at(ui, row, cols[1] + 2.0 * U, &launch_label(&game.plan), fg);
            let uid = game.uid.as_deref().map(format_uid);
            let x = theme::mark(ui.painter(), row, cols[2] + 2.0 * U, uid.is_some(), fg);
            let color = if uid.is_some() { fg } else { weak };
            theme::label_at(ui, row, x, uid.as_deref().unwrap_or("not bound"), color);
        },
    );
    if games.is_empty() {
        let at = pos2(body.min.x + 2.0 * U, body.min.y + ROW / 2.0);
        let row = Rect::from_center_size(at, vec2(0.0, ROW));
        theme::label_at(ui, row, at.x, "No games yet: click Add game.", SHADE);
    }
    if let Some(id) = s.confirm_remove.clone() {
        let game = s.library.config().and_then(|c| c.by_id(&id));
        let name = game.map_or_else(|| id.to_string(), |g| g.name.clone());
        let body = format!(
            "Remove \"{name}\" from the library? Its tag stops launching it. \
             The previous config is kept as config.toml.bak."
        );
        match theme::confirm(ui.ctx(), "Remove game", &body, "Remove", "Cancel") {
            Some(true) => {
                s.confirm_remove = None;
                s.remove(&id);
            }
            Some(false) => s.confirm_remove = None,
            None => {}
        }
    }
    if let Some(i) = clicked.filter(|&i| s.selected != Some(i)) {
        // Another game: what went wrong with the last one is old news.
        s.problem = None;
        s.selected = Some(i);
    }
    if let Some(i) = open {
        s.edit(i);
    }
}

/// The library's LAUNCH column.
pub fn launch_label(plan: &LaunchPlan) -> String {
    match plan {
        LaunchPlan::Steam { appid } => format!("Steam {appid}"),
        LaunchPlan::Exe { .. } => "Program".to_owned(),
        LaunchPlan::Emulator { .. } => "Emulator".to_owned(),
    }
}

fn edit(ui: &mut Ui, s: &mut Settings) {
    let problem = s.form.problem.clone();
    let (save, cancel) = form_screen(ui, problem.as_deref(), |ui| form(ui, s));
    if cancel {
        s.screen = Screen::Library;
    } else if save {
        s.save();
    }
}

/// A form screen: `form` scrolls above the problem line and Save / Cancel.
/// Returns (save, cancel) clicked.
fn form_screen(ui: &mut Ui, problem: Option<&str>, form: impl FnOnce(&mut Ui)) -> (bool, bool) {
    // Buttons first, from the bottom up, so inline errors never push them
    // out of the window: the form scrolls instead.
    let (mut save, mut cancel) = (false, false);
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                cancel = theme::button(ui, "Cancel", true, false).clicked();
                save = theme::button(ui, "Save", true, true).clicked();
            });
        });
        if let Some(problem) = problem {
            theme::warning(ui, 0.0, problem);
        }
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, form);
        });
    });
    (save, cancel)
}

pub(crate) const LABEL_W: f32 = 22.0 * U;

/// A form row: a label, then what `add` adds (given the label's id).
pub(crate) fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui, egui::Id)) {
    ui.horizontal(|ui| {
        let l = theme::form_label(ui, LABEL_W, label);
        add(ui, l.id);
    });
}

/// The error for `field`, if any, under its row.
fn note(ui: &mut Ui, errors: &[(Field, String)], field: Field) {
    if let Some((_, e)) = errors.iter().find(|(f, _)| *f == field) {
        theme::warning(ui, LABEL_W + ui.spacing().item_spacing.x, e);
    }
}

/// A field's width, leaving room for a Browse button if it has one.
pub(crate) fn field_w(ui: &Ui, button: bool) -> f32 {
    (ui.available_width() - if button { 20.0 * U } else { 0.0 }).floor()
}

fn form(ui: &mut Ui, s: &mut Settings) {
    let indent = LABEL_W + ui.spacing().item_spacing.x;
    let emulators = s.library.emulators();
    let f = &mut s.form;
    let mut browse = None;
    let mut write = false;
    theme::group(ui, "Game", |ui| {
        let errors = f.errors.clone();
        let note = |ui: &mut Ui, field| note(ui, &errors, field);
        row(ui, "Name", |ui, id| {
            theme::field(ui, &mut f.name, field_w(ui, false), true, id);
        });
        note(ui, Field::Name);
        let emulator = f.emulator.is_some();
        row(ui, "Launch with", |ui, _| {
            if theme::choice(ui, f.steam && !emulator, "Steam").clicked() {
                (f.steam, f.emulator) = (true, None);
            }
            ui.add_space(2.0 * U);
            if theme::choice(ui, !f.steam && !emulator, "Program").clicked() {
                (f.steam, f.emulator) = (false, None);
            }
            ui.add_space(2.0 * U);
            // Offered once an emulator is set up (on the Emulators screen).
            let first = emulators.first().map(|e| e.id.clone());
            if first.is_some() && theme::choice(ui, emulator, "Emulator").clicked() && !emulator {
                f.emulator = first;
            }
        });
        if let Some(current) = f.emulator.clone() {
            row(ui, "Runs on", |ui, _| {
                for e in emulators {
                    if theme::choice(ui, e.id == current, &e.name).clicked() {
                        f.emulator = Some(e.id.clone());
                    }
                    ui.add_space(2.0 * U);
                }
            });
            row(ui, "Game file", |ui, id| {
                theme::field(ui, &mut f.rom, field_w(ui, true), true, id);
                if theme::button(ui, "Browse", true, false).clicked() {
                    browse = Some(Pick::Rom);
                }
            });
            note(ui, Field::Rom);
            return;
        }
        let steam = f.steam;
        row(ui, "Steam app ID", |ui, id| {
            theme::field(ui, &mut f.app_id, 24.0 * U, steam, id);
        });
        note(ui, Field::AppId);
        row(ui, "Exe path", |ui, id| {
            theme::field(ui, &mut f.exe, field_w(ui, true), !steam, id);
            if theme::button(ui, "Browse", !steam, false).clicked() {
                browse = Some(Pick::Exe);
            }
        });
        note(ui, Field::Exe);
        row(ui, "Arguments", |ui, id| {
            theme::field(ui, &mut f.args, field_w(ui, false), !steam, id);
        });
        note(ui, Field::Args);
        row(ui, "Working dir", |ui, id| {
            theme::field(ui, &mut f.cwd, field_w(ui, true), !steam, id);
            if theme::button(ui, "Browse", !steam, false).clicked() {
                browse = Some(Pick::Cwd);
            }
        });
        note(ui, Field::Cwd);
    });
    ui.add_space(U);
    theme::group(ui, "Tag", |ui| {
        ui.horizontal(|ui| {
            theme::form_label(ui, LABEL_W, "Bound to");
            let (well, _) = ui.allocate_exact_size(vec2(field_w(ui, true), ROW), Sense::hover());
            theme::sunken(ui.painter(), well, FACE);
            let uid = f.uid.as_deref().map(format_uid);
            let x = theme::mark(ui.painter(), well, well.min.x + 2.0 * U, uid.is_some(), INK);
            let (text, color) = match &uid {
                Some(uid) => (uid.as_str(), INK),
                None => ("not bound", SHADE),
            };
            theme::label_at(ui, well, x, text, color);
            if theme::button(ui, "Unbind", uid.is_some(), false).clicked() {
                f.uid = None;
            }
        });
        ui.horizontal(|ui| {
            ui.add_space(indent);
            write = theme::button(ui, "Write tag...", true, false).clicked();
            ui.colored_label(SHADE, "Hold a cart on the reader to write or bind it.");
        });
    });
    if browse.is_some() {
        s.browse = browse;
    }
    if write {
        s.write_tag();
    }
}
