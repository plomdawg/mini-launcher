//! Drives the settings screens headlessly on a temporary config, and (with
//! `--ignored`) renders the design screenshots in `docs/design/` through wgpu.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use egui_kittest::{kittest::Queryable, Harness};
use mini_launcher_core::config::location::ConfigLocation;
use mini_launcher_ui::settings::{self, Host, ReaderStatus, Screen, Settings};
use mini_launcher_ui::theme;

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

struct TestHost {
    dir: PathBuf,
    saves: AtomicUsize,
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
}

impl TestHost {
    fn config(&self) -> String {
        std::fs::read_to_string(self.dir.join("config.toml")).unwrap()
    }
}

fn host(name: &str) -> Arc<TestHost> {
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
    Arc::new(TestHost {
        dir,
        saves: AtomicUsize::new(0),
    })
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
    h.get_by_label("Program").click();
    h.run();
    h.get_by_label("Save").click();
    h.run();
    h.get_by_label("Enter a name.");
    h.get_by_label("Choose the program to start.");
    assert_eq!(h.state().screen, Screen::Edit);
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

/// `cargo test -p mini-launcher-ui -- --ignored` writes the PNGs (needs a
/// wgpu adapter; on Linux, Mesa's lavapipe works).
#[test]
#[ignore = "renders through wgpu; run by hand to refresh docs/design/"]
fn design_screenshots() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design");
    std::fs::create_dir_all(&out).unwrap();
    type Setup = fn(&mut Settings);
    let shots: [(&str, Setup); 4] = [
        ("library", |_| {}),
        ("edit-game", |s| s.edit(0)),
        ("add-game-errors", |s| {
            s.add();
            s.form.steam = false;
            s.form.args = "-iwad \"doom2.wad".into();
            s.save();
        }),
        ("remove-game", |s| s.confirm_remove = Some(0)),
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
