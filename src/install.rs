//! Per-user install, no admin rights needed:
//!
//! ```text
//!   %LOCALAPPDATA%\Programs\Whisker\Whisker.exe    the app
//!   Start menu\Programs\Whisker.lnk                shortcut
//!   HKCU\...\Uninstall\Whisker                     "Apps > Uninstall" entry
//!                                                  -> Whisker.exe --uninstall
//! ```

use crate::autostart::{self, Autostart};
use crate::store;
use crate::win::{Folder, known_folder, system_dir, wide};
use std::path::{Path, PathBuf};
use std::process::Command;
use windows_sys::Win32::System::Registry::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};

pub const UNINSTALL_ARG: &str = "--uninstall";
/// Passed to the freshly installed copy so it waits for the old process to exit.
pub const INSTALLED_ARG: &str = "--installed";

const EXE_NAME: &str = "Whisker.exe";
const LINK_NAME: &str = "Whisker.lnk";
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Whisker";
const DISPLAY_NAME: &str = "Whisker – mouse control";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// How long uninstall waits for a running instance to save and exit.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
pub const AUTOSTART_ON_ARG: &str = "--autostart-on";
pub const AUTOSTART_OFF_ARG: &str = "--autostart-off";

pub fn install_dir() -> Option<PathBuf> {
    Some(known_folder(Folder::LocalAppData)?.join("Programs").join(crate::APP_NAME))
}

pub fn installed_exe() -> Option<PathBuf> {
    Some(install_dir()?.join(EXE_NAME))
}

fn start_menu_link() -> Option<PathBuf> {
    Some(known_folder(Folder::StartPrograms)?.join(LINK_NAME))
}

/// True when this process runs from the install folder.
pub fn is_installed_copy() -> bool {
    let (Ok(cur), Some(target)) = (std::env::current_exe(), installed_exe()) else {
        return false;
    };
    let canon = |p: &PathBuf| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
    canon(&cur) == canon(&target)
}

/// Copies the exe, adds Start menu + uninstall entries, optional autostart,
/// then launches the installed copy. Caller must quit right after `Ok`.
pub fn install(mode: Autostart) -> Result<(), String> {
    let dir = install_dir().ok_or("LocalAppData")?;
    let exe = dir.join(EXE_NAME);
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;

    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::copy(&cur, &exe).map_err(|e| format!("Kopírovanie zlyhalo: {e}"))?;

    if let Some(link) = start_menu_link() {
        let _ = shell_link::create(&exe, &link);
    }
    register_uninstall(&exe, &dir);

    // Autostart must point at the installed copy, so set it from there.
    let arg = if mode == Autostart::On { AUTOSTART_ON_ARG } else { AUTOSTART_OFF_ARG };
    Command::new(&exe).args([INSTALLED_ARG, arg]).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn set_str(key: &[u16], name: &str, value: &str) {
    let (n, v) = (wide(name), wide(value));
    let bytes = (v.len() * 2) as u32;
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), n.as_ptr(), REG_SZ, v.as_ptr().cast(), bytes) };
}

fn set_dword(key: &[u16], name: &str, value: u32) {
    let n = wide(name);
    let p = (&value as *const u32).cast();
    unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), n.as_ptr(), REG_DWORD, p, 4) };
}

fn register_uninstall(exe: &std::path::Path, dir: &std::path::Path) {
    let key = wide(UNINSTALL_KEY);
    let exe_s = exe.display().to_string();
    let size_kb = std::fs::metadata(exe).map(|m| (m.len() / 1024) as u32).unwrap_or(0);

    set_str(&key, "DisplayName", DISPLAY_NAME);
    set_str(&key, "DisplayIcon", &format!("{exe_s},0"));
    set_str(&key, "DisplayVersion", env!("CARGO_PKG_VERSION"));
    set_str(&key, "Publisher", crate::APP_NAME);
    set_str(&key, "InstallLocation", &dir.display().to_string());
    set_str(&key, "UninstallString", &format!("\"{exe_s}\" {UNINSTALL_ARG}"));
    set_dword(&key, "EstimatedSize", size_kb);
    set_dword(&key, "NoModify", 1);
    set_dword(&key, "NoRepair", 1);
}

