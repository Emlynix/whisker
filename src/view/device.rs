//! Settings › Mouse tab (hardware info, diagnostics), the "which mouse
//! do you use?" dialog and the new-mouse banner.

use super::{Act, BODY, CW, Frame, GAP, Glyph, HEIGHT, Mark, Page, Place, R_CTRL, RADIUS, View, WIDTH, X0, glyph, icon, name_lines, text_link, text_w, th, Squares};
use crate::devices::Support;
use crate::gfx::{Align, Color, Rect, Size, Weight};
use crate::i18n::{T, fill, tr};

const ROW: f32 = 26.0;
const PAD: f32 = 20.0;

/// Support level as a colour: full, experimental, partial, statistics only.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Badge {
    Good,
    Warn,
    Partial,
    Muted,
}

/// One model row of the detect dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectRow {
    pub title: String,
    pub sub: String,
    pub badge: String,
    pub kind: Badge,
    /// Model id to store, or `store::GENERIC_DEVICE`.
    pub choice: String,
}

/// A connected mouse on step 1 of the dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub title: String,
    /// Support level as text, e.g. "Supported".
    pub badge: String,
    pub kind: Badge,
    /// The mouse in use now.
    pub current: bool,
    /// Tag after the support level, e.g. `T::ViaReceiver`.
    pub via: Option<T>,
    /// Its catalogue entry as (brand, model) list positions.
    pub brand: usize,
    pub model: usize,
    pub usb: (u16, u16),
}

/// A brand in the left list; `kind` = best support among its models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrandRow {
    pub name: String,
    pub kind: Badge,
}

/// List the arrow keys move in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    Brands,
    Models,
}

/// Step of the dialog: the mice still being listed, the detected mice,
/// or the whole catalogue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DetectStep {
    /// Listing the mice runs on its thread; nothing to choose yet.
    Searching,
    Found,
    List,
}

/// The "which mouse" dialog: detected mice, brands, models of one brand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detect {
    pub step: DetectStep,
    pub found: Vec<Found>,
    pub brands: Vec<BrandRow>,
    /// Models of the selected brand.
    pub models: Vec<DetectRow>,
    pub brand: usize,
    pub model: usize,
    /// The mouse in use: catalogue id and the support its driver found
    /// (an auto-detect entry that named a verified model).
    pub live: Option<(String, Support)>,
    /// The detected mouse the selection came from, if any.
    pub chip: Option<usize>,
    /// Step 1 pick when the lists opened; "Back" restores it.
    pub kept: Option<usize>,
    pub pane: Pane,
    /// First visible row of each list.
    pub brand_top: usize,
    pub model_top: usize,
    /// Arrow keys were used: outline the list they move in.
    pub keys: bool,
    /// A scan is listing the mice again: the rows shown are the last list.
    pub refetching: bool,
}

/// "New mouse connected: Logitech G305 (Supported)  [Not now] [Switch]".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Banner {
    pub name: String,
    pub badge: String,
    pub kind: Badge,
}

/// Device page content prepared by the controller: (caption, value) rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceView {
    pub rows: Vec<(T, String)>,
    /// Firmware version for the Overview line; empty if unknown.
    pub firmware: String,
    /// Raw status bytes as hex, for bug reports; empty if none.
    pub status: String,
    pub offline: bool,
    /// Driver not yet verified on hardware: ask for reports.
    pub experimental: bool,
}

fn badge_color(k: Badge) -> Color {
    match k {
        Badge::Good => th().good,
        Badge::Warn => th().warn,
        Badge::Partial => th().accent,
        // Also a text colour (badges): `subtle`, not the decorative `faint`.
        Badge::Muted => th().subtle,
    }
}

