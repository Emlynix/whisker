//! Platform shell: one Win32 window painted by `gfx`, the message loop,
//! DPI scaling, mouse input and the tray icon. Turns mouse input on the
//! hits of the last frame into `Act`s and hands them to `Hooks`.
//! Knows nothing about the mouse or the app's state.

use crate::gfx::Gfx;
use crate::tray::{self, TrayAction};
use crate::view::{Act, Hit, Kind};
use crate::win::wide;
use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows_sys::Win32::UI::HiDpi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &str = "WhiskerWindow";
const WM_WAKE: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const BASE_DPI: f32 = 96.0;
const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
/// Exe icon resource id set by `winresource`.
const ICON_RESOURCE: usize = 1;
const STYLE: WINDOW_STYLE = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;

/// Keyboard event for the app (shortcut capture, macro recording).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub vk: u16,
    pub down: bool,
    /// Right Ctrl/Alt and other extended keys.
    pub extended: bool,
    /// Auto-repeat of a held key.
    pub repeat: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Move,
    Done,
}

/// Callbacks into the app. Plain fns: the app keeps its own state.
pub struct Hooks {
    pub draw: fn(&mut Gfx, Option<Act>) -> Vec<Hit>,
    pub act: fn(Act),
    pub drag: fn(Act, usize, Phase),
    pub timer: fn(usize),
    pub tray: fn(TrayAction),
    pub hidden: fn(),
    /// Windows settings changed (e.g. light/dark mode).
    pub settings: fn(),
    /// Returns true when the app consumed the key.
    pub key: fn(KeyEvent) -> bool,
    /// The window lost keyboard focus.
    pub focus_lost: fn(),
}

struct Shell {
    gfx: Gfx,
    hooks: Hooks,
    hits: Vec<Hit>,
    hot: Option<Act>,
    pressed: Option<Act>,
    drag: Option<Hit>,
    scale: f32,
    tracking: bool,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
}

static HWND_MAIN: AtomicUsize = AtomicUsize::new(0);
static QUEUE: Mutex<Vec<Box<dyn FnOnce() + Send>>> = Mutex::new(Vec::new());
static TASKBAR_CREATED: AtomicUsize = AtomicUsize::new(0);

fn hwnd() -> HWND {
    HWND_MAIN.load(Ordering::Relaxed) as HWND
}

/// Runs `f` on the UI thread (callable from any thread).
pub fn post(f: impl FnOnce() + Send + 'static) {
    QUEUE.lock().unwrap().push(Box::new(f));
    unsafe { PostMessageW(hwnd(), WM_WAKE, 0, 0) };
}

pub fn invalidate() {
    unsafe { InvalidateRect(hwnd(), std::ptr::null(), 0) };
}

pub fn show() {
    unsafe {
        ShowWindow(hwnd(), if IsIconic(hwnd()) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(hwnd());
    }
}

pub fn quit() {
    unsafe { DestroyWindow(hwnd()) };
}

pub fn set_timer(id: usize, ms: u32) {
    unsafe { SetTimer(hwnd(), id, ms, None) };
}

pub fn kill_timer(id: usize) {
    unsafe { KillTimer(hwnd(), id) };
}

/// Dark or light window title bar to match the theme.
pub fn set_dark_title(dark: bool) {
    let v = dark as u32;
    unsafe { DwmSetWindowAttribute(hwnd(), DWMWA_USE_IMMERSIVE_DARK_MODE, (&v as *const u32).cast(), 4) };
}

/// Modifier keys held right now (for shortcut capture).
#[derive(Clone, Copy, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub win: bool,
}

pub fn modifiers() -> Mods {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState;
    const VK_SHIFT: i32 = 0x10;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_LWIN: i32 = 0x5B;
    const VK_RWIN: i32 = 0x5C;
    let down = |vk: i32| unsafe { GetKeyState(vk) } < 0;
    Mods { ctrl: down(VK_CONTROL), shift: down(VK_SHIFT), alt: down(VK_MENU), win: down(VK_LWIN) || down(VK_RWIN) }
}

pub fn set_tray(level: tray::Level, tip: &str) {
    tray::update(hwnd(), level, tip);
}

