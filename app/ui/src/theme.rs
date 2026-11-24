//! The retro look: a five-colour palette taken from the app icon, one bitmap
//! font, square corners, hard 1-point outlines and bevels, no shadows, no
//! anti-aliased edges and no animation.
//!
//! Everything is drawn on whole points, so at 200 % each "pixel" of a bevel
//! is exactly 2×2 device pixels and stays crisp.

use egui::epaint::text::{FontData, FontDefinitions, FontFamily, FontTweak};
use egui::{
    pos2, vec2, Color32, Context, CornerRadius, FontId, Galley, Id, Margin, Painter, Rect,
    Response, Role, Sense, Shadow, Stroke, TextStyle, Ui, Vec2, Visuals, WidgetInfo,
};
use std::sync::Arc;

/// Text, outlines.
pub const INK: Color32 = Color32::from_rgb(0x00, 0x00, 0x00);
/// Fields, list background, bevel highlights.
pub const PAPER: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
/// The icon's disc: masthead, selection.
pub const BLUE: Color32 = Color32::from_rgb(0x0b, 0x6a, 0xa2);
/// Window chrome (a slightly cool grey, so it sits next to the blue).
pub const FACE: Color32 = Color32::from_rgb(0xc6, 0xca, 0xcf);
/// Bevel shadows, disabled text, etched lines.
pub const SHADE: Color32 = Color32::from_rgb(0x6b, 0x73, 0x7b);
/// The one accent: "tag bound" and "reader ready" marks.
pub const SIGNAL: Color32 = Color32::from_rgb(0xff, 0xc4, 0x00);

/// Departure Mono is drawn on an 11 px grid: only multiples of 11 are crisp.
pub const TEXT: f32 = 11.0;
pub const BIG: f32 = 22.0;
/// Base spacing unit; every gap and margin is a multiple of it.
pub const U: f32 = 4.0;
/// Height of buttons, fields and list rows.
pub const ROW: f32 = 5.0 * U;

const FONT: &[u8] = include_bytes!("../../assets/fonts/DepartureMono-Regular.otf");

/// Installs the font and style on a context. Call once, before the first frame.
pub fn install(ctx: &Context) {
    let mut fonts = FontDefinitions::empty();
    let tweak = FontTweak {
        hinting: Some(false),
        subpixel_binning: Some(false),
        ..Default::default()
    };
    fonts.font_data.insert(
        "departure".into(),
        Arc::new(FontData::from_static(FONT).tweak(tweak)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.insert(family, vec!["departure".into()]);
    }
    ctx.set_fonts(fonts);
    // Anti-aliased shape edges would blur the bevels.
    ctx.tessellation_options_mut(|t| t.feathering = false);
    ctx.all_styles_mut(|style| {
        let mono = |size| FontId::monospace(size);
        style.text_styles = [
            (TextStyle::Small, mono(TEXT)),
            (TextStyle::Body, mono(TEXT)),
            (TextStyle::Button, mono(TEXT)),
            (TextStyle::Monospace, mono(TEXT)),
            (TextStyle::Heading, mono(BIG)),
        ]
        .into();
        style.animation_time = 0.0;
        let s = &mut style.spacing;
        s.item_spacing = vec2(2.0 * U, 2.0 * U);
        s.button_padding = vec2(2.0 * U, U);
        s.interact_size.y = ROW;
        s.window_margin = Margin::same(3 * U as i8);
        s.scroll = egui::style::ScrollStyle::solid();
        s.scroll.bar_width = 4.0 * U;
        // A solid ink handle on a paper track (the face-coloured default
        // handle disappears into the window).
        s.scroll.foreground_color = true;
        style.visuals = visuals();
    });
}

fn visuals() -> Visuals {
    let mut v = Visuals::light();
    v.text_options.font_hinting = false;
    v.override_text_color = Some(INK);
    v.panel_fill = FACE;
    v.window_fill = FACE;
    v.extreme_bg_color = PAPER;
    v.faint_bg_color = FACE;
    v.window_stroke = Stroke::new(1.0, INK);
    v.window_corner_radius = CornerRadius::ZERO;
    v.menu_corner_radius = CornerRadius::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.selection.bg_fill = BLUE;
    v.selection.stroke = Stroke::new(1.0, PAPER);
    v.text_cursor.stroke = Stroke::new(1.0, INK);
    v.text_cursor.blink = false;
    v.hyperlink_color = BLUE;
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::ZERO;
        w.expansion = 0.0;
        w.bg_fill = FACE;
        w.weak_bg_fill = FACE;
        w.bg_stroke = Stroke::new(1.0, INK);
        w.fg_stroke = Stroke::new(1.0, INK);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, SHADE);
    v.widgets.active.bg_fill = SHADE;
    v
}

