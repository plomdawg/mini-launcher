//! The settings window's screens: the library list and the edit form, on
//! the real config (see [`crate::library`]). Plain egui, so they are
//! tested headlessly; the native window is in `window`.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{pos2, vec2, Align, Layout, Rect, Sense, Ui};
use mini_launcher_core::config::format_uid;
use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_core::launch::LaunchPlan;
use mini_launcher_core::payload::Slug;
use mini_launcher_core::session::SessionClient;

use crate::library::{Field, Form, Library};
use crate::theme::{self, BIG, BLUE, FACE, INK, PAPER, ROW, SHADE, SIGNAL, TEXT, U};

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
    fn tag_session(&self) -> Option<SessionClient>;
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
}

/// Which path a Browse button fills in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pick {
    Exe,
    Cwd,
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
    browse: Option<Pick>,
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
            browse: None,
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

    /// Opens an empty form for a new game.
    pub fn add(&mut self) {
        self.form = Form::new();
        self.screen = Screen::Edit;
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
        }
    }

    /// Reloads the library if the config file or its folder changed.
    fn refresh(&mut self) {
        let location = self.host.location();
        if self.library.is_stale(location.as_ref()) {
            self.library = Library::load(location);
            let n = self.library.games().len();
            self.selected = self.selected.filter(|&i| i < n).or((n > 0).then_some(0));
        }
    }
}

/// The whole window: masthead, screen, status bar.
pub fn show(ui: &mut Ui, s: &mut Settings) {
    if s.screen == Screen::Library {
        s.refresh();
    }
    let title = match (s.screen, &s.form.id) {
        (Screen::Library, _) => "LIBRARY",
        (Screen::Edit, Some(_)) => "EDIT GAME",
        (Screen::Edit, None) => "ADD GAME",
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
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Export and import come with #78.
            theme::button(ui, "Import", false, false);
            theme::button(ui, "Export", false, false);
        });
    });
    ui.add_space(U);
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
            let launch = match &game.plan {
                LaunchPlan::Steam { appid } => format!("Steam {appid}"),
                LaunchPlan::Exe { .. } => "Program".to_owned(),
            };
            theme::label_at(ui, row, cols[0] + 2.0 * U, &game.name, fg);
            theme::label_at(ui, row, cols[1] + 2.0 * U, &launch, fg);
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
    if let Some(i) = clicked {
        s.selected = Some(i);
    }
    if let Some(i) = open {
        s.edit(i);
    }
}

fn edit(ui: &mut Ui, s: &mut Settings) {
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
        if let Some(problem) = &s.form.problem {
            theme::warning(ui, 0.0, problem);
        }
        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| form(ui, s));
        });
    });
    if cancel {
        s.screen = Screen::Library;
    } else if save {
        s.save();
    }
}

fn form(ui: &mut Ui, s: &mut Settings) {
    let label_w = 22.0 * U;
    let indent = label_w + ui.spacing().item_spacing.x;
    let field_w = |ui: &Ui, button: bool| {
        (ui.available_width() - if button { 20.0 * U } else { 0.0 }).floor()
    };
    let f = &mut s.form;
    let mut browse = None;
    theme::group(ui, "Game", |ui| {
        let row = |ui: &mut Ui, label: &str, add: &mut dyn FnMut(&mut Ui, egui::Id)| {
            ui.horizontal(|ui| {
                let l = theme::form_label(ui, label_w, label);
                add(ui, l.id);
            });
        };
        let errors = f.errors.clone();
        let note = |ui: &mut Ui, field| {
            if let Some((_, e)) = errors.iter().find(|(f, _)| *f == field) {
                theme::warning(ui, indent, e);
            }
        };
        row(ui, "Name", &mut |ui, id| {
            theme::field(ui, &mut f.name, field_w(ui, false), true, id);
        });
        note(ui, Field::Name);
        row(ui, "Launch with", &mut |ui, _| {
            if theme::choice(ui, f.steam, "Steam").clicked() {
                f.steam = true;
            }
            ui.add_space(2.0 * U);
            if theme::choice(ui, !f.steam, "Program").clicked() {
                f.steam = false;
            }
        });
        let steam = f.steam;
        row(ui, "Steam app ID", &mut |ui, id| {
            theme::field(ui, &mut f.app_id, 24.0 * U, steam, id);
        });
        note(ui, Field::AppId);
        row(ui, "Exe path", &mut |ui, id| {
            theme::field(ui, &mut f.exe, field_w(ui, true), !steam, id);
            if theme::button(ui, "Browse", !steam, false).clicked() {
                browse = Some(Pick::Exe);
            }
        });
        note(ui, Field::Exe);
        row(ui, "Arguments", &mut |ui, id| {
            theme::field(ui, &mut f.args, field_w(ui, false), !steam, id);
        });
        note(ui, Field::Args);
        row(ui, "Working dir", &mut |ui, id| {
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
            theme::form_label(ui, label_w, "Bound to");
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
            theme::button(ui, "Write tag...", false, false);
            ui.colored_label(SHADE, "Writing tags here comes in a later version.");
        });
    });
    if browse.is_some() {
        s.browse = browse;
    }
}
