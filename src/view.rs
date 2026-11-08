//! Screens. `draw` paints the whole UI from a `View` snapshot and returns
//! the clickable areas (`Hit`) of that frame; the window layer turns mouse
//! input on them into `Act`s for the controller. Immediate mode: no widget
//! objects, a frame is just a function of state.
//!
//! ```text
//!  +---------+------------------------------------------+
//!  | Whisker |  Title                                   |
//!  |  nav    |  page content (cards)                    |
//!  |  ...    |                                          |
//!  | status  |                                          |
//!  +---------+------------------------------------------+
//!    210 px          content x = 234 .. 796
//! ```

use crate::gfx::{Align, Color, Gfx, Rect, Weight};
use crate::i18n::{Lang, T, fill, tr};
use crate::protocol::{
    Action, BRIGHTNESS_MAX, BUTTON_COUNT, DPI_COLOR_SLOTS, Direction, Effect, Lighting, Lod, MacroEvent, MacroMode, Power,
    SPEED_MAX,
};

mod buttons;
pub use buttons::{DelayMode, Picker, Tab};
use crate::store::{DEBOUNCE_CHOICES, SLEEP_CHOICES};
use crate::theme::{self, THEMES, Theme};

fn th() -> &'static Theme {
    theme::current()
}

pub const WIDTH: f32 = 820.0;
pub const HEIGHT: f32 = 560.0;

const SIDE_W: f32 = 210.0;
const X0: f32 = 234.0;
const CW: f32 = 562.0;
const TOP: f32 = 70.0;
const GAP: f32 = 14.0;
const RADIUS: f32 = 12.0;

const LOW: u8 = 20;
const MID: u8 = 50;

pub const PAGES: [T; 9] = [
    T::PageOverview, T::PageDpi, T::PagePolling, T::PageLighting, T::PageButtons, T::PageMacros, T::PageStats,
    T::PageAppearance, T::PageSettings,
];
pub const PAGE_SETTINGS: usize = 8;

/// Pickable colors for lighting and the DPI indicator.
pub const PALETTE: [u32; 12] = [
    0xFF0000, 0xFF6A00, 0xFFD000, 0x7CFF00, 0x00FF00, 0x00FFC8,
    0x00B4FF, 0x0000FF, 0x8000FF, 0xFF00FF, 0xFF3C8C, 0xFFFFFF,
];
const EFFECT_NAMES: [T; 8] = [
    T::EffOff, T::EffStreaming, T::EffBreathing, T::EffSteady, T::EffNeon, T::EffSnake, T::EffColorful, T::EffWave,
];
pub const RATES: [u32; 4] = [125, 250, 500, 1000];
const PERIODS: [T; 3] = [T::PeriodToday, T::PeriodWeek, T::PeriodAll];

/// Everything the user can click. Also used as the hit id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Nav(usize),
    Activate(usize),
    Toggle(usize),
    Slider(usize),
    Polling(usize),
    Period(usize),
    Autostart,
    Install,
    Uninstall,
    SetupAutostart,
    SetupInstall,
    SetupSkip,
    /// 0 = follow system, `i + 1` = `THEMES[i]`.
    Theme(usize),
    /// 0 = follow system, `i + 1` = `Lang::ALL[i]`.
    Lang(usize),
    /// Index into `Effect::ALL`.
    Effect(usize),
    Brightness(u8),
    Speed(u8),
    Direction(Direction),
    /// Palette index.
    Steady(usize),
    /// Palette index; toggles the color in the breathing list.
    Breath(usize),
    DarkMoving,
    /// Cycles the indicator color of a DPI stage.
    DpiColor(usize),
    /// Index into `DEBOUNCE_CHOICES`.
    Debounce(usize),
    Lod(Lod),
    /// Index into `SLEEP_CHOICES`.
    Sleep(usize),
    EditButton(usize),
    ResetButtons,
    PickTab(Tab),
    /// Assigns the action to the picked button and closes the picker.
    PickAction(Action),
    /// Toggles a `MOD_*` bit of the keyboard shortcut.
    PickMod(u8),
    PickCapture,
    PickInterval(u8),
    PickLoops(u8),
    PickMacro(usize),
    PickMode(MacroMode),
    PickApply,
    PickCancel,
    MacroSel(usize),
    MacroRecord,
    MacroClear,
    MacroDelay(DelayMode),
}

#[derive(Clone, Copy)]
pub enum Kind {
    Button,
    /// Horizontal slider over `steps` positions between `x0` and `x1`.
    Slider { x0: f32, x1: f32, steps: usize },
}

