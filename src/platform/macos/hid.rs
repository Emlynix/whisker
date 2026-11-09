//! macOS HID access over IOKit: enumerate `IOHIDDevice` services from the
//! registry (descriptors only, nothing sent), exchange input/output or
//! feature reports through `IOHIDDevice`. Knows nothing about any protocol.
//!
//! One IOKit device carries all its top-level collections; like Windows,
//! each collection (`DeviceUsagePairs`) becomes its own `Interface`, all
//! with the device's path:
//!
//! ```text
//!   DevSrvsID:4294970170                          USB / receiver
//!   DevSrvsID:4294970170#{00001812-mac}_b66b18d71e00   Bluetooth: the
//!        address in the shape `hid_link` reads from Windows paths
//! ```
//!
//! Report layout follows hidapi on macOS: byte 0 is the report id; for
//! devices without ids it is 0 and not sent. Input reports of such
//! devices get a 0 prepended, as Windows does.

use super::iokit::{Service, as_number, services};
use crate::hid_link::{DeviceInfo, Interface, ReportLayout, Transport, report_ids};
use objc2_core_foundation::{CFArray, CFData, CFDictionary, CFIndex, CFRetained, CFRunLoop, CFString, CFType};
use objc2_io_kit::{IOHIDDevice, IOHIDReportType, IOReturn, kIOHIDOptionsTypeNone};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::{Duration, Instant};

const HID_CLASS: &std::ffi::CStr = c"IOHIDDevice";
const PATH_PREFIX: &str = "DevSrvsID:";
/// Marks a Bluetooth path for `hid_link::bt_address` (HID-over-GATT tag).
const BT_TAG: &str = "#{00001812-mac}_";
const USAGE_PAGE_DESKTOP: u16 = 0x01;
const USAGE_MOUSE: u16 = 0x02;
/// `kIOReturnSuccess`.
const OK: IOReturn = 0;
/// Input buffer when the device does not state its report size.
const DEFAULT_INPUT_LEN: usize = 64;
/// Unread input reports kept; older ones are dropped.
const INBOX_MAX: usize = 64;
/// Private run-loop mode: `read` services only this device's reports.
const RUN_MODE: &str = "WhiskerHID";

fn number(s: &Service, key: &str) -> u16 {
    s.number(key).unwrap_or(0) as u16
}

fn string(s: &Service, key: &str) -> String {
    s.string(key).unwrap_or_default().trim().to_string()
}

/// `aa-bb-cc-dd-ee-ff` (the serial of Bluetooth HID devices) as
/// `"AABBCCDDEEFF"`.
pub(super) fn address_of(serial: &str) -> Option<String> {
    let hex: String = serial.chars().filter(|c| !matches!(c, '-' | ':')).collect();
    (hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_uppercase())
}

fn is_bluetooth(s: &Service) -> bool {
    s.string("Transport").is_some_and(|t| t.contains("Bluetooth"))
}

/// `(usage page, usage)` of every top-level collection.
fn usage_pairs(s: &Service) -> Vec<(u16, u16)> {
    let primary = vec![(number(s, "PrimaryUsagePage"), number(s, "PrimaryUsage"))];
    let Some(list) = s.property("DeviceUsagePairs") else {
        return primary;
    };
    let Some(list) = list.downcast_ref::<CFArray>() else {
        return primary;
    };
    let page_key = CFString::from_static_str("DeviceUsagePage");
    let usage_key = CFString::from_static_str("DeviceUsage");
    // SAFETY: IOKit fills the array with CF objects.
    let items = unsafe { list.cast_unchecked::<CFType>() };

    let mut out = Vec::new();
    for item in items.iter() {
        let Some(d) = item.downcast_ref::<CFDictionary>() else {
            continue;
        };
        // SAFETY: registry dictionaries have string keys and CF values.
        let d = unsafe { d.cast_unchecked::<CFString, CFType>() };
        let get = |k: &CFString| d.get(k).and_then(|v| as_number(&v)).unwrap_or(0) as u16;
        out.push((get(&page_key), get(&usage_key)));
    }
    if out.is_empty() { primary } else { out }
}

/// Collections of one device, read from the registry only.
fn describe(s: &Service) -> Vec<Interface> {
    let Some(id) = s.id() else {
        return Vec::new();
    };
    let serial = string(s, "SerialNumber");
    let address = is_bluetooth(s).then(|| address_of(&serial)).flatten();
    let path = match &address {
        Some(a) => format!("{PATH_PREFIX}{id}{BT_TAG}{}", a.to_lowercase()),
        None => format!("{PATH_PREFIX}{id}"),
    };
    let info = DeviceInfo {
        vid: number(s, "VendorID"),
        pid: number(s, "ProductID"),
        version: number(s, "VersionNumber"),
        manufacturer: string(s, "Manufacturer"),
        product: string(s, "Product"),
        serial,
        bt_address: None,
    };
    let (output_len, feature_len) = (number(s, "MaxOutputReportSize") as usize, number(s, "MaxFeatureReportSize") as usize);

    usage_pairs(s)
        .into_iter()
        .map(|(usage_page, usage)| Interface { path: path.clone(), info: info.clone(), usage_page, usage, output_len, feature_len })
        .collect()
}

