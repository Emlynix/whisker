//! Roccat Kone mice (experimental, never run on hardware). Feature-report
//! protocol shared by the Kone+ generation, read-modify-write of the
//! active profile's settings block.
//!
//! Sources (facts and constants only, no code copied):
//! - libratbag `src/driver-roccat.c`, `driver-roccat-kone-pure.c`,
//!   `driver-roccat-kone-emp.c`, `data/devices/roccat-*.device` (MIT).
//! - Linux `drivers/hid/hid-roccat-common.c`, `hid-roccat-koneplus.h`
//!   (v3.6 struct layout), `hid-roccat-konepure.c`, `hid-ids.h` (GPL-2.0).
//! - roccat-tools (tux-rampage/roccat-tools mirror): `koneplus.h`,
//!   `konepure.h`, `konextd_rmp_conversions.c`, `roccat_control.c`
//!   (GPL-2.0+).
//!
//! ```text
//!  report 04 control (3 B)   SET 04 pp 80   select settings of profile pp
//!                            GET 04 -> 04 ss ..  ss: 1 ok, 3 busy, else error
//!  report 05 profile (3 B)   GET 05 -> 05 03 pp  active profile 0..4
//!  report 06 settings        GET 06 -> block, SET 06 <- block, then GET 04
//!  report 09 info    (6 B)   GET 09 -> 09 06 fw  firmware 123 -> "1.23"
//!
//!  settings block, LL = block size, CC = 16-bit LE sum of bytes 0..LL-2
//!  Kone+/XTD/Pure: 06 LL pp .. .. .. mask x0..x4 stage y0..y4 .. rate .. CC
//!  Kone EMP:       06 LL pp .. .. .. mask x0..x4 y0..y4 stage rate .. CC
//! ```
//!
//! Not supported (other layouts, or not documented well enough): Kone
//! (2CED, older protocol), Kone Pure Optical (2DB4), Kone Pure Military,
//! Kone AIMO, Kova/Kova+, Kain, Burst, Tyon, Savu, Lua, Pyra.


use super::{Ability, DpiRange, Ident, Reading};
use crate::hid_link::DeviceInfo;
use crate::hid_link::raw::{Channel, Interface, interfaces};
use std::time::{Duration, Instant};

pub const VID: u16 = 0x1E7D;

const ID_CONTROL: u8 = 0x04;
const ID_PROFILE: u8 = 0x05;
const ID_SETTINGS: u8 = 0x06;
const ID_INFO: u8 = 0x09;
const CONTROL_LEN: usize = 3;
const PROFILE_LEN: usize = 3;
const INFO_LEN: usize = 6;
/// Control request: the next report 06 read is this profile's settings.
const REQ_SETTINGS: u8 = 0x80;

const PROFILES: u8 = 5;
const STAGES: usize = 5;
const STAGE_MASK: u8 = 0x1F;
const CHECKSUM_LEN: usize = 2;

const AT_SIZE: usize = 1;
const AT_PROFILE: usize = 2;
const AT_MASK: usize = 6;
const AT_X: usize = 7;

/// Polling rate code is the index (roccat-tools `RoccatPollingRate`).
const RATES: [u32; 4] = [125, 250, 500, 1000];

/// roccat-tools `koneplus_check_write`: 110 ms before the first poll,
/// then a busy wait growing by 110 ms. Bounded here (kernel loops forever).
const FIRST_WAIT: Duration = Duration::from_millis(110);
const BUSY_WAIT: Duration = Duration::from_millis(110);
const READY_TRIES: u32 = 8;
const READY_LIMIT: Duration = Duration::from_secs(3);

const DESKTOP: u16 = 0x01;
const MOUSE: u16 = 0x02;
const PROTOCOL: &str = "Roccat (experimental)";

/// Control report status (kernel `ROCCAT_COMMON_CONTROL_STATUS_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Critical,
    Ok,
    Invalid,
    Busy,
    CriticalNew,
}

impl Status {
    fn from_raw(raw: u8) -> Option<Status> {
        match raw {
            0 => Some(Status::Critical),
            1 => Some(Status::Ok),
            2 => Some(Status::Invalid),
            3 => Some(Status::Busy),
            4 => Some(Status::CriticalNew),
            _ => None,
        }
    }
}

