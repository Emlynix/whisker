//! New version: status bar notice, the update dialog, the About row and
//! the Settings switch.
//!
//! ```text
//!  status bar:  ● Version 0.9.1 is available            ● Connected · 80 %
//!  +--------------------------------------------------+
//!  | Update available                                 |
//!  | [logo] Whisker 0.9.1                 2026-11-20  |
//!  |        Installed: 0.9.0                          |
//!  | What's new                                       |
//!  | +----------------------------------------------+ |
//!  | | notes, plain text, at most NOTE_ROWS lines   | |
//!  | +----------------------------------------------+ |
//!  | Downloading and verifying… / error               |
//!  | [Skip this version]             [Later] [Update] |
//!  +--------------------------------------------------+
//! ```

use super::{Act, Frame, HEIGHT, PAGE_H, R_CTRL, RADIUS, STATUS_H, VERSION, WIDTH, mouse_logo, text_w, th};
use crate::gfx::{Align, Rect, Size, Weight};
use crate::i18n::{T, fill, tr};

const DLG_W: f32 = 520.0;
const DLG_H: f32 = 440.0;
const PAD: f32 = 24.0;
const BTN_H: f32 = 36.0;
const BTN_GAP: f32 = 10.0;
const LINE_H: f32 = 17.0;
/// Changelog lines shown; longer notes end in "…".
pub const NOTE_ROWS: usize = 10;
/// Width the changelog wraps at (area minus its padding).
const NOTE_W: f32 = DLG_W - 2.0 * PAD - 24.0;
const ELLIPSIS: &str = "…";

/// Result of the last check, shown in About.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Probe {
    Idle,
    Checking,
    Current,
    Available,
    Failed,
}

/// Where the dialog is in installing the offer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Ready,
    Downloading,
    Failed,
}

/// What "Update" does on this system.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Install {
    /// Download, verify, replace, restart.
    InPlace,
    /// Open the release page.
    Page,
}

/// A newer release as shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
    pub date: String,
    /// Changelog wrapped to the dialog (`note_lines`).
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateView {
    /// Daily check switched on (Settings).
    pub auto: bool,
    pub probe: Probe,
    pub offer: Option<Offer>,
    /// Status bar notice: an offer the user has not skipped.
    pub notice: bool,
    pub step: Step,
    pub install: Install,
}

impl UpdateView {
    pub fn new(auto: bool, install: Install) -> Self {
        Self { auto, probe: Probe::Idle, offer: None, notice: false, step: Step::Ready, install }
    }
}

