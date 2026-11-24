//! Drives the settings screens headlessly on a temporary config, and (with
//! `--ignored`) renders the design screenshots in `docs/design/` through wgpu.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::{kittest::Queryable, Harness};
use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_core::config::Config;
use mini_launcher_core::emulators::{EDEN, PCSX2};
use mini_launcher_core::launch::LaunchPlan;
use mini_launcher_core::reader::{Simulated, Uid};
use mini_launcher_core::reconnect::{Backoff, Reconnector};
use mini_launcher_core::runner::{Notice, Runner};
use mini_launcher_core::session::{self, SessionServer, Sessions, WriteFailure};
use mini_launcher_core::sound::Sound;
use mini_launcher_core::steam::{SteamGame, SteamKind};
use mini_launcher_ui::library::EmuForm;
use mini_launcher_ui::settings::{self, Host, Pick, ReaderStatus, Screen, Settings};
use mini_launcher_ui::theme;
use mini_launcher_ui::wizard::{Link, Outcome, Step};

const SIZE: [f32; 2] = [640.0, 440.0];

/// A sample library (fictional bindings).
const GAMES: &[(&str, &str, Option<u32>, Option<&str>)] = &[
    (
        "hades",
        "Hades",
        Some(1145360),
        Some("04:A2:3B:1C:5D:80:00"),
    ),
    (
        "celeste",
        "Celeste",
        Some(504230),
        Some("04:19:7E:22:6A:80:00"),
    ),
    (
        "stardew-valley",
        "Stardew Valley",
        Some(413150),
        Some("04:C4:01:9A:3F:81:00"),
    ),
    ("hollow-knight", "Hollow Knight", Some(367520), None),
    (
        "balatro",
        "Balatro",
        Some(2379780),
        Some("04:5B:E2:70:11:80:00"),
    ),
    (
        "outer-wilds",
        "Outer Wilds",
        Some(753640),
        Some("04:8D:36:C9:02:81:00"),
    ),
    ("doom", "DOOM (GZDoom)", None, Some("04:77:A0:4E:B8:80:00")),
    ("factorio", "Factorio", Some(427520), None),
    (
        "portal-2",
        "Portal 2",
        Some(620),
        Some("04:E1:5C:93:27:81:00"),
    ),
    (
        "into-the-breach",
        "Into the Breach",
        Some(590380),
        Some("04:30:BB:68:D4:80:00"),
    ),
    ("slay-the-spire", "Slay the Spire", Some(646570), None),
];

/// Installed Steam games the wizard lists (fictional library).
const STEAM: &[(u32, &str)] = &[
    (1145360, "Hades"),
    (504230, "Celeste"),
    (413150, "Stardew Valley"),
    (367520, "Hollow Knight"),
    (620, "Portal 2"),
    (1794680, "Vampire Survivors"),
    (646570, "Slay the Spire"),
    (250900, "The Binding of Isaac: Rebirth"),
    (1150690, "OMORI"),
];

struct TestHost {
    dir: PathBuf,
    saves: AtomicUsize,
    sessions: Sessions,
    /// The run loop's end until a test takes it (see `host_with_loop`);
    /// kept so sessions stay open in screenshots.
    server: Mutex<Option<SessionServer>>,
    launches: Mutex<Vec<LaunchPlan>>,
    paused: AtomicBool,
    login: AtomicBool,
    /// The sound effects asked for, with their volume.
    played: Mutex<Vec<(Sound, u8)>>,
}

impl Host for TestHost {
    fn location(&self) -> Option<ConfigLocation> {
        Some(ConfigLocation::resolve(&self.dir))
    }
    fn default_dir(&self) -> Option<PathBuf> {
        Some(self.dir.clone())
    }
    fn reader(&self) -> ReaderStatus {
        ReaderStatus {
            text: "Reader COM3: ready".into(),
            ready: true,
        }
    }
    fn saved(&self) {
        self.saves.fetch_add(1, Ordering::SeqCst);
    }
    fn tag_session(&self) -> Option<Box<dyn Link>> {
        Some(Box::new(self.sessions.open()))
    }
    fn steam_games(&self) -> Result<Vec<SteamGame>, String> {
        Ok(STEAM
            .iter()
            .map(|&(appid, name)| SteamGame {
                appid,
                name: name.into(),
                installdir: name.into(),
                library: "C:\\Program Files (x86)\\Steam".into(),
                kind: SteamKind::App,
            })
            .collect())
    }
    fn launch(&self, plan: &LaunchPlan) -> Result<(), String> {
        self.launches.lock().unwrap().push(plan.clone());
        Ok(())
    }
    fn start_at_login(&self) -> Option<bool> {
        Some(self.login.load(Ordering::SeqCst))
    }
    fn set_start_at_login(&self, on: bool) -> Result<(), String> {
        self.login.store(on, Ordering::SeqCst);
        Ok(())
    }
    fn paused(&self) -> Option<bool> {
        Some(self.paused.load(Ordering::SeqCst))
    }
    fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }
    fn play(&self, sound: Sound, volume: u8) {
        self.played.lock().unwrap().push((sound, volume));
    }
}

impl TestHost {
    fn config(&self) -> String {
        std::fs::read_to_string(self.dir.join("config.toml")).unwrap()
    }
    fn played(&self) -> Vec<(Sound, u8)> {
        std::mem::take(&mut self.played.lock().unwrap())
    }
}

/// The tray's run loop, played by hand: a simulated reader fed with
/// `insert`/`remove` lines, ticked by [`Loop::pump`].
type Open = Box<dyn FnMut() -> Result<(String, Simulated), &'static str>>;

struct Loop {
    lines: mpsc::SyncSender<String>,
    server: SessionServer,
    reader: Reconnector<Simulated, Open>,
    runner: Runner,
    now: Instant,
    notices: Vec<Notice>,
}

impl Loop {
    fn line(&self, line: &str) {
        self.lines.send(line.into()).unwrap();
    }
    fn pump(&mut self, ticks: usize) {
        for _ in 0..ticks {
            self.now += Duration::from_millis(200);
            let tick = self
                .server
                .tick(&mut self.reader, &mut self.runner, false, self.now);
            self.notices.extend(self.runner.step(tick, self.now));
        }
    }
}

fn host(name: &str) -> Arc<TestHost> {
    let (host, run_loop) = host_with_loop(name);
    *host.server.lock().unwrap() = Some(run_loop.server);
    host
}

