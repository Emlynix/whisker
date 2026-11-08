//! Per-user install, no admin rights needed:
//!
//! ```text
//!   ~/Applications/Whisker.app                         the app bundle
//!   ~/Library/LaunchAgents/com.emlynix.whisker.plist       autostart (optional)
//! ```
//! Run from a bundle, the whole bundle is copied; run as a bare binary
//! (e.g. `cargo run`), a minimal bundle is built around it.

use super::autostart;
use super::sys::home_dir;
use crate::i18n::{T, tr};
use crate::platform::{AUTOSTART_OFF_ARG, AUTOSTART_ON_ARG, Autostart, INSTALLED_ARG, Origin};
use crate::store;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication};
use objc2_foundation::NSString;
use std::path::{Path, PathBuf};
use std::process::Command;

const BUNDLE_NAME: &str = "Whisker.app";
/// Staging copy next to the target, renamed into place when complete.
const STAGING_NAME: &str = ".Whisker.app.new";
const EXE_NAME: &str = "Whisker";
/// Info.plist for a bundle built around a bare binary.
const INFO_PLIST: &str = include_str!("../../../bundle/macos/Info.plist");
/// How long uninstall waits for a running instance to save and exit.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

fn apps_dir() -> Option<PathBuf> {
    Some(home_dir()?.join("Applications"))
}

pub fn install_dir() -> Option<PathBuf> {
    Some(apps_dir()?.join(BUNDLE_NAME))
}

pub fn installed_exe() -> Option<PathBuf> {
    Some(install_dir()?.join("Contents").join("MacOS").join(EXE_NAME))
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// True when this process runs from the installed bundle.
pub fn is_installed_copy() -> bool {
    let (Ok(cur), Some(target)) = (std::env::current_exe(), installed_exe()) else {
        return false;
    };
    canon(&cur) == canon(&target)
}

/// `X.app` around `exe` when it runs as `X.app/Contents/MacOS/exe`.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent().filter(|p| p.ends_with("MacOS"))?;
    let contents = macos.parent().filter(|p| p.ends_with("Contents"))?;
    contents.parent().filter(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

/// Copies a folder tree; symlinks stay symlinks, permissions are kept.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
            continue;
        }
        if kind.is_dir() {
            copy_tree(&src, &dst)?;
            continue;
        }
        std::fs::copy(&src, &dst)?;
    }
    Ok(())
}

/// Minimal bundle: the binary plus Info.plist.
fn build_bundle(exe: &Path, to: &Path) -> std::io::Result<()> {
    let contents = to.join("Contents");
    std::fs::create_dir_all(contents.join("MacOS"))?;
    std::fs::copy(exe, contents.join("MacOS").join(EXE_NAME))?;
    std::fs::write(contents.join("Info.plist"), INFO_PLIST)
}

/// Copies the bundle to `~/Applications`, optional autostart, then
/// launches the installed copy. Caller must quit right after `Ok`.
pub fn install(mode: Autostart) -> Result<(), String> {
    if is_installed_copy() {
        return Err("Already installed".into());
    }
    let (apps, target) = (apps_dir().ok_or("Home")?, install_dir().ok_or("Home")?);
    let cur = std::env::current_exe().map_err(|e| e.to_string())?;
    let staging = apps.join(STAGING_NAME);

    // Stage the full copy first, so a failure never leaves half a bundle.
    let _ = std::fs::remove_dir_all(&staging);
    let copied = match bundle_of(&cur) {
        Some(bundle) => copy_tree(&bundle, &staging),
        None => build_bundle(&cur, &staging),
    };
    copied.map_err(|e| format!("Copy failed: {e}"))?;
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;

    // Autostart must point at the installed copy, so set it from there.
    let exe = installed_exe().ok_or("Home")?;
    let arg = if mode == Autostart::On { AUTOSTART_ON_ARG } else { AUTOSTART_OFF_ARG };
    Command::new(&exe).args([INSTALLED_ARG, arg]).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

/// Modal Yes / Cancel question; also works before `window::run`.
fn confirm(text: &str) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    // `activate` needs macOS 14; the older call works everywhere.
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);

    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(crate::APP_NAME));
    alert.setInformativeText(&NSString::from_str(text));
    alert.addButtonWithTitle(&NSString::from_str(tr(T::Uninstall)));
    alert.addButtonWithTitle(&NSString::from_str(tr(T::Cancel)));
    alert.runModal() == NSAlertFirstButtonReturn
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

    // A running binary may be deleted on macOS; the process keeps its image.
    if let Some(dir) = install_dir().filter(|d| d.is_dir()) {
        let _ = std::fs::remove_dir_all(dir);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_enclosing_bundle() {
        let exe = Path::new("/Users/a/Downloads/Whisker.app/Contents/MacOS/Whisker");
        assert_eq!(bundle_of(exe).as_deref(), Some(Path::new("/Users/a/Downloads/Whisker.app")));
        assert_eq!(bundle_of(Path::new("/Users/a/src/target/release/whisker")), None);
    }
}
