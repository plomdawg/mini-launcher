//! The Settings screen: app preferences (start at login, a cart left in the
//! reader at start-up, text size, sound effects), the library file (import / export /
//! folder), and the Pause button the library toolbar shows.
//!
//! Preferences other than start at login live in `[settings]` in
//! `config.toml` (see `docs/config.md`) and are saved as soon as they
//! change; start at login is the Windows "Run" entry the tray manages.

use egui::{Align, Context, Layout, Ui};
use mini_launcher_core::config::{
    self, BootCart, Sounds, FONT_SCALE_MAX, FONT_SCALE_MIN, VOLUME_MAX,
};
use mini_launcher_core::sound::Sound;

use super::{backup, Screen, Settings};
use crate::library::Library;
use crate::theme::{self, SHADE, U};

/// The text sizes offered. The window grows with the text (see
/// `window::App`), so larger steps would no longer fit a 1366×768 screen.
pub const TEXT_SIZES: [f32; 4] = [0.9, 1.0, 1.25, 1.5];

#[derive(Debug, Default)]
pub struct Prefs {
    /// Why the last change on the Settings screen failed.
    pub problem: Option<String>,
}

impl Settings {
    /// Opens the Settings screen.
    pub fn open_prefs(&mut self) {
        self.prefs.problem = None;
        self.problem = None;
        self.screen = Screen::Settings;
    }

    /// Saves `[settings]` changed by `change`, and tells the tray.
    pub fn save_prefs(&mut self, change: impl FnOnce(&mut config::Settings)) {
        let mut prefs = self.library.settings();
        change(&mut prefs);
        match self.library.save_settings(&prefs) {
            Ok(()) => {
                self.prefs.problem = None;
                self.host.saved();
            }
            Err(e) => self.prefs.problem = Some(format!("Not saved: {e}")),
        }
    }

    /// The start-at-login checkbox was clicked.
    pub fn set_start_at_login(&mut self, on: bool) {
        self.prefs.problem = self
            .host
            .set_start_at_login(on)
            .err()
            .map(|e| format!("Start at login not changed: {e}"));
    }
}

/// Makes the window's zoom follow `font_scale` (also after an outside edit).
/// Changing [`theme::TEXT`] instead would break the fixed row heights.
pub fn apply_zoom(ctx: &Context, library: &Library) {
    let scale = library
        .settings()
        .font_scale
        .clamp(FONT_SCALE_MIN, FONT_SCALE_MAX);
    if ctx.zoom_factor() != scale {
        ctx.set_zoom_factor(scale);
    }
}

/// The library toolbar's Pause / Resume (as the tray menu's), if the host
/// has a run loop to pause.
pub(super) fn pause_button(ui: &mut Ui, s: &mut Settings) {
    let Some(paused) = s.host.paused() else {
        return;
    };
    let label = if paused { "Resume" } else { "Pause" };
    if theme::button(ui, label, true, false).clicked() {
        s.host.set_paused(!paused);
        ui.ctx().request_repaint();
    }
}

