//! System queries: user folders (XDG), UI language, light/dark setting,
//! clock, OS name, opening web links.

use super::bus;
use gtk::prelude::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Locale variables in the order gettext reads them.
const LOCALE_VARS: [&str; 3] = ["LC_ALL", "LC_MESSAGES", "LANG"];
/// Locales that mean "no language chosen".
const NEUTRAL_LOCALES: [&str; 2] = ["C", "POSIX"];
const OS_RELEASE: &str = "/etc/os-release";
const KERNEL_RELEASE: &str = "/proc/sys/kernel/osrelease";
/// Theme names of dark GTK themes end like this, e.g. `Adwaita-dark`.
const DARK_SUFFIX: &str = "dark";
/// Desktop opener used when GIO finds no handler.
const XDG_OPEN: &str = "xdg-open";

fn absolute(s: String) -> Option<PathBuf> {
    let path = PathBuf::from(s);
    path.is_absolute().then_some(path)
}

fn env_path(var: &str) -> Option<PathBuf> {
    absolute(std::env::var(var).ok()?)
}

/// `$HOME`, else the home folder of the user's passwd entry.
pub fn home_dir() -> Option<PathBuf> {
    env_path("HOME").or_else(|| absolute(gtk::glib::home_dir().to_string_lossy().into_owned()))
}

/// `$XDG_CONFIG_HOME` or `~/.config`; `store` adds the app folder.
pub fn config_dir() -> Option<PathBuf> {
    env_path("XDG_CONFIG_HOME").or_else(|| Some(home_dir()?.join(".config")))
}

/// `$XDG_DATA_HOME` or `~/.local/share`.
pub fn data_dir() -> Option<PathBuf> {
    env_path("XDG_DATA_HOME").or_else(|| Some(home_dir()?.join(".local").join("share")))
}

/// Per-user folder for the instance lock and socket: `$XDG_RUNTIME_DIR`
/// (private, cleared at logout), else `~/.cache`.
pub fn runtime_dir() -> Option<PathBuf> {
    env_path("XDG_RUNTIME_DIR").or_else(|| Some(home_dir()?.join(".cache")))
}

/// First of `LANGUAGE`, `LC_ALL`, `LC_MESSAGES`, `LANG`, e.g. `"sk_SK.UTF-8"`.
pub fn user_locale() -> Option<String> {
    let first_language = std::env::var("LANGUAGE").ok().and_then(|l| l.split(':').next().map(str::to_string));
    let vars = LOCALE_VARS.iter().filter_map(|v| std::env::var(v).ok());
    first_language.into_iter().chain(vars).find(|l| is_language(l))
}

/// A real language, not empty, `C` or `POSIX` (also `C.UTF-8`).
fn is_language(locale: &str) -> bool {
    let base = locale.split(['.', '@']).next().unwrap_or_default();
    !base.is_empty() && !NEUTRAL_LOCALES.contains(&base)
}

/// The desktop prefers dark: the freedesktop portal's `color-scheme`, else
/// a dark GTK theme name (`GTK_THEME` or the theme setting).
pub fn dark_mode() -> bool {
    if let Some(dark) = bus::portal_dark() {
        return dark;
    }
    let env_theme = std::env::var("GTK_THEME").ok();
    // GTK settings exist only after `gtk::init` on the UI thread.
    let set_theme = || {
        if !gtk::is_initialized_main_thread() {
            return None;
        }
        gtk::Settings::default().and_then(|s| s.gtk_theme_name()).map(|n| n.to_string())
    };
    env_theme.or_else(set_theme).is_some_and(|t| is_dark_theme(&t))
}

/// `Adwaita-dark`, `Yaru-dark`, `Adwaita:dark` (GTK_THEME variant).
fn is_dark_theme(name: &str) -> bool {
    name.to_lowercase().trim_end().ends_with(DARK_SUFFIX)
}

/// Opens an `http(s)` link in the default browser; other schemes are refused
/// so a crafted string can never open a local file or app.
pub fn open_url(url: &str) -> bool {
    if !crate::platform::is_web_url(url) {
        return false;
    }
    if gtk::gio::AppInfo::launch_default_for_uri(url, None::<&gtk::gio::AppLaunchContext>).is_ok() {
        return true;
    }
    // Fixed program, the URL as a single argument; no shell.
    Command::new(XDG_OPEN).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
}

