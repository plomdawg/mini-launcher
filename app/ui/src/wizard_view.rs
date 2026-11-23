//! The add-game wizard's screens, in the settings window's retro theme.
//! The logic is in [`crate::wizard`].

use std::path::Path;

use egui::{pos2, vec2, Align, Layout, Rect, Sense, Ui};
use mini_launcher_core::emulators::{EmulatorPreset, PRESETS};

use crate::library::Field;
use crate::settings::{field_w, row, Pick, Settings, LABEL_W};
use crate::theme::{self, BIG, INK, PAPER, ROW, SHADE, SIGNAL, U};
use crate::wizard::{self, Outcome, Step, Wizard};

const STEPS: [&str; 4] = ["1 PICK", "2 TAG", "3 WRITE", "4 TEST"];

/// What the user clicked this frame.
#[derive(Default)]
struct Clicks {
    next: bool,
    program: bool,
    emulator: bool,
    add: Option<&'static EmulatorPreset>,
    cancel: bool,
    skip: bool,
    launch: bool,
    back: bool,
    save_again: bool,
    answer: Option<bool>,
}

pub fn show(ui: &mut Ui, s: &mut Settings) {
    let Some(step) = s.wizard.as_ref().map(|w| w.step.clone()) else {
        return;
    };
    let mut c = Clicks::default();
    step_bar(ui, step.number());
    ui.add_space(2.0 * U);
    // Buttons first, from the bottom up, so the body never pushes them out.
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.horizontal(|ui| buttons(ui, &step, s, &mut c));
        ui.with_layout(Layout::top_down(Align::Min), |ui| match &step {
            Step::Pick => pick(ui, s, &mut c),
            Step::Program => program(ui, s),
            Step::Rom => rom(ui, s, &mut c),
            Step::Name => name(ui, s),
            Step::Tap | Step::Overwrite { .. } | Step::Writing { .. } => tap(ui, s),
            Step::Test { outcome } => test(ui, s, outcome),
        });
    });
    if let Step::Overwrite { question, .. } = &step {
        c.answer = theme::confirm(
            ui.ctx(),
            "Tag in use",
            question,
            "Use this tag",
            "Another tag",
        );
    }
    act(s, c);
}

fn act(s: &mut Settings, c: Clicks) {
    if c.cancel {
        return s.wizard_close();
    }
    if c.program {
        s.wizard_program();
    }
    if c.emulator {
        s.wizard_emulator();
    }
    if let Some(p) = c.add {
        s.wizard_add_emulator(p);
    }
    if c.next {
        s.wizard_next();
    }
    if c.launch {
        s.launch_now();
    }
    if c.back {
        if let Some(w) = &mut s.wizard {
            w.back(&mut s.form);
        }
    }
    let library = &mut s.library;
    if let Some(w) = &mut s.wizard {
        if let Some(yes) = c.answer {
            w.answer(yes, library);
        }
        if c.skip {
            w.skip(library);
        }
        if c.save_again {
            w.retry_save(library);
        }
    }
    if let (Some(w), Some(config)) = (&mut s.wizard, s.library.config()) {
        match w.step {
            Step::Program => w.follow_name(&s.form, config),
            Step::Name => w.follow_rom_name(config),
            _ => {}
        }
    }
}

/// The four steps across the top; the current one lit.
fn step_bar(ui: &mut Ui, current: usize) {
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let w = ((bar.width() - 3.0 * U) / 4.0).floor();
    for (i, label) in STEPS.iter().enumerate() {
        let cell = Rect::from_min_size(
            pos2(bar.min.x + i as f32 * (w + U), bar.min.y),
            vec2(w, ROW),
        );
        let cell = theme::snap(cell);
        let p = ui.painter();
        let color = if i <= current { INK } else { SHADE };
        if i == current {
            p.rect_filled(cell, 0.0, SIGNAL);
            theme::outline(p, cell, INK);
        } else {
            theme::bevel(p, cell, SHADE, PAPER);
        }
        theme::label_at(ui, cell, cell.min.x + 2.0 * U, label, color);
    }
}

