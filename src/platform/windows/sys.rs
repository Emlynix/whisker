//! System queries: user folders, UI language, light/dark setting, clock.

use super::win::{Folder, known_folder, wide};
use std::path::PathBuf;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

const PERSONALIZE_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
const LIGHT_VALUE: &str = "AppsUseLightTheme";
/// LOCALE_NAME_MAX_LENGTH.
const LOCALE_CHARS: usize = 85;

/// Roaming `%APPDATA%`, resolved by the shell.
pub fn config_dir() -> Option<PathBuf> {
    known_folder(Folder::RoamingAppData)
}

/// Windows UI language as a locale name, e.g. `"sk-SK"`, `"zh-TW"`.
pub fn user_locale() -> Option<String> {
    let mut buf = [0u16; LOCALE_CHARS];
    // A LANGID with default sort order is its LCID.
    let lcid = unsafe { GetUserDefaultUILanguage() } as u32;
    let n = unsafe { LCIDToLocaleName(lcid, buf.as_mut_ptr(), buf.len() as i32, 0) };
    if n <= 1 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n as usize - 1]))
}

/// Apps use the dark theme (anything but `AppsUseLightTheme = 1`).
pub fn dark_mode() -> bool {
    let (key, name) = (wide(PERSONALIZE_KEY), wide(LIGHT_VALUE));
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size)
    };
    !(r == 0 && value == 1)
}

/// Opens an `http(s)` link in the default browser; other schemes are refused
/// so a crafted string can never start a program.
pub fn open_url(url: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    /// ShellExecute returns a value above 32 on success.
    const SHELL_OK: isize = 32;

    if !crate::platform::is_web_url(url) {
        return false;
    }
    let (verb, file) = (wide("open"), wide(url));
    let r = unsafe { ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL) };
    r as isize > SHELL_OK
}

/// Local date as `(year, month, day)`.
pub fn today() -> (i64, i64, i64) {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    (t.wYear as i64, t.wMonth as i64, t.wDay as i64)
}