fn host_with_loop(name: &str) -> (Arc<TestHost>, Loop) {
    host_at(std::env::temp_dir().join(format!(
        "mini-launcher-screens-{name}-{}",
        std::process::id()
    )))
}

/// The screenshots' host: a fixed folder, so the paths the config folder
/// dialogs show are the same on every run.
fn design_host() -> Arc<TestHost> {
    let (host, run_loop) = host_at(std::env::temp_dir().join("mini-launcher").join("config"));
    *host.server.lock().unwrap() = Some(run_loop.server);
    host
}

fn host_at(dir: PathBuf) -> (Arc<TestHost>, Loop) {
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut text = String::from("# My carts\n");
    for (id, name, appid, uid) in GAMES {
        text.push_str(&format!("\n[[games]]\nid = \"{id}\"\nname = \"{name}\"\n"));
        if let Some(uid) = uid {
            text.push_str(&format!("uid = \"{uid}\"\n"));
        }
        match appid {
            Some(n) => text.push_str(&format!("steam_appid = {n}\n")),
            None => text.push_str(
                "exe = 'C:\\Games\\GZDoom\\gzdoom.exe'\nargs = [\"-iwad\", \"doom2.wad\"]\n",
            ),
        }
    }
    std::fs::write(dir.join("config.toml"), text).unwrap();
    let (sessions, server) = session::channel();
    let (lines, rx) = mpsc::sync_channel(16);
    let open: Open = Box::new(|| Err("no second reader"));
    let reader = Reconnector::new(
        "COM3".into(),
        Simulated::from_channel(rx),
        open,
        Backoff::default(),
    );
    let host = Arc::new(TestHost {
        dir,
        saves: AtomicUsize::new(0),
        sessions,
        server: Mutex::new(Some(server)),
        launches: Mutex::default(),
        paused: AtomicBool::new(false),
        login: AtomicBool::new(true),
        played: Mutex::default(),
    });
    let server = host.server.lock().unwrap().take().unwrap();
    let run_loop = Loop {
        lines,
        server,
        reader,
        runner: Runner::new(Config::default(), true),
        now: Instant::now(),
        notices: Vec::new(),
    };
    (host, run_loop)
}

fn config_dir(s: &Settings) -> PathBuf {
    match s.library.location() {
        Some(ConfigLocation::Default(d)) => d.clone(),
        other => panic!("{other:?}"),
    }
}

fn harness(host: Arc<TestHost>, scale: f32) -> Harness<'static, Settings> {
    Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(scale)
        .with_context_setup(theme::install)
        .build_ui_state(
            |ui, state| {
                // Fill the whole surface like the real window (the harness adds a margin).
                let full = egui::UiBuilder::new().max_rect(ui.ctx().content_rect());
                ui.scope_builder(full, |ui| settings::show(ui, state));
            },
            Settings::new(host),
        )
}

#[test]
fn library_lists_the_config_and_opens_the_form() {
    let mut h = harness(host("list"), 1.0);
    h.run();
    h.get_by_label("Hollow Knight");
    h.get_by_label("Add game");
    h.get_by_label("Edit").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Edit);
    assert_eq!(h.state().form.name, "Hades");
}

#[test]
fn the_new_cart_notification_opens_the_wizard_without_losing_work() {
    let mut s = Settings::new(host("tray-add"));
    s.from_tray_add();
    assert_eq!(s.screen, Screen::Wizard);
    assert_eq!(s.wizard.as_ref().map(|w| w.step.clone()), Some(Step::Pick));
    // A second click while the wizard runs keeps its progress.
    s.wizard.as_mut().unwrap().search = "port".into();
    s.from_tray_add();
    assert_eq!(s.wizard.as_ref().unwrap().search, "port");
    // A half-edited game is never thrown away.
    s.wizard_close();
    s.edit(0);
    s.form.name = "Hades II".into();
    s.from_tray_add();
    assert_eq!(s.screen, Screen::Edit);
    assert!(s.wizard.is_none());
    assert_eq!(s.form.name, "Hades II");
}

#[test]
fn edit_saves_through_the_config_and_unbinds() {
    let host = host("edit");
    let mut h = harness(host.clone(), 1.0);
    h.state_mut().edit(0);
    h.run();
    for label in [
        "Name",
        "Steam app ID",
        "Exe path",
        "Arguments",
        "Working dir",
    ] {
        h.get_by_label(label);
    }
    h.state_mut().form.name = "Hades II".into();
    h.get_by_label("Unbind").click();
    h.run();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Library);
    assert_eq!(host.saves.load(Ordering::SeqCst), 1);
    let text = host.config();
    assert!(text.starts_with("# My carts") && text.contains("name = \"Hades II\""));
    assert!(!text.contains("04:A2:3B"), "{text}");
}

#[test]
fn invalid_form_shows_errors_and_saves_nothing() {
    let host = host("invalid");
    let before = host.config();
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Add game").click();
    h.run();
    h.get_by_label("Program...").click();
    h.run();
    h.get_by_label("Next").click();
    h.run();
    h.get_by_label("Enter a name.");
    h.get_by_label("Choose the program to start.");
    assert_eq!(h.state().screen, Screen::Wizard);
    assert_eq!(h.state().wizard.as_ref().unwrap().step, Step::Program);
    h.get_by_label("Back").click();
    h.run();
    assert_eq!(h.state().wizard.as_ref().unwrap().step, Step::Pick);
    assert_eq!(host.config(), before);
    assert_eq!(host.saves.load(Ordering::SeqCst), 0);
}

