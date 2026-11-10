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
    pub use crate::platform::hid::Channel;

    /// Collections of a vendor; none in demo mode.
    pub fn interfaces(vid: u16) -> Vec<Interface> {
        if crate::platform::demo() {
            return Vec::new();
        }
        crate::platform::hid::interfaces(vid)
    }
}

/// Time the firmware needs before a reply can be read back (vendor app uses 10 ms).
const REPLY_DELAY: Duration = Duration::from_millis(10);
/// HID-over-GATT service UUID as it appears in HID paths.
const HOGP_TAG: &str = "{00001812-";
const ADDRESS_LEN: usize = 12;
/// Generic desktop / mouse: the collection a `HidDevice` is found by.
const MOUSE_USAGE: (u16, u16) = (0x01, 0x02);
/// Report descriptor items (HID 1.11, 6.2.2): long-item prefix, the
/// Report ID tag with its size bits masked off.
const LONG_ITEM: u8 = 0xFE;
const TAG_MASK: u8 = 0xFC;
const TAG_REPORT_ID: u8 = 0x84;

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

/// How a device is connected.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Bluetooth,
    #[default]
    Other,
}

/// Report sizes, ids and caps of one collection, for bug reports.
/// Read from descriptors only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportLayout {
    pub transport: Transport,
    /// USB interface number (`bInterfaceNumber`), where known.
    pub interface: Option<u8>,
    pub input_len: usize,
    pub output_len: usize,
    pub feature_len: usize,
    /// Report ids of input, output and feature reports, sorted, distinct;
    /// empty for unnumbered reports.
    pub report_ids: Vec<u8>,
    /// Raw report descriptor where the OS exposes it (Linux; not Windows).
    pub descriptor: Option<Vec<u8>>,
    /// Short caps lines, e.g. `"input id 2: buttons 0x09:0x01-0x05"`.
    /// Never holds paths or serials.
    pub summary: Vec<String>,
}

/// One physical mouse: its mouse collection(s) plus the sibling
/// collections (vendor pages etc.) of the same device, mouse first.
#[derive(Clone, Debug)]
pub struct HidDevice {
    pub info: DeviceInfo,
    pub collections: Vec<(Interface, ReportLayout)>,
}

/// Identity of a mouse collection, with its Bluetooth address.
fn identify(i: &Interface) -> DeviceInfo {
    let mut info = i.info.clone();
    info.bt_address = bt_address(&i.path);
    // BLE mice often have no USB product string; use the Bluetooth name.
    if info.product.is_empty() {
        if let Some(n) = info.bt_address.as_deref().and_then(crate::platform::bluetooth::name) {
            info.product = n;
        }
    }
    info
}

/// Mouse collections; none in demo mode.
fn mice() -> Vec<Interface> {
    if crate::platform::demo() {
        return Vec::new();
    }
    crate::platform::hid::mice()
}

/// Connected mice (HID mouse collections), one entry per VID:PID.
/// Only descriptors are read; nothing is sent to these devices.
pub fn list_mice() -> Vec<DeviceInfo> {
    let started = std::time::Instant::now();
    let mut out: Vec<DeviceInfo> = Vec::new();
    for i in mice() {
        if out.iter().any(|d| (d.vid, d.pid) == (i.info.vid, i.info.pid)) {
            continue;
        }
        out.push(identify(&i));
    }
    log_mice(&out, started.elapsed());
    out
}

/// A listing slower than this is logged even when nothing changed.
const SLOW_LISTING: std::time::Duration = std::time::Duration::from_millis(500);

/// Logs the mice when they changed since the last listing or the listing
/// was slow, e.g. `mice in 38 ms: 248A:5B2F "Lionel", 046D:B023 (bluetooth)`.
/// The app lists every few seconds; the log must not fill with repeats.
fn log_mice(mice: &[DeviceInfo], took: std::time::Duration) {
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

    // A Bluetooth name is the owner's to choose ("Marek's mouse"): left out.
    let name = |d: &DeviceInfo| match d.bt_address {
        Some(_) => "(bluetooth)".to_string(),
        None => format!("\"{}\"", crate::log::name(&d.product)),
    };
    let names: Vec<String> = mice.iter().map(|d| format!("{:04X}:{:04X} {}", d.vid, d.pid, name(d))).collect();
    let list = if names.is_empty() { "none".to_string() } else { names.join(", ") };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last == list && took < SLOW_LISTING {
        return;
    }

    crate::log!("mice in {} ms: {list}", took.as_millis());
    *last = list;
}

