//! System queries: user folders, UI language, light/dark setting, clock.

use super::win::{Folder, known_folder, wide};
use std::path::PathBuf;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW};
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

/// File name of the program whose window is in focus, e.g. "game.exe".
/// `None` when no window is, or its process may not be asked (a program
/// run as administrator).
pub fn foreground_app() -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    /// Room for the program's full path, in UTF-16 units.
    const PATH_CHARS: usize = 1024;

    let mut path = [0u16; PATH_CHARS];
    let mut len = PATH_CHARS as u32;
    // SAFETY: plain queries; the handle is closed, `path` has `len` units.
    let found = unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, &mut pid);
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let found = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut len);
        CloseHandle(process);
        found
    };
    if found == 0 {
        return None;
    }
    let full = PathBuf::from(String::from_utf16_lossy(&path[..len as usize]));
    Some(full.file_name()?.to_string_lossy().into_owned())
}

/// Windows always tells which window is in focus.
pub fn foreground_known() -> bool {
    true
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

/// OS name and build for bug reports, e.g. `"Windows 11 26300"`. The
/// registry's ProductName still says "Windows 10" on 11, so the build
/// number decides.
pub fn os_version() -> String {
    const KEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    const BUILD: &str = "CurrentBuild";
    /// First build of Windows 11.
    const WIN11_BUILD: u32 = 22000;
    const CHARS: usize = 32;

    let (key, name) = (wide(KEY), wide(BUILD));
    let mut buf = [0u16; CHARS];
    let mut size = (CHARS * size_of::<u16>()) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if r != 0 {
        return "Windows".into();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(CHARS);
    let build = String::from_utf16_lossy(&buf[..len]);
    let major = if build.parse::<u32>().is_ok_and(|b| b >= WIN11_BUILD) { 11 } else { 10 };
    format!("Windows {major} {build}")
}

/// Local date as `(year, month, day)`.
pub fn today() -> (i64, i64, i64) {
    let mut t: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    (t.wYear as i64, t.wMonth as i64, t.wDay as i64)
}