/// Byte order of the stage table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// Kone+, XTD, Pure: `x0..x4 stage y0..y4 unknown rate`.
    Plus,
    /// Kone EMP: `x0..x4 y0..y4 stage rate`.
    Emp,
}

impl Layout {
    fn y_at(self) -> usize {
        match self {
            Layout::Plus => 13,
            Layout::Emp => 12,
        }
    }

    fn stage_at(self) -> usize {
        match self {
            Layout::Plus => 12,
            Layout::Emp => 17,
        }
    }

    fn rate_at(self) -> usize {
        match self {
            Layout::Plus => 19,
            Layout::Emp => 18,
        }
    }
}

/// DPI = raw * unit + base, writable in `step`s between `min` and `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    unit: u32,
    base: u32,
    min: u32,
    max: u32,
    step: u32,
}

impl Scale {
    /// Raw stage value to DPI, e.g. XTD 16 -> 800.
    pub fn dpi(&self, raw: u8) -> u32 {
        raw as u32 * self.unit + self.base
    }

    /// DPI to raw value: clamped, rounded down to the step.
    pub fn raw(&self, dpi: u32) -> u8 {
        let dpi = dpi.clamp(self.min, self.max);
        let dpi = self.min + (dpi - self.min) / self.step * self.step;
        ((dpi - self.base) / self.unit) as u8
    }

    fn valid(&self, raw: u8) -> bool {
        (self.min..=self.max).contains(&self.dpi(raw))
    }
}

/// Kone+: raw 1..60 = 100..6000 (kernel, roccat-tools).
const SCALE_PLUS: Scale = Scale { unit: 100, base: 0, min: 100, max: 6000, step: 100 };
/// XTD/Pure: raw = rmp << 2, rmp 1..41 = 200..8200 (roccat-tools).
const SCALE_XTD: Scale = Scale { unit: 50, base: 0, min: 200, max: 8200, step: 200 };
/// EMP: raw 0..0x77 = 100..12000 (libratbag).
const SCALE_EMP: Scale = Scale { unit: 100, base: 100, min: 100, max: 12000, step: 100 };

/// Whether settings may be written back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    ReadWrite,
    /// Layout only inferred (libratbag lists the PID, no other source).
    ReadOnly,
}

pub struct Model {
    pub pid: u16,
    pub name: &'static str,
    /// Settings block size with id and checksum (also byte 1 of the block).
    len: usize,
    layout: Layout,
    scale: Scale,
    access: Access,
}

pub const MODELS: &[Model] = &[
    Model { pid: 0x2D51, name: "Kone+", len: 43, layout: Layout::Plus, scale: SCALE_PLUS, access: Access::ReadWrite },
    Model { pid: 0x2E22, name: "Kone XTD", len: 43, layout: Layout::Plus, scale: SCALE_XTD, access: Access::ReadWrite },
    Model { pid: 0x2DBE, name: "Kone Pure", len: 31, layout: Layout::Plus, scale: SCALE_XTD, access: Access::ReadWrite },
    Model { pid: 0x2DC2, name: "Kone Pure (variant)", len: 31, layout: Layout::Plus, scale: SCALE_XTD, access: Access::ReadOnly },
    Model { pid: 0x2DCB, name: "Kone Pure (variant)", len: 31, layout: Layout::Plus, scale: SCALE_XTD, access: Access::ReadOnly },
    Model { pid: 0x2E24, name: "Kone EMP", len: 41, layout: Layout::Emp, scale: SCALE_EMP, access: Access::ReadWrite },
];

/// The values of a validated settings block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Active DPI stage, 0..4.
    stage: usize,
    pub dpi: u32,
    pub rate_hz: u32,
}

/// 16-bit byte sum of everything before the checksum.
pub fn checksum(b: &[u8]) -> u16 {
    let end = b.len().saturating_sub(CHECKSUM_LEN);
    b[..end].iter().fold(0u16, |s, &x| s.wrapping_add(x as u16))
}

/// True when the trailing little-endian checksum matches.
pub fn sum_ok(b: &[u8]) -> bool {
    let n = b.len();
    if n <= CHECKSUM_LEN {
        return false;
    }
    u16::from_le_bytes([b[n - 2], b[n - 1]]) == checksum(b)
}

