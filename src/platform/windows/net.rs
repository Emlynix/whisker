//! HTTPS GET over WinHTTP: system proxy settings, system certificate
//! store, TLS checked by Windows. Redirects, cookies and automatic
//! credentials are switched off; the caller follows redirects itself.

use super::win::wide;
use crate::platform::{Fetch, NetError, Reply};
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Networking::WinHttp::*;

const GET: &str = "GET";
const CHUNK: usize = 16 * 1024;
/// Longest `Location` header read, in UTF-16 units.
const LOCATION_MAX: usize = 8 * 1024;

/// WinHTTP handle, closed on drop.
struct Handle(*mut c_void);

impl Handle {
    fn new(h: *mut c_void) -> Result<Handle, NetError> {
        if h.is_null() {
            return Err(NetError::Failed);
        }
        Ok(Handle(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

pub fn get(req: &Fetch) -> Result<Reply, NetError> {
    let start = Instant::now();
    let agent = wide(req.agent);
    let session = Handle::new(unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0) })?;
    let ms = req.timeout.as_millis().min(i32::MAX as u128) as i32;
    unsafe { WinHttpSetTimeouts(session.0, ms, ms, ms, ms) };

    let host = wide(req.host);
    let connect = Handle::new(unsafe { WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0) })?;
    let (verb, path) = (wide(GET), wide(req.path));
    let request =
        Handle::new(unsafe { WinHttpOpenRequest(connect.0, verb.as_ptr(), path.as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE) })?;
    let off = WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_AUTHENTICATION;
    let set = unsafe { WinHttpSetOption(request.0, WINHTTP_OPTION_DISABLE_FEATURE, (&off as *const u32).cast(), size_of::<u32>() as u32) };
    // Revoked server certificates count as a failed connection.
    let revocation = WINHTTP_ENABLE_SSL_REVOCATION;
    let checked = unsafe { WinHttpSetOption(request.0, WINHTTP_OPTION_ENABLE_FEATURE, (&revocation as *const u32).cast(), size_of::<u32>() as u32) };
    if set == 0 || checked == 0 {
        return Err(NetError::Failed);
    }

    if unsafe { WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) } == 0 {
        return Err(NetError::Failed);
    }
    if unsafe { WinHttpReceiveResponse(request.0, null_mut()) } == 0 {
        return Err(NetError::Failed);
    }
    let status = status_of(&request)?;
    let location = location_of(&request);

    // The receive timeout is per read: the whole body is bounded here.
    let mut body = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        if start.elapsed() > req.timeout {
            return Err(NetError::Failed);
        }
        let mut read = 0u32;
        if unsafe { WinHttpReadData(request.0, buf.as_mut_ptr().cast(), CHUNK as u32, &mut read) } == 0 {
            return Err(NetError::Failed);
        }
        if read == 0 {
            break;
        }
        if body.len() + read as usize > req.limit {
            return Err(NetError::TooLarge);
        }
        body.extend_from_slice(&buf[..read as usize]);
    }
    Ok(Reply { status, location, body })
}

fn status_of(request: &Handle) -> Result<u16, NetError> {
    let mut code = 0u32;
    let mut size = size_of::<u32>() as u32;
    let level = WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER;
    let ok = unsafe { WinHttpQueryHeaders(request.0, level, null(), (&mut code as *mut u32).cast(), &mut size, null_mut()) };
    if ok == 0 {
        return Err(NetError::Failed);
    }
    u16::try_from(code).map_err(|_| NetError::Failed)
}

fn location_of(request: &Handle) -> Option<String> {
    let mut buf = vec![0u16; LOCATION_MAX];
    let mut size = (LOCATION_MAX * size_of::<u16>()) as u32;
    let ok = unsafe { WinHttpQueryHeaders(request.0, WINHTTP_QUERY_LOCATION, null(), buf.as_mut_ptr().cast(), &mut size, null_mut()) };
    if ok == 0 {
        return None;
    }
    let len = (size as usize / size_of::<u16>()).min(LOCATION_MAX);
    String::from_utf16(&buf[..len]).ok()
}
