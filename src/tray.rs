//! Notification-area icon via `Shell_NotifyIconW`. The icon is a battery
//! drawn at runtime whose fill shows the charge level:
//!
//! ```text
//!   +--------------+-+
//!   |██████████    | |   green > 50 %, amber > 20 %, red below,
//!   +--------------+-+   blue while charging, empty when offline
//! ```

use crate::i18n::{T, tr};
use crate::win::wide;
use std::cell::Cell;
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Message the shell sends to our window for icon clicks.
pub const WM_TRAY: u32 = WM_APP + 2;

const ICON_ID: u32 = 1;
const SIZE: i32 = 32;
const LOW: u8 = 20;
const MID: u8 = 50;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;

const GREEN: u32 = 0x3ddc84;
const AMBER: u32 = 0xffb020;
const RED: u32 = 0xff4d4f;
const BLUE: u32 = 0x3ab8ff;
const FRAME: u32 = 0xe8ecf2;
const FRAME_OFF: u32 = 0x808894;

pub enum TrayAction {
    Open,
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Offline,
    Battery { pct: u8, charging: bool },
}

thread_local! {
    static ICON: Cell<HICON> = const { Cell::new(std::ptr::null_mut()) };
    static LEVEL: Cell<Level> = const { Cell::new(Level::Offline) };
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = hwnd;
    d.uID = ICON_ID;
    d
}

fn copy_tip(dst: &mut [u16; 128], tip: &str) {
    let w: Vec<u16> = tip.encode_utf16().take(dst.len() - 1).collect();
    dst[..w.len()].copy_from_slice(&w);
    dst[w.len()] = 0;
}

/// Adds the icon; call again after Explorer restarts.
pub fn add(hwnd: HWND, tip: &str) {
    let icon = draw(LEVEL.get());
    let mut d = data(hwnd);
    d.uFlags = NIF_ICON | NIF_TIP | NIF_MESSAGE;
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = icon;
    copy_tip(&mut d.szTip, tip);
    unsafe { Shell_NotifyIconW(NIM_ADD, &d) };
    replace_icon(icon);
}

pub fn update(hwnd: HWND, level: Level, tip: &str) {
    LEVEL.set(level);
    let icon = draw(level);
    let mut d = data(hwnd);
    d.uFlags = NIF_ICON | NIF_TIP;
    d.hIcon = icon;
    copy_tip(&mut d.szTip, tip);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
    replace_icon(icon);
}

pub fn remove(hwnd: HWND) {
    let d = data(hwnd);
    unsafe { Shell_NotifyIconW(NIM_DELETE, &d) };
    replace_icon(std::ptr::null_mut());
}

fn replace_icon(new: HICON) {
    let old = ICON.replace(new);
    if !old.is_null() {
        unsafe { DestroyIcon(old) };
    }
}

/// Handles `WM_TRAY`: left click opens, right click shows the menu.
pub fn on_message(hwnd: HWND, lparam: isize) -> Option<TrayAction> {
    match (lparam & 0xFFFF) as u32 {
        WM_LBUTTONUP => Some(TrayAction::Open),
        WM_RBUTTONUP => menu(hwnd),
        _ => None,
    }
}

fn menu(hwnd: HWND) -> Option<TrayAction> {
    let (open, quit) = (wide(tr(T::TrayOpen)), wide(tr(T::TrayQuit)));
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(m, MF_STRING, MENU_QUIT, quit.as_ptr());

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // Required so the menu closes when clicking elsewhere.
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, hwnd, std::ptr::null());
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(m);

        match cmd as usize {
            MENU_OPEN => Some(TrayAction::Open),
            MENU_QUIT => Some(TrayAction::Quit),
            _ => None,
        }
    }
}

/// Renders the battery glyph into a 32x32 icon.
fn draw(level: Level) -> HICON {
    let mut px = vec![0u32; (SIZE * SIZE) as usize];
    let mut put = |x: i32, y: i32, rgb: u32| px[(y * SIZE + x) as usize] = 0xFF00_0000 | rgb;

    // Body 2..27 x 8..24 with a 2 px frame, terminal 27..30 x 13..19.
    let (x0, x1, y0, y1) = (2, 27, 8, 24);
    let frame = if level == Level::Offline { FRAME_OFF } else { FRAME };
    for y in y0..y1 {
        for x in x0..x1 {
            if x < x0 + 2 || x >= x1 - 2 || y < y0 + 2 || y >= y1 - 2 {
                put(x, y, frame);
            }
        }
    }
    for y in 13..19 {
        for x in x1..x1 + 3 {
            put(x, y, frame);
        }
    }

    if let Level::Battery { pct, charging } = level {
        let color = match pct {
            _ if charging => BLUE,
            p if p <= LOW => RED,
            p if p <= MID => AMBER,
            _ => GREEN,
        };
        let inner = (x1 - 4) - (x0 + 4);
        let width = (inner * pct.max(8) as i32 / 100).max(2);
        for y in y0 + 4..y1 - 4 {
            for x in x0 + 4..x0 + 4 + width {
                put(x, y, color);
            }
        }
    }

    // 32-bit BGRA color bitmap; alpha drives transparency, the mask is unused.
    unsafe {
        let color = CreateBitmap(SIZE, SIZE, 1, 32, px.as_ptr().cast());
        let mask_bits = vec![0u8; (SIZE * SIZE / 8) as usize];
        let mask = CreateBitmap(SIZE, SIZE, 1, 1, mask_bits.as_ptr().cast());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}