fn buttons(ui: &mut Ui, step: &Step, s: &Settings, c: &mut Clicks) {
    let w = s.wizard.as_ref();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match step {
        Step::Test { .. } => {
            c.cancel = theme::button(ui, "Done", true, true).clicked();
            c.launch = theme::button(ui, "Launch now", true, false).clicked();
            if w.is_some_and(|w| w.problem.is_some()) {
                c.save_again = theme::button(ui, "Save again", true, false).clicked();
            }
        }
        Step::Pick | Step::Program | Step::Rom | Step::Name => {
            c.cancel = theme::button(ui, "Cancel", true, false).clicked();
            let ready = step != &Step::Pick || w.is_some_and(|w| w.selected.is_some());
            c.next = theme::button(ui, "Next", ready, true).clicked();
            if step != &Step::Pick {
                c.back = theme::button(ui, "Back", true, false).clicked();
            }
        }
        Step::Tap | Step::Overwrite { .. } | Step::Writing { .. } => {
            c.cancel = theme::button(ui, "Cancel", true, false).clicked();
            let tap = step == &Step::Tap;
            c.skip = theme::button(ui, "Skip", tap, false).clicked();
            ui.colored_label(SHADE, "Skip: save without a tag");
        }
    });
}

fn pick(ui: &mut Ui, s: &mut Settings, c: &mut Clicks) {
    let config = s.library.config().cloned();
    let Some(w) = &mut s.wizard else {
        return;
    };
    let any = w.steam.is_ok() || !w.battlenet.is_empty();
    ui.horizontal(|ui| {
        let l = theme::form_label(ui, 14.0 * U, "Search");
        let width = (ui.available_width() - 66.0 * U).floor();
        theme::field(ui, &mut w.search, width, any, l.id);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            c.emulator = theme::button(ui, "Emulator game...", true, false).clicked();
            c.program = theme::button(ui, "Program...", true, false).clicked();
        });
    });
    ui.add_space(U);
    // With Battle.net games listed, a missing Steam is not worth a warning.
    if let (Err(e), true) = (&w.steam, w.battlenet.is_empty()) {
        theme::warning(ui, 0.0, &format!("{e}. Use Program... to add any program."));
    }
    let games = w.filtered();
    let mut clicked = None;
    let mut open = None;
    let body = theme::sortable_list(
        ui,
        &[0.0, 0.62, 0.84],
        &["INSTALLED GAME", "STORE", "LIBRARY"],
        (&mut w.sort, &[0, 1]),
        games.len(),
        |ui, i, row, cols| {
            let game = &games[i];
            let on = w.selected.as_ref() == Some(&game.plan);
            let (resp, fg, weak) =
                theme::list_row(ui, row, ("installed", &game.store), on, &game.name);
            if resp.clicked() {
                clicked = Some(i);
            }
            if resp.double_clicked() {
                open = Some(i);
            }
            theme::label_at(ui, row, cols[0] + 2.0 * U, &game.name, fg);
            theme::label_at(ui, row, cols[1] + 2.0 * U, &game.store, fg);
            let added = Wizard::in_library(config.as_ref(), &game.plan).is_some();
            let x = theme::mark(ui.painter(), row, cols[2] + 2.0 * U, added, fg);
            let (text, color) = if added { ("added", fg) } else { ("-", weak) };
            theme::label_at(ui, row, x, text, color);
        },
    );
    if games.is_empty() && any {
        let at = pos2(body.min.x + 2.0 * U, body.min.y + ROW / 2.0);
        let row = Rect::from_center_size(at, vec2(0.0, ROW));
        theme::label_at(ui, row, at.x, "No installed game matches.", SHADE);
    }
    if let Some(i) = clicked.or(open) {
        w.selected = Some(games[i].plan.clone());
    }
    if open.is_some() {
        c.next = true;
    }
}

