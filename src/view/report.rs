//! "Help add your mouse" dialog: pick a connected mouse, read it, show the
//! report; optional guided checks for experimental drivers.
//!
//! ```text
//!  +--------------------------------------------------------------+
//!  | Help add your mouse                           Step 1 of 3    |
//!  | Whisker reads what the mouse reports about itself …          |
//!  | CONNECTED MICE                                               |
//!  |  ▌ Acme Glide          1234:5678 · USB                       |
//!  |    SSCYPL Receiver     248A:5B2F · USB · current             |
//!  |                                  [Cancel] [Read details]     |
//!  +--------------------------------------------------------------+
//!   step 3: report text (scrolls), "Check settings" link above it
//!           [Close]                       [Copy] [Open GitHub issue] [Send]
//!   log:    the diagnostic log in the same view · [Close]  [Copy] [Send]
//!   check:  question, value · [Cancel checks] [Skip] [No] [Yes]
//! ```

use super::{Act, Frame, HEIGHT, R_CTRL, RADIUS, WIDTH, text_link, text_w, th};
use crate::gfx::{Align, Rect, Size, Weight};
use crate::i18n::{T, fill, tr};

/// The user's answer to a guided check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Yes,
    No,
    Skip,
    /// Rate check: measuring finished.
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Listing the connected mice.
    Finding,
    Pick,
    /// Probe running on the mouse thread.
    Reading,
    Result,
    Check,
}

/// What the dialog shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    /// "Help add your mouse": pick, read, report.
    #[default]
    Mouse,
    /// The diagnostic log after a crash or hang: the text only.
    Log,
}

/// Where sending the text stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sending {
    /// "Send" is offered.
    #[default]
    Ready,
    Busy,
    /// Sent: not offered again for the same text.
    Done,
}

/// Buttons a check offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    YesNo,
    /// "Done" after measuring.
    Measure,
}

/// One guided check as shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckView {
    /// "Check 1 of 3 · DPI".
    pub title: String,
    pub question: String,
    /// Live value, e.g. "Measured: 124 Hz"; may be empty.
    pub value: String,
    pub ask: Ask,
}

/// A connected mouse in the pick list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRow {
    pub title: String,
    pub sub: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportView {
    pub kind: Kind,
    pub step: Step,
    pub rows: Vec<PickRow>,
    pub sel: usize,
    /// Report text, line by line (English).
    pub lines: Vec<String>,
    /// First visible report line.
    pub top: usize,
    /// Below the report, e.g. "Copied to clipboard".
    pub note: String,
    /// The note tells of a failure.
    pub note_bad: bool,
    pub sending: Sending,
    /// Id of the report once sent; stays while the dialog is open.
    pub sent: Option<String>,
    /// "Check settings" is offered (experimental driver, this mouse).
    pub can_check: bool,
    pub check: Option<CheckView>,
}

const DLG_W: f32 = 720.0;
const DLG_H: f32 = 500.0;
const PAD: f32 = 24.0;
const BTN_H: f32 = 36.0;
const ROW_H: f32 = 40.0;
/// Pick list rows shown at once.
pub const PICK_ROWS: usize = 6;
const LINE_H: f32 = 17.0;
/// Report lines shown at once; the rest scrolls.
pub const REPORT_ROWS: usize = 17;
const BTN_GAP: f32 = 10.0;
/// Space between the text area's border and its lines.
const TEXT_PAD: f32 = 12.0;
/// `text_w` is an estimate: lines are wrapped a little early, so the
/// platform never cuts one off.
const WRAP_MARGIN: f32 = 0.9;

/// `text` as the dialog shows it: every line whole, long ones wrapped.
/// What is sent must be readable to its end; nothing scrolls sideways.
pub fn report_lines(text: &str) -> Vec<String> {
    let room = (DLG_W - 2.0 * PAD - 2.0 * TEXT_PAD) * WRAP_MARGIN;
    let fits = |s: &str| text_w(s, Size::Caption) <= room;
    text.lines().flat_map(|l| wrap(l, &fits)).collect()
}