/// Writes the checksum into the last two bytes.
pub fn seal(b: &mut [u8]) {
    let n = b.len();
    if n <= CHECKSUM_LEN {
        return;
    }
    let [lo, hi] = checksum(b).to_le_bytes();
    b[n - 2] = lo;
    b[n - 1] = hi;
}

/// `04 pp rr`: select what the next settings read returns.
pub fn control_frame(profile: u8, request: u8) -> [u8; CONTROL_LEN] {
    [ID_CONTROL, profile, request]
}

/// Status of a `GET 04` reply.
pub fn parse_status(r: &[u8]) -> Option<Status> {
    if r.len() < CONTROL_LEN || r[0] != ID_CONTROL {
        return None;
    }
    Status::from_raw(r[1])
}

/// Active profile from `05 03 pp`.
pub fn parse_profile(r: &[u8]) -> Option<u8> {
    if r.len() < PROFILE_LEN || r[0] != ID_PROFILE || r[1] != PROFILE_LEN as u8 || r[2] >= PROFILES {
        return None;
    }
    Some(r[2])
}

/// Firmware from `09 06 fw ..`, e.g. 123 -> "1.23".
pub fn parse_fw(r: &[u8]) -> Option<String> {
    if r.len() < INFO_LEN || r[0] != ID_INFO || r[1] != INFO_LEN as u8 {
        return None;
    }
    Some(format!("{}.{:02}", r[2] / 100, r[2] % 100))
}

/// Validates a settings block of `profile`: ids, size, checksum, stage
/// mask, active stage and rate code.
///
/// ```text
/// XTD: 06 2B 00 .. .. .. 1F 08 10 20 40 A4 01 08 10 20 40 A4 .. 03 .. CC CC
///                  mask ┘ x0 x1 ...     stage 1 = 0x10 * 50 = 800   1000 Hz
/// ```
fn decode(b: &[u8], m: &Model, profile: u8) -> Option<Settings> {
    if b.len() != m.len || b[0] != ID_SETTINGS || b[AT_SIZE] as usize != m.len || b[AT_PROFILE] != profile {
        return None;
    }
    if !sum_ok(b) {
        return None;
    }
    let mask = b[AT_MASK];
    let stage = b[m.layout.stage_at()] as usize;
    if mask & !STAGE_MASK != 0 || stage >= STAGES || mask & (1 << stage) == 0 {
        return None;
    }
    let rate_hz = *RATES.get(b[m.layout.rate_at()] as usize)?;
    let (x, y) = (b[AT_X + stage], b[m.layout.y_at() + stage]);
    if !m.scale.valid(x) || !m.scale.valid(y) {
        return None;
    }
    Some(Settings { stage, dpi: m.scale.dpi(x), rate_hz })
}

/// Sets X and Y of the active stage and reseals.
fn encode_dpi(b: &mut [u8], m: &Model, s: &Settings, dpi: u32) {
    let raw = m.scale.raw(dpi);
    b[AT_X + s.stage] = raw;
    b[m.layout.y_at() + s.stage] = raw;
    seal(b);
}

/// Sets the nearest supported rate and reseals.
fn encode_rate(b: &mut [u8], m: &Model, hz: u32) {
    let Some(code) = (0..RATES.len()).min_by_key(|&i| RATES[i].abs_diff(hz)) else {
        return;
    };
    b[m.layout.rate_at()] = code as u8;
    seal(b);
}

/// USB interface token of a collection path (`..&mi_00&col02..` -> "00").
fn usb_iface(path: &str) -> Option<String> {
    const TAG: &str = "&mi_";
    let p = path.to_lowercase();
    let at = p.find(TAG)? + TAG.len();
    p.get(at..at + 2).map(str::to_string)
}

fn is_mouse(i: &Interface) -> bool {
    i.usage_page == DESKTOP && i.usage == MOUSE
}

/// Collections that may carry the settings reports: on the same USB
/// interface as a mouse collection, not a desktop collection, large
/// enough for the block. Nothing for a PID without a mouse collection.
fn candidates<'a>(ours: &[&'a Interface], m: &Model) -> Vec<&'a Interface> {
    let mice: Vec<Option<String>> = ours.iter().filter(|i| is_mouse(i)).map(|i| usb_iface(&i.path)).collect();
    ours.iter()
        .copied()
        .filter(|i| i.usage_page != DESKTOP && i.feature_len >= m.len)
        .filter(|i| mice.contains(&usb_iface(&i.path)))
        .collect()
}

