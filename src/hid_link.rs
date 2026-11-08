//! HID driver: finds the mouse configuration interface and moves raw
//! 33-byte feature reports. Knows nothing about what the bytes mean.
//! Talks to `hid.dll` / `setupapi.dll` directly (no extra crates).

use crate::protocol::{CONFIG_USAGE_PAGE, PID_DONGLE, PID_WIRED, REPORT_ID, REPORT_LEN, VENDOR_ID};
use crate::win::wide;
use std::time::Duration;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::core::GUID;

/// Time the firmware needs before a reply can be read back (vendor app uses 10 ms).
const REPLY_DELAY: Duration = Duration::from_millis(10);
const VID_TAG: &str = "vid_248a";
const DETAIL_BUF: usize = 1024;
/// `cbSize` of SP_DEVICE_INTERFACE_DETAIL_DATA_W (packing differs per architecture).
#[cfg(target_pointer_width = "64")]
const DETAIL_CB_SIZE: u32 = 8;
#[cfg(target_pointer_width = "32")]
const DETAIL_CB_SIZE: u32 = 6;
/// Only these product ids are ever opened.
const PID_TAGS: [&str; 2] = ["pid_5b2e", "pid_5b2f"];

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub struct HidLink {
    dev: Option<Handle>,
}

/// Lists HID interface paths whose name contains the vendor id.
fn vendor_paths() -> Vec<String> {
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
            let lower = path.to_lowercase();
            if lower.contains(VID_TAG) && PID_TAGS.iter().any(|t| lower.contains(t)) {
                out.push(path);
            }
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    out
}

/// Opens a path if it is the mouse config collection; returns its product id.
fn open_config(path: &str) -> Option<(Handle, u16)> {
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
        if attr.ProductID != PID_WIRED && attr.ProductID != PID_DONGLE {
            return None;
        }

        let mut pp: PHIDP_PREPARSED_DATA = 0;
        if !HidD_GetPreparsedData(h.0, &mut pp) {
            return None;
        }
        let mut caps: HIDP_CAPS = std::mem::zeroed();
        HidP_GetCaps(pp, &mut caps);
        HidD_FreePreparsedData(pp);

        let fits = caps.UsagePage == CONFIG_USAGE_PAGE && caps.FeatureReportByteLength as usize == REPORT_LEN;
        fits.then_some((h, attr.ProductID))
    }
}

impl HidLink {
    pub fn new() -> Self {
        Self { dev: None }
    }

    /// Opens the config interface if needed. Wired beats dongle, as in the vendor app.
    fn ensure_open(&mut self) -> Option<HANDLE> {
        if self.dev.is_none() {
            let mut found: Vec<_> = vendor_paths().iter().filter_map(|p| open_config(p)).collect();
            found.sort_by_key(|(_, pid)| *pid != PID_WIRED);
            self.dev = found.into_iter().next().map(|(h, _)| h);
        }
        self.dev.as_ref().map(|h| h.0)
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