#[test]
fn a_picked_program_fills_in_its_name_and_tag_id_but_never_a_typed_name() {
    let mut h = harness(host("exe-first"), 1.0);
    h.run();
    h.get_by_label("Add game").click();
    h.run();
    h.get_by_label("Program...").click();
    h.run();
    h.get_by_label("Choose program...").click();
    h.run();
    let pick = h.state_mut().take_browse().map(|(pick, _)| pick);
    assert_eq!(pick, Some(Pick::Exe));
    // No such file, so no version info on any OS: named after the file.
    let games = std::env::temp_dir().join("mini-launcher-no-such-dir");
    h.state_mut()
        .picked(Pick::Exe, games.join("Sky Drifter (x64).exe"));
    h.run();
    let s = h.state();
    assert_eq!(s.form.name, "Sky Drifter");
    assert_eq!(s.wizard.as_ref().unwrap().slug, "sky-drifter");
    assert_eq!(s.form.cwd, games.display().to_string());
    // Another pick replaces the filled-in name.
    h.state_mut().picked(Pick::Exe, games.join("hades.exe"));
    h.run();
    assert_eq!(h.state().form.name, "hades");
    assert_eq!(h.state().wizard.as_ref().unwrap().slug, "hades-2");
    // A typed name is kept, and the tag ID follows it.
    h.state_mut().form.name.clear();
    h.run();
    h.get_by_label("Name").focus();
    h.run();
    h.get_by_label("Name").type_text("Hades Remix");
    h.run();
    assert_eq!(h.state().form.name, "Hades Remix");
    h.state_mut().picked(Pick::Exe, games.join("sky.exe"));
    h.run();
    let s = h.state();
    assert_eq!(s.form.name, "Hades Remix");
    assert_eq!(s.wizard.as_ref().unwrap().slug, "hades-remix");
    assert!(s.form.exe.ends_with("sky.exe"));
}

#[test]
fn remove_asks_first() {
    let host = host("remove");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Remove").click();
    h.run();
    h.get_by_label("Cancel").click();
    h.run();
    assert!(host.config().contains("Hades"));
    h.get_by_label("Remove").click();
    h.run();
    // The dialog's Remove is the second one (the toolbar's is first).
    h.get_all_by_label("Remove").last().unwrap().click();
    h.run();
    assert!(!host.config().contains("Hades"));
    assert_eq!(host.saves.load(Ordering::SeqCst), 1);
    assert_eq!(h.state().library.games().len(), GAMES.len() - 1);
}

#[test]
fn remove_targets_the_game_by_id_after_outside_edits() {
    let host = host("remove-id");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Remove").click();
    h.run();
    // While the dialog is open, Hades moves down the file by hand.
    let text = host.config();
    let hades = "\n[[games]]\nid = \"hades\"\nname = \"Hades\"\nuid = \"04:A2:3B:1C:5D:80:00\"\nsteam_appid = 1145360\n";
    let path = host.dir.join("config.toml");
    std::fs::write(&path, format!("{}{hades}", text.replace(hades, ""))).unwrap();
    h.run();
    h.get_all_by_label("Remove").last().unwrap().click();
    h.run();
    let text = host.config();
    assert!(
        !text.contains("Hades") && text.contains("Celeste"),
        "{text}"
    );

    // Removed by hand while the dialog is open: says so, touches nothing.
    h.state_mut().ask_remove(0);
    h.run();
    std::fs::write(&path, "").unwrap();
    h.run();
    h.get_all_by_label("Remove").last().unwrap().click();
    h.run();
    let problem = h.state().problem.clone().unwrap();
    assert!(problem.contains("no longer in the library"), "{problem}");
    assert_eq!(host.config(), "");
}

/// Adds Eden and an "MK8" game on it to the host's config.
fn add_eden(host: &TestHost) {
    let eden = "\n[[emulators]]\nid = \"eden\"\nname = \"Eden\"\nexe = 'C:\\Eden\\eden.exe'\nargs = [\"-g\", \"{rom}\"]\n\n[[games]]\nid = \"mk8\"\nname = \"MK8\"\nemulator = \"eden\"\nrom = 'D:\\mk8.nsp'\n";
    std::fs::write(host.dir.join("config.toml"), host.config() + eden).unwrap();
}

#[test]
fn emulator_games_are_labelled_and_edited_in_the_form() {
    let host = host("emulator-game");
    add_eden(&host);
    let mut h = harness(host.clone(), 1.0);
    h.run();
    let mk8 = h.state().library.games().last().unwrap().clone();
    let emulators = h.state().library.emulators().to_vec();
    assert_eq!(
        settings::launch_label(&mk8.plan, &emulators),
        "Eden: mk8.nsp"
    );
    h.get_by_label("MK8").click();
    h.run();
    h.get_by_label("Edit").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Edit);
    h.get_by_label("Runs on");
    h.state_mut().form.rom = String::new();
    h.get_by_label("Save").click();
    h.run();
    h.get_by_label("Choose the game file.");
    h.state_mut().form.rom = "E:\\mk8 v2.nsp".into();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Library);
    let text = host.config();
    assert!(text.contains("rom = 'E:\\mk8 v2.nsp'"), "{text}");

    // A Steam game can be moved onto the emulator.
    h.state_mut().edit(0);
    h.run();
    h.get_by_label("Emulator").click();
    h.run();
    h.state_mut().form.rom = "D:\\hades.nsp".into();
    h.get_by_label("Save").click();
    h.run();
    let hades = h.state().library.games()[0].clone();
    assert!(
        matches!(hades.plan, LaunchPlan::Emulator { .. }),
        "{hades:?}"
    );
}

#[test]
fn steam_shortcuts_are_labelled_and_edited_in_the_form() {
    let host = host("steam-shortcut");
    let doom = "\n[[games]]\nid = \"gzdoom\"\nname = \"GZDoom\"\nsteam_shortcut = 3237998081\n";
    std::fs::write(host.dir.join("config.toml"), host.config() + doom).unwrap();
    let mut h = harness(host.clone(), 1.0);
    h.run();
    let game = h.state().library.games().last().unwrap().clone();
    assert_eq!(
        settings::launch_label(&game.plan, &[]),
        "Steam shortcut 3237998081"
    );
    let i = h.state().library.games().len() - 1;
    h.state_mut().edit(i);
    h.run();
    assert_eq!(h.state().screen, Screen::Edit);
    h.get_by_label("Shortcut ID");
    assert_eq!(h.state().form.shortcut_id, "3237998081");
    h.state_mut().form.shortcut_id = "x".into();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Edit);
    h.state_mut().form.shortcut_id = "4294967295".into();
    h.state_mut().form.name = "GZDoom 2".into();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Library);
    let text = host.config();
    assert!(
        text.contains("name = \"GZDoom 2\"\nsteam_shortcut = 4294967295"),
        "{text}"
    );

    // A Steam game can be moved onto a shortcut, and back.
    h.state_mut().edit(0);
    h.run();
    h.get_by_label("Steam shortcut").click();
    h.run();
    h.state_mut().form.shortcut_id = "3237998081".into();
    h.get_by_label("Save").click();
    h.run();
    let hades = h.state().library.games()[0].clone();
    assert_eq!(
        hades.plan,
        LaunchPlan::SteamShortcut {
            appid: 3_237_998_081
        }
    );
    h.state_mut().edit(0);
    h.run();
    h.get_by_label("Steam").click();
    h.run();
    h.get_by_label("Steam app ID");
    h.state_mut().form.app_id = "1145360".into();
    h.get_by_label("Save").click();
    h.run();
    let hades = h.state().library.games()[0].clone();
    assert_eq!(hades.plan, LaunchPlan::Steam { appid: 1145360 });
}

