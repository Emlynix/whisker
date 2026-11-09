//! Settings › Mouse tab (hardware info, diagnostics), the "which mouse
//! do you use?" dialog and the new-mouse banner.

use super::{Act, BODY, CW, Frame, GAP, Glyph, HEIGHT, Page, Place, R_CTRL, RADIUS, View, WIDTH, X0, icon, text_w, th};
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

/// A connected mouse in the dialog's "Detected" row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub title: String,
    pub kind: Badge,
    /// The mouse in use now.
    pub current: bool,
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

/// The "which mouse" dialog: detected mice, brands, models of one brand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detect {
    pub found: Vec<Found>,
    pub brands: Vec<BrandRow>,
    /// Models of the selected brand.
    pub models: Vec<DetectRow>,
    pub brand: usize,
    pub model: usize,
    /// The detected mouse the selection came from, if any.
    pub chip: Option<usize>,
    pub pane: Pane,
    /// First visible row of each list.
    pub brand_top: usize,
    pub model_top: usize,
    /// Arrow keys were used: outline the list they move in.
    pub keys: bool,
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
    let d = &v.device;
    let card = Rect::new(X0, BODY, CW, ROW * d.rows.len() as f32 + 16.0);
    f.card(card);

    for (i, (caption, value)) in d.rows.iter().enumerate() {
        let y = card.y + 8.0 + i as f32 * ROW;
        f.caption_in(tr(*caption), card.x + PAD, y + 6.0, 190.0);
        let (text, color) = if value.is_empty() { (tr(T::NotReported).to_string(), th().subtle) } else { (value.clone(), th().text) };
        let r = Rect::new(card.x + 230.0, y, card.w - 250.0, ROW);
        f.g.text(&text, r, Size::Body, Weight::Normal, Align::Left, color);
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

/// Raw status bytes and "Copy for support".
fn diagnostics(f: &mut Frame, d: &DeviceView, y: f32) {
    const BTN_W: f32 = 190.0;
    let card = Rect::new(X0, y, CW, 56.0);
    f.card(card);
    f.caption(tr(T::DevStatus), card.x + PAD, card.y + 8.0);
    let (text, color) = if d.status.is_empty() { (tr(T::NotReported), th().subtle) } else { (d.status.as_str(), th().text) };
    let r = Rect::new(card.x + PAD, card.y + 26.0, card.w - 2.0 * PAD - BTN_W - 12.0, 22.0);
    f.g.text(text, r, Size::Caption, Weight::Normal, Align::Left, color);
    let btn = Rect::new(card.x + card.w - PAD - BTN_W + 8.0, card.y + 10.0, BTN_W - 8.0, 36.0);
    f.push_button(Act::CopyDiagnostics, btn, tr(T::CopySupport), false);
}

// ---- "which mouse" dialog ----------------------------------------------------
//
//  +-----------------------------------------------------------+
//  | Which mouse do you use?                                   |
//  | DETECTED  (● White Shark Lionel · current) (● Corsair …)  |
//  | BRAND               MODEL                                 |
//  | ● White Shark    |  Lionel                     Supported  |
//  | ● Logitech       |  Other White Shark mouse  Experimental |
//  | …  (scrolls)     |  …  (scrolls)                          |
//  | ● Supported ● Experimental ● Partial ● Statistics only    |
//  | [‹ Back to detected]                     [Cancel] [Use]   |
//  +-----------------------------------------------------------+

const DLG_W: f32 = 700.0;
const DLG_H: f32 = 520.0;
const DLG_PAD: f32 = 24.0;
const CHIP_H: f32 = 32.0;
const LIST_ROW: f32 = 28.0;
/// Rows each list shows; longer lists scroll.
pub const LIST_ROWS: usize = 8;
/// "Mouse missing or experimental? Help add your mouse" under the models.
const HINT_H: f32 = 26.0;
const BRAND_W: f32 = 220.0;
const PANE_GAP: f32 = 16.0;
const BADGE_W: f32 = 170.0;

/// Dialog frame and the two list areas (brands, models).
struct Layout {
    dlg: Rect,
    inner: Rect,
    brands: Rect,
    models: Rect,
}

fn layout() -> Layout {
    let dlg = Rect::new((WIDTH - DLG_W) / 2.0, (HEIGHT - DLG_H) / 2.0, DLG_W, DLG_H);
    let inner = dlg.inset(DLG_PAD);
    let top = inner.y + 130.0;
    let h = LIST_ROWS as f32 * LIST_ROW;
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
    f.caption_in(tr(T::DetectedCaption), inner.x, inner.y + 42.0, inner.w);
    found_chips(f, d, Rect::new(inner.x, inner.y + 62.0, inner.w, CHIP_H));

    f.caption_in(tr(T::BrandCaption), l.brands.x, l.brands.y - 22.0, l.brands.w);
    f.caption_in(tr(T::ModelCaption), l.models.x, l.models.y - 22.0, l.models.w);
    brand_list(f, d, l.brands);
    model_list(f, d, l.models);
    report_hint(f, Rect::new(l.models.x, l.models.y + l.models.h + 4.0, l.models.w, HINT_H));
    if d.keys {
        // Which list the arrow keys move in.
        let r = if d.pane == Pane::Brands { l.brands } else { l.models };
        f.g.stroke(Rect::new(r.x - 4.0, r.y - 4.0, r.w + 8.0, r.h + 8.0), R_CTRL + 4.0, 2.0, th().accent);
    }

    let by = l.dlg.y + l.dlg.h - DLG_PAD - 36.0;
    legend(f, inner.x, by - 32.0);
    if !d.found.is_empty() {
        f.glyph_button(Act::DetectBack, Rect::new(inner.x, by, 200.0, 36.0), tr(T::BackToDetected), Glyph::ChevronLeft, Place::Lead);
    }
    let use_btn = Rect::new(inner.x + inner.w - 170.0, by, 170.0, 36.0);
    f.push_button(Act::DetectUse, use_btn, tr(T::UseMouse), true);
    f.push_button(Act::DetectCancel, Rect::new(use_btn.x - 10.0 - 120.0, by, 120.0, 36.0), tr(T::Cancel), false);
}

/// Detected mice as chips: support dot, name, "· current" on the one in use.
fn found_chips(f: &mut Frame, d: &Detect, row: Rect) {
    if d.found.is_empty() {
        f.g.text(tr(T::NoMouse), row, Size::Body, Weight::Normal, Align::Left, th().muted);
        return;
    }
    let mut x = row.x;
    for (i, m) in d.found.iter().enumerate() {
        let tag = if m.current { format!("{} · {}", m.title, tr(T::CurrentTag)) } else { m.title.clone() };
        let w = (text_w(&tag, Size::Body) + 40.0).min(row.x + row.w - x);
        if w < 80.0 {
            break;
        }
        let r = Rect::new(x, row.y, w, row.h);
        let selected = d.chip == Some(i);
        let hot = f.button(Act::DetectFound(i), r);
        let bg = if selected { th().accent_soft } else if hot { th().hover } else { th().card };
        f.g.fill(r, r.h / 2.0, th().card);
        f.g.fill(r, r.h / 2.0, bg);
        let (bw, bc) = if selected { (2.0, th().accent) } else { (1.0, th().border) };
        f.g.stroke(r, r.h / 2.0, bw, bc);
        f.g.circle(r.x + 16.0, r.y + r.h / 2.0, 4.0, badge_color(m.kind));
        let label = Rect::new(r.x + 26.0, r.y, r.w - 34.0, r.h);
        f.g.text(&tag, label, Size::Body, Weight::Semibold, Align::Left, th().text);
        x += w + 8.0;
    }
}

/// One list row: selected = soft fill and a left bar; hover = hover fill.
fn list_row(f: &mut Frame, act: Act, r: Rect, selected: bool) {
    let hot = f.button(act, r);
    if selected || hot {
        f.g.fill(r, R_CTRL, if selected { th().accent_soft } else { th().hover });
    }
    if selected {
        f.g.fill(Rect::new(r.x, r.y + 7.0, 3.0, r.h - 14.0), 1.5, th().accent);
    }
}

/// Thin scroll bar on the right of a list longer than `LIST_ROWS`.
fn scroll_bar(f: &mut Frame, area: Rect, top: usize, count: usize) {
    if count <= LIST_ROWS {
        return;
    }
    let track = Rect::new(area.x + area.w - 3.0, area.y, 3.0, area.h);
    f.g.fill(track, 1.5, th().border);
    let h = track.h * LIST_ROWS as f32 / count as f32;
    let y = track.y + (track.h - h) * top as f32 / (count - LIST_ROWS) as f32;
    f.g.fill(Rect::new(track.x, y, track.w, h), 1.5, th().subtle);
}

/// Brands with a dot for the best support among their models.
fn brand_list(f: &mut Frame, d: &Detect, area: Rect) {
    for (k, b) in d.brands.iter().enumerate().skip(d.brand_top).take(LIST_ROWS) {
        let r = Rect::new(area.x, area.y + (k - d.brand_top) as f32 * LIST_ROW, area.w - 8.0, LIST_ROW - 2.0);
        let sel = k == d.brand;
        list_row(f, Act::DetectBrand(k), r, sel);
        f.g.circle(r.x + 16.0, r.y + r.h / 2.0, 4.0, badge_color(b.kind));
        let weight = if sel { Weight::Semibold } else { Weight::Normal };
        let label = Rect::new(r.x + 28.0, r.y, r.w - 34.0, r.h);
        f.g.text(&b.name, label, Size::Body, weight, Align::Left, if sel { th().text } else { th().muted });
    }
    scroll_bar(f, area, d.brand_top, d.brands.len());
}

/// Models of the selected brand with their support badge.
fn model_list(f: &mut Frame, d: &Detect, area: Rect) {
    for (k, m) in d.models.iter().enumerate().skip(d.model_top).take(LIST_ROWS) {
        let r = Rect::new(area.x, area.y + (k - d.model_top) as f32 * LIST_ROW, area.w - 8.0, LIST_ROW - 2.0);
        let sel = k == d.model;
        list_row(f, Act::DetectSel(k), r, sel);
        let weight = if sel { Weight::Semibold } else { Weight::Normal };
        // The badge takes what it needs, the name the rest.
        let badge_w = (text_w(&m.badge, Size::Caption) + 8.0).min(BADGE_W);
        let name = Rect::new(r.x + 14.0, r.y, r.w - badge_w - 34.0, r.h);
        f.g.text(&m.title, name, Size::Body, weight, Align::Left, if sel { th().text } else { th().muted });
        let badge = Rect::new(r.x + r.w - badge_w - 10.0, r.y, badge_w, r.h);
        f.g.text(&m.badge, badge, Size::Caption, Weight::Semibold, Align::Right, badge_color(m.kind));
    }
    scroll_bar(f, area, d.model_top, d.models.len());
}

/// Next to the auto-detect and "other" entries: a way to get a missing
/// or experimental mouse supported.
fn report_hint(f: &mut Frame, r: Rect) {
    const SIZE: Size = Size::Caption;
    let link = tr(T::HelpAddMouse);
    let link_w = (text_w(link, SIZE) + 20.0).min(r.w / 2.0);
    let lr = Rect::new(r.x + r.w - link_w, r.y, link_w, r.h);
    let hot = f.button(Act::ReportOpen, lr);
    if hot {
        f.g.fill(lr, R_CTRL, th().hover);
    }
    f.g.text(link, Rect::new(lr.x + 8.0, lr.y, lr.w - 16.0, lr.h), SIZE, Weight::Semibold, Align::Right, th().accent);
    let hint = Rect::new(r.x + 14.0, r.y, r.w - link_w - 18.0, r.h);
    f.g.text(tr(T::MissingMouse), hint, SIZE, Weight::Normal, Align::Right, th().muted);
}

/// What the support dots mean.
fn legend(f: &mut Frame, x: f32, y: f32) {
    const SIZE: Size = Size::Caption;
    let items = [(Badge::Good, T::BadgeFull), (Badge::Warn, T::BadgeExperimental), (Badge::Partial, T::BadgePartial), (Badge::Muted, T::LegendStats)];
    let mut x = x;
    for (kind, key) in items {
        let label = tr(key);
        f.g.circle(x + 4.0, y + 9.0, 4.0, badge_color(kind));
        let w = text_w(label, SIZE) + 8.0;
        f.g.text(label, Rect::new(x + 14.0, y, w, 18.0), SIZE, Weight::Normal, Align::Left, th().muted);
        x += 14.0 + w + 18.0;
    }
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
