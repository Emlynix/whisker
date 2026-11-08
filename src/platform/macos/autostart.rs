//! Start at login via a per-user LaunchAgent. The launch line adds
//! `--tray` so the app starts hidden in the menu bar. Always points at the
//! installed copy: a bundle in e.g. Downloads never runs at login.
//!
//! ```text
//!   ~/Library/LaunchAgents/com.whisker.app.plist
//!     ProgramArguments = [~/Applications/Whisker.app/Contents/MacOS/Whisker, --tray]
//!     RunAtLoad = true
//! ```
//! launchd reads it at the next login; nothing is loaded right away.

use super::sys::home_dir;
use crate::platform::{Autostart, TRAY_ARG};
use std::path::PathBuf;

/// Same as `CFBundleIdentifier` in `bundle/macos/Info.plist`.
const LABEL: &str = "com.whisker.app";

fn agent_path() -> Option<PathBuf> {
    Some(home_dir()?.join("Library").join("LaunchAgents").join(format!("{LABEL}.plist")))
}

/// Escapes text for an XML element.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The plist we write, only if the installed binary exists.
fn agent_plist() -> Option<String> {
    let exe = super::install::installed_exe().filter(|p| p.is_file())?;
    let exe = xml(&exe.to_string_lossy());
    Some(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
		<string>{TRAY_ARG}</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>ProcessType</key>
	<string>Interactive</string>
</dict>
</plist>
"#
    ))
}

/// True only when the agent points at our installed copy.
pub fn is_enabled() -> bool {
    let (Some(path), Some(ours)) = (agent_path(), agent_plist()) else {
        return false;
    };
    std::fs::read_to_string(path).is_ok_and(|stored| stored == ours)
}

/// `On` fails when the app is not installed.
pub fn set(mode: Autostart) -> bool {
    let Some(path) = agent_path() else {
        return false;
    };

    if mode == Autostart::Off {
        return match std::fs::remove_file(&path) {
            Ok(()) => true,
            Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        };
    }

    let Some(plist) = agent_plist() else {
        return false;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&path, plist).is_ok()
}
