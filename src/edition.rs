//! The seam another edition of the app attaches to. An executable that
//! calls `run_edition` gets the Overlay page to draw, the presses and
//! keys on it, the live numbers of the mouse, and a few services of the
//! app (its data folder, HTTPS, the clipboard, off-window drawing).
//! Nothing here knows what the edition does with them.
//!
//! ```text
//!  executable ── run_edition(Box<dyn Edition>) ──> app (this library)
//!
//!  app ──> Edition::start               the window exists
//!      ──> Edition::page / text_page    draw the Overlay page
//!      ──> Edition::press / key         input on that page
//!      ──> Edition::quit                the app ends
//!  edition ──> live()                   battery, DPI, rate, clicks
//!          ──> load_text / post_json / Canvas / on_ui …
//! ```
//!
//! Without an edition the Overlay page stays the veiled mock-up.

use crate::platform::{Content, Fetch, NetError, net, window};
use crate::{i18n, platform, stats, store};
use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

pub use crate::gfx::{Align, Color, Rect, Size, Weight};
pub use crate::update::json::{Json, parse as parse_json};
pub use crate::view::kit::{CHECK_H, Choice, Mark, Order, PANEL_GAP, PANEL_HEAD, PANEL_PAD, Palette, Rank, Row, SEGMENT_H, Ui, palette, text_w};
pub use crate::view::{TextBox, TextStyle, TextUi};

/// Longest side of a `Canvas` frame, in pixels.
const CANVAS_MAX: u32 = 8192;
/// Longest name of a text file of the edition.
const NAME_MAX: usize = 64;
/// Most characters `clipboard_text` returns.
const CLIPBOARD_MAX: usize = 256;
/// Most bytes of an HTTPS answer.
const REPLY_MAX: usize = 64 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const AGENT: &str = concat!("Whisker/", env!("CARGO_PKG_VERSION"));

/// A key press on the edition's page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key {
    /// Windows virtual-key code.
    pub vk: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Whether the edition took a key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Used {
    Yes,
    No,
}

/// What an edition adds to the app. Every call comes on the UI thread.
pub trait Edition {
    /// The app's window exists and its loop is about to run.
    fn start(&mut self);

    /// Draws the Overlay page in the graphical mode.
    fn page(&mut self, ui: &mut Ui);

    /// Fills the Overlay page in the text mode.
    fn text_page(&mut self, ui: &mut TextUi);

    /// The control with this id was pressed; the window is redrawn after.
    fn press(&mut self, id: u16);

    /// A key went down while the Overlay page was open.
    fn key(&mut self, key: Key) -> Used;

    /// The app is about to end.
    fn quit(&mut self);

    /// Names of the text files this edition keeps in the app's data
    /// folder; uninstalling removes them with the app's own.
    fn data_files(&self) -> Vec<&'static str>;
}

thread_local! {
    /// The attached edition; lives on the UI thread.
    static SLOT: RefCell<Option<Box<dyn Edition>>> = const { RefCell::new(None) };
}

/// An edition is attached (readable from any thread).
static ATTACHED: AtomicBool = AtomicBool::new(false);

pub(crate) fn attach(edition: Box<dyn Edition>) {
    SLOT.with(|s| *s.borrow_mut() = Some(edition));
    ATTACHED.store(true, Ordering::Relaxed);
}

pub(crate) fn attached() -> bool {
    ATTACHED.load(Ordering::Relaxed)
}

/// Runs `f` on the edition; `None` without one, or while it is busy.
pub(crate) fn with<R>(f: impl FnOnce(&mut dyn Edition) -> R) -> Option<R> {
    SLOT.with(|s| s.try_borrow_mut().ok()?.as_mut().map(|e| f(e.as_mut())))
}

/// The numbers of the mouse right now.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Live {
    pub connected: bool,
    /// Percent; `None` for a mouse that reports none.
    pub battery: Option<u8>,
    pub charging: bool,
    pub dpi: Option<u32>,
    /// Report rate the mouse is set to.
    pub rate_hz: Option<u32>,
    /// Report rate measured while the mouse moved, if recent.
    pub measured_hz: Option<u32>,
    /// Presses since the app started: left button, right button, the others.
    pub left: u64,
    pub right: u64,
    pub other: u64,
    /// Sensor counts travelled since the app started.
    pub travel: u64,
}

/// The part of `Live` the controller knows; the rest is counted as it
/// happens.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Slow {
    pub connected: bool,
    pub battery: Option<u8>,
    pub charging: bool,
    pub dpi: Option<u32>,
    pub rate_hz: Option<u32>,
}

static SLOW: Mutex<Slow> = Mutex::new(Slow { connected: false, battery: None, charging: false, dpi: None, rate_hz: None });
static LEFT: AtomicU64 = AtomicU64::new(0);
static RIGHT: AtomicU64 = AtomicU64::new(0);
static OTHER: AtomicU64 = AtomicU64::new(0);
static TRAVEL: AtomicU64 = AtomicU64::new(0);

pub(crate) fn publish(slow: Slow) {
    if let Ok(mut s) = SLOW.lock() {
        *s = slow;
    }
}

/// Presses of one mouse report, by button.
#[derive(Clone, Copy, Default)]
pub(crate) struct Presses {
    pub left: u64,
    pub right: u64,
    pub other: u64,
}

