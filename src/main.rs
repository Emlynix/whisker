//! Whisker: lightweight control app for gaming mice (first: White Shark Lionel).
//! Sets DPI and polling rate, shows battery and usage statistics, lives
//! in the notification area.

#![windows_subsystem = "windows"]

/// Product name: window title, install folder, registry keys.
pub const APP_NAME: &str = "Whisker";

mod app;
mod autostart;
mod devices;
mod drivers;
mod gfx;
mod hid_link;
mod i18n;
mod keys;
mod install;
mod mouse;
mod protocol;
mod single;
mod stats;
mod store;
mod theme;
mod tray;
mod view;
mod win;
mod window;

use autostart::Autostart;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |a: &str| args.iter().any(|x| x == a);

    store::migrate_old_dir();
    i18n::set_lang(store::load_config().lang.unwrap_or_else(i18n::system_lang));

    if has(install::UNINSTALL_ARG) {
        install::uninstall(install::Origin::External);
        return;
    }

    // Fresh install: the installer process is still exiting.
    let claim = if has(install::INSTALLED_ARG) { single::Claim::WaitForExit } else { single::Claim::Now };
    if let single::Instance::AlreadyRunning = single::claim(claim) {
        return;
    }

    if has(install::AUTOSTART_ON_ARG) {
        autostart::set(Autostart::On);
    }
    if has(install::AUTOSTART_OFF_ARG) {
        autostart::set(Autostart::Off);
    }

    let mode = if has(autostart::TRAY_ARG) { app::StartMode::Hidden } else { app::StartMode::Visible };
    app::run(mode)
}
