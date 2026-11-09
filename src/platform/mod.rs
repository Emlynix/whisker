//! Platform layer: everything that talks to the OS. The core (app, view,
//! store, protocol, devices, drivers, mouse, keys, i18n) uses only the
//! API below; each backend implements it with the same plain structs and
//! functions, selected by `cfg`.
//!
//! ```text
//!   app / view / mouse / drivers / stats / store          portable core
//!        |  gfx::Gfx, hid::Channel, window::post, …
//!   platform  (this module: contract types + cfg switch)
//!        |
//!   windows (Win32, D2D)  |  linux (TODO)  |  macos (AppKit, IOKit)
//! ```
//!
//! # Contract
//!
//! Root functions:
//! - `config_dir() -> Option<PathBuf>`: per-user settings base folder
//!   (`%APPDATA%`, `~/.config`, `~/Library/Application Support`).
//! - `dark_mode() -> bool`: the OS asks apps for a dark theme.
//! - `user_locale() -> Option<String>`: UI language as a locale name,
//!   e.g. `"sk-SK"`; `i18n` maps it to a `Lang`.
//! - `today() -> (i64, i64, i64)`: local date as (year, month, day).
//! - `open_url(url) -> bool`: opens an `http(s)` link in the default
//!   browser; any other scheme is refused (`is_web_url`).
//!
//! `window` — one app window on the UI thread, plus the tray icon:
//! - `create(title, w, h, Hooks) -> bool`: hidden window of `w` x `h` DIPs
//!   and tray icon; input goes through `shell::Pointer` into `Hooks`.
//! - `run()`: event loop until `quit`.
//! - `post(f)`: run `f` on the UI thread; callable from any thread (wake).
//! - `invalidate()`, `show()`, `quit()`: repaint, raise, close for good.
//! - `set_timer(id, ms)`, `kill_timer(id)`: repeating timer to `Hooks::timer`.
//! - `set_title_colors(TitleColors)`: title bar in the theme's colours.
//!   Windows 11 sets caption, text and border colours; Windows 10 only
//!   switches dark/light (`shade`). macOS draws a transparent title bar
//!   over the window background with the matching appearance. Linux
//!   (TODO): a client-side or `_GTK_THEME_VARIANT` dark/light hint.
//! - `modifiers() -> shell::Mods`: modifier keys held now.
//! - `set_clipboard(text) -> bool`: replaces the clipboard with plain text
//!   (the window owns it on Windows).
//! - `set_tray(tray::Level, tip)`: battery glyph (`tray::glyph`) and tooltip.
//!   Tray clicks arrive as `Hooks::tray` (left = open, menu = open/about/quit).
//!
//! `gfx::Gfx` — drawing in DIPs, given to `Hooks::draw` per frame:
//! `clear(c)`, `fill(r, radius, c)`, `stroke(r, radius, width, c)` (inside
//! `r`), `circle(cx, cy, radius, c)`, `ring(cx, cy, radius, width, c)`,
//! `line(x0, y0, x1, y1, width, c)`, `polyline(pts, width, c)`,
//! `text(s, r, size, weight, align, c)` (system UI font).
//!
//! `hid` — HID collections, descriptors read without sending anything:
//! - `interfaces(vid) -> Vec<hid_link::Interface>`: collections of a vendor.
//! - `mice() -> Vec<hid_link::Interface>`: generic desktop / mouse collections.
//! - `Channel` (`Send`): `open(path) -> Option<Channel>`,
//!   `write(report, timeout) -> bool` (output report, byte 0 = id),
//!   `read(len, timeout) -> Option<Vec<u8>>` (next input report),
//!   `read_until(len, timeout, accept) -> Option<T>`,
//!   `set_feature(report) -> bool`, `get_feature(buf) -> bool` (`buf[0]` = id).
//!
//! `bluetooth` — `battery(addr) -> Option<u8>` and `name(addr) ->
//! Option<String>` of a BLE device by its 12-hex-digit address.
//!
//! `input` — statistics hook: `start(fn(&stats::MouseInput))` calls back
//! on its own thread for every mouse report, also while unfocused;
//! `has_vendor(dev, vid) -> bool` for the device id of a report.
//!
//! `autostart` — start hidden at login (`TRAY_ARG`), installed copy only:
//! `is_enabled() -> bool`, `set(Autostart) -> bool`.
//!
//! `install` — per-user install without admin rights:
//! `install_dir() -> Option<PathBuf>`, `is_installed_copy() -> bool`,
//! `install(Autostart) -> Result<(), String>` (copies, registers, launches
//! the copy with `INSTALLED_ARG`), `uninstall(Origin) -> bool` (asks first).
//!
//! `single` — one running instance: `claim(Claim) -> Instance` (a second
//! launch makes the first show its window), `request_quit()`,
//! `wait_closed(limit)`, `listen(on_show, on_quit)`.

use crate::gfx::Color;

/// Start hidden in the tray (autostart launch line).
pub const TRAY_ARG: &str = "--tray";
pub const UNINSTALL_ARG: &str = "--uninstall";
/// Passed to the freshly installed copy so it waits for the old process to exit.
pub const INSTALLED_ARG: &str = "--installed";
pub const AUTOSTART_ON_ARG: &str = "--autostart-on";
pub const AUTOSTART_OFF_ARG: &str = "--autostart-off";

/// Only web links may be opened: `file:`, `ms-settings:` or bare paths
/// would let a string launch local programs.
fn is_web_url(url: &str) -> bool {
    const SCHEMES: [&str; 2] = ["https://", "http://"];
    SCHEMES.iter().any(|s| url.starts_with(s)) && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Title bar look for `window::set_title_colors`, from the active theme.
#[derive(Clone, Copy)]
pub struct TitleColors {
    pub bg: Color,
    // macOS draws title text and border itself.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub text: Color,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub border: Color,
    /// Fallback where custom colours are not supported.
    pub shade: Shade,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Shade {
    Dark,
    Light,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Autostart {
    On,
    Off,
}

pub enum Instance {
    First,
    AlreadyRunning,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Hand over to a running instance.
    Now,
    /// Previous process is exiting (after install); wait for it.
    WaitForExit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Started from the OS app list (`--uninstall`); another instance may run.
    External,
    /// Clicked inside the running app, which quits right after.
    InApp,
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::*;

#[cfg(test)]
mod tests {
    use super::is_web_url;

    #[test]
    fn only_web_links_open() {
        assert!(is_web_url("https://github.com/Emlynix/whisker"));
        assert!(!is_web_url("file:///C:/Windows/System32/calc.exe"));
        assert!(!is_web_url(r"C:\Windows\notepad.exe"));
        assert!(!is_web_url("https://x.y/a b"));
    }
}