pub(super) fn device_page(f: &mut Frame, v: &View) {
    // Second line of a long model name.
    const WRAP_H: f32 = 18.0;
    let d = &v.device;
    let value_w = CW - 250.0;
    // A long model name takes two lines, like in the sidebar.
    let model = d.rows.iter().find(|(c, _)| *c == T::DevModel).map(|(_, s)| name_lines(s, value_w, Size::Body));
    let extra = if model.as_ref().is_some_and(|(_, second)| second.is_some()) { WRAP_H } else { 0.0 };
    let card = Rect::new(X0, BODY, CW, ROW * d.rows.len() as f32 + 16.0 + extra);
    f.card(card);

    let mut y = card.y + 8.0;
    for (caption, value) in &d.rows {
        f.caption_in(tr(*caption), card.x + PAD, y + 6.0, 190.0);
        let r = Rect::new(card.x + 230.0, y, value_w, ROW);
        y += ROW;
        if value.is_empty() {
            f.g.text(tr(T::NotReported), r, Size::Body, Weight::Normal, Align::Left, th().subtle);
            continue;
        }
        if let (T::DevModel, Some((first, Some(second)))) = (*caption, &model) {
            f.g.text(first, r, Size::Body, Weight::Normal, Align::Left, th().text);
            f.g.text(second, Rect::new(r.x, r.y + WRAP_H, r.w, ROW), Size::Body, Weight::Normal, Align::Left, th().text);
            y += WRAP_H;
            continue;
        }
        f.g.text(value, r, Size::Body, Weight::Normal, Align::Left, th().text);
    }

    let mut y = card.y + card.h + GAP;
    if d.experimental {
        let r = Rect::new(X0, y, CW, 36.0);
        f.g.text(tr(T::ExperimentalNote), r, Size::Caption, Weight::Normal, Align::Wrap, th().warn);
        y += 42.0;
    }
    if d.offline {
        let r = Rect::new(X0, y, CW, 18.0);
        f.g.text(tr(T::DevOffline), r, Size::Caption, Weight::Normal, Align::Left, th().muted);
        y += 26.0;
    }
    f.push_button(Act::ChangeMouse, Rect::new(X0, y, 170.0, 36.0), tr(T::ChangeMouse), false);
    let chevron = if v.diagnostics { Glyph::ChevronDown } else { Glyph::ChevronRight };
    f.glyph_button(Act::Diagnostics, Rect::new(X0 + 180.0, y, 170.0, 36.0), tr(T::Diagnostics), chevron, Place::Trail);
    f.push_button(Act::ReportOpen, Rect::new(X0 + 360.0, y, CW - 360.0, 36.0), tr(T::HelpAddMouse), false);
    if v.diagnostics {
        diagnostics(f, d, y + 36.0 + 10.0);
    }
}

/// Raw status bytes, "Copy for support", and the way to the
/// diagnostic log (to read, copy or send it at any time).
fn diagnostics(f: &mut Frame, d: &DeviceView, y: f32) {
    const BTN_W: f32 = 190.0;
    const LINK_GAP: f32 = 24.0;
    let card = Rect::new(X0, y, CW, 56.0);
    f.card(card);
    let caption = tr(T::DevStatus);
    f.caption(caption, card.x + PAD, card.y + 8.0);
    let link_x = card.x + PAD + text_w(caption, Size::Label) + LINK_GAP;
    text_link(f, Act::ShowLog, tr(T::ShowLog), link_x, card.y + 6.0);
    let (text, color) = if d.status.is_empty() { (tr(T::NotReported), th().subtle) } else { (d.status.as_str(), th().text) };
    let r = Rect::new(card.x + PAD, card.y + 26.0, card.w - 2.0 * PAD - BTN_W - 12.0, 22.0);
    f.g.text(text, r, Size::Caption, Weight::Normal, Align::Left, color);
    let btn = Rect::new(card.x + card.w - PAD - BTN_W + 8.0, card.y + 10.0, BTN_W - 8.0, 36.0);
    f.push_button(Act::CopyDiagnostics, btn, tr(T::CopySupport), false);
}

// ---- "which mouse" dialog ----------------------------------------------------
//
//  Step 1: detected mice                  Step 2: catalogue
//  +--------------------------------+     +--------------------------------+
//  | Which mouse do you use?        |     | Which mouse do you use?        |
//  | DETECTED                       |     | (No mouse detected. …)         |
//  | (o) White Shark Lionel         |     | BRAND        MODEL             |
//  |     ● Supported · in use       |     | +--------+  +----------------+ |
//  | ( ) Corsair Katar Pro          |     | |● White |  | Lionel        #| |
//  |     ● Experimental             |     | |● Logi… |  | Other …   v   #| |
//  | My mouse isn't listed ›        |     | +--------+  +----------------+ |
//  |                                |     | ● Experimental   Help add …    |
//  |              [Cancel] [Use]    |     | [‹ Back]       [Cancel] [Use]  |
//  +--------------------------------+     +--------------------------------+
//
//  While the mice are still being listed (the first scan of the session):
//  +--------------------------------+
//  | Which mouse do you use?        |
//  | Looking for mice…              |
//  |                        [Cancel]|
//  +--------------------------------+
//
//  Steps 1 and 2 opened from "Change mouse" also note "Refetching HID
//  devices…" in the top right, until the scan they started brings its list.

