//! The library screen's Export / Import / Folder actions and their dialogs
//! (model in [`crate::transfer`]).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use egui::{Color32, Context, Id, Margin, Stroke, Ui};
use mini_launcher_core::config::location::ConfigLocation;

use super::Settings;
use crate::library::Library;
use crate::theme::{self, FACE, INK, PAPER, SHADE, U};
use crate::transfer::{export, export_name, Existing, ImportPlan, Move};

/// A native file dialog the window should open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Save an export under this suggested name.
    Export(String),
    /// Pick a file to import.
    Import,
    /// Pick the new config folder.
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    /// Import this file? (summary + confirm)
    Import(ImportPlan),
    /// Where the config lives, with Move / Reset.
    Folder,
    /// The new folder already has a config: use it, or copy mine over?
    Existing(Move),
    /// A finished action: title and what happened.
    Done(&'static str, String),
}

#[derive(Debug, Default)]
pub struct Transfer {
    pub dialog: Option<Dialog>,
    request: Option<Request>,
    /// Why the open dialog's last action failed.
    pub problem: Option<String>,
}

impl Settings {
    /// The file dialog the window should open, if a button asked for one.
    pub fn take_request(&mut self) -> Option<Request> {
        self.transfer.request.take()
    }

    fn open(&mut self, dialog: Dialog) {
        self.transfer.problem = None;
        self.transfer.dialog = Some(dialog);
    }

    pub fn export_to(&mut self, path: PathBuf) {
        let Some(location) = self.host.location() else {
            return;
        };
        match export(&location, &path) {
            Ok(()) => {
                let n = self.library.games().len();
                self.open(Dialog::Done(
                    "Export",
                    format!("Saved {n} games to {}.", path.display()),
                ));
            }
            Err(e) => self.problem = Some(format!("Not exported: {e}")),
        }
    }

    /// Validates `path` and asks for confirmation with a summary.
    pub fn import_from(&mut self, path: PathBuf) {
        match ImportPlan::read(&path) {
            Ok(plan) => self.open(Dialog::Import(plan)),
            Err(e) => self.problem = Some(format!("Not imported: {e}")),
        }
    }

    pub fn confirm_import(&mut self, plan: &ImportPlan) {
        let Some(location) = self.host.location() else {
            return;
        };
        match plan.apply(&location, SystemTime::now()) {
            Ok(backup) => {
                self.host.saved();
                self.reload();
                let mut text = format!("Imported {} games.", plan.games);
                if let Some(backup) = backup {
                    text.push_str(&format!(" Your previous config was saved as {backup}."));
                }
                self.open(Dialog::Done("Import", text));
            }
            Err(e) => self.transfer.problem = Some(format!("Not imported: {e}")),
        }
    }

    pub fn open_folder(&mut self) {
        self.open(Dialog::Folder);
    }

    /// The user picked `dir` (or Reset: the default folder).
    pub fn move_to(&mut self, dir: PathBuf) {
        let Some(default) = self.host.default_dir() else {
            return;
        };
        let current = self.host.location();
        match Move::plan(&default, current.as_ref(), &dir) {
            Ok(None) => self.transfer.problem = Some("The config is already there.".into()),
            Ok(Some(mv)) if mv.needs_choice() => self.open(Dialog::Existing(mv)),
            // Nothing there yet: take the library along.
            Ok(Some(mv)) => self.finish_move(&mv, true),
            Err(e) => self.transfer.problem = Some(e),
        }
    }

    pub fn finish_move(&mut self, mv: &Move, copy_mine: bool) {
        let Some(default) = self.host.default_dir() else {
            return;
        };
        let current = self.host.location();
        match mv.apply(&default, current.as_ref(), copy_mine) {
            Ok(()) => {
                self.host.moved();
                self.reload();
                let text = format!("Mini Launcher now uses {}.", mv.dir.display());
                self.open(Dialog::Done("Config folder", text));
            }
            Err(e) => {
                self.open(Dialog::Folder);
                self.transfer.problem = Some(format!("Not moved: {e}"));
            }
        }
    }

    fn reload(&mut self) {
        self.library = Library::load(self.host.location());
        let n = self.library.games().len();
        self.selected = (n > 0).then_some(0);
        self.problem = None;
    }
}

/// The toolbar's right-hand buttons.
pub(super) fn toolbar(ui: &mut Ui, s: &mut Settings) {
    let editable = s.library.problem().is_none();
    if theme::button(ui, "Import...", editable, false).clicked() {
        s.transfer.request = Some(Request::Import);
    }
    if theme::button(ui, "Export...", editable, false).clicked() {
        s.transfer.request = Some(Request::Export(export_name(SystemTime::now())));
    }
    let folder = s.host.default_dir().is_some();
    if theme::button(ui, "Folder...", folder, false).clicked() {
        s.open_folder();
    }
}