#[test]
fn emulators_are_added_edited_and_removed() {
    let host = host("emulators");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Emulators").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Emulators);
    h.get_by_label("Add Eden").click();
    h.run();
    assert_eq!(h.state().screen, Screen::EditEmulator);
    let form = &mut h.state_mut().emulators.form;
    assert_eq!(
        (form.name.as_str(), form.args.as_str()),
        ("Eden", "-f -g {rom}")
    );
    form.exe = "C:\\Eden\\eden.exe".into();
    form.args = "-f -g".into();
    h.get_by_label("Save").click();
    h.run();
    // The core's {rom} rule, next to the field; nothing is saved.
    h.get_by_label("args must contain \"{rom}\" exactly once");
    assert!(!host.config().contains("[[emulators]]"));
    h.state_mut().emulators.form.args = "-f -g {rom}".into();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Emulators);
    h.get_by_label("Edit").click();
    h.run();
    h.state_mut().emulators.form.name = "Eden Nightly".into();
    h.get_by_label("Save").click();
    h.run();
    let text = host.config();
    assert!(
        text.contains("id = \"eden\"") && text.contains("\"Eden Nightly\""),
        "{text}"
    );
    h.get_by_label("Remove").click();
    h.run();
    h.get_all_by_label("Remove").last().unwrap().click();
    h.run();
    assert!(!host.config().contains("[[emulators]]"));
    assert_eq!(host.saves.load(Ordering::SeqCst), 3);

    // Still used by a game: says which, and asks nothing.
    add_eden(&host);
    h.run();
    h.get_by_label("Remove").click();
    h.run();
    let problem = h.state().emulators.problem.clone().unwrap();
    assert!(problem.contains("still used by \"mk8\""), "{problem}");
    assert!(h.state().emulators.confirm_remove.is_none());
    h.get_by_label("Back").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Library);
}

#[test]
fn wizard_adds_a_steam_game_by_writing_the_tag_while_launches_wait() {
    let (host, mut run_loop) = host_with_loop("wizard");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Add game").click();
    h.run();
    h.get_by_label("Vampire Survivors").click();
    h.run();
    h.get_by_label("Next").click();
    h.run();
    // The cart is asked for only once the run loop has the session.
    h.get_by_label("GETTING THE READER...");
    run_loop.pump(1);
    h.run();
    h.get_by_label("HOLD THE CART ON THE READER");
    // A blank tag goes on the reader: it is reported, written, verified.
    run_loop.line("insert 04a1b2c3d4e5f6");
    run_loop.pump(2);
    h.run();
    assert!(matches!(
        h.state().wizard.as_ref().unwrap().step,
        Step::Writing { .. }
    ));
    run_loop.pump(1);
    h.run();
    h.get_by_label("SAVED");
    let cues = [Sound::Waiting, Sound::Writing, Sound::Success];
    assert_eq!(host.played(), cues.map(|c| (c, 60)), "the default volume");
    assert!(
        matches!(&run_loop.notices[..], [Notice::Paused { .. }]),
        "{:?}",
        run_loop.notices
    );
    // Saving gave the reader back; the run loop notices on its next tick.
    run_loop.pump(1);
    assert!(!run_loop.server.is_active());
    let text = host.config();
    assert!(
        text.contains("steam_appid = 1794680\ntag = \"written\""),
        "{text}"
    );
    assert_eq!(host.saves.load(Ordering::SeqCst), 1);
    h.get_by_label("Launch now").click();
    h.run();
    assert_eq!(
        *host.launches.lock().unwrap(),
        [LaunchPlan::Steam { appid: 1794680 }]
    );
    h.get_by_label("Done").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Library);
    assert!(h.state().wizard.is_none());
}

#[test]
fn wizard_offers_the_presets_not_set_up_yet() {
    let (host, _run_loop) = host_with_loop("wizard-pcsx2");
    add_eden(&host);
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Add game").click();
    h.run();
    h.get_by_label("Emulator game...").click();
    h.run();
    assert!(h.query_by_label("Add Eden...").is_none());
    h.get_by_label("Add PCSX2...").click();
    h.run();
    assert_eq!(h.state().screen, Screen::EditEmulator);
    let form = &mut h.state_mut().emulators.form;
    assert_eq!(
        (form.name.as_str(), form.args.as_str()),
        ("PCSX2", "-nogui -fullscreen -- {rom}")
    );
    form.exe = r"C:\Program Files\PCSX2\pcsx2-qt.exe".into();
    h.get_by_label("Save").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Wizard);
    assert!(h.query_by_label("Add PCSX2...").is_none());
    assert_eq!(h.state().game_file_types(), PCSX2.rom_extensions);
    let text = host.config();
    assert!(text.contains("id = \"pcsx2\""), "{text}");
}

