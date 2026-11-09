//! Linux HID access over hidraw: enumerate `/sys/class/hidraw` (sysfs
//! only, nothing opened or sent), exchange reports through `/dev/hidrawN`
//! with `read`/`write` and the `HIDIOC*FEATURE` ioctls. Knows nothing
//! about any protocol.
//!
//! One hidraw node carries all top-level collections of a device; as on
//! macOS each collection becomes its own `Interface`, all with the node's
//! path. Bluetooth devices get their address appended in the shape
//! `hid_link` reads from Windows paths:
//!
//! ```text
//!   /dev/hidraw3                                  USB / receiver
//!   /dev/hidraw4#{00001812-linux}_b66b18d71e00    Bluetooth (HID_UNIQ)
//!
//!   /sys/class/hidraw/hidraw3/device -> …/1-2:1.1/0003:248A:5B2F.0007
//!        uevent (HID_ID, HID_NAME, HID_UNIQ), report_descriptor
//!     ..  USB interface: bInterfaceNumber
//!     ../.. USB device: manufacturer, product, serial, bcdDevice
//! ```
//!
//! Report layout follows Windows and hidapi: byte 0 is the report id, 0
//! for devices without ids. Opening needs read/write access to the node
//! (a udev `uaccess` rule); without it `Channel::open` fails cleanly.

use super::descriptor::collections;
use crate::hid_link::{DeviceInfo, Interface, ReportLayout, Transport, report_ids};
use crate::platform::Access;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CLASS_DIR: &str = "/sys/class/hidraw";
const DEV_DIR: &str = "/dev";
/// Marks a Bluetooth path for `hid_link::bt_address` (HID-over-GATT tag).
const BT_TAG: &str = "#{00001812-linux}_";
const USAGE_PAGE_DESKTOP: u16 = 0x01;
const USAGE_MOUSE: u16 = 0x02;
/// `BUS_*` from `linux/input.h`, the first field of `HID_ID`.
const BUS_USB: u16 = 0x03;
const BUS_BLUETOOTH: u16 = 0x05;
const ADDRESS_LEN: usize = 12;
/// Largest report hidraw hands out (`HID_MAX_BUFFER_SIZE`).
const MAX_REPORT: usize = 16384;
// Other architectures lay out `_IOC` differently.
#[cfg(not(any(target_arch = "x86_64", target_arch = "x86", target_arch = "aarch64", target_arch = "arm", target_arch = "riscv64")))]
compile_error!("hidraw ioctl numbers assume the asm-generic _IOC layout");
/// `_IOC` fields (asm-generic layout: x86, ARM, RISC-V).
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;
const IOC_DIR_SHIFT: u64 = 30;
const IOC_SIZE_SHIFT: u64 = 16;
const IOC_TYPE_SHIFT: u64 = 8;
const HID_IOC_TYPE: u64 = b'H' as u64;
const NR_SET_FEATURE: u64 = 0x06;
const NR_GET_FEATURE: u64 = 0x07;
/// udev rule granting the seat user the catalogue vendors' hidraw nodes
/// and mouse evdev nodes (`TAG+="uaccess"`).
const RULES: &str = include_str!("../../../bundle/linux/70-whisker.rules");
const RULES_NAME: &str = "70-whisker.rules";
const RULES_DIR: &str = "/etc/udev/rules.d";
/// Applies the new rule to devices already connected.
const RELOAD: &str = "sudo udevadm control --reload && sudo udevadm trigger --subsystem-match=hidraw --subsystem-match=input";
/// Debug builds: act as if the rule were missing (snapshots in WSL).
#[cfg(debug_assertions)]
const DENY_VAR: &str = "WHISKER_DENY_HID";

/// `_IOC(READ | WRITE, 'H', nr, len)`: the feature ioctls carry the length.
fn feature_ioctl(nr: u64, len: usize) -> u64 {
    ((IOC_READ | IOC_WRITE) << IOC_DIR_SHIFT) | ((len as u64) << IOC_SIZE_SHIFT) | (HID_IOC_TYPE << IOC_TYPE_SHIFT) | nr
}

/// sysfs facts of one hidraw node.
struct Node {
    /// `hidraw3`.
    name: String,
    /// The HID device folder (canonical).
    dir: PathBuf,
    bus: u16,
    vid: u16,
    pid: u16,
    /// `HID_NAME`: manufacturer and product as one string.
    hid_name: String,
    /// `HID_UNIQ`: serial, or the Bluetooth address.
    uniq: String,
}

