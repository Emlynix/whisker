//! Text mode: the same app drawn as a terminal screen, inside the window.
//! Not a console program: it is a look, switched with the `>_ TUI` button
//! in the title bar or F10. Same data, same actions as the pages.
//!
//! ```text
//!                     Overview                                             row 0: page title
//! ┌ Menu ──────────┐┌──────────────────────────────────────────────────┐
//! │ ▸ Overview     ││ BATTERY        POLLING RATE     TODAY            │   18 columns of menu,
//! │ 2 DPI          ││ ▀█ █▀█ %       …                                 │   76 of page boxes
//! ├ PRO ───────────┤└──────────────────────────────────────────────────┘
//! │ 7 Profiles     │┌ Quick DPI ──────────────────────── Edit stages ──┐
//! │                ││  1000   1600  ▸2400                              │
//! ├────────────────┤└──────────────────────────────────────────────────┘
//! │ 9 Settings     │
//! └────────────────┘
//!  ■ connected · 78 %                 1-9 Page  Tab Move  Ent Select  F10 GUI   last row
//! ```
//! Each page shows what its graphical twin shows, decided by the same
//! data (`overview_stats`, `nav_pages`, the mouse's `Caps`).
//!
//! A page first fills a grid of 94 x 31 cells (`Grid`), then the grid is
//! drawn: text in the mono face, box lines and blocks as plain geometry,
//! so columns never drift with a fallback font.

use super::buttons::{action_label, button_name};
use super::{Act, DpiUi, EFFECT_NAMES, Frame, HEIGHT, Hit, Kind, PALETTE, Page, RATES, Stat, TITLE_H, View, WIDTH, nav_pages, overview_stats, status_line, th};
use crate::gfx::{Align, Color, Rect, Seg, Size, Weight};
use crate::i18n::{Lang, T, fill, tr};
use crate::protocol::{BRIGHTNESS_MAX, Direction, Effect, Lod, SPEED_MAX};
use crate::store::{DEBOUNCE_CHOICES, PROFILE_MAX, SLEEP_CHOICES};
use crate::theme::THEMES;

/// The grid fills the window under the title bar: 94 x 8.4 = 790 px of
/// 800, 31 x 18 = 558 px of 568.
const COLS: i32 = 94;
const ROWS: i32 = 31;
/// Cell of the 14 px mono face (`Size::Term`): 0.6 em wide, 18 px line.
const CELL_W: f32 = 8.4;
const CELL_H: f32 = 18.0;
/// Menu column and the page area right of it.
const MENU_W: i32 = 18;
const PAGE_X: i32 = MENU_W;
const PAGE_W: i32 = COLS - MENU_W;
/// Half of the page area: two boxes side by side.
const HALF_W: i32 = PAGE_W / 2;
/// A character of a script without half-width glyphs takes two cells.
const WIDE_FROM: u32 = 0x2E80;

/// How a cell is painted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Style {
    Text,
    Dim,
    Faint,
    /// Bold text.
    Head,
    Accent,
    AccentBold,
    Warn,
    Border,
    /// Border of the page's first box.
    Focus,
    /// Title of that box.
    Title,
    /// Accent ground: tags, the selected row.
    Tag,
    Selected,
    /// The last row: the connection, then the keys and their labels.
    Bar,
    BarOk,
    FnKey,
    FnLabel,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    style: Style,
    act: Option<Act>,
    /// A color of its own (swatches), over the style's.
    color: Option<u32>,
}

const BLANK: Cell = Cell { ch: ' ', style: Style::Text, act: None, color: None };
/// Second cell of a wide character: drawn by the first.
const CONTINUED: char = '\0';

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lead {
    Focused,
    Plain,
}

struct Grid {
    cells: Vec<Cell>,
}

impl Grid {
    fn new() -> Grid {
        Grid { cells: vec![BLANK; (COLS * ROWS) as usize] }
    }

    fn set(&mut self, x: i32, y: i32, cell: Cell) {
        if (0..COLS).contains(&x) && (0..ROWS).contains(&y) {
            self.cells[(y * COLS + x) as usize] = cell;
        }
    }

    /// Writes `s` from (`x`, `y`); returns the column after it.
    fn paint(&mut self, x: i32, y: i32, s: &str, style: Style, act: Option<Act>, color: Option<u32>) -> i32 {
        let mut at = x;
        for ch in s.chars() {
            self.set(at, y, Cell { ch, style, act, color });
            at += 1;
            if ch as u32 >= WIDE_FROM {
                self.set(at, y, Cell { ch: CONTINUED, style, act, color });
                at += 1;
            }
        }
        at
    }

    fn put(&mut self, x: i32, y: i32, s: &str, style: Style) -> i32 {
        self.paint(x, y, s, style, None, None)
    }

    /// Text that does something when pressed.
    fn link(&mut self, x: i32, y: i32, s: &str, style: Style, act: Act) -> i32 {
        self.paint(x, y, s, style, Some(act), None)
    }

    /// Text cut to `w` cells, ending in "…" when it does not fit.
    fn put_in(&mut self, x: i32, y: i32, w: i32, s: &str, style: Style) -> i32 {
        self.put(x, y, &fit(s, w), style)
    }

    /// A box with its title cut into the top border.
    fn frame(&mut self, x: i32, y: i32, w: i32, h: i32, title: &str, lead: Lead, tag: Option<&str>) {
        let style = if lead == Lead::Focused { Style::Focus } else { Style::Border };
        let bar = "─".repeat((w - 2).max(0) as usize);
        self.put(x, y, &format!("┌{bar}┐"), style);
        for row in 1..h - 1 {
            self.put(x, y + row, "│", style);
            self.put(x + w - 1, y + row, "│", style);
        }
        self.put(x, y + h - 1, &format!("└{bar}┘"), style);
        if title.is_empty() {
            return;
        }
        let title_style = if lead == Lead::Focused { Style::Title } else { Style::Head };
        let end = self.put(x + 2, y, &format!(" {} ", fit(title, w - 12)), title_style);
        if let Some(tag) = tag {
            self.put(end, y, &format!(" {tag} "), Style::Tag);
        }
    }

    /// Digits three rows tall, built from half blocks; returns the column
    /// after the number.
    fn big(&mut self, x: i32, y: i32, number: &str, style: Style) -> i32 {
        const FONT: [(char, [&str; 3]); 11] = [
            ('0', ["█▀█", "█ █", "▀▀▀"]),
            ('1', ["▀█ ", " █ ", "▀▀▀"]),
            ('2', ["▀▀█", "█▀▀", "▀▀▀"]),
            ('3', ["▀▀█", " ▀█", "▀▀▀"]),
            ('4', ["█ █", "▀▀█", "  ▀"]),
            ('5', ["█▀▀", "▀▀█", "▀▀▀"]),
            ('6', ["█▀▀", "█▀█", "▀▀▀"]),
            ('7', ["▀▀█", "  █", "  ▀"]),
            ('8', ["█▀█", "█▀█", "▀▀▀"]),
            ('9', ["█▀█", "▀▀█", "▀▀▀"]),
            ('-', ["   ", "▀▀▀", "   "]),
        ];
        let mut at = x;
        for ch in number.chars() {
            // Anything but a digit is a narrow gap.
            let Some((_, rows)) = FONT.iter().find(|(c, _)| *c == ch) else {
                at += 2;
                continue;
            };
            for (row, line) in rows.iter().enumerate() {
                self.put(at, y + row as i32, line, style);
            }
            at += 4;
        }
        at
    }

