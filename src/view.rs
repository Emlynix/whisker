//! Screens. `draw` paints the whole UI from a `View` snapshot and returns
//! the clickable areas (`Hit`) of that frame; the window layer turns mouse
//! input on them into `Act`s for the controller. Immediate mode: no widget
//! objects, a frame is just a function of state.
//!
//! ```text
//!  +--------------------------------------------------------+
//!  | [logo] WHISKER [mouse name]                    [–] [×] |  title bar 32, drags the window
//!  +-----------+--------------------------------------------+
//!  | nav x6    |  PAGE TITLE               [status chip]    |
//!  | ///////// |  Tab | Tab   (Buttons, Settings only)      |
//!  | [PRO]     |  page content (panels)                     |
//!  | Profiles  |                                            |
//!  | Overlay … |              [toast · Undo]                |
//!  | Settings  |                                            |
//!  +-----------+--------------------------------------------+
//!  | ■ Connected · 78 %                    Update available |  status bar 24
//!  +--------------------------------------------------------+
//!    176 px          content x = 194 .. 782
//! ```

use crate::gfx::{Align, Color, Face, Gfx, Rect, Seg, Size, Weight};
use crate::i18n::{Lang, T, fill, tr};
use crate::protocol::{
    Action, BRIGHTNESS_MAX, BUTTON_COUNT, DPI_COLOR_SLOTS, Direction, Effect, Lighting, Lod, MacroEvent, MacroMode, Power,
    SPEED_MAX,
};

mod art;
mod buttons;
mod device;
mod mesh;
mod pro;
mod report;
mod tui;
mod update;
pub use buttons::{DelayMode, Picker, Tab, button_name};
pub use device::{Badge, Banner, BrandRow, Detect, DetectRow, DetectStep, DeviceView, Found, LIST_ROWS, Pane, detect_pane_at};
pub use report::{Answer, Ask, CheckView, Kind as ReportKind, PickRow, REPORT_ROWS, ReportView, Sending, Step as ReportStep, report_at, report_lines};
pub use tui::page_of_key;
pub use update::{Install, Offer, Probe, Step as UpdateStep, UpdateView, note_lines, notes_cut};
use crate::devices::{Caps, DpiUi};
use crate::platform::Access;
use crate::store::{DEBOUNCE_CHOICES, SLEEP_CHOICES};
use crate::theme::{self, THEMES, Theme};

fn th() -> &'static Theme {
    theme::current()
}

pub const WIDTH: f32 = 800.0;
pub const HEIGHT: f32 = 600.0;
/// Custom title bar: logo, app name, mouse name, window buttons.
const TITLE_H: f32 = 32.0;
/// Status bar across the bottom of the window.
const STATUS_H: f32 = 24.0;
/// Bottom of page content; the status bar sits below.
const PAGE_H: f32 = HEIGHT - STATUS_H;

const SIDE_W: f32 = 176.0;
/// Content padding: 18 left and right, 16 above the page title.
const CONTENT_PAD: f32 = 18.0;
const X0: f32 = SIDE_W + CONTENT_PAD;
const CW: f32 = WIDTH - SIDE_W - 2.0 * CONTENT_PAD;
/// Page title: 16 below the title bar, 24 tall, then its accent rule,
/// 18 above the content.
const TITLE_Y: f32 = TITLE_H + 16.0;
const TOP: f32 = TITLE_Y + 24.0 + 18.0;
/// Spacing on a 4 px grid (8 / 12 / 16 / 24).
const GAP: f32 = 16.0;
/// Panels: a 36 px header row, 12 px padding, 12 px between panels.
const PANEL_HEAD: f32 = 36.0;
const PANEL_PAD: f32 = 12.0;
const PANEL_GAP: f32 = 12.0;
/// Height of a segmented control.
const SEGMENT_H: f32 = 30.0;

/// Setting rows: a 170 px label column, 16 px to the control column, 12 px
/// padding all around.
const LABEL_W: f32 = 170.0;
const ROW_PAD: f32 = 12.0;
const ROW_GAP: f32 = 16.0;
/// Line height of a wrapped caption.
const CAPTION_LINE: f32 = 16.0;

/// One cell of a segmented control.
struct Segment {
    act: Act,
    label: String,
    /// Second, smaller line; empty for a one-line cell.
    sub: String,
    selected: bool,
    /// Values are set in mono; names ("Breathing") in the text face.
    kind: Label,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Label {
    Value,
    Name,
}

impl Segment {
    fn new(act: Act, label: impl Into<String>, selected: bool) -> Segment {
        Segment { act, label: label.into(), sub: String::new(), selected, kind: Label::Value }
    }

    fn named(act: Act, label: impl Into<String>, selected: bool) -> Segment {
        Segment { kind: Label::Name, ..Segment::new(act, label, selected) }
    }
}

/// The main pattern of a settings page: a row with its name and a short
/// description on the left and the control on the right.
///
/// ```text
///  | Debounce            | [4 ms][6 ms][8 ms][12 ms]…  |
///  | Filters double      |                             |
///  | clicks.             |                             |
/// ```
struct SettingRow<'a> {
    title: &'a str,
    desc: &'a str,
    /// Height of the control itself.
    control_h: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowPlace {
    First,
    Next,
}

impl SettingRow<'_> {
    /// Lines the description takes when wrapped to `width`; a little
    /// generous, the width of text is an estimate.
    fn lines(&self, width: f32) -> f32 {
        const SAFETY: f32 = 1.15;
        (text_w(self.desc, Size::Caption) * SAFETY / width).ceil().max(1.0)
    }

    fn control_w(w: f32) -> f32 {
        w - 2.0 * ROW_PAD - LABEL_W - ROW_GAP
    }

    /// More than two lines in the label column: it goes under the control.
    fn desc_under(&self) -> bool {
        const LABEL_LINES_MAX: f32 = 2.0;
        !self.desc.is_empty() && self.lines(LABEL_W) > LABEL_LINES_MAX
    }

    fn height(&self, w: f32) -> f32 {
        let (mut label, mut control) = (18.0, self.control_h);
        if self.desc_under() {
            control += 6.0 + self.lines(Self::control_w(w)) * CAPTION_LINE;
        } else if !self.desc.is_empty() {
            label += 3.0 + self.lines(LABEL_W) * CAPTION_LINE;
        }
        2.0 * ROW_PAD + f32::max(label, control)
    }

    fn control(&self, x: f32, y: f32, w: f32) -> Rect {
        Rect::new(x + ROW_PAD + LABEL_W + ROW_GAP, y + ROW_PAD, Self::control_w(w), self.control_h)
    }
}

/// The "mecha" look: flat cuts and short accent bars on top of the sharp
/// base. These are all of them; nothing else is decorated.
///
/// ```text
///   ▔▔                    ╲     panel: accent tab on the top edge, the
///  ┌────────────────────────╲   top-right and bottom-left corners cut
///  │                        │
///  ╲                        │   selected segment: top-right corner cut
///   ╲───────────────────────┘   primary button: bottom-right corner cut
/// ```
const PANEL_CUT: f32 = 10.0;
const ACCENT_TAB: Rect = Rect::new(14.0, 0.0, 28.0, 2.0);
const SEGMENT_CUT: f32 = 7.0;
const BUTTON_CUT: f32 = 7.0;
/// Selected nav item: its right edge leans in by this much at the bottom.
const NAV_SLANT: f32 = 12.0;
/// Accent bar under the page title.
const TITLE_RULE: Rect = Rect::new(0.0, 4.0, 24.0, 2.0);
/// Battery meter: blocks of 6 px with 2 px between them.
const METER_H: f32 = 6.0;
const METER_BLOCK: f32 = 6.0;
const METER_GAP: f32 = 2.0;

/// Which corners of a box are cut off.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cut {
    /// Top-right and bottom-left: panels.
    Panel,
    TopRight,
    BottomRight,
}

/// Outline of `r` with corners cut by `size`, clockwise from the top-left.
fn cut_box(r: Rect, cut: Cut, size: f32) -> Vec<(f32, f32)> {
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    let mut pts = vec![(x0, y0)];
    match cut {
        Cut::Panel | Cut::TopRight => pts.extend([(x1 - size, y0), (x1, y0 + size)]),
        Cut::BottomRight => pts.push((x1, y0)),
    }
    match cut {
        Cut::BottomRight => pts.extend([(x1, y1 - size), (x1 - size, y1)]),
        _ => pts.push((x1, y1)),
    }
    match cut {
        Cut::Panel => pts.extend([(x0 + size, y1), (x0, y1 - size)]),
        _ => pts.push((x0, y1)),
    }
    pts
}

/// Closed path through `pts`.
fn polygon(pts: &[(f32, f32)]) -> Vec<Seg> {
    let mut segs: Vec<Seg> = pts.iter().enumerate().map(|(k, &(x, y))| if k == 0 { Seg::Move(x, y) } else { Seg::Line(x, y) }).collect();
    segs.push(Seg::Close);
    segs
}

/// Fills a cut box.
fn fill_cut(g: &Gfx, r: Rect, cut: Cut, size: f32, c: Color) {
    g.fill_path(&polygon(&cut_box(r, cut, size)), c);
}

/// 1 px outline of a cut box, inside `r`.
fn stroke_cut(g: &Gfx, r: Rect, cut: Cut, size: f32, c: Color) {
    // Half a pixel in: a 1 px line centred on the path lands on whole pixels.
    g.stroke_path(&polygon(&cut_box(r.inset(0.5), cut, size)), 1.0, c);
}

/// Segmented battery meter: filled blocks in `c`, the rest in the track color.
fn meter(g: &Gfx, r: Rect, pct: u8, c: Color) {
    let blocks = ((r.w + METER_GAP) / (METER_BLOCK + METER_GAP)).floor().max(1.0) as usize;
    let filled = (blocks as f32 * pct.min(100) as f32 / 100.0).round() as usize;
    for k in 0..blocks {
        let x = r.x + k as f32 * (METER_BLOCK + METER_GAP);
        g.fill(Rect::new(x, r.y, METER_BLOCK, METER_H), 0.0, if k < filled { c } else { th().track });
    }
}

/// Sharp and thin: no corner radius anywhere.
const R_INNER: f32 = 0.0;
const R_CTRL: f32 = 0.0;
const RADIUS: f32 = 0.0;

const LOW: u8 = 20;
const MID: u8 = 50;

/// Screens. Macros is a tab of Buttons; Appearance, Mouse and About are
/// tabs of Settings (`General`).
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
    About,
    /// Pages of the PRO version: mock-ups here (`view::pro`).
    Profiles,
    Overlay,
}

/// Sidebar order; Settings (`General`) is pinned at the bottom.
const NAV: [Page; 6] = [Page::Overview, Page::Dpi, Page::Polling, Page::Lighting, Page::Buttons, Page::Stats];
const BUTTON_TABS: [(Page, T); 2] = [(Page::Buttons, T::PageButtons), (Page::Macros, T::PageMacros)];
const SETTING_TABS: [(Page, T); 4] =
    [(Page::General, T::TabGeneral), (Page::Appearance, T::PageAppearance), (Page::Mouse, T::TabMouse), (Page::About, T::About)];

impl Page {
    /// Sidebar entry the page belongs to, e.g. Macros -> Buttons.
    pub fn nav(self) -> Page {
        match self {
            Page::Macros => Page::Buttons,
            Page::Appearance | Page::Mouse | Page::About => Page::General,
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
            Page::Profiles => T::PageProfiles,
            Page::Overlay => T::PageOverlay,
            _ => T::PageSettings,
        }
    }

    /// The page sends settings to the mouse (offline banner, sync chip).
    fn configures(self) -> bool {
        matches!(self, Page::Dpi | Page::Polling | Page::Lighting | Page::Buttons | Page::Macros)
    }
}

/// First nav item 8 below the title bar; items 32 tall, no gaps.
const NAV_TOP: f32 = TITLE_H + 8.0;
const NAV_STEP: f32 = 32.0;
/// Underlined tab row under the title; tabbed pages start below it.
const TAB_H: f32 = 34.0;
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
    /// DPI page: the active stage one step down (-1) or up (1).
    StageNudge(i8),
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
    /// Model row of the detect dialog (index into its models).
    DetectSel(usize),
    DetectBrand(usize),
    /// A detected mouse chip.
    DetectFound(usize),
    DetectUse,
    /// Closes the dialog; the current mouse stays.
    DetectCancel,
    /// From the lists back to the detected mice.
    DetectBack,
    /// From the detected mice to the brand and model lists.
    DetectList,
    /// Reverts the last destructive action (toast button, Ctrl+Z).
    Undo,
    About,
    /// Closes the About or "assign macro" dialog.
    CloseDialog,
    /// Switches between the graphical and the text mode.
    ToggleTui,
    /// Opens or closes the language list.
    LangMenu,
    /// Settings › Appearance › Effects.
    FxTexture,
    FxMotion,
    /// A control of a PRO mock-up: answers the pointer, does nothing.
    Mock(u8),
    /// Title bar: its empty part (moves the window) and the window buttons.
    Caption,
    WinMinimize,
    WinClose,
    /// Crash notice: opens the diagnostic log.
    ShowLog,
    OpenWeb(Web),
    Diagnostics,
    CopyDiagnostics,
    /// Copies the command installing the udev rule (Linux).
    CopyCommand,
    /// Opens the "assign macro to button" dialog.
    AssignMacro,
    /// Assigns the selected macro to this button.
    AssignTo(usize),
    /// New-mouse banner: use the new mouse.
    BannerSwitch,
    /// New-mouse banner: never ask for this mouse again.
    BannerDismiss,
    /// Opens "Help add your mouse".
    ReportOpen,
    /// A connected mouse in the report's pick list.
    ReportSel(usize),
    /// Probes the picked mouse (read-only) and shows the report.
    ReportRead,
    ReportCopy,
    /// Sends the text shown to the developers.
    ReportSend,
    /// The id of the sent report to the clipboard.
    ReportCopyId,
    ReportIssue,
    /// Starts the guided checks of an experimental driver.
    ReportCheck,
    ReportAnswer(Answer),
    /// Ends the checks; every changed setting is put back.
    ReportCancelChecks,
    /// Closes the wizard (checks cancelled and restored first).
    ReportClose,
    /// Opens the update dialog (status bar notice, About).
    UpdateShow,
    /// About: asks the release channel now.
    UpdateCheck,
    /// Settings: daily check on / off.
    UpdateAuto,
    /// Downloads, verifies and installs the offer, then restarts.
    UpdateInstall,
    UpdateLater,
    UpdateSkip,
    /// Opens the release page in the browser.
    UpdatePage,
}