#[derive(Clone, Copy)]
pub struct Hit {
    pub act: Act,
    pub rect: Rect,
    pub kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Charge {
    Discharging,
    Charging,
    Full,
}

pub struct StageView {
    pub dpi: u32,
    pub index: usize,
    pub enabled: bool,
}

/// Snapshot of what the screens show; owned by the controller.
pub struct View {
    pub page: usize,
    pub period: usize,
    pub connected: bool,
    pub link: String,
    pub battery: u8,
    pub charge: Charge,
    pub battery_note: String,
    pub stages: Vec<StageView>,
    pub active: usize,
    pub steps: usize,
    pub polling: usize,
    pub measured: String,
    pub today_clicks: String,
    pub today_distance: String,
    pub tiles: Vec<(&'static str, String)>,
    /// Battery history as (0..1 time, 0..100 %) points.
    pub chart: Vec<(f32, f32)>,
    pub chart_caption: String,
    pub autostart: bool,
    pub installed: bool,
    pub install_dir: String,
    pub error: String,
    pub data_dir: String,
    pub ask_setup: bool,
    pub setup_autostart: bool,
    /// Selected entries of the appearance pickers (0 = system).
    pub theme_pick: usize,
    pub lang_pick: usize,
    pub lighting: Lighting,
    pub dpi_colors: [u32; DPI_COLOR_SLOTS],
    pub debounce: u8,
    pub lod: Lod,
    pub power: Power,
    pub buttons: [Action; BUTTON_COUNT],
    pub macros: Vec<Vec<MacroEvent>>,
    /// Open "assign action" dialog.
    pub picker: Option<Picker>,
    pub macro_sel: usize,
    pub recording: bool,
    pub delay_mode: DelayMode,
    /// Short warning under the buttons list (e.g. last left click removed).
    pub notice: String,
}

struct Frame<'a> {
    g: &'a mut Gfx,
    hot: Option<Act>,
    hits: Vec<Hit>,
}

impl Frame<'_> {
    fn button(&mut self, act: Act, r: Rect) -> bool {
        self.hits.push(Hit { act, rect: r, kind: Kind::Button });
        self.hot == Some(act)
    }

    fn card(&self, r: Rect) {
        self.g.fill(r, RADIUS, th().card);
        self.g.stroke(r, RADIUS, 1.0, th().border);
    }

    fn caption(&mut self, s: &str, x: f32, y: f32) {
        self.caption_in(s, x, y, 300.0);
    }

    /// Caption shrunk to fit `w` (long translations).
    fn caption_in(&mut self, s: &str, x: f32, y: f32, w: f32) {
        let size = fit_size(s, w, 11.5);
        self.g.text(s, Rect::new(x, y, w, 16.0), size, Weight::Semibold, Align::Left, th().muted);
    }

    fn bar(&self, r: Rect, pct: u8, c: Color) {
        self.g.fill(r, r.h / 2.0, th().border);
        let w = r.w * pct.min(100) as f32 / 100.0;
        if w > 0.0 {
            self.g.fill(Rect::new(r.x, r.y, w.max(r.h), r.h), r.h / 2.0, c);
        }
    }

    /// Pill switch, 40 x 22.
    fn toggle(&mut self, act: Option<Act>, x: f32, y: f32, on: bool) {
        let r = Rect::new(x, y, 40.0, 22.0);
        let hot = act.is_some_and(|a| self.button(a, r));
        let dim = act.is_none();

        if on {
            let c = if dim { th().faint } else if hot { th().accent_hover } else { th().accent };
            self.g.fill(r, 11.0, c);
            self.g.circle(x + 29.0, y + 11.0, 7.0, th().on_accent);
        } else {
            self.g.fill(r, 11.0, if hot { th().hover } else { th().card });
            self.g.stroke(r, 11.0, 1.5, th().faint);
            self.g.circle(x + 11.0, y + 11.0, 6.0, th().muted);
        }
    }

    fn push_button(&mut self, act: Act, r: Rect, label: &str, primary: bool) {
        let hot = self.button(act, r);
        if primary {
            self.g.fill(r, 8.0, if hot { th().accent_hover } else { th().accent });
            self.g.text(label, r, 14.0, Weight::Semibold, Align::Center, th().on_accent);
            return;
        }
        self.g.fill(r, 8.0, if hot { th().hover } else { th().card });
        self.g.stroke(r, 8.0, 1.0, th().border);
        self.g.text(label, r, 14.0, Weight::Semibold, Align::Center, th().text);
    }

    /// Selectable tile with a title and optional subtitle.
    fn choice(&mut self, act: Act, r: Rect, title: &str, sub: &str, selected: bool) {
        let hot = self.button(act, r);
        let bg = if selected { th().accent_soft } else if hot { th().hover } else { th().card };
        self.g.fill(r, 10.0, th().card);
        self.g.fill(r, 10.0, bg);
        let (bw, bc) = if selected { (2.0, th().accent) } else { (1.0, th().border) };
        self.g.stroke(r, 10.0, bw, bc);

        let color = if selected { th().text } else { th().muted };
        if sub.is_empty() {
            self.g.text(title, r, fit_size(title, r.w - 14.0, 15.0), Weight::Semibold, Align::Center, color);
            return;
        }
        // Compact tiles (< 56 px) get smaller type so the two lines don't touch.
        let (title_size, sub_size, pad) = if r.h < 56.0 { (14.0, 11.0, 4.0) } else { (17.0, 12.0, 8.0) };
        let top = Rect::new(r.x, r.y + pad, r.w, r.h / 2.0);
        let bottom = Rect::new(r.x, r.y + r.h / 2.0 + pad / 2.0, r.w, r.h / 2.0 - pad - 2.0);
        self.g.text(title, top, title_size, Weight::Semibold, Align::Center, color);
        self.g.text(sub, bottom, sub_size, Weight::Normal, Align::Center, th().faint);
    }
}

/// Shrinks a font size so a one-line label fits `width` (rough estimate:
/// a Latin glyph is ~0.56 em, CJK ~1 em).
fn fit_size(text: &str, width: f32, size: f32) -> f32 {
    const MIN: f32 = 10.0;
    let ems: f32 = text.chars().map(|c| if (c as u32) < 0x2E80 { 0.56 } else { 1.0 }).sum();
    if ems * size <= width {
        return size;
    }
    (width / ems).max(MIN)
}

