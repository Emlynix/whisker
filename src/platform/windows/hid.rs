//! Windows HID access: enumerate collections via SetupDi, exchange
//! input/output reports (overlapped, with timeouts) or feature reports.
//! Knows nothing about any protocol.
//! Talks to `hid.dll` / `setupapi.dll` directly (no extra crates).

use super::win::wide;
use crate::hid_link::{DeviceInfo, Interface};
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
fn describe(path: String) -> Option<Interface> {
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

/// Collections of devices with vendor id `vid` (descriptors only, nothing sent).
pub fn interfaces(vid: u16) -> Vec<Interface> {
    let tag = format!("vid_{vid:04x}");
    interface_paths().into_iter().filter(|p| p.to_lowercase().contains(&tag)).filter_map(describe).collect()
}

/// Mouse collections (generic desktop / mouse) of every vendor.
pub fn mice() -> Vec<Interface> {
    interface_paths().into_iter().filter_map(describe).filter(|i| (i.usage_page, i.usage) == (USAGE_PAGE_DESKTOP, USAGE_MOUSE)).collect()
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
