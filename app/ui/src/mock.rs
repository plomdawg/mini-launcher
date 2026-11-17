//! Static mock screens with fake data, for signing off the look (#64).
//! Nothing here reads or writes the real config yet.

use egui::{pos2, vec2, Align, Layout, Rect, Sense, Ui};

use crate::theme::{self, BIG, BLUE, FACE, INK, PAPER, ROW, SHADE, SIGNAL, TEXT, U};

/// Fake library: name, Steam app ID (`None` = a program), bound tag UID.
pub const GAMES: &[(&str, Option<u32>, Option<&str>)] = &[
    ("Hades", Some(1145360), Some("04:A2:3B:1C:5D:80:00")),
    ("Celeste", Some(504230), Some("04:19:7E:22:6A:80:00")),
    ("Stardew Valley", Some(413150), Some("04:C4:01:9A:3F:81:00")),
    ("Hollow Knight", Some(367520), None),
    ("Balatro", Some(2379780), Some("04:5B:E2:70:11:80:00")),
    ("Outer Wilds", Some(753640), Some("04:8D:36:C9:02:81:00")),
    ("DOOM (GZDoom)", None, Some("04:77:A0:4E:B8:80:00")),
    ("Factorio", Some(427520), None),
    ("Portal 2", Some(620), Some("04:E1:5C:93:27:81:00")),
    (
        "Into the Breach",
        Some(590380),
        Some("04:30:BB:68:D4:80:00"),
    ),
    ("Slay the Spire", Some(646570), None),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Library,
    Edit,
}

pub struct Settings {
    pub screen: Screen,
    pub selected: Option<usize>,
    form: Form,
}

/// The edit form's fields.
struct Form {
    name: String,
    steam: bool,
    app_id: String,
    exe: String,
    args: String,
    cwd: String,
}

impl Settings {
    pub fn new(screen: Screen) -> Self {
        let form = Form {
            name: "Hades".into(),
            steam: true,
            app_id: "1145360".into(),
            exe: String::new(),
            args: "-skipintro".into(),
            cwd: String::new(),
        };
        let selected = Some(0);
        Self {
            screen,
            selected,
            form,
        }
    }
}

/// The whole window: masthead, screen, status bar.
pub fn show(ui: &mut Ui, s: &mut Settings) {
    let title = match s.screen {
        Screen::Library => "LIBRARY",
        Screen::Edit => "EDIT GAME",
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
        .show(ui, status_bar);
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

fn status_bar(ui: &mut Ui) {
    let bound = GAMES.iter().filter(|g| g.2.is_some()).count();
    let cells = [
        format!("{} games", GAMES.len()),
        format!("{bound} tagged"),
        "Reader COM3: ready".to_owned(),
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
                x = theme::mark(ui.painter(), rect, x, true, INK);
            }
            theme::label_at(ui, rect, x, cell, INK);
        }
    });
}

