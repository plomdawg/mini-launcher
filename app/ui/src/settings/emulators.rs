//! The Emulators screen: the `[[emulators]]` list, adding one from a preset,
//! editing it and removing it (refused while games still use it).

use egui::{pos2, vec2, Align, Layout, Rect, Ui};
use mini_launcher_core::emulators::{EmulatorPreset, PRESETS};
use mini_launcher_core::launch::LaunchPlan;
use mini_launcher_core::payload::Slug;

use super::{field_w, form_screen, note, row, Pick, Screen, Settings};
use crate::library::{EmuForm, Field};
use crate::theme::{self, ROW, SHADE, U};

#[derive(Debug, Default)]
pub struct Emulators {
    /// Index into the library's emulators.
    pub selected: Option<usize>,
    pub form: EmuForm,
    /// The emulator a Remove is waiting for confirmation on.
    pub confirm_remove: Option<Slug>,
    /// Why the last change on the list failed.
    pub problem: Option<String>,
    /// The form was opened from the add-game wizard: Save and Cancel go
    /// back there.
    pub for_wizard: bool,
    /// The preset a new entry in the form comes from.
    pub preset: Option<&'static EmulatorPreset>,
}

impl Settings {
    pub fn open_emulators(&mut self) {
        let n = self.library.emulators().len();
        let e = &mut self.emulators;
        e.selected = e.selected.filter(|&i| i < n).or((n > 0).then_some(0));
        e.problem = None;
        e.for_wizard = false;
        self.screen = Screen::Emulators;
    }

    /// The form for a new entry from `preset`, with its exe if it is
    /// installed.
    pub fn add_emulator(&mut self, preset: &'static EmulatorPreset) {
        self.emulators.form = EmuForm::from_preset(preset, preset.detect_exe());
        self.emulators.preset = Some(preset);
        self.screen = Screen::EditEmulator;
    }

    pub fn edit_emulator(&mut self, i: usize) {
        if let Some(e) = self.library.emulators().get(i) {
            self.emulators.selected = Some(i);
            self.emulators.form = EmuForm::from_emulator(e);
            self.emulators.preset = None;
            self.screen = Screen::EditEmulator;
        }
    }

    /// Validates and saves the emulator form; back to the list on success.
    pub fn save_emulator(&mut self) {
        let (Some(config), form) = (self.library.config(), &mut self.emulators.form) else {
            return;
        };
        form.problem = None;
        let emulator = match form.to_emulator(config) {
            Ok(e) => e,
            Err(errors) => return form.errors = errors,
        };
        form.errors.clear();
        match self.library.save_emulator(&emulator) {
            Ok(()) => {
                self.host.saved();
                let mut at = self.library.emulators().iter();
                self.emulators.selected = at.position(|e| e.id == emulator.id);
                self.screen = Screen::Emulators;
                if let (true, Some(w)) = (self.emulators.for_wizard, &mut self.wizard) {
                    w.choose_emulator(emulator.id);
                    self.screen = Screen::Wizard;
                }
            }
            Err(e) => self.emulators.form.problem = Some(format!("Not saved: {e}")),
        }
    }

    /// Asks to remove emulator `i`, or says which games still use it.
    pub fn ask_remove_emulator(&mut self, i: usize) {
        let Some(id) = self.library.emulators().get(i).map(|e| e.id.clone()) else {
            return;
        };
        self.emulators.problem = self.library.removal_problem(&id);
        if self.emulators.problem.is_none() {
            self.emulators.confirm_remove = Some(id);
        }
    }

    /// Removes the emulator with this id (after the user confirmed).
    pub fn remove_emulator(&mut self, id: &Slug) {
        match self.library.remove_emulator(id) {
            Ok(()) => {
                self.host.saved();
                self.emulators.problem = None;
                let n = self.library.emulators().len();
                let e = &mut self.emulators;
                e.selected = (n > 0).then(|| e.selected.unwrap_or(0).min(n - 1));
            }
            Err(e) => self.emulators.problem = Some(format!("Not removed: {e}")),
        }
    }
}