/// The open dialog, if any.
pub(super) fn dialogs(ui: &mut Ui, s: &mut Settings) {
    let Some(dialog) = s.transfer.dialog.clone() else {
        return;
    };
    let problem = s.transfer.problem.clone();
    let ctx = ui.ctx().clone();
    match dialog {
        Dialog::Done(title, text) => {
            if modal(&ctx, title, |ui| body(ui, &text), &[("OK", true)]).is_some() {
                s.transfer.dialog = None;
            }
        }
        Dialog::Import(plan) => {
            let current = s.library.games().len();
            let text = format!(
                "Import {}?\n\nIt has {} games, {} bound to tags. It replaces your \
                 library ({current} games); your current config is backed up first.",
                plan.from
                    .file_name()
                    .map_or(plan.from.display(), |n| Path::new(n).display()),
                plan.games,
                plan.bound,
            );
            let buttons = [("Import", true), ("Cancel", true)];
            match modal(
                &ctx,
                "Import",
                |ui| with_problem(ui, &text, &problem),
                &buttons,
            ) {
                Some(0) => s.confirm_import(&plan),
                Some(_) => s.transfer.dialog = None,
                None => {}
            }
        }
        Dialog::Folder => {
            let location = s.host.location();
            let reset = matches!(
                location,
                Some(ConfigLocation::Override(_) | ConfigLocation::OverrideUnavailable { .. })
            );
            let buttons = [
                ("Move to folder...", location.is_some()),
                ("Reset to default", reset),
                ("Close", true),
            ];
            let show = |ui: &mut Ui| {
                folder_status(ui, location.as_ref());
                ui.add_space(U);
                ui.colored_label(
                    SHADE,
                    "Keep it in a synced folder (e.g. OneDrive) to use one library on several PCs.",
                );
                if let Some(p) = &problem {
                    ui.add_space(U);
                    theme::warning(ui, 0.0, p);
                }
            };
            match modal(&ctx, "Config folder", show, &buttons) {
                Some(0) => {
                    s.transfer.problem = None;
                    s.transfer.request = Some(Request::Folder);
                }
                Some(1) => {
                    if let Some(default) = s.host.default_dir() {
                        s.transfer.problem = None;
                        s.move_to(default);
                    }
                }
                Some(_) => s.transfer.dialog = None,
                None => {}
            }
        }
        Dialog::Existing(mv) => {
            let there = match &mv.existing {
                Existing::Config(n) => format!("a config.toml with {n} games"),
                Existing::Broken(e) => format!("a config.toml that can't be loaded ({e})"),
                Existing::Nothing => "nothing".into(),
            };
            let text = format!(
                "{}\n\nThis folder already has {there}. Use the one there, or copy your \
                 library over it? (The one there is then kept as config.toml.bak.)",
                mv.dir.display()
            );
            let mine = s.host.location().is_some_and(|l| l.is_available());
            let theirs = !matches!(mv.existing, Existing::Broken(_));
            let buttons = [
                ("Use the one there", theirs),
                ("Copy mine over", mine),
                ("Cancel", true),
            ];
            match modal(
                &ctx,
                "Config folder",
                |ui| with_problem(ui, &text, &problem),
                &buttons,
            ) {
                Some(i @ (0 | 1)) => s.finish_move(&mv, i == 1),
                Some(_) => s.open(Dialog::Folder),
                None => {}
            }
        }
    }
}

fn folder_status(ui: &mut Ui, location: Option<&ConfigLocation>) {
    let (kind, dir) = match location {
        None => ("This system has no config folder.", None),
        Some(ConfigLocation::Default(d)) => ("Default folder:", Some(d)),
        Some(ConfigLocation::Override(d)) => ("Custom folder (location.toml):", Some(d)),
        Some(ConfigLocation::OverrideUnavailable { dir, reason }) => {
            ui.label("Custom folder (location.toml), unavailable:");
            if let Some(d) = dir {
                body(ui, &d.display().to_string());
            }
            theme::warning(ui, 0.0, reason);
            return;
        }
    };
    ui.label(kind);
    if let Some(d) = dir {
        body(ui, &d.display().to_string());
    }
}

fn body(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(text).wrap());
}

fn with_problem(ui: &mut Ui, text: &str, problem: &Option<String>) {
    body(ui, text);
    if let Some(p) = problem {
        ui.add_space(U);
        theme::warning(ui, 0.0, p);
    }
}

/// A modal in the style of [`theme::confirm`] with any number of buttons
/// (the first is the default, the last is Cancel/Close and what Escape
/// does). The index of the clicked button, `None` while open.
fn modal(
    ctx: &Context,
    title: &str,
    add: impl FnOnce(&mut Ui),
    buttons: &[(&str, bool)],
) -> Option<usize> {
    let frame = egui::Frame::NONE
        .fill(FACE)
        .stroke(Stroke::new(1.0, INK))
        .inner_margin(Margin::same((3.0 * U) as i8));
    let modal = egui::Modal::new(Id::NULL.with(("dialog", title)))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(64))
        .show(ctx, |ui| {
            ui.set_width(110.0 * U);
            let r = ui.max_rect().expand(3.0 * U);
            theme::bevel(ui.painter(), r.shrink(1.0), PAPER, SHADE);
            ui.label(egui::RichText::new(title).color(INK));
            ui.add_space(U);
            add(ui);
            ui.add_space(2.0 * U);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut clicked = None;
                for (i, (label, enabled)) in buttons.iter().enumerate().rev() {
                    if theme::button(ui, label, *enabled, i == 0).clicked() {
                        clicked = Some(i);
                    }
                }
                clicked
            })
            .inner
        });
    if modal.should_close() {
        Some(buttons.len() - 1)
    } else {
        modal.inner
    }
}