/// One mouse report of the chosen mouse: its presses and its motion.
pub(crate) fn count(presses: Presses, travel: u64) {
    LEFT.fetch_add(presses.left, Ordering::Relaxed);
    RIGHT.fetch_add(presses.right, Ordering::Relaxed);
    OTHER.fetch_add(presses.other, Ordering::Relaxed);
    TRAVEL.fetch_add(travel, Ordering::Relaxed);
}

/// The numbers of the mouse right now. From any thread.
pub fn live() -> Live {
    let slow = SLOW.lock().map(|s| *s).unwrap_or_default();
    Live {
        connected: slow.connected,
        battery: slow.battery,
        charging: slow.charging,
        dpi: slow.dpi,
        rate_hz: slow.rate_hz,
        measured_hz: stats::measured_now(),
        left: LEFT.load(Ordering::Relaxed),
        right: RIGHT.load(Ordering::Relaxed),
        other: OTHER.load(Ordering::Relaxed),
        travel: TRAVEL.load(Ordering::Relaxed),
    }
}

/// Runs `f` on the UI thread. From any thread.
pub fn on_ui(f: impl FnOnce() + Send + 'static) {
    window::post(f);
}

/// Asks for a new frame of the window. UI thread only; from another
/// thread: `on_ui(redraw)`.
pub fn redraw() {
    window::invalidate();
}

/// A plain file name, e.g. `licence.txt`: letters, digits, `.`, `_`
/// and `-` only, so it cannot name a place outside the data folder.
fn plain_name(name: &str) -> bool {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    !name.is_empty() && name.len() <= NAME_MAX && !name.starts_with('.') && name.chars().all(plain)
}

/// A text file in the app's data folder, e.g. `load_text("licence.txt")`.
/// `None` for a name that is not a plain file name.
pub fn load_text(name: &str) -> Option<String> {
    if !plain_name(name) {
        return None;
    }
    store::load_text(name)
}

/// Writes a text file in the app's data folder; a name that is not a
/// plain file name writes nothing.
pub fn save_text(name: &str, text: &str) {
    if !plain_name(name) {
        return;
    }
    store::save_text(name, text);
}

/// Files of the attached edition to delete with the app's data.
pub(crate) fn data_files() -> Vec<&'static str> {
    let names = with(|e| e.data_files()).unwrap_or_default();
    names.into_iter().filter(|n| plain_name(n)).collect()
}

/// Language of the UI as a code, e.g. `"sk"`.
pub fn lang() -> &'static str {
    i18n::lang().code()
}

/// Text on the clipboard, cut to a line's worth. UI thread only.
pub fn clipboard_text() -> Option<String> {
    window::clipboard_text(CLIPBOARD_MAX)
}

/// Opens a page in the user's browser.
pub fn open_url(url: &str) -> bool {
    platform::open_url(url)
}

/// Debug builds started for screenshots: no hardware, no input hook.
pub fn demo() -> bool {
    platform::demo()
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    crate::update::sha256::digest(data)
}

/// Answer of the server to `post_json`.
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// No answer: no connection, TLS error, timeout, or a body too large.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HttpFail;

/// One HTTPS POST of a JSON body to `https://<host><path>`. Blocking:
/// call it off the UI thread. Redirects are not followed.
pub fn post_json(host: &str, path: &str, body: &str) -> Result<HttpReply, HttpFail> {
    let req = Fetch { host, path, agent: AGENT, limit: REPLY_MAX, timeout: HTTP_TIMEOUT };
    match net::post_as(&req, body.as_bytes(), Content::Json) {
        Ok(reply) => Ok(HttpReply { status: reply.status, body: reply.body }),
        Err(NetError::Failed | NetError::TooLarge) => Err(HttpFail),
    }
}

/// Off-window drawing with the app's fonts, for a window of the
/// edition's own. Belongs to the thread that made it.
pub struct Canvas {
    gfx: platform::gfx::Gfx,
}

/// A frame of a `Canvas` while it is drawn; coordinates in DIPs.
pub struct Paint<'a> {
    g: &'a mut platform::gfx::Gfx,
}

impl Canvas {
    pub fn new() -> Option<Canvas> {
        Some(Canvas { gfx: platform::gfx::Gfx::offscreen()? })
    }

    /// Draws a frame of `w` x `h` physical pixels on a clear ground, with
    /// `scale` physical pixels to a DIP. Returns its pixels: B, G, R, A
    /// bytes with the alpha multiplied in, rows from the top.
    pub fn render(&mut self, w: u32, h: u32, scale: f32, draw: impl FnOnce(&mut Paint)) -> Option<&[u8]> {
        let sized = (1..=CANVAS_MAX).contains(&w) && (1..=CANVAS_MAX).contains(&h);
        if !sized || !self.gfx.begin_offscreen(w, h, scale) {
            return None;
        }
        draw(&mut Paint { g: &mut self.gfx });
        self.gfx.end_offscreen()
    }
}

impl Paint<'_> {
    pub fn text(&mut self, s: &str, r: Rect, size: Size, weight: Weight, align: Align, c: Color) {
        self.g.text(s, r, size, weight, align, c);
    }

    pub fn fill(&mut self, r: Rect, c: Color) {
        self.g.fill(r, 0.0, c);
    }

    /// 1 px outline inside `r`.
    pub fn stroke(&mut self, r: Rect, c: Color) {
        self.g.stroke(r, 0.0, 1.0, c);
    }

    pub fn polyline(&mut self, pts: &[(f32, f32)], width: f32, c: Color) {
        self.g.polyline(pts, width, c);
    }
}
