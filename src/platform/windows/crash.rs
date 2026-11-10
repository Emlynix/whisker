//! Crash notes: an exception nobody handled is written into the log
//! before Windows ends the process, e.g.
//! `CRASH exception C0000005 at exe+1A2B3C`, or `at 7FFA12345678` for an
//! address outside the exe (a system DLL).

use crate::log::{CRASH_MARK, crash_raw};
use core::fmt::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

/// Let Windows go on with its own crash handling (error report, exit).
const CONTINUE_SEARCH: i32 = 0;
const LINE_MAX: usize = 160;
/// PE header: offset of the field holding the NT headers' offset.
const PE_LFANEW: usize = 0x3C;
/// NT headers: offset of `OptionalHeader.SizeOfImage` (PE32 and PE32+).
const PE_SIZE_OF_IMAGE: usize = 0x50;

/// Where the exe is mapped, read once: the filter must not ask the
/// loader (its lock may be what the crashing thread holds).
static EXE_BASE: AtomicUsize = AtomicUsize::new(0);
static EXE_SIZE: AtomicUsize = AtomicUsize::new(0);

/// A line built on the stack: the heap may be what broke.
struct Line {
    buf: [u8; LINE_MAX],
    len: usize,
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = LINE_MAX - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// Logs unhandled exceptions from now on.
pub fn install() {
    // SAFETY: the handle of the exe is its mapped image, whose headers
    // stay readable for the life of the process.
    unsafe {
        let base = GetModuleHandleW(std::ptr::null()) as usize;
        if base != 0 {
            let headers = base + ((base + PE_LFANEW) as *const u32).read_unaligned() as usize;
            let size = ((headers + PE_SIZE_OF_IMAGE) as *const u32).read_unaligned() as usize;
            EXE_BASE.store(base, Ordering::Relaxed);
            EXE_SIZE.store(size, Ordering::Relaxed);
        }
        SetUnhandledExceptionFilter(Some(filter));
    }
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    // SAFETY: Windows passes valid pointers for the failing thread.
    let rec = unsafe { (*info).ExceptionRecord };
    if rec.is_null() {
        return CONTINUE_SEARCH;
    }
    let (code, addr) = unsafe { ((*rec).ExceptionCode as u32, (*rec).ExceptionAddress as usize) };

    let mut line = Line { buf: [0; LINE_MAX], len: 0 };
    let _ = write!(line, "{CRASH_MARK}exception {code:08X} at ");
    place(&mut line, addr);
    let _ = line.write_str("\n");
    // The newline must survive a cut-off line.
    line.buf[LINE_MAX - 1] = b'\n';

    crash_raw(&line.buf[..line.len]);
    CONTINUE_SEARCH
}

/// `exe+1A2B3C` for an address in the exe, the bare address otherwise.
fn place(line: &mut Line, addr: usize) {
    let (base, size) = (EXE_BASE.load(Ordering::Relaxed), EXE_SIZE.load(Ordering::Relaxed));
    let inside = addr >= base && addr - base < size;
    let _ = if inside { write!(line, "exe+{:X}", addr - base) } else { write!(line, "{addr:X}") };
}
