//! Windows HID access: enumerate collections via SetupDi, exchange
//! input/output reports (overlapped, with timeouts) or feature reports.
//! Knows nothing about any protocol.
//! Talks to `hid.dll` / `setupapi.dll` directly (no extra crates).

use super::win::wide;
use crate::hid_link::{DeviceInfo, Interface, ReportLayout, Transport};
use std::time::{Duration, Instant};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::core::GUID;

const DETAIL_BUF: usize = 1024;
/// `cbSize` of SP_DEVICE_INTERFACE_DETAIL_DATA_W (packing differs per architecture).
#[cfg(target_pointer_width = "64")]
const DETAIL_CB_SIZE: u32 = 8;
#[cfg(target_pointer_width = "32")]
const DETAIL_CB_SIZE: u32 = 6;
const STRING_CHARS: usize = 127;
const USAGE_PAGE_DESKTOP: u16 = 0x01;
const USAGE_MOUSE: u16 = 0x02;

type HidString = unsafe extern "system" fn(HANDLE, *mut core::ffi::c_void, u32) -> bool;

fn hid_string(h: HANDLE, f: HidString) -> String {
    let mut buf = [0u16; STRING_CHARS + 1];
    if !unsafe { f(h, buf.as_mut_ptr().cast(), (STRING_CHARS * 2) as u32) } {
        return String::new();
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(STRING_CHARS);
    String::from_utf16_lossy(&buf[..len]).trim().to_string()
}

fn read_info(h: HANDLE, attr: &HIDD_ATTRIBUTES) -> DeviceInfo {
    DeviceInfo {
        vid: attr.VendorID,
        pid: attr.ProductID,
        version: attr.VersionNumber,
        manufacturer: hid_string(h, HidD_GetManufacturerString),
        product: hid_string(h, HidD_GetProductString),
        serial: hid_string(h, HidD_GetSerialNumberString),
        bt_address: None,
    }
}

/// All present HID interface paths.
fn interface_paths() -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut guid: GUID = std::mem::zeroed();
        HidD_GetHidGuid(&mut guid);
        let set = SetupDiGetClassDevsW(&guid, std::ptr::null(), std::ptr::null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if set == INVALID_HANDLE_VALUE as isize {
            return out;
        }

        let mut buf = vec![0u64; DETAIL_BUF / 8];
        for i in 0.. {
            let mut did: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            did.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            if SetupDiEnumDeviceInterfaces(set, std::ptr::null(), &guid, i, &mut did) == 0 {
                break;
            }

            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = DETAIL_CB_SIZE;
            let ok = SetupDiGetDeviceInterfaceDetailW(set, &did, detail, DETAIL_BUF as u32, std::ptr::null_mut(), std::ptr::null_mut());
            if ok == 0 {
                continue;
            }

            let p = (*detail).DevicePath.as_ptr();
            let len = (0..).take_while(|&k| *p.add(k) != 0).count();
            let path = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
            out.push(path);
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    out
}

/// Descriptor of one collection, opened for queries only (no reads or writes).
/// Collections `keep` rejects by (usage page, usage) are dropped before
/// their strings are read: each string is a request to the device and
/// can take seconds on a sleeping wireless one.
fn describe(path: String, keep: impl Fn(u16, u16) -> bool) -> Option<Interface> {
    let name = wide(&path);
    let raw = unsafe { CreateFileW(name.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if raw == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut out = None;
    unsafe {
        let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
        attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        let mut pp: PHIDP_PREPARSED_DATA = 0;
        if HidD_GetAttributes(raw, &mut attr) && HidD_GetPreparsedData(raw, &mut pp) {
            let mut caps: HIDP_CAPS = std::mem::zeroed();
            HidP_GetCaps(pp, &mut caps);
            HidD_FreePreparsedData(pp);
            if !keep(caps.UsagePage, caps.Usage) {
                CloseHandle(raw);
                return None;
            }
            out = Some(Interface {
                info: read_info(raw, &attr),
                path,
                usage_page: caps.UsagePage,
                usage: caps.Usage,
                output_len: caps.OutputReportByteLength as usize,
                feature_len: caps.FeatureReportByteLength as usize,
            });
        }
        CloseHandle(raw);
    }
    out
}

/// Report sizes, ids and caps of one collection; opened for queries only
/// (no reads or writes). Windows does not expose the raw descriptor.
pub fn layout(path: &str) -> Option<ReportLayout> {
    let name = wide(path);
    // SAFETY: a NUL-terminated path; access 0 only queries the device.
    let raw = unsafe { CreateFileW(name.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if raw == INVALID_HANDLE_VALUE {
        return None;
    }

    let mut pp: PHIDP_PREPARSED_DATA = 0;
    // SAFETY: `raw` is an open handle; `pp` a valid out pointer.
    let got = unsafe { HidD_GetPreparsedData(raw, &mut pp) };
    // SAFETY: the handle is ours and no longer needed.
    unsafe { CloseHandle(raw) };
    if !got {
        return None;
    }

    let out = caps_layout(pp, path);
    // SAFETY: `pp` came from HidD_GetPreparsedData and is freed once.
    unsafe { HidD_FreePreparsedData(pp) };
    out
}

/// Report types in caps order, with their names for `summary`.
const REPORT_KINDS: [(HIDP_REPORT_TYPE, &str); 3] = [(HidP_Input, "input"), (HidP_Output, "output"), (HidP_Feature, "feature")];
/// Path tags: HID-over-GATT and classic Bluetooth HID service UUIDs.
const BT_TAGS: [&str; 2] = ["{00001812-", "{00001124-"];
const USB_TAG: &str = "hid#vid_";
/// USB interface number in a composite device path, e.g. `&mi_01`.
const INTERFACE_TAG: &str = "&mi_";

fn caps_layout(pp: PHIDP_PREPARSED_DATA, path: &str) -> Option<ReportLayout> {
    let mut caps: HIDP_CAPS = unsafe { std::mem::zeroed() };
    // SAFETY: valid preparsed data and out pointer.
    if unsafe { HidP_GetCaps(pp, &mut caps) } != HIDP_STATUS_SUCCESS {
        return None;
    }

    let mut ids = Vec::new();
    let mut summary = Vec::new();
    for (kind, label) in REPORT_KINDS {
        for b in button_caps(pp, kind) {
            ids.push(b.ReportID);
            summary.push(format!("{label} id {}: buttons {}", b.ReportID, button_usages(&b)));
        }
        for v in value_caps(pp, kind) {
            ids.push(v.ReportID);
            summary.push(format!("{label} id {}: value {}", v.ReportID, value_line(&v)));
        }
    }
    // Id 0 means unnumbered reports.
    ids.retain(|&id| id != 0);
    ids.sort_unstable();
    ids.dedup();

    let lower = path.to_lowercase();
    Some(ReportLayout {
        transport: transport_of(&lower),
        interface: interface_of(&lower),
        input_len: caps.InputReportByteLength as usize,
        output_len: caps.OutputReportByteLength as usize,
        feature_len: caps.FeatureReportByteLength as usize,
        report_ids: ids,
        descriptor: None,
        summary,
    })
}

fn button_caps(pp: PHIDP_PREPARSED_DATA, kind: HIDP_REPORT_TYPE) -> Vec<HIDP_BUTTON_CAPS> {
    // First call with length 0 reports how many caps there are.
    let mut n = 0u16;
    // SAFETY: a null buffer of length 0 only asks for the count.
    unsafe { HidP_GetButtonCaps(kind, std::ptr::null_mut(), &mut n, pp) };
    let mut out = vec![HIDP_BUTTON_CAPS::default(); n as usize];
    // SAFETY: `out` holds `n` entries.
    if n == 0 || unsafe { HidP_GetButtonCaps(kind, out.as_mut_ptr(), &mut n, pp) } != HIDP_STATUS_SUCCESS {
        return Vec::new();
    }
    out.truncate(n as usize);
    out
}

fn value_caps(pp: PHIDP_PREPARSED_DATA, kind: HIDP_REPORT_TYPE) -> Vec<HIDP_VALUE_CAPS> {
    let mut n = 0u16;
    // SAFETY: a null buffer of length 0 only asks for the count.
    unsafe { HidP_GetValueCaps(kind, std::ptr::null_mut(), &mut n, pp) };
    let mut out = vec![HIDP_VALUE_CAPS::default(); n as usize];
    // SAFETY: `out` holds `n` entries.
    if n == 0 || unsafe { HidP_GetValueCaps(kind, out.as_mut_ptr(), &mut n, pp) } != HIDP_STATUS_SUCCESS {
        return Vec::new();
    }
    out.truncate(n as usize);
    out
}

/// `0x09:0x01-0x05` for a usage range, `0x09:0x01` for one usage.
fn button_usages(b: &HIDP_BUTTON_CAPS) -> String {
    // SAFETY: `IsRange` says which union member the OS filled.
    let (lo, hi) = unsafe { if b.IsRange { (b.Anonymous.Range.UsageMin, b.Anonymous.Range.UsageMax) } else { (b.Anonymous.NotRange.Usage, b.Anonymous.NotRange.Usage) } };
    usages(b.UsagePage, lo, hi)
}

/// `0x01:0x30 16 bit -32767..32767`, with `x2` when the field repeats.
fn value_line(v: &HIDP_VALUE_CAPS) -> String {
    // SAFETY: `IsRange` says which union member the OS filled.
    let (lo, hi) = unsafe { if v.IsRange { (v.Anonymous.Range.UsageMin, v.Anonymous.Range.UsageMax) } else { (v.Anonymous.NotRange.Usage, v.Anonymous.NotRange.Usage) } };
    let count = if v.ReportCount > 1 { format!(" x{}", v.ReportCount) } else { String::new() };
    format!("{} {} bit{count} {}..{}", usages(v.UsagePage, lo, hi), v.BitSize, v.LogicalMin, v.LogicalMax)
}

fn usages(page: u16, lo: u16, hi: u16) -> String {
    if lo == hi {
        return format!("{page:#04x}:{lo:#04x}");
    }
    format!("{page:#04x}:{lo:#04x}-{hi:#04x}")
}

/// Bluetooth by service UUID, USB by `hid#vid_`; `lower` is lowercase.
fn transport_of(lower: &str) -> Transport {
    if BT_TAGS.iter().any(|t| lower.contains(t)) {
        return Transport::Bluetooth;
    }
    if lower.contains(USB_TAG) {
        return Transport::Usb;
    }
    Transport::Other
}

/// `&mi_01` → `Some(1)`; `None` for single-interface devices.
fn interface_of(lower: &str) -> Option<u8> {
    let at = lower.find(INTERFACE_TAG)? + INTERFACE_TAG.len();
    let hex = lower.get(at..at + 2)?;
    u8::from_str_radix(hex, 16).ok()
}

/// Collections of devices with vendor id `vid` (descriptors only, nothing sent).
pub fn interfaces(vid: u16) -> Vec<Interface> {
    let tag = format!("vid_{vid:04x}");
    interface_paths().into_iter().filter(|p| p.to_lowercase().contains(&tag)).filter_map(|p| describe(p, |_, _| true)).collect()
}

/// Mouse collections (generic desktop / mouse) of every vendor, USB and
/// Bluetooth only: a laptop's I²C touchpad is no mouse to configure.
pub fn mice() -> Vec<Interface> {
    let mouse = |page, usage| (page, usage) == (USAGE_PAGE_DESKTOP, USAGE_MOUSE);
    interface_paths().into_iter().filter(|p| external(p)).filter_map(|p| describe(p, mouse)).collect()
}

/// Connected over USB or Bluetooth, not built in (I²C, SPI, virtual).
fn external(path: &str) -> bool {
    transport_of(&path.to_lowercase()) != Transport::Other
}

/// Open read/write handle to one collection, with overlapped I/O so reads
/// can time out.
pub struct Channel {
    handle: HANDLE,
    event: HANDLE,
}

// The handles are only used from the thread that owns the channel.
unsafe impl Send for Channel {}

impl Drop for Channel {
    fn drop(&mut self) {
        unsafe {
            CancelIoEx(self.handle, std::ptr::null());
            CloseHandle(self.handle);
            CloseHandle(self.event);
        }
    }
}

impl Channel {
    pub fn open(path: &str) -> Option<Channel> {
        let name = wide(path);
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        // Manual-reset event for the overlapped operations.
        let event = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if event.is_null() {
            unsafe { CloseHandle(h) };
            return None;
        }
        Some(Channel { handle: h, event })
    }

    /// Waits for an overlapped operation; cancels it after `timeout`.
    fn finish(&self, ov: &mut OVERLAPPED, started: bool, timeout: Duration) -> Option<usize> {
        if !started && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return None;
        }
        let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        // Timeout or a failed wait: cancel, so the blocking result below returns.
        if unsafe { WaitForSingleObject(self.event, ms) } != WAIT_OBJECT_0 {
            unsafe { CancelIoEx(self.handle, ov) };
        }
        let mut n = 0u32;
        let ok = unsafe { GetOverlappedResult(self.handle, ov, &mut n, 1) };
        (ok != 0).then_some(n as usize)
    }

    fn overlapped(&self) -> OVERLAPPED {
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = self.event;
        ov
    }

    /// Sends an output report (first byte = report id).
    pub fn write(&self, report: &[u8], timeout: Duration) -> bool {
        let mut ov = self.overlapped();
        let started = unsafe { WriteFile(self.handle, report.as_ptr(), report.len() as u32, std::ptr::null_mut(), &mut ov) } != 0;
        self.finish(&mut ov, started, timeout) == Some(report.len())
    }

    /// Next input report, or `None` after `timeout`.
    pub fn read(&self, len: usize, timeout: Duration) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; len];
        let mut ov = self.overlapped();
        let started = unsafe { ReadFile(self.handle, buf.as_mut_ptr(), len as u32, std::ptr::null_mut(), &mut ov) } != 0;
        let n = self.finish(&mut ov, started, timeout)?;
        buf.truncate(n);
        Some(buf)
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
        unsafe { HidD_SetFeature(self.handle, report.as_ptr().cast(), report.len() as u32) }
    }

    /// Feature report into `buf`; `buf[0]` must hold the report id.
    pub fn get_feature(&self, buf: &mut [u8]) -> bool {
        unsafe { HidD_GetFeature(self.handle, buf.as_mut_ptr().cast(), buf.len() as u32) }
    }
}


/// No per-device permission to grant here.
pub fn access(_vid: u16) -> crate::platform::Access {
    crate::platform::Access::Granted
}

/// Nothing to install: devices open without a rule.
pub fn grant_command(_dir: &std::path::Path) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_and_interface_from_path() {
        let usb = r"\?\hid#vid_248a&pid_5b2f&mi_01&col02#a&2736500e&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let ble = r"\?\hid#{00001812-0000-1000-8000-00805f9b34fb}_dev_vid&02045e_pid&0040_rev&0300_b66b18d71e00&col01#a&1&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(transport_of(usb), Transport::Usb);
        assert_eq!(interface_of(usb), Some(1));
        assert_eq!(transport_of(ble), Transport::Bluetooth);
        assert_eq!(interface_of(ble), None);
        assert_eq!(transport_of(r"\?\hid#converteddevice&col01#1&2d595ca7&0&0000#{x}"), Transport::Other);
    }

    #[test]
    fn laptop_touchpads_are_not_mice() {
        // I²C HID touchpad ("HIDI2C Device"): an ACPI id, no USB vendor id.
        let touchpad = r"\?\hid#asue1306&col01#5&2501cc81&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let receiver = r"\?\hid#vid_046d&pid_c52b&mi_01&col01#8&2cc9b1de&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert!(!external(touchpad));
        assert!(external(receiver));
    }
}
