//! Linux backend: GTK 3 window, Cairo shapes and Pango text, hidraw HID
//! access, evdev statistics, D-Bus (GIO) for theme, notifications and
//! BlueZ, XDG autostart and a per-user install under `~/.local`.
//!
//! ```text
//!   window ── GtkWindow + DrawingArea ── gfx (Cairo + PangoCairo)
//!     ├─ tray (AppIndicator, loaded at run time)   keymap: evdev -> VK
//!     └─ bus (session D-Bus: portal theme, notifications)
//!   hid ── descriptor (report descriptor parser) ── sysfs
//!   input (evdev) · bluetooth (BlueZ, system D-Bus)
//!   autostart / install / single ── sys (folders, locale, theme, date)
//! ```

pub mod autostart;
pub mod bluetooth;
mod bus;
mod descriptor;
pub mod gfx;
pub mod hid;
pub mod input;
pub mod install;
mod keymap;
pub mod net;
pub mod sign;
pub mod single;
mod sys;
mod tray;
pub mod window;

pub use sys::{config_dir, dark_mode, open_url, os_version, today, user_locale};

pub const UPDATE_KIND: crate::platform::UpdateKind = crate::platform::UpdateKind::ReleasePage;
