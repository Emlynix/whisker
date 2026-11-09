//! Screens. `draw` paints the whole UI from a `View` snapshot and returns
//! the clickable areas (`Hit`) of that frame; the window layer turns mouse
//! input on them into `Act`s for the controller. Immediate mode: no widget
//! objects, a frame is just a function of state.
//!
//! ```text
//!  +-----------+----------------------------------------+
//!  | Whisker   |  Title                  [status chip]  |
//!  |  mouse    |  Tab | Tab   (Buttons, Settings only)  |
//!  |  nav x6   |  page content (cards)                  |
//!  |           |                                        |
//!  | Settings  |                                        |
//!  | [status]  |            [toast · Undo]              |
//!  | v0.9.0    |                                        |
//!  +-----------+----------------------------------------+
//!    210 px          content x = 234 .. 796
//! ```

use crate::gfx::{Align, Color, Gfx, Rect, Weight};
use crate::i18n::{Lang, T, fill, tr};
use crate::protocol::{
    Action, BRIGHTNESS_MAX, BUTTON_COUNT, DPI_COLOR_SLOTS, Direction, Effect, Lighting, Lod, MacroEvent, MacroMode, Power,
    SPEED_MAX,
};

mod buttons;
mod device;
pub use buttons::{DelayMode, Picker, Tab, button_name};
pub use device::{Badge, Detect, DetectRow, DeviceView};
use crate::devices::{Caps, DpiUi};
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

/// Screens. Macros is a tab of Buttons; Appearance and Mouse are tabs of
/// Settings (`General`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Overview,
    Dpi,
    Polling,
    Lighting,
    Buttons,
    Macros,
    Stats,
    General,
    Appearance,
    Mouse,
}

/// Sidebar order; Settings (`General`) is pinned at the bottom.
const NAV: [Page; 6] = [Page::Overview, Page::Dpi, Page::Polling, Page::Lighting, Page::Buttons, Page::Stats];
const BUTTON_TABS: [(Page, T); 2] = [(Page::Buttons, T::PageButtons), (Page::Macros, T::PageMacros)];
const SETTING_TABS: [(Page, T); 3] = [(Page::General, T::TabGeneral), (Page::Appearance, T::PageAppearance), (Page::Mouse, T::TabMouse)];

impl Page {
    /// Sidebar entry the page belongs to, e.g. Macros -> Buttons.
    pub fn nav(self) -> Page {
        match self {
            Page::Macros => Page::Buttons,
            Page::Appearance | Page::Mouse => Page::General,
            p => p,
        }
    }

    fn title(self) -> T {
        match self.nav() {
            Page::Overview => T::PageOverview,
            Page::Dpi => T::PageDpi,
            Page::Polling => T::PagePolling,
            Page::Lighting => T::PageLighting,
            Page::Buttons => T::PageButtons,
            Page::Stats => T::PageStats,
            _ => T::PageSettings,
        }
    }

    /// The page sends settings to the mouse (offline banner, sync chip).
    fn configures(self) -> bool {
        matches!(self, Page::Dpi | Page::Polling | Page::Lighting | Page::Buttons | Page::Macros)
    }
}

const NAV_TOP: f32 = 76.0;
const NAV_STEP: f32 = 40.0;
/// Underlined tab row under the title; tabbed pages start below it.
const TAB_H: f32 = 30.0;
const BODY: f32 = TOP + TAB_H + GAP;

const COPYRIGHT: &str = "© 2026 Emlynix s.r.o.";
const LICENSE: &str = "GPL-3.0-or-later";
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether a page applies to the selected mouse.
pub fn page_visible(page: Page, caps: &Caps) -> bool {
    match page.nav() {
        Page::Dpi => caps.dpi != DpiUi::None,
        Page::Polling => caps.polling,
        Page::Lighting => caps.lighting,
        Page::Buttons => caps.buttons,
        _ => true,
    }
}

/// Sidebar pages shown for a mouse, in order (also Ctrl+1..6).
pub fn nav_pages(caps: &Caps) -> Vec<Page> {
    NAV.into_iter().filter(|p| page_visible(*p, caps)).collect()
}

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
    Nav(Page),
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
    ChangeMouse,
    /// Slider of a driver-based mouse's single DPI value.
    BasicDpi,
    BasicRate(u32),
    DetectSel(usize),
    DetectManual,
    DetectUse,
    /// Closes the dialog; the current mouse stays.
    DetectCancel,
    /// From the model list back to the detected mice.
    DetectBack,
    /// Reverts the last destructive action (toast button, Ctrl+Z).
    Undo,
    About,
    /// Closes the About or "assign macro" dialog.
    CloseDialog,
    OpenWeb(Web),
    Diagnostics,
    CopyDiagnostics,
    /// Opens the "assign macro to button" dialog.
    AssignMacro,
    /// Assigns the selected macro to this button.
    AssignTo(usize),
}

/// Pages of the project site the About dialog links to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Web {
    Repo,
    NewIssue,
}

/// Small modal dialogs (the bigger ones have their own state).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialog {
    About,
    AssignMacro,
}

/// Header chip: settings on their way to the mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sync {
    Hidden,
    Waiting,
    /// Fading out; opacity 0..=255.
    Saved(u8),
}

