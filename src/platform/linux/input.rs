//! evdev hook for usage statistics: every event of every mouse, even
//! while the window is unfocused, on its own thread. Needs read access to
//! `/dev/input/event*` (the `input` group); without it no values arrive.
//!
//! A mouse is an event node with relative X/Y and a left button
//! (`capabilities/rel`, `capabilities/key` in sysfs). Events up to
//! `SYN_REPORT` form one `MouseInput`; the device id is the node number.
//! New nodes are picked up by a rescan every few seconds.

use crate::stats::MouseInput;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

const CLASS_DIR: &str = "/sys/class/input";
const DEV_DIR: &str = "/dev/input";
const NODE_PREFIX: &str = "event";
const RESCAN: Duration = Duration::from_secs(5);
const POLL_MS: libc::c_int = 1000;
/// `input-event-codes.h`.
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const SYN_REPORT: u16 = 0;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_WHEEL: u16 = 0x08;
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;
const BTN_FORWARD: u16 = 0x115;
const BTN_BACK: u16 = 0x116;
/// Key event value of a press (0 = release, 2 = auto-repeat).
const PRESSED: i32 = 1;
const WORD_BITS: usize = u64::BITS as usize;
/// Events read per `read` call.
const BATCH: usize = 64;

static HANDLER: OnceLock<fn(&MouseInput)> = OnceLock::new();

/// Starts the listener thread; `on` runs on it for each mouse report.
pub fn start(on: fn(&MouseInput)) {
    let _ = HANDLER.set(on);
    std::thread::Builder::new().name("evdev".into()).spawn(listen).expect("spawn evdev thread");
}

/// The event node `dev` belongs to a device with vendor id `vid`.
pub fn has_vendor(dev: usize, vid: u16) -> bool {
    let path = format!("{CLASS_DIR}/{NODE_PREFIX}{dev}/device/id/vendor");
    std::fs::read_to_string(path).ok().and_then(|v| u16::from_str_radix(v.trim(), 16).ok()) == Some(vid)
}

/// Bit `n` of a sysfs capability mask: hex words, most significant first.
fn has_bit(mask: &str, n: u16) -> bool {
    let n = n as usize;
    let Some(word) = mask.split_whitespace().rev().nth(n / WORD_BITS) else {
        return false;
    };
    u64::from_str_radix(word, 16).is_ok_and(|w| w >> (n % WORD_BITS) & 1 == 1)
}

/// Relative X/Y and a left button: a mouse, not a touchpad or keyboard.
fn is_mouse(node: usize) -> bool {
    let cap = |kind: &str| std::fs::read_to_string(format!("{CLASS_DIR}/{NODE_PREFIX}{node}/device/capabilities/{kind}")).unwrap_or_default();
    let rel = cap("rel");
    has_bit(&rel, REL_X) && has_bit(&rel, REL_Y) && has_bit(&cap("key"), BTN_LEFT)
}

/// Numbers of the mouse event nodes present now.
fn mouse_nodes() -> Vec<usize> {
    let Ok(entries) = std::fs::read_dir(CLASS_DIR) else {
        return Vec::new();
    };
    let numbers = entries.filter_map(|e| e.ok()?.file_name().to_str()?.strip_prefix(NODE_PREFIX)?.parse().ok());
    numbers.filter(|&n| is_mouse(n)).collect()
}