pub fn battery_color(v: &View) -> Color {
    match v.battery {
        _ if v.charge == Charge::Charging => th().accent,
        b if b <= LOW => th().bad,
        b if b <= MID => th().warn,
        _ => th().good,
    }
}

pub fn draw(g: &mut Gfx, v: &View, hot: Option<Act>) -> Vec<Hit> {
    let mut f = Frame { g, hot, hits: Vec::new() };
    f.g.clear(th().bg);

    sidebar(&mut f, v);
    let title = Rect::new(X0, 18.0, CW, 36.0);
    f.g.text(tr(PAGES[v.page]), title, 24.0, Weight::Bold, Align::Left, th().text);

    match v.page {
        0 => overview(&mut f, v),
        1 => dpi(&mut f, v),
        2 => performance(&mut f, v),
        3 => lighting(&mut f, v),
        4 => buttons::buttons_page(&mut f, v),
        5 => buttons::macros_page(&mut f, v),
        6 => stats(&mut f, v),
        7 => appearance(&mut f, v),
        _ => settings(&mut f, v),
    }

    if let Some(p) = &v.picker {
        // Modal: drop hits of the page underneath.
        f.hits.clear();
        buttons::picker(&mut f, v, p);
    }

    if v.ask_setup {
        // The dialog is modal: drop hits of the page underneath.
        f.hits.clear();
        setup_dialog(&mut f, v);
    }
    f.hits
}

// ---- icons (24 x 24 grid scaled into `s` px at x, y) -----------------------

fn icon(g: &Gfx, page: usize, x: f32, y: f32, c: Color) {
    let s = 18.0 / 24.0;
    let p = |a: f32, b: f32| (x + a * s, y + b * s);
    let w = 1.8;

    match page {
        0 => {
            for (a, b) in [(4.0, 4.0), (13.0, 4.0), (4.0, 13.0), (13.0, 13.0)] {
                let (px, py) = p(a, b);
                g.stroke(Rect::new(px, py, 7.0 * s, 7.0 * s), 1.0, w, c);
            }
        }
        1 => {
            let (cx, cy) = p(12.0, 12.0);
            g.ring(cx, cy, 4.0 * s, w, c);
            for (a, b, d, e) in [(12.0, 2.0, 12.0, 6.0), (12.0, 18.0, 12.0, 22.0), (2.0, 12.0, 6.0, 12.0), (18.0, 12.0, 22.0, 12.0)] {
                let ((x0, y0), (x1, y1)) = (p(a, b), p(d, e));
                g.line(x0, y0, x1, y1, w, c);
            }
        }
        2 => {
            let pts: Vec<_> = [(2.0, 12.0), (7.0, 12.0), (9.0, 6.0), (13.0, 18.0), (15.0, 12.0), (22.0, 12.0)].iter().map(|&(a, b)| p(a, b)).collect();
            g.polyline(&pts, w, c);
        }
        3 => {
            // Sun: lighting.
            let (cx, cy) = p(12.0, 12.0);
            g.ring(cx, cy, 4.5 * s, w, c);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let (x0, y0) = (cx + a.cos() * 7.5 * s, cy + a.sin() * 7.5 * s);
                let (x1, y1) = (cx + a.cos() * 10.0 * s, cy + a.sin() * 10.0 * s);
                g.line(x0, y0, x1, y1, w, c);
            }
        }
        4 => {
            // Mouse outline: buttons.
            let (x0, y0) = p(7.0, 3.0);
            g.stroke(Rect::new(x0, y0, 10.0 * s, 18.0 * s), 5.0 * s, w, c);
            let ((lx, ly0), (_, ly1)) = (p(12.0, 4.0), p(12.0, 10.0));
            g.line(lx, ly0, lx, ly1, w, c);
        }
        5 => {
            // Keyboard: macros.
            let (x0, y0) = p(2.5, 6.0);
            g.stroke(Rect::new(x0, y0, 19.0 * s, 12.0 * s), 2.0, w, c);
            for (a, b) in [(6.5, 10.0), (10.5, 10.0), (14.5, 10.0), (17.5, 10.0)] {
                let (dx, dy) = p(a, b);
                g.circle(dx, dy, 1.0, c);
            }
            let ((sx0, sy), (sx1, _)) = (p(8.0, 14.0), p(16.0, 14.0));
            g.line(sx0, sy, sx1, sy, w, c);
        }
        6 => {
            for (a, top) in [(5.0, 11.0), (12.0, 5.0), (19.0, 14.0)] {
                let ((x0, y0), (_, y1)) = (p(a, top), p(a, 20.0));
                g.line(x0, y0, x0, y1, w + 0.4, c);
            }
        }
        7 => {
            // Palette: ring with three paint dots.
            let (cx, cy) = p(12.0, 12.0);
            g.ring(cx, cy, 9.0 * s, w, c);
            for (a, b) in [(8.5, 9.0), (13.0, 7.5), (16.0, 11.5)] {
                let (dx, dy) = p(a, b);
                g.circle(dx, dy, 1.6, c);
            }
        }
        _ => {
            for (b, knob) in [(7.0, 16.0), (17.0, 8.0)] {
                let ((x0, y0), (x1, _)) = (p(3.0, b), p(21.0, b));
                g.line(x0, y0, x1, y0, w, c);
                let (kx, ky) = p(knob, b);
                g.circle(kx, ky, 3.0 * s + 1.0, th().side);
                g.ring(kx, ky, 3.0 * s, w, c);
            }
        }
    }
}