/// Pages of the project site the About dialog links to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Web {
    Site,
    Repo,
}

/// Small modal dialogs (the bigger ones have their own state).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dialog {
    /// The last run crashed or was closed while hung.
    Crashed,
    AssignMacro,
    Update,
}

/// Header chip: settings on their way to the mouse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sync {
    Hidden,
    Waiting,
    /// Fading out; opacity 0..=255.
    Saved(u8),
}

/// Keyboard focus: a hit of the frame by position, since the same act
/// can sit in several places (logo and About item, mouse name and tab).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Focus {
    pub index: usize,
    pub act: Act,
}

impl Focus {
    /// Position of the focused hit among `acts`; `None` if it is gone.
    /// The same position if it still holds the act, else its first place.
    pub fn locate(self, acts: &[Act]) -> Option<usize> {
        if acts.get(self.index) == Some(&self.act) {
            return Some(self.index);
        }
        acts.iter().position(|a| *a == self.act)
    }
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
    /// Empty part of the title bar: dragging it moves the window.
    Caption,
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
    /// Coarse battery level: shown with "≈", no mid-level warning.
    pub battery_approx: bool,
    pub charge: Charge,
    pub battery_note: String,
    pub stages: Vec<StageView>,
    pub active: usize,
    pub steps: usize,
    pub polling: usize,
    /// Report rate measured from mouse input, while the mouse moves.
    pub measured: Option<u32>,
    /// Last rate measured, kept while the mouse rests (Overview card).
    pub last_rate: Option<u32>,
    pub today_clicks: String,
    pub today_distance: String,
    pub tiles: Vec<(&'static str, String)>,
    /// Battery history as (0..1 time, 0..100 %) points.
    pub chart: Vec<(f32, f32)>,
    /// X axis labels at 0..1, e.g. (0.5, "6.11.").
    pub chart_ticks: Vec<(f32, String)>,
    /// First day of a history shorter than the period, e.g. "8.11.".
    pub chart_since: Option<String>,
    pub autostart: bool,
    pub installed: bool,
    pub install_dir: String,
    pub error: String,
    pub data_dir: String,
    pub ask_setup: bool,
    pub setup_autostart: bool,
    /// Selected entries of the appearance pickers (0 = system).
    pub theme_pick: usize,
    /// Effects: background grid and texture, subtle animations.
    pub fx_texture: bool,
    pub fx_motion: bool,
    /// Opacity of the connection mark; it breathes while animations run.
    pub status_pulse: u8,
    /// Text mode: the window drawn as a terminal screen (`view::tui`).
    pub tui: bool,
    pub lang_pick: usize,
    /// The language list is open (Settings › General).
    pub lang_menu: bool,
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
    /// Keyboard focus ring (Tab).
    pub focus: Option<Focus>,
    pub toast: Option<Toast>,
    pub sync: Sync,
    pub dialog: Option<Dialog>,
    /// Diagnostics section of the Mouse tab is expanded.
    pub diagnostics: bool,
    /// Opacity of the pulsing recording dot.
    pub pulse: u8,
    /// "New mouse connected" offer over the title row.
    pub banner: Option<Banner>,
    /// The chosen mouse's devices refuse this user (Linux, no udev rule).
    pub access: Access,
    /// "Help add your mouse" wizard.
    pub report: Option<ReportView>,
    pub update: UpdateView,
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
        self.surface(r, th().panel);
    }

    /// Panel: a flat fill with a 1 px hairline border, two corners cut,
    /// and the accent tab on its top edge.
    fn surface(&self, r: Rect, c: Color) {
        fill_cut(self.g, r, Cut::Panel, PANEL_CUT, c);
        stroke_cut(self.g, r, Cut::Panel, PANEL_CUT, th().line);
        self.g.fill(Rect::new(r.x + ACCENT_TAB.x, r.y, ACCENT_TAB.w, ACCENT_TAB.h), 0.0, th().acc_fill);
    }

    /// Panel with a header row: the title on the left, a hairline under
    /// it. Returns the body below the header.
    fn panel(&mut self, r: Rect, title: &str) -> Rect {
        self.card(r);
        self.g.fill(Rect::new(r.x, r.y + PANEL_HEAD - 1.0, r.w, 1.0), 0.0, th().line);
        let label = Rect::new(r.x + PANEL_PAD, r.y, r.w - 2.0 * PANEL_PAD, PANEL_HEAD - 1.0);
        self.g.text(title, label, Size::Body, Weight::Semibold, Align::Left, th().text);
        Rect::new(r.x, r.y + PANEL_HEAD, r.w, r.h - PANEL_HEAD)
    }

    /// Caption on the right of a panel header.
    fn panel_note(&mut self, r: Rect, note: &str) {
        let label = Rect::new(r.x + r.w / 3.0, r.y, r.w * 2.0 / 3.0 - PANEL_PAD, PANEL_HEAD - 1.0);
        self.g.text(note, label, Size::Caption, Weight::Normal, Align::Right, th().muted);
    }

    /// Text link on the right of a panel header.
    fn panel_link(&mut self, r: Rect, act: Act, label: &str) {
        // Semibold runs wider than the estimate for regular text.
        const LINK_ROOM: f32 = 1.12;
        let w = text_w(label, Size::Caption) * LINK_ROOM;
        text_link(self, act, label, r.x + r.w - PANEL_PAD - w, r.y + (PANEL_HEAD - 18.0) / 2.0);
    }

    /// Segmented control: equal cells in one frame, a hairline between
    /// them. The selected one has a soft accent ground, a 1 px accent
    /// outline inside and accent text. Labels are values: mono. A cell
    /// may carry a second, smaller line ("500 Hz" over "2 ms").
    fn segments(&mut self, r: Rect, items: &[Segment]) {
        if items.is_empty() {
            return;
        }
        self.g.fill(r, 0.0, th().input);
        let w = r.w / items.len() as f32;
        for (k, item) in items.iter().enumerate() {
            let cell = Rect::new(r.x + k as f32 * w, r.y, w, r.h);
            let hot = self.button(item.act, cell);
            if item.selected {
                fill_cut(self.g, cell, Cut::TopRight, SEGMENT_CUT, th().acc_soft);
            } else if hot {
                self.g.fill(cell, 0.0, th().sel);
            }
            if k > 0 {
                self.g.fill(Rect::new(cell.x, r.y, 1.0, r.h), 0.0, th().line2);
            }
            let (color, weight) = if item.selected { (th().acc, Weight::Semibold) } else { (th().text, Weight::Medium) };
            if item.sub.is_empty() {
                let size = if item.kind == Label::Name { Size::Body } else { Size::Value };
                self.g.text(&item.label, Rect::new(cell.x + 4.0, cell.y, cell.w - 8.0, cell.h), size, weight, Align::Center, color);
                continue;
            }
            let half = cell.h / 2.0;
            self.g.text(&item.label, Rect::new(cell.x, cell.y + 3.0, cell.w, half), Size::Value, weight, Align::Center, color);
            let sub_c = if item.selected { th().acc } else { th().muted };
            self.g.text(&item.sub, Rect::new(cell.x, cell.y + half - 2.0, cell.w, half), Size::Fine, Weight::Medium, Align::Center, sub_c);
        }
        self.g.stroke(r, 0.0, 1.0, th().line2);
        // Drawn last, over the dividers and the frame.
        for (k, item) in items.iter().enumerate() {
            if item.selected {
                stroke_cut(self.g, Rect::new(r.x + k as f32 * w, r.y, w + 1.0, r.h), Cut::TopRight, SEGMENT_CUT, th().acc);
            }
        }
    }

    /// Title and description of a setting row at `y`, with a hairline
    /// above every row but the first. Returns where its control goes.
    fn row(&mut self, x: f32, y: f32, w: f32, r: &SettingRow, place: RowPlace) -> Rect {
        if place == RowPlace::Next {
            self.g.fill(Rect::new(x + 1.0, y, w - 2.0, 1.0), 0.0, th().line);
        }
        let label_x = x + ROW_PAD;
        self.g.text(r.title, Rect::new(label_x, y + ROW_PAD, LABEL_W, 18.0), Size::Body, Weight::Semibold, Align::Left, th().text);
        let control = r.control(x, y, w);
        if r.desc.is_empty() {
            return control;
        }
        // A short description sits under the title; a long one under the
        // control, where the lines are longer.
        let desc = match r.desc_under() {
            false => Rect::new(label_x, y + ROW_PAD + 21.0, LABEL_W, r.height(w)),
            true => Rect::new(control.x, control.y + r.control_h + 6.0, control.w, r.height(w)),
        };
        self.g.text(r.desc, desc, Size::Caption, Weight::Normal, Align::Wrap, th().muted);
        control
    }

    fn caption(&mut self, s: &str, x: f32, y: f32) {
        self.caption_in(s, x, y, 300.0);
    }

    /// Upper-case section caption; a long translation ends in "…" at `w`.
    fn caption_in(&mut self, s: &str, x: f32, y: f32, w: f32) {
        self.g.text(s, Rect::new(x, y, w, 16.0), Size::Label, Weight::Semibold, Align::Left, th().muted);
    }

    /// Progress bar: a flat track with the fill from the left.
    fn bar(&self, r: Rect, pct: u8, c: Color) {
        self.g.fill(r, 0.0, th().track);
        let w = r.w * pct.min(100) as f32 / 100.0;
        if w > 0.0 {
            self.g.fill(Rect::new(r.x, r.y, w, r.h), 0.0, c);
        }
    }

    /// On/off as a 14 x 14 checkbox (the design has no switches), in the
    /// 40 x 22 box rows reserve for it. `None` = cannot be changed now.
    fn toggle(&mut self, act: Option<Act>, x: f32, y: f32, on: bool) {
        const BOX: f32 = 14.0;
        const DIMMED: u8 = 0x73;
        let area = Rect::new(x, y, 40.0, 22.0);
        let hot = act.is_some_and(|a| self.button(a, area));
        let alpha = if act.is_none() { DIMMED } else { 0xff };
        let b = Rect::new(x, y + (area.h - BOX) / 2.0, BOX, BOX);

        if on {
            self.g.fill(b, 0.0, Color(th().acc_fill.0, alpha));
            glyph(self.g, Glyph::Check, b.x + BOX / 2.0, b.y + BOX / 2.0, Color(th().acc_on.0, alpha));
            return;
        }
        self.g.fill(b, 0.0, Color(th().input.0, alpha));
        let edge = if hot { th().acc } else { th().line2 };
        self.g.stroke(b, 0.0, 1.0, Color(edge.0, alpha));
    }

    /// Button: 1 px border on a transparent ground; the primary one is
    /// filled with the accent. At most one primary per panel or dialog.
    fn push_button(&mut self, act: Act, r: Rect, label: &str, primary: bool) {
        let hot = self.button(act, r);
        let label_r = Rect::new(r.x + 8.0, r.y, r.w - 16.0, r.h);
        if primary {
            fill_cut(self.g, r, Cut::BottomRight, BUTTON_CUT, th().acc_fill);
            if hot {
                stroke_cut(self.g, r, Cut::BottomRight, BUTTON_CUT, th().text);
            }
            self.g.text(label, label_r, Size::Body, Weight::Semibold, Align::Center, th().acc_on);
            return;
        }
        if hot {
            self.g.fill(r, 0.0, th().sel);
        }
        self.g.stroke(r, 0.0, 1.0, th().line2);
        self.g.text(label, label_r, Size::Body, Weight::Semibold, Align::Center, th().text);
    }

    /// Secondary button with a glyph before or after the label, e.g. "‹ Back".
    fn glyph_button(&mut self, act: Act, r: Rect, label: &str, g: Glyph, place: Place) {
        const GLYPH_W: f32 = 16.0;
        let hot = self.button(act, r);
        if hot {
            self.g.fill(r, 0.0, th().sel);
        }
        self.g.stroke(r, 0.0, 1.0, th().line2);

        // Label and glyph centered as one group.
        let label_w = text_w(label, Size::Body).min(r.w - GLYPH_W - 24.0);
        let x = r.x + (r.w - label_w - GLYPH_W - 4.0) / 2.0;
        let (gx, lx) = match place {
            Place::Lead => (x + GLYPH_W / 2.0, x + GLYPH_W + 4.0),
            Place::Trail => (x + label_w + 4.0 + GLYPH_W / 2.0, x),
        };
        glyph(self.g, g, gx, r.y + r.h / 2.0, th().text);
        let label_r = Rect::new(lx, r.y, label_w + 8.0, r.h);
        self.g.text(label, label_r, Size::Body, Weight::Semibold, Align::Left, th().text);
    }

    /// Button that can't be pressed now, e.g. "Clear" on an empty macro.
    fn dead_button(&mut self, r: Rect, label: &str) {
        self.g.stroke(r, 0.0, 1.0, th().line);
        self.g.text(label, Rect::new(r.x + 8.0, r.y, r.w - 16.0, r.h), Size::Body, Weight::Semibold, Align::Center, th().line2);
    }

    /// Underlined text tabs, e.g. "Buttons | Macros"; a tab is a page.
    fn tabs(&mut self, items: &[(Page, &str)], current: Page) {
        const GAP: f32 = 20.0;
        let y = TOP;
        self.g.fill(Rect::new(X0, y + TAB_H - 1.0, CW, 1.0), 0.0, th().line);

        let mut x = X0;
        for &(page, label) in items {
            let r = Rect::new(x, y, text_w(label, Size::Body) + 8.0, TAB_H);
            let hot = self.button(Act::Nav(page), r);
            let sel = page == current;
            let color = if sel || hot { th().text } else { th().muted };
            let weight = if sel { Weight::Semibold } else { Weight::Normal };
            self.g.text(label, Rect::new(r.x, r.y, r.w + 20.0, r.h - 2.0), Size::Body, weight, Align::Left, color);
            if sel {
                self.g.fill(Rect::new(r.x, r.y + r.h - 2.0, r.w - 4.0, 2.0), 0.0, th().acc_fill);
            }
            x += r.w + GAP - 4.0;
        }
    }

