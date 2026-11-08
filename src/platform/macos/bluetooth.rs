//! Battery and name of Bluetooth mice from the IOKit registry. macOS
//! publishes the level as `BatteryPercent` on the HID device (or a parent)
//! for devices whose battery it tracks (Apple mice, most BLE mice using the
//! standard GATT Battery Service). The address is the device serial:
//!
//! ```text
//!   IOHIDDevice  Transport = "Bluetooth Low Energy"
//!                SerialNumber = "b6-6b-18-d7-1e-00"   BatteryPercent = 87
//! ```
//! Devices without that property report `None`.

use super::hid::address_of;
use super::iokit::{Service, as_number, services};

const HID_CLASS: &std::ffi::CStr = c"IOHIDDevice";
const BATTERY_KEY: &str = "BatteryPercent";
const MAX_PERCENT: i64 = 100;

/// HID device whose serial is this 12-hex-digit address.
fn find(address: &str) -> Option<Service> {
    let want = address.to_uppercase();
    services(HID_CLASS).into_iter().find(|s| s.string("SerialNumber").and_then(|v| address_of(&v)).is_some_and(|a| a == want))
}

/// Battery % macOS last read from the device with this address.
pub fn battery(address: &str) -> Option<u8> {
    let v = find(address)?.inherited(BATTERY_KEY)?;
    Some(as_number(&v)?.clamp(0, MAX_PERCENT) as u8)
}

/// Product name of the device, e.g. "AET-MS480BBT2".
pub fn name(address: &str) -> Option<String> {
    find(address)?.string("Product").filter(|s| !s.is_empty())
}
