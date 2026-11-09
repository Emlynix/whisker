//! Windows backend: Win32 shell, Direct2D/DirectWrite drawing,
//! SetupDi/HID device access, registry and shell integration.

pub mod autostart;
pub mod bluetooth;
pub mod gfx;
pub mod hid;
pub mod input;
pub mod install;
pub mod net;
pub mod sign;
pub mod single;
mod sys;
mod tray;
mod win;
pub mod window;

pub use sys::{config_dir, dark_mode, open_url, os_version, today, user_locale};

pub const UPDATE_KIND: crate::platform::UpdateKind = crate::platform::UpdateKind::InPlace;
