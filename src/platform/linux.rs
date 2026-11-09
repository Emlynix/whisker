//! TODO: Linux backend. Implement the contract in `platform/mod.rs`
//! with the same names and signatures as `platform/windows`, then
//! re-export it from `platform/mod.rs` like the Windows one:
//!
//! - root: `config_dir` (`$XDG_CONFIG_HOME` or `~/.config`), `dark_mode`
//!   (freedesktop portal `color-scheme`), `user_locale` (`LC_ALL`,
//!   `LC_MESSAGES`, `LANG`), `today` (local date).
//! - `window`: create, run, post, invalidate, show, quit, set_timer,
//!   kill_timer, set_title_colors, modifiers, set_tray (StatusNotifierItem),
//!   notify (org.freedesktop.Notifications).
//! - `gfx::Gfx`: clear, fill, stroke, circle, ring, line, polyline, text.
//! - `hid`: interfaces, mice, layout (sysfs `report_descriptor`,
//!   `HIDIOCGRAWINFO` bus type), `Channel` over `/dev/hidraw*` (read, write,
//!   read_until, get_feature, set_feature via `HIDIOC*` ioctls).
//! - `bluetooth`: battery, name (BlueZ). `input`: start, has_vendor
//!   (evdev). `autostart` (XDG autostart .desktop), `install`, `single`.