    /// A bar of `w` cells, `frac` of it filled.
    fn gauge(&mut self, x: i32, y: i32, w: i32, frac: f32, style: Style) {
        let filled = ((frac * w as f32).round() as i32).clamp(0, w);
        self.put(x, y, &"█".repeat(filled as usize), style);
        self.put(x + filled, y, &"░".repeat((w - filled) as usize), Style::Faint);
    }

    fn radio(&mut self, x: i32, y: i32, label: &str, on: bool, act: Act) -> i32 {
        let mark = if on { "(•) " } else { "( ) " };
        self.link(x, y, &format!("{mark}{label}"), if on { Style::AccentBold } else { Style::Text }, act)
    }

    /// `None` = shown, but nothing to press now.
    fn check(&mut self, x: i32, y: i32, label: &str, on: bool, act: Option<Act>) -> i32 {
        let mark = if on { "[x] " } else { "[ ] " };
        let style = if on && act.is_some() { Style::Text } else { Style::Dim };
        self.paint(x, y, &format!("{mark}{label}"), style, act, None)
    }

    /// A row of the accent ground, for the selected item of a list.
    fn selected_row(&mut self, x: i32, y: i32, w: i32, act: Option<Act>) {
        self.paint(x, y, &" ".repeat(w.max(0) as usize), Style::Selected, act, None);
    }

    /// Text wrapped at spaces into lines of `w` cells; returns the rows used.
    fn wrap(&mut self, x: i32, y: i32, w: i32, text: &str, style: Style) -> i32 {
        let mut row = 0;
        let mut line = String::new();
        for word in text.split_whitespace() {
            if !line.is_empty() && cells(&line) + 1 + cells(word) > w {
                self.put(x, y + row, &line, style);
                row += 1;
                line.clear();
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        if !line.is_empty() {
            self.put_in(x, y + row, w, &line, style);
            row += 1;
        }
        row
    }

    /// One of a few values in a line: `[6]` selected, ` 7 ` not.
    fn choice(&mut self, x: i32, y: i32, label: &str, on: bool, act: Act) -> i32 {
        let text = if on { format!("[{label}]") } else { format!(" {label} ") };
        self.link(x, y, &text, if on { Style::Selected } else { Style::Text }, act) + 1
    }
}

/// Cells a text takes.
fn cells(s: &str) -> i32 {
    s.chars().map(|c| if c as u32 >= WIDE_FROM { 2 } else { 1 }).sum()
}

/// `s` cut to `w` cells, with "…" when something was cut.
fn fit(s: &str, w: i32) -> String {
    if cells(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let cw = if ch as u32 >= WIDE_FROM { 2 } else { 1 };
        if used + cw > w - 1 {
            break;
        }
        out.push(ch);
        used += cw;
    }
    out.push('…');
    out
}

/// "1 853" -> "1853": the big font has digits only.
fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

// ---- the screen -------------------------------------------------------------

/// The page of the text mode a GUI page belongs to (its tabs have none
/// of their own).
fn text_page(p: Page) -> Page {
    p.nav()
}

fn build(v: &View) -> Grid {
    let mut g = Grid::new();
    let page = text_page(v.page);
    g.put(PAGE_X + 1, 0, tr(page.title()), Style::Head);
    menu(&mut g, v);
    status(&mut g, v);
    match page {
        Page::Overview => overview(&mut g, v),
        Page::Dpi => dpi(&mut g, v),
        Page::Polling => performance(&mut g, v),
        Page::Lighting => lighting(&mut g, v),
        Page::Buttons => buttons(&mut g, v),
        Page::Stats => statistics(&mut g, v),
        Page::Profiles => profiles(&mut g, v),
        Page::Overlay => overlay(&mut g),
        _ => settings(&mut g, v),
    }
    g
}

/// Pages in menu order with their numbers: the mouse's own pages, the
/// one of the PRO version, Settings.
fn menu_pages(v: &View) -> Vec<(usize, Page)> {
    let mut pages = nav_pages(&v.caps);
    pages.extend([Page::Overlay, Page::General]);
    pages.into_iter().enumerate().map(|(i, p)| (i + 1, p)).collect()
}

/// Page of a number key in the text mode.
pub fn page_of_key(v: &View, number: usize) -> Option<Page> {
    menu_pages(v).into_iter().find(|(n, _)| *n == number).map(|(_, p)| p)
}

/// The sidebar: the mouse's pages, the PRO group, Settings pinned at the
/// bottom over a line.
fn menu(g: &mut Grid, v: &View) {
    const BOTTOM: i32 = ROWS - 2;
    g.frame(0, 1, MENU_W, BOTTOM, "Menu", Lead::Plain, None);
    let rule = format!("├{}┤", "─".repeat((MENU_W - 2) as usize));
    let current = text_page(v.page);
    let mut y = 3;
    for (number, page) in menu_pages(v) {
        if page == Page::Overlay {
            g.put(0, y, &rule, Style::Border);
            g.put(2, y, " PRO ", Style::Tag);
            y += 1;
        }
        if page == Page::General {
            g.put(0, BOTTOM - 2, &rule, Style::Border);
            y = BOTTOM - 1;
        }
        let label = fit(tr(page.title()), MENU_W - 6);
        let act = Act::Nav(page);
        if page == current {
            g.selected_row(1, y, MENU_W - 2, Some(act));
            g.link(1, y, &format!(" ▸ {label}"), Style::Selected, act);
        } else {
            // The whole row answers a press, not only its letters.
            g.paint(1, y, &" ".repeat((MENU_W - 2) as usize), Style::Text, Some(act), None);
            g.link(1, y, &format!(" {number}"), Style::Accent, act);
            g.link(3, y, &format!(" {label}"), Style::Text, act);
        }
        y += 1;
    }
}

/// The last row, as the status bar: connection and battery on the left
/// (a press opens Settings › Mouse), the keys on the right.
fn status(g: &mut Grid, v: &View) {
    const KEYS: [(&str, &str); 4] = [("1-9", "Page"), ("Tab", "Move"), ("Ent", "Select"), ("F10", "GUI")];
    let y = ROWS - 1;
    g.put(0, y, &" ".repeat(COLS as usize), Style::Bar);

    let keys_w: i32 = KEYS.iter().map(|(key, label)| cells(key) + cells(label) + 3).sum();
    let keys_x = COLS - keys_w;
    let mut x = keys_x;
    for (key, label) in KEYS {
        x = g.put(x, y, &format!(" {key} "), Style::FnKey);
        let act = (key == "F10").then_some(Act::ToggleTui);
        x = g.paint(x, y, &format!("{label} "), Style::FnLabel, act, None);
    }

    let act = Act::Nav(Page::Mouse);
    let (text, _) = status_line(v);
    g.link(0, y, " ■ ", if v.connected { Style::BarOk } else { Style::Bar }, act);
    g.link(3, y, &fit(&text, keys_x - 5), Style::Bar, act);
}

/// Overview: the numbers of the graphical Overview, the quick DPI switch
/// for a mouse with stages, the profiles for one with button settings.
fn overview(g: &mut Grid, v: &View) {
    const STRIP_H: i32 = 9;
    const PANEL_H: i32 = 5;
    let r = PAGE_X;
    let stats = overview_stats(v);
    g.frame(r, 1, PAGE_W, STRIP_H, "", Lead::Focused, None);
    let step = (PAGE_W - 2) / stats.len().max(1) as i32;
    for (k, stat) in stats.iter().enumerate() {
        stat_cell(g, r + 2 + k as i32 * step, 2, step - 2, stat);
    }
    let mut y = 1 + STRIP_H;

    if v.caps.dpi == DpiUi::Stages {
        g.frame(r, y, PAGE_W, PANEL_H, tr(T::QuickDpi), Lead::Plain, None);
        let edit = format!(" {} ", tr(T::EditStages));
        g.link(r + PAGE_W - 3 - cells(&edit), y, &edit, Style::Accent, Act::Nav(Page::Dpi));
        let mut x = r + 2;
        for (i, s) in v.stages.iter().enumerate().filter(|(_, s)| s.enabled) {
            let on = i == v.active;
            let text = if on { format!(" ▸{} ", s.dpi) } else { format!("  {} ", s.dpi) };
            x = g.link(x, y + 2, &text, if on { Style::Selected } else { Style::Text }, Act::Activate(i)) + 1;
        }
        y += PANEL_H;
    }

    if !v.caps.buttons {
        return;
    }
    // One radio per profile, on as many rows as the panel has.
    const NAME_W: i32 = 12;
    const RADIO_W: i32 = 4;
    const GAP: i32 = 3;
    g.frame(r, y, PAGE_W, PANEL_H, tr(T::Profile), Lead::Plain, None);
    let manage = format!(" {} ", tr(T::ProfileManage));
    g.link(r + PAGE_W - 3 - cells(&manage), y, &manage, Style::Accent, Act::Nav(Page::Profiles));
    let (mut x, mut row) = (r + 2, y + 1);
    for (i, p) in v.profiles.iter().enumerate() {
        let name = fit(&p.name, NAME_W);
        if x + RADIO_W + cells(&name) > r + PAGE_W - 2 {
            (x, row) = (r + 2, row + 1);
        }
        x = g.radio(x, row, &name, i == v.profile, Act::Profile(i)) + GAP;
    }
}

/// One number of the Overview in `w` cells: caption, the value in big
/// digits with its unit, a bar, a note.
///
/// ```text
///  BATTERY
///  ▀▀█ █▀█
///   ▀█ █ █ %
///  ▀▀▀ ▀▀▀
///  ████████░░░░░
///  Lasts about
///  12 more days
/// ```
fn stat_cell(g: &mut Grid, x: i32, y: i32, w: i32, stat: &Stat) {
    const APPROX: char = '≈';
    const DIGIT_W: i32 = 4;
    const BAR_MAX: i32 = 16;
    g.put_in(x, y, w, &stat.label.to_uppercase(), Style::Dim);

    // Big digits for a plain number that fits; anything else as text.
    let number = stat.value.trim_start_matches(APPROX).trim();
    let plain = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit() || c.is_whitespace());
    let number = digits(number);
    let approx = stat.value.starts_with(APPROX);
    // The last digit has no gap after it unless a unit follows.
    let unit_w = if stat.unit.is_empty() { -1 } else { cells(stat.unit) };
    let big_w = if approx { 2 } else { 0 } + number.len() as i32 * DIGIT_W + unit_w;
    if plain && big_w <= w {
        if approx {
            g.put(x, y + 2, &APPROX.to_string(), Style::Dim);
        }
        let end = g.big(x + if approx { 2 } else { 0 }, y + 1, &number, Style::Head);
        g.put(end, y + 3, stat.unit, Style::Dim);
    } else {
        let end = g.put_in(x, y + 2, w, &stat.value, Style::Head);
        g.put(end + 1, y + 2, stat.unit, Style::Dim);
    }

    if let Some((pct, color)) = stat.bar {
        let bar_w = (w - 2).min(BAR_MAX);
        let filled = (pct.min(100) as i32 * bar_w + 50) / 100;
        g.paint(x, y + 4, &"█".repeat(filled as usize), Style::Text, None, Some(color.0));
        g.put(x + filled, y + 4, &"░".repeat((bar_w - filled) as usize), Style::Faint);
    }
    g.wrap(x, y + 5, w, &stat.note, Style::Dim);
}

/// A chart of `rows` text rows: one column per cell, built from the
/// eighth-blocks, with a 100 / 50 / 0 axis on the left.
fn chart(g: &mut Grid, x: i32, y: i32, w: i32, rows: i32, points: &[(f32, f32)], empty: &str) {
    const AXIS_W: i32 = 5;
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if points.len() < 2 {
        g.put_in(x + 2, y + rows / 2, w - 4, empty, Style::Faint);
        return;
    }
    for (row, label) in [(0, "100"), (rows / 2, " 50"), (rows - 1, "  0")] {
        g.put(x, y + row, label, Style::Faint);
    }
    for row in 0..rows {
        g.put(x + AXIS_W - 2, y + row, "┤", Style::Faint);
    }

    let width = w - AXIS_W;
    for col in 0..width {
        // The sample nearest to this column's time.
        let t = col as f32 / (width - 1).max(1) as f32;
        let nearest = points.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()));
        let Some(&(_, pct)) = nearest else {
            continue;
        };
        // Eighths of a row, from the bottom.
        let height = (pct.clamp(0.0, 100.0) / 100.0 * (rows * 8) as f32).round() as i32;
        for row in 0..rows {
            let level = height - (rows - 1 - row) * 8;
            if level <= 0 {
                continue;
            }
            let ch = LEVELS[(level.min(8) - 1) as usize];
            g.put(x + AXIS_W + col, y + row, &ch.to_string(), Style::Accent);
        }
    }
}