fn mouse_logo(g: &Gfx, x: f32, y: f32) {
    g.stroke(Rect::new(x, y, 18.0, 27.0), 9.0, 2.0, th().accent);
    g.line(x + 9.0, y + 1.0, x + 9.0, y + 10.0, 2.0, th().accent);
}

// ---- sidebar -------------------------------------------------------------

fn sidebar(f: &mut Frame, v: &View) {
    f.g.fill(Rect::new(0.0, 0.0, SIDE_W, HEIGHT), 0.0, th().side);

    mouse_logo(f.g, 26.0, 20.0);
    f.g.text(crate::APP_NAME, Rect::new(56.0, 16.0, 140.0, 24.0), 18.0, Weight::Bold, Align::Left, th().text);
    f.g.text("White Shark", Rect::new(56.0, 38.0, 140.0, 16.0), 11.0, Weight::Normal, Align::Left, th().faint);

    for (i, &key) in PAGES.iter().enumerate() {
        let name = tr(key);
        let r = Rect::new(14.0, 80.0 + i as f32 * 44.0, SIDE_W - 28.0, 40.0);
        let hot = f.button(Act::Nav(i), r);
        let sel = v.page == i;
        if sel || hot {
            f.g.fill(r, 8.0, if sel { th().accent_soft } else { th().hover });
        }
        icon(f.g, i, r.x + 12.0, r.y + 11.0, if sel { th().accent } else { th().muted });
        let label = Rect::new(r.x + 42.0, r.y, r.w - 42.0, r.h);
        f.g.text(name, label, 14.0, Weight::Normal, Align::Left, if sel { th().text } else { th().muted });
    }

    // Connection + battery summary.
    let card = Rect::new(14.0, HEIGHT - 78.0, SIDE_W - 28.0, 64.0);
    f.g.fill(card, 10.0, th().card);
    f.g.circle(card.x + 16.0, card.y + 20.0, 4.0, if v.connected { th().good } else { th().faint });
    f.g.text(&v.link, Rect::new(card.x + 28.0, card.y + 10.0, card.w - 34.0, 20.0), 12.0, Weight::Normal, Align::Left, th().muted);

    if v.connected {
        f.bar(Rect::new(card.x + 12.0, card.y + 41.0, card.w - 70.0, 6.0), v.battery, battery_color(v));
        let pct = Rect::new(card.x + card.w - 52.0, card.y + 34.0, 40.0, 20.0);
        f.g.text(&format!("{} %", v.battery), pct, 12.0, Weight::Semibold, Align::Right, th().text);
    }
}

// ---- pages ---------------------------------------------------------------

fn stat_card(f: &mut Frame, r: Rect, label: &str, value: &str, note: &str) {
    f.card(r);
    f.caption(label, r.x + 16.0, r.y + 16.0);
    f.g.text(value, Rect::new(r.x + 16.0, r.y + 34.0, r.w - 32.0, 36.0), 26.0, Weight::Bold, Align::Left, th().text);
    f.g.text(note, Rect::new(r.x + 16.0, r.y + 74.0, r.w - 32.0, 32.0), 12.0, Weight::Normal, Align::Wrap, th().muted);
}

fn overview(f: &mut Frame, v: &View) {
    let tint = battery_color(v);

    let card = Rect::new(X0, TOP, CW, 118.0);
    f.card(card);
    f.caption(tr(T::Battery), card.x + 20.0, card.y + 22.0);
    let big = if v.connected { format!("{} %", v.battery) } else { "—".into() };
    let big_c = if v.connected { tint } else { th().faint };
    f.g.text(&big, Rect::new(card.x + 20.0, card.y + 40.0, 170.0, 56.0), 44.0, Weight::Bold, Align::Left, big_c);

    let note = match v.charge {
        _ if !v.connected => v.link.as_str(),
        Charge::Charging => tr(T::Charging),
        Charge::Full => tr(T::FullyCharged),
        Charge::Discharging => v.battery_note.as_str(),
    };
    f.bar(Rect::new(card.x + 200.0, card.y + 46.0, card.w - 220.0, 10.0), if v.connected { v.battery } else { 0 }, tint);
    f.g.text(note, Rect::new(card.x + 200.0, card.y + 64.0, card.w - 220.0, 22.0), 13.0, Weight::Normal, Align::Left, th().muted);

    let w = (CW - 2.0 * GAP) / 3.0;
    let y = TOP + 118.0 + GAP;
    let (dpi, note) = match v.stages.get(v.active) {
        Some(s) => (s.dpi.to_string(), fill(tr(T::StageOf), &[&(v.active + 1).to_string(), &v.stages.iter().filter(|s| s.enabled).count().to_string()])),
        None => ("—".into(), String::new()),
    };
    stat_card(f, Rect::new(X0, y, w, 110.0), tr(T::ActiveDpi), &dpi, &note);
    let hz = format!("{} Hz", RATES[v.polling]);
    stat_card(f, Rect::new(X0 + w + GAP, y, w, 110.0), tr(T::PollingRate), &hz, &fill(tr(T::MeasuredValue), &[&v.measured]));
    let today = fill(tr(T::ClicksMoved), &[&v.today_distance]);
    stat_card(f, Rect::new(X0 + 2.0 * (w + GAP), y, w, 110.0), tr(T::Today), &v.today_clicks, &today);

    // Quick DPI switch: one chip per enabled stage.
    let card = Rect::new(X0, y + 110.0 + GAP, CW, 96.0);
    f.card(card);
    f.caption(tr(T::QuickDpi), card.x + 20.0, card.y + 16.0);
    let mut x = card.x + 20.0;
    for (i, s) in v.stages.iter().enumerate().filter(|(_, s)| s.enabled) {
        f.choice(Act::Activate(i), Rect::new(x, card.y + 40.0, 80.0, 38.0), &s.dpi.to_string(), "", i == v.active);
        x += 88.0;
    }
}