/// Creates the (hidden) main window of `w` x `h` DIPs and the tray icon.
pub fn create(title: &str, w: f32, h: f32, hooks: Hooks) -> bool {
    let (class, title) = (wide(CLASS), wide(title));
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let module = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: module,
            hIcon: LoadIconW(module, ICON_RESOURCE as *const u16),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), STYLE, CW_USEDEFAULT, CW_USEDEFAULT, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), module, std::ptr::null());
        if hwnd.is_null() {
            return false;
        }
        HWND_MAIN.store(hwnd as usize, Ordering::Relaxed);
        TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) as usize, Ordering::Relaxed);


        let dpi = GetDpiForWindow(hwnd);
        let Some(mut gfx) = Gfx::new(windows::Win32::Foundation::HWND(hwnd)) else {
            return false;
        };
        gfx.set_dpi(dpi);
        let scale = dpi as f32 / BASE_DPI;
        SHELL.with(|s| *s.borrow_mut() = Some(Shell { gfx, hooks, hits: Vec::new(), hot: None, pressed: None, drag: None, scale, tracking: false }));

        place(hwnd, w, h, dpi);
        tray::add(hwnd, crate::APP_NAME);
    }
    true
}

/// Sizes the window for `w` x `h` DIPs of client area, centered on the work area.
unsafe fn place(hwnd: HWND, w: f32, h: f32, dpi: u32) {
    let s = dpi as f32 / BASE_DPI;
    let mut r = RECT { left: 0, top: 0, right: (w * s) as i32, bottom: (h * s) as i32 };
    unsafe {
        AdjustWindowRectExForDpi(&mut r, STYLE, 0, 0, dpi);
        let mut work: RECT = std::mem::zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, (&mut work as *mut RECT).cast(), 0);

        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let x = work.left + ((work.right - work.left) - ww) / 2;
        let y = work.top + ((work.bottom - work.top) - wh) / 2;
        SetWindowPos(hwnd, std::ptr::null_mut(), x, y, ww, wh, SWP_NOZORDER | SWP_NOACTIVATE);
    }
}

/// Returns unused pages to the OS so the tray-only app stays small.
fn trim_memory() {
    unsafe { SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX) };
}

pub fn run() {
    crate::win::pump_messages();
}

fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

fn mouse_pos(s: &Shell, lp: LPARAM) -> (f32, f32) {
    let x = (lp & 0xFFFF) as i16 as f32;
    let y = ((lp >> 16) & 0xFFFF) as i16 as f32;
    (x / s.scale, y / s.scale)
}

fn hit_at(s: &Shell, x: f32, y: f32) -> Option<Hit> {
    // Last pushed is drawn on top, so search from the end.
    s.hits.iter().rev().find(|h| h.rect.contains(x, y)).copied()
}

fn slider_value(h: &Hit, x: f32) -> usize {
    let Kind::Slider { x0, x1, steps } = h.kind else {
        return 0;
    };
    let t = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    (t * (steps - 1) as f32).round() as usize
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);

        with_shell(|s| {
            if !s.gfx.begin(rc.right as u32, rc.bottom as u32) {
                return;
            }
            s.hits = (s.hooks.draw)(&mut s.gfx, s.hot);
            s.gfx.end();
        });
        EndPaint(hwnd, &ps);
    }
}

fn on_move(hwnd: HWND, lp: LPARAM) {
    let mut call: Option<(fn(Act, usize, Phase), Act, usize)> = None;
    with_shell(|s| {
        if !s.tracking {
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut t) };
            s.tracking = true;
        }

        let (x, y) = mouse_pos(s, lp);
        if let Some(h) = s.drag {
            call = Some((s.hooks.drag, h.act, slider_value(&h, x)));
            return;
        }
        let hot = hit_at(s, x, y).map(|h| h.act);
        if hot != s.hot {
            s.hot = hot;
            invalidate();
        }
    });
    if let Some((f, act, v)) = call {
        f(act, v, Phase::Move);
    }
}

