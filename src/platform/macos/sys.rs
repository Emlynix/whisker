//! System queries: user folders, UI language, light/dark setting, clock.

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSCalendar, NSCalendarUnit, NSDate, NSHomeDirectory, NSLocale, NSString, NSTemporaryDirectory, NSURL, NSUserDefaults};
use std::path::PathBuf;

/// Global default that holds `"Dark"` while the system uses dark mode.
const STYLE_KEY: &str = "AppleInterfaceStyle";
const STYLE_DARK: &str = "Dark";

/// Home folder from Foundation (not `$HOME`, which any parent process can
/// fake). Only absolute paths are returned.
pub fn home_dir() -> Option<PathBuf> {
    absolute(NSHomeDirectory().to_string())
}

/// Per-user temporary folder (`/var/folders/…/T`).
pub fn temp_dir() -> Option<PathBuf> {
    absolute(NSTemporaryDirectory().to_string())
}

fn absolute(s: String) -> Option<PathBuf> {
    let path = PathBuf::from(s);
    path.is_absolute().then_some(path)
}

/// `~/Library/Application Support`; `store` adds the app folder.
pub fn config_dir() -> Option<PathBuf> {
    Some(home_dir()?.join("Library").join("Application Support"))
}

/// First preferred language, e.g. `"sk-SK"`, `"zh-Hant-TW"`.
pub fn user_locale() -> Option<String> {
    let first = NSLocale::preferredLanguages().firstObject()?;
    Some(first.to_string()).filter(|s| !s.is_empty())
}

/// The system appearance is Dark (the key is absent in Light mode).
pub fn dark_mode() -> bool {
    let key = NSString::from_str(STYLE_KEY);
    let style = NSUserDefaults::standardUserDefaults().stringForKey(&key);
    style.is_some_and(|s| s.to_string() == STYLE_DARK)
}

/// Opens an `http(s)` link in the default browser; other schemes are refused
/// so a crafted string can never open a local file or app.
pub fn open_url(url: &str) -> bool {
    if !crate::platform::is_web_url(url) {
        return false;
    }
    let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) else {
        return false;
    };
    NSWorkspace::sharedWorkspace().openURL(&u)
}

/// Local date as `(year, month, day)`.
pub fn today() -> (i64, i64, i64) {
    let (cal, now) = (NSCalendar::currentCalendar(), NSDate::now());
    let part = |unit: NSCalendarUnit| cal.component_fromDate(unit, &now) as i64;
    (part(NSCalendarUnit::Year), part(NSCalendarUnit::Month), part(NSCalendarUnit::Day))
}