/// `HID_ID=0003:0000248A:00005B2F` -> (bus, vid, pid).
fn parse_hid_id(v: &str) -> Option<(u16, u16, u16)> {
    let mut f = v.split(':').map(|x| u32::from_str_radix(x, 16).ok());
    Some((f.next()?? as u16, f.next()?? as u16, f.next()?? as u16))
}

fn uevent_value<'a>(uevent: &'a str, key: &str) -> Option<&'a str> {
    uevent.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
}

fn node(name: &str) -> Option<Node> {
    let dir = std::fs::canonicalize(Path::new(CLASS_DIR).join(name).join("device")).ok()?;
    let uevent = std::fs::read_to_string(dir.join("uevent")).ok()?;
    let (bus, vid, pid) = parse_hid_id(uevent_value(&uevent, "HID_ID")?)?;
    let text = |k: &str| uevent_value(&uevent, k).unwrap_or_default().trim().to_string();
    Some(Node { name: name.to_string(), dir, bus, vid, pid, hid_name: text("HID_NAME"), uniq: text("HID_UNIQ") })
}

/// All hidraw nodes, `hidraw0` first.
fn nodes() -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(CLASS_DIR) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    names.sort_by_key(|n| (n.len(), n.clone()));
    names.iter().filter_map(|n| node(n)).collect()
}

fn read_trimmed(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

/// `aa:bb:cc:dd:ee:ff` as `"AABBCCDDEEFF"`.
pub(super) fn address_of(uniq: &str) -> Option<String> {
    let hex: String = uniq.chars().filter(|c| !matches!(c, '-' | ':')).collect();
    (hex.len() == ADDRESS_LEN && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| hex.to_uppercase())
}

fn transport_of(bus: u16) -> Transport {
    match bus {
        BUS_USB => Transport::Usb,
        BUS_BLUETOOTH => Transport::Bluetooth,
        _ => Transport::Other,
    }
}

impl Node {
    fn path(&self) -> String {
        let dev = format!("{DEV_DIR}/{}", self.name);
        match address_of(&self.uniq).filter(|_| self.bus == BUS_BLUETOOTH) {
            Some(a) => format!("{dev}{BT_TAG}{}", a.to_lowercase()),
            None => dev,
        }
    }

    fn descriptor(&self) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join("report_descriptor")).ok()
    }

    /// USB interface and device folders above the HID device.
    fn usb_dirs(&self) -> Option<(&Path, &Path)> {
        if self.bus != BUS_USB {
            return None;
        }
        let interface = self.dir.parent()?;
        Some((interface, interface.parent()?))
    }

    fn info(&self) -> DeviceInfo {
        let usb = self.usb_dirs().map(|(_, d)| d);
        let attr = |k: &str| usb.and_then(|d| read_trimmed(&d.join(k)));
        let version = attr("bcdDevice").and_then(|v| u16::from_str_radix(&v, 16).ok()).unwrap_or(0);
        DeviceInfo {
            vid: self.vid,
            pid: self.pid,
            version,
            manufacturer: attr("manufacturer").unwrap_or_default(),
            product: attr("product").unwrap_or_else(|| self.hid_name.clone()),
            serial: attr("serial").unwrap_or_else(|| self.uniq.clone()),
            bt_address: None,
        }
    }

    /// Collections of the node, read from sysfs only.
    fn describe(&self) -> Vec<Interface> {
        let (path, info) = (self.path(), self.info());
        let desc = self.descriptor().unwrap_or_default();
        collections(&desc)
            .into_iter()
            .map(|c| Interface { path: path.clone(), info: info.clone(), usage_page: c.usage_page, usage: c.usage, output_len: c.output_len, feature_len: c.feature_len })
            .collect()
    }
}

/// Collections of devices with vendor id `vid` (descriptors only, nothing sent).
pub fn interfaces(vid: u16) -> Vec<Interface> {
    nodes().iter().filter(|n| n.vid == vid).flat_map(Node::describe).collect()
}

/// Mouse collections (generic desktop / mouse) of every vendor.
pub fn mice() -> Vec<Interface> {
    nodes().iter().flat_map(Node::describe).filter(|i| (i.usage_page, i.usage) == (USAGE_PAGE_DESKTOP, USAGE_MOUSE)).collect()
}

/// Read and write permission on `path` for this user (ACLs included).
fn may_open(path: &Path) -> std::io::Result<()> {
    let c = CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: valid NUL-terminated path; `access` only reads it.
    let r = unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) };
    if r == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Denied when nodes exist and every one refuses for lack of permission.
fn verdict(results: Vec<std::io::Result<()>>) -> Access {
    let refused = |r: &std::io::Result<()>| r.as_ref().is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied);
    if !results.is_empty() && results.iter().all(refused) {
        return Access::Denied;
    }
    Access::Granted
}

