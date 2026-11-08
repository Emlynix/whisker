//! Raw Input hook for usage statistics: every mouse report of every
//! device, even while the window is unfocused, on its own thread.

use super::win;
use crate::stats::MouseInput;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const MOUSE_DESKTOP: u16 = 0x01;
const USAGE_MOUSE: u16 = 0x02;
const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;

const BTN_LEFT: u16 = 0x0001;
const BTN_RIGHT: u16 = 0x0004;
const BTN_MIDDLE: u16 = 0x0010;
const BTN_4: u16 = 0x0040;
const BTN_5: u16 = 0x0100;
const BTN_WHEEL: u16 = 0x0400;

static HANDLER: OnceLock<fn(&MouseInput)> = OnceLock::new();

/// Starts the listener thread; `on` runs on it for each mouse report.
pub fn start(on: fn(&MouseInput)) {
    let _ = HANDLER.set(on);
    std::thread::Builder::new()
        .name("rawinput".into())
        .spawn(listen)
        .expect("spawn raw input thread");
}

/// Device name contains `VID_xxxx` of the vendor.
pub fn has_vendor(dev: usize, vid: u16) -> bool {
    let dev = dev as HANDLE;
    let mut len = 0u32;
    unsafe {
        GetRawInputDeviceInfoW(dev, RIDI_DEVICENAME, std::ptr::null_mut(), &mut len);
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = unsafe { GetRawInputDeviceInfoW(dev, RIDI_DEVICENAME, buf.as_mut_ptr().cast(), &mut len) };
    if n == u32::MAX {
        return false;
    }
    String::from_utf16_lossy(&buf).to_uppercase().contains(&format!("VID_{vid:04X}"))
}

fn listen() {
    let Some(hwnd) = win::message_window("WhiskerRawInput", Some(wndproc)) else {
        return;
    };

    // INPUTSINK: receive mouse input even when our window is not focused.
    let dev = RAWINPUTDEVICE {
        usUsagePage: MOUSE_DESKTOP,
        usUsage: USAGE_MOUSE,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe {
        RegisterRawInputDevices(&dev, 1, size_of::<RAWINPUTDEVICE>() as u32);
    }
    win::pump_messages();
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        if let (Some(input), Some(on)) = (unsafe { read_raw(lp) }, HANDLER.get()) {
            on(&decode(&input));
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe fn read_raw(lp: LPARAM) -> Option<RAWINPUT> {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let n = unsafe { GetRawInputData(lp as HRAWINPUT, RID_INPUT, (&mut raw as *mut RAWINPUT).cast(), &mut size, header) };

    if n == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE {
        return None;
    }
    Some(raw)
}

fn decode(raw: &RAWINPUT) -> MouseInput {
    let m = unsafe { raw.data.mouse };
    let flags = unsafe { m.Anonymous.Anonymous.usButtonFlags };
    let relative = m.usFlags & MOUSE_MOVE_ABSOLUTE == 0;
    let (dx, dy) = if relative { (m.lLastX, m.lLastY) } else { (0, 0) };
    MouseInput {
        dev: raw.header.hDevice as usize,
        left: flags & BTN_LEFT != 0,
        right: flags & BTN_RIGHT != 0,
        middle: flags & BTN_MIDDLE != 0,
        side: flags & (BTN_4 | BTN_5) != 0,
        wheel: flags & BTN_WHEEL != 0,
        dx,
        dy,
    }
}