/// `r` with its corners moved to whole points.
pub fn snap(r: Rect) -> Rect {
    Rect::from_min_max(r.min.round(), r.max.round())
}

/// A 1-point frame just inside `r`.
pub fn outline(p: &Painter, r: Rect, color: Color32) {
    bevel(p, r, color, color);
}

/// 1-point `light` edges on top/left and `dark` edges on bottom/right.
pub fn bevel(p: &Painter, r: Rect, light: Color32, dark: Color32) {
    let r = snap(r);
    let (a, b) = (r.min, r.max);
    p.rect_filled(Rect::from_min_max(a, pos2(b.x, a.y + 1.0)), 0.0, light);
    p.rect_filled(Rect::from_min_max(a, pos2(a.x + 1.0, b.y)), 0.0, light);
    p.rect_filled(Rect::from_min_max(pos2(a.x, b.y - 1.0), b), 0.0, dark);
    p.rect_filled(Rect::from_min_max(pos2(b.x - 1.0, a.y), b), 0.0, dark);
}

/// An outlined, raised panel (buttons, column headers).
pub fn raised(p: &Painter, r: Rect) {
    let r = snap(r);
    p.rect_filled(r, 0.0, FACE);
    outline(p, r, INK);
    bevel(p, r.shrink(1.0), PAPER, SHADE);
}

/// An outlined, sunken well (fields, the list) filled with `fill`.
pub fn sunken(p: &Painter, r: Rect, fill: Color32) {
    let r = snap(r);
    p.rect_filled(r, 0.0, fill);
    outline(p, r, INK);
    bevel(p, r.shrink(1.0), SHADE, fill);
}

/// The classic dotted keyboard-focus rectangle.
pub fn focus_dots(p: &Painter, r: Rect) {
    let r = snap(r);
    let dot =
        |x: f32, y: f32| p.rect_filled(Rect::from_min_size(pos2(x, y), Vec2::splat(1.0)), 0.0, INK);
    let mut x = r.min.x;
    while x < r.max.x {
        dot(x, r.min.y);
        dot(x, r.max.y - 1.0);
        x += 2.0;
    }
    let mut y = r.min.y;
    while y < r.max.y {
        dot(r.min.x, y);
        dot(r.max.x - 1.0, y);
        y += 2.0;
    }
}

pub fn galley(ui: &Ui, text: &str, size: f32, color: Color32) -> Arc<Galley> {
    ui.painter()
        .layout_no_wrap(text.to_owned(), FontId::monospace(size), color)
}

/// Text placed on whole points (keeps the bitmap glyphs crisp).
pub fn text(p: &Painter, pos: egui::Pos2, galley: Arc<Galley>) {
    p.galley(pos.round(), galley, INK);
}

/// One line of text at `x`, centred vertically on `row`.
pub fn label_at(ui: &Ui, row: Rect, x: f32, label: &str, color: Color32) {
    let g = galley(ui, label, TEXT, color);
    let pos = pos2(x, row.center().y - g.size().y / 2.0).round();
    ui.painter().galley(pos, g, color);
}

/// [`label_at`], cut short with "…" where it would pass `right` (the next
/// column).
pub fn label_fit(ui: &Ui, row: Rect, x: f32, right: f32, label: &str, color: Color32) {
    let mut job =
        egui::text::LayoutJob::simple_singleline(label.to_owned(), FontId::monospace(TEXT), color);
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(0.0));
    let g = ui.painter().layout_job(job);
    let pos = pos2(x, row.center().y - g.size().y / 2.0).round();
    ui.painter().galley(pos, g, color);
}

/// A 6-point status square at `x` on `row`, filled with [`SIGNAL`] when
/// `on`. Returns where the text after it starts.
pub fn mark(p: &Painter, row: Rect, x: f32, on: bool, edge: Color32) -> f32 {
    let m = snap(Rect::from_min_size(
        pos2(x, row.center().y - 3.0),
        Vec2::splat(6.0),
    ));
    if on {
        p.rect_filled(m, 0.0, SIGNAL);
    }
    outline(p, m, edge);
    m.max.x + 2.0 * U
}

