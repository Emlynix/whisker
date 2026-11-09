//! Platform shell: one NSWindow whose flipped content view is painted by
//! `gfx`, the AppKit run loop, mouse/keyboard input and the menu-bar
//! item. Mouse input goes through `shell::Pointer`, whose `Act`s are
//! handed to `Hooks`. Knows nothing about the mouse or the app's state.
//!
//! ```text
//!   NSApplication (accessory: menu bar only, no Dock icon)
//!     ├─ NSWindow ── Delegate (close = hide, focus lost)
//!     │    └─ View: drawRect -> Hooks::draw, mouse -> Pointer, keys -> keymap
//!     ├─ NSStatusItem (tray.rs)
//!     └─ main dispatch queue: post(); NSTimer: set_timer()
//! ```

use super::gfx::Gfx;
use super::{keymap, tray};
use crate::shell::{Hooks, KeyEvent, Mods, Moved, Phase, Pointer, Released};
use crate::tray::{Level, TrayAction};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSCursor, NSEvent,
    NSEventModifierFlags, NSEventType, NSGraphicsContext, NSResponder, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowDelegate,
    NSWindowStyleMask,
};
use objc2_foundation::{NSDistributedNotificationCenter, NSNotification, NSOperationQueue, NSPoint, NSRect, NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ptr::NonNull;

/// Posted by the system when Light/Dark appearance changes.
const THEME_CHANGED: &str = "AppleInterfaceThemeChangedNotification";
const MS_PER_SEC: f64 = 1000.0;
/// `kVK_ANSI_Q` / `kVK_ANSI_W`: Command-Q quits, Command-W closes (the
/// accessory app has no main menu that would provide them).
const KEY_Q: u16 = 0x0C;
const KEY_W: u16 = 0x0D;

struct Shell {
    gfx: Gfx,
    hooks: Hooks,
    pointer: Pointer,
}

/// AppKit objects of the window, kept alive for the process.
struct Ui {
    window: Retained<NSWindow>,
    view: Retained<View>,
    _delegate: Retained<Delegate>,
    timers: HashMap<usize, Retained<NSTimer>>,
}

thread_local! {
    static SHELL: RefCell<Option<Shell>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Down,
    Up,
}

define_class!(
    /// Content view: flipped (top-left origin, like the core's DIPs).
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WhiskerView"]
    struct View;

    impl View {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            paint(self);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, e: &NSEvent) {
            on_move(self.point(e));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, e: &NSEvent) {
            on_move(self.point(e));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, e: &NSEvent) {
            on_down(self.point(e));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, e: &NSEvent) {
            on_up(self.point(e));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _e: &NSEvent) {
            with_shell(|s| s.pointer.leave());
            set_cursor(false);
            invalidate();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, e: &NSEvent) {
            on_key(e, Edge::Down);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, e: &NSEvent) {
            on_key(e, Edge::Up);
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, e: &NSEvent) {
            on_flags(e);
        }
    }
);

impl View {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        // SAFETY: designated initializer of NSView; no ivars to set.
        let view: Retained<Self> = unsafe { msg_send![Self::alloc(mtm), initWithFrame: frame] };

        // Hover tracking over the whole view, also while the app is inactive.
        let opts = NSTrackingAreaOptions::MouseMoved | NSTrackingAreaOptions::MouseEnteredAndExited | NSTrackingAreaOptions::ActiveAlways | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: the view owns the area and outlives it.
        let area = unsafe { NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), NSRect::ZERO, opts, Some(&view), None) };
        view.addTrackingArea(&area);
        view
    }

    /// Event position in view points (top-left origin).
    fn point(&self, e: &NSEvent) -> (f32, f32) {
        let p = self.convertPoint_fromView(e.locationInWindow(), None);
        (p.x as f32, p.y as f32)
    }
}

define_class!(
    /// Window delegate: closing only hides, losing key status = focus lost.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WhiskerWindowDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSWindow) -> bool {
            hide();
            false
        }

        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _n: &NSNotification) {
            if let Some(f) = with_shell(|s| s.hooks.focus_lost) {
                f();
            }
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        // SAFETY: `init` of an NSObject subclass without ivars.
        unsafe { msg_send![Self::alloc(mtm), init] }
    }
}