/// Changelog wrapped at the dialog width, at most `NOTE_ROWS` lines; runs
/// of blank lines collapse to one, overlong words break anywhere.
pub fn note_lines(notes: &str) -> Vec<String> {
    let fits = |s: &str| text_w(s, Size::Caption) <= NOTE_W;
    let mut lines: Vec<String> = Vec::new();
    for para in notes.trim().split('\n') {
        let para = para.trim_end();
        if para.is_empty() {
            if lines.last().is_some_and(|l| !l.is_empty()) {
                lines.push(String::new());
            }
            continue;
        }
        let mut line = String::new();
        for word in para.split(' ') {
            let joined = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
            if fits(&joined) {
                line = joined;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // A word wider than the area (or CJK text): by characters.
            for ch in word.chars() {
                if !fits(&format!("{line}{ch}")) {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        lines.push(line);
    }
    if lines.len() > NOTE_ROWS {
        lines.truncate(NOTE_ROWS);
        if let Some(last) = lines.last_mut() {
            last.push_str(ELLIPSIS);
        }
    }
    lines
}

/// Left side of the status bar: "● Version 0.9.1 is available", opens the dialog.
pub(super) fn status_notice(f: &mut Frame, u: &UpdateView) {
    // Left edge flush with the cards.
    const X: f32 = super::X0;
    let Some(o) = u.offer.as_ref().filter(|_| u.notice) else {
        return;
    };
    let text = fill(tr(T::UpdateAvailable), &[&o.version]);
    let w = text_w(&text, Size::Caption) + 14.0;
    let r = Rect::new(X, PAGE_H + (STATUS_H - 16.0) / 2.0, w, 16.0);
    let area = Rect::new(r.x - 8.0, PAGE_H + 3.0, r.w + 16.0, STATUS_H - 6.0);
    if f.button(Act::UpdateShow, area) {
        f.g.fill(area, R_CTRL / 2.0, th().hover);
    }
    f.g.circle(r.x + 3.5, r.y + r.h / 2.0, 3.5, th().accent);
    let label = Rect::new(r.x + 12.0, r.y, r.w - 12.0, r.h);
    f.g.text(&text, label, Size::Caption, Weight::Semibold, Align::Left, th().accent);
}

/// About: "Check for updates" and what the last check found.
pub(super) fn about_row(f: &mut Frame, u: &UpdateView, x: f32, y: f32, w: f32) {
    const H: f32 = 32.0;
    let label = tr(T::UpdateCheck);
    let bw = text_w(label, Size::Body) + 36.0;
    let b = Rect::new(x, y, bw, H);
    if u.probe == Probe::Checking {
        f.dead_button(b, label);
    } else {
        f.push_button(Act::UpdateCheck, b, label, false);
    }

    let r = Rect::new(x + bw + 12.0, y, w - bw - 12.0, H);
    let (text, color) = match u.probe {
        Probe::Idle => return,
        Probe::Checking => (tr(T::UpdateChecking).to_string(), th().muted),
        Probe::Current => (tr(T::UpToDate).to_string(), th().good),
        Probe::Failed => (tr(T::UpdateCheckFailed).to_string(), th().bad),
        Probe::Available => {
            let Some(o) = &u.offer else {
                return;
            };
            // Opens the dialog, like the status bar notice.
            let text = fill(tr(T::UpdateAvailable), &[&o.version]);
            let link = Rect::new(r.x, r.y, (text_w(&text, Size::Caption) + 4.0).min(r.w), r.h);
            let hot = f.button(Act::UpdateShow, link);
            let c = if hot { th().text } else { th().accent };
            f.g.text(&text, link, Size::Caption, Weight::Semibold, Align::Left, c);
            return;
        }
    };
    f.g.text(&text, r, Size::Caption, Weight::Normal, Align::Left, color);
}

/// Settings › General card: the daily check switch.
pub(super) fn settings_card(f: &mut Frame, u: &UpdateView, card: Rect) {
    f.card(card);
    let title = Rect::new(card.x + 18.0, card.y + 14.0, card.w - 100.0, 22.0);
    f.g.text(tr(T::UpdateAuto), title, Size::Large, Weight::Semibold, Align::Left, th().text);
    let desc = Rect::new(card.x + 18.0, card.y + 40.0, card.w - 100.0, 34.0);
    f.g.text(tr(T::UpdateAutoDesc), desc, Size::Caption, Weight::Normal, Align::Wrap, th().muted);
    f.toggle(Some(Act::UpdateAuto), card.x + card.w - 58.0, card.y + (card.h - 22.0) / 2.0, u.auto);
}

/// Buttons right-aligned from `right`; the last one is primary.
fn button_row(f: &mut Frame, right: f32, y: f32, items: &[(Act, &str)]) {
    let mut x = right;
    for (k, (act, label)) in items.iter().enumerate().rev() {
        let w = text_w(label, Size::Body) + 36.0;
        x -= w;
        f.push_button(*act, Rect::new(x, y, w, BTN_H), label, k == items.len() - 1);
        x -= BTN_GAP;
    }
}

pub(super) fn update_dialog(f: &mut Frame, u: &UpdateView) {
    let Some(o) = &u.offer else {
        return;
    };
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    let d = Rect::new((WIDTH - DLG_W) / 2.0, (HEIGHT - DLG_H) / 2.0, DLG_W, DLG_H);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let inner = d.inset(PAD);
    f.caption_in(tr(T::UpdateTitle), inner.x, inner.y, inner.w);
    mouse_logo(f.g, inner.x + 4.0, inner.y + 34.0);
    let name = format!("{} {}", crate::APP_NAME, o.version);
    let name_r = Rect::new(inner.x + 40.0, inner.y + 26.0, inner.w - 160.0, 30.0);
    f.g.text(&name, name_r, Size::Subtitle, Weight::Bold, Align::Left, th().text);
    let date_r = Rect::new(inner.x + inner.w - 120.0, inner.y + 32.0, 120.0, 20.0);
    f.g.text(&o.date, date_r, Size::Caption, Weight::Normal, Align::Right, th().muted);
    let installed = fill(tr(T::UpdateInstalled), &[VERSION]);
    f.g.text(&installed, Rect::new(name_r.x, inner.y + 56.0, name_r.w, 18.0), Size::Caption, Weight::Normal, Align::Left, th().muted);

    // Changelog: plain text, pre-wrapped.
    f.caption_in(tr(T::UpdateNotes), inner.x, inner.y + 92.0, inner.w);
    let area = Rect::new(inner.x, inner.y + 114.0, inner.w, NOTE_ROWS as f32 * LINE_H + 16.0);
    f.g.fill(area, R_CTRL, th().card);
    f.g.stroke(area, R_CTRL, 1.0, th().border);
    for (k, line) in o.notes.iter().take(NOTE_ROWS).enumerate() {
        let lr = Rect::new(area.x + 12.0, area.y + 8.0 + k as f32 * LINE_H, area.w - 24.0, LINE_H);
        f.g.text(line, lr, Size::Caption, Weight::Normal, Align::Left, th().text);
    }

    let status = Rect::new(inner.x, area.y + area.h + 8.0, inner.w, 36.0);
    match u.step {
        Step::Ready => {}
        Step::Downloading => f.g.text(tr(T::UpdateDownloading), status, Size::Caption, Weight::Normal, Align::Wrap, th().muted),
        Step::Failed => f.g.text(tr(T::UpdateFailed), status, Size::Caption, Weight::Normal, Align::Wrap, th().bad),
    }

    let by = d.y + d.h - PAD - BTN_H;
    let right = inner.x + inner.w;
    let skip = tr(T::UpdateSkip);
    let skip_r = Rect::new(inner.x, by, text_w(skip, Size::Body) + 36.0, BTN_H);
    let install = match u.install {
        Install::InPlace => (Act::UpdateInstall, tr(T::UpdateNow)),
        Install::Page => (Act::UpdatePage, tr(T::UpdatePage)),
    };
    match u.step {
        Step::Ready => {
            f.push_button(Act::UpdateSkip, skip_r, skip, false);
            button_row(f, right, by, &[(Act::UpdateLater, tr(T::UpdateLater)), install]);
        }
        Step::Downloading => {
            // Later hides the dialog; the download goes on.
            f.dead_button(skip_r, skip);
            let now = tr(T::UpdateNow);
            let now_w = text_w(now, Size::Body) + 36.0;
            f.dead_button(Rect::new(right - now_w, by, now_w, BTN_H), now);
            button_row(f, right - now_w - BTN_GAP, by, &[(Act::UpdateLater, tr(T::UpdateLater))]);
        }
        Step::Failed => button_row(f, right, by, &[(Act::CloseDialog, tr(T::Close)), (Act::UpdatePage, tr(T::UpdatePage))]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_wrap_and_cap() {
        let lines = note_lines("Short line.\n\n\n\n- second");
        assert_eq!(lines, vec!["Short line.", "", "- second"]);

        let long = "word ".repeat(200);
        let lines = note_lines(&long);
        assert_eq!(lines.len(), NOTE_ROWS);
        assert!(lines.iter().all(|l| text_w(l.trim_end_matches(ELLIPSIS), Size::Caption) <= NOTE_W));
        assert!(lines.last().unwrap().ends_with(ELLIPSIS));

        // No spaces (CJK, a long URL): broken by characters.
        let lines = note_lines(&"字".repeat(100));
        assert!(lines.len() > 1 && lines.iter().all(|l| text_w(l, Size::Caption) <= NOTE_W));
        assert!(note_lines("").is_empty());
    }
}
