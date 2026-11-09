//! Tray icon through AppIndicator (StatusNotifierItem on D-Bus), showing
//! the battery glyph of `crate::tray`. The library is loaded at run time:
//! without it (or without a tray host) the app runs window-only.
//!
//! ```text
//!   libayatana-appindicator3.so.1 (or libappindicator3.so.1)
//!     icon  = $XDG_RUNTIME_DIR/whisker-tray/whisker-<level>.png (theme path + name)
//!     menu  = Open / About / — / Quit (any click opens it; middle click = Open)
//!     title = tooltip text (hosts show it where they have no tooltip)
//! ```

use super::sys::runtime_dir;
use crate::i18n::{T, tr};
use crate::tray::{ICON_SIZE, Level, TrayAction, glyph};
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::PathBuf;

const LIBRARIES: [&CStr; 2] = [c"libayatana-appindicator3.so.1", c"libappindicator3.so.1"];
const INDICATOR_ID: &CStr = c"whisker";
const ICON_DIR: &str = "whisker-tray";
/// `APP_INDICATOR_CATEGORY_APPLICATION_STATUS`.
const CATEGORY_STATUS: c_int = 0;
/// `AppIndicatorStatus`: hidden / shown.
const STATUS_PASSIVE: c_int = 0;
const STATUS_ACTIVE: c_int = 1;
const BYTES_PER_PIXEL: usize = 4;

type NewFn = unsafe extern "C" fn(*const c_char, *const c_char, c_int) -> *mut c_void;
type SetIntFn = unsafe extern "C" fn(*mut c_void, c_int);
type SetPtrFn = unsafe extern "C" fn(*mut c_void, *mut c_void);
type SetStrFn = unsafe extern "C" fn(*mut c_void, *const c_char);
type SetIconFn = unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char);

/// The few `app_indicator_*` calls used, resolved from the library.
struct Api {
    new: NewFn,
    set_status: SetIntFn,
    set_menu: SetPtrFn,
    set_secondary: SetPtrFn,
    set_icon_full: SetIconFn,
    set_theme_path: SetStrFn,
    set_title: SetStrFn,
}

struct Item {
    api: Api,
    /// `AppIndicator*`, a GObject kept for the whole process.
    indicator: *mut c_void,
    _menu: gtk::Menu,
    on: fn(TrayAction),
    /// Icon name shown now; a new level gets a new name (hosts cache by name).
    shown: Option<String>,
}

thread_local! {
    static ITEM: RefCell<Option<Item>> = const { RefCell::new(None) };
}

/// Resolves `name` in `lib` as a function pointer of type `F`.
///
/// SAFETY: `F` must be the C signature of the symbol.
unsafe fn symbol<F: Copy>(lib: *mut c_void, name: &CStr) -> Option<F> {
    // SAFETY: `lib` is a live handle; the name is NUL-terminated.
    let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
    if p.is_null() {
        return None;
    }
    // SAFETY: caller guarantees `F` is a fn pointer of the symbol's type.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) })
}

fn load() -> Option<Api> {
    // SAFETY: plain dlopen of a system library; never closed.
    let lib = LIBRARIES.iter().map(|n| unsafe { libc::dlopen(n.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) }).find(|h| !h.is_null())?;
    // SAFETY: the types match the AppIndicator 0.1 C API.
    unsafe {
        Some(Api {
            new: symbol(lib, c"app_indicator_new")?,
            set_status: symbol(lib, c"app_indicator_set_status")?,
            set_menu: symbol(lib, c"app_indicator_set_menu")?,
            set_secondary: symbol(lib, c"app_indicator_set_secondary_activate_target")?,
            set_icon_full: symbol(lib, c"app_indicator_set_icon_full")?,
            set_theme_path: symbol(lib, c"app_indicator_set_icon_theme_path")?,
            set_title: symbol(lib, c"app_indicator_set_title")?,
        })
    }
}

/// Calls the hook outside the borrow, so it may update the item.
fn fire(a: TrayAction) {
    let Some(on) = ITEM.with(|i| i.borrow().as_ref().map(|i| i.on)) else {
        return;
    };
    on(a);
}

fn menu_item(menu: &gtk::Menu, title: &str, action: fn() -> TrayAction) -> gtk::MenuItem {
    let item = gtk::MenuItem::with_label(title);
    item.connect_activate(move |_| fire(action()));
    menu.append(&item);
    item
}