pub(super) fn show(ui: &mut Ui, s: &mut Settings) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::button(ui, "Back", true, false).clicked() {
                s.screen = Screen::Library;
            }
        });
    });
    ui.add_space(U);
    backup::dialogs(ui, s);
    let readable = s.library.problem().is_none();
    if let Some(problem) = s.library.problem() {
        let problem = format!("The settings can't be shown: {problem}");
        theme::warning(ui, 0.0, &problem);
    }
    for problem in [&s.prefs.problem, &s.problem].into_iter().flatten() {
        theme::warning(ui, 0.0, problem);
    }
    let prefs = s.library.settings();
    let login = s.host.start_at_login();
    let (mut login_to, mut boot_to, mut size_to) = (None, None, None);
    let (mut sounds_to, mut test) = (None, None);
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            theme::group(ui, "Startup", |ui| {
                let label = "Start Mini Launcher when I sign in";
                let on = login.unwrap_or(false);
                if theme::check(ui, on, label, login.is_some()).clicked() {
                    login_to = Some(!on);
                }
                ui.label("When a cart is already in the reader at startup:");
                ui.horizontal(|ui| {
                    ui.add_space(3.0 * U);
                    for (boot, label) in [
                        (BootCart::Wait, "Wait until it's re-inserted"),
                        (BootCart::Launch, "Launch it"),
                    ] {
                        let on = readable && prefs.boot_cart == boot;
                        if theme::choice(ui, on, label).clicked() && readable {
                            boot_to = Some(boot);
                        }
                        ui.add_space(2.0 * U);
                    }
                });
            });
            theme::group(ui, "Text size", |ui| {
                ui.horizontal(|ui| {
                    // A hand-edited size between the steps is shown too.
                    let mut sizes = TEXT_SIZES.to_vec();
                    if readable && !sizes.contains(&prefs.font_scale) {
                        sizes.push(prefs.font_scale);
                        sizes.sort_by(f32::total_cmp);
                    }
                    for size in sizes {
                        let label = format!("{}%", (size * 100.0).round());
                        let on = readable && prefs.font_scale == size;
                        if theme::choice(ui, on, &label).clicked() && readable {
                            size_to = Some(size);
                        }
                        ui.add_space(2.0 * U);
                    }
                    ui.colored_label(SHADE, "(this window)");
                });
            });
            theme::group(ui, "Sounds", |ui| {
                sounds_to = sound_controls(ui, readable, &prefs.sounds, &mut test);
            });
            theme::group(ui, "Library file", |ui| {
                ui.horizontal(|ui| {
                    backup::toolbar(ui, s);
                    ui.colored_label(SHADE, "Back up, restore, or sync it.");
                });
            });
        });
    if let Some(on) = login_to {
        s.set_start_at_login(on);
    }
    if let Some(boot) = boot_to.filter(|&b| b != prefs.boot_cart) {
        s.save_prefs(|p| p.boot_cart = boot);
    }
    if let Some(size) = size_to.filter(|&z| z != prefs.font_scale) {
        s.save_prefs(|p| p.font_scale = size);
    }
    if let Some((sounds, preview)) = sounds_to.filter(|(to, _)| *to != prefs.sounds) {
        s.save_prefs(|p| p.sounds = sounds);
        // Let the user hear what they turned on, at the new volume.
        if let Some(sound) = preview.filter(|&p| sounds.plays(p)) {
            s.host.play(sound, sounds.volume);
        }
    }
    if let Some(sound) = test {
        // An explicit Test plays even with sounds off.
        s.host.play(sound, prefs.sounds.volume);
    }
}

/// The events on the Sounds group, with their check labels.
pub const SOUND_EVENTS: [(Sound, &str); 5] = [
    (Sound::Waiting, "Waiting for tag"),
    (Sound::Writing, "Writing"),
    (Sound::Success, "Success"),
    (Sound::Failure, "Failure"),
    (Sound::Launch, "Game launch"),
];

/// How much one segment of the volume meter (and `-` / `+`) is worth.
const VOLUME_STEP: u8 = 10;

/// The Sounds group: master switch, volume and one switch per event.
/// Returns the changed settings with the sound to preview, if any; `test`
/// is set when the Test button was clicked.
fn sound_controls(
    ui: &mut Ui,
    readable: bool,
    now: &Sounds,
    test: &mut Option<Sound>,
) -> Option<(Sounds, Option<Sound>)> {
    let mut to = None;
    let on = readable && now.enabled;
    ui.horizontal(|ui| {
        if theme::check(ui, readable && now.enabled, "Play sounds", readable).clicked() {
            to = Some((
                Sounds {
                    enabled: !now.enabled,
                    ..*now
                },
                Some(Sound::Success),
            ));
        }
        ui.add_space(4.0 * U);
        ui.label("Volume");
        let volume = |v: u8| {
            let sounds = Sounds {
                volume: v.min(VOLUME_MAX),
                ..*now
            };
            Some((sounds, Some(Sound::Success)))
        };
        let down = now.volume.saturating_sub(1) / VOLUME_STEP * VOLUME_STEP;
        if theme::small_button(ui, "-", on && now.volume > 0).clicked() {
            to = volume(down);
        }
        let steps = usize::from(VOLUME_MAX / VOLUME_STEP);
        let lit = usize::from(now.volume / VOLUME_STEP);
        if let (_, Some(n)) = theme::level(ui, lit, steps, on, "Volume") {
            to = volume(n as u8 * VOLUME_STEP);
        }
        let up = (now.volume / VOLUME_STEP + 1) * VOLUME_STEP;
        if theme::small_button(ui, "+", on && now.volume < VOLUME_MAX).clicked() {
            to = volume(up);
        }
        ui.label(format!("{}%", now.volume));
        ui.add_space(4.0 * U);
        if theme::button(ui, "Test", readable && now.volume > 0, false).clicked() {
            *test = Some(Sound::Success);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.add_space(3.0 * U);
        for (sound, label) in SOUND_EVENTS {
            let mut next = *now;
            let flag = match sound {
                Sound::Waiting => &mut next.waiting,
                Sound::Writing => &mut next.writing,
                Sound::Success => &mut next.success,
                Sound::Failure => &mut next.failure,
                Sound::Launch => &mut next.launch,
            };
            let was = *flag;
            if theme::check(ui, readable && was, label, on).clicked() && on {
                *flag = !was;
                to = Some((next, (!was).then_some(sound)));
            }
            ui.add_space(2.0 * U);
        }
    });
    to
}