const DLG_W: f32 = 700.0;
const DLG_H: f32 = 520.0;
const DLG_PAD: f32 = 24.0;
const BTN_H: f32 = 36.0;
const USE_W: f32 = 170.0;
const CANCEL_W: f32 = 120.0;
/// Room for the chevron and padding of "‹ Back".
const BACK_PAD: f32 = 56.0;
/// Step 1: one detected mouse per two-line row.
const FOUND_TOP: f32 = 68.0;
const FOUND_H: f32 = 56.0;
const FOUND_GAP: f32 = 8.0;
/// Detected rows that fit; any more are in the lists.
const FOUND_ROWS: usize = 5;
const RADIO_R: f32 = 9.0;
/// Step 2: lists inside framed boxes.
const LIST_TOP: f32 = 84.0;
const LIST_ROW: f32 = 30.0;
/// Rows each list shows; longer lists scroll.
pub const LIST_ROWS: usize = 9;
const FRAME_PAD: f32 = 6.0;
const SCROLL_W: f32 = 6.0;
const THUMB_MIN: f32 = 24.0;
/// "More below" fade over the last visible row, in bands.
const FADE_H: f32 = 28.0;
const FADE_STEPS: usize = 7;
const BRAND_W: f32 = 220.0;
const PANE_GAP: f32 = 16.0;
const SUPPORT_H: f32 = 24.0;

/// Dialog frame and the two list boxes (brands, models).
struct Layout {
    dlg: Rect,
    inner: Rect,
    brands: Rect,
    models: Rect,
}

fn layout() -> Layout {
    let dlg = Rect::new((WIDTH - DLG_W) / 2.0, (HEIGHT - DLG_H) / 2.0, DLG_W, DLG_H);
    let inner = dlg.inset(DLG_PAD);
    let top = inner.y + LIST_TOP;
    let h = LIST_ROWS as f32 * LIST_ROW + 2.0 * FRAME_PAD;
    let brands = Rect::new(inner.x, top, BRAND_W, h);
    let models = Rect::new(inner.x + BRAND_W + PANE_GAP, top, inner.w - BRAND_W - PANE_GAP, h);
    Layout { dlg, inner, brands, models }
}

/// The list under a pointer position, for the mouse wheel.
pub fn detect_pane_at(x: f32, y: f32) -> Option<Pane> {
    let l = layout();
    if l.brands.contains(x, y) {
        return Some(Pane::Brands);
    }
    l.models.contains(x, y).then_some(Pane::Models)
}

pub(super) fn detect_dialog(f: &mut Frame, d: &Detect) {
    let l = layout();
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    f.g.fill(l.dlg, RADIUS, th().card_hi);
    f.g.stroke(l.dlg, RADIUS, 1.0, th().border);

    let inner = l.inner;
    f.g.text(tr(T::DetectTitle), Rect::new(inner.x, inner.y, inner.w, 28.0), Size::Subtitle, Weight::Bold, Align::Left, th().text);
    match d.step {
        DetectStep::Searching => searching_step(f, inner),
        DetectStep::Found => {
            found_step(f, d, inner);
            refetch_note(f, d, inner);
            buttons(f, d, inner);
        }
        DetectStep::List => {
            list_step(f, d, &l);
            refetch_note(f, d, inner);
            buttons(f, d, inner);
        }
    }
}

/// Top right, where the "Help add your mouse" dialog shows its step: the
/// rows are the last list and the one "Change mouse" started is on its way.
fn refetch_note(f: &mut Frame, d: &Detect, inner: Rect) {
    if !d.refetching {
        return;
    }
    let label = tr(T::DetectRefetching);
    let w = text_w(label, Size::Caption);
    f.g.text(label, Rect::new(inner.x + inner.w - w, inner.y + 4.0, w, 20.0), Size::Caption, Weight::Normal, Align::Right, th().muted);
}