fn on_down(hwnd: HWND, lp: LPARAM) {
    let mut call = None;
    with_shell(|s| {
        let (x, y) = mouse_pos(s, lp);
        let Some(h) = hit_at(s, x, y) else {
            return;
        };
        if let Kind::Slider { .. } = h.kind {
            s.drag = Some(h);
            unsafe { SetCapture(hwnd) };
            call = Some((s.hooks.drag, h.act, slider_value(&h, x)));
            return;
        }
        s.pressed = Some(h.act);
    });
    if let Some((f, act, v)) = call {
        f(act, v, Phase::Move);
    }
}

fn on_up(lp: LPARAM) {
    let mut drag_call = None;
    let mut click = None;
    with_shell(|s| {
        let (x, y) = mouse_pos(s, lp);
        if let Some(h) = s.drag.take() {
            unsafe { ReleaseCapture() };
            drag_call = Some((s.hooks.drag, h.act, slider_value(&h, x)));
            return;
        }
        let pressed = s.pressed.take();
        let under = hit_at(s, x, y).map(|h| h.act);
        if pressed.is_some() && pressed == under {
            click = pressed.map(|a| (s.hooks.act, a));
        }
    });
    if let Some((f, act, v)) = drag_call {
        f(act, v, Phase::Done);
    }
    if let Some((f, a)) = click {
        f(a);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let taskbar = TASKBAR_CREATED.load(Ordering::Relaxed) as u32;
    match msg {
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return 1,
        WM_MOUSEMOVE => on_move(hwnd, lp),
        WM_LBUTTONDOWN => on_down(hwnd, lp),
        WM_LBUTTONUP => on_up(lp),
        WM_MOUSELEAVE => {
            with_shell(|s| {
                s.tracking = false;
                s.hot = None;
            });
            invalidate();
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            let hot = with_shell(|s| s.hot.is_some() || s.drag.is_some()).unwrap_or(false);
            let cursor = if hot { IDC_HAND } else { IDC_ARROW };
            unsafe { SetCursor(LoadCursorW(std::ptr::null_mut(), cursor)) };
            return 1;
        }
        WM_SIZE => {
            let (w, h) = ((lp & 0xFFFF) as u32, ((lp >> 16) & 0xFFFF) as u32);
            with_shell(|s| s.gfx.resize(w, h));
        }
        WM_DPICHANGED => {
            let dpi = ((wp >> 16) & 0xFFFF) as u32;
            with_shell(|s| {
                s.scale = dpi as f32 / BASE_DPI;
                s.gfx.set_dpi(dpi);
            });
            let r = unsafe { &*(lp as *const RECT) };
            unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE) };
        }
        WM_TIMER => {
            if let Some(f) = with_shell(|s| s.hooks.timer) {
                f(wp);
            }
        }
        WM_WAKE => {
            let jobs: Vec<_> = std::mem::take(&mut *QUEUE.lock().unwrap());
            for job in jobs {
                job();
            }
        }
        tray::WM_TRAY => {
            if let (Some(a), Some(f)) = (tray::on_message(hwnd, lp), with_shell(|s| s.hooks.tray)) {
                f(a);
            }
        }
        WM_CLOSE => {
            // Closing only hides; the app stays in the tray with drawing memory freed.
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            with_shell(|s| s.gfx.release());
            trim_memory();
            if let Some(f) = with_shell(|s| s.hooks.hidden) {
                f();
            }
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            const EXTENDED_BIT: isize = 1 << 24;
            const REPEAT_BIT: isize = 1 << 30;
            let ev = KeyEvent {
                vk: wp as u16,
                down: msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN,
                extended: lp & EXTENDED_BIT != 0,
                repeat: (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) && lp & REPEAT_BIT != 0,
            };
            let used = with_shell(|s| s.hooks.key).is_some_and(|f| f(ev));
            if !used {
                return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            }
        }
        WM_ACTIVATE if (wp & 0xFFFF) as u32 == WA_INACTIVE => {
            if let Some(f) = with_shell(|s| s.hooks.focus_lost) {
                f();
            }
        }
        WM_SETTINGCHANGE => {
            if let Some(f) = with_shell(|s| s.hooks.settings) {
                f();
            }
        }
        WM_DESTROY => {
            tray::remove(hwnd);
            unsafe { PostQuitMessage(0) };
        }
        m if m == taskbar && m != 0 => tray::add(hwnd, crate::APP_NAME),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
    0
}
