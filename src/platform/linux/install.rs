//! Per-user install, no root needed (XDG data folder):
//!
//! ```text
//!   ~/.local/share/whisker/whisker                      the binary
//!   ~/.local/share/whisker/whisker.png                  icon
//!   ~/.local/share/applications/com.emlynix.whisker.desktop
//!        app menu entry, with an "Uninstall" action
//!   ~/.config/autostart/com.emlynix.whisker.desktop     autostart (optional)
//! ```

use super::autostart;
use super::sys::data_dir;
use super::window::{APP_ID, app_icon};
use crate::i18n::{T, tr};
use crate::platform::{AUTOSTART_OFF_ARG, AUTOSTART_ON_ARG, Autostart, INSTALLED_ARG, Origin, UNINSTALL_ARG};
use crate::store;
use gtk::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;

const DIR_NAME: &str = "whisker";
const EXE_NAME: &str = "whisker";
/// Staging copy next to the target, renamed into place when complete.
const STAGING_NAME: &str = ".whisker.new";
const ICON_NAME: &str = "whisker.png";
const ICON_FORMAT: &str = "png";
/// Owner rwx, others rx.
const EXE_MODE: u32 = 0o755;
/// How long uninstall waits for a running instance to save and exit.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn install_dir() -> Option<PathBuf> {
    Some(data_dir()?.join(DIR_NAME))
}

pub fn installed_exe() -> Option<PathBuf> {
    Some(install_dir()?.join(EXE_NAME))
}

fn icon_path() -> Option<PathBuf> {
    Some(install_dir()?.join(ICON_NAME))
}

fn launcher_path() -> Option<PathBuf> {
    Some(data_dir()?.join("applications").join(format!("{APP_ID}.desktop")))
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// True when this process runs from the installed copy.
pub fn is_installed_copy() -> bool {
    let (Ok(cur), Some(target)) = (std::env::current_exe(), installed_exe()) else {
        return false;
    };
    canon(&cur) == canon(&target)
}

/// `Exec` value: the program quoted per the Desktop Entry spec (`"`,
/// `` ` ``, `$`, `\` escaped inside quotes, then `\` doubled and `%`
/// doubled for the file format), then the optional argument.
/// `None` for paths a desktop file can't hold (line breaks).
fn exec_line(exe: &Path, arg: Option<&str>) -> Option<String> {
    let path = exe.to_str()?;
    if path.chars().any(|c| c.is_control()) {
        return None;
    }
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    let mut line = quoted.replace('\\', "\\\\").replace('%', "%%");
    if let Some(a) = arg {
        line.push(' ');
        line.push_str(a);
    }
    Some(line)
}

/// `[Desktop Entry]` group launching `exe` (with `arg`).
pub(super) fn desktop_entry(exe: &Path, arg: Option<&str>) -> Option<String> {
    let exec = exec_line(exe, arg)?;
    let icon = icon_path().and_then(|p| p.to_str().map(str::to_string)).unwrap_or_default();
    let name = crate::APP_NAME;
    Some(format!("[Desktop Entry]\nType=Application\nName={name}\nExec={exec}\nIcon={icon}\nTerminal=false\nStartupWMClass={APP_ID}\n"))
}

/// App menu entry with an Uninstall action.
fn launcher(exe: &Path) -> Option<String> {
    let main = desktop_entry(exe, None)?;
    let uninstall = exec_line(exe, Some(UNINSTALL_ARG))?;
    let label = tr(T::Uninstall).replace(['\n', '\r'], " ");
    Some(format!("{main}Categories=Settings;HardwareSettings;\nActions=uninstall;\n\n[Desktop Action uninstall]\nName={label}\nExec={uninstall}\n"))
}

/// Copies the binary into the staging folder, made executable.
fn stage(cur: &Path, staging: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(staging)?;
    let exe = staging.join(EXE_NAME);
    std::fs::copy(cur, &exe)?;
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(EXE_MODE))?;
    if let Some(icon) = app_icon() {
        let _ = icon.savev(staging.join(ICON_NAME), ICON_FORMAT, &[]);
    }
    Ok(())
}

/// Copies the binary to `~/.local/share/whisker`, adds the menu entry and
/// optional autostart, then launches the installed copy. Caller must quit
/// right after `Ok`.
pub fn install(mode: Autostart) -> Result<(), String> {
    if is_installed_copy() {
        return Err("Already installed".into());
    }
    let target = install_dir().ok_or("Home")?;
    let staging = target.parent().ok_or("Home")?.join(STAGING_NAME);
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;

    // Stage the full copy first, so a failure never leaves half an install.
    let _ = std::fs::remove_dir_all(&staging);
    stage(&cur, &staging).map_err(|e| format!("Copy failed: {e}"))?;
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;

    let exe = installed_exe().ok_or("Home")?;
    if let (Some(path), Some(text)) = (launcher_path(), launcher(&exe)) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
    }

    // Autostart must point at the installed copy, so set it from there.
    let arg = if mode == Autostart::On { AUTOSTART_ON_ARG } else { AUTOSTART_OFF_ARG };
    Command::new(&exe).args([INSTALLED_ARG, arg]).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Modal Uninstall / Cancel question; also works before `window::run`.
fn confirm(text: &str) -> bool {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return false;
    }
    let dialog = gtk::MessageDialog::new(None::<&gtk::Window>, gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::None, text);
    dialog.set_title(crate::APP_NAME);
    dialog.add_button(tr(T::Cancel), gtk::ResponseType::Cancel);
    dialog.add_button(tr(T::Uninstall), gtk::ResponseType::Accept);
    let answer = dialog.run();
    dialog.close();
    answer == gtk::ResponseType::Accept
}

/// Removes everything `install` created plus settings and statistics.
/// Returns false when the user cancels.
pub fn uninstall(origin: Origin) -> bool {
    if !confirm(tr(T::UninstallConfirm)) {
        return false;
    }

    // Let a running instance save and exit before its files are removed.
    if origin == Origin::External {
        super::single::request_quit();
        super::single::wait_closed(QUIT_WAIT);
    }
    autostart::set(Autostart::Off);
    store::remove_data();

    if let Some(path) = launcher_path() {
        let _ = std::fs::remove_file(path);
    }
    // A running binary may be deleted on Linux; the process keeps its image.
    if let Some(dir) = install_dir().filter(|d| d.is_dir()) {
        let _ = std::fs::remove_dir_all(dir);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_line_quotes_per_spec() {
        let p = Path::new("/home/a b/whisker");
        assert_eq!(exec_line(p, Some("--tray")).as_deref(), Some("\"/home/a b/whisker\" --tray"));
        // `$` escaped inside quotes, then the backslash doubled for the file.
        assert_eq!(exec_line(Path::new("/x/$y%"), None).as_deref(), Some("\"/x/\\\\$y%%\""));
        assert_eq!(exec_line(Path::new("/x\ny"), None), None);
    }
}