/// While the first list of mice is on its thread: the caption the "Help add
/// your mouse" dialog shows in the same situation, and nothing but Cancel.
fn searching_step(f: &mut Frame, inner: Rect) {
    f.g.text(tr(T::ReportFinding), Rect::new(inner.x, inner.y + 44.0, inner.w, 22.0), Size::Body, Weight::Normal, Align::Left, th().muted);
    let by = inner.y + inner.h - BTN_H;
    f.push_button(Act::DetectCancel, Rect::new(inner.x + inner.w - CANCEL_W, by, CANCEL_W, BTN_H), tr(T::Cancel), true);
}

/// Buttons: "‹ Back" on step 2 when step 1 exists; Cancel, Use.
fn buttons(f: &mut Frame, d: &Detect, inner: Rect) {
    let by = inner.y + inner.h - BTN_H;
    if d.step == DetectStep::List && !d.found.is_empty() {
        let label = tr(T::BackToDetected);
        let back = Rect::new(inner.x, by, text_w(label, Size::Body) + BACK_PAD, BTN_H);
        f.glyph_button(Act::DetectBack, back, label, Glyph::ChevronLeft, Place::Lead);
    }
    let use_btn = Rect::new(inner.x + inner.w - USE_W, by, USE_W, BTN_H);
    f.push_button(Act::DetectCancel, Rect::new(use_btn.x - 10.0 - CANCEL_W, by, CANCEL_W, BTN_H), tr(T::Cancel), false);
    // Step 1 needs a pick first.
    if d.step == DetectStep::Found && d.chip.is_none() {
        f.dead_button(use_btn, tr(T::UseMouse));
        return;
    }
    f.push_button(Act::DetectUse, use_btn, tr(T::UseMouse), true);
}

/// Step 1: the detected mice as large rows, then the way to the lists.
fn found_step(f: &mut Frame, d: &Detect, inner: Rect) {
    f.caption_in(tr(T::DetectedCaption), inner.x, inner.y + 44.0, inner.w);
    let mut y = inner.y + FOUND_TOP;
    for (i, m) in d.found.iter().enumerate().take(FOUND_ROWS) {
        let r = Rect::new(inner.x, y, inner.w, FOUND_H);
        found_row(f, m, Act::DetectFound(i), r, Mark::of(d.chip == Some(i)));
        y += FOUND_H + FOUND_GAP;
    }
    more_link(f, Act::DetectList, tr(T::NotListed), inner.x, y);
}