pub struct Roccat {
    ch: Channel,
    /// Collection feature length; Windows wants buffers this long.
    feat_len: usize,
    model: &'static Model,
    fw: String,
}

impl Roccat {
    /// Finds a listed PID and the collection that answers the control and
    /// profile reports and returns a valid settings block. The only write
    /// before that is the settings select (`04 pp 80`).
    pub fn open() -> Option<(Roccat, DeviceInfo)> {
        let all = interfaces(VID);
        for model in MODELS {
            let ours: Vec<&Interface> = all.iter().filter(|i| i.info.pid == model.pid).collect();
            for i in candidates(&ours, model) {
                let Some(ch) = Channel::open(&i.path) else {
                    continue;
                };
                let mut d = Roccat { ch, feat_len: i.feature_len, model, fw: String::new() };
                if !d.probe() {
                    continue;
                }
                d.fw = d.get(ID_INFO, INFO_LEN).and_then(|r| parse_fw(&r)).unwrap_or_default();
                return Some((d, i.info.clone()));
            }
        }
        None
    }

    /// Read-only checks first, then one validated settings read.
    fn probe(&self) -> bool {
        if self.get(ID_CONTROL, CONTROL_LEN).and_then(|r| parse_status(&r)).is_none() {
            return false;
        }
        self.fetch().is_some()
    }

    /// Feature report `id`, first `len` bytes, id byte checked.
    fn get(&self, id: u8, len: usize) -> Option<Vec<u8>> {
        let mut b = vec![0u8; self.feat_len.max(len)];
        b[0] = id;
        if !self.ch.get_feature(&mut b) || b[0] != id {
            return None;
        }
        b.truncate(len);
        Some(b)
    }

    /// Sends `frame` zero-padded to the collection's feature length.
    fn set(&self, frame: &[u8]) -> bool {
        let mut b = frame.to_vec();
        b.resize(self.feat_len.max(frame.len()), 0);
        self.ch.set_feature(&b)
    }

    /// Polls the control report until OK; any error status or the time
    /// limit fails.
    fn wait_ready(&self) -> bool {
        let start = Instant::now();
        let mut wait = BUSY_WAIT;
        std::thread::sleep(FIRST_WAIT);

        for _ in 0..READY_TRIES {
            let Some(s) = self.get(ID_CONTROL, CONTROL_LEN).and_then(|r| parse_status(&r)) else {
                return false;
            };
            if s == Status::Ok {
                return true;
            }
            if s != Status::Busy || start.elapsed() + wait > READY_LIMIT {
                return false;
            }
            std::thread::sleep(wait);
            wait += BUSY_WAIT;
        }
        false
    }

    /// Selects and reads the active profile's settings.
    fn fetch(&self) -> Option<(Vec<u8>, Settings)> {
        let profile = self.get(ID_PROFILE, PROFILE_LEN).and_then(|r| parse_profile(&r))?;
        if !self.set(&control_frame(profile, REQ_SETTINGS)) || !self.wait_ready() {
            return None;
        }
        let b = self.get(ID_SETTINGS, self.model.len)?;
        let s = decode(&b, self.model, profile)?;
        Some((b, s))
    }

    /// Two equal validated reads, so a glitched block is never written.
    fn fetch_stable(&self) -> Option<(Vec<u8>, Settings)> {
        if self.model.access != Access::ReadWrite {
            return None;
        }
        let (a, s) = self.fetch()?;
        let (b, _) = self.fetch()?;
        (a == b).then_some((a, s))
    }

    /// Writes a sealed block, waits for the mouse and reads it back.
    fn store(&self, b: &[u8]) -> bool {
        if !sum_ok(b) || !self.set(b) || !self.wait_ready() {
            return false;
        }
        self.fetch().is_some_and(|(r, _)| r == b)
    }

    pub fn ident(&mut self) -> Ident {
        Ident { name: format!("Roccat {}", self.model.name), firmware: self.fw.clone(), protocol: PROTOCOL.into(), ..Ident::default() }
    }

    /// Read-only models offer no DPI or rates.
    pub fn ability(&mut self) -> Ability {
        if self.model.access != Access::ReadWrite {
            return Ability::default();
        }
        let sc = self.model.scale;
        Ability {
            dpi: Some(DpiRange::Steps { min: sc.min, max: sc.max, step: sc.step }),
            rates: RATES.to_vec(),
            ..Ability::default()
        }
    }

