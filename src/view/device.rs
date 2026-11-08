//! Device page (hardware info) and the "which mouse do you use?" dialog.

use super::{Act, CW, Frame, HEIGHT, RADIUS, TOP, View, WIDTH, X0, th};
use crate::gfx::{Align, Color, Rect, Weight};
use crate::i18n::{T, tr};

const ROW: f32 = 30.0;
const PAD: f32 = 20.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Badge {
    Good,
    Warn,
    Muted,
}

/// One selectable line of the detect dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectRow {
    pub title: String,
    pub sub: String,
    pub badge: String,
    pub kind: Badge,
    /// Model id to store, or `store::GENERIC_DEVICE`.
    pub choice: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detect {
    pub rows: Vec<DetectRow>,
    pub sel: usize,
    /// Showing the model list instead of detected mice.
    pub manual: bool,
    /// Nothing was detected.
    pub empty: bool,
}

/// Device page content prepared by the controller: (caption, value) rows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceView {
    pub rows: Vec<(T, String)>,
    pub offline: bool,
    /// Driver not yet verified on hardware: ask for reports.
    pub experimental: bool,
}

fn badge_color(k: Badge) -> Color {
    match k {
        Badge::Good => th().good,
        Badge::Warn => th().warn,
        Badge::Muted => th().faint,
    }
}

pub(super) fn device_page(f: &mut Frame, v: &View) {
    let d = &v.device;
    let card = Rect::new(X0, TOP, CW, ROW * d.rows.len() as f32 + 24.0);
    f.card(card);

    for (i, (caption, value)) in d.rows.iter().enumerate() {
        let y = card.y + 12.0 + i as f32 * ROW;
        f.caption_in(tr(*caption), card.x + PAD, y + 8.0, 190.0);
        let (text, color) = if value.is_empty() { (tr(T::NotReported).to_string(), th().faint) } else { (value.clone(), th().text) };
        let r = Rect::new(card.x + 230.0, y, card.w - 250.0, ROW);
        let size = super::fit_size(&text, r.w, 13.5);
        f.g.text(&text, r, size, Weight::Normal, Align::Left, color);
    }

    let mut y = card.y + card.h + 14.0;
    if d.experimental {
        let r = Rect::new(X0, y, CW, 36.0);
        f.g.text(tr(T::ExperimentalNote), r, 12.0, Weight::Normal, Align::Wrap, th().warn);
        y += 44.0;
    }
    f.push_button(Act::ChangeMouse, Rect::new(X0, y, 180.0, 36.0), tr(T::ChangeMouse), false);
    if d.offline {
        let r = Rect::new(X0 + 196.0, y, CW - 196.0, 36.0);
        f.g.text(tr(T::DevOffline), r, 12.0, Weight::Normal, Align::Left, th().muted);
    }
}

pub(super) fn detect_dialog(f: &mut Frame, d: &Detect) {
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    let dlg = Rect::new((WIDTH - 600.0) / 2.0, (HEIGHT - 450.0) / 2.0, 600.0, 450.0);
    f.g.fill(dlg, RADIUS, th().card_hi);
    f.g.stroke(dlg, RADIUS, 1.0, th().border);

    let inner = dlg.inset(24.0);
    f.g.text(tr(T::DetectTitle), Rect::new(inner.x, inner.y, inner.w, 28.0), 18.0, Weight::Bold, Align::Left, th().text);
    let text = if d.empty && !d.manual { tr(T::NoMouse) } else { tr(T::DetectText) };
    f.g.text(text, Rect::new(inner.x, inner.y + 36.0, inner.w, 40.0), 13.0, Weight::Normal, Align::Wrap, th().muted);

    const ROW_H: f32 = 58.0;
    const MAX_ROWS: usize = 4;
    for (i, r) in d.rows.iter().take(MAX_ROWS).enumerate() {
        let rr = Rect::new(inner.x, inner.y + 88.0 + i as f32 * (ROW_H + 8.0), inner.w, ROW_H);
        let hot = f.button(Act::DetectSel(i), rr);
        let sel = i == d.sel;
        f.g.fill(rr, 10.0, if sel { th().accent_soft } else if hot { th().hover } else { th().card });
        let (bw, bc) = if sel { (2.0, th().accent) } else { (1.0, th().border) };
        f.g.stroke(rr, 10.0, bw, bc);

        let left = Rect::new(rr.x + 16.0, rr.y + 9.0, rr.w - 250.0, 22.0);
        f.g.text(&r.title, left, 14.5, Weight::Semibold, Align::Left, th().text);
        let sub = Rect::new(rr.x + 16.0, rr.y + 31.0, rr.w - 250.0, 18.0);
        f.g.text(&r.sub, sub, 12.0, Weight::Normal, Align::Left, th().muted);
        let badge = Rect::new(rr.x + rr.w - 236.0, rr.y, 220.0, rr.h);
        let size = super::fit_size(&r.badge, badge.w, 12.5);
        f.g.text(&r.badge, badge, size, Weight::Semibold, Align::Right, badge_color(r.kind));
    }

    let by = dlg.y + dlg.h - 24.0 - 36.0;
    if !d.manual {
        f.push_button(Act::DetectManual, Rect::new(inner.x, by, 230.0, 36.0), tr(T::ChooseManually), false);
    }
    if !d.rows.is_empty() {
        let use_btn = Rect::new(dlg.x + dlg.w - 24.0 - 180.0, by, 180.0, 36.0);
        f.push_button(Act::DetectUse, use_btn, tr(T::UseMouse), true);
    }
}