/// Detected mouse: radio marker, name, support level ("· in use" on the
/// mouse in use). Picked = accent border and soft fill, like tiles.
fn found_row(f: &mut Frame, m: &Found, act: Act, r: Rect, mark: Mark) {
    let sel = mark == Mark::Current;
    let hot = f.button(act, r);
    let bg = if sel { th().accent_soft } else if hot { th().hover } else { th().card };
    f.g.fill(r, R_CTRL, th().card);
    f.g.fill(r, R_CTRL, bg);
    let (bw, bc) = if sel { (2.0, th().accent) } else { (1.0, th().border) };
    f.g.stroke(r, R_CTRL, bw, bc);

    // Radio marker: a ring, filled with a dot when picked.
    let (cx, cy) = (r.x + 26.0, r.y + r.h / 2.0);
    f.g.frame(cx, cy, RADIO_R, 1.5, if sel { th().accent } else { th().subtle });
    if sel {
        f.g.dot(cx, cy, RADIO_R / 2.0, th().accent);
    }

    // Name on top; support dot and level below.
    let (x, w) = (r.x + 50.0, r.w - 66.0);
    f.g.text(&m.title, Rect::new(x, r.y + 8.0, w, 20.0), Size::Body, Weight::Semibold, Align::Left, th().text);
    let mut sub = m.badge.clone();
    if m.current {
        sub = format!("{sub} · {}", tr(T::CurrentTag));
    }
    if let Some(via) = m.via {
        sub = format!("{sub} · {}", tr(via));
    }
    f.g.dot(x + 4.0, r.y + 39.0, 3.5, badge_color(m.kind));
    f.g.text(&sub, Rect::new(x + 14.0, r.y + 30.0, w - 14.0, 18.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
}

/// Accent link, e.g. "My mouse isn't listed". No chevron: the label
/// width is only estimated, so a trailing glyph floats off the text.
fn more_link(f: &mut Frame, act: Act, label: &str, x: f32, y: f32) {
    const H: f32 = 32.0;
    const SLACK: f32 = 16.0;
    let r = Rect::new(x, y, text_w(label, Size::Body) + SLACK, H);
    let hot = f.button(act, r);
    let c = if hot { th().text } else { th().accent };
    f.g.text(label, r, Size::Body, Weight::Semibold, Align::Left, c);
}

/// Step 2: brand and model lists, the picked model's support below.
fn list_step(f: &mut Frame, d: &Detect, l: &Layout) {
    let inner = l.inner;
    if d.found.is_empty() {
        let r = Rect::new(inner.x, inner.y + 36.0, inner.w, 18.0);
        f.g.text(tr(T::NoMouse), r, Size::Caption, Weight::Normal, Align::Left, th().muted);
    }
    f.caption_in(tr(T::BrandCaption), l.brands.x, l.brands.y - 22.0, l.brands.w);
    f.caption_in(tr(T::ModelCaption), l.models.x, l.models.y - 22.0, l.models.w);
    brand_list(f, d, l.brands);
    model_list(f, d, l.models);
    if d.keys {
        // Which list the arrow keys move in.
        const OFF: f32 = 3.0;
        let r = if d.pane == Pane::Brands { l.brands } else { l.models };
        let ring = Rect::new(r.x - OFF, r.y - OFF, r.w + 2.0 * OFF, r.h + 2.0 * OFF);
        f.g.stroke(ring, R_CTRL + OFF, 2.0, th().accent);
    }
    let y = l.models.y + l.models.h + 10.0;
    support_line(f, d, Rect::new(inner.x, y, inner.w, SUPPORT_H));
}

/// One list row: selected = soft fill and a left bar; hover = hover fill.
fn list_row(f: &mut Frame, act: Act, r: Rect, selected: bool) {
    let hot = f.button(act, r);
    if selected || hot {
        f.g.fill(r, R_CTRL, if selected { th().accent_soft } else { th().hover });
    }
    if selected {
        f.g.fill(Rect::new(r.x, r.y + 7.0, 3.0, r.h - 14.0), 0.0, th().accent);
    }
}

/// List box: card fill and border, so it reads as a scrollable field.
fn list_frame(f: &Frame, r: Rect) {
    f.g.fill(r, R_CTRL, th().card);
    f.g.stroke(r, R_CTRL, 1.0, th().border);
}

/// Row `k` of a list box scrolled to `top`; keeps the scroll bar clear.
fn row_rect(area: Rect, k: usize, top: usize, count: usize) -> Rect {
    let bar = if count > LIST_ROWS { SCROLL_W + FRAME_PAD } else { 0.0 };
    let y = area.y + FRAME_PAD + (k - top) as f32 * LIST_ROW;
    Rect::new(area.x + FRAME_PAD, y, area.w - 2.0 * FRAME_PAD - bar, LIST_ROW - 2.0)
}

/// Rows follow below the visible ones.
fn more_below(top: usize, count: usize) -> bool {
    top + LIST_ROWS < count
}

/// Scroll thumb in `track`: as long as the visible share, at least
/// `THUMB_MIN`, placed by `top`.
fn thumb(track: Rect, top: usize, count: usize) -> Rect {
    let hidden = count.saturating_sub(LIST_ROWS);
    if hidden == 0 {
        return track;
    }
    let h = (track.h * LIST_ROWS as f32 / count as f32).max(THUMB_MIN);
    let y = track.y + (track.h - h) * top.min(hidden) as f32 / hidden as f32;
    Rect::new(track.x, y, track.w, h)
}

/// Scroll bar with a track on a list longer than `LIST_ROWS`; a fade and
/// a chevron over the last row while more rows follow.
fn scroll_cue(f: &mut Frame, area: Rect, top: usize, count: usize) {
    if count <= LIST_ROWS {
        return;
    }
    let track = Rect::new(area.x + area.w - FRAME_PAD - SCROLL_W, area.y + FRAME_PAD, SCROLL_W, area.h - 2.0 * FRAME_PAD);
    f.g.fill(track, 0.0, th().border);
    f.g.fill(thumb(track, top, count), 0.0, th().subtle);
    if !more_below(top, count) {
        return;
    }

    // Bands of the box colour, more opaque towards the bottom edge.
    let (x, bottom) = (area.x + FRAME_PAD, track.y + track.h);
    let w = track.x - FRAME_PAD - x;
    let band = FADE_H / FADE_STEPS as f32;
    for i in 0..FADE_STEPS {
        let alpha = (255 * (i + 1) / (FADE_STEPS + 1)) as u8;
        let r = Rect::new(x, bottom - FADE_H + i as f32 * band, w, band);
        f.g.fill(r, 0.0, Color(th().card.0, alpha));
    }
    glyph(f.g, Glyph::ChevronDown, x + w / 2.0, bottom - 8.0, th().muted);
}

/// Brands with a dot for the best support among their models.
fn brand_list(f: &mut Frame, d: &Detect, area: Rect) {
    list_frame(f, area);
    let count = d.brands.len();
    for (k, b) in d.brands.iter().enumerate().skip(d.brand_top).take(LIST_ROWS) {
        let r = row_rect(area, k, d.brand_top, count);
        let sel = k == d.brand;
        list_row(f, Act::DetectBrand(k), r, sel);
        f.g.dot(r.x + 16.0, r.y + r.h / 2.0, 4.0, badge_color(b.kind));
        let weight = if sel { Weight::Semibold } else { Weight::Normal };
        let label = Rect::new(r.x + 28.0, r.y, r.w - 34.0, r.h);
        f.g.text(&b.name, label, Size::Body, weight, Align::Left, if sel { th().text } else { th().muted });
    }
    scroll_cue(f, area, d.brand_top, count);
}

/// Models of the selected brand with a support dot, like the brands;
/// the picked model's support in words shows below.
fn model_list(f: &mut Frame, d: &Detect, area: Rect) {
    list_frame(f, area);
    let count = d.models.len();
    for (k, m) in d.models.iter().enumerate().skip(d.model_top).take(LIST_ROWS) {
        let r = row_rect(area, k, d.model_top, count);
        let sel = k == d.model;
        list_row(f, Act::DetectSel(k), r, sel);
        f.g.dot(r.x + 16.0, r.y + r.h / 2.0, 4.0, badge_color(m.kind));
        let weight = if sel { Weight::Semibold } else { Weight::Normal };
        let name = Rect::new(r.x + 28.0, r.y, r.w - 34.0, r.h);
        f.g.text(&m.title, name, Size::Body, weight, Align::Left, if sel { th().text } else { th().muted });
    }
    scroll_cue(f, area, d.model_top, count);
}

/// The picked model's support level; for auto-detect, experimental and
/// "other" entries also a way to get the mouse supported.
fn support_line(f: &mut Frame, d: &Detect, r: Rect) {
    const SIZE: Size = Size::Caption;
    let Some(m) = d.models.get(d.model) else {
        return;
    };
    let color = badge_color(m.kind);
    f.g.dot(r.x + 4.0, r.y + r.h / 2.0, 4.0, color);
    let badge_w = text_w(&m.badge, SIZE) + 12.0;
    f.g.text(&m.badge, Rect::new(r.x + 14.0, r.y, badge_w, r.h), SIZE, Weight::Semibold, Align::Left, color);
    if !matches!(m.kind, Badge::Warn | Badge::Muted) {
        return;
    }
    let x = r.x + 14.0 + badge_w + 12.0;
    report_hint(f, Rect::new(x, r.y, r.x + r.w - x, r.h));
}

/// "Mouse missing or experimental? Help add your mouse", right-aligned;
/// the question is left out when it does not fit.
fn report_hint(f: &mut Frame, r: Rect) {
    const SIZE: Size = Size::Caption;
    let link = tr(T::HelpAddMouse);
    let link_w = (text_w(link, SIZE) + 20.0).min(r.w);
    let lr = Rect::new(r.x + r.w - link_w, r.y, link_w, r.h);
    let hot = f.button(Act::ReportOpen, lr);
    if hot {
        f.g.fill(lr, R_CTRL, th().hover);
    }
    f.g.text(link, Rect::new(lr.x + 8.0, lr.y, lr.w - 16.0, lr.h), SIZE, Weight::Semibold, Align::Right, th().accent);

    let hint = tr(T::MissingMouse);
    let room = r.w - link_w;
    if text_w(hint, SIZE) > room {
        return;
    }
    f.g.text(hint, Rect::new(r.x, r.y, room, r.h), SIZE, Weight::Normal, Align::Right, th().muted);
}

/// New-mouse offer over the title row: mouse icon, name with its support
/// badge, "Not now" and "Switch".
pub(super) fn banner(f: &mut Frame, b: &Banner) {
    const TOP: f32 = 12.0;
    const H: f32 = 48.0;
    const BTN_H: f32 = 32.0;
    const BTN_GAP: f32 = 8.0;
    let r = Rect::new(X0, TOP, CW, H);
    f.g.fill(r, RADIUS, th().card_hi);
    f.g.stroke(r, RADIUS, 1.0, th().accent);
    icon(f.g, Page::Buttons, r.x + 14.0, r.y + (H - 20.0) / 2.0, th().accent);

    // Buttons from the right: Switch (primary), then Not now.
    let by = r.y + (H - BTN_H) / 2.0;
    let switch_w = text_w(tr(T::SwitchMouse), Size::Body) + 32.0;
    let later_w = text_w(tr(T::NotNow), Size::Body) + 32.0;
    let switch = Rect::new(r.x + r.w - 8.0 - switch_w, by, switch_w, BTN_H);
    let later = Rect::new(switch.x - BTN_GAP - later_w, by, later_w, BTN_H);
    f.push_button(Act::BannerDismiss, later, tr(T::NotNow), false);
    f.push_button(Act::BannerSwitch, switch, tr(T::SwitchMouse), true);

    // Text: what was connected on top, its support badge below.
    let text_x = r.x + 44.0;
    let room = later.x - 12.0 - text_x;
    let title = fill(tr(T::NewMouseBanner), &[&b.name]);
    f.g.text(&title, Rect::new(text_x, r.y + 5.0, room, 20.0), Size::Body, Weight::Semibold, Align::Left, th().text);
    f.g.text(&b.badge, Rect::new(text_x, r.y + 25.0, room, 18.0), Size::Caption, Weight::Semibold, Align::Left, badge_color(b.kind));
}

/// "Whisker needs permission to access the mouse  [Copy command]" over
/// the title row, in the warning colour.
pub(super) fn access_banner(f: &mut Frame) {
    const TOP: f32 = 12.0;
    const H: f32 = 48.0;
    const BTN_H: f32 = 32.0;
    // Room for the semibold label, measured at normal weight.
    const BTN_PAD: f32 = 48.0;
    let r = Rect::new(X0, TOP, CW, H);
    f.g.fill(r, RADIUS, th().card_hi);
    f.g.stroke(r, RADIUS, 1.0, th().warn);

    let btn_w = text_w(tr(T::CopyCommand), Size::Body) + BTN_PAD;
    let btn = Rect::new(r.x + r.w - 8.0 - btn_w, r.y + (H - BTN_H) / 2.0, btn_w, BTN_H);
    f.push_button(Act::CopyCommand, btn, tr(T::CopyCommand), true);

    // What is missing on top, what to do below; no icon: the text needs the room.
    let text_x = r.x + 16.0;
    let room = btn.x - 12.0 - text_x;
    f.g.text(tr(T::AccessNeeded), Rect::new(text_x, r.y + 5.0, room, 20.0), Size::Body, Weight::Semibold, Align::Left, th().text);
    f.g.text(tr(T::AccessSteps), Rect::new(text_x, r.y + 25.0, room, 18.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_cue_follows_hidden_rows() {
        assert!(!more_below(0, LIST_ROWS));
        assert!(more_below(0, LIST_ROWS + 1));
        assert!(!more_below(1, LIST_ROWS + 1));

        // Thumb at the top, at the bottom, never shorter than THUMB_MIN.
        let track = Rect::new(0.0, 10.0, SCROLL_W, 200.0);
        let count = LIST_ROWS * 10;
        let t = thumb(track, 0, count);
        assert_eq!((t.y, t.h), (10.0, THUMB_MIN));
        let t = thumb(track, count - LIST_ROWS, count);
        assert_eq!(t.y + t.h, 210.0);
        assert_eq!(thumb(track, 0, LIST_ROWS).h, 200.0, "nothing hidden: full track");
    }
}