fn dpi(g: &mut Grid, v: &View) {
    const LEVEL_W: i32 = 44;
    const TRACK_W: i32 = 58;
    let r = PAGE_X;
    if v.caps.dpi == DpiUi::Single {
        // One value, set with the pointer in the graphical mode.
        g.frame(r, 1, PAGE_W, 8, tr(T::ActiveDpi), Lead::Focused, None);
        let dpi = v.basic.dpi.map(|d| d.to_string()).unwrap_or_default();
        let end = g.big(r + 3, 3, &dpi, Style::Head);
        g.put(end, 5, "DPI", Style::Dim);
        return;
    }
    let top = crate::protocol::dpi_of(v.steps.saturating_sub(1)).max(1);

    g.frame(r, 1, PAGE_W, 10, tr(T::Stages), Lead::Focused, None);
    let cycle = fit(&tr(T::ColCycle).to_uppercase(), 14);
    g.put(r + PAGE_W - 2 - cells(&cycle), 2, &cycle, Style::Dim);
    for (i, s) in v.stages.iter().enumerate() {
        let y = 3 + i as i32;
        let on = i == v.active;
        let act = if s.enabled { Act::Activate(i) } else { Act::Toggle(i) };
        if on {
            g.selected_row(r + 1, y, PAGE_W - 2, None);
        }
        let style = if on { Style::Selected } else if s.enabled { Style::Text } else { Style::Dim };
        let name = fit(&fill(tr(T::StageN), &[&(i + 1).to_string()]).to_lowercase(), 10);
        g.link(r + 2, y, &format!("{} {name}", if on { "▸" } else { " " }), style, act);
        g.link(r + 15, y, &format!("{:>5}", s.dpi), if on { Style::Selected } else { Style::Head }, act);
        let filled = (s.dpi as f32 / top as f32 * LEVEL_W as f32).round() as i32;
        let bar = if on { Style::Selected } else if s.enabled { Style::Accent } else { Style::Faint };
        g.put(r + 23, y, &"█".repeat(filled.clamp(0, LEVEL_W) as usize), bar);
        g.put(r + 23 + filled, y, &"░".repeat((LEVEL_W - filled).clamp(0, LEVEL_W) as usize), if on { Style::Selected } else { Style::Faint });
        // The active stage always stays in the cycle.
        let mark = if s.enabled { "[x]" } else { "[ ]" };
        g.paint(r + 70, y, mark, style, (!on).then_some(Act::Toggle(i)), None);
    }

    let Some(s) = v.stages.get(v.active) else {
        return;
    };
    g.frame(r, 11, PAGE_W, 10, &fill(tr(T::StageN), &[&(v.active + 1).to_string()]), Lead::Plain, None);
    g.link(r + 3, 14, "[ - ]", Style::Accent, Act::StageNudge(-1));
    let end = g.big(r + 28, 13, &s.dpi.to_string(), Style::Head);
    g.put(end, 15, "DPI", Style::Dim);
    g.link(r + 67, 14, "[ + ]", Style::Accent, Act::StageNudge(1));

    // The slider as a track: drag or click along it.
    let lowest = crate::protocol::dpi_of(0);
    g.put(r + 3, 17, &format!("{lowest:>4} ├"), Style::Dim);
    let pos = (s.index as f32 / v.steps.saturating_sub(1).max(1) as f32 * (TRACK_W - 1) as f32).round() as i32;
    for col in 0..TRACK_W {
        let (ch, style) = match col.cmp(&pos) {
            std::cmp::Ordering::Less => ("━", Style::Accent),
            std::cmp::Ordering::Equal => ("■", Style::Accent),
            std::cmp::Ordering::Greater => ("─", Style::Faint),
        };
        g.link(r + 9 + col, 17, ch, style, Act::Slider(v.active));
    }
    g.put(r + 9 + TRACK_W, 17, &format!("┤ {top}"), Style::Dim);

    // What the DPI button steps through.
    g.frame(r, 21, PAGE_W, 4, tr(T::ColCycle), Lead::Plain, None);
    let mut x = r + 2;
    for (i, s) in v.stages.iter().enumerate().filter(|(_, s)| s.enabled) {
        let on = i == v.active;
        let text = if on { format!("[{}]", s.dpi) } else { s.dpi.to_string() };
        x = g.link(x, 22, &text, if on { Style::AccentBold } else { Style::Text }, Act::Activate(i));
        x = g.put(x, 22, " → ", Style::Faint);
    }

    g.frame(r, 25, PAGE_W, 5, "", Lead::Plain, None);
    g.wrap(r + 2, 26, PAGE_W - 4, tr(T::ActiveLocked), Style::Faint);
}