/// Collections of devices with vendor id `vid` (descriptors only, nothing sent).
pub fn interfaces(vid: u16) -> Vec<Interface> {
    services(HID_CLASS).iter().filter(|s| number(s, "VendorID") == vid).flat_map(describe).collect()
}

/// Mouse collections (generic desktop / mouse) of every vendor.
pub fn mice() -> Vec<Interface> {
    services(HID_CLASS).iter().flat_map(describe).filter(|i| (i.usage_page, i.usage) == (USAGE_PAGE_DESKTOP, USAGE_MOUSE)).collect()
}

/// Report sizes, ids and raw descriptor of a device, from the registry
/// only. All collections of one device share this layout; `summary` stays
/// empty (the descriptor carries the details).
pub fn layout(path: &str) -> Option<ReportLayout> {
    let s = Service::by_id(entry_id(path)?)?;
    let size = |key: &str| s.number(key).unwrap_or(0).max(0) as usize;
    let descriptor = s.property("ReportDescriptor").and_then(|v| v.downcast_ref::<CFData>().map(CFData::to_vec));
    let transport = transport_of(&s);
    // Only USB devices have a USB interface above them.
    let interface = match transport {
        Transport::Usb => s.inherited("bInterfaceNumber").and_then(|v| as_number(&v)).and_then(|n| u8::try_from(n).ok()),
        _ => None,
    };

    Some(ReportLayout {
        transport,
        interface,
        input_len: size("MaxInputReportSize"),
        output_len: size("MaxOutputReportSize"),
        feature_len: size("MaxFeatureReportSize"),
        report_ids: descriptor.as_deref().map(report_ids).unwrap_or_default(),
        descriptor,
        summary: Vec::new(),
    })
}

/// `Transport` property: "USB", "Bluetooth", "Bluetooth Low Energy", …
fn transport_of(s: &Service) -> Transport {
    let t = s.string("Transport").unwrap_or_default();
    if t.contains("Bluetooth") {
        return Transport::Bluetooth;
    }
    if t.contains("USB") {
        return Transport::Usb;
    }
    Transport::Other
}

/// Registry entry id from a path made by `describe`.
fn entry_id(path: &str) -> Option<u64> {
    let rest = path.strip_prefix(PATH_PREFIX)?;
    rest.split('#').next()?.parse().ok()
}

/// Input reports collected by the IOKit callback until `read` takes them.
#[derive(Default)]
struct Inbox {
    reports: RefCell<VecDeque<Vec<u8>>>,
}

/// Open device; input reports arrive on the run loop of the thread that
/// reads, in a private mode, so `read` can wait with a timeout.
pub struct Channel {
    dev: CFRetained<IOHIDDevice>,
    inbox: Box<Inbox>,
    /// IOKit writes each input report here before the callback runs.
    _buf: Box<[u8]>,
    mode: CFRetained<CFString>,
    scheduled: RefCell<Option<CFRetained<CFRunLoop>>>,
}

// SAFETY: IOHIDDevice calls are thread-safe; the inbox and run-loop state
// are only used by the thread that owns the channel (it is not `Sync`).
unsafe impl Send for Channel {}

impl Drop for Channel {
    fn drop(&mut self) {
        if let Some(rl) = self.scheduled.take() {
            // SAFETY: unscheduling from the loop it was scheduled on.
            unsafe { self.dev.unschedule_from_run_loop(&rl, &self.mode) };
        }
        self.dev.close(kIOHIDOptionsTypeNone);
    }
}

/// Copies one input report into the inbox (IOKit callback).
unsafe extern "C-unwind" fn on_report(ctx: *mut c_void, result: IOReturn, _sender: *mut c_void, _kind: IOHIDReportType, id: u32, report: NonNull<u8>, len: CFIndex) {
    if result != OK || ctx.is_null() {
        return;
    }
    // SAFETY: `ctx` is the channel's boxed inbox, alive while registered.
    let inbox = unsafe { &*(ctx as *const Inbox) };
    // SAFETY: IOKit passes `len` valid bytes.
    let bytes = unsafe { std::slice::from_raw_parts(report.as_ptr(), len.max(0) as usize) };

    // Unnumbered reports: prepend id 0, as Windows does.
    let mut r = Vec::with_capacity(bytes.len() + 1);
    if id == 0 {
        r.push(0);
    }
    r.extend_from_slice(bytes);

    let mut q = inbox.reports.borrow_mut();
    if q.len() >= INBOX_MAX {
        q.pop_front();
    }
    q.push_back(r);
}

