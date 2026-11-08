//! Start with Windows via `HKCU\...\Run`. Launch line adds `--tray` so the
//! app starts hidden in the notification area. Always points at the
//! installed copy: a portable exe in e.g. Downloads never runs at login.

use super::win::wide;
use crate::platform::{Autostart, TRAY_ARG};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::*;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = crate::APP_NAME;
const MAX_LINE: usize = 1024;

/// `"C:\...\Programs\Whisker\Whisker.exe" --tray`, only if that exe exists.
fn launch_line() -> Option<String> {
    let exe = super::install::installed_exe().filter(|p| p.is_file())?;
    Some(format!("\"{}\" {TRAY_ARG}", exe.display()))
}

fn stored_line() -> Option<String> {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));
    let mut buf = vec![0u16; MAX_LINE];
    let mut size = (buf.len() * size_of::<u16>()) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut size)
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// True only when the Run entry points at our installed copy.
pub fn is_enabled() -> bool {
    match (stored_line(), launch_line()) {
        (Some(stored), Some(ours)) => stored.eq_ignore_ascii_case(&ours),
        _ => false,
    }
}

/// `On` fails when the app is not installed.
pub fn set(mode: Autostart) -> bool {
    let (key, name) = (wide(RUN_KEY), wide(VALUE_NAME));

    if mode == Autostart::Off {
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        return r == ERROR_SUCCESS;
    }

    let Some(line) = launch_line() else {
        return false;
    };
    let data = wide(&line);
    let bytes = (data.len() * size_of::<u16>()) as u32;
    let r = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, data.as_ptr().cast(), bytes) };
    r == ERROR_SUCCESS
}