/// Bottom message, e.g. "All buttons reset · Undo".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub text: String,
    pub undo: bool,
}

/// Whether a list entry is the current one (sidebar, picker categories).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Current,
    Other,
}

impl Mark {
    fn of(current: bool) -> Mark {
        if current { Mark::Current } else { Mark::Other }
    }
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
    pub page: Page,
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
    /// Report rate measured from mouse input, while the mouse moves.
    pub measured: Option<u32>,
    pub today_clicks: String,
    pub today_distance: String,
    pub tiles: Vec<(&'static str, String)>,
    /// Battery history as (0..1 time, 0..100 %) points.
    pub chart: Vec<(f32, f32)>,
    /// X axis labels at 0..1, e.g. (0.5, "6.11.").
    pub chart_ticks: Vec<(f32, String)>,
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
    /// What the selected mouse supports; hides other pages.
    pub caps: Caps,
    /// Selected mouse, shown under the app name.
    pub device_name: String,
    pub device: DeviceView,
    /// Open "which mouse" dialog.
    pub detect: Option<Detect>,
    /// DPI and report rate of driver-based mice.
    pub basic: BasicView,
    /// The mouse reports a battery level.
    pub has_battery: bool,
    /// Keyboard focus ring (Tab), on the hit with this act.
    pub focus: Option<Act>,
    pub toast: Option<Toast>,
    pub sync: Sync,
    pub dialog: Option<Dialog>,
    /// Diagnostics section of the Mouse tab is expanded.
    pub diagnostics: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BasicView {
    pub dpi: Option<u32>,
    /// Current / total DPI stage for mice that only report the stage.
    pub stage: Option<u8>,
    pub stages: Option<u8>,
    /// Selectable DPI values (slider positions).
    pub values: Vec<u32>,
    pub rates: Vec<u32>,
    pub rate: Option<u32>,
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
        // Long translations shrink rather than spill out of the button.
        let size = fit_size(label, r.w - 16.0, 14.0);
        if primary {
            self.g.fill(r, 8.0, if hot { th().accent_hover } else { th().accent });
            self.g.text(label, r, size, Weight::Semibold, Align::Center, th().on_accent);
            return;
        }
        self.g.fill(r, 8.0, if hot { th().hover } else { th().card });
        self.g.stroke(r, 8.0, 1.0, th().border);
        self.g.text(label, r, size, Weight::Semibold, Align::Center, th().text);
    }

    /// Button that can't be pressed now, e.g. "Clear" on an empty macro.
    fn dead_button(&mut self, r: Rect, label: &str) {
        self.g.stroke(r, 8.0, 1.0, th().border);
        self.g.text(label, r, 14.0, Weight::Semibold, Align::Center, th().faint);
    }

    /// Underlined text tabs, e.g. "Buttons | Macros"; a tab is a page.
    fn tabs(&mut self, items: &[(Page, T)], current: Page) {
        const SIZE: f32 = 14.0;
        const PAD: f32 = 12.0;
        let y = TOP;
        self.g.fill(Rect::new(X0, y + TAB_H - 1.0, CW, 1.0), 0.0, th().border);

        let mut x = X0;
        for &(page, key) in items {
            let label = tr(key);
            let r = Rect::new(x, y, text_w(label, SIZE) + 2.0 * PAD, TAB_H);
            let hot = self.button(Act::Nav(page), r);
            let sel = page == current;
            let color = if sel || hot { th().text } else { th().muted };
            let weight = if sel { Weight::Semibold } else { Weight::Normal };
            self.g.text(label, Rect::new(r.x, r.y, r.w, r.h - 4.0), SIZE, weight, Align::Center, color);
            if sel {
                self.g.fill(Rect::new(r.x + 4.0, r.y + r.h - 3.0, r.w - 8.0, 3.0), 1.5, th().accent);
            }
            x += r.w + 4.0;
        }
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
    let ems = ems(text);
    if ems * size <= width {
        return size;
    }
    (width / ems).max(MIN)
}

/// Rough text width in em (see `fit_size`).
fn ems(text: &str) -> f32 {
    text.chars().map(|c| if (c as u32) < 0x2E80 { 0.56 } else { 1.0 }).sum()
}

/// Rough width of a one-line text in DIPs.
fn text_w(text: &str, size: f32) -> f32 {
    ems(text) * size
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
    f.g.text(tr(v.page.title()), title, 24.0, Weight::Bold, Align::Left, th().text);
    header_status(&mut f, v);

    match v.page {
        Page::Overview => overview(&mut f, v),
        Page::Dpi => dpi(&mut f, v),
        Page::Polling => performance(&mut f, v),
        Page::Lighting => lighting(&mut f, v),
        Page::Buttons | Page::Macros => {
            f.tabs(&BUTTON_TABS, v.page);
            if v.page == Page::Buttons { buttons::buttons_page(&mut f, v) } else { buttons::macros_page(&mut f, v) }
        }
        Page::Stats => stats(&mut f, v),
        Page::General | Page::Appearance | Page::Mouse => {
            f.tabs(&SETTING_TABS, v.page);
            match v.page {
                Page::General => settings(&mut f, v),
                Page::Appearance => appearance(&mut f, v),
                _ => device::device_page(&mut f, v),
            }
        }
    }

    if let Some(t) = &v.toast {
        toast(&mut f, t);
    }

    // Modal dialogs: drop hits of the page underneath. Install comes first.
    if let Some(p) = &v.picker {
        f.hits.clear();
        buttons::picker(&mut f, v, p);
    }
    match v.dialog {
        Some(Dialog::About) => {
            f.hits.clear();
            about_dialog(&mut f);
        }
        Some(Dialog::AssignMacro) => {
            f.hits.clear();
            buttons::assign_dialog(&mut f, v);
        }
        None => {}
    }
    if v.ask_setup {
        f.hits.clear();
        setup_dialog(&mut f, v);
    } else if let Some(d) = &v.detect {
        f.hits.clear();
        device::detect_dialog(&mut f, d);
    }

    focus_ring(&f, v.focus);
    f.hits
}

/// Keyboard focus: a ring around the focused hit of this frame.
fn focus_ring(f: &Frame, focus: Option<Act>) {
    let Some(h) = focus.and_then(|a| f.hits.iter().find(|h| h.act == a)) else {
        return;
    };
    let r = Rect::new(h.rect.x - 3.0, h.rect.y - 3.0, h.rect.w + 6.0, h.rect.h + 6.0);
    // Text color, so it differs from the accent border of a selected tile.
    f.g.stroke(r, 10.0, 2.0, th().text);
}

/// Right side of the title row: offline note on pages that configure the
/// mouse, otherwise whether the last change reached it.
fn header_status(f: &mut Frame, v: &View) {
    const H: f32 = 28.0;
    const SIZE: f32 = 12.5;
    const MAX_W: f32 = 380.0;
    let offline = v.page.configures() && !v.connected;
    let (text, dot, alpha) = match v.sync {
        _ if offline => (tr(T::OfflineBanner), th().warn, 255),
        Sync::Waiting => (tr(T::SyncWaiting), th().faint, 255),
        Sync::Saved(a) => (tr(T::SyncSaved), th().good, a),
        Sync::Hidden => return,
    };
    let w = (text_w(text, SIZE) + 52.0).min(MAX_W);
    let r = Rect::new(X0 + CW - w, 22.0, w, H);
    f.g.fill(r, H / 2.0, Color(th().card.0, alpha));
    f.g.stroke(r, H / 2.0, 1.0, Color(th().border.0, alpha));
    f.g.circle(r.x + 15.0, r.y + H / 2.0, 4.0, Color(dot.0, alpha));
    let label = Rect::new(r.x + 26.0, r.y, r.w - 34.0, H);
    f.g.text(text, label, fit_size(text, label.w, SIZE), Weight::Normal, Align::Left, Color(th().muted.0, alpha));
}

/// Bottom message with an optional "Undo".
fn toast(f: &mut Frame, t: &Toast) {
    const W: f32 = 420.0;
    const H: f32 = 44.0;
    let r = Rect::new(X0 + (CW - W) / 2.0, HEIGHT - H - 16.0, W, H);
    f.g.fill(r, 10.0, th().card_hi);
    f.g.stroke(r, 10.0, 1.0, th().faint);

    let undo_w = if t.undo { text_w(tr(T::UndoBtn), 14.0) + 32.0 } else { 0.0 };
    let label = Rect::new(r.x + 18.0, r.y, r.w - 36.0 - undo_w, r.h);
    f.g.text(&t.text, label, fit_size(&t.text, label.w, 13.5), Weight::Normal, Align::Left, th().text);
    if !t.undo {
        return;
    }
    let b = Rect::new(r.x + r.w - undo_w - 8.0, r.y + 6.0, undo_w, r.h - 12.0);
    let hot = f.button(Act::Undo, b);
    f.g.fill(b, 8.0, if hot { th().hover } else { th().card_hi });
    f.g.text(tr(T::UndoBtn), b, 14.0, Weight::Semibold, Align::Center, th().accent);
}

// ---- icons (24 x 24 grid scaled into `s` px at x, y) -----------------------

fn icon(g: &Gfx, page: Page, x: f32, y: f32, c: Color) {
    let s = 18.0 / 24.0;
    let p = |a: f32, b: f32| (x + a * s, y + b * s);
    let w = 1.8;

    match page {
        Page::Overview => {
            for (a, b) in [(4.0, 4.0), (13.0, 4.0), (4.0, 13.0), (13.0, 13.0)] {
                let (px, py) = p(a, b);
                g.stroke(Rect::new(px, py, 7.0 * s, 7.0 * s), 1.0, w, c);
            }
        }
        Page::Dpi => {
            let (cx, cy) = p(12.0, 12.0);
            g.ring(cx, cy, 4.0 * s, w, c);
            for (a, b, d, e) in [(12.0, 2.0, 12.0, 6.0), (12.0, 18.0, 12.0, 22.0), (2.0, 12.0, 6.0, 12.0), (18.0, 12.0, 22.0, 12.0)] {
                let ((x0, y0), (x1, y1)) = (p(a, b), p(d, e));
                g.line(x0, y0, x1, y1, w, c);
            }
        }
        Page::Polling => {
            let pts: Vec<_> = [(2.0, 12.0), (7.0, 12.0), (9.0, 6.0), (13.0, 18.0), (15.0, 12.0), (22.0, 12.0)].iter().map(|&(a, b)| p(a, b)).collect();
            g.polyline(&pts, w, c);
        }
        Page::Lighting => {
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
        Page::Buttons => {
            // Mouse outline: buttons.
            let (x0, y0) = p(7.0, 3.0);
            g.stroke(Rect::new(x0, y0, 10.0 * s, 18.0 * s), 5.0 * s, w, c);
            let ((lx, ly0), (_, ly1)) = (p(12.0, 4.0), p(12.0, 10.0));
            g.line(lx, ly0, lx, ly1, w, c);
        }
        Page::Stats => {
            for (a, top) in [(5.0, 11.0), (12.0, 5.0), (19.0, 14.0)] {
                let ((x0, y0), (_, y1)) = (p(a, top), p(a, 20.0));
                g.line(x0, y0, x0, y1, w + 0.4, c);
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

/// Circled "i" (About), on the same 18 px grid as `icon`.
fn info_icon(g: &Gfx, x: f32, y: f32, c: Color) {
    const S: f32 = 18.0 / 24.0;
    let (cx, cy) = (x + 12.0 * S, y + 12.0 * S);
    g.ring(cx, cy, 9.0 * S, 1.8, c);
    g.circle(cx, cy - 4.0 * S, 1.3, c);
    g.line(cx, cy - 1.0 * S, cx, cy + 5.0 * S, 1.8, c);
}

/// The app's mouse with whiskers, matching the exe icon.
fn mouse_logo(g: &Gfx, x: f32, y: f32) {
    g.stroke(Rect::new(x, y, 18.0, 27.0), 9.0, 2.0, th().accent);
    g.line(x + 9.0, y + 1.0, x + 9.0, y + 10.0, 2.0, th().accent);

    // Pink nose on top.
    const NOSE: Color = Color::rgb(0xff8fa8);
    g.circle(x + 9.0, y + 0.5, 3.0, NOSE);

    // Three short whiskers per side, from beside the nose, slanting up and out.
    const LEN: f32 = 6.0;
    let (cx, wy) = (x + 9.0, y + 2.5);
    for side in [-1.0, 1.0] {
        for (oy, inset, rise) in [(0.0, 0.0, 2.6), (1.8, 0.6, 1.5), (3.6, 1.4, 0.5)] {
            let x0 = cx + side * (6.5 + inset);
            g.line(x0, wy + oy, x0 + side * LEN, wy + oy - rise, 1.1, th().text);
        }
    }
}

// ---- sidebar -------------------------------------------------------------

fn sidebar(f: &mut Frame, v: &View) {
    f.g.fill(Rect::new(0.0, 0.0, SIDE_W, HEIGHT), 0.0, th().side);

    const BOTTOM: f32 = HEIGHT - 14.0;
    const FOOTER_H: f32 = 64.0;
    const ITEM_H: f32 = NAV_STEP - 4.0;

    // Logo and name open About; the mouse name opens its details.
    let brand = Rect::new(14.0, 12.0, SIDE_W - 28.0, 28.0);
    let hot = f.button(Act::About, brand);
    mouse_logo(f.g, 24.0, 20.0);
    let name_c = if hot { th().accent } else { th().text };
    f.g.text(crate::APP_NAME, Rect::new(62.0, 16.0, 140.0, 24.0), 18.0, Weight::Bold, Align::Left, name_c);
    let sub = Rect::new(62.0, 38.0, SIDE_W - 76.0, 16.0);
    let hot = f.button(Act::Nav(Page::Mouse), Rect::new(sub.x, sub.y - 2.0, sub.w, sub.h + 4.0));
    let sub_c = if hot { th().accent } else { th().faint };
    f.g.text(&v.device_name, sub, fit_size(&v.device_name, sub.w, 11.0), Weight::Normal, Align::Left, sub_c);

    for (k, page) in nav_pages(&v.caps).into_iter().enumerate() {
        let r = Rect::new(14.0, NAV_TOP + k as f32 * NAV_STEP, SIDE_W - 28.0, NAV_STEP - 4.0);
        nav_item(f, Nav::Page(page), r, Mark::of(v.page.nav() == page));
    }

    // Pinned at the bottom: Settings, then About right under it.
    let about = Rect::new(14.0, BOTTOM - ITEM_H, SIDE_W - 28.0, ITEM_H);
    nav_item(f, Nav::About, about, Mark::of(v.dialog == Some(Dialog::About)));
    let r = Rect::new(14.0, about.y - NAV_STEP, SIDE_W - 28.0, ITEM_H);
    nav_item(f, Nav::Page(Page::General), r, Mark::of(v.page.nav() == Page::General));

    let footer = v.caps.battery || v.has_battery;
    let footer_y = r.y - 8.0 - FOOTER_H;

    // Connection + battery summary (mice with a driver or a Bluetooth battery).
    if !footer {
        return;
    }
    let card = Rect::new(14.0, footer_y, SIDE_W - 28.0, FOOTER_H);
    let hot = f.button(Act::Nav(Page::Mouse), card);
    f.g.fill(card, 10.0, if hot { th().hover } else { th().card });
    f.g.circle(card.x + 16.0, card.y + 20.0, 4.0, if v.connected { th().good } else { th().faint });
    f.g.text(&v.link, Rect::new(card.x + 28.0, card.y + 10.0, card.w - 34.0, 20.0), 12.0, Weight::Normal, Align::Left, th().muted);

    if v.connected && v.has_battery {
        f.bar(Rect::new(card.x + 12.0, card.y + 41.0, card.w - 70.0, 6.0), v.battery, battery_color(v));
        let pct = Rect::new(card.x + card.w - 52.0, card.y + 34.0, 40.0, 20.0);
        f.g.text(&format!("{} %", v.battery), pct, 12.0, Weight::Semibold, Align::Right, th().text);
    }
}

/// Sidebar entry: a page, or About (opens its dialog).
#[derive(Clone, Copy)]
enum Nav {
    Page(Page),
    About,
}

/// Sidebar entry: icon, name; the current one gets a tint and a left bar.
fn nav_item(f: &mut Frame, item: Nav, r: Rect, mark: Mark) {
    let (act, label) = match item {
        Nav::Page(p) => (Act::Nav(p), tr(p.title())),
        Nav::About => (Act::About, tr(T::About)),
    };
    let sel = mark == Mark::Current;
    let hot = f.button(act, r);
    if sel || hot {
        f.g.fill(r, 8.0, if sel { th().accent_soft } else { th().hover });
    }
    if sel {
        f.g.fill(Rect::new(r.x, r.y + 7.0, 3.0, r.h - 14.0), 1.5, th().accent);
    }
    let c = if sel { th().accent } else { th().muted };
    match item {
        Nav::Page(p) => icon(f.g, p, r.x + 12.0, r.y + 9.0, c),
        Nav::About => info_icon(f.g, r.x + 12.0, r.y + 9.0, c),
    }
    let text_r = Rect::new(r.x + 42.0, r.y, r.w - 42.0, r.h);
    let weight = if sel { Weight::Semibold } else { Weight::Normal };
    f.g.text(label, text_r, 14.0, weight, Align::Left, if sel { th().text } else { th().muted });
}

// ---- pages ---------------------------------------------------------------

fn stat_card(f: &mut Frame, r: Rect, label: &str, value: &str, note: &str) {
    f.card(r);
    f.caption_in(label, r.x + 16.0, r.y + 16.0, r.w - 32.0);
    f.g.text(value, Rect::new(r.x + 16.0, r.y + 34.0, r.w - 32.0, 36.0), 26.0, Weight::Bold, Align::Left, th().text);
    f.g.text(note, Rect::new(r.x + 16.0, r.y + 74.0, r.w - 32.0, 32.0), 12.0, Weight::Normal, Align::Wrap, th().muted);
}

fn overview(f: &mut Frame, v: &View) {
    if !v.caps.battery {
        overview_basic(f, v);
        return;
    }
    let tint = battery_color(v);

    let card = Rect::new(X0, TOP, CW, 118.0);
    f.card(card);
    f.caption(tr(T::Battery), card.x + 20.0, card.y + 22.0);
    let shown = v.connected && v.has_battery;
    let big = if shown { format!("{} %", v.battery) } else { "—".into() };
    let big_c = if shown { tint } else { th().faint };
    f.g.text(&big, Rect::new(card.x + 20.0, card.y + 40.0, 170.0, 56.0), 44.0, Weight::Bold, Align::Left, big_c);

    let note = match v.charge {
        _ if !shown => v.link.as_str(),
        Charge::Charging => tr(T::Charging),
        Charge::Full => tr(T::FullyCharged),
        Charge::Discharging => v.battery_note.as_str(),
    };
    f.bar(Rect::new(card.x + 200.0, card.y + 46.0, card.w - 220.0, 10.0), if shown { v.battery } else { 0 }, tint);
    f.g.text(note, Rect::new(card.x + 200.0, card.y + 64.0, card.w - 220.0, 22.0), 13.0, Weight::Normal, Align::Left, th().muted);

    let w = (CW - 2.0 * GAP) / 3.0;
    let y = TOP + 118.0 + GAP;
    let stages = v.caps.dpi == DpiUi::Stages;
    let (dpi, note) = match v.stages.get(v.active) {
        _ if !stages => {
            let stage_note = match (v.basic.stage, v.basic.stages) {
                (Some(s), Some(n)) => fill(tr(T::StageOf), &[&s.to_string(), &n.to_string()]),
                _ => String::new(),
            };
            match (v.basic.dpi, v.basic.stage, v.basic.stages) {
                (Some(d), ..) => (d.to_string(), stage_note),
                (None, Some(s), Some(n)) => (format!("{s} / {n}"), stage_note),
                _ => ("—".into(), String::new()),
            }
        }
        Some(s) => (s.dpi.to_string(), fill(tr(T::StageOf), &[&(v.active + 1).to_string(), &v.stages.iter().filter(|s| s.enabled).count().to_string()])),
        None => ("—".into(), String::new()),
    };
    stat_card(f, Rect::new(X0, y, w, 110.0), tr(T::ActiveDpi), &dpi, &note);
    let hz = match stages {
        true => format!("{} Hz", RATES[v.polling]),
        false => v.basic.rate.map_or("—".into(), |r| format!("{r} Hz")),
    };
    if v.caps.polling {
        // Only a real measurement is worth a line.
        let note = v.measured.map(|hz| fill(tr(T::MeasuredValue), &[&format!("{hz} Hz")])).unwrap_or_default();
        stat_card(f, Rect::new(X0 + w + GAP, y, w, 110.0), tr(T::PollingRate), &hz, &note);
    } else {
        // The mouse doesn't report its rate: show what Raw Input measures.
        stat_card(f, Rect::new(X0 + w + GAP, y, w, 110.0), tr(T::Measured), &measured(v), tr(T::PollingRate));
    }
    let today = fill(tr(T::ClicksMoved), &[&v.today_distance]);
    stat_card(f, Rect::new(X0 + 2.0 * (w + GAP), y, w, 110.0), tr(T::Today), &v.today_clicks, &today);

    // Quick DPI switch: one chip per enabled stage.
    let mut y = y + 110.0 + GAP;
    if stages {
        let card = Rect::new(X0, y, CW, 96.0);
        f.card(card);
        f.caption(tr(T::QuickDpi), card.x + 20.0, card.y + 16.0);
        let mut x = card.x + 20.0;
        for (i, s) in v.stages.iter().enumerate().filter(|(_, s)| s.enabled) {
            f.choice(Act::Activate(i), Rect::new(x, card.y + 40.0, 80.0, 38.0), &s.dpi.to_string(), "", i == v.active);
            x += 88.0;
        }
        y += card.h + GAP;
    }
    device_line(f, v, y);
}

fn measured(v: &View) -> String {
    v.measured.map_or("—".into(), |hz| format!("{hz} Hz"))
}

/// Compact mouse line, e.g. "White Shark Lionel · FW 1.77      Details ›";
/// opens Settings › Mouse.
fn device_line(f: &mut Frame, v: &View, y: f32) {
    const H: f32 = 40.0;
    let r = Rect::new(X0, y, CW, H);
    let hot = f.button(Act::Nav(Page::Mouse), r);
    f.g.fill(r, RADIUS, if hot { th().hover } else { th().card });
    f.g.stroke(r, RADIUS, 1.0, th().border);

    let fw = &v.device.firmware;
    let text = if fw.is_empty() { v.device_name.clone() } else { format!("{} · FW {fw}", v.device_name) };
    let details = format!("{} ›", tr(T::Details));
    let link_w = text_w(&details, 13.0) + 20.0;
    let left = Rect::new(r.x + 18.0, r.y, r.w - 36.0 - link_w, H);
    f.g.text(&text, left, fit_size(&text, left.w, 13.0), Weight::Normal, Align::Left, th().muted);
    let right = Rect::new(r.x + r.w - 18.0 - link_w, r.y, link_w, H);
    f.g.text(&details, right, 13.0, Weight::Semibold, Align::Right, if hot { th().accent_hover } else { th().accent });
}

/// Overview for a mouse without a driver: model and today's statistics.
fn overview_basic(f: &mut Frame, v: &View) {
    let card = Rect::new(X0, TOP, CW, 96.0);
    f.card(card);
    f.caption(tr(T::DevModel), card.x + 20.0, card.y + 20.0);
    f.g.text(&v.device_name, Rect::new(card.x + 20.0, card.y + 40.0, card.w - 240.0, 36.0), 24.0, Weight::Bold, Align::Left, th().text);

    // Bluetooth battery, when Windows reports one.
    if v.connected && v.has_battery {
        let tint = battery_color(v);
        f.caption(tr(T::Battery), card.x + card.w - 200.0, card.y + 20.0);
        let pct = Rect::new(card.x + card.w - 200.0, card.y + 36.0, 180.0, 30.0);
        f.g.text(&format!("{} %", v.battery), pct, 22.0, Weight::Bold, Align::Left, tint);
        f.bar(Rect::new(card.x + card.w - 200.0, card.y + 70.0, 180.0, 8.0), v.battery, tint);
    }

    let w = (CW - GAP) / 2.0;
    let y = card.y + card.h + GAP;
    let today = fill(tr(T::ClicksMoved), &[&v.today_distance]);
    stat_card(f, Rect::new(X0, y, w, 110.0), tr(T::Today), &v.today_clicks, &today);
    stat_card(f, Rect::new(X0 + w + GAP, y, w, 110.0), tr(T::Measured), &measured(v), tr(T::PollingRate));
    device_line(f, v, y + 110.0 + GAP);
}

/// One DPI value for driver-based mice: big number and a slider.
fn dpi_single(f: &mut Frame, v: &View) {
    let card = Rect::new(X0, TOP, CW, 140.0);
    f.card(card);
    f.caption(tr(T::ActiveDpi), card.x + 20.0, card.y + 18.0);
    let value = v.basic.dpi.map_or("—".into(), |d| format!("{d} DPI"));
    f.g.text(&value, Rect::new(card.x + 20.0, card.y + 36.0, card.w - 40.0, 44.0), 32.0, Weight::Bold, Align::Left, th().text);

    let vals = &v.basic.values;
    if vals.len() < 2 {
        return;
    }
    let (x0, x1, cy) = (card.x + 30.0, card.x + card.w - 30.0, card.y + 106.0);
    let idx = v.basic.dpi.and_then(|d| vals.iter().position(|&x| x >= d)).unwrap_or(0);
    let kx = x0 + (x1 - x0) * idx as f32 / (vals.len() - 1) as f32;
    f.g.fill(Rect::new(x0, cy - 2.0, x1 - x0, 4.0), 2.0, th().border);
    f.g.fill(Rect::new(x0, cy - 2.0, kx - x0, 4.0), 2.0, th().accent);
    f.g.circle(kx, cy, 9.0, th().card_hi);
    f.g.circle(kx, cy, 6.5, th().accent);
    let area = Rect::new(x0 - 10.0, cy - 16.0, x1 - x0 + 20.0, 32.0);
    f.hits.push(Hit { act: Act::BasicDpi, rect: area, kind: Kind::Slider { x0, x1, steps: vals.len() } });

    let ends = Rect::new(x0, cy + 10.0, x1 - x0, 18.0);
    f.g.text(&vals[0].to_string(), ends, 11.0, Weight::Normal, Align::Left, th().faint);
    f.g.text(&vals[vals.len() - 1].to_string(), ends, 11.0, Weight::Normal, Align::Right, th().faint);
}

fn dpi(f: &mut Frame, v: &View) {
    if v.caps.dpi == DpiUi::Single {
        dpi_single(f, v);
        return;
    }
    const ROW: f32 = 52.0;
    const HEAD: f32 = 34.0;
    // Column x offsets: active radio, cycle switch, color dot, slider.
    const COL_CYCLE: f32 = 72.0;
    const COL_COLOR: f32 = 132.0;
    const COL_DPI: f32 = 184.0;
    let card = Rect::new(X0, TOP, CW, HEAD + ROW * v.stages.len() as f32 + 8.0);
    f.card(card);

    // Column headers instead of help sentences.
    let hy = card.y + 14.0;
    f.caption_in(tr(T::ColActive), card.x + 16.0, hy, COL_CYCLE - 20.0);
    f.caption_in(tr(T::ColCycle), card.x + COL_CYCLE, hy, COL_COLOR - COL_CYCLE - 4.0);
    f.caption_in(tr(T::ColorCaption), card.x + COL_COLOR, hy, COL_DPI - COL_COLOR - 4.0);
    f.caption_in("DPI", card.x + COL_DPI, hy, 60.0);

    for (i, s) in v.stages.iter().enumerate() {
        let y = card.y + HEAD + i as f32 * ROW;
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

        // The active stage can't be switched off (note under the card).
        let toggle_act = (!active).then_some(Act::Toggle(i));
        f.toggle(toggle_act, card.x + COL_CYCLE, cy - 11.0, s.enabled);

        // Indicator color; click cycles through the palette.
        let (dx, dot) = (card.x + COL_COLOR + 12.0, Color::rgb(v.dpi_colors[i]));
        let hot = f.button(Act::DpiColor(i), Rect::new(dx - 12.0, cy - 12.0, 24.0, 24.0));
        if hot {
            f.g.ring(dx, cy, 10.0, 1.5, th().muted);
        }
        f.g.circle(dx, cy, 7.0, dot);
        f.g.ring(dx, cy, 7.0, 1.0, th().border);

        // Slider.
        let (x0, x1) = (card.x + COL_DPI + 8.0, card.x + card.w - 120.0);
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

    // Why the active stage's switch is greyed out.
    let r = Rect::new(X0, card.y + card.h + 12.0, CW, 36.0);
    f.g.text(tr(T::ActiveLocked), r, 12.0, Weight::Normal, Align::Wrap, th().muted);
}

fn performance(f: &mut Frame, v: &View) {
    const TILE_Y: f32 = TOP + 22.0;
    const TILE_H: f32 = 56.0;
    f.caption(tr(T::PollingRate), X0, TOP);
    if v.caps.extras {
        let w = (CW - 3.0 * 12.0) / 4.0;
        for (i, hz) in RATES.iter().enumerate() {
            let r = Rect::new(X0 + i as f32 * (w + 12.0), TILE_Y, w, TILE_H);
            f.choice(Act::Polling(i), r, &format!("{hz} Hz"), &format!("{} ms", 1000 / hz), v.polling == i);
        }
    } else {
        // Rates the mouse reported, e.g. 125 / 250 / 500 / 1000 Hz.
        let n = v.basic.rates.len().max(1) as f32;
        let w = (CW - (n - 1.0) * 12.0) / n;
        for (i, hz) in v.basic.rates.iter().enumerate() {
            let r = Rect::new(X0 + i as f32 * (w + 12.0), TILE_Y, w, TILE_H);
            let ms = format!("{:.1} ms", 1000.0 / *hz as f32).replace(".0 ", " ");
            f.choice(Act::BasicRate(*hz), r, &format!("{hz} Hz"), &ms, v.basic.rate == Some(*hz));
        }
    }

    // Measured rate: one line, then how to measure it.
    let y = TILE_Y + TILE_H + 10.0;
    let line = fill(tr(T::MeasuredValue), &[&measured(v)]);
    f.g.text(&line, Rect::new(X0, y, CW, 20.0), 13.5, Weight::Semibold, Align::Left, th().text);
    let hint = Rect::new(X0, y + 22.0, CW, 32.0);
    f.g.text(tr(T::PollingHelp), hint, 12.0, Weight::Normal, Align::Wrap, th().muted);

    if !v.caps.extras {
        return;
    }

    // Debounce.
    let card = Rect::new(X0, hint.y + hint.h + GAP, CW, 112.0);
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
    // The chart follows the selected period.
    let title = [T::ChartToday, T::ChartTitle, T::ChartAll][v.period.min(PERIODS.len() - 1)];
    f.caption(tr(title), card.x + 16.0, card.y + 14.0);

    let plot = Rect::new(card.x + 16.0, card.y + 40.0, card.w - 64.0, card.h - 72.0);
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

    // Time axis: a tick and a label per day (hours for "today").
    const LABEL_W: f32 = 60.0;
    for (t, label) in &v.chart_ticks {
        let x = plot.x + t * plot.w;
        f.g.line(x, plot.y + plot.h, x, plot.y + plot.h + 4.0, 1.0, th().border);
        let lx = (x - LABEL_W / 2.0).clamp(plot.x - 8.0, plot.x + plot.w + 8.0 - LABEL_W);
        let r = Rect::new(lx, plot.y + plot.h + 6.0, LABEL_W, 16.0);
        f.g.text(label, r, 10.5, Weight::Normal, Align::Center, th().faint);
    }

    let pts: Vec<(f32, f32)> = v.chart.iter().map(|&(t, pct)| (plot.x + t * plot.w, plot.y + plot.h * (1.0 - pct / 100.0))).collect();
    f.g.polyline(&pts, 2.0, th().accent);
}

fn settings(f: &mut Frame, v: &View) {
    let card = Rect::new(X0, BODY, CW, 104.0);
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

    let card = Rect::new(X0, card.y + card.h + GAP, CW, 76.0);
    f.card(card);
    let title = Rect::new(card.x + 18.0, card.y + 16.0, 400.0, 22.0);
    f.g.text(tr(T::Autostart), title, 15.0, Weight::Semibold, Align::Left, th().text);
    let desc = Rect::new(card.x + 18.0, card.y + 40.0, 420.0, 20.0);
    f.g.text(tr(T::AutostartDesc), desc, 12.0, Weight::Normal, Align::Left, th().muted);
    // Autostart only points at the installed copy.
    let act = v.installed.then_some(Act::Autostart);
    f.toggle(act, card.x + card.w - 58.0, card.y + 27.0, v.autostart);

    // Tray behaviour and where data lives (About has its own dialog).
    let card = Rect::new(X0, card.y + card.h + GAP, CW, 88.0);
    f.card(card);
    let r = Rect::new(card.x + 18.0, card.y + 14.0, card.w - 36.0, 34.0);
    f.g.text(tr(T::AboutTray), r, 12.0, Weight::Normal, Align::Wrap, th().muted);
    let r = Rect::new(card.x + 18.0, card.y + 58.0, card.w - 36.0, 18.0);
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

/// About: logo, name and version, copyright, license, project links.
fn about_dialog(f: &mut Frame) {
    const W: f32 = 480.0;
    const H: f32 = 244.0;
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    let d = Rect::new((WIDTH - W) / 2.0, (HEIGHT - H) / 2.0, W, H);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let inner = d.inset(24.0);
    f.caption(tr(T::About), inner.x, inner.y);
    mouse_logo(f.g, inner.x + 4.0, inner.y + 34.0);
    let name = Rect::new(inner.x + 40.0, inner.y + 28.0, inner.w - 40.0, 30.0);
    f.g.text(crate::APP_NAME, name, 22.0, Weight::Bold, Align::Left, th().text);
    let ver = Rect::new(name.x + text_w(crate::APP_NAME, 22.0) + 10.0, name.y + 4.0, 120.0, 26.0);
    f.g.text(VERSION, ver, 15.0, Weight::Normal, Align::Left, th().muted);
    let tag = Rect::new(name.x, name.y + 30.0, name.w, 18.0);
    f.g.text(tr(T::AboutTagline), tag, 12.5, Weight::Normal, Align::Left, th().muted);

    let y = inner.y + 98.0;
    f.g.text(COPYRIGHT, Rect::new(inner.x, y, inner.w, 18.0), 12.5, Weight::Normal, Align::Left, th().text);
    let license = fill(tr(T::License), &[LICENSE]);
    f.g.text(&license, Rect::new(inner.x, y + 20.0, inner.w, 18.0), 12.5, Weight::Normal, Align::Left, th().text);

    let by = d.y + d.h - 24.0 - 36.0;
    let report = tr(T::ReportMouse);
    let report_w = text_w(report, 14.0) + 32.0;
    f.push_button(Act::OpenWeb(Web::Repo), Rect::new(inner.x, by, 100.0, 36.0), "GitHub", false);
    f.push_button(Act::OpenWeb(Web::NewIssue), Rect::new(inner.x + 110.0, by, report_w, 36.0), report, false);
    f.push_button(Act::CloseDialog, Rect::new(d.x + d.w - 24.0 - 110.0, by, 110.0, 36.0), tr(T::Close), true);
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
    let card = Rect::new(X0, BODY, CW, 44.0 + rows * (TILE_H + CELL_GAP) + 6.0);
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
