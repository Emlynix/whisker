//! TODO: macOS backend. Implement the contract in `platform/mod.rs`
//! with the same names and signatures as `platform/windows`, then
//! re-export it from `platform/mod.rs` like the Windows one:
//!
//! - root: `config_dir` (`~/Library/Application Support`), `dark_mode`
//!   (`AppleInterfaceStyle`), `user_locale` (`AppleLanguages`), `today`.
//! - `window`: create, run, post, invalidate, show, quit, set_timer,
//!   kill_timer, set_dark_title, modifiers, set_tray (NSStatusItem).
//! - `gfx::Gfx`: clear, fill, stroke, circle, ring, line, polyline, text
//!   (Core Graphics / Core Text).
//! - `hid`: interfaces, mice, `Channel` over IOHIDManager / IOHIDDevice.
//! - `bluetooth`: battery, name. `input`: start, has_vendor
//!   (IOHIDManager input callbacks). `autostart` (login item),
//!   `install`, `single`.