fn dpi(f: &mut Frame, v: &View) {
    const ROW: f32 = 52.0;
    let card = Rect::new(X0, TOP, CW, ROW * v.stages.len() as f32 + 16.0);
    f.card(card);

    for (i, s) in v.stages.iter().enumerate() {
        let y = card.y + 8.0 + i as f32 * ROW;
        let active = i == v.active;
        if active {
            f.g.fill(Rect::new(card.x + 8.0, y, card.w - 16.0, ROW), 8.0, th().card_hi);
        }
        let cy = y + ROW / 2.0;

        // Active-stage radio; only enabled stages can be made active.
        let radio = Rect::new(card.x + 16.0, y + 10.0, 32.0, 32.0);
        if s.enabled {
            f.button(Act::Activate(i), radio);
        }
        f.g.ring(card.x + 30.0, cy, 8.0, 2.0, if active { th().accent } else { th().faint });
        if active {
            f.g.circle(card.x + 30.0, cy, 4.0, th().accent);
        }

        let num_c = if s.enabled { th().text } else { th().faint };
        f.g.text(&(i + 1).to_string(), Rect::new(card.x + 50.0, y, 16.0, ROW), 14.0, Weight::Semibold, Align::Left, num_c);

        // The active stage can't be switched off.
        let toggle_act = (!active).then_some(Act::Toggle(i));
        f.toggle(toggle_act, card.x + 72.0, cy - 11.0, s.enabled);

        // Indicator color; click cycles through the palette.
        let (dx, dot) = (card.x + 132.0, Color::rgb(v.dpi_colors[i]));
        let hot = f.button(Act::DpiColor(i), Rect::new(dx - 12.0, cy - 12.0, 24.0, 24.0));
        if hot {
            f.g.ring(dx, cy, 10.0, 1.5, th().muted);
        }
        f.g.circle(dx, cy, 7.0, dot);
        f.g.ring(dx, cy, 7.0, 1.0, th().border);

        // Slider.
        let (x0, x1) = (card.x + 160.0, card.x + card.w - 120.0);
        let t = s.index as f32 / (v.steps - 1).max(1) as f32;
        let kx = x0 + (x1 - x0) * t;
        let track_c = if s.enabled { th().accent } else { th().faint };
        f.g.fill(Rect::new(x0, cy - 2.0, x1 - x0, 4.0), 2.0, th().border);
        f.g.fill(Rect::new(x0, cy - 2.0, kx - x0, 4.0), 2.0, track_c);
        f.g.circle(kx, cy, 9.0, th().card_hi);
        f.g.circle(kx, cy, 6.5, track_c);
        if s.enabled {
            let area = Rect::new(x0 - 10.0, y + 6.0, x1 - x0 + 20.0, ROW - 12.0);
            f.hits.push(Hit { act: Act::Slider(i), rect: area, kind: Kind::Slider { x0, x1, steps: v.steps } });
        }

        let label = Rect::new(card.x + card.w - 108.0, y, 92.0, ROW);
        f.g.text(&format!("{} DPI", s.dpi), label, 15.0, Weight::Semibold, Align::Right, num_c);
    }

    let note = format!("{} {}", tr(T::DpiHelp), tr(T::DpiColorHelp));
    let r = Rect::new(X0, card.y + card.h + 12.0, CW, 54.0);
    f.g.text(&note, r, 12.0, Weight::Normal, Align::Wrap, th().muted);
}