/// Disabled text: shade with a 1-point white emboss.
fn embossed(p: &Painter, pos: egui::Pos2, g: Arc<Galley>) {
    p.galley_with_override_text_color(pos.round() + vec2(1.0, 1.0), g.clone(), PAPER);
    p.galley_with_override_text_color(pos.round(), g, SHADE);
}

/// A push button. `default` draws the thick outline of the dialog's default action.
pub fn button(ui: &mut Ui, label: &str, enabled: bool, default: bool) -> Response {
    let g = galley(ui, label, TEXT, INK);
    let width = (g.size().x + 4.0 * U).max(18.0 * U).round();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW), sense);
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, label));
    let p = ui.painter();
    let down = enabled && resp.is_pointer_button_down_on();
    let mut face = snap(rect);
    if default {
        outline(p, face, INK);
        face = face.shrink(1.0);
    }
    p.rect_filled(face, 0.0, FACE);
    outline(p, face, INK);
    if down {
        bevel(p, face.shrink(1.0), SHADE, FACE);
    } else {
        bevel(p, face.shrink(1.0), PAPER, SHADE);
    }
    let shift = if down { vec2(1.0, 1.0) } else { Vec2::ZERO };
    let pos = face.center() - g.size() / 2.0 + shift;
    if enabled {
        text(p, pos, g);
    } else {
        embossed(p, pos, g);
    }
    if resp.has_focus() {
        focus_dots(p, face.shrink(3.0));
    }
    resp
}

/// A one-line text field in a sunken well, `width` points wide.
pub fn field(ui: &mut Ui, value: &mut String, width: f32, enabled: bool, label: Id) -> Response {
    let (rect, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
    sunken(ui.painter(), rect, if enabled { PAPER } else { FACE });
    let edit = egui::TextEdit::singleline(value)
        .frame(egui::Frame::NONE)
        .margin(Margin::ZERO)
        .text_color(if enabled { INK } else { SHADE })
        .desired_width(width - 3.0 * U);
    let inner = Rect::from_center_size(rect.center(), vec2(width - 3.0 * U, TEXT + 2.0));
    ui.place(inner, |ui: &mut Ui| {
        // Don't let the edit's minimum height push the row taller than ROW.
        ui.spacing_mut().interact_size.y = TEXT;
        ui.add_enabled(enabled, edit)
    })
    .labelled_by(label)
}

/// A left-aligned form label in a fixed-width column; pass its id to the
/// field's `labelled_by`.
pub fn form_label(ui: &mut Ui, width: f32, text: &str) -> Response {
    let layout = egui::Layout::left_to_right(egui::Align::Center);
    ui.allocate_ui_with_layout(vec2(width, ROW), layout, |ui| {
        ui.set_min_width(width);
        ui.add(egui::Label::new(text).selectable(false))
    })
    .inner
}

/// Raised column headers across `row`; column `i` spans `cols[i]..cols[i + 1]`
/// (the last one runs to the row's end).
pub fn column_headers(ui: &Ui, row: Rect, cols: &[f32], labels: &[&str]) {
    for (i, label) in labels.iter().enumerate() {
        let end = cols.get(i + 1).copied().unwrap_or(row.max.x);
        let cell = Rect::from_min_max(pos2(cols[i], row.min.y), pos2(end, row.max.y));
        raised(ui.painter(), cell);
        label_at(ui, cell, cell.min.x + 2.0 * U, label, INK);
    }
}

/// A selectable list row: senses clicks, reports itself to screen readers
/// and paints the selection. Returns the response and the text colours to
/// draw its cells with (normal, weak).
pub fn list_row(
    ui: &Ui,
    row: Rect,
    id: impl egui::AsIdSalt,
    selected: bool,
    label: &str,
) -> (Response, Color32, Color32) {
    let row = snap(row);
    let resp = ui.interact(row, ui.scope_id().with(id), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::ListItem, true, selected, label));
    if selected {
        ui.painter().rect_filled(row, 0.0, BLUE);
        (resp, PAPER, PAPER)
    } else {
        (resp, INK, SHADE)
    }
}

/// A square radio choice (round marks don't survive a pixel grid).
pub fn choice(ui: &mut Ui, selected: bool, label: &str) -> Response {
    let g = galley(ui, label, TEXT, INK);
    let size = vec2(3.0 * U + U + g.size().x, ROW);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::RadioButton, true, selected, label));
    let p = ui.painter();
    let mark = snap(Rect::from_min_size(
        pos2(rect.min.x, rect.center().y - 6.0),
        Vec2::splat(12.0),
    ));
    sunken(p, mark, PAPER);
    if selected {
        p.rect_filled(mark.shrink(3.0), 0.0, INK);
    }
    let pos = pos2(mark.max.x + U, rect.center().y - g.size().y / 2.0);
    text(p, pos, g);
    if resp.has_focus() {
        focus_dots(
            p,
            Rect::from_min_max(pos - vec2(2.0, 2.0), rect.max + vec2(2.0, 0.0)),
        );
    }
    resp
}