#[test]
fn wizard_adds_an_emulator_game_after_adding_eden() {
    let (host, mut run_loop) = host_with_loop("wizard-emulator");
    let roms = host.dir.join("roms");
    std::fs::create_dir_all(&roms).unwrap();
    std::fs::write(roms.join("Sky Drifter [0100AB][v0].nsp"), b"").unwrap();
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Add game").click();
    h.run();
    h.get_by_label("Emulator game...").click();
    h.run();
    h.get_by_label("No emulator is set up yet.");
    h.get_by_label("Add Eden...").click();
    h.run();
    assert_eq!(h.state().screen, Screen::EditEmulator);
    h.state_mut().emulators.form.exe = "C:\\Eden\\eden.exe".into();
    h.get_by_label("Save").click();
    h.run();
    // Back in the wizard, on the new emulator.
    assert_eq!(h.state().screen, Screen::Wizard);
    h.get_by_label("Runs on");
    assert_eq!(h.state().game_file_types(), EDEN.rom_extensions);
    h.get_by_label("Next").click();
    h.run();
    h.get_by_label("Choose the game file.");
    h.state_mut().picked(Pick::GameFolder, roms.clone());
    finish_scan(&mut h);
    h.get_by_label("Sky Drifter").click();
    h.run();
    h.get_by_label("Next").click();
    h.run();
    let w = h.state().wizard.as_ref().unwrap();
    assert_eq!(
        (w.emu.name.as_str(), w.slug.as_str()),
        ("Sky Drifter", "sky-drifter")
    );
    h.get_by_label("Next").click();
    h.run();
    run_loop.pump(1);
    h.run();
    h.get_by_label("Tag gets: id:sky-drifter");
    run_loop.line("insert 04a1b2c3d4e5f6");
    run_loop.pump(2);
    h.run();
    run_loop.pump(1);
    h.run();
    h.get_by_label("SAVED");
    let text = host.config();
    assert!(text.contains("emulator = \"eden\""), "{text}");
    assert!(text.contains("Sky Drifter [0100AB][v0].nsp"), "{text}");
    h.get_by_label("Launch now").click();
    h.run();
    let launches = host.launches.lock().unwrap();
    assert!(matches!(&launches[..], [LaunchPlan::Emulator { .. }]));
}

#[test]
fn a_truncated_rom_scan_and_skipped_updates_say_so() {
    let host = host("wizard-scan");
    add_eden(&host);
    let roms = host.dir.join("roms");
    let mut h = harness(host.clone(), 1.0);
    h.state_mut().add();
    h.state_mut().wizard_emulator();
    h.run();
    h.state_mut().picked(Pick::GameFolder, roms.clone());
    let w = h.state().wizard.as_ref().unwrap();
    assert_eq!(w.scanning(), Some(roms.as_path()), "scanning on a worker");
    finish_scan(&mut h);
    let w = h.state_mut().wizard.as_mut().unwrap();
    w.emu.scan.as_mut().unwrap().1.truncated = true;
    h.run();
    h.get_by_label_contains("Showing 0 game files; the folder has more.");
    assert!(h.query_by_label_contains("update/DLC").is_none());
    let w = h.state_mut().wizard.as_mut().unwrap();
    w.emu.scan.as_mut().unwrap().1.skipped_updates = 1;
    h.run();
    h.get_by_label_contains("Skipped 1 update/DLC file. Install updates and DLC in Eden once");
    let w = h.state_mut().wizard.as_mut().unwrap();
    w.emu.scan.as_mut().unwrap().1.skipped_updates = 3;
    h.run();
    h.get_by_label_contains("Skipped 3 update/DLC files.");
    h.get_by_label("Back").click();
    h.run();
    assert_eq!(h.state().wizard.as_ref().unwrap().step, Step::Pick);
}

#[test]
fn write_tag_from_the_edit_form_and_cancel_goes_back_to_it() {
    let (host, mut run_loop) = host_with_loop("write-tag");
    let mut h = harness(host.clone(), 1.0);
    h.state_mut().edit(3);
    h.run();
    h.get_by_label("Write tag...").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Wizard);
    run_loop.pump(1);
    assert!(run_loop.server.is_active());
    h.get_by_label("Cancel").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Edit);
    assert_eq!(h.state().form.name, "Hollow Knight");
    run_loop.pump(1);
    assert!(!run_loop.server.is_active(), "cancel gives the reader back");
}

#[test]
fn write_tag_from_the_edit_form_records_the_written_tag() {
    let (host, mut run_loop) = host_with_loop("rewrite-tag");
    let mut h = harness(host.clone(), 1.0);
    h.state_mut().edit(3);
    h.run();
    assert_eq!(settings::tag_label(&h.state().library.games()[3]), None);
    h.get_by_label("Write tag...").click();
    h.run();
    run_loop.pump(1);
    h.run();
    run_loop.line("insert 04a1b2c3d4e5f6");
    run_loop.pump(2);
    h.run();
    run_loop.pump(1);
    h.run();
    h.get_by_label("SAVED");
    let text = host.config();
    assert!(
        text.contains("name = \"Hollow Knight\"\nsteam_appid = 367520\ntag = \"written\""),
        "{text}"
    );
    let game = h.state().library.games()[3].clone();
    assert_eq!(
        settings::tag_label(&game).as_deref(),
        Some("steam://367520")
    );
}

#[test]
fn library_shows_how_each_game_is_detected() {
    let host = host("tag-states");
    // Factorio's tag was written; Hades has a UID too, which wins.
    let text = host.config();
    let text = text
        .replace(
            "steam_appid = 427520\n",
            "steam_appid = 427520\ntag = \"written\"\n",
        )
        .replace(
            "steam_appid = 1145360\n",
            "steam_appid = 1145360\ntag = \"written\"\n",
        )
        .replace(
            "args = [\"-iwad\", \"doom2.wad\"]\n",
            "args = [\"-iwad\", \"doom2.wad\"]\ntag = \"written\"\n",
        );
    // DOOM keeps its UID; an exe game written without one shows its id.
    let text = text.replace("uid = \"04:77:A0:4E:B8:80:00\"\n", "");
    std::fs::write(host.dir.join("config.toml"), text).unwrap();
    let mut h = harness(host.clone(), 1.0);
    h.run();
    let tag = |id: &str| {
        let games = h.state().library.games();
        let game = games.iter().find(|g| g.id.as_str() == id).unwrap();
        settings::tag_label(game)
    };
    assert_eq!(tag("hades").as_deref(), Some("UID 04:A2:3B:1C:5D:80:00"));
    assert_eq!(tag("factorio").as_deref(), Some("steam://427520"));
    assert_eq!(tag("doom").as_deref(), Some("id:doom"));
    assert_eq!(tag("hollow-knight"), None);
    // Unbinding a UID in the form keeps the written tag.
    h.state_mut().edit(0);
    h.run();
    h.get_by_label("Detected by");
    h.get_by_label("Unbind").click();
    h.run();
    assert!(h.state().form.uid.is_none() && h.state().form.tag_written);
}

