//! Profiles: the list with its buttons, and the switch on Overview.
//!
//! ```text
//!  PROFILES
//!  A profile holds the button layout and the macros. …
//!  +- Profiles ------- [New][Duplicate][Rename][Delete] -+
//!  | Name              Buttons    Macros                 |
//!  |▌Default           2          1               Active |
//!  | FPS               —          —                      |
//!  +- Automatic switching [PRO] ------------------------+
//! ```
//! A press on a row makes its profile the active one. "Rename" turns the
//! active row's name into a field; the controller feeds it the keys.

use super::{Act, CW, Frame, PANEL_GAP, PANEL_HEAD, PANEL_PAD, Page, ProfileRow, SEGMENT_H, Segment, TOP, View, X0, pro, text_w, th};
use crate::gfx::{Align, Rect, Size, Weight};
use crate::i18n::{T, tr};
use crate::store::PROFILE_MAX;

const BTN_H: f32 = 28.0;
const TABLE_ROW: f32 = 30.0;
/// Two lines of the note under the page title.
const NOTE_H: f32 = 40.0;
/// Left edge of the name, buttons and macros columns, as parts of the width.
const COLS: [f32; 3] = [0.0, 0.50, 0.68];
/// The field a name is typed into.
const FIELD_W: f32 = 230.0;
const FIELD_PAD: f32 = 6.0;

/// A header button; without an act it is shown as unavailable.
fn head_button(f: &mut Frame, act: Option<Act>, r: Rect, label: &str, primary: bool) {
    let Some(act) = act else {
        f.g.stroke(r, 0.0, 1.0, th().line);
        f.g.text(label, Rect::new(r.x + 8.0, r.y, r.w - 16.0, r.h), Size::Body, Weight::Semibold, Align::Center, th().faint);
        return;
    };
    f.push_button(act, r, label, primary);
}

/// "2", or a dash for none.
fn count(n: usize) -> String {
    if n == 0 { "—".into() } else { n.to_string() }
}

/// The name of the active row while it is typed: a field with a caret,
/// the old name behind it until the first key.
fn name_field(f: &mut Frame, r: Rect, typed: &str, old: &str) {
    const CARET_W: f32 = 1.0;
    f.g.fill(r, 0.0, th().input);
    f.g.stroke(r, 0.0, 1.0, th().acc);
    let text = Rect::new(r.x + FIELD_PAD, r.y, r.w - 2.0 * FIELD_PAD, r.h);
    if typed.is_empty() {
        f.g.text(old, text, Size::Body, Weight::Normal, Align::Left, th().faint);
    } else {
        f.g.text(typed, text, Size::Body, Weight::Semibold, Align::Left, th().text);
    }
    let caret_x = (text.x + text_w(typed, Size::Body)).min(text.x + text.w);
    f.g.fill(Rect::new(caret_x + 1.0, r.y + 5.0, CARET_W, r.h - 10.0), 0.0, th().acc);
}

fn row(f: &mut Frame, v: &View, i: usize, p: &ProfileRow, r: Rect, col_x: impl Fn(usize) -> f32) {
    let active = i == v.profile;
    let hot = f.button(Act::Profile(i), r);
    f.g.fill(Rect::new(r.x + 1.0, r.y, r.w - 2.0, 1.0), 0.0, th().line);
    if active {
        f.g.fill(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 1.0), 0.0, th().acc_soft);
        f.g.fill(Rect::new(r.x, r.y, 2.0, r.h), 0.0, th().acc_fill);
    } else if hot {
        f.g.fill(Rect::new(r.x + 1.0, r.y + 1.0, r.w - 2.0, r.h - 1.0), 0.0, th().sel);
    }

    // The active row while it is renamed: the field and how to end it.
    if let Some(typed) = v.rename.as_deref().filter(|_| active) {
        name_field(f, Rect::new(col_x(0) - FIELD_PAD, r.y + 3.0, FIELD_W, r.h - 5.0), typed, &p.name);
        let hint = Rect::new(col_x(1), r.y, r.x + r.w - PANEL_PAD - col_x(1), r.h);
        f.g.text(tr(T::RenameHint), hint, Size::Caption, Weight::Normal, Align::Left, th().muted);
        return;
    }
    let weight = if active { Weight::Semibold } else { Weight::Normal };
    f.g.text(&p.name, Rect::new(col_x(0), r.y, col_x(1) - col_x(0) - 8.0, r.h), Size::Body, weight, Align::Left, th().text);
    for (k, n) in [(1, p.remapped), (2, p.macros)] {
        f.g.text(&count(n), Rect::new(col_x(k), r.y, 80.0, r.h), Size::Value, Weight::Normal, Align::Left, th().muted);
    }
    if active {
        let tag = Rect::new(r.x, r.y, r.w - PANEL_PAD, r.h);
        f.g.text(tr(T::ProfileActive), tag, Size::Caption, Weight::Semibold, Align::Right, th().acc);
    }
}