    /// One option of a set: a cell of a segmented control. Selected: soft
    /// accent ground, a 1 px accent outline, accent text; nothing else.
    fn choice(&mut self, act: Act, r: Rect, title: &str, sub: &str, selected: bool) {
        let hot = self.button(act, r);
        if selected {
            self.g.fill(r, 0.0, th().input);
            fill_cut(self.g, r, Cut::TopRight, SEGMENT_CUT, th().acc_soft);
            stroke_cut(self.g, r, Cut::TopRight, SEGMENT_CUT, th().acc);
        } else {
            self.g.fill(r, 0.0, if hot { th().sel } else { th().input });
            self.g.stroke(r, 0.0, 1.0, th().line2);
        }

        let color = if selected { th().acc } else { th().text };
        let weight = if selected { Weight::Semibold } else { Weight::Medium };
        let (x, w) = (r.x + 6.0, r.w - 12.0);
        if sub.is_empty() {
            self.g.text(title, Rect::new(x, r.y, w, r.h), Size::Body, weight, Align::Center, color);
            return;
        }
        let top = Rect::new(x, r.y + 3.0, w, r.h / 2.0 - 1.0);
        let bottom = Rect::new(x, r.y + r.h / 2.0 + 1.0, w, r.h / 2.0 - 4.0);
        self.g.text(title, top, Size::Body, weight, Align::Center, color);
        self.g.text(sub, bottom, Size::Caption, Weight::Normal, Align::Center, if selected { th().acc } else { th().muted });
    }
}

/// The design has no round shapes: what used to be a disc or a ring is a
/// square of the same extent (`r` = half its side).
pub(self) trait Squares {
    fn dot(&self, cx: f32, cy: f32, r: f32, c: Color);
    fn frame(&self, cx: f32, cy: f32, r: f32, width: f32, c: Color);
}

impl Squares for Gfx {
    fn dot(&self, cx: f32, cy: f32, r: f32, c: Color) {
        self.fill(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), 0.0, c);
    }

    fn frame(&self, cx: f32, cy: f32, r: f32, width: f32, c: Color) {
        self.stroke(Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r), 0.0, width, c);
    }
}

/// Slider: a 2 px track, filled from the left up to the thumb, a 10 x 14
/// thumb outlined in the fill's color.
fn slider_mark(g: &Gfx, x0: f32, x1: f32, kx: f32, cy: f32, fill: Color) {
    const TRACK: f32 = 2.0;
    const THUMB_W: f32 = 10.0;
    const THUMB_H: f32 = 14.0;
    g.fill(Rect::new(x0, cy - TRACK / 2.0, x1 - x0, TRACK), 0.0, th().track);
    g.fill(Rect::new(x0, cy - TRACK / 2.0, kx - x0, TRACK), 0.0, fill);
    let thumb = Rect::new(kx - THUMB_W / 2.0, cy - THUMB_H / 2.0, THUMB_W, THUMB_H);
    g.fill(thumb, 0.0, th().panel);
    g.stroke(thumb, 0.0, 1.0, fill);
}

/// Where a glyph sits next to a label.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Lead,
    Trail,
}

/// Line glyphs on a 16 px box, 1.5 px stroke.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Glyph {
    Check,
    ChevronRight,
    ChevronLeft,
    ChevronDown,
    ArrowRight,
    ArrowLeft,
}

/// Draws `g` centered at `cx`, `cy`.
fn glyph(gfx: &Gfx, g: Glyph, cx: f32, cy: f32, c: Color) {
    const W: f32 = 1.5;
    // The left arrow is the right one mirrored.
    let mirror = if g == Glyph::ArrowLeft { -1.0 } else { 1.0 };
    let pts: [(f32, f32); 3] = match g {
        Glyph::Check => [(-3.5, 0.0), (-1.0, 2.5), (3.5, -2.5)],
        Glyph::ChevronRight => [(-2.0, -4.5), (2.5, 0.0), (-2.0, 4.5)],
        Glyph::ChevronLeft => [(2.0, -4.5), (-2.5, 0.0), (2.0, 4.5)],
        Glyph::ChevronDown => [(-4.5, -2.0), (0.0, 2.5), (4.5, -2.0)],
        Glyph::ArrowRight | Glyph::ArrowLeft => {
            // Shaft here, the head below.
            gfx.line(cx - 6.0, cy, cx + 6.0, cy, W, c);
            [(2.0, -4.0), (6.0, 0.0), (2.0, 4.0)]
        }
    };
    gfx.polyline(&pts.map(|(x, y)| (cx + x * mirror, cy + y)), W, c);
}

/// Placeholder bar while a value from the mouse is still on its way.
fn skeleton(g: &Gfx, r: Rect) {
    g.fill(r, 0.0, th().track);
}

/// Mouse name for the sidebar in up to two lines: breaks after a space or
/// "/" so "Logitech Marathon Mouse/Performance Plus M705" stays inside the
/// panel; the second line is trimmed with "…" when still too long.
fn name_lines(name: &str, width: f32, size: Size) -> (String, Option<String>) {
    if text_w(name, size) <= width {
        return (name.to_string(), None);
    }

    // Last break (after a space or "/") whose first line still fits.
    let breaks = name.char_indices().filter(|&(_, c)| c == ' ' || c == '/').map(|(i, c)| i + c.len_utf8());
    let Some(at) = breaks.rev().find(|&i| text_w(name[..i].trim_end(), size) <= width) else {
        return (name.to_string(), None);
    };
    (name[..at].trim_end().to_string(), Some(name[at..].trim_start().to_string()))
}

/// Rough text width in em: an Inter glyph is ~0.56 em on average, a
/// JetBrains Mono one exactly 0.6 em, CJK ~1 em in either.
fn ems(text: &str, face: Face) -> f32 {
    const UI_EM: f32 = 0.56;
    const MONO_EM: f32 = 0.6;
    let latin = if face == Face::Mono { MONO_EM } else { UI_EM };
    text.chars().map(|c| if (c as u32) < 0x2E80 { latin } else { 1.0 }).sum()
}

/// Rough width of a one-line text in DIPs.
fn text_w(text: &str, size: Size) -> f32 {
    ems(text, size.face()) * size.px() + text.chars().count() as f32 * size.tracking()
}

/// "56 %", or "≈ 50 %" for a coarse level.
/// Line under the battery bar. A battery reported in a few coarse
/// steps (M705) gets no runtime estimate: it would only mislead.
pub fn battery_hint(v: &View) -> &str {
    match v.charge {
        Charge::Charging => tr(T::Charging),
        Charge::Full => tr(T::FullyCharged),
        Charge::Discharging if v.battery_approx => "",
        Charge::Discharging => v.battery_note.as_str(),
    }
}

pub fn battery_text(v: &View) -> String {
    let approx = if v.battery_approx { "≈ " } else { "" };
    format!("{approx}{} %", v.battery)
}

pub fn battery_color(v: &View) -> Color {
    match v.battery {
        _ if v.charge == Charge::Charging => th().accent,
        b if b <= LOW => th().bad,
        b if b <= MID && !v.battery_approx => th().warn,
        _ => th().good,
    }
}