/// `line` in pieces that fit, broken after a space where there is one.
fn wrap(line: &str, fits: &impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while !fits(rest) {
        // Longest start that fits; one character at least, to move on.
        let mut end = rest.chars().next().map_or(0, char::len_utf8);
        for (at, c) in rest.char_indices() {
            let next = at + c.len_utf8();
            if !fits(&rest[..next]) {
                break;
            }
            end = next;
        }
        if end >= rest.len() {
            break;
        }
        // A word that ends right here stays whole; else back to the last space.
        let cut = match rest[end..].starts_with(' ') {
            true => end + 1,
            false => rest[..end].rfind(' ').filter(|&space| space > 0).map_or(end, |space| space + 1),
        };
        out.push(rest[..cut].trim_end().to_string());
        rest = &rest[cut..];
    }
    out.push(rest.to_string());
    out
}

fn dialog_rect() -> Rect {
    Rect::new((WIDTH - DLG_W) / 2.0, (HEIGHT - DLG_H) / 2.0, DLG_W, DLG_H)
}

/// Button width for its label.
fn btn_w(label: &str) -> f32 {
    text_w(label, Size::Body) + 36.0
}

/// Buttons right-aligned from the right edge; the last one is primary.
fn button_row(f: &mut Frame, right: f32, y: f32, items: &[(Act, &str)]) {
    let mut x = right;
    for (k, (act, label)) in items.iter().enumerate().rev() {
        let w = btn_w(label);
        x -= w;
        f.push_button(*act, Rect::new(x, y, w, BTN_H), label, k == items.len() - 1);
        x -= BTN_GAP;
    }
}

pub(super) fn report_dialog(f: &mut Frame, r: &ReportView) {
    let d = dialog_rect();
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let inner = d.inset(PAD);
    let title = if r.kind == Kind::Log { T::CrashTitle } else { T::HelpAddMouse };
    f.g.text(tr(title), Rect::new(inner.x, inner.y, inner.w - 160.0, 28.0), Size::Subtitle, Weight::Bold, Align::Left, th().text);
    let step = match r.step {
        Step::Finding | Step::Pick => 1,
        Step::Reading => 2,
        Step::Result | Step::Check => 3,
    };
    // The log has no steps before it.
    if r.kind == Kind::Mouse {
        let of = fill(tr(T::StepOf), &[&step.to_string(), "3"]);
        f.g.text(&of, Rect::new(inner.x + inner.w - 160.0, inner.y + 4.0, 160.0, 20.0), Size::Caption, Weight::Normal, Align::Right, th().muted);
    }

    let body = Rect::new(inner.x, inner.y + 40.0, inner.w, inner.h - 40.0 - BTN_H - 16.0);
    let by = d.y + d.h - PAD - BTN_H;
    let right = inner.x + inner.w;
    match r.step {
        Step::Finding | Step::Reading => {
            let t = if r.step == Step::Finding { T::ReportFinding } else { T::ReportReading };
            f.g.text(tr(t), Rect::new(body.x, body.y, body.w, 22.0), Size::Body, Weight::Normal, Align::Left, th().muted);
            button_row(f, right, by, &[(Act::ReportClose, tr(T::Cancel))]);
        }
        Step::Pick => pick(f, r, body, right, by),
        Step::Result => result(f, r, body, right, by),
        Step::Check => check(f, r, body, right, by),
    }
}

