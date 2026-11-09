//! Platform shell: one GTK 3 window whose drawing area is painted by
//! `gfx`, the GLib main loop, mouse/keyboard input, the tray item and
//! desktop notifications. Mouse input goes through `shell::Pointer`,
//! whose `Act`s are handed to `Hooks`. Knows nothing about the mouse or
//! the app's state.
//!
//! ```text
//!   GtkWindow (close = hide, focus-out = focus lost, keys -> keymap)
//!     ├─ GtkHeaderBar: title bar in the theme's colours (CSS)
//!     └─ GtkDrawingArea: draw -> Hooks::draw, pointer -> Pointer
//!   tray (AppIndicator) · bus (portal theme, notifications)
//!   GLib main context: post() = invoke, set_timer() = timeout source
//! ```
//!
//! Debug aid: with `WHISKER_SNAPSHOT=/path/frame.png` the whole window
//! is written to that PNG after each frame (screenshots where the
//! compositor allows none).

use super::gfx::Gfx;
use super::{bus, keymap, tray};
use crate::platform::{Shade, TitleColors};
use crate::shell::{Hooks, KeyEvent, Mods, Moved, Phase, Pointer, Released};
use crate::tray::{Level, TrayAction};
use gtk::gdk::{self, EventMask, EventType, ModifierType, ScrollDirection};
use gtk::gdk_pixbuf::{Pixbuf, PixbufLoader};
use gtk::glib::{self, ControlFlow, Propagation, SourceId};
use gtk::prelude::*;
use gtk::cairo;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Wayland app id / X11 class; matches the installed `.desktop` file.
pub(super) const APP_ID: &str = "com.emlynix.whisker";
/// Writes every frame to this PNG when set (debug screenshots).
const SNAPSHOT_VAR: &str = "WHISKER_SNAPSHOT";
const ICON_ICO: &[u8] = include_bytes!("../../../assets/icon.ico");
/// Primary (left) mouse button in GDK numbering.
const BUTTON_PRIMARY: u32 = 1;
const HAND_CURSOR: &str = "pointer";

struct Shell {
    gfx: Gfx,
    hooks: Hooks,
    pointer: Pointer,
    /// Keys held now, to flag auto-repeat (GDK 3 has no repeat bit).
    held: HashSet<u16>,
    /// The hand cursor is shown.
    hand: bool,
}

/// GTK objects of the window, kept alive for the process.
struct Ui {
    window: gtk::Window,
    area: gtk::DrawingArea,
    css: gtk::CssProvider,
    timers: HashMap<usize, SourceId>,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static SNAP: Cell<Snap> = const { Cell::new(Snap::Idle) };
}

/// Debug snapshot progress: one at a time, and not while it draws.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Snap {
    Idle,
    Queued,
    Drawing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Down,
    Up,
}

fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Runs `f` on the UI thread (callable from any thread).
pub fn post(f: impl FnOnce() + Send + 'static) {
    glib::MainContext::default().invoke(f);
}

pub fn invalidate() {
    with_ui(|u| u.area.queue_draw());
}

pub fn show() {
    with_ui(|u| {
        u.window.show_all();
        u.window.present();
    });
}

/// Hides the window; the app stays in the tray.
fn hide() {
    with_ui(|u| u.window.hide());
    with_shell(|s| s.gfx.release());
    if let Some(f) = with_shell(|s| s.hooks.hidden) {
        f();
    }
}

/// Removes the tray item and ends `run`.
pub fn quit() {
    tray::remove();
    with_ui(|u| {
        for (_, t) in u.timers.drain() {
            t.remove();
        }
        u.window.hide();
    });
    if gtk::main_level() > 0 {
        gtk::main_quit();
    }
}

/// Repeating timer to `Hooks::timer`; the same `id` replaces the old one.
pub fn set_timer(id: usize, ms: u32) {
    kill_timer(id);
    let source = glib::timeout_add_local(Duration::from_millis(ms as u64), move || {
        if let Some(f) = with_shell(|s| s.hooks.timer) {
            f(id);
        }
        ControlFlow::Continue
    });
    with_ui(|u| u.timers.insert(id, source));
}

pub fn kill_timer(id: usize) {
    if let Some(t) = with_ui(|u| u.timers.remove(&id)).flatten() {
        t.remove();
    }
}

/// `#rrggbb` of a colour (alpha dropped).
fn hex(c: crate::gfx::Color) -> String {
    format!("#{:06x}", c.0 & 0xFF_FFFF)
}

/// Title bar in the theme's colours: header bar background, title and
/// button glyphs, bottom border; the GTK dark variant for the rest.
pub fn set_title_colors(c: TitleColors) {
    let (bg, text, border) = (hex(c.bg), hex(c.text), hex(c.border));
    let css = format!(
        "headerbar {{ background: {bg}; color: {text}; border-color: {border}; box-shadow: none; }}\n\
         headerbar .title, headerbar button {{ color: {text}; }}"
    );
    if let Some(s) = gtk::Settings::default() {
        s.set_gtk_application_prefer_dark_theme(c.shade == Shade::Dark);
    }
    with_ui(|u| {
        let _ = u.css.load_from_data(css.as_bytes());
    });
}