fn program(ui: &mut Ui, s: &mut Settings) {
    let label_w = 22.0 * U;
    let indent = label_w + ui.spacing().item_spacing.x;
    let field_w = |ui: &Ui, button: bool| {
        (ui.available_width() - if button { 20.0 * U } else { 0.0 }).floor()
    };
    let Some(w) = &mut s.wizard else {
        return;
    };
    let f = &mut s.form;
    let mut browse = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            theme::group(ui, "Program", |ui| {
                let errors = f.errors.clone();
                let note = |ui: &mut Ui, field| {
                    if let Some((_, e)) = errors.iter().find(|(f, _)| *f == field) {
                        theme::warning(ui, indent, e);
                    }
                };
                ui.horizontal(|ui| {
                    let l = theme::form_label(ui, label_w, "Name");
                    theme::field(ui, &mut f.name, field_w(ui, false), true, l.id);
                });
                note(ui, Field::Name);
                ui.horizontal(|ui| {
                    let l = theme::form_label(ui, label_w, "Tag ID");
                    let r = theme::field(ui, &mut w.slug, 40.0 * U, true, l.id);
                    if r.changed() {
                        w.auto_slug = false;
                    }
                    ui.colored_label(SHADE, "written as id:<this>");
                });
                if let Some(e) = &w.slug_error {
                    theme::warning(ui, indent, e);
                }
                ui.horizontal(|ui| {
                    let l = theme::form_label(ui, label_w, "Exe path");
                    theme::field(ui, &mut f.exe, field_w(ui, true), true, l.id);
                    if theme::button(ui, "Browse", true, false).clicked() {
                        browse = Some(Pick::Exe);
                    }
                });
                note(ui, Field::Exe);
                ui.horizontal(|ui| {
                    let l = theme::form_label(ui, label_w, "Arguments");
                    theme::field(ui, &mut f.args, field_w(ui, false), true, l.id);
                });
                note(ui, Field::Args);
                ui.horizontal(|ui| {
                    let l = theme::form_label(ui, label_w, "Working dir");
                    theme::field(ui, &mut f.cwd, field_w(ui, true), true, l.id);
                    if theme::button(ui, "Browse", true, false).clicked() {
                        browse = Some(Pick::Cwd);
                    }
                });
                note(ui, Field::Cwd);
            });
        });
    if browse.is_some() {
        s.browse = browse;
    }
}