fn with_shell<R>(f: impl FnOnce(&mut Shell) -> R) -> Option<R> {
    SHELL.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Runs `f` on the UI thread (callable from any thread).
pub fn post(f: impl FnOnce() + Send + 'static) {
    DispatchQueue::main().exec_async(f);
}

pub fn invalidate() {
    with_ui(|u| u.view.setNeedsDisplay(true));
}

pub fn show() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // `activate` needs macOS 14; the older call works everywhere.
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    with_ui(|u| {
        if u.window.isMiniaturized() {
            u.window.deminiaturize(None);
        }
        u.window.makeKeyAndOrderFront(None);
    });
}

/// Hides the window; the app stays in the menu bar.
fn hide() {
    with_ui(|u| u.window.orderOut(None));
    with_shell(|s| s.gfx.release());
    if let Some(f) = with_shell(|s| s.hooks.hidden) {
        f();
    }
}

/// Removes the menu-bar item and ends `run`.
pub fn quit() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    tray::remove();
    with_ui(|u| {
        for t in u.timers.values() {
            t.invalidate();
        }
        u.timers.clear();
        u.window.orderOut(None);
    });

    // `stop` takes effect after the next event, so post an empty one.
    let app = NSApplication::sharedApplication(mtm);
    app.stop(None);
    let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::ZERO,
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        0,
        0,
        0,
    );
    if let Some(e) = wake {
        app.postEvent_atStart(&e, true);
    }
}

/// Repeating timer to `Hooks::timer`; the same `id` replaces the old one.
pub fn set_timer(id: usize, ms: u32) {
    kill_timer(id);
    let block = RcBlock::new(move |_t: NonNull<NSTimer>| {
        if let Some(f) = with_shell(|s| s.hooks.timer) {
            f(id);
        }
    });
    // SAFETY: the block runs on the main run loop, the thread that made it.
    let timer = unsafe { NSTimer::timerWithTimeInterval_repeats_block(ms as f64 / MS_PER_SEC, true, &block) };
    // Common modes: keep ticking while a menu or drag tracks the mouse.
    // SAFETY: `NSRunLoopCommonModes` is a Foundation constant.
    unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes) };
    with_ui(|u| u.timers.insert(id, timer));
}

pub fn kill_timer(id: usize) {
    if let Some(t) = with_ui(|u| u.timers.remove(&id)).flatten() {
        t.invalidate();
    }
}

/// Dark or light window title bar (and controls) to match the theme.
pub fn set_dark_title(dark: bool) {
    // SAFETY: AppKit constants, initialized when the framework loads.
    let name = unsafe { if dark { NSAppearanceNameDarkAqua } else { NSAppearanceNameAqua } };
    let look = NSAppearance::appearanceNamed(name);
    with_ui(|u| u.window.setAppearance(look.as_deref()));
}

/// Control, Shift, Option and Command (as the Windows key).
pub fn modifiers() -> Mods {
    let f = NSEvent::modifierFlags_class();
    let on = |m: NSEventModifierFlags| f.contains(m);
    Mods { ctrl: on(NSEventModifierFlags::Control), shift: on(NSEventModifierFlags::Shift), alt: on(NSEventModifierFlags::Option), win: on(NSEventModifierFlags::Command) }
}

/// Puts `text` on the general pasteboard as plain text.
pub fn set_clipboard(text: &str) -> bool {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    // SAFETY: AppKit constant, initialized when the framework loads.
    let kind = unsafe { NSPasteboardTypeString };
    pb.setString_forType(&NSString::from_str(text), kind)
}

pub fn set_tray(level: Level, tip: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    tray::update(mtm, level, tip);
}

fn on_tray(a: TrayAction) {
    if let Some(f) = with_shell(|s| s.hooks.tray) {
        f(a);
    }
}