/// Whether this user may open the hidraw nodes of vendor `vid`
/// (checked with `access(2)`, nothing opened).
pub fn access(vid: u16) -> Access {
    #[cfg(debug_assertions)]
    if std::env::var_os(DENY_VAR).is_some() {
        return Access::Denied;
    }
    let results = nodes().iter().filter(|n| n.vid == vid).map(|n| may_open(&Path::new(DEV_DIR).join(&n.name))).collect();
    verdict(results)
}

/// `it's` -> `'it'\''s'`: one shell word whatever the path holds.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `sudo install` of the rules file at `src`, then reload and trigger.
fn install_command(src: &Path) -> String {
    let src = shell_quote(&src.to_string_lossy());
    format!("sudo install -m 644 {src} {RULES_DIR}/{RULES_NAME} && {RELOAD}")
}

/// Writes the bundled rule into `dir` and returns the command that
/// installs it; `None` when the file can't be written.
pub fn grant_command(dir: &Path) -> Option<String> {
    let path = dir.join(RULES_NAME);
    std::fs::create_dir_all(dir).ok()?;
    std::fs::write(&path, RULES).ok()?;
    Some(install_command(&path))
}

/// `hidraw3` from a path made by `Node::path`.
fn node_name(path: &str) -> Option<&str> {
    let dev = path.split('#').next()?;
    let name = dev.strip_prefix(DEV_DIR)?.strip_prefix('/')?;
    (name.starts_with("hidraw") && !name.contains('/')).then_some(name)
}

/// Report sizes, ids and raw descriptor of a device, from sysfs only.
/// All collections of one node share it (the longest of each kind);
/// `summary` stays empty (the descriptor carries the details).
pub fn layout(path: &str) -> Option<ReportLayout> {
    let n = node(node_name(path)?)?;
    let desc = n.descriptor()?;
    let tops = collections(&desc);
    let longest = |f: fn(&super::descriptor::Collection) -> usize| tops.iter().map(f).max().unwrap_or(0);
    let interface = n.usb_dirs().and_then(|(i, _)| read_trimmed(&i.join("bInterfaceNumber"))).and_then(|v| u8::from_str_radix(&v, 16).ok());

    Some(ReportLayout {
        transport: transport_of(n.bus),
        interface,
        input_len: longest(|c| c.input_len),
        output_len: longest(|c| c.output_len),
        feature_len: longest(|c| c.feature_len),
        report_ids: report_ids(&desc),
        descriptor: Some(desc),
        summary: Vec::new(),
    })
}

/// Open hidraw node.
pub struct Channel {
    file: File,
    /// The device uses report ids; otherwise input reports get a 0 prepended.
    numbered: bool,
}

