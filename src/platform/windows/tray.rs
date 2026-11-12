//! Notification-area icon via `Shell_NotifyIconW`, showing the battery
//! glyph of `crate::tray`.

use super::win::wide;
use crate::i18n::{T, tr};
use crate::tray::{ICON_SIZE, Level, TrayAction, glyph};
use std::cell::{Cell, RefCell};
use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Message the shell sends to our window for icon clicks.
pub const WM_TRAY: u32 = WM_APP + 2;

const ICON_ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_QUIT: usize = 2;
const MENU_ABOUT: usize = 3;
/// Id of the first profile in the menu; the others follow.
const MENU_PROFILE: usize = 100;

thread_local! {
    static ICON: Cell<HICON> = const { Cell::new(std::ptr::null_mut()) };
    static LEVEL: Cell<Level> = const { Cell::new(Level::Offline) };
    /// Profile names for the menu and the active one's index.
    static PROFILES: RefCell<(Vec<String>, usize)> = const { RefCell::new((Vec::new(), 0)) };
}

/// The menu's profile list; none hides it.
pub fn set_profiles(names: &[String], active: usize) {
    PROFILES.set((names.to_vec(), active));
}

fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = hwnd;
    d.uID = ICON_ID;
    d
}

/// Copies `tip` into a fixed UTF-16 field, cut to fit, NUL-terminated.
fn copy_tip<const N: usize>(dst: &mut [u16; N], tip: &str) {
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

/// Balloon (a toast on Windows 10/11) next to the icon; a click on it
/// comes back as `TrayAction::Open`.
pub fn notify(hwnd: HWND, title: &str, text: &str) {
    let mut d = data(hwnd);
    d.uFlags = NIF_INFO;
    d.dwInfoFlags = NIIF_INFO;
    copy_tip(&mut d.szInfoTitle, title);
    copy_tip(&mut d.szInfo, text);
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &d) };
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

/// Handles `WM_TRAY`: a double click or a click on our notification
/// opens, right click shows the menu.
pub fn on_message(hwnd: HWND, lparam: isize) -> Option<TrayAction> {
    let event = (lparam & 0xFFFF) as u32;
    if opens(event) {
        return Some(TrayAction::Open);
    }
    if event == WM_RBUTTONUP {
        return menu(hwnd);
    }
    None
}

/// A single click only passes by (the tooltip tells the battery); as
/// with other tray icons, the window takes a double click.
fn opens(event: u32) -> bool {
    matches!(event, WM_LBUTTONDBLCLK | NIN_BALLOONUSERCLICK)
}

fn menu(hwnd: HWND) -> Option<TrayAction> {
    let (open, about, quit) = (wide(tr(T::TrayOpen)), wide(tr(T::About)), wide(tr(T::TrayQuit)));
    unsafe {
        let m = CreatePopupMenu();
        AppendMenuW(m, MF_STRING, MENU_OPEN, open.as_ptr());
        AppendMenuW(m, MF_STRING, MENU_ABOUT, about.as_ptr());
        let count = profile_menu(m);
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
            MENU_ABOUT => Some(TrayAction::About),
            MENU_QUIT => Some(TrayAction::Quit),
            id if (MENU_PROFILE..MENU_PROFILE + count).contains(&id) => Some(TrayAction::Profile(id - MENU_PROFILE)),
            _ => None,
        }
    }
}

/// Appends "Profile ▸" with the profiles, the active one marked. Returns
/// how many it lists.
fn profile_menu(m: HMENU) -> usize {
    let (names, active) = PROFILES.with_borrow(|p| p.clone());
    if names.is_empty() {
        return 0;
    }
    let title = wide(tr(T::Profile));
    unsafe {
        let sub = CreatePopupMenu();
        for (i, name) in names.iter().enumerate() {
            let label = wide(&plain_label(name));
            AppendMenuW(sub, MF_STRING, MENU_PROFILE + i, label.as_ptr());
        }
        let (first, last) = (MENU_PROFILE as u32, (MENU_PROFILE + names.len() - 1) as u32);
        CheckMenuRadioItem(sub, first, last, (MENU_PROFILE + active) as u32, MF_BYCOMMAND);
        // The parent menu owns the list: destroyed with it.
        AppendMenuW(m, MF_POPUP, sub as usize, title.as_ptr());
    }
    names.len()
}

/// A name as menu text: `&` would underline the next letter.
fn plain_label(name: &str) -> String {
    name.replace('&', "&&")
}

/// Turns the battery glyph into an icon.
fn draw(level: Level) -> HICON {
    let px = glyph(level);
    let size = ICON_SIZE as i32;

    // 32-bit BGRA color bitmap; alpha drives transparency, the mask is unused.
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, px.as_ptr().cast());
        let mask_bits = vec![0u8; ICON_SIZE * ICON_SIZE / 8];
        let mask = CreateBitmap(size, size, 1, 1, mask_bits.as_ptr().cast());
        let info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info);
        DeleteObject(color);
        DeleteObject(mask);
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_name_is_no_menu_mnemonic() {
        assert_eq!(plain_label("R&D"), "R&&D");
        assert_eq!(plain_label("FPS"), "FPS");
    }

    #[test]
    fn the_window_opens_on_a_double_click_only() {
        assert!(opens(WM_LBUTTONDBLCLK));
        assert!(opens(NIN_BALLOONUSERCLICK));
        // Issue #6: a single click opened it by accident.
        assert!(!opens(WM_LBUTTONUP));
        assert!(!opens(WM_LBUTTONDOWN));
    }
}