/// Control, Shift, Alt and Super (as the Windows key), held now.
pub fn modifiers() -> Mods {
    let keymap = gdk::Display::default().and_then(|d| gdk::Keymap::for_display(&d));
    let state = keymap.map(|k| ModifierType::from_bits_truncate(k.modifier_state())).unwrap_or_else(ModifierType::empty);
    let on = |m: ModifierType| state.intersects(m);
    Mods { ctrl: on(ModifierType::CONTROL_MASK), shift: on(ModifierType::SHIFT_MASK), alt: on(ModifierType::MOD1_MASK), win: on(ModifierType::SUPER_MASK | ModifierType::MOD4_MASK) }
}

/// Puts `text` on the clipboard; stored so it survives the app.
pub fn set_clipboard(text: &str) -> bool {
    let clip = gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD);
    clip.set_text(text);
    clip.store();
    true
}

pub fn set_tray(level: Level, tip: &str) {
    tray::update(level, tip);
}

/// Desktop notification; a click on it opens the window.
pub fn notify(title: &str, text: &str) {
    bus::notify(title, text, crate::i18n::tr(crate::i18n::T::TrayOpen));
}

fn on_tray(a: TrayAction) {
    if let Some(f) = with_shell(|s| s.hooks.tray) {
        f(a);
    }
}

fn on_settings() {
    if let Some(f) = with_shell(|s| s.hooks.settings) {
        f();
    }
}

/// App icon decoded from the Windows `.ico` (largest image).
pub(super) fn app_icon() -> Option<Pixbuf> {
    let loader = PixbufLoader::new();
    loader.write(ICON_ICO).ok()?;
    loader.close().ok()?;
    loader.pixbuf()
}