fn performance(f: &mut Frame, v: &View) {
    let w = (CW - 3.0 * 12.0) / 4.0;
    for (i, hz) in RATES.iter().enumerate() {
        let r = Rect::new(X0 + i as f32 * (w + 12.0), TOP, w, 60.0);
        f.choice(Act::Polling(i), r, &format!("{hz} Hz"), &format!("{} ms", 1000 / hz), v.polling == i);
    }

    let card = Rect::new(X0, TOP + 60.0 + GAP, CW, 88.0);
    f.card(card);
    f.caption(tr(T::Measured), card.x + 20.0, card.y + 16.0);
    f.g.text(&v.measured, Rect::new(card.x + 20.0, card.y + 34.0, 200.0, 42.0), 30.0, Weight::Bold, Align::Left, th().text);
    let hint = Rect::new(card.x + 230.0, card.y + 14.0, card.w - 250.0, 66.0);
    f.g.text(tr(T::PollingHelp), hint, 12.0, Weight::Normal, Align::Wrap, th().muted);

    // Debounce.
    let card = Rect::new(X0, card.y + card.h + GAP, CW, 112.0);
    f.card(card);
    f.caption(tr(T::Debounce), card.x + 20.0, card.y + 16.0);
    let help = Rect::new(card.x + 20.0, card.y + 34.0, card.w - 40.0, 32.0);
    f.g.text(tr(T::DebounceHelp), help, 12.0, Weight::Normal, Align::Wrap, th().muted);
    for (i, ms) in DEBOUNCE_CHOICES.iter().enumerate() {
        let r = Rect::new(card.x + 20.0 + i as f32 * 86.0, card.y + 68.0, 78.0, 32.0);
        f.choice(Act::Debounce(i), r, &format!("{ms} ms"), "", v.debounce == *ms);
    }

    // Lift-off distance and sleep timer.
    let card = Rect::new(X0, card.y + card.h + GAP, CW, 84.0);
    f.card(card);
    f.caption_in(tr(T::LiftOff), card.x + 20.0, card.y + 14.0, 186.0);
    for (k, lod) in [Lod::Mm1, Lod::Mm2].into_iter().enumerate() {
        let r = Rect::new(card.x + 20.0 + k as f32 * 70.0, card.y + 38.0, 62.0, 32.0);
        f.choice(Act::Lod(lod), r, &format!("{} mm", lod as u8), "", v.lod == lod);
    }
    let sx = card.x + 222.0;
    let caption = format!("{} ({})", tr(T::SleepAfter), fill(tr(T::Minutes), &[""]).trim());
    f.caption_in(&caption, sx, card.y + 14.0, card.w - 242.0);
    for (i, m) in SLEEP_CHOICES.iter().enumerate() {
        let r = Rect::new(sx + i as f32 * 40.0, card.y + 38.0, 36.0, 32.0);
        f.choice(Act::Sleep(i), r, &m.to_string(), "", v.power.sleep_min == *m);
    }
}

/// Color swatch; `selected` draws a ring around it.
fn swatch(f: &mut Frame, act: Act, cx: f32, cy: f32, color: u32, selected: bool) {
    let hot = f.button(act, Rect::new(cx - 15.0, cy - 15.0, 30.0, 30.0));
    if selected || hot {
        f.g.ring(cx, cy, 14.5, 2.0, if selected { th().text } else { th().muted });
    }
    f.g.circle(cx, cy, 11.0, Color::rgb(color));
    f.g.ring(cx, cy, 11.0, 1.0, th().border);
}

fn lighting(f: &mut Frame, v: &View) {
    const PAD: f32 = 20.0;
    let l = &v.lighting;

    // Effect picker: 4 x 2 chips.
    let card = Rect::new(X0, TOP, CW, 132.0);
    f.card(card);
    f.caption(tr(T::EffectCaption), card.x + PAD, card.y + 14.0);
    let w = (CW - 2.0 * PAD - 3.0 * 8.0) / 4.0;
    for (i, e) in Effect::ALL.iter().enumerate() {
        let (row, col) = ((i / 4) as f32, (i % 4) as f32);
        let r = Rect::new(card.x + PAD + col * (w + 8.0), card.y + 36.0 + row * 44.0, w, 36.0);
        f.choice(Act::Effect(i), r, tr(EFFECT_NAMES[i]), "", l.effect == *e);
    }

    // Brightness, speed, direction.
    let card = Rect::new(X0, card.y + card.h + GAP, CW, 84.0);
    f.card(card);
    let chip = |i: usize, x: f32| Rect::new(x + i as f32 * 42.0, card.y + 38.0, 36.0, 32.0);
    let x = card.x + PAD;
    f.caption_in(tr(T::Brightness), x, card.y + 14.0, 180.0);
    for b in 1..=BRIGHTNESS_MAX {
        f.choice(Act::Brightness(b), chip(b as usize - 1, x), &b.to_string(), "", l.brightness == b);
    }
    let x = card.x + PAD + 196.0;
    if l.effect.has_speed() {
        f.caption_in(tr(T::Speed), x, card.y + 14.0, 150.0);
        for sp in 1..=SPEED_MAX {
            f.choice(Act::Speed(sp), chip(sp as usize - 1, x), &sp.to_string(), "", l.speed == sp);
        }
    }
    let x = card.x + PAD + 360.0;
    if l.effect == Effect::Streaming {
        f.caption_in(tr(T::DirectionCaption), x, card.y + 14.0, 160.0);
        for (k, (d, arrow)) in [(Direction::Forward, "→"), (Direction::Reverse, "←")].into_iter().enumerate() {
            f.choice(Act::Direction(d), chip(k, x), arrow, "", l.direction == d);
        }
    }

    // Colors: one for steady, several for breathing.
    let mut y = card.y + card.h + GAP;
    if matches!(l.effect, Effect::Steady | Effect::Breathing) {
        let card = Rect::new(X0, y, CW, 84.0);
        f.card(card);
        let multi = l.effect == Effect::Breathing;
        f.caption(tr(if multi { T::ColorsCaption } else { T::ColorCaption }), card.x + PAD, card.y + 14.0);
        for (i, &c) in PALETTE.iter().enumerate() {
            let cx = card.x + PAD + 15.0 + i as f32 * 40.0;
            let (act, sel) = if multi { (Act::Breath(i), l.breathing.contains(&c)) } else { (Act::Steady(i), l.steady == c) };
            swatch(f, act, cx, card.y + 54.0, c, sel);
        }
        y = card.y + card.h + GAP;
    }

    // Lights off while moving, plus the wake-up note.
    let card = Rect::new(X0, y, CW, 56.0);
    f.card(card);
    let label = Rect::new(card.x + PAD, card.y, card.w - 100.0, card.h);
    f.g.text(tr(T::DarkMoving), label, 14.0, Weight::Semibold, Align::Left, th().text);
    f.toggle(Some(Act::DarkMoving), card.x + card.w - 58.0, card.y + 17.0, v.power.dark_while_moving);
    let note = Rect::new(X0, card.y + card.h + 10.0, CW, 34.0);
    f.g.text(tr(T::LightingNote), note, 12.0, Weight::Normal, Align::Wrap, th().muted);
}