/// Creates the (hidden) main window of `w` x `h` DIPs and the menu-bar item.
pub fn create(title: &str, w: f32, h: f32, hooks: Hooks) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    // Menu-bar app without a Dock icon, also when started as a bare binary.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(w as f64, h as f64));
    let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
    // SAFETY: plain window init; released only by our `Retained`.
    let window = unsafe { NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), frame, style, NSBackingStoreType::Buffered, false) };
    // SAFETY: we keep the window in `UI`, so AppKit must not release it on close.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(title));

    let view = View::new(mtm, frame);
    window.setContentView(Some(&view));
    // Keys go to the view, not the window.
    window.makeFirstResponder(Some(&view));
    window.setAcceptsMouseMovedEvents(true);
    let delegate = Delegate::new(mtm);
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    window.center();

    SHELL.with(|s| *s.borrow_mut() = Some(Shell { gfx: Gfx::new(), hooks, pointer: Pointer::default() }));
    UI.with(|u| *u.borrow_mut() = Some(Ui { window, view, _delegate: delegate, timers: HashMap::new() }));
    tray::add(mtm, on_tray);
    watch_theme();
    true
}

/// Calls `Hooks::settings` when the system switches Light/Dark.
fn watch_theme() {
    let block = RcBlock::new(|_n: NonNull<NSNotification>| {
        if let Some(f) = with_shell(|s| s.hooks.settings) {
            f();
        }
    });
    let center = NSDistributedNotificationCenter::defaultCenter();
    let name = NSString::from_str(THEME_CHANGED);
    // SAFETY: delivered on the main queue, the thread that owns the hooks.
    let token = unsafe { center.addObserverForName_object_queue_usingBlock(Some(&name), None, Some(&NSOperationQueue::mainQueue()), &block) };
    // The observer stays for the whole process.
    std::mem::forget(token);
}

pub fn run() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    NSApplication::sharedApplication(mtm).run();
}

fn paint(view: &View) {
    let Some(ctx) = NSGraphicsContext::currentContext().map(|c| c.CGContext().into()) else {
        return;
    };
    let size = view.bounds().size;
    with_shell(|s| {
        s.gfx.begin(ctx, size.width as f32, size.height as f32);
        let hits = (s.hooks.draw)(&mut s.gfx, s.pointer.hot());
        s.pointer.set_hits(hits);
        s.gfx.end();
    });
}

fn set_cursor(hand: bool) {
    let c = if hand { NSCursor::pointingHandCursor() } else { NSCursor::arrowCursor() };
    c.set();
}

fn on_move((x, y): (f32, f32)) {
    let mut call = None;
    with_shell(|s| {
        match s.pointer.moved(x, y) {
            Moved::Drag(act, v) => call = Some((s.hooks.drag, act, v)),
            Moved::Redraw => invalidate(),
            Moved::Nothing => {}
        }
        set_cursor(s.pointer.busy());
    });
    if let Some((f, act, v)) = call {
        f(act, v, Phase::Move);
    }
}

fn on_down((x, y): (f32, f32)) {
    // AppKit sends drags to this view while the button is held: no capture.
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

fn send_key(ev: KeyEvent) -> bool {
    with_shell(|s| s.hooks.key).is_some_and(|f| f(ev))
}

fn on_key(e: &NSEvent, edge: Edge) {
    let code = e.keyCode();
    let used = keymap::vk_of(code).is_some_and(|(vk, extended)| {
        let down = edge == Edge::Down;
        send_key(KeyEvent { vk, down, extended, repeat: down && e.isARepeat() })
    });
    if used || edge == Edge::Up || !e.modifierFlags().contains(NSEventModifierFlags::Command) {
        return;
    }
    match code {
        KEY_Q => on_tray(TrayAction::Quit),
        KEY_W => hide(),
        _ => {}
    }
}

fn on_flags(e: &NSEvent) {
    let Some((vk, extended, down)) = keymap::modifier_of(e.keyCode(), e.modifierFlags().0) else {
        return;
    };
    send_key(KeyEvent { vk, down, extended, repeat: false });
}
