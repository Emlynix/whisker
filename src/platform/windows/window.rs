//! Platform shell: one Win32 window painted by `gfx`, the message loop,
//! DPI scaling, mouse input and the tray icon. Mouse input goes through
//! `shell::Pointer`, whose `Act`s are handed to `Hooks`.
//! Knows nothing about the mouse or the app's state.

use super::gfx::Gfx;
use super::tray;
use super::win::wide;
use crate::gfx::Color;
use crate::platform::{Shade, TitleColors};
use crate::shell::{Hooks, KeyEvent, Mods, Moved, Phase, Pointer, Released};
use crate::tray::Level;
use std::cell::RefCell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT, ScreenToClient};
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

struct Shell {
    gfx: Gfx,
    hooks: Hooks,
    pointer: Pointer,
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

/// The window is the foreground one.
pub fn is_active() -> bool {
    unsafe { GetForegroundWindow() == hwnd() }
}

/// Minimizes to the taskbar.
pub fn minimize() {
    unsafe { ShowWindow(hwnd(), SW_MINIMIZE) };
}

/// The app's close button: the same as the system's.
pub fn close() {
    unsafe { PostMessageW(hwnd(), WM_CLOSE, 0, 0) };
}

pub fn quit() {
    // Drawing objects go first, while the window they draw to still exists.
    free_shell();
    unsafe { DestroyWindow(hwnd()) };
}

pub fn set_timer(id: usize, ms: u32) {
    unsafe { SetTimer(hwnd(), id, ms, None) };
}

pub fn kill_timer(id: usize) {
    unsafe { KillTimer(hwnd(), id) };
}

/// Title bar in the theme's colours. Windows 11 takes caption, text and
/// border colours; Windows 10 refuses them and keeps the dark/light bar.
pub fn set_title_colors(c: TitleColors) {
    const DWMWA_BORDER_COLOR: u32 = 34;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    const DWMWA_TEXT_COLOR: u32 = 36;

    // Dark mode also picks light caption buttons, so set it everywhere.
    dwm_set(DWMWA_USE_IMMERSIVE_DARK_MODE, (c.shade == Shade::Dark) as u32);
    if !dwm_set(DWMWA_CAPTION_COLOR, colorref(c.bg)) {
        return;
    }
    dwm_set(DWMWA_TEXT_COLOR, colorref(c.text));
    dwm_set(DWMWA_BORDER_COLOR, colorref(c.border));
}

/// One 32-bit DWM window attribute; false if Windows refuses it.
fn dwm_set(attr: u32, v: u32) -> bool {
    const S_OK: i32 = 0;
    unsafe { DwmSetWindowAttribute(hwnd(), attr, (&v as *const u32).cast(), size_of::<u32>() as u32) == S_OK }
}

/// `0xRRGGBB` -> COLORREF `0x00BBGGRR`.
fn colorref(c: Color) -> u32 {
    let v = c.0;
    ((v & 0xFF) << 16) | (v & 0xFF00) | ((v >> 16) & 0xFF)
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

/// Puts `text` on the clipboard as Unicode text; the window owns it.
pub fn set_clipboard(text: &str) -> bool {
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    /// Standard clipboard format of UTF-16 text.
    const CF_UNICODETEXT: u32 = 13;

    let data = super::win::wide(text);
    let bytes = data.len() * size_of::<u16>();
    // No owner window: EmptyClipboard would leave the clipboard ownerless
    // and SetClipboardData would fail after the user's content is gone.
    if hwnd().is_null() {
        return false;
    }
    unsafe {
        // Prepare the text first, so a failed allocation never empties the
        // user's clipboard.
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if mem.is_null() {
            return false;
        }
        let dst = GlobalLock(mem) as *mut u16;
        if dst.is_null() {
            GlobalFree(mem);
            return false;
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        GlobalUnlock(mem);

        if OpenClipboard(hwnd()) == 0 {
            GlobalFree(mem);
            return false;
        }
        EmptyClipboard();
        // On success the clipboard owns the memory; free it only on failure.
        let ok = !SetClipboardData(CF_UNICODETEXT, mem).is_null();
        if !ok {
            GlobalFree(mem);
        }
        CloseClipboard();
        ok
    }
}

pub fn set_tray(level: Level, tip: &str) {
    tray::update(hwnd(), level, tip);
}

pub fn set_tray_profiles(names: &[String], active: usize) {
    tray::set_profiles(names, active);
}

/// System notification from the tray icon; a click on it opens the window.
pub fn notify(title: &str, text: &str) {
    tray::notify(hwnd(), title, text);
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
        SHELL.with(|s| *s.borrow_mut() = Some(Shell { gfx, hooks, pointer: Pointer::default(), scale, tracking: false }));

        // Sharp corners are part of the design; Windows 11 rounds by default.
        const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
        const DWMWCP_DONOTROUND: u32 = 1;
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, (&DWMWCP_DONOTROUND as *const u32).cast(), size_of::<u32>() as u32);
        place(hwnd, w, h, dpi);
        tray::add(hwnd, crate::APP_NAME);
    }
    true
}

/// Sizes the window to `w` x `h` DIPs, centered on the work area. There
/// is no frame to add: the client area is the whole window (`WM_NCCALCSIZE`).
unsafe fn place(hwnd: HWND, w: f32, h: f32, dpi: u32) {
    let s = dpi as f32 / BASE_DPI;
    let r = RECT { left: 0, top: 0, right: (w * s) as i32, bottom: (h * s) as i32 };
    unsafe {
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
    super::win::pump_messages();
    free_shell();
}

/// Frees the drawing objects once the window is gone. They must not wait for
/// the thread-local destructor at process exit: Direct2D then destroys its
/// software rasterizer after the thread pool it schedules on has been shut
/// down, and the resulting exception kills an otherwise clean exit.
fn free_shell() {
    SHELL.with(|s| {
        let Ok(mut cell) = s.try_borrow_mut() else { return };
        let Some(mut shell) = cell.take() else { return };
        shell.gfx.release();
    });
}

fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

/// `WM_NCHITTEST`: the empty part of the app's title bar is the caption
/// (Windows then moves the window itself), the rest is client area.
fn hit_test(hwnd: HWND, lp: LPARAM) -> LRESULT {
    let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
    unsafe { ScreenToClient(hwnd, &mut pt) };
    let caption = with_shell(|s| s.pointer.caption_at(pt.x as f32 / s.scale, pt.y as f32 / s.scale)).unwrap_or(false);
    (if caption { HTCAPTION } else { HTCLIENT }) as LRESULT
}

fn mouse_pos(s: &Shell, lp: LPARAM) -> (f32, f32) {
    let x = (lp & 0xFFFF) as i16 as f32;
    let y = ((lp >> 16) & 0xFFFF) as i16 as f32;
    (x / s.scale, y / s.scale)
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
            let hits = (s.hooks.draw)(&mut s.gfx, s.pointer.hot());
            s.pointer.set_hits(hits);
            s.gfx.end();
        });
        EndPaint(hwnd, &ps);
    }
}

