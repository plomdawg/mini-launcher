//! Drives the mock screens headlessly, and (with `--ignored`) renders the
//! design screenshots in `docs/design/` through wgpu.

use egui_kittest::{kittest::Queryable, Harness};
use mini_launcher_ui::mock::{self, Screen, Settings};
use mini_launcher_ui::theme;

const SIZE: [f32; 2] = [640.0, 440.0];

fn harness(screen: Screen, scale: f32) -> Harness<'static, Settings> {
    Harness::builder()
        .with_size(SIZE)
        .with_pixels_per_point(scale)
        .with_context_setup(theme::install)
        .build_ui_state(
            |ui, state| {
                // Fill the whole surface like the real window (the harness adds a margin).
                let full = egui::UiBuilder::new().max_rect(ui.ctx().content_rect());
                ui.scope_builder(full, |ui| mock::show(ui, state));
            },
            Settings::new(screen),
        )
}

#[test]
fn library_lists_games_and_opens_the_form() {
    let mut h = harness(Screen::Library, 1.0);
    h.run();
    h.get_by_label("Hollow Knight");
    for label in ["Add game", "Remove", "Export", "Import"] {
        h.get_by_label(label);
    }
    h.get_by_label("Edit").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Edit);
}

#[test]
fn edit_form_has_labelled_fields_and_returns() {
    let mut h = harness(Screen::Edit, 1.0);
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
    h.get_by_label("Program").click();
    h.run();
    h.get_by_label("Cancel").click();
    h.run();
    assert_eq!(h.state().screen, Screen::Library);
}

/// `cargo test -p mini-launcher-ui -- --ignored` writes the PNGs (needs a
/// wgpu adapter; on Linux, Mesa's lavapipe works).
#[test]
#[ignore = "renders through wgpu; run by hand to refresh docs/design/"]
fn design_screenshots() {
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design");
    std::fs::create_dir_all(&out).unwrap();
    for (screen, name) in [(Screen::Library, "library"), (Screen::Edit, "edit-game")] {
        for scale in [1.0, 2.0] {
            let mut h = harness(screen, scale);
            h.run();
            let image = h.render().expect("render");
            let file = out.join(format!("{name}@{scale}x.png"));
            image.save(&file).unwrap();
        }
    }
}