fn performance(g: &mut Grid, v: &View) {
    let r = PAGE_X;
    g.frame(r, 1, PAGE_W, 9, tr(T::PollingRate), Lead::Focused, None);
    let rates: Vec<(u32, Act, bool)> = if v.caps.extras {
        RATES.iter().enumerate().map(|(i, &hz)| (hz, Act::Polling(i), v.polling == i)).collect()
    } else {
        v.basic.rates.iter().map(|&hz| (hz, Act::BasicRate(hz), v.basic.rate == Some(hz))).collect()
    };
    let fastest = rates.iter().map(|r| r.0).max().unwrap_or(1).max(1);
    for (row, (hz, act, on)) in rates.iter().take(5).enumerate() {
        let y = 3 + row as i32;
        g.radio(r + 2, y, &format!("{hz} Hz"), *on, *act);
        let ms = format!("{:.1} ms", 1000.0 / (*hz).max(1) as f32).replace(".0 ", " ");
        g.put(r + 17, y, &ms, Style::Dim);
        g.gauge(r + 25, y, 16, *hz as f32 / fastest as f32, if *on { Style::Accent } else { Style::Faint });
    }
    // What Raw Input measures while the mouse moves.
    g.put_in(r + 46, 2, 28, &fill(tr(T::MeasuredValue), &[""]).trim().trim_end_matches(':').to_uppercase(), Style::Dim);
    match v.measured {
        Some(hz) => {
            let end = g.big(r + 46, 3, &hz.to_string(), Style::Head);
            g.put(end, 5, "Hz", Style::Dim);
        }
        None => {
            g.wrap(r + 46, 4, 28, tr(T::MoveMouse), Style::Faint);
        }
    }
    if !v.caps.extras {
        g.frame(r, 10, PAGE_W, 6, "", Lead::Plain, None);
        g.wrap(r + 2, 11, PAGE_W - 4, tr(T::PollingHelp), Style::Dim);
        return;
    }

    g.frame(r, 10, HALF_W, 10, tr(T::Debounce), Lead::Plain, None);
    for (i, ms) in DEBOUNCE_CHOICES.iter().enumerate() {
        g.radio(r + 2, 11 + i as i32, &format!("{ms} ms"), v.debounce == *ms, Act::Debounce(i));
    }

    g.frame(r + HALF_W, 10, HALF_W, 5, tr(T::LiftOff), Lead::Plain, None);
    for (k, lod) in [Lod::Mm1, Lod::Mm2].into_iter().enumerate() {
        g.radio(r + HALF_W + 2 + k as i32 * 12, 12, &format!("{} mm", lod as u8), v.lod == lod, Act::Lod(lod));
    }

    let minutes = fill(tr(T::Minutes), &[""]);
    g.frame(r + HALF_W, 15, HALF_W, 5, &format!("{} ({})", tr(T::SleepAfter), minutes.trim()), Lead::Plain, None);
    let mut x = r + HALF_W + 2;
    for (i, m) in SLEEP_CHOICES.iter().enumerate() {
        x = g.choice(x, 17, &m.to_string(), v.power.sleep_min == *m, Act::Sleep(i));
    }

    g.frame(r, 20, PAGE_W, 10, "", Lead::Plain, None);
    let used = g.wrap(r + 2, 22, PAGE_W - 4, tr(T::PollingHelp), Style::Dim);
    g.wrap(r + 2, 23 + used, PAGE_W - 4, tr(T::DebounceHelp), Style::Dim);
}

fn lighting(g: &mut Grid, v: &View) {
    let r = PAGE_X;
    let l = &v.lighting;
    g.frame(r, 1, HALF_W, 11, tr(T::EffectCaption), Lead::Focused, None);
    for (i, e) in Effect::ALL.iter().enumerate() {
        g.radio(r + 2, 3 + i as i32, &fit(tr(EFFECT_NAMES[i]), HALF_W - 8), l.effect == *e, Act::Effect(i));
    }

    // Colors as real colored blocks; the chosen ones in brackets.
    let multi = l.effect == Effect::Breathing;
    let has_colors = matches!(l.effect, Effect::Steady | Effect::Breathing);
    g.frame(r + HALF_W, 1, HALF_W, 11, tr(if multi { T::ColorsCaption } else { T::ColorCaption }), Lead::Plain, None);
    if has_colors {
        for (i, &color) in PALETTE.iter().enumerate() {
            let (x, y) = (r + HALF_W + 3 + (i as i32 % 4) * 8, 3 + (i as i32 / 4) * 2);
            let (act, on) = if multi { (Act::Breath(i), l.breathing.contains(&color)) } else { (Act::Steady(i), l.steady == color) };
            g.put(x, y, if on { "[" } else { " " }, Style::Accent);
            g.paint(x + 1, y, "████", Style::Text, Some(act), Some(color));
            g.put(x + 5, y, if on { "]" } else { " " }, Style::Accent);
        }
    }

    g.frame(r, 12, PAGE_W, 5, tr(T::Brightness), Lead::Plain, None);
    let mut x = r + 2;
    for b in 1..=BRIGHTNESS_MAX {
        x = g.choice(x, 14, &b.to_string(), l.brightness == b, Act::Brightness(b));
    }
    g.gauge(r + 30, 14, 40, l.brightness as f32 / BRIGHTNESS_MAX as f32, Style::Accent);

    g.frame(r, 17, PAGE_W, 5, tr(T::Speed), Lead::Plain, None);
    if l.effect.has_speed() {
        let mut x = r + 2;
        for sp in 1..=SPEED_MAX {
            x = g.choice(x, 19, &sp.to_string(), l.speed == sp, Act::Speed(sp));
        }
        g.gauge(r + 30, 19, 40, l.speed as f32 / SPEED_MAX as f32, Style::Accent);
    }
    if l.effect == Effect::Streaming {
        let x = g.radio(r + 2, 20, &format!("→ {}", tr(T::DirForward)), l.direction == Direction::Forward, Act::Direction(Direction::Forward));
        g.radio(x + 3, 20, &format!("← {}", tr(T::DirReverse)), l.direction == Direction::Reverse, Act::Direction(Direction::Reverse));
    }

    g.frame(r, 22, PAGE_W, 8, "", Lead::Plain, None);
    g.check(r + 2, 24, &fit(tr(T::DarkMoving), PAGE_W - 10), v.power.dark_while_moving, Some(Act::DarkMoving));
    g.wrap(r + 2, 26, PAGE_W - 4, tr(T::LightingNote), Style::Faint);
}

