//! Session D-Bus services over GIO: the desktop portal's colour scheme
//! and its change signal, desktop notifications and their clicks.
//! A missing service (e.g. no portal, no notification daemon) only means
//! "no answer"; nothing here fails loudly.
//!
//! ```text
//!   org.freedesktop.portal.Desktop   Settings.ReadOne / SettingChanged
//!   org.freedesktop.Notifications    Notify -> id, ActionInvoked(id, key)
//! ```

use gtk::gio::{self, BusType, DBusCallFlags, DBusConnection, DBusSignalFlags};
use gtk::glib::{Variant, VariantTy};
use std::cell::RefCell;
use std::collections::HashMap;

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_SETTINGS: &str = "org.freedesktop.portal.Settings";
const APPEARANCE: &str = "org.freedesktop.appearance";
const COLOR_SCHEME: &str = "color-scheme";
/// `color-scheme` values: 0 = no preference, 1 = dark, 2 = light.
const SCHEME_DARK: u32 = 1;
const SCHEME_LIGHT: u32 = 2;

const NOTIFY_NAME: &str = "org.freedesktop.Notifications";
const NOTIFY_PATH: &str = "/org/freedesktop/Notifications";
/// Action invoked by a click on the notification body.
const DEFAULT_ACTION: &str = "default";
/// Server picks how long the notification stays.
const EXPIRE_DEFAULT: i32 = -1;
/// Ids of our notifications kept for click matching.
const SENT_MAX: usize = 16;
/// Calls to the session bus wait at most this long (ms).
const CALL_TIMEOUT_MS: i32 = 500;

thread_local! {
    /// Notification ids we sent, newest last.
    static SENT: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

fn session() -> Option<DBusConnection> {
    gio::bus_get_sync(BusType::Session, None::<&gio::Cancellable>).ok()
}

/// Unwraps nested `v` variants down to the value.
fn unbox(mut v: Variant) -> Variant {
    while let Some(inner) = v.as_variant() {
        v = inner;
    }
    v
}

/// Portal colour scheme: `Some(true)` dark, `Some(false)` light, `None`
/// when there is no portal or no preference.
pub fn portal_dark() -> Option<bool> {
    let bus = session()?;
    let args = (APPEARANCE, COLOR_SCHEME).into();
    let call = |method: &str| {
        bus.call_sync(Some(PORTAL_NAME), PORTAL_PATH, PORTAL_SETTINGS, method, Some(&args), None, DBusCallFlags::NONE, CALL_TIMEOUT_MS, None::<&gio::Cancellable>)
    };
    // `ReadOne` (portal v2) answers `(v)`; older portals only have `Read`.
    let reply = call("ReadOne").or_else(|_| call("Read")).ok()?;
    match unbox(reply.child_value(0)).get::<u32>()? {
        SCHEME_DARK => Some(true),
        SCHEME_LIGHT => Some(false),
        _ => None,
    }
}

/// Calls `on_change` when the portal's colour scheme changes.
pub fn watch_scheme(on_change: fn()) {
    let Some(bus) = session() else {
        return;
    };
    let on_signal = move |_: &DBusConnection, _: &str, _: &str, _: &str, _: &str, args: &Variant| {
        let namespace = args.try_child_value(0).and_then(|v| v.get::<String>());
        if namespace.as_deref() == Some(APPEARANCE) {
            on_change();
        }
    };
    // The subscription stays for the whole process.
    let _ = bus.signal_subscribe(Some(PORTAL_NAME), Some(PORTAL_SETTINGS), Some("SettingChanged"), Some(PORTAL_PATH), None, DBusSignalFlags::NONE, on_signal);
}

/// Calls `on_open` when the user clicks one of our notifications.
pub fn watch_clicks(on_open: fn()) {
    let Some(bus) = session() else {
        return;
    };
    let on_signal = move |_: &DBusConnection, _: &str, _: &str, _: &str, _: &str, args: &Variant| {
        let Some((id, _key)) = args.get::<(u32, String)>() else {
            return;
        };
        if SENT.with(|s| s.borrow().contains(&id)) {
            on_open();
        }
    };
    let _ = bus.signal_subscribe(Some(NOTIFY_NAME), Some(NOTIFY_NAME), Some("ActionInvoked"), Some(NOTIFY_PATH), None, DBusSignalFlags::NONE, on_signal);
}

/// Shows a desktop notification; `open` labels the click action. Async:
/// the UI never waits for the notification daemon.
pub fn notify(title: &str, text: &str, open: &str) {
    let Some(bus) = session() else {
        return;
    };
    let hints: HashMap<String, Variant> = HashMap::new();
    let actions = vec![DEFAULT_ACTION.to_string(), open.to_string()];
    // Servers read the body as markup: "<b>" or "&" must stay plain text.
    let body = gtk::glib::markup_escape_text(text);
    let args = (crate::APP_NAME, 0u32, "", title, body.as_str(), actions, hints, EXPIRE_DEFAULT).into();
    let reply = VariantTy::new("(u)").ok();
    bus.call(Some(NOTIFY_NAME), NOTIFY_PATH, NOTIFY_NAME, "Notify", Some(&args), reply, DBusCallFlags::NONE, CALL_TIMEOUT_MS, None::<&gio::Cancellable>, |r| {
        let Some((id,)) = r.ok().and_then(|v| v.get::<(u32,)>()) else {
            return;
        };
        SENT.with(|s| {
            let mut s = s.borrow_mut();
            if s.len() >= SENT_MAX {
                s.remove(0);
            }
            s.push(id);
        });
    });
}