/// Connected mice as in `list_mice`, each with every collection of the
/// same VID:PID (and Bluetooth address) and its report layout.
/// Descriptors only; nothing is opened for I/O or sent.
pub fn mouse_devices() -> Vec<HidDevice> {
    let mut out: Vec<HidDevice> = Vec::new();
    for m in mice() {
        if out.iter().any(|d| (d.info.vid, d.info.pid) == (m.info.vid, m.info.pid)) {
            continue;
        }
        let info = identify(&m);
        let collections = siblings(m, &info).into_iter().map(|i| {
            // A collection that vanished meanwhile keeps an empty layout.
            let layout = crate::platform::hid::layout(&i.path).unwrap_or_default();
            (i, layout)
        });
        out.push(HidDevice { collections: collections.collect(), info });
    }
    out
}

/// `mouse` and the other collections of its device, each once (Linux
/// shares one hidraw node per device, so usage keeps them apart), mouse first.
fn siblings(mouse: Interface, info: &DeviceInfo) -> Vec<Interface> {
    let same = |i: &Interface| (i.info.vid, i.info.pid) == (info.vid, info.pid) && bt_address(&i.path) == info.bt_address;
    let key = |i: &Interface| (i.path.clone(), i.usage_page, i.usage);

    let mut out = vec![mouse];
    for i in interfaces(info.vid) {
        if !same(&i) || out.iter().any(|o| key(o) == key(&i)) {
            continue;
        }
        out.push(i);
    }
    // Stable sort: the found mouse stays ahead of further mouse collections.
    out.sort_by_key(|i| (i.usage_page, i.usage) != MOUSE_USAGE);
    out
}

/// Report ids declared in a raw report descriptor, sorted, distinct.
/// Walks short items for Report ID (`0x85 id`); skips long items; stops
/// at a truncated item, so garbage never panics.
#[cfg_attr(windows, allow(dead_code))] // Windows reads ids from caps
pub fn report_ids(desc: &[u8]) -> Vec<u8> {
    let mut ids = Vec::new();
    let mut at = 0;
    while let Some(&prefix) = desc.get(at) {
        // Long item: prefix, data size, tag, data.
        if prefix == LONG_ITEM {
            let Some(&len) = desc.get(at + 1) else {
                break;
            };
            at += 3 + len as usize;
            continue;
        }

        // Short item: size code 3 means 4 data bytes.
        let len = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let Some(data) = desc.get(at + 1..at + 1 + len) else {
            break;
        };
        if prefix & TAG_MASK == TAG_REPORT_ID {
            if let Some(&id) = data.first().filter(|&&id| id != 0) {
                ids.push(id);
            }
        }
        at += 1 + len;
    }
    ids.sort_unstable();
    ids.dedup();
    ids
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

    /// Mouse (id 2: 5 buttons, X/Y 16 bit) plus a vendor collection
    /// (id 5, 32-byte feature) with a long item in between.
    const MOUSE_DESC: [u8; 76] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00,
        0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
        0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x06, 0xC0, 0xC0, 0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1,
        0x01, 0xFE, 0x01, 0x10, 0x85, 0x85, 0x05, 0x95, 0x20, 0x75, 0x08, 0x09, 0x02, 0xB1, 0x02, 0xC0,
    ];

    #[test]
    fn ids_from_mouse_descriptor() {
        assert_eq!(report_ids(&MOUSE_DESC), vec![2, 5]);
        assert_eq!(report_ids(&[0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0xC0]), Vec::<u8>::new());
    }

    #[test]
    fn ids_from_garbage_never_panic() {
        assert_eq!(report_ids(&[0x85]), Vec::<u8>::new());
        assert_eq!(report_ids(&[0xFE]), Vec::<u8>::new());
        assert_eq!(report_ids(&[0xFE, 0xFF, 0x00]), Vec::<u8>::new());
        assert_eq!(report_ids(&[0x87, 0x01, 0x00]), Vec::<u8>::new());
        let noise: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for n in 0..noise.len() {
            report_ids(&noise[n..]);
        }
    }

    /// Hardware probe: `cargo test -- --ignored --nocapture print_mouse_devices`.
    #[test]
    #[ignore]
    fn print_mouse_devices() {
        for d in mouse_devices() {
            println!("{:?}", d.info);
            for (i, l) in &d.collections {
                let head = format!("  {:#06x}:{:#06x} {:?} if {:?}", i.usage_page, i.usage, l.transport, l.interface);
                println!("{head} in {} out {} feat {} ids {:?}", l.input_len, l.output_len, l.feature_len, l.report_ids);
                for s in &l.summary {
                    println!("    {s}");
                }
            }
        }
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
