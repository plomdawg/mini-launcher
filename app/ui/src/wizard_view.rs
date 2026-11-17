//! The add-game wizard's screens, in the settings window's retro theme.
//! The logic is in [`crate::wizard`].

use egui::{pos2, vec2, Align, Layout, Rect, Sense, Ui};

use crate::library::Field;
use crate::settings::{Pick, Settings};
use crate::theme::{self, BIG, INK, PAPER, ROW, SHADE, SIGNAL, U};
use crate::wizard::{self, Outcome, Step, Wizard};

const STEPS: [&str; 4] = ["1 PICK", "2 TAG", "3 WRITE", "4 TEST"];

/// What the user clicked this frame.
#[derive(Default)]
struct Clicks {
    next: bool,
    program: bool,
    cancel: bool,
    skip: bool,
    launch: bool,
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
    if c.next {
        s.wizard_next();
    }
    if c.launch {
        s.launch_now();
    }
    let library = &mut s.library;
    if let Some(w) = &mut s.wizard {
        if let Some(yes) = c.answer {
            w.answer(yes, library);
        }
        if c.skip {
            w.skip(library);
        }
    }
    if let (Some(w), Some(config)) = (&mut s.wizard, s.library.config()) {
        if w.step == Step::Program {
            w.follow_name(&s.form, config);
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
        }
        Step::Pick | Step::Program => {
            c.cancel = theme::button(ui, "Cancel", true, false).clicked();
            let ready = step == &Step::Program || w.is_some_and(|w| w.selected.is_some());
            c.next = theme::button(ui, "Next", ready, true).clicked();
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
    ui.horizontal(|ui| {
        let l = theme::form_label(ui, 14.0 * U, "Search");
        let width = (ui.available_width() - 30.0 * U).floor();
        theme::field(ui, &mut w.search, width, w.steam.is_ok(), l.id);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            c.program = theme::button(ui, "Program...", true, false).clicked();
        });
    });
    ui.add_space(U);
    if let Err(e) = &w.steam {
        theme::warning(ui, 0.0, &format!("{e}. Use Program... to add any program."));
    }
    let games = w.filtered();
    let mut clicked = None;
    let mut open = None;
    let body = theme::list(
        ui,
        &[0.0, 0.62, 0.80],
        &["STEAM GAME", "APP ID", "LIBRARY"],
        games.len(),
        |ui, i, row, cols| {
            let game = games[i];
            let on = w.selected == Some(game.appid);
            let (resp, fg, weak) = theme::list_row(ui, row, ("steam", game.appid), on, &game.name);
            if resp.clicked() {
                clicked = Some(game.appid);
            }
            if resp.double_clicked() {
                open = Some(game.appid);
            }
            theme::label_at(ui, row, cols[0] + 2.0 * U, &game.name, fg);
            theme::label_at(ui, row, cols[1] + 2.0 * U, &game.appid.to_string(), fg);
            let added = Wizard::in_library(config.as_ref(), game.appid).is_some();
            let x = theme::mark(ui.painter(), row, cols[2] + 2.0 * U, added, fg);
            let (text, color) = if added { ("added", fg) } else { ("-", weak) };
            theme::label_at(ui, row, x, text, color);
        },
    );
    if games.is_empty() && w.steam.is_ok() {
        let at = pos2(body.min.x + 2.0 * U, body.min.y + ROW / 2.0);
        let row = Rect::from_center_size(at, vec2(0.0, ROW));
        theme::label_at(ui, row, at.x, "No installed game matches.", SHADE);
    }
    if let Some(appid) = clicked.or(open) {
        w.selected = Some(appid);
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