/// Step 1: intro and the connected mice.
fn pick(f: &mut Frame, r: &ReportView, body: Rect, right: f32, by: f32) {
    f.g.text(tr(T::ReportIntro), Rect::new(body.x, body.y, body.w, 40.0), Size::Body, Weight::Normal, Align::Wrap, th().muted);
    f.caption_in(tr(T::ReportPickCaption), body.x, body.y + 56.0, body.w);

    let top = body.y + 78.0;
    if r.rows.is_empty() {
        f.g.text(tr(T::ReportNoMouse), Rect::new(body.x, top, body.w, 40.0), Size::Body, Weight::Normal, Align::Wrap, th().muted);
        button_row(f, right, by, &[(Act::ReportClose, tr(T::Cancel)), (Act::ReportOpen, tr(T::SearchAgain))]);
        return;
    }
    // Longer lists follow the selection (arrow keys, wheel).
    let first = r.sel.saturating_sub(PICK_ROWS - 1);
    for (k, row) in r.rows.iter().enumerate().skip(first).take(PICK_ROWS) {
        let rr = Rect::new(body.x, top + (k - first) as f32 * ROW_H, body.w, ROW_H - 4.0);
        let sel = k == r.sel;
        let hot = f.button(Act::ReportSel(k), rr);
        if sel || hot {
            f.g.fill(rr, R_CTRL, if sel { th().accent_soft } else { th().hover });
        }
        if sel {
            f.g.fill(Rect::new(rr.x, rr.y + 9.0, 3.0, rr.h - 18.0), 1.5, th().accent);
        }
        let weight = if sel { Weight::Semibold } else { Weight::Normal };
        let half = (rr.w - 28.0) / 2.0;
        f.g.text(&row.title, Rect::new(rr.x + 14.0, rr.y, half, rr.h), Size::Body, weight, Align::Left, th().text);
        f.g.text(&row.sub, Rect::new(rr.x + 14.0 + half, rr.y, half, rr.h), Size::Caption, Weight::Normal, Align::Right, th().muted);
    }
    button_row(f, right, by, &[(Act::ReportClose, tr(T::Cancel)), (Act::ReportRead, tr(T::ReportRead))]);
}

/// Step 3: the report text, scrollable, and what to do with it.
fn result(f: &mut Frame, r: &ReportView, body: Rect, right: f32, by: f32) {
    let caption = if r.kind == Kind::Log { T::LogCaption } else { T::ReportCaption };
    f.caption_in(tr(caption), body.x, body.y, body.w);
    // Beside the caption: the button row is for what happens to the text.
    // Not while sending: the checks would change the text under way.
    if r.can_check && r.sending != Sending::Busy {
        let label = tr(T::ReportCheck);
        text_link(f, Act::ReportCheck, label, right - text_w(label, Size::Caption) - 4.0, body.y - 2.0);
    }
    let area = Rect::new(body.x, body.y + 22.0, body.w, REPORT_ROWS as f32 * LINE_H + 16.0);
    f.g.fill(area, R_CTRL, th().card);
    f.g.stroke(area, R_CTRL, 1.0, th().border);
    for (k, line) in r.lines.iter().skip(r.top).take(REPORT_ROWS).enumerate() {
        let lr = Rect::new(area.x + TEXT_PAD, area.y + 8.0 + k as f32 * LINE_H, area.w - 2.0 * TEXT_PAD, LINE_H);
        f.g.text(line, lr, Size::Caption, Weight::Normal, Align::Left, th().text);
    }
    scroll_bar(f, area, r.top, r.lines.len());
    status_line(f, r, Rect::new(body.x, area.y + area.h + 6.0, body.w, 18.0));

    let close = tr(T::Close);
    f.push_button(Act::ReportClose, Rect::new(body.x, by, btn_w(close), BTN_H), close, false);
    let mut items = vec![(Act::ReportCopy, tr(T::Copy))];
    if r.kind == Kind::Mouse {
        items.push((Act::ReportIssue, tr(T::ReportIssue)));
    }
    // Last, so it is the primary button; gone once the text was sent.
    if r.sending == Sending::Ready {
        items.push((Act::ReportSend, tr(T::Send)));
    }
    button_row(f, right, by, &items);
}

/// Under the text. Once sent: the report id, to stay (it is what the
/// user quotes later), with a link that copies it; other notes move to
/// the right. Before that: the last note, else where "Send" takes the text.
fn status_line(f: &mut Frame, r: &ReportView, nr: Rect) {
    let color = if r.note_bad { th().bad } else { th().good };
    let Some(id) = &r.sent else {
        if r.note.is_empty() {
            f.g.text(tr(T::SendHint), nr, Size::Caption, Weight::Normal, Align::Left, th().muted);
        } else {
            f.g.text(&r.note, nr, Size::Caption, Weight::Semibold, Align::Left, color);
        }
        return;
    };

    let sent = fill(tr(T::Sent), &[id]);
    f.g.text(&sent, nr, Size::Caption, Weight::Semibold, Align::Left, th().good);
    text_link(f, Act::ReportCopyId, tr(T::CopyId), nr.x + text_w(&sent, Size::Caption) + 14.0, nr.y);
    f.g.text(&r.note, nr, Size::Caption, Weight::Semibold, Align::Right, color);
}

