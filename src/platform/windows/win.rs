//! Small Win32 helpers shared by platform modules.

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy)]
pub enum Folder {
    LocalAppData,
    RoamingAppData,
    /// Start menu `Programs` folder of the user.
    StartPrograms,
}

/// Known folder from the shell (not env vars, which any parent process can
/// fake). Only absolute paths are returned.
pub fn known_folder(f: Folder) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::*;

    let id = match f {
        Folder::LocalAppData => FOLDERID_LocalAppData,
        Folder::RoamingAppData => FOLDERID_RoamingAppData,
        Folder::StartPrograms => FOLDERID_Programs,
    };
    let mut p: windows_sys::core::PWSTR = std::ptr::null_mut();
    let hr = unsafe { SHGetKnownFolderPath(&id, 0, std::ptr::null_mut(), &mut p) };
    if hr != 0 || p.is_null() {
        return None;
    }

    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) });
    unsafe { CoTaskMemFree(p.cast()) };

    let path = std::path::PathBuf::from(s);
    path.is_absolute().then_some(path)
}

/// `C:\Windows\System32`, for running system tools by full path.
pub fn system_dir() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

/// Null-terminated UTF-16 string for Win32 calls.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Creates a hidden message-only window with the given window procedure.
pub fn message_window(class: &str, proc: WNDPROC) -> Option<HWND> {
    let name = wide(class);
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: proc,
            hInstance: module,
            lpszClassName: name.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            module,
            std::ptr::null(),
        );
        (!hwnd.is_null()).then_some(hwnd)
    }
}

/// Runs a message loop on the current thread until WM_QUIT.
pub fn pump_messages() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}