#[test]
fn editing_what_a_written_tag_says_drops_the_marker() {
    let host = host("tag-edit");
    let text = host.config().replace(
        "steam_appid = 427520\n",
        "steam_appid = 427520\ntag = \"written\"\n",
    );
    std::fs::write(host.dir.join("config.toml"), text).unwrap();
    let mut h = harness(host.clone(), 1.0);
    h.run();
    let factorio = |h: &Harness<'static, Settings>| {
        let games = h.state().library.games();
        games
            .iter()
            .find(|g| g.id.as_str() == "factorio")
            .cloned()
            .unwrap()
    };
    // A new name keeps it: the tag still says steam://427520.
    h.state_mut().edit(7);
    h.state_mut().form.name = "Factorio: Space Age".into();
    h.run();
    h.get_by_label("Save").click();
    h.run();
    assert!(factorio(&h).tag_written, "{}", host.config());
    // A new app ID doesn't: the cart still says steam://427520.
    h.state_mut().edit(7);
    h.state_mut().form.app_id = "999".into();
    h.run();
    h.get_by_label("Save").click();
    h.run();
    let game = factorio(&h);
    assert!(!game.tag_written && settings::tag_label(&game).is_none());
    assert!(!host.config().contains("tag = \"written\""));
}

#[test]
fn launch_labels_show_what_starts_the_game() {
    use mini_launcher_core::battlenet::Code;
    use std::path::PathBuf;
    let label = |plan: LaunchPlan| settings::launch_label(&plan, &[]);
    assert_eq!(label(LaunchPlan::Steam { appid: 620 }), "steam://620");
    assert_eq!(
        label(LaunchPlan::SteamShortcut { appid: 3237998081 }),
        "Steam shortcut 3237998081"
    );
    let code = Code::new("WoWF").unwrap();
    assert_eq!(label(LaunchPlan::BattleNet { code }), "battlenet://WoWF");
    for path in ["C:\\Games\\GZDoom\\gzdoom.exe", "/opt/gzdoom/gzdoom.exe"] {
        let plan = LaunchPlan::Exe {
            path: PathBuf::from(path),
            args: vec!["-iwad".into()],
            cwd: None,
        };
        assert_eq!(label(plan), "Program: gzdoom.exe");
    }
    // No file name to show: the whole path.
    let plan = LaunchPlan::Exe {
        path: PathBuf::from("/"),
        args: Vec::new(),
        cwd: None,
    };
    assert_eq!(label(plan), "Program: /");
    // An emulator missing from the list (never in a loaded config) shows
    // its id.
    let plan = LaunchPlan::Emulator {
        emulator: mini_launcher_core::payload::Slug::new("eden").unwrap(),
        rom: PathBuf::from("D:\\Switch\\Sky Drifter.nsp"),
        path: PathBuf::from("eden.exe"),
        args: Vec::new(),
        cwd: None,
    };
    assert_eq!(label(plan), "eden: Sky Drifter.nsp");
}

#[test]
fn launch_in_the_edit_form_tries_the_unsaved_options() {
    let host = host("form-launch");
    let before = host.config();
    let mut h = harness(host.clone(), 1.0);
    h.state_mut().edit(6);
    h.run();
    assert_eq!(h.state().form.name, "DOOM (GZDoom)");
    h.state_mut().form.args = "-iwad plutonia.wad".into();
    h.get_by_label("Launch").click();
    h.run();
    match &host.launches.lock().unwrap()[..] {
        [LaunchPlan::Exe { args, .. }] => assert_eq!(args, &["-iwad", "plutonia.wad"]),
        other => panic!("{other:?}"),
    }
    h.get_by_label_contains("Launched: check the game started");
    assert_eq!(h.state().screen, Screen::Edit);
    assert_eq!(host.saves.load(Ordering::SeqCst), 0);
    assert_eq!(host.config(), before);
    // Editing the form again drops the note about the old options.
    h.state_mut().form.args = "-iwad tnt.wad".into();
    h.run();
    assert!(h.query_by_label_contains("Launched:").is_none());
    assert!(h.state().launched.is_none());
    // A form that can't make a game shows why and launches nothing.
    h.state_mut().form.exe = String::new();
    h.get_by_label("Launch").click();
    h.run();
    h.get_by_label("Choose the program to start.");
    assert!(h.query_by_label_contains("Launched:").is_none());
    assert_eq!(host.launches.lock().unwrap().len(), 1);
    assert_eq!(host.config(), before);
}

#[test]
fn export_and_import_with_summary_and_confirm() {
    let host = host("import");
    let out = host.dir.join("backup.toml");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.state_mut().export_to(out.clone());
    h.run();
    h.get_by_label(&format!("Saved 11 games to {}.", out.display()));
    h.get_by_label("OK").click();
    h.run();
    assert!(h.state().transfer.dialog.is_none());

    std::fs::write(host.dir.join("config.toml"), "").unwrap();
    h.run();
    assert_eq!(h.state().library.games().len(), 0);
    h.state_mut().import_from(out.clone());
    h.run();
    // Cancel changes nothing.
    h.get_by_label("Cancel").click();
    h.run();
    assert_eq!(host.config(), "");
    h.state_mut().import_from(out);
    h.run();
    h.get_all_by_label("Import").last().unwrap().click();
    h.run();
    assert_eq!(h.state().library.games().len(), GAMES.len());
    assert_eq!(host.saves.load(Ordering::SeqCst), 1);

    // A bad file is reported inline and imports nothing.
    let bad = host.dir.join("bad.toml");
    std::fs::write(&bad, "[[games]]\nid = \"x\"\n").unwrap();
    h.get_by_label("OK").click();
    h.run();
    h.state_mut().import_from(bad);
    h.run();
    assert!(h
        .state()
        .problem
        .as_deref()
        .unwrap()
        .starts_with("Not imported"));
    assert!(h.state().transfer.dialog.is_none());
}

#[test]
fn config_folder_moves_and_resets() {
    let host = host("folder");
    let synced = host.dir.join("synced");
    std::fs::create_dir(&synced).unwrap();
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Settings").click();
    h.run();
    h.get_by_label("Folder...").click();
    h.run();
    h.get_by_label("Default folder:");
    h.state_mut().move_to(synced.clone());
    h.run();
    // Empty folder: the library is copied there and the tray told.
    assert_eq!(
        host.location(),
        Some(ConfigLocation::Override(synced.clone()))
    );
    assert_eq!(h.state().library.games().len(), GAMES.len());
    assert_eq!(host.saves.load(Ordering::SeqCst), 1);
    h.get_by_label("OK").click();
    h.run();

    // Reset: the default folder has a config, so the user chooses.
    std::fs::write(host.dir.join("config.toml"), "").unwrap();
    h.get_by_label("Folder...").click();
    h.run();
    h.get_by_label("Reset to default").click();
    h.run();
    h.get_by_label("Use the one there").click();
    h.run();
    assert_eq!(
        host.location(),
        Some(ConfigLocation::Default(host.dir.clone()))
    );
    assert_eq!(h.state().library.games().len(), 0);
}

