//! Drawing kit of an attached edition (`crate::edition`): the controls
//! of the pages, handed out through a narrow public face. A page drawn
//! with it looks like the built-in ones and cannot reach the rest of
//! the view.
//!
//! ```text
//!  edition page  ->  Ui (this file)  ->  Frame (private)  ->  Gfx
//!  control id u16 ->  Act::Ext(id)   ->  hit of the frame
//! ```

use super::{Act, CW, Frame, PAGE_H, RowPlace, Segment, SettingRow, TOP, X0, pro, text_link, th};
use crate::gfx::{Align, Color, Rect, Size, Weight};

/// Height of a segmented control.
pub const SEGMENT_H: f32 = super::SEGMENT_H;
/// Header row of a panel.
pub const PANEL_HEAD: f32 = super::PANEL_HEAD;
/// Padding inside a panel.
pub const PANEL_PAD: f32 = super::PANEL_PAD;
/// Space between two panels.
pub const PANEL_GAP: f32 = super::PANEL_GAP;
/// Height of a checkbox with its label.
pub const CHECK_H: f32 = 22.0;
/// Space kept free above the status bar.
const BOTTOM_PAD: f32 = 12.0;

/// Colors of the current theme, by role.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub panel: Color,
    pub sel: Color,
    pub line: Color,
    pub line2: Color,
    pub text: Color,
    pub muted: Color,
    pub acc: Color,
    pub acc_fill: Color,
    pub acc_on: Color,
    pub acc_soft: Color,
    pub input: Color,
    pub track: Color,
    pub ok: Color,
    pub warn: Color,
    pub bad: Color,
}

/// The current theme's colors.
pub fn palette() -> Palette {
    let t = th();
    Palette {
        bg: t.bg,
        panel: t.panel,
        sel: t.sel,
        line: t.line,
        line2: t.line2,
        text: t.text,
        muted: t.muted,
        acc: t.acc,
        acc_fill: t.acc_fill,
        acc_on: t.acc_on,
        acc_soft: t.acc_soft,
        input: t.input,
        track: t.track,
        ok: t.ok,
        warn: t.warn,
        bad: t.bad,
    }
}

/// Estimated width of `text` in DIPs.
pub fn text_w(text: &str, size: Size) -> f32 {
    super::text_w(text, size)
}

/// One cell of a segmented control.
pub struct Choice {
    pub id: u16,
    pub label: String,
    pub selected: bool,
}

/// A setting row: its name and a short description left, the control right.
pub struct Row<'a> {
    pub title: &'a str,
    pub desc: &'a str,
    /// Height of the control itself.
    pub control_h: f32,
}

impl Row<'_> {
    fn inner(&self) -> SettingRow<'_> {
        SettingRow { title: self.title, desc: self.desc, control_h: self.control_h }
    }

    /// Height of the row in a panel `w` wide.
    pub fn height(&self, w: f32) -> f32 {
        self.inner().height(w)
    }
}

/// Whether a row is the first of its panel (no hairline above it).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Order {
    First,
    Next,
}

/// Weight of a button in its panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rank {
    Primary,
    Plain,
}

/// State of a checkbox.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    On,
    Off,
}

/// The page of an edition while it is drawn.
pub struct Ui<'a, 'b> {
    f: &'a mut Frame<'b>,
}

impl Ui<'_, '_> {
    /// Where the page may draw: under the page title, above the status bar.
    pub fn area(&self) -> Rect {
        Rect::new(X0, TOP, CW, PAGE_H - TOP - BOTTOM_PAD)
    }

    /// A clickable area that sends `id`; true while the pointer is on it.
    pub fn hit(&mut self, id: u16, r: Rect) -> bool {
        self.f.button(Act::Ext(id), r)
    }

    pub fn text(&mut self, s: &str, r: Rect, size: Size, weight: Weight, align: Align, c: Color) {
        self.f.g.text(s, r, size, weight, align, c);
    }

    pub fn fill(&mut self, r: Rect, c: Color) {
        self.f.g.fill(r, 0.0, c);
    }

    /// 1 px outline inside `r`.
    pub fn stroke(&mut self, r: Rect, c: Color) {
        self.f.g.stroke(r, 0.0, 1.0, c);
    }

    pub fn polyline(&mut self, pts: &[(f32, f32)], width: f32, c: Color) {
        self.f.g.polyline(pts, width, c);
    }

    /// A panel without a header.
    pub fn card(&mut self, r: Rect) {
        self.f.card(r);
    }

    /// A panel with a header row; returns the body under the header.
    pub fn panel(&mut self, r: Rect, title: &str) -> Rect {
        self.f.panel(r, title)
    }

    /// Caption on the right of a panel's header.
    pub fn panel_note(&mut self, r: Rect, note: &str) {
        self.f.panel_note(r, note);
    }

    /// Name and description of a setting row at `y`; returns where its
    /// control goes.
    pub fn row(&mut self, x: f32, y: f32, w: f32, row: &Row, order: Order) -> Rect {
        let place = if order == Order::First { RowPlace::First } else { RowPlace::Next };
        self.f.row(x, y, w, &row.inner(), place)
    }

    pub fn segments(&mut self, r: Rect, items: &[Choice]) {
        let cells: Vec<Segment> = items.iter().map(|c| Segment::new(Act::Ext(c.id), c.label.as_str(), c.selected)).collect();
        self.f.segments(r, &cells);
    }

    /// A checkbox with its label, `CHECK_H` tall.
    pub fn check(&mut self, id: u16, x: f32, y: f32, w: f32, label: &str, mark: Mark) {
        pro::check_box(self.f, Act::Ext(id), x, y, w, label, mark == Mark::On, true);
    }

    pub fn button(&mut self, id: u16, r: Rect, label: &str, rank: Rank) {
        self.f.push_button(Act::Ext(id), r, label, rank == Rank::Primary);
    }

    /// A button that cannot be pressed now.
    pub fn dead_button(&mut self, r: Rect, label: &str) {
        self.f.dead_button(r, label);
    }

    /// A text link; returns its width.
    pub fn link(&mut self, id: u16, label: &str, x: f32, y: f32) -> f32 {
        text_link(self.f, Act::Ext(id), label, x, y)
    }

    /// A key cap, display only; returns its width.
    pub fn key_cap(&mut self, x: f32, y: f32, label: &str) -> f32 {
        pro::key_cap(self.f, x, y, label)
    }

    /// The PRO tag; returns its width.
    pub fn pro_tag(&mut self, x: f32, y: f32) -> f32 {
        super::pro_tag(self.f, x, y)
    }
}

/// Lets the attached edition draw the Overlay page; false without one.
pub(super) fn page(f: &mut Frame) -> bool {
    crate::edition::with(|e| e.page(&mut Ui { f })).is_some()
}