fn stats(f: &mut Frame, v: &View) {
    for (i, name) in PERIODS.iter().enumerate() {
        let r = Rect::new(X0 + i as f32 * 104.0, TOP, 96.0, 36.0);
        f.choice(Act::Period(i), r, tr(*name), "", v.period == i);
    }

    let w = (CW - 2.0 * GAP) / 3.0;
    for (i, (label, value)) in v.tiles.iter().enumerate() {
        let (row, col) = ((i / 3) as f32, (i % 3) as f32);
        let r = Rect::new(X0 + col * (w + GAP), TOP + 48.0 + row * (78.0 + 12.0), w, 78.0);
        f.card(r);
        f.caption(label, r.x + 14.0, r.y + 14.0);
        f.g.text(value, Rect::new(r.x + 14.0, r.y + 32.0, r.w - 28.0, 34.0), 22.0, Weight::Bold, Align::Left, th().text);
    }

    let card = Rect::new(X0, TOP + 48.0 + 2.0 * 90.0, CW, HEIGHT - (TOP + 48.0 + 180.0) - 24.0);
    f.card(card);
    f.caption(tr(T::ChartTitle), card.x + 16.0, card.y + 14.0);
    let cap = Rect::new(card.x + card.w - 216.0, card.y + 12.0, 200.0, 18.0);
    f.g.text(&v.chart_caption, cap, 12.0, Weight::Normal, Align::Right, th().faint);

    let plot = Rect::new(card.x + 16.0, card.y + 40.0, card.w - 64.0, card.h - 56.0);
    if v.chart.len() < 2 {
        let msg = tr(T::ChartEmpty);
        f.g.text(msg, plot, 13.0, Weight::Normal, Align::Center, th().faint);
        return;
    }

    for (pct, label) in [(100.0, "100 %"), (50.0, "50 %"), (0.0, "0 %")] {
        let y = plot.y + plot.h * (1.0 - pct / 100.0);
        f.g.line(plot.x, y, plot.x + plot.w, y, 1.0, th().border);
        let r = Rect::new(plot.x + plot.w + 6.0, y - 8.0, 40.0, 16.0);
        f.g.text(label, r, 10.5, Weight::Normal, Align::Left, th().faint);
    }

    let pts: Vec<(f32, f32)> = v.chart.iter().map(|&(t, pct)| (plot.x + t * plot.w, plot.y + plot.h * (1.0 - pct / 100.0))).collect();
    f.g.polyline(&pts, 2.0, th().accent);
}

fn settings(f: &mut Frame, v: &View) {
    let card = Rect::new(X0, TOP, CW, 104.0);
    f.card(card);
    let title = if v.installed { tr(T::Installed) } else { tr(T::NotInstalled) };
    f.g.text(title, Rect::new(card.x + 18.0, card.y + 16.0, 380.0, 22.0), 15.0, Weight::Semibold, Align::Left, th().text);
    let desc = if v.installed { v.install_dir.clone() } else { fill(tr(T::InstallDesc), &[&v.install_dir]) };
    f.g.text(&desc, Rect::new(card.x + 18.0, card.y + 42.0, 390.0, 34.0), 12.0, Weight::Normal, Align::Wrap, th().muted);
    if !v.error.is_empty() {
        f.g.text(&v.error, Rect::new(card.x + 18.0, card.y + 78.0, 390.0, 20.0), 12.0, Weight::Normal, Align::Left, th().bad);
    }
    let btn = Rect::new(card.x + card.w - 150.0, card.y + 34.0, 132.0, 36.0);
    if v.installed {
        f.push_button(Act::Uninstall, btn, tr(T::Uninstall), false);
    } else {
        f.push_button(Act::Install, btn, tr(T::Install), true);
    }

    let card = Rect::new(X0, TOP + 104.0 + GAP, CW, 76.0);
    f.card(card);
    let title = Rect::new(card.x + 18.0, card.y + 16.0, 400.0, 22.0);
    f.g.text(tr(T::Autostart), title, 15.0, Weight::Semibold, Align::Left, th().text);
    let desc = Rect::new(card.x + 18.0, card.y + 40.0, 420.0, 20.0);
    f.g.text(tr(T::AutostartDesc), desc, 12.0, Weight::Normal, Align::Left, th().muted);
    // Autostart only points at the installed copy.
    let act = v.installed.then_some(Act::Autostart);
    f.toggle(act, card.x + card.w - 58.0, card.y + 27.0, v.autostart);

    let card = Rect::new(X0, TOP + 194.0 + GAP, CW, 152.0);
    f.card(card);
    f.g.text(tr(T::About), Rect::new(card.x + 18.0, card.y + 14.0, 400.0, 22.0), 15.0, Weight::Semibold, Align::Left, th().text);
    let lines = [tr(T::AboutTray)];
    for (i, l) in lines.iter().enumerate() {
        let r = Rect::new(card.x + 18.0, card.y + 44.0 + i as f32 * 36.0, card.w - 36.0, 34.0);
        f.g.text(l, r, 12.0, Weight::Normal, Align::Wrap, th().muted);
    }
    let r = Rect::new(card.x + 18.0, card.y + 122.0, card.w - 36.0, 18.0);
    f.g.text(&fill(tr(T::DataDir), &[&v.data_dir]), r, 12.0, Weight::Normal, Align::Left, th().faint);
}

