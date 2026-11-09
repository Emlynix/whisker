//! Start at login via an XDG autostart entry. The launch line adds
//! `--tray` so the app starts hidden in the tray. Always points at the
//! installed copy: a binary in e.g. Downloads never runs at login.
//!
//! ```text
//!   ~/.config/autostart/com.emlynix.whisker.desktop
//!     Exec="~/.local/share/whisker/whisker" --tray
//! ```
//! The session reads it at the next login; nothing is started right away.

use super::install::{desktop_entry, installed_exe};
use super::sys::config_dir;
use super::window::APP_ID;
use crate::platform::{Autostart, TRAY_ARG};
use std::path::PathBuf;

/// Keys only the autostart entry has.
const AUTOSTART_KEYS: &str = "NoDisplay=true\nX-GNOME-Autostart-enabled=true\n";

fn entry_path() -> Option<PathBuf> {
    Some(config_dir()?.join("autostart").join(format!("{APP_ID}.desktop")))
}

/// The entry we write, only if the installed binary exists.
fn entry() -> Option<String> {
    let exe = installed_exe().filter(|p| p.is_file())?;
    Some(desktop_entry(&exe, Some(TRAY_ARG))? + AUTOSTART_KEYS)
}

/// True only when the entry points at our installed copy.
pub fn is_enabled() -> bool {
    let (Some(path), Some(ours)) = (entry_path(), entry()) else {
        return false;
    };
    std::fs::read_to_string(path).is_ok_and(|stored| stored == ours)
}

/// `On` fails when the app is not installed.
pub fn set(mode: Autostart) -> bool {
    let Some(path) = entry_path() else {
        return false;
    };

    if mode == Autostart::Off {
        return match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
    }

    let Some(text) = entry() else {
        return false;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, text).is_ok()
}
