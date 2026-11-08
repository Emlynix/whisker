//! One running instance. A second launch signals the first one to show
//! its window, then exits. The uninstaller signals it to quit.

use crate::win::wide;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::*;

const MUTEX_NAME: &str = "Local\\Whisker.SingleInstance";
const SHOW_EVENT: &str = "Local\\Whisker.Show";
const QUIT_EVENT: &str = "Local\\Whisker.Quit";
const RETRY_GAP: Duration = Duration::from_millis(100);
const WAIT_LIMIT: Duration = Duration::from_secs(5);
const WAIT_OBJECT_0: u32 = 0;

pub enum Instance {
    First,
    AlreadyRunning,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Hand over to a running instance.
    Now,
    /// Previous process is exiting (after install); wait for it.
    WaitForExit,
}

fn try_mutex() -> bool {
    let name = wide(MUTEX_NAME);
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        // Handle intentionally kept: the mutex lives as long as the process.
        return true;
    }
    unsafe { CloseHandle(h) };
    false
}

fn signal(event: &str) {
    let ev = wide(event);
    let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, ev.as_ptr()) };
    if h.is_null() {
        return;
    }
    unsafe {
        SetEvent(h);
        CloseHandle(h);
    }
}

/// Claims the instance; on `AlreadyRunning` the first instance was told to show.
pub fn claim(mode: Claim) -> Instance {
    let start = Instant::now();
    loop {
        if try_mutex() {
            return Instance::First;
        }
        if mode == Claim::Now || start.elapsed() > WAIT_LIMIT {
            break;
        }
        std::thread::sleep(RETRY_GAP);
    }

    signal(SHOW_EVENT);
    Instance::AlreadyRunning
}

/// Asks a running instance to exit (used by the uninstaller).
pub fn request_quit() {
    signal(QUIT_EVENT);
}

/// Waits until no instance holds the mutex, at most `limit`.
pub fn wait_closed(limit: Duration) {
    let name = wide(MUTEX_NAME);
    let start = Instant::now();
    while start.elapsed() < limit {
        let h = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        if h.is_null() {
            return;
        }
        unsafe { CloseHandle(h) };
        std::thread::sleep(RETRY_GAP);
    }
}

fn on_event(name: &str, f: impl Fn() + Send + 'static) {
    let ev = wide(name);
    let h: HANDLE = unsafe { CreateEventW(std::ptr::null(), 0, 0, ev.as_ptr()) };
    if h.is_null() {
        return;
    }

    let h = h as usize;
    std::thread::spawn(move || {
        while unsafe { WaitForSingleObject(h as HANDLE, INFINITE) } == WAIT_OBJECT_0 {
            f();
        }
    });
}

/// Calls `on_show` / `on_quit` when another process asks for it.
pub fn listen(on_show: impl Fn() + Send + 'static, on_quit: impl Fn() + Send + 'static) {
    on_event(SHOW_EVENT, on_show);
    on_event(QUIT_EVENT, on_quit);
}