/// Creates the (hidden) main window of `w` x `h` DIPs and the tray item.
pub fn create(title: &str, w: f32, h: f32, hooks: Hooks) -> bool {
    glib::set_prgname(Some(APP_ID));
    glib::set_application_name(crate::APP_NAME);
    if gtk::init().is_err() {
        return false;
    }

    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let header = gtk::HeaderBar::new();
    header.set_title(Some(title));
    header.set_show_close_button(true);
    window.set_titlebar(Some(&header));
    window.set_title(title);
    window.set_resizable(false);
    window.set_position(gtk::WindowPosition::Center);
    if let Some(icon) = app_icon() {
        window.set_icon(Some(&icon));
    }

    let area = gtk::DrawingArea::new();
    area.set_size_request(w.round() as i32, h.round() as i32);
    area.add_events(EventMask::POINTER_MOTION_MASK | EventMask::BUTTON_PRESS_MASK | EventMask::BUTTON_RELEASE_MASK | EventMask::SCROLL_MASK | EventMask::LEAVE_NOTIFY_MASK);
    window.add(&area);
    connect(&window, &area);

    let css = gtk::CssProvider::new();
    if let Some(screen) = gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(&screen, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    SHELL.with(|s| *s.borrow_mut() = Some(Shell { gfx: Gfx::new(), hooks, pointer: Pointer::default(), held: HashSet::new(), hand: false }));
    UI.with(|u| *u.borrow_mut() = Some(Ui { window, area, css, timers: HashMap::new() }));
    tray::add(on_tray);
    bus::watch_clicks(|| on_tray(TrayAction::Open));
    watch_theme();
    true
}

/// Window and drawing-area signals into the shell.
fn connect(window: &gtk::Window, area: &gtk::DrawingArea) {
    window.connect_delete_event(|_, _| {
        hide();
        Propagation::Stop
    });
    window.connect_focus_out_event(|_, _| {
        with_shell(|s| s.held.clear());
        if let Some(f) = with_shell(|s| s.hooks.focus_lost) {
            f();
        }
        Propagation::Proceed
    });
    window.connect_key_press_event(|_, e| on_key(e, Edge::Down));
    window.connect_key_release_event(|_, e| on_key(e, Edge::Up));

    area.connect_draw(|a, cr| {
        paint(a, cr);
        Propagation::Stop
    });
    area.connect_motion_notify_event(|_, e| {
        on_move(pos(e.position()));
        Propagation::Stop
    });
    area.connect_button_press_event(|_, e| {
        // Double-click events repeat a press already delivered.
        if e.button() == BUTTON_PRIMARY && e.event_type() == EventType::ButtonPress {
            on_down(pos(e.position()));
        }
        Propagation::Stop
    });
    area.connect_button_release_event(|_, e| {
        if e.button() == BUTTON_PRIMARY {
            on_up(pos(e.position()));
        }
        Propagation::Stop
    });
    area.connect_scroll_event(|_, e| {
        on_wheel(pos(e.position()), e.direction());
        Propagation::Stop
    });
    area.connect_leave_notify_event(|_, _| {
        with_shell(|s| s.pointer.leave());
        set_cursor(false);
        invalidate();
        Propagation::Proceed
    });
}

fn pos((x, y): (f64, f64)) -> (f32, f32) {
    (x as f32, y as f32)
}

/// Calls `Hooks::settings` when the desktop switches light/dark: the
/// portal's colour scheme or the GTK theme.
fn watch_theme() {
    bus::watch_scheme(on_settings);
    if let Some(s) = gtk::Settings::default() {
        s.connect_gtk_theme_name_notify(|_| on_settings());
    }
}

pub fn run() {
    gtk::main();
}

fn render(cr: &cairo::Context, w: f32, h: f32) {
    with_shell(|s| {
        s.gfx.begin(cr, w, h);
        let hits = (s.hooks.draw)(&mut s.gfx, s.pointer.hot());
        s.pointer.set_hits(hits);
        s.gfx.end();
    });
}

fn paint(area: &gtk::DrawingArea, cr: &cairo::Context) {
    render(cr, area.allocated_width() as f32, area.allocated_height() as f32);
    if !snapshot_wanted() || SNAP.get() != Snap::Idle {
        return;
    }
    // Once the frame is shown, draw the whole window (title bar too).
    SNAP.set(Snap::Queued);
    glib::idle_add_local_once(snapshot);
}

/// Debug builds only: release never writes window images.
fn snapshot_wanted() -> bool {
    cfg!(debug_assertions) && std::env::var_os(SNAPSHOT_VAR).is_some()
}

/// Draws the whole window into an image at device resolution and saves
/// it to `WHISKER_SNAPSHOT` as PNG.
fn snapshot() {
    SNAP.set(Snap::Drawing);
    let path = std::env::var_os(SNAPSHOT_VAR);
    with_ui(|u| {
        let (w, h, scale) = (u.window.allocated_width(), u.window.allocated_height(), u.window.scale_factor());
        let Ok(image) = cairo::ImageSurface::create(cairo::Format::ARgb32, w * scale, h * scale) else {
            return;
        };
        image.set_device_scale(scale as f64, scale as f64);
        if let Ok(icr) = cairo::Context::new(&image) {
            u.window.draw(&icr);
        }
        if let Some(mut f) = path.and_then(|p| std::fs::File::create(p).ok()) {
            let _ = image.write_to_png(&mut f);
        }
    });
    SNAP.set(Snap::Idle);
}

fn set_cursor(hand: bool) {
    let changed = with_shell(|s| std::mem::replace(&mut s.hand, hand) != hand).unwrap_or(false);
    if !changed {
        return;
    }
    with_ui(|u| {
        let Some(win) = u.area.window() else {
            return;
        };
        let cursor = hand.then(|| gdk::Cursor::from_name(&win.display(), HAND_CURSOR)).flatten();
        win.set_cursor(cursor.as_ref());
    });
}

fn on_move((x, y): (f32, f32)) {
    let mut call = None;
    let mut busy = false;
    with_shell(|s| {
        match s.pointer.moved(x, y) {
            Moved::Drag(act, v) => call = Some((s.hooks.drag, act, v)),
            Moved::Redraw => invalidate(),
            Moved::Nothing => {}
        }
        busy = s.pointer.busy();
    });
    set_cursor(busy);
    if let Some((f, act, v)) = call {
        f(act, v, Phase::Move);
    }
}

/// One wheel click as one notch, positive = away from the user.
fn on_wheel((x, y): (f32, f32), dir: ScrollDirection) {
    let notches = match dir {
        ScrollDirection::Up => 1,
        ScrollDirection::Down => -1,
        _ => return,
    };
    let Some(f) = with_shell(|s| s.hooks.wheel) else {
        return;
    };
    f(x, y, notches);
}

fn on_down((x, y): (f32, f32)) {
    // GTK grabs the pointer for the held button: drags keep arriving here.
    let Some((f, act, v)) = with_shell(|s| s.pointer.down(x, y).map(|(act, v)| (s.hooks.drag, act, v))).flatten() else {
        return;
    };
    f(act, v, Phase::Move);
}

fn on_up((x, y): (f32, f32)) {
    let mut drag_call = None;
    let mut click = None;
    with_shell(|s| match s.pointer.up(x, y) {
        Released::Drag(act, v) => drag_call = Some((s.hooks.drag, act, v)),
        Released::Click(a) => click = Some((s.hooks.act, a)),
        Released::Nothing => {}
    });
    if let Some((f, act, v)) = drag_call {
        f(act, v, Phase::Done);
    }
    if let Some((f, a)) = click {
        f(a);
    }
}

fn on_key(e: &gdk::EventKey, edge: Edge) -> Propagation {
    let code = e.hardware_keycode();
    let Some((vk, extended)) = keymap::vk_of(code) else {
        return Propagation::Proceed;
    };
    let down = edge == Edge::Down;
    // A press of a key already held is auto-repeat.
    let repeat = with_shell(|s| {
        if down {
            return !s.held.insert(code);
        }
        s.held.remove(&code);
        false
    })
    .unwrap_or(false);
    let Some(f) = with_shell(|s| s.hooks.key) else {
        return Propagation::Proceed;
    };
    if f(KeyEvent { vk, down, extended, repeat }) {
        return Propagation::Stop;
    }
    Propagation::Proceed
}