fn setup_dialog(f: &mut Frame, v: &View) {
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);

    let d = Rect::new((WIDTH - 460.0) / 2.0, (HEIGHT - 290.0) / 2.0, 460.0, 290.0);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let inner = d.inset(24.0);
    f.g.text(tr(T::SetupTitle), Rect::new(inner.x, inner.y, inner.w, 28.0), 18.0, Weight::Bold, Align::Left, th().text);
    let text = fill(tr(T::SetupText), &[&v.install_dir]);
    f.g.text(&text, Rect::new(inner.x, inner.y + 38.0, inner.w, 64.0), 13.0, Weight::Normal, Align::Wrap, th().muted);

    let ty = inner.y + 118.0;
    f.toggle(Some(Act::SetupAutostart), inner.x, ty, v.setup_autostart);
    let label = Rect::new(inner.x + 52.0, ty - 6.0, inner.w - 52.0, 34.0);
    f.g.text(tr(T::SetupAutostart), label, 13.0, Weight::Normal, Align::Left, th().text);

    let by = d.y + d.h - 24.0 - 36.0;
    f.push_button(Act::SetupSkip, Rect::new(d.x + d.w - 24.0 - 128.0 - 10.0 - 120.0, by, 120.0, 36.0), tr(T::JustRun), false);
    f.push_button(Act::SetupInstall, Rect::new(d.x + d.w - 24.0 - 128.0, by, 128.0, 36.0), tr(T::Install), true);
}

// ---- appearance ------------------------------------------------------------

/// Picker tile showing a theme's own colors: card, three swatches, name.
fn theme_tile(f: &mut Frame, act: Act, r: Rect, t: &Theme, name: &str, selected: bool) {
    let hot = f.button(act, r);
    f.g.fill(r, 10.0, t.bg);
    let (bw, bc) = if selected { (2.0, th().accent) } else if hot { (1.5, th().muted) } else { (1.0, th().border) };
    f.g.stroke(r, 10.0, bw, bc);

    for (k, c) in [t.card, t.accent, t.text].into_iter().enumerate() {
        f.g.circle(r.x + 16.0 + k as f32 * 14.0, r.y + 18.0, 5.5, c);
    }
    let label = Rect::new(r.x + 10.0, r.y + 30.0, r.w - 20.0, 22.0);
    f.g.text(name, label, 12.5, Weight::Semibold, Align::Left, t.text);
}

fn appearance(f: &mut Frame, v: &View) {
    const COLS: usize = 5;
    const PAD: f32 = 20.0;
    const CELL_GAP: f32 = 10.0;
    let cell_w = (CW - 2.0 * PAD - (COLS - 1) as f32 * CELL_GAP) / COLS as f32;
    let cell = |i: usize, y0: f32, h: f32| {
        let (row, col) = ((i / COLS) as f32, (i % COLS) as f32);
        Rect::new(X0 + PAD + col * (cell_w + CELL_GAP), y0 + row * (h + CELL_GAP), cell_w, h)
    };

    // Themes: "system" first, shown with the light/dark pair it switches between.
    const TILE_H: f32 = 58.0;
    let rows = (THEMES.len() + 1).div_ceil(COLS) as f32;
    let card = Rect::new(X0, TOP, CW, 44.0 + rows * (TILE_H + CELL_GAP) + 6.0);
    f.card(card);
    f.caption(tr(T::Theme), card.x + PAD, card.y + 16.0);

    let sys = cell(0, card.y + 40.0, TILE_H);
    theme_tile(f, Act::Theme(0), sys, th(), tr(T::FollowSystem), v.theme_pick == 0);
    for (i, t) in THEMES.iter().enumerate() {
        let r = cell(i + 1, card.y + 40.0, TILE_H);
        theme_tile(f, Act::Theme(i + 1), r, t, t.name, v.theme_pick == i + 1);
    }

    // Languages: native names, "system" first.
    const LANG_H: f32 = 34.0;
    let y = card.y + card.h + GAP;
    let rows = (Lang::ALL.len() + 1).div_ceil(COLS) as f32;
    let card = Rect::new(X0, y, CW, 44.0 + rows * (LANG_H + CELL_GAP) + 6.0);
    f.card(card);
    f.caption(tr(T::Language), card.x + PAD, card.y + 16.0);

    f.choice(Act::Lang(0), cell(0, card.y + 40.0, LANG_H), tr(T::FollowSystem), "", v.lang_pick == 0);
    for (i, l) in Lang::ALL.iter().enumerate() {
        let r = cell(i + 1, card.y + 40.0, LANG_H);
        f.choice(Act::Lang(i + 1), r, l.native(), "", v.lang_pick == i + 1);
    }
}
