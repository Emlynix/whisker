//! macOS backend: AppKit window and menu-bar item, Core Graphics drawing,
//! IOKit HID access, LaunchAgent autostart, `~/Applications` install.
//!
//! ```text
//!   window ── View (NSView) ── gfx (CGContext + string drawing)
//!     └─ tray (NSStatusItem)            keymap: macOS key codes -> VK
//!   hid / input / bluetooth ── iokit (registry properties)
//!   autostart / install / single ── sys (folders, locale, theme, date)
//! ```

pub mod autostart;
pub mod bluetooth;
pub mod gfx;
pub mod hid;
pub mod input;
pub mod install;
mod iokit;
mod keymap;
pub mod single;
mod sys;
mod tray;
pub mod window;

pub use sys::{config_dir, dark_mode, open_url, os_version, today, user_locale};
