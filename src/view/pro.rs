//! Pages of the PRO version, as mock-ups: the open app shows what
//! Whisker Pro has, with controls that answer the pointer and do
//! nothing. The real pages live in the private Pro build.
//!
//! Texts here are English and fixed on purpose: they are placeholders,
//! translated when the feature behind them exists.
//!
//! ```text
//!  PROFILES [PRO]                         OVERLAY & WIDGET [PRO]
//!  Part of Whisker Pro.  whisker.emlynix… In-game overlay | Desktop widget
//!  +- Profiles ---- [New][Dup][Del] -+    [x] Show overlay in games
//!  | Name    Switch on app  Saved in |    +- preview --------------------+
//!  |▌Default —              Mouse    |    +- Style / Shows / Position ---+
//!  +- Automatic switching -----------+
//! ```

use super::{
    Act, CW, Frame, Glyph, PANEL_GAP, PANEL_HEAD, PANEL_PAD, Page, RowPlace, SEGMENT_H, Segment, SettingRow, TOP, Web, X0, glyph, text_link, text_w, th,
};
use crate::gfx::{Align, Color, Rect, Size, Weight};
use crate::i18n::{T, tr};

const BTN_H: f32 = 28.0;
const TABLE_ROW: f32 = 32.0;
const CHECK: f32 = 14.0;
const SITE: &str = "whisker.emlynix.com";
/// Height of the line under the page title that says what this is.
const NOTE_H: f32 = 24.0;

/// Mock controls are numbered only so each has its own hover.
fn mock(id: u8) -> Act {
    Act::Mock(id)
}