fn on_move(hwnd: HWND, lp: LPARAM) {
    let mut call = None;
    with_shell(|s| {
        if !s.tracking {
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut t) };
            s.tracking = true;
        }

        let (x, y) = mouse_pos(s, lp);
        match s.pointer.moved(x, y) {
            Moved::Drag(act, v) => call = Some((s.hooks.drag, act, v)),
            Moved::Redraw => invalidate(),
            Moved::Nothing => {}
        }
    });
    if let Some((f, act, v)) = call {
        f(act, v, Phase::Move);
    }
}

/// Wheel notches (120 units each, at least one) at the pointer, which
/// the message gives in screen coordinates.
fn on_wheel(hwnd: HWND, wp: WPARAM, lp: LPARAM) {
    const NOTCH: i32 = 120;
    let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
    let notches = if delta.abs() < NOTCH { delta.signum() } else { delta / NOTCH };
    let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
    unsafe { ScreenToClient(hwnd, &mut pt) };
    let Some((f, x, y)) = with_shell(|s| (s.hooks.wheel, pt.x as f32 / s.scale, pt.y as f32 / s.scale)) else {
        return;
    };
    f(x, y, notches);
}

fn on_down(hwnd: HWND, lp: LPARAM) {
    let mut call = None;
    with_shell(|s| {
        let (x, y) = mouse_pos(s, lp);
        let Some((act, v)) = s.pointer.down(x, y) else {
            return;
        };
        unsafe { SetCapture(hwnd) };
        call = Some((s.hooks.drag, act, v));
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
        match s.pointer.up(x, y) {
            Released::Drag(act, v) => {
                unsafe { ReleaseCapture() };
                drag_call = Some((s.hooks.drag, act, v));
            }
            Released::Click(a) => click = Some((s.hooks.act, a)),
            Released::Nothing => {}
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
        WM_MOUSEWHEEL => on_wheel(hwnd, wp, lp),
        // No system title bar or frame: the app draws the whole window.
        // The style keeps WS_CAPTION, so the taskbar, Win+arrows and the
        // minimize animation still treat it as a normal window.
        WM_NCCALCSIZE if wp != 0 => return 0,
        WM_NCHITTEST => return hit_test(hwnd, lp),
        WM_LBUTTONDOWN => on_down(hwnd, lp),
        WM_LBUTTONUP => on_up(lp),
        WM_MOUSELEAVE => {
            with_shell(|s| {
                s.tracking = false;
                s.pointer.leave();
            });
            invalidate();
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            let hot = with_shell(|s| s.pointer.busy()).unwrap_or(false);
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

#[cfg(test)]
mod tests {
    //! The bug this covers only shows in a process that really exits: the
    //! drawing objects sat in a thread-local, so Direct2D destroyed its
    //! software rasterizer during process teardown, when the thread pool it
    //! schedules on is already gone (0xc000000d from ntdll, an Application
    //! Error in the event log). The test therefore runs `child_paints_and_exits`
    //! in a second process and reads that process' exit code.

    use super::*;
    use std::time::{Duration, Instant};

    /// Set only in the child, which paints a frame and then exits.
    const CHILD_VAR: &str = "WHISKER_EXIT_CHILD";
    const CHILD_TEST: &str = "platform::windows::window::tests::child_paints_and_exits";
    /// The child found no window to draw in, so there was nothing to check.
    const SKIPPED: i32 = 7;
    const CHILD_LIMIT: Duration = Duration::from_secs(30);

    fn stubs() -> Hooks {
        Hooks {
            draw: |_, _| Vec::new(),
            act: |_| {},
            drag: |_, _, _| {},
            wheel: |_, _, _| {},
            timer: |_| {},
            tray: |_| {},
            hidden: || {},
            settings: || {},
            key: |_| false,
            focus_lost: || {},
        }
    }

    /// A window that drew one frame, which is what makes Direct2D build the
    /// software render target the bug needs, then quits like the app does.
    #[test]
    fn child_paints_and_exits() {
        if std::env::var_os(CHILD_VAR).is_none() {
            return;
        }
        if !create("Whisker test", 240.0, 160.0, stubs()) {
            std::process::exit(SKIPPED);
        }
        with_shell(|s| {
            s.gfx.begin(240, 160);
            s.gfx.end();
        });
        quit();
        run();
        // Ends the process from this thread, as the app's main does, so the
        // teardown that used to destroy the drawing objects is the one under test.
        std::process::exit(0);
    }

    #[test]
    fn exit_after_a_frame_releases_the_drawing() {
        let spawned = std::process::Command::new(std::env::current_exe().unwrap())
            .arg(CHILD_TEST)
            .arg("--exact")
            .env(CHILD_VAR, "1")
            .spawn();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => panic!("could not start the child process: {e}"),
        };

        // The child ends itself; a crash or a WER dialog would leave it alive,
        // so the wait is bounded.
        let started = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if started.elapsed() > CHILD_LIMIT {
                        let _ = child.kill();
                        panic!("the child did not exit within {} s", CHILD_LIMIT.as_secs());
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => panic!("waiting for the child failed: {e}"),
            }
        };

        let code = status.code().unwrap_or(0);
        assert!(code == 0 || code == SKIPPED, "child exited with {code:#x}: the drawing objects were still held when the process ended");
    }
}