pub fn draw(g: &mut Gfx, v: &View, hot: Option<Act>) -> Vec<Hit> {
    let mut f = Frame { g, hot, hits: Vec::new() };
    f.g.clear(th().bg);
    if v.fx_texture {
        backdrop(f.g);
    }

    title_bar(&mut f, v);
    if v.tui {
        tui::draw(&mut f, v);
    } else {
        sidebar(&mut f, v);
        status_bar(&mut f, v);
        let title = Rect::new(X0, TITLE_Y, CW, 24.0);
        f.g.text(&tr(v.page.title()).to_uppercase(), title, Size::Title, Weight::Semibold, Align::Left, th().text);
        f.g.fill(Rect::new(X0, TITLE_Y + 24.0 + TITLE_RULE.y, TITLE_RULE.w, TITLE_RULE.h), 0.0, th().acc_fill);
        if matches!(v.page.nav(), Page::Profiles | Page::Overlay) {
            let title_w = text_w(&tr(v.page.title()).to_uppercase(), Size::Title);
            pro_tag(&mut f, X0 + title_w + 12.0, TITLE_Y + 4.0);
        }
        header_status(&mut f, v);
        // Over the title row: never covers controls of the page.
        if let Some(b) = &v.banner {
            device::banner(&mut f, b);
        }
        // No permission: say so where the mouse is expected.
        let hint_page = matches!(v.page, Page::Overview | Page::Mouse);
        if v.banner.is_none() && hint_page && v.access == Access::Denied {
            device::access_banner(&mut f);
        }

        match v.page {
            Page::Overview => overview(&mut f, v),
            Page::Dpi => dpi(&mut f, v),
            Page::Polling => performance(&mut f, v),
            Page::Lighting => lighting(&mut f, v),
            Page::Buttons | Page::Macros => {
                f.tabs(&BUTTON_TABS.map(|(p, t)| (p, tr(t))), v.page);
                if v.page == Page::Buttons { buttons::buttons_page(&mut f, v) } else { buttons::macros_page(&mut f, v) }
            }
            Page::Stats => stats(&mut f, v),
            Page::Profiles => pro::profiles_page(&mut f),
            Page::Overlay => pro::overlay_page(&mut f),
            Page::General | Page::Appearance | Page::Mouse | Page::About => {
                f.tabs(&SETTING_TABS.map(|(p, t)| (p, tr(t))), v.page);
                match v.page {
                    Page::General => settings(&mut f, v),
                    Page::Appearance => appearance(&mut f, v),
                    Page::About => about_page(&mut f, v),
                    _ => device::device_page(&mut f, v),
                }
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
        Some(Dialog::Crashed) => {
            f.hits.clear();
            crash_dialog(&mut f);
        }
        Some(Dialog::AssignMacro) => {
            f.hits.clear();
            buttons::assign_dialog(&mut f, v);
        }
        Some(Dialog::Update) => {
            f.hits.clear();
            update::update_dialog(&mut f, &v.update);
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
    // Opened from the detect dialog too: on top of it.
    if let Some(r) = v.report.as_ref().filter(|_| !v.ask_setup) {
        f.hits.clear();
        report::report_dialog(&mut f, r);
    }

    focus_ring(&f, v.focus);
    f.hits
}

/// Keyboard focus: a ring around the focused hit of this frame.
fn focus_ring(f: &Frame, focus: Option<Focus>) {
    let acts: Vec<Act> = f.hits.iter().map(|h| h.act).collect();
    let Some(h) = focus.and_then(|x| x.locate(&acts)).map(|i| f.hits[i]) else {
        return;
    };
    // 2 px accent ring, 2 px away from the element (outside a selected
    // tile's own accent border).
    const OFFSET: f32 = 2.0;
    const WIDTH: f32 = 2.0;
    let d = OFFSET + WIDTH;
    let r = Rect::new(h.rect.x - d, h.rect.y - d, h.rect.w + 2.0 * d, h.rect.h + 2.0 * d);
    f.g.stroke(r, R_CTRL + d, WIDTH, th().accent);
}

/// Right side of the title row: offline note on pages that configure the
/// mouse, otherwise whether the last change reached it.
fn header_status(f: &mut Frame, v: &View) {
    const H: f32 = 28.0;
    const MAX_W: f32 = 380.0;
    let offline = v.page.configures() && !v.connected;
    let (text, dot, alpha) = match v.sync {
        _ if offline => (tr(T::OfflineBanner), th().warn, 255),
        Sync::Waiting => (tr(T::SyncWaiting), th().subtle, 255),
        Sync::Saved(a) => (tr(T::SyncSaved), th().good, a),
        Sync::Hidden => return,
    };
    let w = (text_w(text, Size::Caption) + 52.0).min(MAX_W);
    let r = Rect::new(X0 + CW - w, TITLE_Y - 2.0, w, H);
    f.g.fill(r, 0.0, Color(th().panel.0, alpha));
    f.g.stroke(r, 0.0, 1.0, Color(th().line.0, alpha));
    f.g.dot(r.x + 15.0, r.y + H / 2.0, 4.0, Color(dot.0, alpha));
    let label = Rect::new(r.x + 26.0, r.y, r.w - 34.0, H);
    f.g.text(text, label, Size::Caption, Weight::Normal, Align::Left, Color(th().muted.0, alpha));
}

/// Bottom message with an optional "Undo".
fn toast(f: &mut Frame, t: &Toast) {
    const W: f32 = 420.0;
    const H: f32 = 44.0;
    let r = Rect::new(X0 + (CW - W) / 2.0, PAGE_H - H - 4.0, W, H);
    f.g.fill(r, RADIUS, th().card_hi);
    f.g.stroke(r, RADIUS, 1.0, th().border);

    let undo_w = if t.undo { text_w(tr(T::UndoBtn), Size::Body) + 32.0 } else { 0.0 };
    let label = Rect::new(r.x + 18.0, r.y, r.w - 36.0 - undo_w, r.h);
    f.g.text(&t.text, label, Size::Body, Weight::Normal, Align::Left, th().text);
    if !t.undo {
        return;
    }
    let b = Rect::new(r.x + r.w - undo_w - 8.0, r.y + 6.0, undo_w, r.h - 12.0);
    let hot = f.button(Act::Undo, b);
    f.g.fill(b, R_CTRL, if hot { th().hover } else { th().card_hi });
    f.g.text(tr(T::UndoBtn), b, Size::Body, Weight::Semibold, Align::Center, th().accent);
}

// ---- icons (24-unit drawings on a 20 px grid at x, y) ------------------------

const ICON_PX: f32 = 20.0;
const ICON_SCALE: f32 = ICON_PX / 24.0;
const ICON_STROKE: f32 = 1.5;

fn icon(g: &Gfx, page: Page, x: f32, y: f32, c: Color) {
    let s = ICON_SCALE;
    let p = |a: f32, b: f32| (x + a * s, y + b * s);
    let w = ICON_STROKE;

    match page {
        Page::Overview => {
            for (a, b) in [(4.0, 4.0), (13.0, 4.0), (4.0, 13.0), (13.0, 13.0)] {
                let (px, py) = p(a, b);
                g.stroke(Rect::new(px, py, 7.0 * s, 7.0 * s), 0.0, w, c);
            }
        }
        Page::Dpi => {
            let (cx, cy) = p(12.0, 12.0);
            g.frame(cx, cy, 4.0 * s, w, c);
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
            g.frame(cx, cy, 4.5 * s, w, c);
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
            g.stroke(Rect::new(x0, y0, 10.0 * s, 18.0 * s), 0.0, w, c);
            let ((lx, ly0), (_, ly1)) = (p(12.0, 4.0), p(12.0, 10.0));
            g.line(lx, ly0, lx, ly1, w, c);
        }
        Page::Stats => {
            for (a, top) in [(5.0, 11.0), (12.0, 5.0), (19.0, 14.0)] {
                let ((x0, y0), (_, y1)) = (p(a, top), p(a, 20.0));
                g.line(x0, y0, x0, y1, w + 0.5, c);
            }
        }
        _ => {
            // Two sliders: a track broken around its knob.
            const KNOB: f32 = 3.0;
            for (b, knob) in [(7.0, 16.0), (17.0, 8.0)] {
                let ((x0, y0), (x1, _)) = (p(3.0, b), p(21.0, b));
                let (kx, ky) = p(knob, b);
                let gap = (KNOB + 1.5) * s;
                g.line(x0, y0, kx - gap, y0, w, c);
                g.line(kx + gap, y0, x1, y0, w, c);
                g.frame(kx, ky, KNOB * s, w, c);
            }
        }
    }
}

/// The logo: a mouse with whiskers drawn as circuit traces, on a 24 x 24
/// grid scaled to `size`. Strokes in the text color, the nose in the accent.
///
/// ```text
///   ─╲___   ___╱─      whiskers: three per side, the outer ones bend
///   ──────■──────      nose
///   ─╱‾‾‾ ┌─┬─┐ ‾‾‾╲─
///         ├─┴─┤        body: chamfered corners, split between the buttons
///         └───┘
/// ```
fn logo(g: &Gfx, x: f32, y: f32, size: f32) {
    const GRID: f32 = 24.0;
    /// 1.3 on the grid; never thinner than a pixel.
    const STROKE: f32 = 1.3;
    const BODY: [(f32, f32); 8] = [(9.0, 7.5), (15.0, 7.5), (16.5, 9.0), (16.5, 21.0), (15.0, 22.5), (9.0, 22.5), (7.5, 21.0), (7.5, 9.0)];
    const SPLIT: [[(f32, f32); 2]; 2] = [[(12.0, 7.5), (12.0, 12.5)], [(7.5, 12.5), (16.5, 12.5)]];
    /// Left side; the right one mirrors it around x = 12.
    const WHISKERS: [&[(f32, f32)]; 3] =
        [&[(10.0, 3.5), (6.5, 3.5), (5.0, 2.0), (2.5, 2.0)], &[(10.0, 5.0), (2.0, 5.0)], &[(10.0, 6.5), (6.5, 6.5), (5.0, 8.0), (2.5, 8.0)]];

    let s = size / GRID;
    let w = (STROKE * s).max(1.0);
    let c = th().text;
    let at = |(px, py): (f32, f32)| (x + px * s, y + py * s);
    let trace = |pts: &[(f32, f32)]| {
        for pair in pts.windows(2) {
            let ((x0, y0), (x1, y1)) = (at(pair[0]), at(pair[1]));
            g.line(x0, y0, x1, y1, w, c);
        }
    };

    let mut body = BODY.to_vec();
    body.push(BODY[0]);
    trace(&body);
    for line in SPLIT {
        trace(&line);
    }
    for whisker in WHISKERS {
        trace(whisker);
        let mirrored: Vec<(f32, f32)> = whisker.iter().map(|&(px, py)| (GRID - px, py)).collect();
        trace(&mirrored);
    }
    let (nx, ny) = at((11.0, 4.0));
    g.fill(Rect::new(nx, ny, 2.0 * s, 2.0 * s), 0.0, th().acc_fill);
}

/// The "Alive" backdrop of the content area: a grid that fades in toward
/// the bottom-right, where pages are empty, and a field of dots in that
/// corner. Static; drawn under everything.
///
/// ```text
///  dense content here: nothing     ·
///                              ┼───┼───┼
///                          ┼───┼ · ┼ · ┼   grid and dots grow stronger
///                      ┼───┼ · ┼ · ┼ · ┼   toward this corner
/// ```
fn backdrop(g: &Gfx) {
    const CELL: f32 = 24.0;
    /// The grid starts 30 % of the way along the diagonal.
    const GRID_FROM: f32 = 0.3;
    const DOT_STEP: f32 = 6.0;
    const DOT: f32 = 1.4;
    const DOT_AREA: f32 = 0.65;
    const DOT_ALPHA: f32 = 0.28;
    /// The dots are gone 70 % of the way from the corner.
    const DOT_REACH: f32 = 0.7;
    /// Fainter than this is not worth a draw call.
    const VISIBLE: f32 = 0.03;
    let area = Rect::new(SIDE_W, TITLE_H, WIDTH - SIDE_W, PAGE_H - TITLE_H);
    let strength = if th().dark { 0.6 } else { 0.9 };
    let ink = |c: Color, a: f32| Color(c.0, (a * 255.0) as u8);

    // 0 in the top-left corner of the area, 1 in the bottom-right one.
    let along = |x: f32, y: f32| ((x - area.x) / area.w + (y - area.y) / area.h) / 2.0;
    let grid_alpha = |x: f32, y: f32| ((along(x, y) - GRID_FROM) / (1.0 - GRID_FROM)).clamp(0.0, 1.0) * strength * 0.7;
    let (cols, rows) = ((area.w / CELL).ceil() as usize, (area.h / CELL).ceil() as usize);
    for row in 0..rows {
        for col in 0..cols {
            let (x, y) = (area.x + col as f32 * CELL, area.y + row as f32 * CELL);
            let a = grid_alpha(x + CELL / 2.0, y + CELL / 2.0);
            if a < VISIBLE {
                continue;
            }
            // Each cell draws its top and left edge.
            g.fill(Rect::new(x, y, CELL, 1.0), 0.0, ink(th().line2, a));
            g.fill(Rect::new(x, y, 1.0, CELL), 0.0, ink(th().line2, a));
        }
    }

    // Dots: strongest in the corner, fading with the distance from it.
    let (corner_x, corner_y) = (area.x + area.w, area.y + area.h);
    let (reach_w, reach_h) = (area.w * DOT_AREA, area.h * DOT_AREA);
    let mut y = corner_y - DOT_STEP / 2.0;
    while y > corner_y - reach_h {
        let mut x = corner_x - DOT_STEP / 2.0;
        while x > corner_x - reach_w {
            let far = (((corner_x - x) / reach_w).powi(2) + ((corner_y - y) / reach_h).powi(2)).sqrt() / DOT_REACH;
            let a = DOT_ALPHA * (1.0 - far);
            if a >= VISIBLE {
                g.fill(Rect::new(x, y, DOT, DOT), 0.0, ink(th().text, a));
            }
            x -= DOT_STEP;
        }
        y -= DOT_STEP;
    }
}

/// Fine grain on the sidebar: sparse 1 px specks, the same every frame.
fn speckle(g: &Gfx, r: Rect) {
    const SPECKS: usize = 2200;
    const SEED: u32 = 0x5eed_1e55;
    let alpha = if th().dark { 0x1c } else { 0x16 };
    let mut state = SEED;
    // A small linear congruential generator: the pattern must not change
    // between frames, so no clock and no global randomness.
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    for _ in 0..SPECKS {
        let (x, y) = (r.x + (next() * (r.w - 1.0)).floor(), r.y + (next() * r.h).floor());
        g.fill(Rect::new(x, y, 1.0, 1.0), 0.0, Color(th().text.0, alpha));
    }
}

/// Title bar: logo, app name, the mouse in use (a click changes it),
/// window buttons. Its empty part moves the window.
fn title_bar(f: &mut Frame, v: &View) {
    const PAD: f32 = 12.0;
    const LOGO: f32 = 18.0;
    const GAP: f32 = 8.0;
    const DEVICE_H: f32 = 22.0;
    const DEVICE_PAD: f32 = 9.0;
    const DEVICE_MAX: f32 = 300.0;
    const WIN_BTN_W: f32 = 40.0;
    /// Window buttons, right to left.
    const WIN_BUTTONS: [(Act, WinIcon); 2] = [(Act::WinClose, WinIcon::Close), (Act::WinMinimize, WinIcon::Minimize)];

    let bar = Rect::new(0.0, 0.0, WIDTH, TITLE_H);
    f.g.fill(bar, 0.0, th().side);
    f.g.fill(Rect::new(0.0, TITLE_H - 1.0, WIDTH, 1.0), 0.0, th().line);
    // Under everything else in the bar: what is not a control drags.
    f.hits.push(Hit { act: Act::Caption, rect: bar, kind: Kind::Caption });

    logo(f.g, PAD, (TITLE_H - LOGO) / 2.0, LOGO);
    let name = crate::APP_NAME.to_uppercase();
    let name_x = PAD + LOGO + GAP;
    let name_w = text_w(&name, Size::AppName);
    f.g.text(&name, Rect::new(name_x, 0.0, name_w + 8.0, TITLE_H), Size::AppName, Weight::Semibold, Align::Left, th().text);

    // The mouse in use; with no name yet there is nothing to show.
    if !v.device_name.is_empty() {
        let w = (text_w(&v.device_name, Size::Caption) + 2.0 * DEVICE_PAD).min(DEVICE_MAX);
        let r = Rect::new(name_x + name_w + 14.0, (TITLE_H - DEVICE_H) / 2.0, w, DEVICE_H);
        let hot = f.button(Act::ChangeMouse, r);
        f.g.fill(r, 0.0, if hot { th().sel } else { th().input });
        f.g.stroke(r, 0.0, 1.0, th().line2);
        let label = Rect::new(r.x + DEVICE_PAD, r.y, r.w - 2.0 * DEVICE_PAD, r.h);
        f.g.text(&v.device_name, label, Size::Caption, Weight::Medium, Align::Left, th().text);
    }

    // Text mode switch: ">_ TUI", or "GUI" on the accent while it is on.
    const MODE_H: f32 = 22.0;
    let label = if v.tui { "GUI" } else { ">_ TUI" };
    let mode_w = text_w(label, Size::Key) + 22.0;
    let mode = Rect::new(WIDTH - WIN_BUTTONS.len() as f32 * WIN_BTN_W - 9.0 - mode_w, (TITLE_H - MODE_H) / 2.0, mode_w, MODE_H);
    let hot = f.button(Act::ToggleTui, mode);
    if v.tui {
        f.g.fill(mode, 0.0, th().acc_fill);
    } else {
        f.g.fill(mode, 0.0, if hot { th().sel } else { th().input });
        f.g.stroke(mode, 0.0, 1.0, th().line2);
    }
    let ink = if v.tui { th().acc_on } else if hot { th().text } else { th().muted };
    f.g.text(label, mode, Size::Key, Weight::Semibold, Align::Center, ink);

    for (k, (act, icon)) in WIN_BUTTONS.into_iter().enumerate() {
        let r = Rect::new(WIDTH - (k + 1) as f32 * WIN_BTN_W, 0.0, WIN_BTN_W, TITLE_H - 1.0);
        if f.button(act, r) {
            f.g.fill(r, 0.0, th().sel);
        }
        f.g.fill(Rect::new(r.x, 0.0, 1.0, TITLE_H - 1.0), 0.0, th().line);
        win_icon(f.g, icon, r.x + r.w / 2.0, TITLE_H / 2.0);
    }
}

#[derive(Clone, Copy)]
enum WinIcon {
    Minimize,
    Close,
}

/// A 10 px stroke icon of a window button, centred on (`cx`, `cy`).
fn win_icon(g: &Gfx, icon: WinIcon, cx: f32, cy: f32) {
    const HALF: f32 = 5.0;
    let c = th().text;
    match icon {
        WinIcon::Minimize => g.line(cx - HALF, cy, cx + HALF, cy, 1.0, c),
        WinIcon::Close => {
            g.line(cx - HALF, cy - HALF, cx + HALF, cy + HALF, 1.0, c);
            g.line(cx - HALF, cy + HALF, cx + HALF, cy - HALF, 1.0, c);
        }
    }
}

/// The "PRO" tag: features of the paid version.
fn pro_tag(f: &mut Frame, x: f32, y: f32) -> f32 {
    const H: f32 = 16.0;
    const PAD: f32 = 5.0;
    const LABEL: &str = "PRO";
    let w = text_w(LABEL, Size::Tag) + 2.0 * PAD;
    let r = Rect::new(x, y, w, H);
    f.g.fill(r, 0.0, th().acc_fill);
    f.g.text(LABEL, Rect::new(r.x + PAD, r.y, r.w, r.h), Size::Tag, Weight::Semibold, Align::Left, th().acc_on);
    w
}

/// Hatched strip between the free pages and the PRO group: diagonal
/// hairlines every 6 px, a hairline above and below.
fn hatch(g: &Gfx, r: Rect) {
    const STEP: f32 = 6.0;
    g.fill(Rect::new(r.x, r.y, r.w, 1.0), 0.0, th().line);
    g.fill(Rect::new(r.x, r.y + r.h - 1.0, r.w, 1.0), 0.0, th().line);

    // Each line climbs from the bottom edge to the top one, 45 degrees.
    let rise = r.h - 2.0;
    let mut x = r.x - rise;
    while x < r.x + r.w {
        // Clipped to the strip at both ends.
        let (x0, x1) = (x.max(r.x), (x + rise).min(r.x + r.w));
        let (y0, y1) = (r.y + r.h - 1.0 - (x0 - x), r.y + r.h - 1.0 - (x1 - x));
        g.line(x0, y0, x1, y1, 1.0, th().line2);
        x += STEP;
    }
}

fn sidebar(f: &mut Frame, v: &View) {
    const HATCH_H: f32 = 8.0;
    const PRO_ROW_H: f32 = 30.0;
    const FOOT_PAD: f32 = 4.0;
    /// Pages of the PRO version: mock-ups in this one.
    const PRO_PAGES: [Page; 2] = [Page::Profiles, Page::Overlay];

    let side = Rect::new(0.0, TITLE_H, SIDE_W, PAGE_H - TITLE_H);
    f.g.fill(side, 0.0, th().side);
    if v.fx_texture {
        speckle(f.g, side);
    }
    f.g.fill(Rect::new(SIDE_W - 1.0, TITLE_H, 1.0, side.h), 0.0, th().line);
    let item = |y: f32| Rect::new(0.0, y, SIDE_W - 1.0, NAV_STEP);

    let mut y = NAV_TOP;
    for page in nav_pages(&v.caps) {
        nav_item(f, Nav::Page(page), item(y), Mark::of(v.page.nav() == page));
        y += NAV_STEP;
    }

    // The PRO group: one hatched strip, the tag, its pages.
    y += 8.0;
    hatch(f.g, Rect::new(0.0, y, SIDE_W - 1.0, HATCH_H));
    y += HATCH_H;
    pro_tag(f, NAV_PAD, y + (PRO_ROW_H - 16.0) / 2.0);
    y += PRO_ROW_H;
    for page in PRO_PAGES {
        nav_item(f, Nav::Page(page), item(y), Mark::of(v.page.nav() == page));
        y += NAV_STEP;
    }

    // Pinned at the bottom, over a hairline: Settings (About is a tab of it).
    let foot = PAGE_H - FOOT_PAD - NAV_STEP;
    f.g.fill(Rect::new(0.0, foot - FOOT_PAD, SIDE_W - 1.0, 1.0), 0.0, th().line);
    nav_item(f, Nav::Page(Page::General), item(foot), Mark::of(v.page.nav() == Page::General));
}

/// Status bar: connection and battery on the left (a click opens
/// Settings › Mouse), a new version on the right.
fn status_bar(f: &mut Frame, v: &View) {
    const PAD: f32 = 12.0;
    const DOT: f32 = 11.0;
    const MARK_H: f32 = 8.0;
    const MARK_SKEW: f32 = 3.0;
    let bar = Rect::new(0.0, PAGE_H, WIDTH, STATUS_H);
    f.g.fill(bar, 0.0, th().side);
    f.g.fill(Rect::new(0.0, PAGE_H, WIDTH, 1.0), 0.0, th().line);
    update::status_notice(f, &v.update);

    let (text, c) = status_line(v);
    let w = text_w(&text, Size::Caption) + DOT + 8.0 + 2.0 * PAD;
    let area = Rect::new(0.0, PAGE_H + 1.0, w, STATUS_H - 1.0);
    if f.button(Act::Nav(Page::Mouse), area) {
        f.g.fill(area, 0.0, th().sel);
    }
    // An 11 x 8 parallelogram, leaning right.
    let (mx, my) = (PAD, PAGE_H + (STATUS_H - MARK_H) / 2.0 + 0.5);
    let mark_c = if v.connected { Color(c.0, v.status_pulse) } else { c };
    f.g.fill_path(&polygon(&[(mx + MARK_SKEW, my), (mx + DOT, my), (mx + DOT - MARK_SKEW, my + MARK_H), (mx, my + MARK_H)]), mark_c);
    let label = Rect::new(PAD + DOT + 8.0, PAGE_H + 1.0, w, STATUS_H - 1.0);
    f.g.text(&text, label, Size::Caption, Weight::Normal, Align::Left, th().muted);
}

/// "Connected · 55 %" (⚡ while charging) and the color of its square:
/// green, the warning color when the battery runs low, muted offline.
fn status_text(v: &View) -> (String, Color) {
    const LOW_PCT: u8 = 20;
    if !v.connected {
        (tr(T::Disconnected).to_string(), th().muted)
    } else if !v.has_battery {
        (tr(T::LinkConnected).to_string(), th().good)
    } else {
        let charging = if v.charge == Charge::Charging { " ⚡" } else { "" };
        let c = if v.battery < LOW_PCT { th().warn } else { th().good };
        (format!("{} · {}{charging}", tr(T::LinkConnected), battery_text(v)), c)
    }
}

/// The status text with the firmware version after it, when known.
fn status_line(v: &View) -> (String, Color) {
    let (text, c) = status_text(v);
    if !v.connected || v.device.firmware.is_empty() {
        return (text, c);
    }
    (format!("{text} · FW {}", v.device.firmware), c)
}

/// Sidebar entry: a page.
#[derive(Clone, Copy)]
enum Nav {
    Page(Page),
}

/// Left padding of nav items and the PRO tag.
const NAV_PAD: f32 = 14.0;
/// Nav icons are drawn for 24 px rows; their box is 20 px.
const NAV_ICON: f32 = 20.0;

/// Sidebar entry: icon, name. The current one gets the selection fill
/// and a 2 px accent bar on the left, its only mark.
fn nav_item(f: &mut Frame, item: Nav, r: Rect, mark: Mark) {
    const BAR_W: f32 = 2.0;
    let (act, label) = match item {
        Nav::Page(p) => (Act::Nav(p), tr(p.title())),
    };
    let sel = mark == Mark::Current;
    let hot = f.button(act, r);
    if sel {
        // The right edge leans in toward the bottom.
        let (x1, y1) = (r.x + r.w, r.y + r.h);
        f.g.fill_path(&polygon(&[(r.x, r.y), (x1, r.y), (x1 - NAV_SLANT, y1), (r.x, y1)]), th().sel);
        f.g.fill(Rect::new(r.x, r.y, BAR_W, r.h), 0.0, th().acc_fill);
    } else if hot {
        f.g.fill(r, 0.0, Color(th().sel.0, 0x80));
    }
    let c = if sel { th().text } else { th().muted };
    let icon_y = r.y + (r.h - NAV_ICON) / 2.0;
    match item {
        Nav::Page(Page::Profiles) => pro_icon(f.g, ProIcon::Profiles, r.x + NAV_PAD, r.y + (r.h - 14.0) / 2.0, c),
        Nav::Page(Page::Overlay) => pro_icon(f.g, ProIcon::Overlay, r.x + NAV_PAD, r.y + (r.h - 14.0) / 2.0, c),
        Nav::Page(p) => icon(f.g, p, r.x + NAV_PAD - 3.0, icon_y, c),
    }
    let text_x = r.x + NAV_PAD + NAV_ICON + 4.0;
    let weight = if sel { Weight::Semibold } else { Weight::Medium };
    f.g.text(label, Rect::new(text_x, r.y, r.w - text_x - 4.0, r.h), Size::Body, weight, Align::Left, c);
}

#[derive(Clone, Copy)]
enum ProIcon {
    Profiles,
    Overlay,
}

/// Icon of a PRO page, on a 14 px grid.
fn pro_icon(g: &Gfx, icon: ProIcon, x: f32, y: f32, c: Color) {
    match icon {
        // Two stacked squares.
        ProIcon::Profiles => {
            g.stroke(Rect::new(x + 3.0, y, 11.0, 11.0), 0.0, 1.0, c);
            g.line(x + 0.5, y + 3.0, x + 0.5, y + 13.5, 1.0, c);
            g.line(x + 0.5, y + 13.5, x + 11.0, y + 13.5, 1.0, c);
        }
        // A screen with a box in its corner.
        ProIcon::Overlay => {
            g.stroke(Rect::new(x, y + 1.0, 14.0, 12.0), 0.0, 1.0, c);
            g.stroke(Rect::new(x + 2.5, y + 3.5, 5.0, 4.0), 0.0, 1.0, c);
        }
    }
}

// ---- pages ---------------------------------------------------------------

/// Value not known yet (no answer from the mouse, no measurement).
const UNKNOWN: &str = "—";

/// Whether a stat strip carries the "+" corner marks (Overview only).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Corners {
    Marked,
    Plain,
}

/// One cell of a stat strip: caption, big number with its unit, then a
/// progress bar and a note.
struct Stat<'a> {
    label: &'a str,
    /// `UNKNOWN` shows a skeleton bar; empty shows the note alone.
    value: String,
    unit: &'a str,
    note: String,
    /// Percent and color of the bar under the number.
    bar: Option<(u8, Color)>,
}

/// The numbers of a page at a glance: equal cells in one panel, split by
/// hairlines, with "+" marks on the outer corners.
///
/// ```text
///  +                                              +
///   Battery     | DPI          | Polling rate | Today
///   78 %        | 2400         | 500 Hz       | 1 853
///   ━━━━━━──    | Stage 3 of 6 |              | clicks · 36 m moved
///   ≈ 21 days
///  +                                              +
/// ```
fn stat_strip(f: &mut Frame, r: Rect, cells: &[Stat], corners: Corners) {
    const PAD_X: f32 = 14.0;
    const PAD_Y: f32 = 12.0;
    const VALUE_Y: f32 = 30.0;
    const VALUE_H: f32 = 34.0;
    const BAR_H: f32 = METER_H;
    f.card(r);
    if corners == Corners::Marked {
        corner_marks(f.g, r);
    }

    let w = r.w / cells.len().max(1) as f32;
    for (k, c) in cells.iter().enumerate() {
        let x = r.x + k as f32 * w;
        if k > 0 {
            f.g.fill(Rect::new(x, r.y, 1.0, r.h), 0.0, th().line);
        }
        let (cx, cw) = (x + PAD_X, w - 2.0 * PAD_X);
        f.g.text(c.label, Rect::new(cx, r.y + PAD_Y, cw, 16.0), Size::Caption, Weight::Normal, Align::Left, th().muted);

        let value = Rect::new(cx, r.y + VALUE_Y, cw, VALUE_H);
        let mut below = value.y + VALUE_H + 6.0;
        if c.value == UNKNOWN {
            skeleton(f.g, Rect::new(value.x, value.y + 8.0, 72.0, 20.0));
        } else if c.value.is_empty() {
            // No number to show: the note takes its place.
            below = value.y + 8.0;
        } else {
            f.g.text(&c.value, value, Size::Hero, Weight::Medium, Align::Left, th().text);
            let unit_x = value.x + text_w(&c.value, Size::Hero) + 6.0;
            f.g.text(c.unit, Rect::new(unit_x, value.y + 13.0, cw, 16.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
        }
        if let Some((pct, color)) = c.bar {
            meter(f.g, Rect::new(cx, below, cw, BAR_H), pct, color);
            below += BAR_H + 6.0;
        }
        f.g.text(&c.note, Rect::new(cx, below, cw, 32.0), Size::Caption, Weight::Normal, Align::Wrap, th().muted);
    }
}

/// Crosshair marks "+" centered on the outer corners of `r`: the one
/// ornament of the Overview strip.
fn corner_marks(g: &Gfx, r: Rect) {
    const ARM: f32 = 4.5;
    for (x, y) in [(r.x, r.y), (r.x + r.w, r.y), (r.x, r.y + r.h), (r.x + r.w, r.y + r.h)] {
        let (x, y) = (x - 0.5, y - 0.5);
        g.line(x - ARM, y + 0.5, x + ARM + 1.0, y + 0.5, 1.0, th().line2);
        g.line(x + 0.5, y - ARM, x + 0.5, y + ARM + 1.0, 1.0, th().line2);
    }
}

/// "500 Hz" -> ("500", "Hz"); a value without a unit keeps it empty.
fn split_unit(value: &str) -> (String, &str) {
    match value.rsplit_once(' ') {
        Some((number, unit)) if number.chars().any(|c| c.is_ascii_digit()) && !unit.chars().any(|c| c.is_ascii_digit()) => (number.to_string(), unit),
        _ => (value.to_string(), ""),
    }
}

/// The numbers of the Overview, for both the graphical and the text mode:
/// battery, DPI, polling rate, today. A cell the mouse has nothing for is
/// left out.
fn overview_stats(v: &View) -> Vec<Stat<'static>> {
    const RATE_UNIT: &str = "Hz";
    let mut cells = Vec::new();

    // Battery: from the mouse's driver, or from the OS for a Bluetooth mouse.
    if v.caps.battery || (v.connected && v.has_battery) {
        let shown = v.connected && v.has_battery;
        // "78 %" or "≈ 50 %": the number, then its unit.
        let (value, unit) = if shown { (battery_text(v).trim_end_matches(" %").to_string(), "%") } else { (UNKNOWN.to_string(), "") };
        let note = if shown { battery_hint(v) } else { v.link.as_str() };
        cells.push(Stat { label: tr(T::Battery), value, unit, note: note.to_string(), bar: Some((if shown { v.battery } else { 0 }, battery_color(v))) });
    }

    // No DPI cell for a mouse that neither sets nor reports DPI (M705).
    let stages = v.caps.dpi == DpiUi::Stages;
    if v.caps.dpi != DpiUi::None || v.basic.dpi.is_some() || v.basic.stage.is_some() {
        let (dpi, note) = match v.stages.get(v.active) {
            _ if !stages => {
                let stage_note = match (v.basic.stage, v.basic.stages) {
                    (Some(s), Some(n)) => fill(tr(T::StageOf), &[&s.to_string(), &n.to_string()]),
                    _ => String::new(),
                };
                match (v.basic.dpi, v.basic.stage, v.basic.stages) {
                    (Some(d), ..) => (d.to_string(), stage_note),
                    (None, Some(s), Some(n)) => (format!("{s} / {n}"), stage_note),
                    _ => (UNKNOWN.into(), String::new()),
                }
            }
            Some(s) => (s.dpi.to_string(), fill(tr(T::StageOf), &[&(v.active + 1).to_string(), &v.stages.iter().filter(|s| s.enabled).count().to_string()])),
            None => (UNKNOWN.into(), String::new()),
        };
        cells.push(Stat { label: tr(T::ActiveDpi), value: dpi, unit: "", note, bar: None });
    }

    // Polling rate: what the mouse is set to; where it does not tell,
    // what Raw Input measures while it moves.
    let (hz, note) = match (v.caps.polling, stages) {
        (true, true) => (format!("{} Hz", RATES[v.polling]), measured_note(v)),
        (true, false) => (v.basic.rate.map_or(UNKNOWN.into(), |r| format!("{r} Hz")), measured_note(v)),
        (false, _) => {
            let (value, note) = rate_value(v.last_rate);
            (value, note.to_string())
        }
    };
    // The unit outlives the text it was split from.
    let (value, unit) = match split_unit(&hz) {
        (number, "") => (number, ""),
        (number, _) => (number, RATE_UNIT),
    };
    cells.push(Stat { label: tr(T::PollingRate), value, unit, note, bar: None });

    let today = fill(tr(T::ClicksMoved), &[&v.today_distance]);
    cells.push(Stat { label: tr(T::Today), value: v.today_clicks.clone(), unit: "", note: today, bar: None });
    cells
}

/// Overview: what the mouse reports, as one strip of numbers; then the
/// quick DPI switch and the profile switch of the PRO version.
fn overview(f: &mut Frame, v: &View) {
    const STRIP_H: f32 = 108.0;
    let stages = v.caps.dpi == DpiUi::Stages;
    let cells = overview_stats(v);
    stat_strip(f, Rect::new(X0, TOP, CW, STRIP_H), &cells, Corners::Marked);
    let mut y = TOP + STRIP_H + PANEL_GAP;

    // Quick DPI switch: one segment per enabled stage.
    if stages {
        let panel = Rect::new(X0, y, CW, PANEL_HEAD + SEGMENT_H + 2.0 * PANEL_PAD);
        let body = f.panel(panel, tr(T::QuickDpi));
        f.panel_link(panel, Act::Nav(Page::Dpi), tr(T::EditStages));
        let items: Vec<Segment> =
            v.stages.iter().enumerate().filter(|(_, s)| s.enabled).map(|(i, s)| Segment::new(Act::Activate(i), s.dpi.to_string(), i == v.active)).collect();
        f.segments(Rect::new(body.x + PANEL_PAD, body.y + PANEL_PAD, body.w - 2.0 * PANEL_PAD, SEGMENT_H), &items);
        y += panel.h + PANEL_GAP;
    }
    pro::profile_switch(f, Rect::new(X0, y, CW, PANEL_HEAD + SEGMENT_H + 2.0 * PANEL_PAD));
}

/// "Measured: 498 Hz" once Raw Input has a real measurement.
fn measured_note(v: &View) -> String {
    v.measured.map(|hz| fill(tr(T::MeasuredValue), &[&format!("{hz} Hz")])).unwrap_or_default()
}

fn measured(v: &View) -> String {
    v.measured.map_or(UNKNOWN.into(), |hz| format!("{hz} Hz"))
}

/// Measured rate: (value, note); no value until the mouse moved.
fn rate_value(last: Option<u32>) -> (String, &'static str) {
    match last {
        Some(hz) => (format!("{hz} Hz"), tr(T::MeasuredMoving)),
        None => (String::new(), tr(T::MoveMouse)),
    }
}

/// Big DPI number with its unit, as the head of an editor panel.
fn dpi_number(f: &mut Frame, x: f32, y: f32, dpi: Option<u32>) {
    const UNIT: &str = "DPI";
    let Some(d) = dpi else {
        skeleton(f.g, Rect::new(x, y + 8.0, 96.0, 22.0));
        return;
    };
    let value = d.to_string();
    f.g.text(&value, Rect::new(x, y, 200.0, 36.0), Size::Hero, Weight::Medium, Align::Left, th().text);
    let unit_x = x + text_w(&value, Size::Hero) + 6.0;
    f.g.text(UNIT, Rect::new(unit_x, y + 14.0, 60.0, 16.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
}

/// Slider with its scale underneath: `labels` spread evenly from end to end.
fn dpi_slider(f: &mut Frame, act: Act, r: Rect, index: usize, steps: usize, labels: &[u32]) {
    let (x0, x1, cy) = (r.x + 5.0, r.x + r.w - 5.0, r.y + 10.0);
    let kx = x0 + (x1 - x0) * index as f32 / (steps - 1).max(1) as f32;
    slider_mark(f.g, x0, x1, kx, cy, th().acc_fill);
    f.hits.push(Hit { act, rect: Rect::new(r.x - 5.0, r.y - 4.0, r.w + 10.0, 28.0), kind: Kind::Slider { x0, x1, steps } });

    let last = labels.len().saturating_sub(1).max(1) as f32;
    for (k, value) in labels.iter().enumerate() {
        let x = x0 + (x1 - x0) * k as f32 / last;
        let (rect, align) = match k {
            0 => (Rect::new(x - 5.0, r.y + 26.0, 80.0, 16.0), Align::Left),
            _ if k == labels.len() - 1 => (Rect::new(x - 75.0, r.y + 26.0, 80.0, 16.0), Align::Right),
            _ => (Rect::new(x - 40.0, r.y + 26.0, 80.0, 16.0), Align::Center),
        };
        f.g.text(&value.to_string(), rect, Size::Key, Weight::Medium, align, th().muted);
    }
}

/// One DPI value for driver-based mice: big number and a slider.
fn dpi_single(f: &mut Frame, v: &View) {
    let panel = Rect::new(X0, TOP, CW, PANEL_HEAD + 116.0);
    let body = f.panel(panel, tr(T::ActiveDpi));
    dpi_number(f, body.x + PANEL_PAD, body.y + PANEL_PAD, v.basic.dpi);

    let vals = &v.basic.values;
    if vals.len() < 2 {
        return;
    }
    let idx = v.basic.dpi.and_then(|d| vals.iter().position(|&x| x >= d)).unwrap_or(0);
    let track = Rect::new(body.x + PANEL_PAD, body.y + 62.0, body.w - 2.0 * PANEL_PAD, 20.0);
    dpi_slider(f, Act::BasicDpi, track, idx, vals.len(), &[vals[0], vals[vals.len() - 1]]);
}

/// DPI stages: the list (a click makes a stage the active one, the box
/// on the right keeps it in the cycle of the DPI button), and the editor
/// of the active stage below.
///
/// ```text
///  Stages
///  Stage 1   1000  ━━──────────────────── | [x]
///  Stage 3   2400  ━━━━━━━━────────────── | [x]   active: tinted, bar on the left
///  Stage 3                                 Color ■
///  2400 DPI                          [-] [+]
///  ━━━━━━━━━━▮───────────────────────────────
///  200     2600      5000      7400     10000
/// ```
fn dpi(f: &mut Frame, v: &View) {
    if v.caps.dpi == DpiUi::Single {
        dpi_single(f, v);
        return;
    }
    const ROW: f32 = 32.0;
    const CHECK_W: f32 = 44.0;
    const NAME_W: f32 = 84.0;
    const VALUE_W: f32 = 56.0;
    const BAR_H: f32 = 4.0;
    let panel = Rect::new(X0, TOP, CW, PANEL_HEAD + ROW * v.stages.len() as f32);
    let body = f.panel(panel, tr(T::Stages));
    f.panel_note(panel, tr(T::ColCycle));
    let top_dpi = protocol_dpi(v.steps.saturating_sub(1)).max(1);

    for (i, s) in v.stages.iter().enumerate() {
        let y = body.y + i as f32 * ROW;
        let active = i == v.active;
        let row = Rect::new(body.x, y, body.w - CHECK_W, ROW);
        // A stage out of the cycle is switched on first; it cannot be active.
        let act = if s.enabled { Act::Activate(i) } else { Act::Toggle(i) };
        let hot = !active && f.button(act, row);
        if active {
            f.g.fill(Rect::new(body.x + 1.0, y, body.w - 2.0, ROW), 0.0, th().acc_soft);
            f.g.fill(Rect::new(body.x, y, 2.0, ROW), 0.0, th().acc_fill);
        } else if hot {
            f.g.fill(Rect::new(body.x + 1.0, y, row.w - 1.0, ROW), 0.0, th().sel);
        }
        if i > 0 {
            f.g.fill(Rect::new(body.x + 1.0, y, body.w - 2.0, 1.0), 0.0, th().line);
        }

        let ink = if s.enabled { th().text } else { th().line2 };
        let weight = if active { Weight::Semibold } else { Weight::Normal };
        let name = fill(tr(T::StageN), &[&(i + 1).to_string()]);
        f.g.text(&name, Rect::new(row.x + PANEL_PAD, y, NAME_W, ROW), Size::Body, weight, Align::Left, ink);
        let value_x = row.x + PANEL_PAD + NAME_W;
        f.g.text(&s.dpi.to_string(), Rect::new(value_x, y, VALUE_W, ROW), Size::Value, Weight::Semibold, Align::Left, ink);

        // The value against the sensor's top DPI.
        let bar = Rect::new(value_x + VALUE_W + 8.0, y + (ROW - BAR_H) / 2.0, row.x + row.w - PANEL_PAD - (value_x + VALUE_W + 8.0), BAR_H);
        let bar_c = if active { th().acc_fill } else if s.enabled { th().line2 } else { th().line };
        f.g.fill(bar, 0.0, th().track);
        f.g.fill(Rect::new(bar.x, bar.y, bar.w * s.dpi as f32 / top_dpi as f32, BAR_H), 0.0, bar_c);

        // In the cycle or not; the active stage always is.
        let cell_x = body.x + body.w - CHECK_W;
        f.g.fill(Rect::new(cell_x, y, 1.0, ROW), 0.0, th().line);
        let toggle_act = (!active).then_some(Act::Toggle(i));
        f.toggle(toggle_act, cell_x + (CHECK_W - 14.0) / 2.0, y + (ROW - 22.0) / 2.0, s.enabled);
    }

    let Some(s) = v.stages.get(v.active) else {
        return;
    };
    let editor = Rect::new(X0, panel.y + panel.h + PANEL_GAP, CW, PANEL_HEAD + 124.0);
    let body = f.panel(editor, &fill(tr(T::StageN), &[&(v.active + 1).to_string()]));

    // Indicator color of the stage; a click steps through the palette.
    const SWATCH: f32 = 16.0;
    let sx = editor.x + editor.w - PANEL_PAD - SWATCH;
    let sy = editor.y + (PANEL_HEAD - SWATCH) / 2.0;
    let hot = f.button(Act::DpiColor(v.active), Rect::new(sx - 4.0, sy - 4.0, SWATCH + 8.0, SWATCH + 8.0));
    f.g.fill(Rect::new(sx, sy, SWATCH, SWATCH), 0.0, Color::rgb(v.dpi_colors[v.active]));
    f.g.stroke(Rect::new(sx, sy, SWATCH, SWATCH), 0.0, 1.0, if hot { th().text } else { th().line2 });
    let caption = Rect::new(editor.x + editor.w / 2.0, editor.y, editor.w / 2.0 - PANEL_PAD - SWATCH - 10.0, PANEL_HEAD - 1.0);
    f.g.text(tr(T::ColorCaption), caption, Size::Caption, Weight::Normal, Align::Right, th().muted);

    dpi_number(f, body.x + PANEL_PAD, body.y + PANEL_PAD, Some(s.dpi));
    // One step down or up, for values the slider is too coarse for.
    const STEP_W: f32 = 28.0;
    let right = body.x + body.w - PANEL_PAD;
    let by = body.y + PANEL_PAD + 4.0;
    f.push_button(Act::StageNudge(-1), Rect::new(right - 2.0 * STEP_W - 6.0, by, STEP_W, 28.0), "−", false);
    f.push_button(Act::StageNudge(1), Rect::new(right - STEP_W, by, STEP_W, 28.0), "+", false);

    let last = v.steps.saturating_sub(1);
    let labels: Vec<u32> = (0..=4).map(|k| protocol_dpi(last * k / 4)).collect();
    let track = Rect::new(body.x + PANEL_PAD, body.y + 66.0, body.w - 2.0 * PANEL_PAD, 20.0);
    dpi_slider(f, Act::Slider(v.active), track, s.index, v.steps, &labels);
}

/// DPI of a step of the sensor's table.
fn protocol_dpi(index: usize) -> u32 {
    crate::protocol::dpi_of(index)
}

/// Performance: one panel of setting rows. Polling rate for every
/// mouse that can set it; debounce, lift-off distance and the sleep
/// timer for those that have them.
fn performance(f: &mut Frame, v: &View) {
    const TWO_LINE_H: f32 = 42.0;
    const NARROW_CELL: f32 = 60.0;
    // Under the rate segments: what Raw Input measures right now.
    const MEASURED_H: f32 = 8.0 + 18.0;
    let (x, w) = (X0, CW);

    let polling = SettingRow { title: tr(T::PollingRate), desc: tr(T::PollingHelp), control_h: TWO_LINE_H + MEASURED_H };
    let debounce = SettingRow { title: tr(T::Debounce), desc: tr(T::DebounceHelp), control_h: SEGMENT_H };
    let lift_off = SettingRow { title: tr(T::LiftOff), desc: "", control_h: SEGMENT_H };
    let minutes = fill(tr(T::Minutes), &[""]);
    let sleep = SettingRow { title: tr(T::SleepAfter), desc: minutes.trim(), control_h: SEGMENT_H };

    let extras = if v.caps.extras { debounce.height(w) + lift_off.height(w) + sleep.height(w) } else { 0.0 };
    f.card(Rect::new(x, TOP, w, polling.height(w) + extras));

    // Rates: the Lionel family's four, else what the mouse reported.
    let mut y = TOP;
    let c = f.row(x, y, w, &polling, RowPlace::First);
    let rates: Vec<Segment> = if v.caps.extras {
        RATES.iter().enumerate().map(|(i, hz)| Segment { act: Act::Polling(i), label: format!("{hz} Hz"), sub: format!("{} ms", 1000 / hz), selected: v.polling == i, kind: Label::Value }).collect()
    } else {
        let rate = |hz: &u32| {
            let ms = format!("{:.1} ms", 1000.0 / *hz as f32).replace(".0 ", " ");
            Segment { act: Act::BasicRate(*hz), label: format!("{hz} Hz"), sub: ms, selected: v.basic.rate == Some(*hz), kind: Label::Value }
        };
        v.basic.rates.iter().map(rate).collect()
    };
    f.segments(Rect::new(c.x, c.y, c.w, TWO_LINE_H), &rates);
    let line = fill(tr(T::MeasuredValue), &[&measured(v)]);
    f.g.text(&line, Rect::new(c.x, c.y + TWO_LINE_H + 8.0, c.w, 18.0), Size::Caption, Weight::Semibold, Align::Left, th().text);
    y += polling.height(w);
    if !v.caps.extras {
        return;
    }

    let c = f.row(x, y, w, &debounce, RowPlace::Next);
    let items: Vec<Segment> = DEBOUNCE_CHOICES.iter().enumerate().map(|(i, ms)| Segment::new(Act::Debounce(i), format!("{ms} ms"), v.debounce == *ms)).collect();
    f.segments(c, &items);
    y += debounce.height(w);

    let c = f.row(x, y, w, &lift_off, RowPlace::Next);
    let items: Vec<Segment> = [Lod::Mm1, Lod::Mm2].into_iter().map(|lod| Segment::new(Act::Lod(lod), format!("{} mm", lod as u8), v.lod == lod)).collect();
    f.segments(Rect::new(c.x, c.y, NARROW_CELL * items.len() as f32, c.h), &items);
    y += lift_off.height(w);

    let c = f.row(x, y, w, &sleep, RowPlace::Next);
    let items: Vec<Segment> = SLEEP_CHOICES.iter().enumerate().map(|(i, m)| Segment::new(Act::Sleep(i), m.to_string(), v.power.sleep_min == *m)).collect();
    f.segments(c, &items);
}

/// Color swatch: a bordered square; the selected one gets a ring in the
/// text color at a 2 px gap.
fn swatch(f: &mut Frame, act: Act, cx: f32, cy: f32, color: u32, selected: bool) {
    const HALF: f32 = 11.0;
    const GAP: f32 = 3.0;
    let hot = f.button(act, Rect::new(cx - 15.0, cy - 15.0, 30.0, 30.0));
    if selected || hot {
        f.g.frame(cx, cy, HALF + GAP, 1.0, if selected { th().text } else { th().muted });
    }
    f.g.dot(cx, cy, HALF, Color::rgb(color));
    f.g.frame(cx, cy, HALF, 1.0, th().line2);
}

/// Lighting: one panel of setting rows. Rows an effect has no use for
/// are left out, not greyed.
fn lighting(f: &mut Frame, v: &View) {
    const EFFECT_COLS: usize = 3;
    const NARROW_CELL: f32 = 44.0;
    const SWATCH_STEP: f32 = 34.0;
    const SWATCH_H: f32 = 28.0;
    const CHECK_H: f32 = 22.0;
    let l = &v.lighting;
    let (x, w) = (X0, CW);

    let effect_rows = Effect::ALL.len().div_ceil(EFFECT_COLS) as f32;
    let effect = SettingRow { title: tr(T::EffectCaption), desc: "", control_h: effect_rows * (SEGMENT_H - 1.0) + 1.0 };
    let brightness = SettingRow { title: tr(T::Brightness), desc: "", control_h: SEGMENT_H };
    let speed = SettingRow { title: tr(T::Speed), desc: "", control_h: SEGMENT_H };
    let direction = SettingRow { title: tr(T::DirectionCaption), desc: "", control_h: SEGMENT_H };
    let multi = l.effect == Effect::Breathing;
    let colors = SettingRow { title: tr(if multi { T::ColorsCaption } else { T::ColorCaption }), desc: "", control_h: SWATCH_H };
    let dark = SettingRow { title: tr(T::DarkMoving), desc: tr(T::LightingNote), control_h: CHECK_H };

    let has_colors = matches!(l.effect, Effect::Steady | Effect::Breathing);
    let shown: [(&SettingRow, bool); 6] =
        [(&effect, true), (&brightness, true), (&speed, l.effect.has_speed()), (&direction, l.effect == Effect::Streaming), (&colors, has_colors), (&dark, true)];
    let total: f32 = shown.iter().filter(|(_, on)| *on).map(|(r, _)| r.height(w)).sum();
    f.card(Rect::new(x, TOP, w, total));

    // Effects: a grid of segments, four to a line.
    let mut y = TOP;
    let c = f.row(x, y, w, &effect, RowPlace::First);
    for (line, chunk) in Effect::ALL.chunks(EFFECT_COLS).enumerate() {
        let first = line * EFFECT_COLS;
        let mut items: Vec<Segment> = chunk.iter().enumerate().map(|(k, e)| Segment::named(Act::Effect(first + k), tr(EFFECT_NAMES[first + k]), l.effect == *e)).collect();
        // A short last line keeps the cell width of the full ones.
        let cell_w = c.w / EFFECT_COLS as f32;
        let r = Rect::new(c.x, c.y + line as f32 * (SEGMENT_H - 1.0), cell_w * items.len() as f32, SEGMENT_H);
        f.segments(r, &items);
        items.clear();
    }
    y += effect.height(w);

    let c = f.row(x, y, w, &brightness, RowPlace::Next);
    let items: Vec<Segment> = (1..=BRIGHTNESS_MAX).map(|b| Segment::new(Act::Brightness(b), b.to_string(), l.brightness == b)).collect();
    f.segments(Rect::new(c.x, c.y, NARROW_CELL * items.len() as f32, c.h), &items);
    y += brightness.height(w);

    if l.effect.has_speed() {
        let c = f.row(x, y, w, &speed, RowPlace::Next);
        let items: Vec<Segment> = (1..=SPEED_MAX).map(|sp| Segment::new(Act::Speed(sp), sp.to_string(), l.speed == sp)).collect();
        f.segments(Rect::new(c.x, c.y, NARROW_CELL * items.len() as f32, c.h), &items);
        y += speed.height(w);
    }

    if l.effect == Effect::Streaming {
        let c = f.row(x, y, w, &direction, RowPlace::Next);
        let dirs = [(Direction::Forward, "→", T::DirForward), (Direction::Reverse, "←", T::DirReverse)];
        let items: Vec<Segment> = dirs.into_iter().map(|(d, arrow, name)| Segment::named(Act::Direction(d), format!("{arrow} {}", tr(name)), l.direction == d)).collect();
        f.segments(Rect::new(c.x, c.y, c.w / 2.0, c.h), &items);
        y += direction.height(w);
    }

    // Colors: one for steady, several for breathing.
    if has_colors {
        let c = f.row(x, y, w, &colors, RowPlace::Next);
        for (i, &color) in PALETTE.iter().enumerate() {
            let cx = c.x + SWATCH_H / 2.0 + i as f32 * SWATCH_STEP;
            let (act, sel) = if multi { (Act::Breath(i), l.breathing.contains(&color)) } else { (Act::Steady(i), l.steady == color) };
            swatch(f, act, cx, c.y + SWATCH_H / 2.0, color, sel);
        }
        y += colors.height(w);
    }

    // Lights off while moving, with the wake-up note as its description.
    let c = f.row(x, y, w, &dark, RowPlace::Next);
    f.toggle(Some(Act::DarkMoving), c.x, c.y, v.power.dark_while_moving);
}

/// Statistics: the period, six numbers in two strips, the battery chart.
fn stats(f: &mut Frame, v: &View) {
    const PERIOD_CELL: f32 = 104.0;
    const STRIP_H: f32 = 74.0;
    const STRIP_CELLS: usize = 3;
    let periods: Vec<Segment> = PERIODS.iter().enumerate().map(|(i, name)| Segment::named(Act::Period(i), tr(*name), v.period == i)).collect();
    f.segments(Rect::new(X0, TOP, PERIOD_CELL * periods.len() as f32, SEGMENT_H), &periods);

    // "36 m" is a number and its unit; counts have none.
    let mut y = TOP + SEGMENT_H + PANEL_GAP;
    for row in v.tiles.chunks(STRIP_CELLS) {
        let cells: Vec<Stat> = row
            .iter()
            .map(|(label, value)| {
                let (number, unit) = split_unit(value);
                Stat { label, value: number, unit, note: String::new(), bar: None }
            })
            .collect();
        stat_strip(f, Rect::new(X0, y, CW, STRIP_H), &cells, Corners::Plain);
        // The strips share the hairline between them.
        y += STRIP_H - 1.0;
    }

    // The chart follows the selected period.
    let title = [T::ChartToday, T::ChartTitle, T::ChartAll][v.period.min(PERIODS.len() - 1)];
    let card = Rect::new(X0, y + 1.0 + PANEL_GAP, CW, PAGE_H - (y + 1.0 + PANEL_GAP) - 12.0);
    let body = f.panel(card, tr(title));
    // History shorter than the period: the axis starts at the first sample.
    if let Some(day) = &v.chart_since {
        f.panel_note(card, &fill(tr(T::ChartSince), &[day]));
    }

    let plot = Rect::new(body.x + PANEL_PAD, body.y + PANEL_PAD + 4.0, body.w - 2.0 * PANEL_PAD - 44.0, body.h - 2.0 * PANEL_PAD - 26.0);
    if v.chart.len() < 2 {
        let msg = tr(T::ChartEmpty);
        f.g.text(msg, plot, Size::Body, Weight::Normal, Align::Center, th().subtle);
        return;
    }

    for (pct, label) in [(100.0, "100 %"), (50.0, "50 %"), (0.0, "0 %")] {
        let y = plot.y + plot.h * (1.0 - pct / 100.0);
        f.g.line(plot.x, y, plot.x + plot.w, y, 1.0, th().border);
        let r = Rect::new(plot.x + plot.w + 6.0, y - 8.0, 40.0, 16.0);
        f.g.text(label, r, Size::Caption, Weight::Normal, Align::Left, th().subtle);
    }

    // Time axis: a tick and a label per day (hours for "today").
    const LABEL_W: f32 = 60.0;
    for (t, label) in &v.chart_ticks {
        let x = plot.x + t * plot.w;
        f.g.line(x, plot.y + plot.h, x, plot.y + plot.h + 4.0, 1.0, th().border);
        let lx = (x - LABEL_W / 2.0).clamp(plot.x - 8.0, plot.x + plot.w + 8.0 - LABEL_W);
        let r = Rect::new(lx, plot.y + plot.h + 6.0, LABEL_W, 16.0);
        f.g.text(label, r, Size::Caption, Weight::Normal, Align::Center, th().subtle);
    }

    let pts: Vec<(f32, f32)> = v.chart.iter().map(|&(t, pct)| (plot.x + t * plot.w, plot.y + plot.h * (1.0 - pct / 100.0))).collect();
    f.g.polyline(&pts, 1.5, th().acc_fill);
}

/// Settings › General: one panel of setting rows (install, start at
/// sign-in, update check), then where the app lives while its window is
/// closed and where its data is.
fn settings(f: &mut Frame, v: &View) {
    const BTN_H: f32 = 28.0;
    const BTN_W: f32 = 150.0;
    const CHECK_H: f32 = 22.0;
    const ERROR_H: f32 = 22.0;
    let (x, w) = (X0, CW);

    let title = if v.installed { tr(T::Installed) } else { tr(T::NotInstalled) };
    let desc = if v.installed { v.install_dir.clone() } else { fill(tr(T::InstallDesc), &[&v.install_dir]) };
    let error_h = if v.error.is_empty() { 0.0 } else { ERROR_H };
    let install = SettingRow { title, desc: &desc, control_h: BTN_H + error_h };
    // Autostart only points at the installed copy: disabled, and why.
    let auto_desc = if v.installed { tr(T::AutostartDesc).to_string() } else { format!("{} {}", tr(T::AutostartDesc), tr(T::AutostartLater)) };
    let autostart = SettingRow { title: tr(T::Autostart), desc: &auto_desc, control_h: CHECK_H };
    let update = SettingRow { title: tr(T::UpdateAuto), desc: tr(T::UpdateAutoDesc), control_h: CHECK_H };
    let language = SettingRow { title: tr(T::Language), desc: "", control_h: BTN_H };

    let panel = Rect::new(x, BODY, w, install.height(w) + autostart.height(w) + update.height(w) + language.height(w));
    f.card(panel);

    let mut y = BODY;
    let c = f.row(x, y, w, &install, RowPlace::First);
    let btn = Rect::new(c.x, c.y, BTN_W, BTN_H);
    if v.installed {
        f.push_button(Act::Uninstall, btn, tr(T::Uninstall), false);
    } else {
        f.push_button(Act::Install, btn, tr(T::Install), true);
    }
    if !v.error.is_empty() {
        f.g.text(&v.error, Rect::new(c.x + BTN_W + 12.0, c.y, c.w - BTN_W - 12.0, BTN_H), Size::Caption, Weight::Normal, Align::Left, th().bad);
    }
    y += install.height(w);

    let c = f.row(x, y, w, &autostart, RowPlace::Next);
    f.toggle(v.installed.then_some(Act::Autostart), c.x, c.y, v.autostart);
    y += autostart.height(w);

    let c = f.row(x, y, w, &update, RowPlace::Next);
    f.toggle(Some(Act::UpdateAuto), c.x, c.y, v.update.auto);
    y += update.height(w);

    // Language: a select with the languages by their own names.
    let c = f.row(x, y, w, &language, RowPlace::Next);
    let select = Rect::new(c.x, c.y, LANG_SELECT_W, BTN_H);
    select_box(f, Act::LangMenu, select, lang_names()[v.lang_pick.min(Lang::ALL.len())], v.lang_menu);

    // Tray behaviour and where data lives.
    let note = Rect::new(x, panel.y + panel.h + PANEL_GAP, w, 76.0);
    f.card(note);
    let r = Rect::new(note.x + PANEL_PAD, note.y + PANEL_PAD, note.w - 2.0 * PANEL_PAD, 34.0);
    f.g.text(tr(T::AboutTray), r, Size::Caption, Weight::Normal, Align::Wrap, th().muted);
    let r = Rect::new(note.x + PANEL_PAD, note.y + 50.0, note.w - 2.0 * PANEL_PAD, 18.0);
    f.g.text(&fill(tr(T::DataDir), &[&v.data_dir]), r, Size::Caption, Weight::Normal, Align::Left, th().muted);

    // Drawn last: the open list lies over the rest of the page.
    if v.lang_menu {
        lang_list(f, v, select);
    }
}

/// Width of the language select and of a column of its list.
const LANG_SELECT_W: f32 = 190.0;

/// "System" and the languages by their own names, in the order of `Act::Lang`.
fn lang_names() -> Vec<&'static str> {
    std::iter::once(tr(T::FollowSystem)).chain(Lang::ALL.iter().map(|l| l.native())).collect()
}

/// A select box: the chosen value and a chevron; a press opens its list.
fn select_box(f: &mut Frame, act: Act, r: Rect, label: &str, open: bool) {
    let hot = f.button(act, r);
    f.g.fill(r, 0.0, th().input);
    f.g.stroke(r, 0.0, 1.0, if hot || open { th().acc } else { th().line2 });
    f.g.text(label, Rect::new(r.x + 10.0, r.y, r.w - 40.0, r.h), Size::Body, Weight::Normal, Align::Left, th().text);
    glyph(f.g, Glyph::ChevronDown, r.x + r.w - 15.0, r.y + r.h / 2.0, th().muted);
}

/// The open language list: two columns over the page, above the select
/// when there is no room below. A press anywhere else closes it.
fn lang_list(f: &mut Frame, v: &View, select: Rect) {
    const ROW: f32 = 26.0;
    const COLS: usize = 2;
    let names = lang_names();
    let rows = names.len().div_ceil(COLS);
    let (w, h) = (COLS as f32 * LANG_SELECT_W, rows as f32 * ROW + 2.0);
    let below = select.y + select.h + 2.0;
    let y = if below + h <= PAGE_H { below } else { select.y - h - 2.0 };
    let list = Rect::new(select.x, y, w, h);

    // Under the list: the whole window closes it.
    f.button(Act::LangMenu, Rect::new(0.0, TITLE_H, WIDTH, HEIGHT - TITLE_H));
    f.g.fill(list, 0.0, th().panel);
    f.g.stroke(list, 0.0, 1.0, th().line2);
    for (i, name) in names.iter().enumerate() {
        // Down the first column, then the second.
        let (col, row) = (i / rows, i % rows);
        let r = Rect::new(list.x + 1.0 + col as f32 * LANG_SELECT_W, list.y + 1.0 + row as f32 * ROW, LANG_SELECT_W - 2.0, ROW);
        let hot = f.button(Act::Lang(i), r);
        let selected = v.lang_pick == i;
        if selected {
            f.g.fill(r, 0.0, th().acc_soft);
            f.g.fill(Rect::new(r.x, r.y, 2.0, r.h), 0.0, th().acc_fill);
        } else if hot {
            f.g.fill(r, 0.0, th().sel);
        }
        let weight = if selected { Weight::Semibold } else { Weight::Normal };
        f.g.text(name, Rect::new(r.x + 12.0, r.y, r.w - 16.0, r.h), Size::Body, weight, Align::Left, th().text);
    }
}

fn setup_dialog(f: &mut Frame, v: &View) {
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);

    let d = Rect::new((WIDTH - 460.0) / 2.0, (HEIGHT - 290.0) / 2.0, 460.0, 290.0);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let inner = d.inset(24.0);
    f.g.text(tr(T::SetupTitle), Rect::new(inner.x, inner.y, inner.w, 28.0), Size::Subtitle, Weight::Bold, Align::Left, th().text);
    let text = fill(tr(T::SetupText), &[&v.install_dir]);
    f.g.text(&text, Rect::new(inner.x, inner.y + 38.0, inner.w, 64.0), Size::Body, Weight::Normal, Align::Wrap, th().muted);

    let ty = inner.y + 118.0;
    f.toggle(Some(Act::SetupAutostart), inner.x, ty, v.setup_autostart);
    let label = Rect::new(inner.x + 52.0, ty - 6.0, inner.w - 52.0, 34.0);
    f.g.text(tr(T::SetupAutostart), label, Size::Body, Weight::Normal, Align::Left, th().text);

    let by = d.y + d.h - 24.0 - 36.0;
    f.push_button(Act::SetupSkip, Rect::new(d.x + d.w - 24.0 - 128.0 - 10.0 - 120.0, by, 120.0, 36.0), tr(T::JustRun), false);
    f.push_button(Act::SetupInstall, Rect::new(d.x + d.w - 24.0 - 128.0, by, 128.0, 36.0), tr(T::Install), true);
}

/// The last run ended badly: what the log is for, and a button that
/// shows it. Nothing leaves the computer on its own.
fn crash_dialog(f: &mut Frame) {
    const W: f32 = 460.0;
    const H: f32 = 250.0;
    const COPY_W: f32 = 180.0;
    const CLOSE_W: f32 = 110.0;
    f.g.fill(Rect::new(0.0, 0.0, WIDTH, HEIGHT), 0.0, th().scrim);
    let d = Rect::new((WIDTH - W) / 2.0, (HEIGHT - H) / 2.0, W, H);
    f.g.fill(d, RADIUS, th().card_hi);
    f.g.stroke(d, RADIUS, 1.0, th().border);

    let inner = d.inset(24.0);
    f.g.text(tr(T::CrashTitle), Rect::new(inner.x, inner.y, inner.w, 28.0), Size::Subtitle, Weight::Bold, Align::Left, th().text);
    let text = Rect::new(inner.x, inner.y + 38.0, inner.w, 104.0);
    f.g.text(tr(T::CrashText), text, Size::Body, Weight::Normal, Align::Wrap, th().muted);

    let by = d.y + d.h - 24.0 - 36.0;
    let right = d.x + d.w - 24.0;
    f.push_button(Act::CloseDialog, Rect::new(right - COPY_W - 10.0 - CLOSE_W, by, CLOSE_W, 36.0), tr(T::Close), false);
    f.push_button(Act::ShowLog, Rect::new(right - COPY_W, by, COPY_W, 36.0), tr(T::ShowLog), true);
}

/// Settings › About: logo, name and version, copyright, license,
/// project links, update check, and the way to report a mouse.
fn about_page(f: &mut Frame, v: &View) {
    const H: f32 = 296.0;
    const LOGO: f32 = 32.0;
    let top = pro::backup_panel(f, BODY) + PANEL_GAP;
    let panel = Rect::new(X0, top, CW, H);
    f.card(panel);
    let inner = panel.inset(16.0);

    logo(f.g, inner.x, inner.y, LOGO);
    let name = crate::APP_NAME.to_uppercase();
    let name_x = inner.x + LOGO + 12.0;
    f.g.text(&name, Rect::new(name_x, inner.y - 2.0, 200.0, 22.0), Size::Title, Weight::Semibold, Align::Left, th().text);
    let ver_x = name_x + text_w(&name, Size::Title) + 12.0;
    f.g.text(VERSION, Rect::new(ver_x, inner.y, 120.0, 20.0), Size::Value, Weight::Medium, Align::Left, th().muted);
    f.g.text(tr(T::AboutTagline), Rect::new(name_x, inner.y + 20.0, inner.w - LOGO - 12.0, 18.0), Size::Caption, Weight::Normal, Align::Left, th().muted);

    let y = inner.y + 58.0;
    f.g.text(COPYRIGHT, Rect::new(inner.x, y, inner.w, 18.0), Size::Caption, Weight::Normal, Align::Left, th().text);
    let license = fill(tr(T::License), &[LICENSE]);
    f.g.text(&license, Rect::new(inner.x, y + 20.0, inner.w, 18.0), Size::Caption, Weight::Normal, Align::Left, th().text);

    // Links go to the web; buttons below are actions.
    const SITE: &str = "whisker.emlynix.com";
    const REPO: &str = "GitHub";
    let x = text_link(f, Act::OpenWeb(Web::Site), SITE, inner.x, y + 42.0);
    f.g.text("·", Rect::new(x + 6.0, y + 42.0, 10.0, 18.0), Size::Caption, Weight::Normal, Align::Left, th().muted);
    text_link(f, Act::OpenWeb(Web::Repo), REPO, x + 18.0, y + 42.0);

    f.g.fill(Rect::new(panel.x + 1.0, y + 72.0, panel.w - 2.0, 1.0), 0.0, th().line);
    update::about_row(f, &v.update, inner.x, y + 86.0, inner.w);
    let privacy = Rect::new(inner.x, y + 144.0, inner.w, 34.0);
    f.g.text(tr(T::UpdatePrivacy), privacy, Size::Caption, Weight::Normal, Align::Wrap, th().muted);

    let report = tr(T::HelpAddMouse);
    let report_w = text_w(report, Size::Body) + 32.0;
    f.push_button(Act::ReportOpen, Rect::new(inner.x, panel.y + panel.h - 16.0 - 28.0, report_w, 28.0), report, false);
}

/// Accent text that opens `act`, underlined on hover; returns its right edge.
fn text_link(f: &mut Frame, act: Act, label: &str, x: f32, y: f32) -> f32 {
    let r = Rect::new(x, y, text_w(label, Size::Caption) * 1.12 + 4.0, 18.0);
    let hot = f.button(act, r);
    let c = if hot { th().text } else { th().accent };
    f.g.text(label, r, Size::Caption, Weight::Semibold, Align::Left, c);
    if hot {
        f.g.fill(Rect::new(r.x, r.y + r.h - 1.0, r.w - 4.0, 1.0), 0.0, c);
    }
    r.x + r.w
}

// ---- appearance ------------------------------------------------------------

/// Picker tile of a theme in its own colors: a small picture of the
/// window (sidebar, three bars for text, line and accent), the name
/// below. Selected: an accent border with a second outline inside.
fn theme_tile(f: &mut Frame, act: Act, r: Rect, t: &Theme, name: &str, selected: bool) {
    const PREVIEW_H: f32 = 44.0;
    const SIDE: f32 = 0.24;
    let hot = f.button(act, r);
    let preview = Rect::new(r.x, r.y, r.w, PREVIEW_H);
    f.g.fill(preview, 0.0, t.bg);
    let side_w = (r.w * SIDE).round();
    f.g.fill(Rect::new(r.x, r.y, side_w, PREVIEW_H), 0.0, t.side);
    f.g.fill(Rect::new(r.x + side_w, r.y, 1.0, PREVIEW_H), 0.0, t.line);
    let bx = r.x + side_w + 9.0;
    let bw = r.w - side_w - 18.0;
    for (k, (c, part)) in [(t.text, 0.55), (t.line2, 0.85), (t.acc_fill, 0.4)].into_iter().enumerate() {
        f.g.fill(Rect::new(bx, r.y + 10.0 + k as f32 * 10.0, bw * part, 4.0), 0.0, c);
    }

    f.g.fill(Rect::new(r.x, r.y + PREVIEW_H, r.w, r.h - PREVIEW_H), 0.0, th().input);
    f.g.fill(Rect::new(r.x, r.y + PREVIEW_H, r.w, 1.0), 0.0, th().line);
    let label = Rect::new(r.x + 8.0, r.y + PREVIEW_H, r.w - 16.0, r.h - PREVIEW_H);
    let weight = if selected { Weight::Semibold } else { Weight::Normal };
    f.g.text(name, label, Size::Caption, weight, Align::Left, th().text);

    f.g.stroke(r, 0.0, 1.0, if selected || hot { th().acc } else { th().line2 });
    if selected {
        f.g.stroke(r.inset(1.0), 0.0, 1.0, th().acc);
    }
}

/// Settings › Appearance: theme tiles and the effects.
fn appearance(f: &mut Frame, v: &View) {
    const TILE_GAP: f32 = 8.0;
    const TILE_H: f32 = 70.0;

    // Themes: "system" first, shown in the colors it resolves to now.
    let tiles = THEMES.len() + 1;
    let panel = Rect::new(X0, BODY, CW, PANEL_HEAD + 2.0 * PANEL_PAD + TILE_H);
    let body = f.panel(panel, tr(T::Theme));
    let tile_w = (body.w - 2.0 * PANEL_PAD - (tiles - 1) as f32 * TILE_GAP) / tiles as f32;
    let tile = |i: usize| Rect::new(body.x + PANEL_PAD + i as f32 * (tile_w + TILE_GAP), body.y + PANEL_PAD, tile_w, TILE_H);
    theme_tile(f, Act::Theme(0), tile(0), th(), tr(T::FollowSystem), v.theme_pick == 0);
    for (i, t) in THEMES.iter().enumerate() {
        theme_tile(f, Act::Theme(i + 1), tile(i + 1), t, t.name, v.theme_pick == i + 1);
    }

    // Effects: both on by default; everything stops while minimized.
    const CHECK_ROW: f32 = 26.0;
    let panel = Rect::new(X0, panel.y + panel.h + PANEL_GAP, CW, PANEL_HEAD + 2.0 * PANEL_PAD + 2.0 * CHECK_ROW - 4.0);
    let body = f.panel(panel, tr(T::Effects));
    f.panel_note(panel, tr(T::FxNote));
    for (k, (act, label, on)) in [(Act::FxMotion, T::FxMotion, v.fx_motion), (Act::FxTexture, T::FxTexture, v.fx_texture)].into_iter().enumerate() {
        let y = body.y + PANEL_PAD + k as f32 * CHECK_ROW;
        f.toggle(Some(act), body.x + PANEL_PAD, y, on);
        let label_r = Rect::new(body.x + PANEL_PAD + 24.0, y, body.w - 2.0 * PANEL_PAD - 24.0, 22.0);
        f.g.text(tr(label), label_r, Size::Body, Weight::Normal, Align::Left, th().text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_mouse_name_wraps_in_two_lines() {
        const W: f32 = 124.0; // sidebar name width
        let (first, second) = name_lines("Logitech Marathon Mouse/Performance Plus M705", W, Size::Caption);
        assert_eq!(first, "Logitech Marathon");
        assert_eq!(second.as_deref(), Some("Mouse/Performance Plus M705"));
        assert!(text_w(&first, Size::Caption) <= W);

        assert_eq!(name_lines("White Shark Lionel", W, Size::Caption), ("White Shark Lionel".into(), None));

        // One unbreakable word: left to the "…" trimming.
        let long = "Superlongmodelnamewithoutspaces";
        assert_eq!(name_lines(long, W, Size::Caption), (long.into(), None));
    }

    #[test]
    fn rate_card_keeps_the_last_measurement() {
        // Before any motion: a hint, not a loading skeleton.
        assert_eq!(rate_value(None), (String::new(), tr(T::MoveMouse)));
        assert_eq!(rate_value(Some(125)), ("125 Hz".into(), tr(T::MeasuredMoving)));
    }
}