/// A checkbox: the [`choice`] well with a pixel tick when `on`.
pub fn check(ui: &mut Ui, on: bool, label: &str, enabled: bool) -> Response {
    let g = galley(ui, label, TEXT, INK);
    let size = vec2(3.0 * U + U + g.size().x, ROW);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    resp.widget_info(|| WidgetInfo::selected(Role::CheckBox, enabled, on, label));
    let p = ui.painter();
    let well = snap(Rect::from_min_size(
        pos2(rect.min.x, rect.center().y - 6.0),
        Vec2::splat(12.0),
    ));
    sunken(p, well, if enabled { PAPER } else { FACE });
    if on {
        // A 2-point stroke tick, drawn as squares on the point grid.
        let ink = if enabled { INK } else { SHADE };
        let o = well.min;
        for (x, y) in [(3, 6), (4, 7), (5, 8), (6, 7), (7, 6), (8, 5), (9, 4)] {
            let at = o + vec2(x as f32 - 0.5, y as f32 - 1.5);
            p.rect_filled(Rect::from_min_size(at.round(), Vec2::splat(2.0)), 0.0, ink);
        }
    }
    let pos = pos2(well.max.x + U, rect.center().y - g.size().y / 2.0);
    if enabled {
        text(p, pos, g);
    } else {
        embossed(p, pos, g);
    }
    if resp.has_focus() {
        focus_dots(
            p,
            Rect::from_min_max(pos - vec2(2.0, 2.0), rect.max + vec2(2.0, 0.0)),
        );
    }
    resp
}

/// An etched group box with its title set into the top line.
pub fn group<R>(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.add_space(U);
    let margin = Margin {
        left: (3.0 * U) as i8,
        right: (3.0 * U) as i8,
        top: (4.0 * U) as i8,
        bottom: (3.0 * U) as i8,
    };
    let out = egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        add(ui)
    });
    let r = snap(out.response.rect);
    let p = ui.painter();
    outline(p, r.translate(vec2(1.0, 1.0)), PAPER);
    outline(p, r, SHADE);
    let g = galley(ui, title, TEXT, INK);
    let at = pos2(r.min.x + 2.0 * U, r.min.y - (g.size().y / 2.0).round());
    p.rect_filled(
        snap(Rect::from_min_size(at, g.size() + vec2(2.0 * U, 0.0))),
        0.0,
        FACE,
    );
    text(p, at + vec2(U, 0.0), g);
    out.inner
}

/// A sunken list with raised column headers and scrolling rows. Columns
/// start at the fractions `cols` of the list's width. `row` paints row `i`
/// into its rect (see [`list_row`]). Returns the rows' area.
pub fn list(
    ui: &mut Ui,
    cols: &[f32],
    headers: &[&str],
    rows: usize,
    row: impl FnMut(&mut Ui, usize, Rect, &[f32]),
) -> Rect {
    list_with(ui, cols, headers, None, rows, row)
}

/// How a sortable list is sorted: by column `column` (an index into its
/// headers), A to Z unless `descending`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sort {
    pub column: usize,
    pub descending: bool,
}

impl Sort {
    /// A header click: sort by `column`, or reverse if it already is.
    pub fn click(&mut self, column: usize) {
        if self.column == column {
            self.descending = !self.descending;
        } else {
            *self = Sort {
                column,
                descending: false,
            };
        }
    }
}

/// Like [`list`], but the headers of the columns in `sortable` are buttons
/// that change `sort`; the sorted column shows an arrow. The caller sorts
/// the rows.
pub fn sortable_list(
    ui: &mut Ui,
    cols: &[f32],
    headers: &[&str],
    sort: (&mut Sort, &[usize]),
    rows: usize,
    row: impl FnMut(&mut Ui, usize, Rect, &[f32]),
) -> Rect {
    list_with(ui, cols, headers, Some(sort), rows, row)
}