impl Channel {
    pub fn open(path: &str) -> Option<Channel> {
        let service = Service::by_id(entry_id(path)?)?;
        let input_len = service.number("MaxInputReportSize").map_or(DEFAULT_INPUT_LEN, |n| (n as usize).max(1));
        let dev = IOHIDDevice::new(None, service.0)?;
        if dev.open(kIOHIDOptionsTypeNone) != OK {
            return None;
        }

        let inbox = Box::<Inbox>::default();
        let mut buf = vec![0u8; input_len].into_boxed_slice();
        let ctx = (&*inbox as *const Inbox).cast_mut().cast::<c_void>();
        let ptr = NonNull::new(buf.as_mut_ptr())?;
        // SAFETY: `buf` and `inbox` are heap blocks owned by the channel,
        // which unregisters (closes the device) before freeing them.
        unsafe { dev.register_input_report_callback(ptr, input_len as CFIndex, Some(on_report), ctx) };

        let mode = CFString::from_static_str(RUN_MODE);
        Some(Channel { dev, inbox, _buf: buf, mode, scheduled: RefCell::new(None) })
    }

    /// Attaches report delivery to the calling thread's run loop.
    fn schedule(&self) -> Option<()> {
        let here = CFRunLoop::current()?;
        let mut cur = self.scheduled.borrow_mut();
        if cur.as_ref().is_some_and(|rl| CFRetained::as_ptr(rl) == CFRetained::as_ptr(&here)) {
            return Some(());
        }
        // SAFETY: (un)scheduling on live run loops in our private mode.
        unsafe {
            if let Some(old) = cur.take() {
                self.dev.unschedule_from_run_loop(&old, &self.mode);
            }
            self.dev.schedule_with_run_loop(&here, &self.mode);
        }
        *cur = Some(here);
        Some(())
    }

    /// Sends an output report (first byte = report id).
    pub fn write(&self, report: &[u8], _timeout: Duration) -> bool {
        // IOHIDDeviceSetReport is synchronous with IOKit's own timeout.
        self.send(IOHIDReportType::Output, report)
    }

    fn send(&self, kind: IOHIDReportType, report: &[u8]) -> bool {
        let Some(&id) = report.first() else {
            return false;
        };
        // Report id 0 means "no ids": the id byte is not part of the report.
        let data = if id == 0 { &report[1..] } else { report };
        let Some(ptr) = NonNull::new(data.as_ptr().cast_mut()) else {
            return false;
        };
        // SAFETY: IOKit only reads `data.len()` bytes for a set-report.
        unsafe { self.dev.set_report(kind, id as CFIndex, ptr, data.len() as CFIndex) == OK }
    }

    /// Next input report, or `None` after `timeout`.
    pub fn read(&self, len: usize, timeout: Duration) -> Option<Vec<u8>> {
        self.schedule()?;
        let start = Instant::now();
        loop {
            if let Some(mut r) = self.inbox.reports.borrow_mut().pop_front() {
                r.truncate(len);
                return Some(r);
            }
            let left = timeout.checked_sub(start.elapsed()).filter(|d| !d.is_zero())?;
            CFRunLoop::run_in_mode(Some(&self.mode), left.as_secs_f64(), true);
        }
    }

    /// Reads input reports until `accept` returns a value or the time runs out.
    pub fn read_until<T>(&self, len: usize, timeout: Duration, mut accept: impl FnMut(&[u8]) -> Option<T>) -> Option<T> {
        let start = Instant::now();
        while let Some(left) = timeout.checked_sub(start.elapsed()) {
            let r = self.read(len, left)?;
            if let Some(v) = accept(&r) {
                return Some(v);
            }
        }
        None
    }

    pub fn set_feature(&self, report: &[u8]) -> bool {
        self.send(IOHIDReportType::Feature, report)
    }

    /// Feature report into `buf`; `buf[0]` must hold the report id.
    pub fn get_feature(&self, buf: &mut [u8]) -> bool {
        let Some(&id) = buf.first() else {
            return false;
        };
        // Unnumbered: the report goes after the id byte, which stays 0.
        let data = if id == 0 { &mut buf[1..] } else { &mut buf[..] };
        let mut len = data.len() as CFIndex;
        let (Some(ptr), Some(len_ptr)) = (NonNull::new(data.as_mut_ptr()), NonNull::new(&mut len)) else {
            return false;
        };
        // SAFETY: IOKit writes at most `len` bytes into `data`.
        unsafe { self.dev.report(IOHIDReportType::Feature, id as CFIndex, ptr, len_ptr) == OK }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_round_trip() {
        assert_eq!(entry_id("DevSrvsID:4294970170"), Some(4294970170));
        assert_eq!(entry_id("DevSrvsID:42#{00001812-mac}_b66b18d71e00"), Some(42));
        assert_eq!(address_of("b6-6b-18-d7-1e-00").as_deref(), Some("B66B18D71E00"));
        assert_eq!(address_of("ABC123"), None);
    }
}
