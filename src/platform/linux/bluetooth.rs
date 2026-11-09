//! Battery and name of Bluetooth mice from BlueZ on the system D-Bus.
//! BlueZ publishes the level as `org.bluez.Battery1.Percentage` for
//! devices with the GATT Battery Service (most BLE mice):
//!
//! ```text
//!   /org/bluez/hci0/dev_B6_6B_18_D7_1E_00
//!     org.bluez.Device1   Address = "B6:6B:18:D7:1E:00", Alias = "…"
//!     org.bluez.Battery1  Percentage = 87
//! ```
//! Without BlueZ or that interface the answer is `None`.

use gtk::gio::{self, BusType, DBusCallFlags};
use gtk::glib::Variant;

const BLUEZ: &str = "org.bluez";
const OBJECT_MANAGER: &str = "org.freedesktop.DBus.ObjectManager";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const BATTERY_IFACE: &str = "org.bluez.Battery1";
const CALL_TIMEOUT_MS: i32 = 1000;
const MAX_PERCENT: u8 = 100;

/// `"B66B18D71E00"` as BlueZ writes it: `"B6:6B:18:D7:1E:00"`.
fn colon_address(address: &str) -> String {
    let hex = address.to_uppercase();
    let pairs: Vec<&str> = hex.as_bytes().chunks(2).filter_map(|p| std::str::from_utf8(p).ok()).collect();
    pairs.join(":")
}

/// Value under `key` in a string-keyed dictionary (`a{s*}`), unboxed
/// from its `v` wrapper.
fn lookup(dict: &Variant, key: &str) -> Option<Variant> {
    let entry = dict.iter().find(|e| e.try_child_value(0).and_then(|k| k.str().map(|s| s == key)).unwrap_or(false))?;
    let value = entry.try_child_value(1)?;
    Some(value.as_variant().unwrap_or(value))
}

/// Interfaces (`a{sa{sv}}`) of the BlueZ device with this address.
fn device(address: &str) -> Option<Variant> {
    let want = colon_address(address);
    let bus = gio::bus_get_sync(BusType::System, None::<&gio::Cancellable>).ok()?;
    let reply = bus.call_sync(Some(BLUEZ), "/", OBJECT_MANAGER, "GetManagedObjects", None, None, DBusCallFlags::NONE, CALL_TIMEOUT_MS, None::<&gio::Cancellable>).ok()?;
    // `(a{oa{sa{sv}}})`: object path -> interface -> properties.
    let objects = reply.try_child_value(0)?;
    objects.iter().map(|entry| entry.child_value(1)).find(|ifaces| {
        let addr = lookup(ifaces, DEVICE_IFACE).and_then(|d| lookup(&d, "Address")).and_then(|a| a.get::<String>());
        addr.is_some_and(|a| a.eq_ignore_ascii_case(&want))
    })
}

fn property(address: &str, iface: &str, name: &str) -> Option<Variant> {
    lookup(&lookup(&device(address)?, iface)?, name)
}

/// Battery % BlueZ last read from the device with this address.
pub fn battery(address: &str) -> Option<u8> {
    let pct = property(address, BATTERY_IFACE, "Percentage")?.get::<u8>()?;
    Some(pct.min(MAX_PERCENT))
}

/// Name of the device as the user sees it (alias), e.g. "AET-MS480BBT2".
pub fn name(address: &str) -> Option<String> {
    property(address, DEVICE_IFACE, "Alias")?.get::<String>().filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_gets_colons() {
        assert_eq!(colon_address("b66b18d71e00"), "B6:6B:18:D7:1E:00");
    }

    #[test]
    fn no_bluez_is_none() {
        assert_eq!(battery("000000000000"), None);
    }
}
