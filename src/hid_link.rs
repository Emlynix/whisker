//! HID driver: finds the mouse configuration interface and moves raw
//! 33-byte feature reports. Knows nothing about what the bytes mean.
//! Talks to `hid.dll` / `setupapi.dll` directly (no extra crates).

use crate::protocol::{CONFIG_USAGE_PAGE, PID_WIRED, REPORT_ID, REPORT_LEN, VENDOR_ID};
use crate::win::wide;
use std::time::Duration;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::core::GUID;

pub mod raw;

/// Time the firmware needs before a reply can be read back (vendor app uses 10 ms).
const REPLY_DELAY: Duration = Duration::from_millis(10);
const VID_TAG: &str = "vid_248a";
const DETAIL_BUF: usize = 1024;
/// `cbSize` of SP_DEVICE_INTERFACE_DETAIL_DATA_W (packing differs per architecture).
#[cfg(target_pointer_width = "64")]
const DETAIL_CB_SIZE: u32 = 8;
#[cfg(target_pointer_width = "32")]
const DETAIL_CB_SIZE: u32 = 6;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub struct HidLink {
    dev: Option<Handle>,
    info: Option<DeviceInfo>,
}

/// Identity of a USB HID device, as reported by Windows.
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

/// Opens a HID path for queries only (no reads or writes).
fn open_query(path: &str) -> Option<Handle> {
    let name = wide(path);
    let raw = unsafe { CreateFileW(name.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    (raw != INVALID_HANDLE_VALUE).then_some(Handle(raw))
}

fn usage_of(h: HANDLE) -> Option<(u16, u16)> {
    unsafe {
        let mut pp: PHIDP_PREPARSED_DATA = 0;
        if !HidD_GetPreparsedData(h, &mut pp) {
            return None;
        }
        let mut caps: HIDP_CAPS = std::mem::zeroed();
        HidP_GetCaps(pp, &mut caps);
        HidD_FreePreparsedData(pp);
        Some((caps.UsagePage, caps.Usage))
    }
}

/// Connected mice (HID mouse collections), one entry per VID:PID.
/// Only descriptors are read; nothing is sent to these devices.
pub fn list_mice() -> Vec<DeviceInfo> {
    let mut out: Vec<DeviceInfo> = Vec::new();
    for path in interface_paths() {
        let Some(h) = open_query(&path) else {
            continue;
        };
        if usage_of(h.0) != Some((USAGE_PAGE_DESKTOP, USAGE_MOUSE)) {
            continue;
        }
        let mut attr: HIDD_ATTRIBUTES = unsafe { std::mem::zeroed() };
        attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        if !unsafe { HidD_GetAttributes(h.0, &mut attr) } {
            continue;
        }
        if out.iter().any(|d| (d.vid, d.pid) == (attr.VendorID, attr.ProductID)) {
            continue;
        }
        let mut info = read_info(h.0, &attr);
        info.bt_address = crate::bluetooth::address_from_path(&path);
        // BLE mice often have no USB product string; use the Bluetooth name.
        if info.product.is_empty() {
            if let Some(n) = info.bt_address.as_deref().and_then(crate::bluetooth::name) {
                info.product = n;
            }
        }
        out.push(info);
    }
    out
}

/// Lists HID interface paths whose name contains the vendor id.
fn vendor_paths() -> Vec<String> {
    interface_paths()
        .into_iter()
        .filter(|p| {
            let lower = p.to_lowercase();
            lower.contains(VID_TAG)
        })
        .collect()
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

/// Opens a path if it is the mouse config collection; returns its identity.
fn open_config(path: &str) -> Option<(Handle, DeviceInfo)> {
    let name = wide(path);
    let raw = unsafe {
        CreateFileW(name.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    if raw == INVALID_HANDLE_VALUE {
        return None;
    }
    let h = Handle(raw);

    unsafe {
        let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
        attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
        if !HidD_GetAttributes(h.0, &mut attr) || attr.VendorID != VENDOR_ID {
            return None;
        }
        // Any product of the vendor with the same config collection: other
        // models of the Lionel family. Writes stay locked until the status
        // reply carries the protocol signature (see mouse.rs).
        let mut pp: PHIDP_PREPARSED_DATA = 0;
        if !HidD_GetPreparsedData(h.0, &mut pp) {
            return None;
        }
        let mut caps: HIDP_CAPS = std::mem::zeroed();
        HidP_GetCaps(pp, &mut caps);
        HidD_FreePreparsedData(pp);

        let fits = caps.UsagePage == CONFIG_USAGE_PAGE && caps.FeatureReportByteLength as usize == REPORT_LEN;
        if !fits {
            return None;
        }
        let info = read_info(h.0, &attr);
        Some((h, info))
    }
}

impl HidLink {
    pub fn new() -> Self {
        Self { dev: None, info: None }
    }

    /// Opens the config interface if needed. Wired beats dongle, as in the vendor app.
    fn ensure_open(&mut self) -> Option<HANDLE> {
        if self.dev.is_none() {
            let mut found: Vec<_> = vendor_paths().iter().filter_map(|p| open_config(p)).collect();
            found.sort_by_key(|(_, info)| info.pid != PID_WIRED);
            let first = found.into_iter().next();
            self.info = first.as_ref().map(|(_, i)| i.clone());
            self.dev = first.map(|(h, _)| h);
        }
        self.dev.as_ref().map(|h| h.0)
    }

    /// Identity of the opened device, if any.
    pub fn info(&self) -> Option<&DeviceInfo> {
        self.info.as_ref()
    }

    pub fn is_present(&mut self) -> bool {
        self.ensure_open().is_some()
    }

    pub fn send(&mut self, frame: &[u8; REPORT_LEN]) -> bool {
        let Some(h) = self.ensure_open() else {
            return false;
        };
        let ok = unsafe { HidD_SetFeature(h, frame.as_ptr().cast(), REPORT_LEN as u32) };
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

        let h = self.ensure_open()?;
        let mut buf = [0u8; REPORT_LEN];
        buf[0] = REPORT_ID;
        let ok = unsafe { HidD_GetFeature(h, buf.as_mut_ptr().cast(), REPORT_LEN as u32) };
        if !ok {
            self.dev = None;
            return None;
        }
        Some(buf)
    }
}

#[cfg(test)]
mod tests {
    /// Hardware probe: `cargo test -- --ignored --nocapture print_mice`.
    #[test]
    #[ignore]
    fn print_mice() {
        for m in super::list_mice() {
            println!("{m:?}");
        }
        let mut l = super::HidLink::new();
        println!("config link present: {} {:?}", l.is_present(), l.info());
    }
}