fn confirm(text: &str) -> bool {
    let (t, c) = (wide(text), wide(crate::APP_NAME));
    unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Started from Windows "Apps" (`--uninstall`); another instance may run.
    External,
    /// Clicked inside the running app, which quits right after.
    InApp,
}

/// Removes everything `install` created plus settings and statistics.
/// Returns false when the user cancels.
pub fn uninstall(origin: Origin) -> bool {
    if !confirm(crate::i18n::tr(crate::i18n::T::UninstallConfirm)) {
        return false;
    }

    // Let a running instance save and exit before its files are removed.
    if origin == Origin::External {
        crate::single::request_quit();
        crate::single::wait_closed(QUIT_WAIT);
    }
    autostart::set(Autostart::Off);

    let key = wide(UNINSTALL_KEY);
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr()) };
    if let Some(link) = start_menu_link() {
        let _ = std::fs::remove_file(link);
    }
    store::remove_data();

    // The running exe can't delete itself: a detached shell deletes it and the
    // (then empty) folder after this process exits. Only our own exe is
    // deleted; `rmdir` without /s leaves the folder if anything else is in it.
    let (Some(sys), Some(dir)) = (system_dir(), install_dir()) else {
        return true;
    };
    let Some(script) = cleanup_script(&sys, &dir.join(EXE_NAME), &dir) else {
        return true;
    };
    use std::os::windows::process::CommandExt;
    // raw_arg: cmd.exe does not understand Rust's \" quote escaping.
    // /D: skip the user's cmd AutoRun hook.
    let _ = Command::new(sys.join("cmd.exe"))
        .args(["/D", "/C"])
        .raw_arg(&script)
        .current_dir(&sys)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    true
}

/// `cmd /C` line that waits ~2 s (ping), deletes `exe`, then removes `dir` if
/// empty. `None` for relative paths or characters cmd would interpret
/// (`%` expands variables, `"` ends quoting) — then nothing is deleted.
fn cleanup_script(sys: &Path, exe: &Path, dir: &Path) -> Option<String> {
    let paths = [sys, exe, dir];
    let unsafe_char = |p: &Path| p.to_string_lossy().contains(['%', '"', '^', '!']);
    if paths.iter().any(|p| !p.is_absolute() || unsafe_char(p)) {
        return None;
    }

    let ping = sys.join("ping.exe");
    let inner = format!("\"{}\" -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"", ping.display(), exe.display(), dir.display());
    // Outer quotes: cmd strips the first and last quote of a /C line.
    Some(format!("\"{inner}\""))
}

mod shell_link {
    use std::path::Path;
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{HSTRING, Interface};

    pub fn create(target: &Path, link: &Path) -> windows::core::Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            sl.SetPath(&HSTRING::from(target.as_os_str()))?;
            sl.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
            sl.SetDescription(&HSTRING::from("Whisker – mouse control"))?;

            let pf: IPersistFile = sl.cast()?;
            pf.Save(&HSTRING::from(link.as_os_str()), true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn cleanup_script_uses_full_paths_and_outer_quotes() {
        let s = cleanup_script(Path::new(r"C:\Windows\System32"), Path::new(r"C:\Users\a\AppData\Local\Programs\Whisker\Whisker.exe"), Path::new(r"C:\Users\a\AppData\Local\Programs\Whisker")).unwrap();
        assert_eq!(
            s,
            r#"""C:\Windows\System32\ping.exe" -n 3 127.0.0.1 >nul & del /f /q "C:\Users\a\AppData\Local\Programs\Whisker\Whisker.exe" & rmdir "C:\Users\a\AppData\Local\Programs\Whisker"""#
        );
    }

    #[test]
    fn cleanup_script_rejects_cmd_metacharacters() {
        let sys = Path::new(r"C:\Windows\System32");
        assert!(cleanup_script(sys, Path::new(r"C:\Users\%x%\L.exe"), Path::new(r"C:\Users\%x%")).is_none());
        assert!(cleanup_script(sys, Path::new(r#"C:\a"b\L.exe"#), Path::new(r#"C:\a"b"#)).is_none());
        assert!(cleanup_script(sys, Path::new(r"Programs\Whisker\L.exe"), Path::new(r"Programs\Whisker")).is_none());
    }
}