/// Thin bar on the right of a report longer than the area.
fn scroll_bar(f: &mut Frame, area: Rect, top: usize, count: usize) {
    if count <= REPORT_ROWS {
        return;
    }
    let track = Rect::new(area.x + area.w - 6.0, area.y + 6.0, 3.0, area.h - 12.0);
    let h = track.h * REPORT_ROWS as f32 / count as f32;
    let y = track.y + (track.h - h) * top as f32 / (count - REPORT_ROWS) as f32;
    f.g.fill(Rect::new(track.x, y, track.w, h), 1.5, th().subtle);
}

/// A guided check: what changed, the question, the answers.
fn check(f: &mut Frame, r: &ReportView, body: Rect, right: f32, by: f32) {
    let Some(c) = &r.check else {
        return;
    };
    f.g.text(tr(T::CheckIntro), Rect::new(body.x, body.y, body.w, 40.0), Size::Body, Weight::Normal, Align::Wrap, th().muted);
    let card = Rect::new(body.x, body.y + 56.0, body.w, 176.0);
    f.card(card);
    let inner = card.inset(20.0);
    f.caption_in(&c.title, inner.x, inner.y, inner.w);
    f.g.text(&c.question, Rect::new(inner.x, inner.y + 24.0, inner.w, 74.0), Size::Large, Weight::Semibold, Align::Wrap, th().text);
    f.g.text(&c.value, Rect::new(inner.x, inner.y + 106.0, inner.w, 22.0), Size::Body, Weight::Normal, Align::Left, th().accent);

    let cancel = tr(T::CheckCancel);
    f.push_button(Act::ReportCancelChecks, Rect::new(body.x, by, btn_w(cancel), BTN_H), cancel, false);
    let skip = (Act::ReportAnswer(Answer::Skip), tr(T::Skip));
    match c.ask {
        Ask::YesNo => button_row(f, right, by, &[skip, (Act::ReportAnswer(Answer::No), tr(T::No)), (Act::ReportAnswer(Answer::Yes), tr(T::Yes))]),
        Ask::Measure => button_row(f, right, by, &[skip, (Act::ReportAnswer(Answer::Done), tr(T::Done))]),
    }
}

/// The dialog contains the point (wheel scrolls the report).
pub fn report_at(x: f32, y: f32) -> bool {
    dialog_rect().contains(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ten characters fit.
    fn ten(s: &str) -> bool {
        s.chars().count() <= 10
    }

    #[test]
    fn long_lines_wrap_so_all_of_the_text_is_seen() {
        assert_eq!(wrap("short", &ten), ["short"]);
        assert_eq!(wrap("", &ten), [""]);
        // After a space where there is one.
        assert_eq!(wrap("mice in 38 ms: none", &ten), ["mice in 38", "ms: none"]);
        // No space: cut where it must be.
        assert_eq!(wrap("0123456789ABCDEF", &ten), ["0123456789", "ABCDEF"]);
        // Nothing is lost.
        let line = "features (index:id): 00:0000, 01:0001, 02:0003";
        assert_eq!(wrap(line, &ten).concat().replace(' ', ""), line.replace(' ', ""));
    }

    #[test]
    fn wrapping_ends_even_when_nothing_fits() {
        assert_eq!(wrap("ab", &|_: &str| false), ["a", "b"]);
    }

    #[test]
    fn report_lines_keep_short_lines_and_blank_ones() {
        assert_eq!(report_lines("### Title\n\n- one"), ["### Title", "", "- one"]);
        let long = "x".repeat(400);
        assert!(report_lines(&long).len() > 1);
    }
}