#[test]
fn settings_save_boot_behaviour_and_text_size_to_the_config() {
    let host = host("prefs");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Settings").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Settings);
    // The default: wait for a re-insert, nothing written yet.
    assert!(!host.config().contains("[settings]"));
    h.get_by_label("Launch it").click();
    h.run();
    let text = host.config();
    assert!(text.starts_with("# My carts"), "{text}");
    assert!(
        text.contains("[settings]\nboot_cart = \"launch\""),
        "{text}"
    );
    assert_eq!(host.saves.load(Ordering::SeqCst), 1);
    h.get_by_label("150%").click();
    h.run();
    assert!(
        host.config().contains("font_scale = 1.5"),
        "{}",
        host.config()
    );
    // Applied to the window at once.
    h.run();
    assert_eq!(h.ctx.zoom_factor(), 1.5);
    h.get_by_label("Wait until it's re-inserted").click();
    h.run();
    let text = host.config();
    assert!(text.contains("boot_cart = \"wait\""), "{text}");
    assert_eq!(host.saves.load(Ordering::SeqCst), 3);
    // Start at login goes to the host, not the config.
    h.get_by_label("Start Mini Launcher when I sign in").click();
    h.run();
    assert!(!host.login.load(Ordering::SeqCst));
    assert!(!host.config().contains("login"));
    h.get_by_label("Back").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Library);
}

#[test]
fn settings_save_sounds_and_volume_to_the_config() {
    let host = host("sounds");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Settings").click();
    h.run();
    // Turning an event off saves it and plays nothing.
    h.get_by_label("Failure").click();
    h.run();
    let text = host.config();
    assert!(
        text.contains("[settings.sounds]\nfailure = false"),
        "{text}"
    );
    assert_eq!(host.played(), []);
    // Turning it back on lets the user hear it.
    h.get_by_label("Failure").click();
    h.run();
    assert!(host.config().contains("failure = true"));
    assert_eq!(host.played(), [(Sound::Failure, 60)]);
    // The volume steps by 10 and previews at the new level.
    h.get_by_label("+").click();
    h.run();
    assert!(host.config().contains("volume = 70"), "{}", host.config());
    assert_eq!(host.played(), [(Sound::Success, 70)]);
    h.get_by_label("70%");
    for _ in 0..2 {
        h.get_by_label("-").click();
        h.run();
    }
    assert!(host.config().contains("volume = 50"), "{}", host.config());
    assert_eq!(host.played(), [(Sound::Success, 60), (Sound::Success, 50)]);
    // Test plays the jingle at the set volume.
    h.get_by_label("Test").click();
    h.run();
    assert_eq!(host.played(), [(Sound::Success, 50)]);
    // The master switch: off saves and silences, and greys out the rest.
    h.get_by_label("Play sounds").click();
    h.run();
    assert!(
        host.config().contains("enabled = false"),
        "{}",
        host.config()
    );
    assert_eq!(host.played(), []);
    h.get_by_label("Writing").click();
    h.run();
    assert!(!host.config().contains("writing"), "disabled while off");
    h.get_by_label("Play sounds").click();
    h.run();
    assert!(
        host.config().contains("enabled = true"),
        "{}",
        host.config()
    );
    assert_eq!(host.played(), [(Sound::Success, 50)]);
}

#[test]
fn the_wizard_plays_only_the_sounds_that_are_on() {
    for (name, settings, want) in [
        (
            "sounds-some",
            "writing = false\nvolume = 30\n",
            &[(Sound::Waiting, 30), (Sound::Success, 30)][..],
        ),
        ("sounds-off", "enabled = false\n", &[][..]),
    ] {
        let (host, mut run_loop) = host_with_loop(name);
        let text = format!("{}\n[settings.sounds]\n{settings}", host.config());
        std::fs::write(host.dir.join("config.toml"), text).unwrap();
        let mut h = harness(host.clone(), 1.0);
        h.run();
        pick_vampire(h.state_mut());
        run_loop.pump(1);
        h.run();
        run_loop.line("insert 04a1b2c3d4e5f6");
        run_loop.pump(2);
        h.run();
        run_loop.pump(1);
        h.run();
        h.get_by_label("SAVED");
        assert_eq!(host.played(), want, "{name}");
    }
}

#[test]
fn a_failed_write_buzzes() {
    let (host, mut run_loop) = host_with_loop("sounds-failure");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    pick_vampire(h.state_mut());
    run_loop.pump(1);
    h.run();
    // The tag is lifted before the write: back to the tap step.
    run_loop.line("insert 04a1b2c3d4e5f6");
    run_loop.pump(2);
    run_loop.line("remove");
    run_loop.pump(1);
    h.run();
    assert!(matches!(
        h.state().wizard.as_ref().unwrap().step,
        Step::Writing { .. }
    ));
    run_loop.pump(1);
    h.run();
    assert_eq!(h.state().wizard.as_ref().unwrap().step, Step::Tap);
    let cues = [Sound::Waiting, Sound::Writing, Sound::Failure];
    assert_eq!(host.played(), cues.map(|c| (c, 60)));
}

#[test]
fn the_library_pauses_and_resumes_launching() {
    let host = host("pause");
    let mut h = harness(host.clone(), 1.0);
    h.run();
    h.get_by_label("Pause").click();
    h.run();
    assert!(host.paused.load(Ordering::SeqCst));
    h.get_by_label("Resume").click();
    h.run();
    assert!(!host.paused.load(Ordering::SeqCst));
}