/// The Profiles page.
pub(super) fn page(f: &mut Frame, v: &View) {
    f.g.text(tr(T::ProfileNote), Rect::new(X0, TOP - 4.0, CW, NOTE_H - 6.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
    let y = TOP + NOTE_H;

    let panel = Rect::new(X0, y, CW, PANEL_HEAD + TABLE_ROW * (v.profiles.len() + 1) as f32);
    let body = f.panel(panel, tr(T::PageProfiles));
    // Header buttons, right to left; "New" is the panel's primary one.
    let room = v.profiles.len() < PROFILE_MAX;
    let buttons = [
        (T::ProfileDelete, (v.profiles.len() > 1).then_some(Act::ProfileDelete), false),
        (T::ProfileRename, Some(Act::ProfileRename), false),
        (T::ProfileDuplicate, room.then_some(Act::ProfileDuplicate), false),
        (T::ProfileNew, room.then_some(Act::ProfileNew), true),
    ];
    let mut right = panel.x + panel.w - PANEL_PAD;
    for (label, act, primary) in buttons {
        let w = text_w(tr(label), Size::Body) + 26.0;
        right -= w;
        head_button(f, act, Rect::new(right, panel.y + 4.0, w, BTN_H), tr(label), primary);
        right -= 6.0;
    }

    let col_x = |k: usize| body.x + PANEL_PAD + (body.w - 2.0 * PANEL_PAD) * COLS[k];
    for (k, name) in [T::ColName, T::PageButtons, T::PageMacros].into_iter().enumerate() {
        f.g.text(tr(name), Rect::new(col_x(k), body.y, 160.0, TABLE_ROW), Size::Caption, Weight::Normal, Align::Left, th().muted);
    }
    for (i, p) in v.profiles.iter().enumerate() {
        let r = Rect::new(body.x, body.y + (i + 1) as f32 * TABLE_ROW, body.w, TABLE_ROW);
        row(f, v, i, p, r, col_x);
    }
    pro::auto_switch(f, panel.y + panel.h + PANEL_GAP);
}

/// Most profiles in one row of the Overview switch: names stay readable.
const SWITCH_ROW: usize = 4;
const SWITCH_GAP: f32 = 6.0;

/// Height of the Overview panel for `count` profiles.
pub(super) fn switch_h(count: usize) -> f32 {
    let rows = count.div_ceil(SWITCH_ROW).max(1) as f32;
    PANEL_HEAD + 2.0 * PANEL_PAD + rows * SEGMENT_H + (rows - 1.0) * SWITCH_GAP
}

/// Overview: one segment per profile, four to a row, with "Manage".
pub(super) fn switch(f: &mut Frame, v: &View, r: Rect) {
    let body = f.panel(r, tr(T::Profile));
    f.panel_link(r, Act::Nav(Page::Profiles), tr(T::ProfileManage));
    let items: Vec<Segment> = v.profiles.iter().enumerate().map(|(i, p)| Segment::named(Act::Profile(i), p.name.clone(), i == v.profile)).collect();
    for (k, row) in items.chunks(SWITCH_ROW).enumerate() {
        let y = body.y + PANEL_PAD + k as f32 * (SEGMENT_H + SWITCH_GAP);
        f.segments(Rect::new(body.x + PANEL_PAD, y, body.w - 2.0 * PANEL_PAD, SEGMENT_H), row);
    }
}