/// The emulator game's emulator and file: typed, browsed for, or picked
/// from a scanned folder.
fn rom(ui: &mut Ui, s: &mut Settings, c: &mut Clicks) {
    let indent = LABEL_W + ui.spacing().item_spacing.x;
    let emulators = s.library.emulators();
    let Some(w) = &mut s.wizard else {
        return;
    };
    let config = s.library.config();
    let preset = config
        .and_then(|c| w.emulator(c))
        .and_then(wizard::preset_of);
    let mut browse = None;
    theme::group(ui, "Emulator game", |ui| {
        row(ui, "Runs on", |ui, _| {
            for e in emulators {
                let on = w.emu.emulator.as_ref() == Some(&e.id);
                if theme::choice(ui, on, &e.name).clicked() {
                    w.choose_emulator(e.id.clone());
                }
                ui.add_space(2.0 * U);
            }
            // Presets not set up yet, so a second emulator can be added here too.
            let missing = PRESETS
                .iter()
                .filter(|p| !emulators.iter().any(|e| wizard::preset_of(e) == Some(*p)));
            for p in missing {
                let label = format!("Add {}...", p.name);
                if theme::button(ui, &label, true, false).clicked() {
                    c.add = Some(p);
                }
            }
            if emulators.is_empty() {
                ui.colored_label(SHADE, "No emulator is set up yet.");
            }
        });
        row(ui, "Game file", |ui, id| {
            theme::field(ui, &mut w.emu.rom, field_w(ui, true), true, id);
            if theme::button(ui, "Browse", true, false).clicked() {
                browse = Some(Pick::GameFile);
            }
        });
        if let Some(e) = &w.emu.rom_error {
            theme::warning(ui, indent, e);
        }
        row(ui, "", |ui, _| {
            if theme::button(ui, "Scan folder...", preset.is_some(), false).clicked() {
                browse = Some(Pick::GameFolder);
            }
            let hint = match preset {
                Some(_) => "or pick one from a folder of games",
                None => "Scan doesn't know this emulator's game files: use Browse.",
            };
            ui.colored_label(SHADE, hint);
        });
    });
    if browse.is_some() {
        s.browse = browse;
    }
    if let Some(dir) = w.scanning() {
        ui.add_space(U);
        let body = theme::list(ui, &[0.0, 0.5], &["GAME", "FILE"], 0, |_, _, _, _| {});
        list_note(ui, body, &format!("Scanning {}...", dir.display()));
        return;
    }
    if w.emu.scan_failed {
        ui.add_space(U);
        let body = theme::list(ui, &[0.0, 0.5], &["GAME", "FILE"], 0, |_, _, _, _| {});
        list_note(ui, body, "Couldn't scan this folder.");
        return;
    }
    let Some((dir, found)) = &w.emu.scan else {
        return;
    };
    ui.add_space(U);
    if found.truncated {
        let n = found.roms.len();
        let text = format!(
            "Showing {n} game files; the folder has more. Choose a smaller folder, or Browse for the file."
        );
        theme::warning(ui, 0.0, &text);
    }
    let hint = preset.and_then(|p| p.updates_hint).unwrap_or_default();
    if let Some(text) = skipped_note(found.skipped_updates, hint) {
        ui.colored_label(SHADE, text);
    }
    let (mut clicked, mut open) = (None, None);
    // Paths in the scan start with `dir` as given.
    let dir = dir.display().to_string();
    let body = theme::list(
        ui,
        &[0.0, 0.5],
        &["GAME", "FILE"],
        found.roms.len(),
        |ui, i, row, cols| {
            let rom = &found.roms[i];
            let on = wizard::same_path(Path::new(w.emu.rom.trim()), &rom.path);
            let (resp, fg, _) = theme::list_row(ui, row, ("rom", i), on, &rom.name);
            clicked = clicked.or(resp.clicked().then_some(i));
            open = open.or(resp.double_clicked().then_some(i));
            let path = rom.path.display().to_string();
            let file = path
                .strip_prefix(&dir)
                .map_or(&*path, |f| f.trim_start_matches(['/', '\\']));
            theme::label_at(ui, row, cols[0] + 2.0 * U, &rom.name, fg);
            theme::label_at(ui, row, cols[1] + 2.0 * U, file, fg);
        },
    );
    if found.roms.is_empty() {
        list_note(ui, body, "No game files in this folder.");
    }
    if let Some(i) = clicked.or(open) {
        w.emu.rom = found.roms[i].path.display().to_string();
    }
    c.next |= open.is_some();
}

/// What the scan says about the update and DLC files it left out.
fn skipped_note(n: usize, hint: &str) -> Option<String> {
    let files = match n {
        0 => return None,
        1 => "1 update/DLC file".to_owned(),
        n => format!("{n} update/DLC files"),
    };
    Some(format!("Skipped {files}. {hint}").trim_end().to_owned())
}

/// A grey line in the first row of an empty list.
fn list_note(ui: &Ui, body: Rect, text: &str) {
    let at = pos2(body.min.x + 2.0 * U, body.min.y + ROW / 2.0);
    let row = Rect::from_center_size(at, vec2(0.0, ROW));
    theme::label_at(ui, row, at.x, text, SHADE);
}

/// The emulator game's name and tag ID.
fn name(ui: &mut Ui, s: &mut Settings) {
    let indent = LABEL_W + ui.spacing().item_spacing.x;
    let Some(w) = &mut s.wizard else {
        return;
    };
    let config = s.library.config();
    let emulator = config.and_then(|c| w.emulator(c)).map(|e| e.name.clone());
    theme::group(ui, "Emulator game", |ui| {
        row(ui, "Name", |ui, id| {
            theme::field(ui, &mut w.emu.name, field_w(ui, false), true, id);
        });
        if let Some(e) = &w.emu.name_error {
            theme::warning(ui, indent, e);
        }
        row(ui, "Tag ID", |ui, id| {
            if theme::field(ui, &mut w.slug, 40.0 * U, true, id).changed() {
                w.auto_slug = false;
            }
            ui.colored_label(SHADE, "written as id:<this>");
        });
        if let Some(e) = &w.slug_error {
            theme::warning(ui, indent, e);
        }
        row(ui, "Runs on", |ui, _| {
            ui.label(emulator.unwrap_or_default());
        });
        row(ui, "Game file", |ui, _| {
            ui.add(egui::Label::new(w.emu.rom.trim()).truncate());
        });
    });
}

