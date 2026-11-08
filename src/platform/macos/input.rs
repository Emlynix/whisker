//! IOHIDManager hook for usage statistics: every value of every mouse,
//! even while the window is unfocused, on its own thread and run loop.
//! Needs the Input Monitoring permission (System Settings > Privacy &
//! Security); macOS asks on first use, without it no values arrive.
//!
//! IOKit delivers one value per element (button, X, Y, wheel); values of
//! one report share a timestamp and are merged into one `MouseInput`.

use crate::stats::MouseInput;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFRunLoop, CFString, kCFRunLoopDefaultMode};
use objc2_io_kit::{IOHIDDevice, IOHIDManager, IOHIDValue, IOReturn, kIOHIDOptionsTypeNone};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Mutex, OnceLock};

const PAGE_DESKTOP: u32 = 0x01;
const PAGE_BUTTON: u32 = 0x09;
const USAGE_MOUSE: i32 = 0x02;
const USAGE_X: u32 = 0x30;
const USAGE_Y: u32 = 0x31;
const USAGE_WHEEL: u32 = 0x38;
const BTN_LEFT: u32 = 1;
const BTN_RIGHT: u32 = 2;
const BTN_MIDDLE: u32 = 3;
const BTN_4: u32 = 4;
const BTN_5: u32 = 5;

static HANDLER: OnceLock<fn(&MouseInput)> = OnceLock::new();
/// Vendor id of each device seen, by device id.
static VENDORS: Mutex<Option<HashMap<usize, u16>>> = Mutex::new(None);

/// Report being assembled: `(device, timestamp, input)`.
type Pending = Option<(usize, u64, MouseInput)>;

thread_local! {
    static PENDING: RefCell<Pending> = const { RefCell::new(None) };
}

/// Starts the listener thread; `on` runs on it for each mouse report.
pub fn start(on: fn(&MouseInput)) {
    let _ = HANDLER.set(on);
    std::thread::Builder::new().name("hidinput".into()).spawn(listen).expect("spawn HID input thread");
}

/// The device of a report has vendor id `vid`.
pub fn has_vendor(dev: usize, vid: u16) -> bool {
    VENDORS.lock().unwrap().as_ref().and_then(|m| m.get(&dev).copied()) == Some(vid)
}

fn listen() {
    let manager = IOHIDManager::new(None, kIOHIDOptionsTypeNone);
    let keys = [CFString::from_static_str("DeviceUsagePage"), CFString::from_static_str("DeviceUsage")];
    let values = [CFNumber::new_i32(PAGE_DESKTOP as i32), CFNumber::new_i32(USAGE_MOUSE)];
    let matching = CFDictionary::from_slices(&[&*keys[0], &*keys[1]], &[&*values[0], &*values[1]]);
    let Some(rl) = CFRunLoop::current() else {
        return;
    };

    // SAFETY: the dictionary is copied by the call; the callback is a plain
    // fn needing no context; mode is a CF constant.
    unsafe {
        manager.set_device_matching(Some(matching.as_opaque()));
        manager.register_input_value_callback(Some(on_value), std::ptr::null_mut());
        let Some(mode) = kCFRunLoopDefaultMode else {
            return;
        };
        manager.schedule_with_run_loop(&rl, mode);
    }
    // Fails (kIOReturnNotPermitted) until Input Monitoring is granted.
    if manager.open(kIOHIDOptionsTypeNone) != 0 {
        return;
    }
    CFRunLoop::run();
}

fn vendor_of(dev: &IOHIDDevice) -> u16 {
    let key = CFString::from_static_str("VendorID");
    dev.property(&key).and_then(|v| super::iokit::as_number(&v)).unwrap_or(0) as u16
}

fn device_id(dev: &CFRetained<IOHIDDevice>) -> usize {
    CFRetained::as_ptr(dev).as_ptr() as usize
}

fn blank(dev: usize) -> MouseInput {
    MouseInput { dev, left: false, right: false, middle: false, side: false, wheel: false, dx: 0, dy: 0 }
}

fn emit(p: Pending) {
    let (Some((_, _, input)), Some(on)) = (p, HANDLER.get()) else {
        return;
    };
    on(&input);
}

/// Merges one element value into the pending report (IOKit callback).
unsafe extern "C-unwind" fn on_value(_ctx: *mut c_void, result: IOReturn, _sender: *mut c_void, value: NonNull<IOHIDValue>) {
    if result != 0 {
        return;
    }
    // SAFETY: IOKit passes a valid value for the duration of the call.
    let value = unsafe { value.as_ref() };
    let element = value.element();
    let dev = element.device();
    let id = device_id(&dev);
    let stamp = value.time_stamp();

    {
        let mut vendors = VENDORS.lock().unwrap();
        vendors.get_or_insert_with(HashMap::new).entry(id).or_insert_with(|| vendor_of(&dev));
    }

    // A new report starts: hand over the finished one.
    let done = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let same = p.as_ref().is_some_and(|(d, t, _)| (*d, *t) == (id, stamp));
        if same { None } else { p.replace((id, stamp, blank(id))) }
    });
    emit(done);

    let v = value.integer_value();
    PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let Some((_, _, m)) = p.as_mut() else {
            return;
        };
        match (element.usage_page(), element.usage()) {
            (PAGE_BUTTON, BTN_LEFT) => m.left |= v != 0,
            (PAGE_BUTTON, BTN_RIGHT) => m.right |= v != 0,
            (PAGE_BUTTON, BTN_MIDDLE) => m.middle |= v != 0,
            (PAGE_BUTTON, BTN_4 | BTN_5) => m.side |= v != 0,
            (PAGE_DESKTOP, USAGE_X) if element.is_relative() => m.dx += v as i32,
            (PAGE_DESKTOP, USAGE_Y) if element.is_relative() => m.dy += v as i32,
            (PAGE_DESKTOP, USAGE_WHEEL) => m.wheel |= v != 0,
            _ => {}
        }
    });
}