pub(super) fn list(ui: &mut Ui, s: &mut Settings) {
    let editable = s.library.problem().is_none();
    let selected = s.emulators.selected.filter(|_| editable);
    ui.horizontal(|ui| {
        for p in PRESETS {
            if theme::button(ui, &format!("Add {}", p.name), editable, false).clicked() {
                s.add_emulator(p);
            }
        }
        if theme::button(ui, "Edit", selected.is_some(), false).clicked() {
            s.edit_emulator(selected.unwrap_or_default());
        }
        if theme::button(ui, "Remove", selected.is_some(), false).clicked() {
            s.ask_remove_emulator(selected.unwrap_or_default());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::button(ui, "Back", true, false).clicked() {
                s.screen = Screen::Library;
            }
        });
    });
    ui.add_space(U);
    if let Some(problem) = s.library.problem().or(s.emulators.problem.as_deref()) {
        theme::warning(ui, 0.0, problem);
    }
    let (emulators, games) = (s.library.emulators(), s.library.games());
    let (mut clicked, mut open) = (None, None);
    let body = theme::list(
        ui,
        &[0.0, 0.30, 0.84],
        &["NAME", "PROGRAM", "GAMES"],
        emulators.len(),
        |ui, i, row, cols| {
            let e = &emulators[i];
            let on = s.emulators.selected == Some(i);
            let (resp, fg, _) = theme::list_row(ui, row, ("emulator", i), on, &e.name);
            clicked = clicked.or(resp.clicked().then_some(i));
            open = open.or(resp.double_clicked().then_some(i));
            let used = games.iter().filter(
                |g| matches!(&g.plan, LaunchPlan::Emulator { emulator, .. } if *emulator == e.id),
            );
            theme::label_at(ui, row, cols[0] + 2.0 * U, &e.name, fg);
            let exe = e.exe.display().to_string();
            theme::label_at(ui, row, cols[1] + 2.0 * U, &exe, fg);
            theme::label_at(ui, row, cols[2] + 2.0 * U, &used.count().to_string(), fg);
        },
    );
    if emulators.is_empty() {
        let at = pos2(body.min.x + 2.0 * U, body.min.y + ROW / 2.0);
        let row = Rect::from_center_size(at, vec2(0.0, ROW));
        let text = "No emulators yet: add one above.";
        theme::label_at(ui, row, at.x, text, SHADE);
    }
    if let Some(id) = s.emulators.confirm_remove.clone() {
        let body = format!(
            "Remove the emulator \"{id}\"? The previous config is kept as config.toml.bak."
        );
        match theme::confirm(ui.ctx(), "Remove emulator", &body, "Remove", "Cancel") {
            Some(true) => {
                s.emulators.confirm_remove = None;
                s.remove_emulator(&id);
            }
            Some(false) => s.emulators.confirm_remove = None,
            None => {}
        }
    }
    if let Some(i) = clicked {
        s.emulators.selected = Some(i);
    }
    if let Some(i) = open {
        s.edit_emulator(i);
    }
}

pub(super) fn edit(ui: &mut Ui, s: &mut Settings) {
    let problem = s.emulators.form.problem.clone();
    let mut browse = false;
    let (save, cancel) = form_screen(ui, problem.as_deref(), |ui| {
        let f = &mut s.emulators.form;
        let errors = f.errors.clone();
        theme::group(ui, "Emulator", |ui| {
            row(ui, "Name", |ui, id| {
                theme::field(ui, &mut f.name, field_w(ui, false), true, id);
            });
            note(ui, &errors, Field::Name);
            row(ui, "Exe path", |ui, id| {
                theme::field(ui, &mut f.exe, field_w(ui, true), true, id);
                browse = theme::button(ui, "Browse", true, false).clicked();
            });
            note(ui, &errors, Field::Exe);
            let preset = s.emulators.preset.filter(|_| f.id.is_none());
            if let Some(p) = preset.filter(|_| f.exe.is_empty()) {
                if !errors.iter().any(|e| e.0 == Field::Exe) {
                    let exe = p.exe_names().next().unwrap_or_default();
                    let text = format!("{} was not found: Browse for {exe}.", p.name);
                    row(ui, "", |ui, _| {
                        ui.colored_label(SHADE, text);
                    });
                }
            }
            row(ui, "Arguments", |ui, id| {
                theme::field(ui, &mut f.args, field_w(ui, false), true, id);
            });
            note(ui, &errors, Field::Args);
            row(ui, "", |ui, _| {
                ui.colored_label(SHADE, "{rom} stands for the game's file.");
            });
        });
    });
    if browse {
        s.browse = Some(Pick::EmulatorExe);
    }
    if cancel {
        s.screen = match s.emulators.for_wizard {
            true => Screen::Wizard,
            false => Screen::Emulators,
        };
    } else if save {
        s.save_emulator();
    }
}