fn open(node: usize) -> Option<File> {
    File::options().read(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(format!("{DEV_DIR}/{NODE_PREFIX}{node}")).ok()
}

fn blank(dev: usize) -> MouseInput {
    MouseInput { dev, left: false, right: false, middle: false, side: false, wheel: false, dx: 0, dy: 0 }
}

/// One open mouse and the report being assembled.
struct Device {
    file: File,
    pending: MouseInput,
    /// Events arrived since the last `SYN_REPORT`.
    dirty: bool,
}

impl Device {
    fn apply(&mut self, e: &libc::input_event) {
        let m = &mut self.pending;
        let down = e.value == PRESSED;
        match (e.type_, e.code) {
            (EV_SYN, SYN_REPORT) => {
                if self.dirty {
                    emit(m);
                }
                *m = blank(m.dev);
                self.dirty = false;
                return;
            }
            (EV_KEY, BTN_LEFT) => m.left |= down,
            (EV_KEY, BTN_RIGHT) => m.right |= down,
            (EV_KEY, BTN_MIDDLE) => m.middle |= down,
            (EV_KEY, BTN_SIDE | BTN_EXTRA | BTN_FORWARD | BTN_BACK) => m.side |= down,
            (EV_REL, REL_X) => m.dx += e.value,
            (EV_REL, REL_Y) => m.dy += e.value,
            (EV_REL, REL_WHEEL) => m.wheel |= e.value != 0,
            _ => return,
        }
        self.dirty = true;
    }

    /// Reads what is queued; false when the device is gone.
    fn drain(&mut self) -> bool {
        let size = size_of::<libc::input_event>();
        let mut buf = vec![0u8; size * BATCH];
        loop {
            let n = match self.file.read(&mut buf) {
                Ok(n) => n,
                Err(e) => return e.kind() == std::io::ErrorKind::WouldBlock,
            };
            if n == 0 {
                return false;
            }
            for chunk in buf[..n].chunks_exact(size) {
                // SAFETY: the kernel writes whole `input_event` structs;
                // read_unaligned copes with the byte buffer's alignment.
                let e = unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<libc::input_event>()) };
                self.apply(&e);
            }
        }
    }
}

fn emit(m: &MouseInput) {
    if let Some(on) = HANDLER.get() {
        on(m);
    }
}

fn listen() {
    let mut devices: HashMap<usize, Device> = HashMap::new();
    let mut last_scan: Option<Instant> = None;
    loop {
        if last_scan.is_none_or(|t| t.elapsed() >= RESCAN) {
            last_scan = Some(Instant::now());
            for n in mouse_nodes() {
                if devices.contains_key(&n) {
                    continue;
                }
                if let Some(file) = open(n) {
                    devices.insert(n, Device { file, pending: blank(n), dirty: false });
                }
            }
        }
        if devices.is_empty() {
            std::thread::sleep(RESCAN);
            continue;
        }

        let nodes: Vec<usize> = devices.keys().copied().collect();
        let mut fds: Vec<libc::pollfd> = nodes.iter().map(|n| libc::pollfd { fd: devices[n].file.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
        // SAFETY: `fds` holds `fds.len()` valid entries.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, POLL_MS) };
        if ready <= 0 {
            continue;
        }
        for (n, p) in nodes.iter().zip(&fds) {
            if p.revents == 0 {
                continue;
            }
            let alive = p.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0 && devices.get_mut(n).is_some_and(Device::drain);
            if !alive {
                devices.remove(n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_bits() {
        // rel "103" = REL_X | REL_Y | REL_WHEEL; key: BTN_LEFT is bit 272,
        // in the 5th word from the right (272 / 64 = 4).
        assert!(has_bit("103", REL_X) && has_bit("103", REL_Y) && has_bit("103", REL_WHEEL));
        assert!(!has_bit("103", 2));
        let key = "70000 0 0 0 0";
        assert!(has_bit(key, BTN_LEFT) && has_bit(key, BTN_MIDDLE));
        assert!(!has_bit("0", BTN_LEFT));
    }

    #[test]
    fn events_merge_into_one_report() {
        static SEEN: std::sync::Mutex<Vec<(bool, i32, i32)>> = std::sync::Mutex::new(Vec::new());
        let _ = HANDLER.set(|m| SEEN.lock().unwrap().push((m.left, m.dx, m.dy)));
        let Some(file) = File::open("/dev/null").ok() else {
            return;
        };
        let mut d = Device { file, pending: blank(7), dirty: false };
        let ev = |type_, code, value| libc::input_event { time: libc::timeval { tv_sec: 0, tv_usec: 0 }, type_, code, value };
        for e in [ev(EV_REL, REL_X, 3), ev(EV_REL, REL_Y, -2), ev(EV_KEY, BTN_LEFT, PRESSED), ev(EV_SYN, SYN_REPORT, 0), ev(EV_SYN, SYN_REPORT, 0)] {
            d.apply(&e);
        }
        // Only the first SYN_REPORT had events.
        assert_eq!(*SEEN.lock().unwrap(), [(true, 3, -2)]);
    }
}