fn buttons(g: &mut Grid, v: &View) {
    const MAP_W: i32 = 30;
    const ART: [&str; 15] = [
        "   ┌────────┬────────┐ ",
        "  ┌┘        │        └┐",
        "  │         │         │",
        "  │         │         │",
        "  │         │         │",
        "  │         │         │",
        "  ├─────────┴─────────┤",
        "  │                   │",
        "  │                   │",
        "  │                   │",
        "  │                   │",
        "  │                   │",
        "  │                   │",
        "  └┐                 ┌┘",
        "   └─────────────────┘ ",
    ];
    let r = PAGE_X;
    // The button the pointer or the keyboard is on.
    let lit = v.focus.and_then(|f| match f.act {
        Act::EditButton(i) => Some(i),
        _ => None,
    });

    g.frame(r, 1, MAP_W, 20, tr(T::PageButtons), Lead::Focused, None);
    let (mx, my) = (r + 3, 3);
    for (row, line) in ART.iter().enumerate() {
        g.put(mx, my + row as i32, line, Style::Border);
    }
    // The two main buttons shade when lit.
    for (button, from, to) in [(0, 3, 11), (1, 13, 21)] {
        if lit != Some(button) {
            continue;
        }
        g.put(mx + if from == 3 { 4 } else { 13 }, my + 1, &"▒".repeat(8), Style::Accent);
        for row in 2..=5 {
            g.put(mx + from, my + row, &"▒".repeat((to - from + 1) as usize), Style::Accent);
        }
    }
    let part = |button: usize| if lit == Some(button) { Style::AccentBold } else { Style::Head };
    // Wheel, DPI button, the two side buttons: forward above back.
    g.put(mx + 12, my + 2, "┃", part(2));
    g.put(mx + 12, my + 3, "┃", part(2));
    g.put(mx + 12, my + 5, "■", part(5));
    for row in [8, 9] {
        g.put(mx + 1, my + row, "▐", part(4));
    }
    for row in [11, 12] {
        g.put(mx + 1, my + row, "▐", part(3));
    }
    let mut label = |x: i32, y: i32, button: usize| {
        let style = if lit == Some(button) { Style::Tag } else { Style::Head };
        g.link(x, y, &format!(" {} ", button + 1), style, Act::EditButton(button));
    };
    label(mx + 5, my + 3, 0);
    label(mx + 15, my + 3, 1);
    label(mx + 23, my + 2, 2);
    label(mx + 23, my + 5, 5);
    label(mx - 2, my + 8, 4);
    label(mx - 2, my + 11, 3);

    // What each button does; a press opens the picker.
    g.frame(r + MAP_W, 1, PAGE_W - MAP_W, 20, "", Lead::Plain, None);
    for (i, a) in v.buttons.iter().enumerate() {
        let y = 3 + i as i32 * 2;
        let on = lit == Some(i);
        let act = Act::EditButton(i);
        if on {
            g.selected_row(r + MAP_W + 1, y, PAGE_W - MAP_W - 2, Some(act));
        }
        let style = if on { Style::Selected } else { Style::Text };
        g.link(r + MAP_W + 2, y, &(i + 1).to_string(), if on { Style::Selected } else { Style::Accent }, act);
        g.link(r + MAP_W + 5, y, &fit(button_name(i), 19), style, act);
        g.link(r + MAP_W + 26, y, &format!("{} ▾", fit(&action_label(*a), 16)), if on { Style::Selected } else { Style::Head }, act);
    }

    g.frame(r, 21, PAGE_W, 9, "", Lead::Plain, None);
    let used = g.wrap(r + 2, 23, PAGE_W - 4, tr(T::ButtonsNote), Style::Dim);
    g.link(r + 2, 24 + used, &format!("[ {} ]", tr(T::PageMacros)), Style::Accent, Act::Nav(Page::Macros));
    if v.buttons != crate::protocol::DEFAULT_ACTIONS {
        g.link(r + 22, 24 + used, &format!("[ {} ]", fit(tr(T::ResetAll), 30)), Style::Accent, Act::ResetButtons);
    }
    if !v.notice.is_empty() {
        g.put_in(r + 2, 26 + used, PAGE_W - 4, &v.notice, Style::Warn);
    }
}

fn statistics(g: &mut Grid, v: &View) {
    const PERIODS: [T; 3] = [T::PeriodToday, T::PeriodWeek, T::PeriodAll];
    let r = PAGE_X;
    let tile = |i: usize| v.tiles.get(i).map(|(label, value)| (*label, value.as_str())).unwrap_or(("", ""));

    // Total clicks and distance of the chosen period, as big numbers.
    let (label, clicks) = tile(0);
    g.frame(r, 1, HALF_W, 7, label, Lead::Focused, None);
    g.big(r + 2, 3, &digits(clicks), Style::Head);
    let (label, distance) = tile(5);
    g.frame(r + HALF_W, 1, HALF_W, 7, label, Lead::Plain, None);
    let (number, unit) = distance.rsplit_once(' ').unwrap_or((distance, ""));
    let end = g.big(r + HALF_W + 2, 3, &digits(number), Style::Head);
    g.put(end, 5, unit, Style::Dim);

    // The period, in the border between the boxes and the chart.
    let mut x = r + 2;
    for (i, name) in PERIODS.into_iter().enumerate() {
        x = g.choice(x, 8, tr(name), v.period == i, Act::Period(i)) + 1;
    }

    let title = [T::ChartToday, T::ChartTitle, T::ChartAll][v.period.min(2)];
    g.frame(r, 9, PAGE_W, 13, tr(title), Lead::Plain, None);
    chart(g, r + 1, 10, PAGE_W - 8, 10, &v.chart, tr(T::ChartEmpty));

    // Left, right, middle + side, wheel: each against the total.
    g.frame(r, 22, PAGE_W, 8, "", Lead::Plain, None);
    let total = digits(clicks).parse::<f32>().unwrap_or(0.0).max(1.0);
    for (row, i) in [1, 2, 3, 4].into_iter().enumerate() {
        let (label, value) = tile(i);
        let y = 24 + row as i32;
        g.put_in(r + 2, y, 18, label, Style::Dim);
        // The wheel counts notches, not clicks: no share of the total.
        if i != 4 {
            g.gauge(r + 21, y, 40, digits(value).parse::<f32>().unwrap_or(0.0) / total, Style::Accent);
        }
        g.put(r + 64, y, &format!("{value:>10}"), Style::Text);
    }
}

