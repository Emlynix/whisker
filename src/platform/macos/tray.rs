//! Menu-bar item (`NSStatusItem`) showing the battery glyph of
//! `crate::tray`. Left click opens the window, right or Control click
//! shows the Open / Quit menu.

use crate::i18n::{T, tr};
use crate::tray::{ICON_SIZE, Level, TrayAction, glyph};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, Sel};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSBitmapImageRep, NSDeviceRGBColorSpace, NSEventMask, NSEventModifierFlags, NSEventType, NSImage, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSPoint, NSSize, NSString};
use std::cell::RefCell;

/// Height of the menu bar glyph in points; the 32 px glyph stays sharp
/// on Retina screens.
const ICON_POINTS: f64 = 22.0;
const BYTES_PER_PIXEL: usize = 4;
/// Gap between the menu-bar item and the menu, in points.
const MENU_GAP: f64 = 4.0;

struct Item {
    status: Retained<NSStatusItem>,
    /// Retained here: buttons and menu items hold their target weakly.
    _target: Retained<Target>,
    menu: Retained<NSMenu>,
    on: fn(TrayAction),
}

thread_local! {
    static ITEM: RefCell<Option<Item>> = const { RefCell::new(None) };
}

define_class!(
    /// Receives clicks of the menu-bar button and its menu items.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WhiskerTrayTarget"]
    struct Target;

    impl Target {
        #[unsafe(method(clicked:))]
        fn clicked(&self, _sender: Option<&NSObject>) {
            on_click(self.mtm());
        }

        #[unsafe(method(open:))]
        fn open(&self, _sender: Option<&NSObject>) {
            fire(TrayAction::Open);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&NSObject>) {
            fire(TrayAction::Quit);
        }
    }
);

/// Calls the hook outside the borrow, so it may update the item.
fn fire(a: TrayAction) {
    let Some(on) = ITEM.with(|i| i.borrow().as_ref().map(|i| i.on)) else {
        return;
    };
    on(a);
}

fn on_click(mtm: MainThreadMarker) {
    let event = NSApplication::sharedApplication(mtm).currentEvent();
    let menu_click = event.is_some_and(|e| e.r#type() == NSEventType::RightMouseUp || e.modifierFlags().contains(NSEventModifierFlags::Control));
    if !menu_click {
        fire(TrayAction::Open);
        return;
    }

    // Clone out of the borrow: the menu runs a nested event loop.
    let Some((status, menu)) = ITEM.with(|i| i.borrow().as_ref().map(|i| (i.status.clone(), i.menu.clone()))) else {
        return;
    };
    let Some(button) = status.button(mtm) else {
        return;
    };
    let below = NSPoint::new(0.0, button.bounds().size.height + MENU_GAP);
    menu.popUpMenuPositioningItem_atLocation_inView(None, below, Some(&button));
}

fn menu_item(mtm: MainThreadMarker, title: &str, action: Sel, target: &Target) -> Retained<NSMenuItem> {
    let empty = NSString::new();
    // SAFETY: `action` is implemented by `Target`.
    let item = unsafe { NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), &NSString::from_str(title), Some(action), &empty) };
    // SAFETY: the target lives in `ITEM` as long as the menu.
    unsafe { item.setTarget(Some(target)) };
    item
}

/// Adds the item; `on` receives clicks.
pub fn add(mtm: MainThreadMarker, on: fn(TrayAction)) {
    let status = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    // SAFETY: `init` of an NSObject subclass without ivars.
    let target: Retained<Target> = unsafe { msg_send![Target::alloc(mtm), init] };

    let menu = NSMenu::new(mtm);
    menu.addItem(&menu_item(mtm, tr(T::TrayOpen), sel!(open:), &target));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&menu_item(mtm, tr(T::TrayQuit), sel!(quit:), &target));

    if let Some(button) = status.button(mtm) {
        // SAFETY: `clicked:` is implemented by `Target`, kept alive in `ITEM`.
        unsafe {
            button.setTarget(Some(&target));
            button.setAction(Some(sel!(clicked:)));
        }
        button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
    }

    ITEM.with(|i| *i.borrow_mut() = Some(Item { status, _target: target, menu, on }));
    update(mtm, Level::Offline, crate::APP_NAME);
}

pub fn update(mtm: MainThreadMarker, level: Level, tip: &str) {
    let Some(status) = ITEM.with(|i| i.borrow().as_ref().map(|i| i.status.clone())) else {
        return;
    };
    let Some(button) = status.button(mtm) else {
        return;
    };
    button.setImage(draw(level).as_deref());
    button.setToolTip(Some(&NSString::from_str(tip)));
}

pub fn remove() {
    let Some(item) = ITEM.with(|i| i.borrow_mut().take()) else {
        return;
    };
    NSStatusBar::systemStatusBar().removeStatusItem(&item.status);
    // Not released: this may run inside the target's own action (menu Quit),
    // and the process ends right after anyway.
    std::mem::forget(item);
}

/// Turns the battery glyph into an image.
fn draw(level: Level) -> Option<Retained<NSImage>> {
    let px = glyph(level);
    let side = ICON_SIZE as isize;
    let row = ICON_SIZE * BYTES_PER_PIXEL;

    // SAFETY: null planes make the rep allocate its own RGBA buffer of
    // `row * ICON_SIZE` bytes; the color space name is an AppKit constant.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            side,
            side,
            8,
            BYTES_PER_PIXEL as isize,
            true,
            false,
            NSDeviceRGBColorSpace,
            row as isize,
            (BYTES_PER_PIXEL * 8) as isize,
        )
    }?;

    let data = rep.bitmapData();
    if data.is_null() {
        return None;
    }
    // SAFETY: the buffer is `row * ICON_SIZE` bytes (see above) and only
    // this function touches it.
    let buf = unsafe { std::slice::from_raw_parts_mut(data, row * ICON_SIZE) };
    // 0xAARRGGBB -> R, G, B, A; alpha is 0 or 255, so premultiplied is the same.
    for (dst, p) in buf.as_chunks_mut::<BYTES_PER_PIXEL>().0.iter_mut().zip(px) {
        *dst = [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8];
    }

    let image = NSImage::initWithSize(NSImage::alloc(), NSSize::new(ICON_POINTS, ICON_POINTS));
    image.addRepresentation(&rep);
    Some(image)
}
