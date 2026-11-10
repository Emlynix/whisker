//! Whisker: lightweight control app for gaming mice (first: White Shark Lionel).
//! Sets DPI and polling rate, shows battery and usage statistics, lives
//! in the notification area.

#![windows_subsystem = "windows"]

/// Product name: window title, install folder, registry keys.
pub const APP_NAME: &str = "Whisker";

mod app;
mod devices;
mod drivers;
mod gfx;
mod hid_link;
mod i18n;
mod keys;
mod mouse;
mod platform;
mod protocol;
mod shell;
mod stats;
mod store;
mod theme;
mod tray;
mod update;
mod view;

use platform::{Autostart, Claim, Instance, autostart, install, single};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |a: &str| args.iter().any(|x| x == a);

    store::migrate_old_dir();
    i18n::set_lang(store::load_config().lang.unwrap_or_else(i18n::system_lang));

    if has(platform::UNINSTALL_ARG) {
        install::uninstall(platform::Origin::External);
        return;
    }

    // Fresh install or update: the previous process is still exiting.
    let handover = has(platform::INSTALLED_ARG) || has(platform::UPDATED_ARG);
    let claim = if handover { Claim::WaitForExit } else { Claim::Now };
    // Demo mode runs next to the installed app.
    if !platform::demo()
        && let Instance::AlreadyRunning = single::claim(claim)
    {
        return;
    }
    update::cleanup();

    if has(platform::AUTOSTART_ON_ARG) {
        autostart::set(Autostart::On);
    }
    if has(platform::AUTOSTART_OFF_ARG) {
        autostart::set(Autostart::Off);
    }

    let mode = if has(platform::TRAY_ARG) { app::StartMode::Hidden } else { app::StartMode::Visible };
    app::run(mode)
}