/// `cargo test -p mini-launcher-ui -- --ignored` writes the PNGs (needs a
/// wgpu adapter; on Linux, Mesa's lavapipe works).
#[test]
#[ignore = "renders through wgpu; run by hand to refresh docs/design/"]
fn design_screenshots() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design");
    std::fs::create_dir_all(&out).unwrap();
    type Setup = fn(&mut Settings);
    let shots: [(&str, Setup); 19] = [
        ("library", |_| {}),
        ("edit-game", |s| {
            s.edit(0);
            s.launch_form();
        }),
        ("add-game-errors", |s| {
            // The edit form for a new entry (Add game opens the wizard).
            s.screen = Screen::Edit;
            s.form.steam = false;
            s.form.args = "-iwad \"doom2.wad".into();
            s.save();
        }),
        ("remove-game", |s| s.ask_remove(0)),
        ("wizard-1-pick", |s| {
            s.add();
            s.wizard.as_mut().unwrap().selected = Some(LaunchPlan::Steam { appid: 1794680 });
        }),
        ("wizard-1-program", |s| {
            s.add();
            s.wizard_program();
            s.form.name = "DOOM II (GZDoom)".into();
            s.form.exe = "C:\\Games\\GZDoom\\gzdoom.exe".into();
            s.form.args = "-iwad doom2.wad".into();
        }),
        ("wizard-1-emulator-file", |s| {
            emulator_wizard(s);
            s.picked(Pick::GameFolder, config_dir(s).join("Games"));
            let w = s.wizard.as_mut().unwrap();
            while w.poll_scan() {
                std::thread::sleep(Duration::from_millis(5));
            }
            // Shown as a Windows folder, whatever OS renders this.
            let (dir, found) = w.emu.scan.as_mut().unwrap();
            *dir = PathBuf::from(GAMES_DIR);
            for rom in &mut found.roms {
                let file = rom.path.file_name().unwrap().to_string_lossy();
                rom.path = PathBuf::from(format!("{GAMES_DIR}\\{file}"));
            }
            w.emu.rom = format!("{GAMES_DIR}\\Sky Drifter [0100AB][v0].nsp");
        }),
        ("wizard-1-emulator-name", |s| {
            emulator_wizard(s);
            let rom = format!("{GAMES_DIR}\\Sky Drifter [0100AB][v0].nsp");
            s.wizard.as_mut().unwrap().emu.rom = rom;
            s.wizard_next();
        }),
        ("wizard-2-tap", |s| {
            pick_vampire(s);
            s.wizard.as_mut().unwrap().reader = Some(true);
        }),
        ("wizard-2-overwrite", |s| {
            pick_vampire(s);
            let w = s.wizard.as_mut().unwrap();
            w.reader = Some(true);
            w.step = Step::Overwrite {
                uid: tag_uid(),
                question: "This tag holds \"Portal 2\" (steam://620). It is bound to \
                           \"Portal 2\" in your library; that game loses this tag. Use it \
                           for \"Vampire Survivors\"?"
                    .into(),
            };
        }),
        ("wizard-3-writing", |s| {
            pick_vampire(s);
            let w = s.wizard.as_mut().unwrap();
            w.reader = Some(true);
            w.step = Step::Writing { uid: tag_uid() };
        }),
        ("wizard-4-written", |s| {
            pick_vampire(s);
            let w = s.wizard.as_mut().unwrap();
            w.step = Step::Test {
                outcome: Outcome::Written { uid: tag_uid() },
            };
        }),
        ("wizard-4-uid-fallback", |s| {
            pick_vampire(s);
            let w = s.wizard.as_mut().unwrap();
            w.step = Step::Test {
                outcome: Outcome::Bound {
                    uid: tag_uid(),
                    failure: WriteFailure::Locked,
                },
            };
        }),
        ("import-confirm", |s| {
            let dir = config_dir(s);
            let out = dir.join("mini-launcher-backup-2026-11-17.toml");
            s.export_to(out.clone());
            s.import_from(out);
        }),
        ("emulators", |s| {
            let eden = EmuForm::from_preset(&EDEN, Some(r"C:\Emulators\Eden\eden.exe".into()));
            let eden = eden.to_emulator(s.library.config().unwrap()).unwrap();
            s.library.save_emulator(&eden).unwrap();
            s.open_emulators();
        }),
        ("edit-emulator", |s| {
            s.add_emulator(&EDEN);
            s.emulators.form.exe.clear();
            s.emulators.form.args = "-f -g".into();
            s.save_emulator();
        }),
        ("settings", |s| s.open_prefs()),
        ("config-folder", |s| s.open_folder()),
        ("config-folder-existing", |s| {
            let synced = config_dir(s).parent().unwrap().join("OneDrive");
            std::fs::create_dir_all(&synced).unwrap();
            std::fs::write(synced.join("config.toml"), "").unwrap();
            s.move_to(synced);
        }),
    ];
    for (name, setup) in shots {
        for scale in [1.0, 2.0] {
            let mut h = harness(design_host(), scale);
            setup(h.state_mut());
            h.run();
            let image = h.render().expect("render");
            let file = out.join(format!("{name}@{scale}x.png"));
            image.save(&file).unwrap();
        }
    }
}

fn tag_uid() -> Uid {
    Uid::from_hex("04A1B2C3D4E5F6").unwrap()
}

/// The wizard on the emulator game's file step, with Eden and a folder of
/// (empty, fictional) game files.
fn emulator_wizard(s: &mut Settings) {
    let eden = EmuForm::from_preset(&EDEN, Some(r"C:\Emulators\Eden\eden.exe".into()));
    let eden = eden.to_emulator(s.library.config().unwrap()).unwrap();
    s.library.save_emulator(&eden).unwrap();
    let roms = config_dir(s).join("Games");
    std::fs::create_dir_all(&roms).unwrap();
    for name in [
        "Sky Drifter [0100AB][v0].nsp",
        "Tile Tumble (USA).xci",
        "Moss Garden Deluxe [v65536].nsp",
    ] {
        std::fs::write(roms.join(name), b"").unwrap();
    }
    s.add();
    s.wizard_emulator();
}

/// The game folder the emulator screenshots show.
const GAMES_DIR: &str = r"D:\Games";

/// Lets the wizard's folder scan finish.
fn finish_scan(h: &mut Harness<'static, Settings>) {
    let start = Instant::now();
    loop {
        h.run();
        if h.state().wizard.as_ref().unwrap().scanning().is_none() {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "scan never ended"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The wizard on the tap step for Vampire Survivors.
fn pick_vampire(s: &mut Settings) {
    s.add();
    s.wizard.as_mut().unwrap().selected = Some(LaunchPlan::Steam { appid: 1794680 });
    s.wizard_next();
}