/// Profiles: the list, its buttons, and the PRO version's switching by app.
fn profiles(g: &mut Grid, v: &View) {
    const NAME_X: i32 = 5;
    const NAME_W: i32 = 24;
    const BUTTONS_X: i32 = 32;
    const MACROS_X: i32 = 46;
    let r = PAGE_X;
    let count = v.profiles.len() as i32;
    let count_text = |n: usize| if n == 0 { "—".to_string() } else { n.to_string() };

    // Border, header, rows, a gap, two rows of buttons, border.
    let list_h = count + 6;
    g.frame(r, 1, PAGE_W, list_h, tr(T::PageProfiles), Lead::Focused, None);
    g.put_in(r + NAME_X, 2, NAME_W, &tr(T::ColName).to_uppercase(), Style::Dim);
    g.put_in(r + BUTTONS_X, 2, MACROS_X - BUTTONS_X - 1, &tr(T::PageButtons).to_uppercase(), Style::Dim);
    g.put_in(r + MACROS_X, 2, 12, &tr(T::PageMacros).to_uppercase(), Style::Dim);
    for (i, p) in v.profiles.iter().enumerate() {
        let y = 3 + i as i32;
        let act = Act::Profile(i);
        let active = i == v.profile;
        let style = if active { Style::Selected } else { Style::Text };
        if active {
            g.selected_row(r + 1, y, PAGE_W - 2, Some(act));
        } else {
            // The whole row answers a press, not only its letters.
            g.paint(r + 1, y, &" ".repeat((PAGE_W - 2) as usize), Style::Text, Some(act), None);
        }
        g.link(r + 2, y, if active { "▸" } else { " " }, style, act);
        // The active row while it is renamed: what was typed, and how to end.
        if let Some(typed) = v.rename.as_deref().filter(|_| active) {
            let shown = if typed.is_empty() { format!("▏{}", fit(&p.name, NAME_W - 1)) } else { format!("{}▏", fit(typed, NAME_W - 1)) };
            g.link(r + NAME_X, y, &shown, style, act);
            g.link(r + BUTTONS_X, y, &fit(tr(T::RenameHint), PAGE_W - BUTTONS_X - 2), style, act);
            continue;
        }
        g.link(r + NAME_X, y, &fit(&p.name, NAME_W), style, act);
        g.link(r + BUTTONS_X, y, &count_text(p.remapped), style, act);
        g.link(r + MACROS_X, y, &count_text(p.macros), style, act);
        if active {
            let tag = fit(tr(T::ProfileActive), 14);
            g.link(r + PAGE_W - 2 - cells(&tag), y, &tag, style, act);
        }
    }
    let room = v.profiles.len() < PROFILE_MAX;
    let buttons = [
        (T::ProfileNew, room.then_some(Act::ProfileNew)),
        (T::ProfileDuplicate, room.then_some(Act::ProfileDuplicate)),
        (T::ProfileRename, Some(Act::ProfileRename)),
        (T::ProfileDelete, (v.profiles.len() > 1).then_some(Act::ProfileDelete)),
    ];
    let mut x = r + 2;
    for (label, act) in buttons {
        let text = format!("[ {} ]", tr(label));
        x = match act {
            Some(act) => g.link(x, list_h - 2, &text, Style::Accent, act),
            None => g.put(x, list_h - 2, &text, Style::Faint),
        } + 2;
    }
    // The file buttons, and whose profiles these are.
    let mut x = r + 2;
    for (label, act) in [(T::ProfileExport, Act::ProfileExport), (T::ProfileExportAll, Act::ProfileExportAll), (T::ProfileImport, Act::ProfileImport)] {
        x = g.link(x, list_h - 1, &format!("[ {} ]", tr(label)), Style::Accent, act) + 2;
    }
    let mouse = fit(&v.device_name, r + PAGE_W - 2 - x);
    g.put(r + PAGE_W - 2 - cells(&mouse), list_h - 1, &mouse, Style::Dim);

    let y = 1 + list_h;
    g.frame(r, y, PAGE_W, 4, "Automatic switching", Lead::Plain, Some("PRO"));
    let x = g.check(r + 2, y + 1, "Switch by active app", false, Some(Act::Mock(30)));
    let x = g.put(x + 4, y + 1, "otherwise use", Style::Dim);
    g.radio(x + 2, y + 1, "Default", true, Act::Mock(31));
    g.put_in(r + 2, y + 2, PAGE_W - 4, tr(T::ProOnly), Style::Faint);

    g.wrap(r + 1, y + 5, PAGE_W - 2, tr(T::ProfileNote), Style::Faint);
}

/// The in-game overlay of the PRO version: a mock-up.
fn overlay(g: &mut Grid) {
    const SHOWS: [&str; 6] = ["Clicks per second", "Polling rate", "Battery", "DPI", "Acceleration graph", "Speed graph"];
    const SHOWN: usize = 4;
    let r = PAGE_X;
    g.frame(r, 1, PAGE_W, 8, "Overlay preview", Lead::Focused, Some("PRO"));
    for y in 2..8 {
        let dots: String = (0..PAGE_W - 2).map(|i| if (i + y) % 4 == 0 { '·' } else { ' ' }).collect();
        g.put(r + 1, y, &dots, Style::Faint);
    }
    let sample = "7 CPS · 500 Hz · 78 % · 2400 DPI";
    g.put(r + PAGE_W - 3 - cells(sample), 2, sample, Style::AccentBold);

    g.frame(r, 9, HALF_W, 9, "Overlay", Lead::Plain, None);
    g.check(r + 2, 10, "Show in games", true, Some(Act::Mock(40)));
    for (k, name) in ["Minimal", "Compact", "Full"].into_iter().enumerate() {
        g.radio(r + 2, 12 + k as i32, name, k == 0, Act::Mock(50 + k as u8));
    }
    g.put(r + 2, 16, "toggle ", Style::Dim);
    g.put(r + 9, 16, "Ctrl+Shift+O", Style::Accent);

    g.frame(r + HALF_W, 9, HALF_W, 9, "Position & size", Lead::Plain, None);
    let gx = r + HALF_W + 3;
    for (row, line) in ["┌───┬───┐", "│   │   │", "├───┼───┤", "│   │   │", "└───┴───┘"].into_iter().enumerate() {
        g.put(gx, 11 + row as i32, line, Style::Border);
    }
    for (k, (x, y)) in [(1, 12), (5, 12), (1, 14), (5, 14)].into_iter().enumerate() {
        let on = k == 1;
        g.link(gx + x, y, if on { " ■ " } else { "   " }, if on { Style::AccentBold } else { Style::Text }, Act::Mock(70 + k as u8));
    }
    for (k, pct) in [100, 125, 150, 200].into_iter().enumerate() {
        g.radio(r + HALF_W + 16, 11 + k as i32, &format!("{pct}%"), k == 1, Act::Mock(80 + k as u8));
    }

    // What the overlay shows: two columns, as in the graphical mode.
    g.frame(r, 18, PAGE_W, 7, "Shows", Lead::Plain, None);
    for (k, name) in SHOWS.into_iter().enumerate() {
        let (x, y) = (r + 2 + (k as i32 % 2) * HALF_W, 20 + k as i32 / 2);
        g.put(x, y, "≡", Style::Faint);
        g.check(x + 2, y, name, k < SHOWN, Some(Act::Mock(60 + k as u8)));
    }
    g.put_in(r + 2, 26, PAGE_W - 4, tr(T::ProOnly), Style::Faint);
}