/// X11 property with the process id of a window's program.
const WM_PID: &str = "_NET_WM_PID";
const CARDINAL: &str = "CARDINAL";
/// Bits of one item of a CARDINAL property.
const PID_FORMAT: i32 = 32;

/// File name of the program whose window is in focus, e.g. "firefox".
/// Asked through GDK, which knows the active window under X11 only;
/// `None` under Wayland, or when the window names no process.
pub fn foreground_app() -> Option<String> {
    use gtk::glib::translate::{ToGlibPtr, from_glib_full};
    if !gtk::is_initialized() {
        return None;
    }
    let screen = gtk::gdk::Screen::default()?;
    // SAFETY: a live screen; the call returns a new reference or NULL.
    let window: Option<gtk::gdk::Window> = unsafe { from_glib_full(gtk::gdk::ffi::gdk_screen_get_active_window(screen.to_glib_none().0)) };
    let window = window?;
    // A format-32 property comes as C longs.
    let long = size_of::<std::ffi::c_long>();
    let (_, format, data) = gtk::gdk::property_get(&window, &gtk::gdk::Atom::intern(WM_PID), &gtk::gdk::Atom::intern(CARDINAL), 0, long as u64, 0)?;
    let pid = std::ffi::c_long::from_ne_bytes(data.get(..long)?.try_into().ok()?);
    // Any program may set the property: only a plain process id is used.
    if format != PID_FORMAT || pid <= 0 {
        return None;
    }
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(exe.file_name()?.to_string_lossy().into_owned())
}

/// Only an X11 session tells which window is in focus.
pub fn foreground_known() -> bool {
    // GDK may be asked only once GTK runs (it does not in unit tests).
    gtk::is_initialized() && gtk::gdk::Display::default().is_some_and(|d| d.type_().name().contains("X11"))
}

/// Distribution and kernel for bug reports, e.g. `"Ubuntu 26.04 LTS (Linux 6.6.87)"`.
pub fn os_version() -> String {
    let distro = std::fs::read_to_string(OS_RELEASE).ok().and_then(|t| pretty_name(&t)).unwrap_or_else(|| "Linux".into());
    let kernel = std::fs::read_to_string(KERNEL_RELEASE).unwrap_or_default();
    let kernel = kernel.trim();
    if kernel.is_empty() {
        return distro;
    }
    format!("{distro} (Linux {kernel})")
}

/// `PRETTY_NAME="Ubuntu 26.04 LTS"` from os-release.
fn pretty_name(os_release: &str) -> Option<String> {
    let line = os_release.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))?;
    let name = line.trim().trim_matches(['"', '\'']).trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Local date as `(year, month, day)`.
pub fn today() -> (i64, i64, i64) {
    const TM_YEAR_BASE: i64 = 1900;
    // SAFETY: `time(NULL)` only returns the clock; `localtime_r` writes
    // into our zeroed `tm` and is thread-safe.
    let tm = unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    (tm.tm_year as i64 + TM_YEAR_BASE, tm.tm_mon as i64 + 1, tm.tm_mday as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_locales_are_skipped() {
        assert!(is_language("sk_SK.UTF-8"));
        assert!(is_language("de"));
        assert!(!is_language("C.UTF-8"));
        assert!(!is_language("POSIX"));
        assert!(!is_language(""));
    }

    #[test]
    fn dark_theme_names() {
        assert!(is_dark_theme("Adwaita-dark"));
        assert!(is_dark_theme("Adwaita:dark"));
        assert!(!is_dark_theme("Yaru"));
    }

    #[test]
    fn os_release_pretty_name() {
        let t = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 26.04 LTS\"\nID=ubuntu\n";
        assert_eq!(pretty_name(t).as_deref(), Some("Ubuntu 26.04 LTS"));
        assert_eq!(pretty_name("ID=arch\n"), None);
    }

    #[test]
    fn today_is_a_date() {
        let (y, m, d) = today();
        assert!(y >= 2024 && (1..=12).contains(&m) && (1..=31).contains(&d));
    }
}