/// "Part of Whisker Pro." and the link to the site, under the page title.
/// Returns the y where the page goes on.
fn pro_note(f: &mut Frame, y: f32) -> f32 {
    let note = tr(T::ProOnly);
    f.g.text(note, Rect::new(X0, y, CW, 18.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
    text_link(f, Act::OpenWeb(Web::Site), SITE, X0 + text_w(note, Size::Caption) + 10.0, y);
    y + NOTE_H
}

/// A checkbox with its label; pressing it does nothing.
fn check(f: &mut Frame, id: u8, x: f32, y: f32, w: f32, label: &str, on: bool, live: bool) {
    const DIMMED: u8 = 0x73;
    let alpha = if live { 0xff } else { DIMMED };
    let r = Rect::new(x, y, w, 22.0);
    let hot = live && f.button(mock(id), r);
    let b = Rect::new(x, y + (22.0 - CHECK) / 2.0, CHECK, CHECK);
    if on {
        f.g.fill(b, 0.0, Color(th().acc_fill.0, alpha));
        glyph(f.g, Glyph::Check, b.x + CHECK / 2.0, b.y + CHECK / 2.0, Color(th().acc_on.0, alpha));
    } else {
        f.g.fill(b, 0.0, Color(th().input.0, alpha));
        f.g.stroke(b, 0.0, 1.0, Color(if hot { th().acc.0 } else { th().line2.0 }, alpha));
    }
    let ink = if live { th().text } else { th().muted };
    f.g.text(label, Rect::new(x + CHECK + 10.0, y, w - CHECK - 10.0, 22.0), Size::Body, Weight::Normal, Align::Left, Color(ink.0, alpha));
}

/// A select box showing `label`.
fn select(f: &mut Frame, id: u8, r: Rect, label: &str) {
    let hot = f.button(mock(id), r);
    f.g.fill(r, 0.0, th().input);
    f.g.stroke(r, 0.0, 1.0, if hot { th().acc } else { th().line2 });
    f.g.text(label, Rect::new(r.x + 10.0, r.y, r.w - 40.0, r.h), Size::Body, Weight::Normal, Align::Left, th().text);
    glyph(f.g, Glyph::ChevronDown, r.x + r.w - 15.0, r.y + r.h / 2.0, th().muted);
}

/// A key cap: display only.
fn key_cap(f: &mut Frame, x: f32, y: f32, label: &str) -> f32 {
    let w = text_w(label, Size::Key) + 12.0;
    let r = Rect::new(x, y, w, 20.0);
    f.g.fill(r, 0.0, th().input);
    f.g.stroke(r, 0.0, 1.0, th().line2);
    f.g.fill(Rect::new(r.x, r.y + r.h - 2.0, r.w, 2.0), 0.0, th().line2);
    f.g.text(label, Rect::new(r.x, r.y - 1.0, r.w, r.h), Size::Key, Weight::Semibold, Align::Center, th().text);
    w
}

/// Profiles: the table of profiles and automatic switching.
pub(super) fn profiles_page(f: &mut Frame) {
    const PROFILES: [(&str, &str, &str); 3] = [("Default", "—", "Mouse"), ("FPS", "game.exe", "Mouse"), ("Desktop", "explorer.exe", "This PC")];
    const COLS: [(f32, &str); 3] = [(0.0, "Name"), (0.45, "Switch on app"), (0.82, "Saved in")];
    const SELECTED: usize = 0;
    let y = pro_note(f, TOP);

    let panel = Rect::new(X0, y, CW, PANEL_HEAD + TABLE_ROW * (PROFILES.len() + 1) as f32);
    let body = f.panel(panel, "Profiles");
    // Header buttons, right to left; "New" is the panel's primary one.
    let mut right = panel.x + panel.w - PANEL_PAD;
    for (k, (label, primary)) in [("Delete", false), ("Duplicate", false), ("New", true)].into_iter().enumerate() {
        let w = text_w(label, Size::Body) + 26.0;
        right -= w;
        f.push_button(mock(10 + k as u8), Rect::new(right, panel.y + 4.0, w, BTN_H), label, primary);
        right -= 6.0;
    }

    let col_x = |part: f32| body.x + PANEL_PAD + (body.w - 2.0 * PANEL_PAD) * part;
    for (part, name) in COLS {
        f.g.text(name, Rect::new(col_x(part), body.y, 160.0, TABLE_ROW), Size::Caption, Weight::Normal, Align::Left, th().muted);
    }
    for (i, (name, app, place)) in PROFILES.into_iter().enumerate() {
        let r = Rect::new(body.x, body.y + (i + 1) as f32 * TABLE_ROW, body.w, TABLE_ROW);
        let hot = f.button(mock(20 + i as u8), r);
        f.g.fill(Rect::new(r.x + 1.0, r.y, r.w - 2.0, 1.0), 0.0, th().line);
        if i == SELECTED {
            f.g.fill(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 1.0), 0.0, th().acc_soft);
            f.g.fill(Rect::new(r.x, r.y, 2.0, r.h), 0.0, th().acc_fill);
        } else if hot {
            f.g.fill(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 1.0), 0.0, th().sel);
        }
        let weight = if i == SELECTED { Weight::Semibold } else { Weight::Normal };
        f.g.text(name, Rect::new(col_x(COLS[0].0), r.y, 200.0, r.h), Size::Body, weight, Align::Left, th().text);
        f.g.text(app, Rect::new(col_x(COLS[1].0), r.y, 200.0, r.h), Size::Body, Weight::Normal, Align::Left, th().muted);
        f.g.text(place, Rect::new(col_x(COLS[2].0), r.y, 100.0, r.h), Size::Body, Weight::Normal, Align::Left, th().muted);
    }

    let auto = SettingRow { title: "Automatic switching", desc: "Activates a profile while its app is in focus.", control_h: 22.0 + 10.0 + BTN_H };
    let panel = Rect::new(X0, panel.y + panel.h + PANEL_GAP, CW, auto.height(CW));
    f.card(panel);
    let c = f.row(panel.x, panel.y, panel.w, &auto, RowPlace::First);
    check(f, 30, c.x, c.y, 240.0, "Switch by active app", true, true);
    let label = "Otherwise use";
    f.g.text(label, Rect::new(c.x, c.y + 32.0, 120.0, BTN_H), Size::Body, Weight::Normal, Align::Left, th().muted);
    select(f, 31, Rect::new(c.x + text_w(label, Size::Body) + 12.0, c.y + 32.0, 150.0, BTN_H), "Default");
}

