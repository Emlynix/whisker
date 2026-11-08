//! Generic HID access for mouse drivers: find interfaces by vendor and
//! usage, then exchange input/output reports (with timeouts) or feature
//! reports. Knows nothing about any protocol.

use super::{DeviceInfo, interface_paths, read_info};
use crate::win::wide;
use std::time::{Duration, Instant};
use windows_sys::Win32::Devices::HumanInterfaceDevice::*;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadFile, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

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

/// Collections of devices with vendor id `vid` (descriptors only, nothing sent).
pub fn interfaces(vid: u16) -> Vec<Interface> {
    let tag = format!("vid_{vid:04x}");
    let mut out = Vec::new();

    for path in interface_paths().into_iter().filter(|p| p.to_lowercase().contains(&tag)) {
        let name = wide(&path);
        let raw = unsafe { CreateFileW(name.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
        if raw == INVALID_HANDLE_VALUE {
            continue;
        }

        unsafe {
            let mut attr: HIDD_ATTRIBUTES = std::mem::zeroed();
            attr.Size = size_of::<HIDD_ATTRIBUTES>() as u32;
            let mut pp: PHIDP_PREPARSED_DATA = 0;
            if HidD_GetAttributes(raw, &mut attr) && HidD_GetPreparsedData(raw, &mut pp) {
                let mut caps: HIDP_CAPS = std::mem::zeroed();
                HidP_GetCaps(pp, &mut caps);
                HidD_FreePreparsedData(pp);
                out.push(Interface {
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
    }
    out
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
        if unsafe { WaitForSingleObject(self.event, ms) } == WAIT_TIMEOUT {
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