fn library(ui: &mut Ui, s: &mut Settings) {
    let has = s.selected.is_some();
    ui.horizontal(|ui| {
        if theme::button(ui, "Add game", true, false).clicked() {
            s.selected = None;
            s.screen = Screen::Edit;
        }
        if theme::button(ui, "Edit", has, false).clicked() {
            s.screen = Screen::Edit;
        }
        theme::button(ui, "Remove", has, false);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            theme::button(ui, "Import", true, false);
            theme::button(ui, "Export", true, false);
        });
    });
    ui.add_space(U);
    let rect = theme::snap(ui.available_rect_before_wrap());
    theme::sunken(ui.painter(), rect, PAPER);
    let inner = rect.shrink(2.0);
    // Columns: name | launch | tag. Widths are in characters of the mono font.
    let cols = [
        inner.min.x,
        inner.min.x + inner.width() * 0.40,
        inner.min.x + inner.width() * 0.66,
    ];
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), ROW));
    for (i, label) in ["NAME", "LAUNCH", "TAG"].iter().enumerate() {
        let end = cols.get(i + 1).copied().unwrap_or(inner.max.x);
        let cell = Rect::from_min_max(pos2(cols[i], header.min.y), pos2(end, header.max.y));
        theme::raised(ui.painter(), cell);
        theme::label_at(ui, cell, cell.min.x + 2.0 * U, label, INK);
    }
    for (i, &(name, app_id, tag)) in GAMES.iter().enumerate() {
        let top = header.max.y + i as f32 * ROW;
        let row = theme::snap(Rect::from_min_size(
            pos2(inner.min.x, top),
            vec2(inner.width(), ROW),
        ));
        if row.max.y > inner.max.y {
            break;
        }
        let resp = ui.interact(row, ui.scope_id().with(("game", i)), Sense::click());
        let on = s.selected == Some(i);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::ListItem, true, on, name));
        if resp.clicked() {
            s.selected = Some(i);
        }
        if resp.double_clicked() {
            s.screen = Screen::Edit;
        }
        let (fg, weak) = if on { (PAPER, PAPER) } else { (INK, SHADE) };
        if on {
            ui.painter().rect_filled(row, 0.0, BLUE);
        }
        let launch = app_id.map_or("Program".into(), |id| format!("Steam {id}"));
        theme::label_at(ui, row, cols[0] + 2.0 * U, name, fg);
        theme::label_at(ui, row, cols[1] + 2.0 * U, &launch, fg);
        let x = theme::mark(ui.painter(), row, cols[2] + 2.0 * U, tag.is_some(), fg);
        theme::label_at(
            ui,
            row,
            x,
            tag.unwrap_or("not bound"),
            if tag.is_some() { fg } else { weak },
        );
    }
}

fn edit(ui: &mut Ui, s: &mut Settings) {
    let f = &mut s.form;
    let label_w = 22.0 * U;
    let field_w = |ui: &Ui, button: bool| {
        (ui.available_width() - if button { 20.0 * U } else { 0.0 }).floor()
    };
    theme::group(ui, "Game", |ui| {
        let row = |ui: &mut Ui, label: &str, add: &mut dyn FnMut(&mut Ui, egui::Id)| {
            ui.horizontal(|ui| {
                let l = label_cell(ui, label_w, label);
                add(ui, l.id);
            });
        };
        row(ui, "Name", &mut |ui, id| {
            theme::field(ui, &mut f.name, field_w(ui, false), true, id);
        });
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
        row(ui, "Exe path", &mut |ui, id| {
            theme::field(ui, &mut f.exe, field_w(ui, true), !steam, id);
            theme::button(ui, "Browse", !steam, false);
        });
        row(ui, "Arguments", &mut |ui, id| {
            theme::field(ui, &mut f.args, field_w(ui, false), true, id);
        });
        row(ui, "Working dir", &mut |ui, id| {
            theme::field(ui, &mut f.cwd, field_w(ui, true), !steam, id);
            theme::button(ui, "Browse", !steam, false);
        });
    });
    ui.add_space(U);
    theme::group(ui, "Tag", |ui| {
        ui.horizontal(|ui| {
            label_cell(ui, label_w, "Bound to");
            let (well, _) = ui.allocate_exact_size(vec2(field_w(ui, true), ROW), Sense::hover());
            theme::sunken(ui.painter(), well, FACE);
            let x = theme::mark(ui.painter(), well, well.min.x + 2.0 * U, true, INK);
            theme::label_at(ui, well, x, "04:A2:3B:1C:5D:80:00  steam://1145360", INK);
            theme::button(ui, "Unbind", true, false);
        });
        ui.horizontal(|ui| {
            ui.add_space(label_w + ui.spacing().item_spacing.x);
            theme::button(ui, "Write tag...", true, false);
            ui.colored_label(SHADE, "then hold the cart on the reader");
        });
    });
    ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::button(ui, "Cancel", true, false).clicked() {
                    s.screen = Screen::Library;
                }
                if theme::button(ui, "Save", true, true).clicked() {
                    s.screen = Screen::Library;
                }
            });
        });
    });
}

/// A left-aligned form label in a fixed-width column.
fn label_cell(ui: &mut Ui, width: f32, text: &str) -> egui::Response {
    let layout = Layout::left_to_right(Align::Center);
    ui.allocate_ui_with_layout(vec2(width, ROW), layout, |ui| {
        ui.set_min_width(width);
        ui.add(egui::Label::new(text).selectable(false))
    })
    .inner
}
