//! HID driver: finds the mouse configuration interface and moves raw
//! 33-byte feature reports. Knows nothing about what the bytes mean.
//! OS access goes through `raw` (enumeration and report I/O).
//!
//! BLE mice carry their Bluetooth address in the Windows HID path:
//!
//! ```text
//! \\?\hid#{00001812-…}_dev_vid&02045e_pid&0040_rev&0300_b66b18d71e00&col01#…
//!                                         address ^^^^^^^^^^^^
//! ```

use crate::protocol::{CONFIG_USAGE_PAGE, PID_WIRED, REPORT_ID, REPORT_LEN, VENDOR_ID};
use raw::{Channel, interfaces};
use std::time::Duration;

/// Generic HID access for mouse drivers: find interfaces by vendor and
/// usage, then exchange input/output or feature reports with timeouts.
pub mod raw {
    pub use super::Interface;
    pub use crate::platform::hid::{Channel, interfaces};
}

/// Time the firmware needs before a reply can be read back (vendor app uses 10 ms).
const REPLY_DELAY: Duration = Duration::from_millis(10);
/// HID-over-GATT service UUID as it appears in HID paths.
const HOGP_TAG: &str = "{00001812-";
const ADDRESS_LEN: usize = 12;

pub struct HidLink {
    dev: Option<Channel>,
    info: Option<DeviceInfo>,
}

/// Identity of a USB HID device, as reported by the OS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub vid: u16,
    pub pid: u16,
    /// `bcdDevice`: firmware release of the receiver or cable device.
    pub version: u16,
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    /// Bluetooth LE address for mice connected over Bluetooth.
    pub bt_address: Option<String>,
}

/// One HID top-level collection of a device.
#[derive(Clone, Debug)]
pub struct Interface {
    pub path: String,
    pub info: DeviceInfo,
    pub usage_page: u16,
    pub usage: u16,
    pub output_len: usize,
    pub feature_len: usize,
}

/// Connected mice (HID mouse collections), one entry per VID:PID.
/// Only descriptors are read; nothing is sent to these devices.
pub fn list_mice() -> Vec<DeviceInfo> {
    let mut out: Vec<DeviceInfo> = Vec::new();
    for i in crate::platform::hid::mice() {
        let mut info = i.info;
        if out.iter().any(|d| (d.vid, d.pid) == (info.vid, info.pid)) {
            continue;
        }
        info.bt_address = bt_address(&i.path);
        // BLE mice often have no USB product string; use the Bluetooth name.
        if info.product.is_empty() {
            if let Some(n) = info.bt_address.as_deref().and_then(crate::platform::bluetooth::name) {
                info.product = n;
            }
        }
        out.push(info);
    }
    out
}

/// Bluetooth address of a BLE HID device from its interface path, e.g.
/// `"B66B18D71E00"`; `None` for USB devices.
fn bt_address(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    if !lower.contains(HOGP_TAG) {
        return None;
    }
    // The address is the 12 hex digits after the last "_" before "&col" / "#".
    let head = lower.split('#').nth(1)?;
    let head = head.split("&col").next()?;
    let addr = head.rsplit('_').next()?;
    (addr.len() == ADDRESS_LEN && addr.chars().all(|c| c.is_ascii_hexdigit())).then(|| addr.to_uppercase())
}

/// Config collections of the vendor, wired first as in the vendor app.
/// Any product with the same collection counts: other models of the
/// Lionel family. Writes stay locked until the status reply carries the
/// protocol signature (see mouse.rs).
fn config_interfaces() -> Vec<Interface> {
    let mut found: Vec<Interface> = interfaces(VENDOR_ID)
        .into_iter()
        .filter(|i| i.info.vid == VENDOR_ID && i.usage_page == CONFIG_USAGE_PAGE && i.feature_len == REPORT_LEN)
        .collect();
    found.sort_by_key(|i| i.info.pid != PID_WIRED);
    found
}

impl HidLink {
    pub fn new() -> Self {
        Self { dev: None, info: None }
    }

    /// Opens the config interface if needed.
    fn ensure_open(&mut self) -> Option<&Channel> {
        if self.dev.is_none() {
            let first = config_interfaces().into_iter().find_map(|i| Some((Channel::open(&i.path)?, i.info)));
            self.info = first.as_ref().map(|(_, i)| i.clone());
            self.dev = first.map(|(c, _)| c);
        }
        self.dev.as_ref()
    }

    /// Identity of the opened device, if any.
    pub fn info(&self) -> Option<&DeviceInfo> {
        self.info.as_ref()
    }

    pub fn is_present(&mut self) -> bool {
        self.ensure_open().is_some()
    }

    pub fn send(&mut self, frame: &[u8; REPORT_LEN]) -> bool {
        let Some(ch) = self.ensure_open() else {
            return false;
        };
        let ok = ch.set_feature(frame);
        if !ok {
            self.dev = None;
        }
        ok
    }

    /// Sends a request and reads the reply report.
    pub fn query(&mut self, frame: &[u8; REPORT_LEN]) -> Option<[u8; REPORT_LEN]> {
        if !self.send(frame) {
            return None;
        }
        std::thread::sleep(REPLY_DELAY);

        let ch = self.ensure_open()?;
        let mut buf = [0u8; REPORT_LEN];
        buf[0] = REPORT_ID;
        if !ch.get_feature(&mut buf) {
            self.dev = None;
            return None;
        }
        Some(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_from_ble_hid_path() {
        let p = r"\\?\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev_vid&02045e_pid&0040_rev&0300_b66b18d71e00&col01#a&1&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(bt_address(p).as_deref(), Some("B66B18D71E00"));
    }

    #[test]
    fn usb_path_has_no_address() {
        let p = r"\\?\hid#vid_248a&pid_5b2f&mi_00#a&2736500e&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(bt_address(p), None);
    }

    /// Hardware probe: `cargo test -- --ignored --nocapture print_mice`.
    #[test]
    #[ignore]
    fn print_mice() {
        for m in list_mice() {
            println!("{m:?}");
        }
        let mut l = HidLink::new();
        println!("config link present: {} {:?}", l.is_present(), l.info());
    }
}