/// The in-game overlay.
pub(super) fn overlay_page(f: &mut Frame) {
    const PREVIEW_H: f32 = 84.0;
    const PREVIEW_BG: Color = Color::rgb(0x1c2026);
    const PREVIEW_INK: Color = Color::rgb(0xededed);
    const CELL_H: f32 = 28.0;
    const SHOWS: [(&str, bool); 6] =
        [("Clicks per second", true), ("Polling rate", true), ("Battery", true), ("DPI", true), ("Acceleration graph", false), ("Speed graph", false)];
    let mut y = TOP;

    // One row above the preview: the switch and the hotkey.
    check(f, 40, X0, y, 260.0, "Show overlay in games", true, true);
    const KEY: &str = "Ctrl+Shift+O";
    let key_w = text_w(KEY, Size::Key) + 12.0;
    key_cap(f, X0 + CW - key_w, y + 1.0, KEY);
    f.g.text("Toggle in game", Rect::new(X0, y, CW - key_w - 8.0, 22.0), Size::Caption, Weight::Normal, Align::Right, th().muted);
    y += 22.0 + 10.0;

    // The overlay as it would sit in a game: one line, top right.
    let preview = Rect::new(X0, y, CW, PREVIEW_H);
    f.g.fill(preview, 0.0, PREVIEW_BG);
    f.g.stroke(preview, 0.0, 1.0, th().line2);
    let sample = "7 CPS · 500 Hz · 78 % · 2400 DPI";
    f.g.text(sample, Rect::new(preview.x, preview.y + 10.0, preview.w - 12.0, 18.0), Size::Fine, Weight::Semibold, Align::Right, PREVIEW_INK);
    let caption = Rect::new(preview.x + 12.0, preview.y + PREVIEW_H - 24.0, 300.0, 16.0);
    f.g.text("Preview · sample data", caption, Size::Caption, Weight::Normal, Align::Left, Color(PREVIEW_INK.0, 0x8c));
    y += PREVIEW_H + PANEL_GAP;

    let style = SettingRow { title: "Style", desc: "Minimal is one line, like an FPS counter.", control_h: SEGMENT_H };
    let shows = SettingRow { title: "Shows", desc: "In this order. Drag to reorder.", control_h: CELL_H * 3.0 };
    let place = SettingRow { title: "Position and size", desc: "", control_h: 50.0 };
    let panel = Rect::new(X0, y, CW, style.height(CW) + shows.height(CW) + place.height(CW));
    f.card(panel);

    let c = f.row(panel.x, y, CW, &style, RowPlace::First);
    let styles: Vec<Segment> = ["Minimal", "Compact", "Full"].into_iter().enumerate().map(|(k, name)| Segment::new(mock(50 + k as u8), name, k == 0)).collect();
    f.segments(c, &styles);
    y += style.height(CW);

    // What the overlay shows: a bordered two-column list.
    let c = f.row(panel.x, y, CW, &shows, RowPlace::Next);
    f.g.stroke(Rect::new(c.x, c.y, c.w, CELL_H * 3.0), 0.0, 1.0, th().line);
    f.g.fill(Rect::new(c.x + c.w / 2.0, c.y, 1.0, CELL_H * 3.0), 0.0, th().line);
    for (k, (name, on)) in SHOWS.into_iter().enumerate() {
        let (col, row) = ((k % 2) as f32, (k / 2) as f32);
        let (cx, cy) = (c.x + col * c.w / 2.0, c.y + row * CELL_H);
        if row > 0.0 {
            f.g.fill(Rect::new(cx, cy, c.w / 2.0, 1.0), 0.0, th().line);
        }
        // Drag handle: two columns of three dots.
        for dot in 0..6 {
            let (dx, dy) = ((dot % 2) as f32 * 4.0, (dot / 2) as f32 * 4.0);
            f.g.fill(Rect::new(cx + 8.0 + dx, cy + 9.0 + dy, 2.0, 2.0), 0.0, th().line2);
        }
        check(f, 60 + k as u8, cx + 24.0, cy + 3.0, c.w / 2.0 - 30.0, name, on, true);
    }
    y += shows.height(CW);

    // Corner of the screen and scale.
    let c = f.row(panel.x, y, CW, &place, RowPlace::Next);
    const CORNER: f32 = 22.0;
    const SCALE_CELL: f32 = 58.0;
    const SELECTED_CORNER: usize = 1;
    f.g.stroke(Rect::new(c.x, c.y, 2.0 * CORNER + 9.0, 2.0 * CORNER + 9.0), 0.0, 1.0, th().line2);
    for k in 0..4 {
        let r = Rect::new(c.x + 3.0 + (k % 2) as f32 * (CORNER + 3.0), c.y + 3.0 + (k / 2) as f32 * (CORNER + 3.0), CORNER, CORNER);
        let hot = f.button(mock(70 + k as u8), r);
        if k == SELECTED_CORNER {
            f.g.fill(r, 0.0, th().acc_fill);
        } else {
            f.g.fill(r, 0.0, if hot { th().sel } else { th().input });
            f.g.stroke(r, 0.0, 1.0, th().line2);
        }
    }
    let scales: Vec<Segment> = [100, 125, 150, 200].into_iter().enumerate().map(|(k, pct)| Segment::new(mock(80 + k as u8), format!("{pct} %"), k == 1)).collect();
    f.segments(Rect::new(c.x + 2.0 * CORNER + 9.0 + 16.0, c.y + 10.0, SCALE_CELL * 4.0, SEGMENT_H), &scales);
}