fn settings(g: &mut Grid, v: &View) {
    const LANG_COLS: usize = 4;
    const LANG_STEP: i32 = 18;
    let r = PAGE_X;

    // Themes: "system" first, each with blocks in its own colors.
    g.frame(r, 1, PAGE_W, 6, tr(T::Theme), Lead::Focused, None);
    let themes: Vec<(String, Option<&crate::theme::Theme>)> =
        std::iter::once((tr(T::FollowSystem).to_string(), None)).chain(THEMES.iter().map(|t| (t.name.to_string(), Some(t)))).collect();
    for (i, (name, theme)) in themes.iter().enumerate() {
        let x = r + 2 + i as i32 * 15;
        g.radio(x, 3, &fit(name, 10), v.theme_pick == i, Act::Theme(i));
        let t = theme.unwrap_or(th());
        for (k, c) in [t.bg, t.side, t.text, t.acc_fill].into_iter().enumerate() {
            g.paint(x + 4 + k as i32 * 2, 4, "██", Style::Text, Some(Act::Theme(i)), Some(c.0));
        }
    }

    g.frame(r, 7, HALF_W, 8, tr(T::TabGeneral), Lead::Plain, None);
    g.check(r + 2, 9, &fit(tr(T::Autostart), HALF_W - 8), v.autostart, v.installed.then_some(Act::Autostart));
    g.check(r + 2, 10, &fit(tr(T::UpdateAuto), HALF_W - 8), v.update.auto, Some(Act::UpdateAuto));
    let (state, act, label) = if v.installed { (T::Installed, Act::Uninstall, T::Uninstall) } else { (T::NotInstalled, Act::Install, T::Install) };
    g.put_in(r + 2, 12, HALF_W - 4, tr(state), Style::Dim);
    g.link(r + 2, 13, &format!("[ {} ]", fit(tr(label), HALF_W - 10)), Style::Accent, act);

    // The look itself: graphical or text, and the effects.
    g.frame(r + HALF_W, 7, HALF_W, 8, tr(T::PageAppearance), Lead::Plain, None);
    g.radio(r + HALF_W + 2, 9, "GUI", false, Act::ToggleTui);
    g.radio(r + HALF_W + 12, 9, "TUI", true, Act::ToggleTui);
    g.check(r + HALF_W + 2, 11, &fit(tr(T::FxMotion), HALF_W - 8), v.fx_motion, Some(Act::FxMotion));
    g.check(r + HALF_W + 2, 12, &fit(tr(T::FxTexture), HALF_W - 8), v.fx_texture, Some(Act::FxTexture));

    // Languages by their own names.
    g.frame(r, 15, PAGE_W, 7, tr(T::Language), Lead::Plain, None);
    let names: Vec<&str> = std::iter::once(tr(T::FollowSystem)).chain(Lang::ALL.iter().map(|l| l.native())).collect();
    for (i, name) in names.iter().enumerate() {
        let (x, y) = (r + 2 + (i % LANG_COLS) as i32 * LANG_STEP, 17 + (i / LANG_COLS) as i32);
        g.radio(x, y, &fit(name, LANG_STEP - 6), v.lang_pick == i, Act::Lang(i));
    }

    // The mouse as Settings › Mouse lists it: the status bar leads here.
    const FACTS: usize = 5;
    const CAPTION_W: i32 = 13;
    g.frame(r, 22, HALF_W, 8, tr(T::TabMouse), Lead::Plain, None);
    for (row, (caption, value)) in v.device.rows.iter().take(FACTS).enumerate() {
        let y = 23 + row as i32;
        g.put_in(r + 2, y, CAPTION_W - 1, tr(*caption), Style::Dim);
        let (text, style) = if value.is_empty() { (tr(T::NotReported), Style::Faint) } else { (value.as_str(), Style::Text) };
        g.put_in(r + 2 + CAPTION_W, y, HALF_W - 4 - CAPTION_W, text, style);
    }
    g.link(r + 2, 28, &format!("[ {} ]", fit(tr(T::ChangeMouse), HALF_W - 8)), Style::Accent, Act::ChangeMouse);

    let x = r + HALF_W + 2;
    g.frame(r + HALF_W, 22, HALF_W, 8, tr(T::About), Lead::Plain, None);
    g.put_in(x, 23, HALF_W - 4, &format!("{} {} · GPL-3.0-or-later", crate::APP_NAME, env!("CARGO_PKG_VERSION")), Style::Text);
    for (k, (label, act)) in [(T::UpdateCheck, Act::UpdateCheck), (T::HelpAddMouse, Act::ReportOpen), (T::ShowLog, Act::ShowLog)].into_iter().enumerate() {
        g.link(x, 24 + k as i32, &format!("[ {} ]", fit(tr(label), HALF_W - 8)), Style::Accent, act);
    }
    g.put_in(x, 28, HALF_W - 4, &fill(tr(T::DataDir), &[&v.data_dir]), Style::Faint);
}

// ---- drawing ----------------------------------------------------------------

/// Colors of a style: text, and the ground if it has one.
fn colors(style: Style) -> (Color, Option<Color>, Weight) {
    let t = th();
    match style {
        Style::Text => (t.text, None, Weight::Medium),
        Style::Dim => (t.muted, None, Weight::Medium),
        Style::Faint | Style::Border => (t.line2, None, Weight::Medium),
        Style::Head => (t.text, None, Weight::Semibold),
        Style::Accent => (t.acc, None, Weight::Medium),
        Style::AccentBold | Style::Title => (t.acc, None, Weight::Semibold),
        Style::Warn => (t.warn, None, Weight::Medium),
        Style::Focus => (t.acc_fill, None, Weight::Medium),
        Style::Tag | Style::Selected => (t.acc_on, Some(t.acc_fill), Weight::Semibold),
        Style::Bar | Style::FnLabel => (t.text, Some(t.sel), Weight::Medium),
        Style::BarOk => (t.ok, Some(t.sel), Weight::Medium),
        Style::FnKey => (t.text, None, Weight::Semibold),
    }
}

/// Where the grid sits in the window: centred, its last row on the
/// window's bottom edge like the status bar.
fn origin() -> (f32, f32) {
    ((WIDTH - COLS as f32 * CELL_W) / 2.0, HEIGHT - ROWS as f32 * CELL_H)
}

/// The box of a cell, its edges on whole pixels: a cell is 8.4 px wide,
/// and blocks drawn at fractions would leave seams between them.
fn cell_rect(x: i32, y: i32) -> Rect {
    let (ox, oy) = origin();
    let edge = |col: i32| (ox + col as f32 * CELL_W).round();
    Rect::new(edge(x), oy + y as f32 * CELL_H, edge(x + 1) - edge(x), CELL_H)
}