    /// DPI of the active stage and report rate; `None` when the block
    /// cannot be read or does not validate.
    pub fn read(&mut self) -> Option<Reading> {
        let (_, s) = self.fetch()?;
        Some(Reading { dpi: Some(s.dpi), rate_hz: Some(s.rate_hz), ..Reading::default() })
    }

    /// DPI of the active stage, read-modify-write.
    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        let Some((mut b, s)) = self.fetch_stable() else {
            return false;
        };
        encode_dpi(&mut b, self.model, &s, dpi);
        self.store(&b)
    }

    /// Report rate (nearest supported), read-modify-write.
    pub fn set_rate(&mut self, hz: u32) -> bool {
        let Some((mut b, _)) = self.fetch_stable() else {
            return false;
        };
        encode_rate(&mut b, self.model, hz);
        self.store(&b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(pid: u16) -> &'static Model {
        MODELS.iter().find(|m| m.pid == pid).unwrap()
    }

    /// Kone XTD block, profile 2: stages 400/800/1600/3200/8200, stage 1
    /// active, 1000 Hz.
    fn xtd() -> Vec<u8> {
        let m = model(0x2E22);
        let mut b = vec![0u8; m.len];
        b[0] = ID_SETTINGS;
        b[AT_SIZE] = m.len as u8;
        b[AT_PROFILE] = 2;
        b[AT_MASK] = 0x1F;
        b[AT_X..AT_X + 5].copy_from_slice(&[8, 16, 32, 64, 164]);
        b[12] = 1;
        b[13..18].copy_from_slice(&[8, 16, 32, 64, 164]);
        b[19] = 3;
        b[25] = 0x55;
        seal(&mut b);
        b
    }

    /// Kone EMP block, profile 0: stages 100..500, stage 4 active, 500 Hz.
    fn emp() -> Vec<u8> {
        let m = model(0x2E24);
        let mut b = vec![0u8; m.len];
        b[0] = ID_SETTINGS;
        b[AT_SIZE] = m.len as u8;
        b[AT_MASK] = 0x1F;
        b[AT_X..AT_X + 5].copy_from_slice(&[0, 1, 2, 3, 4]);
        b[12..17].copy_from_slice(&[0, 1, 2, 3, 4]);
        b[17] = 4;
        b[18] = 2;
        seal(&mut b);
        b
    }

    #[test]
    fn checksum_sum() {
        assert_eq!(checksum(&[0x06, 0x03, 0xFF, 0x00, 0x00]), 0x0108);
        assert_eq!(checksum(&[0xFF; 300]), (0xFF * 298) as u16);
        assert_eq!(checksum(&[1]), 0);
        assert!(!sum_ok(&[0, 0]));
        let mut b = vec![0x06, 0x05, 0x01, 0, 0];
        seal(&mut b);
        assert_eq!(&b[3..], &[0x0C, 0x00]);
        assert!(sum_ok(&b));
        b[2] = 2;
        assert!(!sum_ok(&b));
    }

    #[test]
    fn frames() {
        assert_eq!(control_frame(3, REQ_SETTINGS), [0x04, 0x03, 0x80]);
        assert_eq!(parse_status(&[0x04, 0x01, 0x80]), Some(Status::Ok));
        assert_eq!(parse_status(&[0x04, 0x03, 0x00]), Some(Status::Busy));
        assert_eq!(parse_status(&[0x04, 0x09, 0x00]), None);
        assert_eq!(parse_status(&[0x05, 0x01, 0x00]), None);
        assert_eq!(parse_status(&[0x04, 0x01]), None);
        assert_eq!(parse_profile(&[0x05, 0x03, 0x04]), Some(4));
        assert_eq!(parse_profile(&[0x05, 0x03, 0x05]), None);
        assert_eq!(parse_profile(&[0x05, 0x04, 0x00]), None);
        assert_eq!(parse_fw(&[0x09, 0x06, 123, 0, 0, 0]).as_deref(), Some("1.23"));
        assert_eq!(parse_fw(&[0x09, 0x06, 105]).as_deref(), None);
    }

    #[test]
    fn decode_xtd() {
        let s = decode(&xtd(), model(0x2E22), 2).unwrap();
        assert_eq!((s.stage, s.dpi, s.rate_hz), (1, 800, 1000));
    }

    #[test]
    fn decode_emp() {
        let s = decode(&emp(), model(0x2E24), 0).unwrap();
        assert_eq!((s.stage, s.dpi, s.rate_hz), (4, 500, 500));
    }

    #[test]
    fn decode_rejects() {
        let m = model(0x2E22);
        let check = |f: &dyn Fn(&mut Vec<u8>), reseal: bool| {
            let mut b = xtd();
            f(&mut b);
            if reseal {
                seal(&mut b);
            }
            decode(&b, m, 2)
        };
        assert!(check(&|b| b[30] ^= 1, false).is_none());
        assert!(check(&|b| b[0] = 0x07, true).is_none());
        assert!(check(&|b| b[AT_SIZE] = 31, true).is_none());
        assert!(check(&|b| b[AT_PROFILE] = 1, true).is_none());
        assert!(check(&|b| b[AT_MASK] = 0x3F, true).is_none());
        assert!(check(&|b| b[AT_MASK] = 0x1D, true).is_none());
        assert!(check(&|b| b[12] = 5, true).is_none());
        assert!(check(&|b| b[19] = 4, true).is_none());
        assert!(check(&|b| b[AT_X + 1] = 3, true).is_none());
        assert!(check(&|b| b[14] = 165, true).is_none());
        assert!(check(&|b| b.truncate(31), true).is_none());
        assert!(decode(&xtd(), model(0x2DBE), 2).is_none());
        assert!(decode(&[], m, 2).is_none());
    }

    #[test]
    fn scales() {
        assert_eq!(SCALE_XTD.raw(800), 16);
        assert_eq!(SCALE_XTD.raw(850), 16);
        assert_eq!(SCALE_XTD.raw(50_000), 164);
        assert_eq!(SCALE_XTD.raw(0), 4);
        assert_eq!(SCALE_PLUS.raw(1650), 16);
        assert_eq!(SCALE_EMP.raw(100), 0);
        assert_eq!(SCALE_EMP.raw(12_000), 0x77);
        assert_eq!(SCALE_EMP.dpi(0x77), 12_000);
    }

    #[test]
    fn dpi_changes_active_stage_only() {
        let m = model(0x2E22);
        let mut b = xtd();
        let s = decode(&b, m, 2).unwrap();
        encode_dpi(&mut b, m, &s, 1200);
        let mut want = xtd();
        want[AT_X + 1] = 24;
        want[14] = 24;
        seal(&mut want);
        assert_eq!(b, want);
        assert_eq!(decode(&b, m, 2).unwrap().dpi, 1200);
    }

    #[test]
    fn emp_dpi_and_rate() {
        let m = model(0x2E24);
        let mut b = emp();
        let s = decode(&b, m, 0).unwrap();
        encode_dpi(&mut b, m, &s, 3200);
        assert_eq!((b[AT_X + 4], b[16]), (31, 31));
        encode_rate(&mut b, m, 1000);
        assert_eq!(b[18], 3);
        let s = decode(&b, m, 0).unwrap();
        assert_eq!((s.dpi, s.rate_hz), (3200, 1000));
    }

    #[test]
    fn rate_nearest() {
        let m = model(0x2E22);
        let mut b = xtd();
        encode_rate(&mut b, m, 300);
        assert_eq!(b[19], 1);
        assert!(sum_ok(&b));
    }

    #[test]
    fn iface_token() {
        assert_eq!(usb_iface(r"\\?\HID#VID_1E7D&PID_2E22&MI_00&Col02#x").as_deref(), Some("00"));
        assert_eq!(usb_iface(r"\\?\hid#vid_1e7d&pid_2e22#x"), None);
        assert_eq!(usb_iface("&mi_"), None);
    }

    #[test]
    fn models_consistent() {
        for (i, m) in MODELS.iter().enumerate() {
            assert!(MODELS[i + 1..].iter().all(|o| o.pid != m.pid));
            let last = (m.layout.y_at() + STAGES).max(m.layout.stage_at() + 1).max(m.layout.rate_at() + 1);
            assert!(m.len >= last + CHECKSUM_LEN && m.len <= u8::MAX as usize);
            let sc = m.scale;
            assert!(sc.valid(sc.raw(sc.min)) && sc.valid(sc.raw(sc.max)));
        }
    }
}