/// Overview: the profile switch of the PRO version, with "Manage".
pub(super) fn profile_switch(f: &mut Frame, r: Rect) {
    const TITLE: &str = "Profile";
    let body = f.panel(r, TITLE);
    super::pro_tag(f, r.x + PANEL_PAD + text_w(TITLE, Size::Body) + 10.0, r.y + (PANEL_HEAD - 16.0) / 2.0);
    f.panel_link(r, Act::Nav(Page::Profiles), "Manage");
    let items: Vec<Segment> = ["Default", "FPS", "Desktop"].into_iter().enumerate().map(|(k, name)| Segment::new(mock(120 + k as u8), name, k == 0)).collect();
    f.segments(Rect::new(body.x + PANEL_PAD, body.y + PANEL_PAD, body.w - 2.0 * PANEL_PAD, SEGMENT_H), &items);
}

/// Settings › About: "Backup & sync" of the PRO version, as a mock-up.
/// Returns the y under the panel.
pub(super) fn backup_panel(f: &mut Frame, y: f32) -> f32 {
    const TITLE: &str = "Backup & sync";
    const ROW: f32 = 40.0;
    let panel = Rect::new(X0, y, CW, PANEL_HEAD + ROW);
    let body = f.panel(panel, TITLE);
    super::pro_tag(f, panel.x + PANEL_PAD + text_w(TITLE, Size::Body) + 10.0, panel.y + (PANEL_HEAD - 16.0) / 2.0);
    f.panel_note(panel, tr(T::ProOnly));

    check(f, 130, body.x + PANEL_PAD, body.y + (ROW - 22.0) / 2.0, 200.0, "Sync automatically", false, true);
    // Buttons from the right: the file ones, then "Sync now".
    let mut right = body.x + body.w - PANEL_PAD;
    for (k, label) in ["Import…", "Export…", "Sync now"].into_iter().enumerate() {
        let w = text_w(label, Size::Body) + 26.0;
        right -= w;
        f.push_button(mock(131 + k as u8), Rect::new(right, body.y + (ROW - BTN_H) / 2.0, w, BTN_H), label, false);
        right -= 6.0;
    }
    panel.y + panel.h
}