/// Characters drawn as geometry: box lines, blocks, marks. Whatever a
/// font may lack or draw at another width never goes through the font.
fn shape(f: &mut Frame, ch: char, r: Rect, c: Color) -> bool {
    const THIN: f32 = 1.0;
    const THICK: f32 = 2.0;
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (left, right, top, bottom) = (r.x, r.x + r.w, r.y, r.y + r.h);
    // Arms of a box-drawing character from the middle of its cell.
    let arms = |f: &mut Frame, l: bool, rt: bool, u: bool, d: bool, w: f32| {
        let half = w / 2.0;
        if l {
            f.g.fill(Rect::new(left, cy - half, cx - left + half, w), 0.0, c);
        }
        if rt {
            f.g.fill(Rect::new(cx - half, cy - half, right - cx + half, w), 0.0, c);
        }
        if u {
            f.g.fill(Rect::new(cx - half, top, w, cy - top + half), 0.0, c);
        }
        if d {
            f.g.fill(Rect::new(cx - half, cy - half, w, bottom - cy + half), 0.0, c);
        }
    };
    let shade = |a: u8| Color(c.0, ((c.1 as u16 * a as u16) / 255) as u8);
    match ch {
        '─' => arms(f, true, true, false, false, THIN),
        '━' => arms(f, true, true, false, false, THICK),
        '│' => arms(f, false, false, true, true, THIN),
        '┃' => arms(f, false, false, true, true, THICK),
        '┌' => arms(f, false, true, false, true, THIN),
        '┐' => arms(f, true, false, false, true, THIN),
        '└' => arms(f, false, true, true, false, THIN),
        '┘' => arms(f, true, false, true, false, THIN),
        '├' => arms(f, false, true, true, true, THIN),
        '┤' => arms(f, true, false, true, true, THIN),
        '┬' => arms(f, true, true, false, true, THIN),
        '┴' => arms(f, true, true, true, false, THIN),
        '┼' => arms(f, true, true, true, true, THIN),
        '█' => f.g.fill(r, 0.0, c),
        '▒' => f.g.fill(r, 0.0, shade(0x80)),
        '░' => f.g.fill(r, 0.0, shade(0x38)),
        '▀' => f.g.fill(Rect::new(left, top, r.w, r.h / 2.0), 0.0, c),
        '▐' => f.g.fill(Rect::new(cx, top, r.w / 2.0, r.h), 0.0, c),
        '▁'..='▇' => {
            let eighths = (ch as u32 - '▁' as u32 + 1) as f32;
            let h = r.h * eighths / 8.0;
            f.g.fill(Rect::new(left, bottom - h, r.w, h), 0.0, c);
        }
        '■' => f.g.fill(Rect::new(cx - 2.5, cy - 2.5, 5.0, 5.0), 0.0, c),
        '•' => f.g.fill(Rect::new(cx - 1.5, cy - 1.5, 3.0, 3.0), 0.0, c),
        '▸' => f.g.fill_path(&[Seg::Move(cx - 2.0, cy - 3.5), Seg::Line(cx + 2.5, cy), Seg::Line(cx - 2.0, cy + 3.5), Seg::Close], c),
        '▾' => f.g.fill_path(&[Seg::Move(cx - 3.0, cy - 1.5), Seg::Line(cx + 3.0, cy - 1.5), Seg::Line(cx, cy + 2.5), Seg::Close], c),
        '≡' => {
            for dy in [-3.0, 0.0, 3.0] {
                f.g.fill(Rect::new(left + 1.0, cy + dy - 0.5, r.w - 2.0, 1.0), 0.0, c);
            }
        }
        _ => return false,
    }
    true
}

/// Draws the text mode under the title bar and registers what can be
/// pressed.
pub(super) fn draw(f: &mut Frame, v: &View) {
    let grid = build(v);
    f.g.fill(Rect::new(0.0, TITLE_H, WIDTH, HEIGHT - TITLE_H), 0.0, th().bg);

    for y in 0..ROWS {
        let row = &grid.cells[(y * COLS) as usize..((y + 1) * COLS) as usize];
        let mut x = 0;
        while x < COLS {
            let cell = row[x as usize];
            // A run: neighbours of one style, one action, drawn as one text.
            let mut end = x + 1;
            while end < COLS && same_run(&cell, &row[end as usize]) {
                end += 1;
            }
            draw_run(f, &row[x as usize..end as usize], x, y);
            x = end;
        }
    }
}

/// Cells that can share a run: same look, same action, plain letters.
fn same_run(a: &Cell, b: &Cell) -> bool {
    a.style == b.style && a.act == b.act && a.color == b.color
}

fn draw_run(f: &mut Frame, run: &[Cell], x: i32, y: i32) {
    let first = run[0];
    let (fg, bg, weight) = colors(first.style);
    let fg = first.color.map_or(fg, Color::rgb);
    let (start, last) = (cell_rect(x, y), cell_rect(x + run.len() as i32 - 1, y));
    let area = Rect::new(start.x, start.y, last.x + last.w - start.x, CELL_H);
    if let Some(bg) = bg {
        f.g.fill(area, 0.0, bg);
    }

    // What can be pressed answers the pointer with an underline.
    let mut hot = false;
    if let Some(act) = first.act {
        let kind = match act {
            Act::Slider(_) => {
                // The whole track is one slider.
                let (x0, x1) = (area.x + CELL_W / 2.0, area.x + area.w - CELL_W / 2.0);
                Kind::Slider { x0, x1, steps: slider_steps() }
            }
            _ => Kind::Button,
        };
        f.hits.push(Hit { act, rect: area, kind });
        hot = f.hot == Some(act) && bg.is_none() && !matches!(act, Act::Slider(_));
    }

    // Letters go through the font word by word; everything else is drawn.
    let mut word = String::new();
    let mut word_x = 0;
    for (k, cell) in run.iter().enumerate() {
        let r = cell_rect(x + k as i32, y);
        let plain = cell.ch != ' ' && cell.ch != CONTINUED && !shape(f, cell.ch, r, fg);
        if plain {
            if word.is_empty() {
                word_x = k;
            }
            word.push(cell.ch);
            continue;
        }
        flush(f, &mut word, x + word_x as i32, y, weight, fg);
    }
    flush(f, &mut word, x + word_x as i32, y, weight, fg);
    if hot {
        f.g.fill(Rect::new(area.x, area.y + CELL_H - 1.5, area.w, 1.0), 0.0, fg);
    }
}

/// Steps of the DPI slider: as many as the sensor has; the track only
/// shows them coarser.
fn slider_steps() -> usize {
    crate::protocol::dpi_steps()
}

/// Draws the letters gathered so far, starting at their own cell, so a
/// font that runs wide or narrow cannot shift what follows.
fn flush(f: &mut Frame, word: &mut String, x: i32, y: i32, weight: Weight, c: Color) {
    if word.is_empty() {
        return;
    }
    let r = cell_rect(x, y);
    let w = cells(word) as f32 * CELL_W + CELL_W;
    f.g.text(word, Rect::new(r.x, r.y, w, CELL_H), Size::Term, weight, Align::Left, c);
    word.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_cut_to_its_cells() {
        assert_eq!(fit("Overview", 10), "Overview");
        assert_eq!(fit("Forward side button", 10), "Forward s…");
        // A wide character takes two cells.
        assert_eq!(cells("日本語"), 6);
        assert_eq!(fit("日本語日本語", 7), "日本語…");
    }

    #[test]
    fn writing_never_leaves_the_grid() {
        let mut g = Grid::new();
        let end = g.put(COLS - 3, 0, "abcdef", Style::Text);
        assert_eq!(end, COLS + 3);
        g.put(0, ROWS + 5, "below", Style::Text);
        g.put(-4, 0, "left", Style::Text);
        assert_eq!(g.cells.len(), (COLS * ROWS) as usize);
        assert_eq!(g.cells[(COLS - 1) as usize].ch, 'c');
    }

    #[test]
    fn big_numbers_are_three_rows_of_blocks() {
        let mut g = Grid::new();
        let end = g.big(0, 0, "78", Style::Head);
        assert_eq!(end, 8);
        let row = |y: i32| -> String { (0..7).map(|x| g.cells[(y * COLS + x) as usize].ch).collect() };
        assert_eq!((row(0), row(1), row(2)), ("▀▀█ █▀█".to_string(), "  █ █▀█".to_string(), "  ▀ ▀▀▀".to_string()));
    }

    #[test]
    fn a_gauge_fills_its_share() {
        let mut g = Grid::new();
        g.gauge(0, 0, 10, 0.78, Style::Accent);
        let filled = (0..10).filter(|&x| g.cells[x as usize].ch == '█').count();
        assert_eq!(filled, 8);
        assert_eq!(g.cells[9].ch, '░');
    }

    #[test]
    fn long_text_wraps_at_spaces() {
        let mut g = Grid::new();
        let rows = g.wrap(0, 0, 12, "one two three four five", Style::Dim);
        assert_eq!(rows, 3);
        let row = |y: i32| -> String { (0..12).map(|x| g.cells[(y * COLS + x) as usize].ch).collect::<String>().trim_end().to_string() };
        assert_eq!([row(0), row(1), row(2)], ["one two", "three four", "five"]);
    }
}