fn icon_dir() -> Option<PathBuf> {
    let dir = runtime_dir()?.join(ICON_DIR);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Adds the item if AppIndicator is available; `on` receives clicks.
pub fn add(on: fn(TrayAction)) {
    let Some(api) = load() else {
        return;
    };
    let Some(dir) = icon_dir().and_then(|d| CString::new(d.to_string_lossy().into_owned()).ok()) else {
        return;
    };

    let menu = gtk::Menu::new();
    let open = menu_item(&menu, tr(T::TrayOpen), || TrayAction::Open);
    menu_item(&menu, tr(T::About), || TrayAction::About);
    menu.append(&gtk::SeparatorMenuItem::new());
    menu_item(&menu, tr(T::TrayQuit), || TrayAction::Quit);
    menu.show_all();

    // SAFETY: valid strings; the indicator and menu live for the process.
    let indicator = unsafe { (api.new)(INDICATOR_ID.as_ptr(), c"".as_ptr(), CATEGORY_STATUS) };
    if indicator.is_null() {
        return;
    }
    let menu_ptr: *mut gtk::ffi::GtkMenu = menu.to_glib_none().0;
    let open_ptr: *mut gtk::ffi::GtkMenuItem = open.to_glib_none().0;
    // SAFETY: GTK objects owned by `Item` (the menu holds its items).
    unsafe {
        (api.set_theme_path)(indicator, dir.as_ptr());
        (api.set_menu)(indicator, menu_ptr.cast());
        (api.set_secondary)(indicator, open_ptr.cast());
        (api.set_status)(indicator, STATUS_ACTIVE);
    }

    ITEM.with(|i| *i.borrow_mut() = Some(Item { api, indicator, _menu: menu, on, shown: None }));
    update(Level::Offline, crate::APP_NAME);
}

/// `whisker-off`, `whisker-57`, `whisker-57c` (charging).
fn icon_name(level: Level) -> String {
    match level {
        Level::Offline => "whisker-off".into(),
        Level::Battery { pct, charging: true } => format!("whisker-{pct}c"),
        Level::Battery { pct, charging: false } => format!("whisker-{pct}"),
    }
}

/// Writes the glyph as `<name>.png` into the icon folder (once per name).
fn write_icon(level: Level, name: &str) -> Option<()> {
    let path = icon_dir()?.join(format!("{name}.png"));
    if path.is_file() {
        return Some(());
    }
    // 0xAARRGGBB in native order is Cairo's ARGB32 (alpha is 0 or 255,
    // so premultiplied is the same).
    let bytes: Vec<u8> = glyph(level).iter().flat_map(|p| p.to_ne_bytes()).collect();
    let side = ICON_SIZE as i32;
    let stride = (ICON_SIZE * BYTES_PER_PIXEL) as i32;
    let surface = gtk::cairo::ImageSurface::create_for_data(bytes, gtk::cairo::Format::ARgb32, side, side, stride).ok()?;
    let mut file = std::fs::File::create(&path).ok()?;
    surface.write_to_png(&mut file).ok()
}

pub fn update(level: Level, tip: &str) {
    let name = icon_name(level);
    let fresh = ITEM.with(|i| i.borrow().as_ref().is_some_and(|i| i.shown.as_deref() != Some(&name)));
    if fresh && write_icon(level, &name).is_none() {
        return;
    }
    let (Ok(c_name), Ok(c_tip)) = (CString::new(name.clone()), CString::new(tip.replace('\0', ""))) else {
        return;
    };
    ITEM.with(|i| {
        let mut i = i.borrow_mut();
        let Some(item) = i.as_mut() else {
            return;
        };
        // SAFETY: live indicator; strings outlive the calls (copied inside).
        unsafe {
            if fresh {
                (item.api.set_icon_full)(item.indicator, c_name.as_ptr(), c_tip.as_ptr());
            }
            (item.api.set_title)(item.indicator, c_tip.as_ptr());
        }
        item.shown = Some(name);
    });
}

/// Hides the item (the process ends right after).
pub fn remove() {
    ITEM.with(|i| {
        let Some(item) = i.borrow().as_ref().map(|i| (i.api.set_status, i.indicator)) else {
            return;
        };
        // SAFETY: live indicator.
        unsafe { (item.0)(item.1, STATUS_PASSIVE) };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_names_differ_per_level() {
        assert_eq!(icon_name(Level::Offline), "whisker-off");
        assert_eq!(icon_name(Level::Battery { pct: 57, charging: true }), "whisker-57c");
        assert_ne!(icon_name(Level::Battery { pct: 57, charging: false }), icon_name(Level::Battery { pct: 58, charging: false }));
    }
}
