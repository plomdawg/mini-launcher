//! Drives the settings screens headlessly on a temporary config, and (with
//! `--ignored`) renders the design screenshots in `docs/design/` through wgpu.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::{kittest::Queryable, Harness};
use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_core::config::Config;
use mini_launcher_core::launch::LaunchPlan;
use mini_launcher_core::reader::{Simulated, Uid};
use mini_launcher_core::reconnect::{Backoff, Reconnector};
use mini_launcher_core::runner::{Notice, Runner};
use mini_launcher_core::session::{self, SessionServer, Sessions, WriteFailure};
use mini_launcher_core::steam::SteamGame;
use mini_launcher_ui::settings::{self, Host, ReaderStatus, Screen, Settings};
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
}

impl Host for TestHost {
    fn location(&self) -> Option<ConfigLocation> {
        Some(ConfigLocation::Default(self.dir.clone()))
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
            })
            .collect())
    }
    fn launch(&self, plan: &LaunchPlan) -> Result<(), String> {
        self.launches.lock().unwrap().push(plan.clone());
        Ok(())
    }
}

impl TestHost {
    fn config(&self) -> String {
        std::fs::read_to_string(self.dir.join("config.toml")).unwrap()
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
    let dir = std::env::temp_dir().join(format!(
        "mini-launcher-screens-{name}-{}",
        std::process::id()
    ));
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
    assert_eq!(host.config(), before);
    assert_eq!(host.saves.load(Ordering::SeqCst), 0);
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
    assert!(
        matches!(&run_loop.notices[..], [Notice::Paused { .. }]),
        "{:?}",
        run_loop.notices
    );
    // Saving gave the reader back; the run loop notices on its next tick.
    run_loop.pump(1);
    assert!(!run_loop.server.is_active());
    assert!(host.config().contains("steam_appid = 1794680"));
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

/// `cargo test -p mini-launcher-ui -- --ignored` writes the PNGs (needs a
/// wgpu adapter; on Linux, Mesa's lavapipe works).
#[test]
#[ignore = "renders through wgpu; run by hand to refresh docs/design/"]
fn design_screenshots() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design");
    std::fs::create_dir_all(&out).unwrap();
    type Setup = fn(&mut Settings);
    let shots: [(&str, Setup); 11] = [
        ("library", |_| {}),
        ("edit-game", |s| s.edit(0)),
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
            s.wizard.as_mut().unwrap().selected = Some(1794680);
        }),
        ("wizard-1-program", |s| {
            s.add();
            s.wizard_program();
            s.form.name = "DOOM II (GZDoom)".into();
            s.form.exe = "C:\\Games\\GZDoom\\gzdoom.exe".into();
            s.form.args = "-iwad doom2.wad".into();
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
    ];
    for (name, setup) in shots {
        for scale in [1.0, 2.0] {
            let mut h = harness(host(name), scale);
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

/// The wizard on the tap step for Vampire Survivors.
fn pick_vampire(s: &mut Settings) {
    s.add();
    s.wizard.as_mut().unwrap().selected = Some(1794680);
    s.wizard_next();
}
