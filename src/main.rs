//! Whisker: lightweight control app for gaming mice (first: White Shark Lionel).
//! Sets DPI and polling rate, shows battery and usage statistics, lives
//! in the notification area.

#![windows_subsystem = "windows"]

/// Product name: window title, install folder, registry keys.
pub const APP_NAME: &str = "Whisker";

mod app;
mod devices;
mod drivers;
mod feedback;
mod gfx;
mod hid_link;
mod i18n;
mod keys;
mod log;
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

    let last = log::start(store::data_dir(), &log_header());
    crash_notes();

    if has(platform::AUTOSTART_ON_ARG) {
        autostart::set(Autostart::On);
    }
    if has(platform::AUTOSTART_OFF_ARG) {
        autostart::set(Autostart::Off);
    }

    let mode = if has(platform::TRAY_ARG) { app::StartMode::Hidden } else { app::StartMode::Visible };
    app::run(mode, last);
    log::exit();
}

/// First line of a run in the log, e.g.
/// `2026-11-10 Whisker 0.9.0, Windows 11 26200, installed`.
fn log_header() -> String {
    let (y, m, d) = platform::today();
    let copy = if install::is_installed_copy() { "installed" } else { "portable" };
    format!("{y}-{m:02}-{d:02} {APP_NAME} {}, {}, {copy}", env!("CARGO_PKG_VERSION"), platform::os_version())
}

/// `panic at app.rs:120: index out of bounds`. The file's name only: a
/// dependency's full path holds the builder's user name.
fn panic_note(info: &std::panic::PanicHookInfo) -> String {
    const MESSAGE_MAX: usize = 200;
    let place = info.location().map_or(String::new(), |l| {
        let file = l.file().rsplit(['/', '\\']).next().unwrap_or_default();
        format!(" at {file}:{}", l.line())
    });
    let message: String = info.payload_as_str().unwrap_or("").chars().take(MESSAGE_MAX).collect();
    // A path in the message would name the user's home folder.
    let message = if message.contains(['/', '\\']) { "(message with a path left out)" } else { message.as_str() };
    format!("panic{place}: {message}")
}

/// Panics and crashes of the process go into the log.
fn crash_notes() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::crash(&panic_note(info));
        default(info);
    }));
    platform::crash::install();
}