/// A big headline, then lines of detail, in a sunken panel.
fn panel(ui: &mut Ui, headline: &str, add: impl FnOnce(&mut Ui)) {
    let frame = egui::Frame::NONE.inner_margin(egui::Margin::same((4.0 * U) as i8));
    let out = frame.show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(egui::RichText::new(headline).size(BIG).color(INK));
        ui.add_space(2.0 * U);
        add(ui);
    });
    let r = theme::snap(out.response.rect);
    theme::bevel(ui.painter(), r, SHADE, PAPER);
}

/// A label row with a status mark.
fn marked(ui: &mut Ui, on: bool, text: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(6.0, ROW), Sense::hover());
        theme::mark(ui.painter(), r, r.min.x, on, INK);
        ui.add(egui::Label::new(text).wrap());
    });
}

fn tap(ui: &mut Ui, s: &mut Settings) {
    let Some(w) = &s.wizard else {
        return;
    };
    let name = w.game.as_ref().map(|g| g.name.as_str()).unwrap_or_default();
    let payload = w
        .payload()
        .map(|p| wizard::payload_text(&p))
        .unwrap_or_default();
    let writing = matches!(w.step, Step::Writing { .. });
    // Until the run loop takes up the session, launches may still be on:
    // don't ask for the cart yet.
    let starting = w.has_reader() && !w.armed();
    let headline = if writing {
        "WRITING..."
    } else if starting {
        "GETTING THE READER..."
    } else {
        "HOLD THE CART ON THE READER"
    };
    panel(ui, headline, |ui| {
        ui.label(format!("Game: {name}"));
        ui.label(format!("Tag gets: {payload}"));
        ui.add_space(U);
        if writing {
            ui.label("Keep the cart still until this finishes.");
        } else {
            ui.colored_label(
                SHADE,
                "A blank tag is written. A tag that can't be written is remembered by its UID.",
            );
        }
        ui.add_space(U);
        match (w.has_reader(), w.reader) {
            (false, _) => {}
            (true, Some(true)) => marked(
                ui,
                true,
                "Reader ready. Launches are off while you add a game.",
            ),
            (true, Some(false)) => marked(
                ui,
                false,
                "Reader not connected. Plug it in; the wizard keeps waiting.",
            ),
            (true, None) => marked(
                ui,
                false,
                "Taking over the reader; wait a moment before putting the cart on.",
            ),
        }
    });
    if let Some(problem) = &w.problem {
        ui.add_space(U);
        theme::warning(ui, 0.0, problem);
    }
}

fn test(ui: &mut Ui, s: &mut Settings, outcome: &Outcome) {
    let Some(w) = &s.wizard else {
        return;
    };
    let name = w.game.as_ref().map(|g| g.name.as_str()).unwrap_or_default();
    let headline = if w.problem.is_some() {
        "NOT SAVED"
    } else {
        "SAVED"
    };
    panel(ui, headline, |ui| {
        ui.label(format!("Game: {name}"));
        ui.add_space(U);
        let fallback = matches!(outcome, Outcome::Bound { .. });
        let text = wizard::outcome_text(outcome, w.payload().as_ref());
        marked(ui, !fallback, &text);
        ui.add_space(U);
        if !matches!(outcome, Outcome::Skipped) {
            ui.label("Take the cart off and put it back to try it, or use Launch now.");
        } else if !w.from_edit {
            ui.label("Use Write tag... in Edit to add a tag later.");
        }
    });
    if let Some(problem) = &w.problem {
        ui.add_space(U);
        theme::warning(ui, 0.0, problem);
    }
    match &w.launched {
        Some(Ok(())) => {
            ui.add_space(U);
            ui.label(format!("Started {name}."));
        }
        Some(Err(e)) => {
            ui.add_space(U);
            theme::warning(ui, 0.0, &format!("Could not start it: {e}"));
        }
        None => {}
    }
}