impl Channel {
    pub fn open(path: &str) -> Option<Channel> {
        let name = node_name(path)?;
        let numbered = node(name).and_then(|n| n.descriptor()).is_some_and(|d| !report_ids(&d).is_empty());
        let file = File::options().read(true).write(true).custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC).open(Path::new(DEV_DIR).join(name)).ok()?;
        Some(Channel { file, numbered })
    }

    /// Waits until the node is ready for `events`, at most `timeout`.
    fn wait(&self, events: libc::c_short, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            let left = timeout.saturating_sub(start.elapsed());
            let mut p = libc::pollfd { fd: self.file.as_raw_fd(), events, revents: 0 };
            let ms = left.as_millis().min(i32::MAX as u128) as libc::c_int;
            // SAFETY: one valid pollfd.
            let n = unsafe { libc::poll(&mut p, 1, ms) };
            if n > 0 {
                return p.revents & events != 0;
            }
            let interrupted = n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted;
            if !interrupted || left.is_zero() {
                return false;
            }
        }
    }

    /// Sends an output report (first byte = report id, 0 = unnumbered;
    /// the kernel drops a 0 id byte).
    pub fn write(&self, report: &[u8], timeout: Duration) -> bool {
        if report.is_empty() || !self.wait(libc::POLLOUT, timeout) {
            return false;
        }
        (&self.file).write(report).is_ok_and(|n| n == report.len())
    }

    /// Next input report, or `None` after `timeout`.
    pub fn read(&self, len: usize, timeout: Duration) -> Option<Vec<u8>> {
        let start = Instant::now();
        let mut buf = vec![0u8; MAX_REPORT];
        loop {
            let left = timeout.checked_sub(start.elapsed())?;
            if !self.wait(libc::POLLIN, left) {
                return None;
            }
            // Another reader may have taken it: wait again.
            let Ok(n) = (&self.file).read(&mut buf) else {
                continue;
            };
            let mut r = Vec::with_capacity(n + 1);
            // Unnumbered reports: prepend id 0, as Windows does.
            if !self.numbered {
                r.push(0);
            }
            r.extend_from_slice(&buf[..n]);
            r.truncate(len);
            return Some(r);
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

    /// `HIDIOCSFEATURE`: `report[0]` is the id (0 = unnumbered).
    pub fn set_feature(&self, report: &[u8]) -> bool {
        if report.is_empty() || report.len() >= MAX_REPORT {
            return false;
        }
        // SAFETY: the kernel reads `report.len()` bytes from the pointer.
        let n = unsafe { libc::ioctl(self.file.as_raw_fd(), feature_ioctl(NR_SET_FEATURE, report.len()) as _, report.as_ptr()) };
        n >= 0
    }

    /// Feature report into `buf`; `buf[0]` must hold the report id.
    pub fn get_feature(&self, buf: &mut [u8]) -> bool {
        if buf.is_empty() || buf.len() >= MAX_REPORT {
            return false;
        }
        // SAFETY: the kernel writes at most `buf.len()` bytes.
        let n = unsafe { libc::ioctl(self.file.as_raw_fd(), feature_ioctl(NR_GET_FEATURE, buf.len()) as _, buf.as_mut_ptr()) };
        n >= 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hid_id_fields() {
        assert_eq!(parse_hid_id("0003:0000248A:00005B2F"), Some((0x03, 0x248A, 0x5B2F)));
        assert_eq!(parse_hid_id("0005:xyz"), None);
    }

    #[test]
    fn path_round_trip() {
        assert_eq!(node_name("/dev/hidraw3"), Some("hidraw3"));
        assert_eq!(node_name("/dev/hidraw4#{00001812-linux}_b66b18d71e00"), Some("hidraw4"));
        assert_eq!(node_name("/dev/sda"), None);
        assert_eq!(node_name("/dev/hidraw0/../sda"), None);
        assert_eq!(address_of("b6:6b:18:d7:1e:00").as_deref(), Some("B66B18D71E00"));
    }

    #[test]
    fn bluetooth_path_carries_the_address() {
        let n = Node { name: "hidraw4".into(), dir: PathBuf::new(), bus: BUS_BLUETOOTH, vid: 1, pid: 2, hid_name: String::new(), uniq: "b6:6b:18:d7:1e:00".into() };
        assert_eq!(n.path(), "/dev/hidraw4#{00001812-linux}_b66b18d71e00");
        let usb = Node { bus: BUS_USB, ..n };
        assert_eq!(usb.path(), "/dev/hidraw4");
    }

    #[test]
    fn feature_ioctl_numbers_match_the_kernel() {
        // HIDIOCSFEATURE(33) / HIDIOCGFEATURE(33) on x86-64.
        assert_eq!(feature_ioctl(NR_SET_FEATURE, 33), 0xC021_4806);
        assert_eq!(feature_ioctl(NR_GET_FEATURE, 33), 0xC021_4807);
    }

    #[test]
    fn denied_only_when_no_node_opens() {
        let denied = || Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(verdict(Vec::new()), Access::Granted);
        assert_eq!(verdict(vec![denied()]), Access::Denied);
        assert_eq!(verdict(vec![denied(), Ok(())]), Access::Granted);
        // Gone in between: not a permission problem.
        assert_eq!(verdict(vec![Err(std::io::Error::from(std::io::ErrorKind::NotFound))]), Access::Granted);
    }

    #[test]
    fn command_quotes_the_rules_path() {
        let c = install_command(Path::new("/home/o'neil/.config/Whisker/70-whisker.rules"));
        assert!(c.starts_with("sudo install -m 644 '/home/o'\\''neil/.config/Whisker/70-whisker.rules' /etc/udev/rules.d/70-whisker.rules && "), "{c}");
        assert!(c.ends_with(RELOAD), "{c}");
    }

    #[test]
    fn grant_writes_the_rules() {
        let dir = std::env::temp_dir().join(format!("whisker-udev-{}", std::process::id()));
        let cmd = grant_command(&dir).expect("command");
        assert_eq!(std::fs::read_to_string(dir.join(RULES_NAME)).ok().as_deref(), Some(RULES));
        assert!(cmd.contains(RULES_NAME));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_devices_is_not_an_error() {
        // WSL and containers have no hidraw nodes; this must not panic.
        let _ = mice();
        assert!(Channel::open("/dev/hidraw999").is_none());
        assert!(layout("/dev/hidraw999").is_none());
        assert_eq!(access(0xFFFF), Access::Granted);
    }
}