fn list_with(
    ui: &mut Ui,
    cols: &[f32],
    headers: &[&str],
    sort: Option<(&mut Sort, &[usize])>,
    rows: usize,
    mut row: impl FnMut(&mut Ui, usize, Rect, &[f32]),
) -> Rect {
    let rect = snap(ui.available_rect_before_wrap());
    ui.allocate_rect(rect, Sense::hover());
    sunken(ui.painter(), rect, PAPER);
    let inner = rect.shrink(2.0);
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), ROW));
    let body = Rect::from_min_max(pos2(inner.min.x, header.max.y), inner.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
    child.set_clip_rect(body);
    // Rows touch; show_rows sizes the content with this spacing.
    child.spacing_mut().item_spacing.y = 0.0;
    // Column positions follow the whole list, so they don't move when a
    // scroll bar appears.
    let x: Vec<f32> = cols.iter().map(|f| body.min.x + body.width() * f).collect();
    egui::ScrollArea::vertical().auto_shrink(false).show_rows(
        &mut child,
        ROW,
        rows,
        |ui, range| {
            let width = ui.available_width();
            for i in range {
                let (r, _) = ui.allocate_exact_size(vec2(width, ROW), Sense::hover());
                row(ui, i, r, &x);
            }
        },
    );
    column_headers(ui, header, &x, headers);
    if let Some((sort, sortable)) = sort {
        sort_headers(ui, header, &x, headers, sort, sortable);
    }
    body
}

/// Makes the `sortable` headers clickable and marks the sorted one.
fn sort_headers(
    ui: &Ui,
    row: Rect,
    cols: &[f32],
    labels: &[&str],
    sort: &mut Sort,
    sortable: &[usize],
) {
    for &i in sortable.iter().filter(|&&i| i < labels.len()) {
        let end = cols.get(i + 1).copied().unwrap_or(row.max.x);
        let cell = Rect::from_min_max(pos2(cols[i], row.min.y), pos2(end, row.max.y));
        let resp = ui.interact(cell, ui.scope_id().with(("sort", i)), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, labels[i]));
        if resp.clicked() {
            sort.click(i);
        }
        if sort.column == i {
            // A small triangle after the label: up for A-Z, down for Z-A.
            let w = galley(ui, labels[i], TEXT, INK).size().x;
            let c = pos2(cell.min.x + 2.0 * U + w + 2.0 * U + 4.0, cell.center().y);
            let (tip, base) = if sort.descending {
                (4.0, -3.0)
            } else {
                (-4.0, 3.0)
            };
            let points = vec![
                pos2(c.x - 4.0, c.y + base),
                pos2(c.x + 4.0, c.y + base),
                pos2(c.x, c.y + tip),
            ];
            ui.painter()
                .add(egui::Shape::convex_polygon(points, INK, Stroke::NONE));
        }
    }
}

/// A warning line: a small yellow "!" box, then `text`, `indent` points in.
pub fn warning(ui: &mut Ui, indent: f32, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(indent);
        let (r, _) = ui.allocate_exact_size(vec2(12.0, ROW), Sense::hover());
        let b = snap(Rect::from_center_size(r.center(), Vec2::splat(12.0)));
        ui.painter().rect_filled(b, 0.0, SIGNAL);
        outline(ui.painter(), b, INK);
        let g = galley(ui, "!", TEXT, INK);
        text_at_center(ui.painter(), b, g);
        ui.add(egui::Label::new(text).wrap());
    });
}

fn text_at_center(p: &Painter, r: Rect, g: Arc<Galley>) {
    text(p, r.center() - g.size() / 2.0, g);
}

/// A modal question with two buttons; `Some(true)` when `yes` is clicked,
/// `Some(false)` on the other button or Escape, `None` while open.
pub fn confirm(ctx: &Context, title: &str, body: &str, yes: &str, no: &str) -> Option<bool> {
    let frame = egui::Frame::NONE
        .fill(FACE)
        .stroke(Stroke::new(1.0, INK))
        .inner_margin(Margin::same((3.0 * U) as i8));
    let modal = egui::Modal::new(Id::NULL.with(("confirm", title)))
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(64))
        .show(ctx, |ui| {
            ui.set_width(100.0 * U);
            let r = ui.max_rect().expand(3.0 * U);
            bevel(ui.painter(), r.shrink(1.0), PAPER, SHADE);
            ui.label(egui::RichText::new(title).color(INK));
            ui.add_space(U);
            ui.add(egui::Label::new(body).wrap());
            ui.add_space(2.0 * U);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, no, true, false).clicked() {
                    Some(false)
                } else if button(ui, yes, true, true).clicked() {
                    Some(true)
                } else {
                    None
                }
            })
            .inner
        });
    if modal.should_close() {
        Some(false)
    } else {
        modal.inner
    }
}
