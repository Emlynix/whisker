//! Corsair "Bragi" mice (experimental, untested on hardware). Protocol
//! facts (constants only, own code) from:
//! - ckb-next, src/daemon/bragi_proto.h, bragi_common.c, device_bragi.c,
//!   dpi_bragi.c, usb.h (github.com/ckb-next/ckb-next, GPL-2.0)
//! - OpenRGB, Controllers/CorsairPeripheralV2Controller
//!   (gitlab.com/CalcProgrammer1/OpenRGB, GPL-2.0-or-later)
//!
//! Output report on the vendor collection (usage page FF42), padded to
//! the report size; the reply is the next input report:
//!
//! ```text
//! request:  00 | target | cmd | prop | 00 | value lo | value hi | 0…
//!           id   08 = device on cable / dongle itself
//!                09 = mouse paired to the dongle
//! reply:    00 | ?      | cmd | status | value (u32 LE) …
//!           id            echo  0 = ok, 1 invalid, 3 failed, 5 unsupported
//! e.g. get battery:  00 09 02 0F 00  →  00 09 02 00 52 03  = 850 ‰
//! ```
//!
//! Older NXP-protocol mice are not supported: Dark Core RGB / RGB SE
//! (non-Pro), M65 Pro / RGB Elite, Sabre, Nightsword, Ironclaw (wired),
//! Harpoon (wired), Glaive, Scimitar (non-Bragi), Katar. Katar Elite and
//! Sabre RGB Pro are left out: no source lists their PIDs.
//!
//! The mouse stays in hardware mode (ckb-next switches to software mode,
//! which reroutes buttons; we never do). Some firmware may refuse a
//! property there; the call then fails cleanly.


use super::{Ability, DpiRange, Ident, Pick, Reading};
use crate::hid_link::DeviceInfo;
use crate::hid_link::raw::{Channel, Interface};
use std::time::{Duration, Instant};

pub const VID: u16 = 0x1B1C;

const REPORT_ID: u8 = 0x00;
const VENDOR_PAGE: u16 = 0xFF42;
/// Report id, target, cmd, prop, pad, value lo, value hi.
const MIN_OUT_LEN: usize = 7;
/// Reply bytes before the value (after the report id).
const VALUE_AT: usize = 3;
const VALUE_LEN: usize = 4;
/// Large enough for any input report (OpenRGB reads 1025).
const READ_LEN: usize = 1025;
/// OpenRGB waits 50 ms; the radio hop gets more.
const REPLY_WAIT: Duration = Duration::from_millis(100);
const WRITE_WAIT: Duration = Duration::from_millis(100);
const DRAIN_WAIT: Duration = Duration::from_millis(3);
const DRAIN_MAX: usize = 8;
/// A sleeping mouse wakes up after a few tries (OpenRGB uses 8 × 120 ms).
const HELLO_TRIES: usize = 3;
const HELLO_PAUSE: Duration = Duration::from_millis(120);
const PROTOCOL: &str = "Corsair Bragi (experimental)";

/// Packet targets.
const TO_DEVICE: u8 = 0x08;
const TO_PAIRED: u8 = 0x09;

/// Commands.
const CMD_SET: u8 = 0x01;
const CMD_GET: u8 = 0x02;

/// Properties.
const PROP_RATE: u8 = 0x01;
const PROP_BATTERY: u8 = 0x0F;
const PROP_BATT_STATE: u8 = 0x10;
const PROP_PID: u8 = 0x12;
const PROP_APP_VER: u8 = 0x13;
const PROP_DPI_X: u8 = 0x21;
const PROP_DPI_Y: u8 = 0x22;
/// Untested in ckb-next (seen in iCUE logs only).
const PROP_MAX_RATE: u8 = 0x96;

const STATUS_OK: u8 = 0x00;
/// Battery level is in per mille.
const BATTERY_FULL: u32 = 1000;
const PER_MILLE_PER_PCT: u32 = 10;
const BATT_CHARGING: u32 = 1;

/// Report rate codes (Hz, code).
const RATES: [(u32, u8); 7] = [(125, 1), (250, 2), (500, 3), (1000, 4), (2000, 5), (4000, 6), (8000, 7)];
/// Used when the mouse does not report its maximum.
const DEFAULT_MAX_RATE: u32 = 1000;
const DPI_MIN: u32 = 100;
const DPI_STEP: u32 = 50;

/// How the HID device reaches the mouse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    /// The mouse itself (cable or wired-only model).
    Cable,
    /// A receiver sold with one mouse model.
    Dongle,
    /// A receiver that can pair with other devices: the paired PID must
    /// be a known mouse.
    Shared,
}

struct Model {
    name: &'static str,
    pid: u16,
    link: Link,
    dpi_max: u32,
}

const fn m(name: &'static str, pid: u16, link: Link, dpi_max: u32) -> Model {
    Model { name, pid, link, dpi_max }
}

/// PIDs from ckb-next `USES_BRAGI` and OpenRGB CorsairPeripheralV2.
/// 0x1B4B is left out: OpenRGB lists it, ckb-next calls it NXP.
const MODELS: [Model; 21] = [
    m("Dark Core RGB Pro", 0x1B80, Link::Cable, 18000),
    m("Dark Core RGB Pro", 0x1B81, Link::Dongle, 18000),
    m("Dark Core RGB Pro SE", 0x1B7E, Link::Cable, 18000),
    m("Dark Core RGB Pro SE", 0x1B7F, Link::Dongle, 18000),
    m("Harpoon RGB Wireless", 0x1B5E, Link::Cable, 10000),
    m("Ironclaw RGB Wireless", 0x1B4C, Link::Cable, 18000),
    m("Ironclaw RGB Wireless SE", 0x2B32, Link::Cable, 18000),
    m("Ironclaw RGB Wireless SE", 0x2B00, Link::Dongle, 18000),
    m("Katar Pro", 0x1B93, Link::Cable, 12400),
    m("Katar Pro V2", 0x1BBA, Link::Cable, 12400),
    m("Katar Pro XT", 0x1BAC, Link::Cable, 18000),
    m("M55 RGB Pro", 0x1B70, Link::Cable, 12400),
    m("M65 RGB Ultra", 0x1B9E, Link::Cable, 26000),
    m("M65 RGB Ultra Wireless", 0x1BB5, Link::Cable, 26000),
    m("M75", 0x1BF0, Link::Cable, 26000),
    m("M75 Wireless", 0x1BF1, Link::Cable, 26000),
    m("M75 Wireless", 0x1BDC, Link::Dongle, 26000),
    m("Scimitar Elite RGB", 0x1BE3, Link::Cable, 26000),
    // Slipstream receivers (ckb-next: Harpoon / Ironclaw dongles).
    m("Slipstream receiver", 0x1B65, Link::Shared, 0),
    m("Slipstream receiver", 0x1B66, Link::Shared, 0),
    m("Slipstream receiver", 0x1BA6, Link::Shared, 0),
];

/// Known models as (name, product id), for the catalogue.
pub fn listed() -> Vec<(&'static str, u16)> {
    MODELS.iter().map(|m| (m.name, m.pid)).collect()
}

fn model(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pid == pid)
}

/// Mouse model a shared receiver reports; receivers do not count.
fn paired_model(pid: u32) -> Option<&'static Model> {
    let pid = u16::try_from(pid).ok()?;
    model(pid).filter(|m| m.link != Link::Shared)
}

fn target(link: Link) -> u8 {
    match link {
        Link::Cable => TO_DEVICE,
        Link::Dongle | Link::Shared => TO_PAIRED,
    }
}

/// Output report of exactly `len` bytes. E.g. set DPI X 800 to the
/// paired mouse: `[00 09 01 21 00 20 03 0…]`. `None` if it does not fit.
fn frame(to: u8, cmd: u8, prop: u8, value: u16, len: usize) -> Option<Vec<u8>> {
    if len < MIN_OUT_LEN {
        return None;
    }
    let v = value.to_le_bytes();
    let mut r = vec![0u8; len];
    r[..MIN_OUT_LEN].copy_from_slice(&[REPORT_ID, to, cmd, prop, 0x00, v[0], v[1]]);
    Some(r)
}

/// Why a reply was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fail {
    /// Not an answer to this command (stale or notification).
    Other,
    /// Answer, but too short to hold the value.
    Short,
    /// Answer with an error status.
    Status(u8),
}

/// Value of an input report answering `cmd` (report id included, as
/// ReadFile returns it). E.g. `[00 08 02 00 E8 03 00 00]` → 1000.
fn parse_reply(r: &[u8], cmd: u8) -> Result<u32, Fail> {
    let Some(p) = r.get(1..) else {
        return Err(Fail::Other);
    };
    if p.get(1) != Some(&cmd) {
        return Err(Fail::Other);
    }
    let Some(&status) = p.get(2) else {
        return Err(Fail::Short);
    };
    if status != STATUS_OK {
        return Err(Fail::Status(status));
    }
    if cmd == CMD_SET {
        return Ok(0);
    }
    let Some(v) = p.get(VALUE_AT..VALUE_AT + VALUE_LEN) else {
        return Err(Fail::Short);
    };
    Ok(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
}

/// Battery % from per mille; `None` when out of range.
fn battery_pct(per_mille: u32) -> Option<u8> {
    if per_mille > BATTERY_FULL {
        return None;
    }
    Some(((per_mille + PER_MILLE_PER_PCT / 2) / PER_MILLE_PER_PCT) as u8)
}

fn rate_hz(code: u32) -> Option<u32> {
    RATES.iter().find(|(_, c)| *c as u32 == code).map(|(hz, _)| *hz)
}

/// Code of the nearest supported rate not above `max`.
fn rate_code(hz: u32, max: u32) -> Option<u8> {
    RATES.iter().filter(|(r, _)| *r <= max).min_by_key(|(r, _)| r.abs_diff(hz)).map(|(_, c)| *c)
}

/// Version from the app-version property: bytes `major minor patch`
/// (ckb-next byte-swaps the 24-bit value). E.g. 0x0A0305 → "v5.3.10".
fn version(v: u32) -> String {
    let b = v.to_le_bytes();
    format!("v{}.{}.{}", b[0], b[1], b[2])
}

/// Model to drive when `dev` answers the PID query with `pid`, so writes
/// never reach a keyboard or headset: a cable device must report itself,
/// a dongle its own mouse, a shared receiver any known mouse.
fn accept(dev: &'static Model, pid: u32) -> Option<&'static Model> {
    match dev.link {
        Link::Cable => (pid == dev.pid as u32).then_some(dev),
        Link::Dongle => paired_model(pid).filter(|m| m.name == dev.name).map(|_| dev),
        Link::Shared => paired_model(pid),
    }
}

pub struct Corsair {
    ch: Channel,
    model: &'static Model,
    to: u8,
    out_len: usize,
    max_rate: u32,
}

impl Corsair {
    /// Opens the vendor collection of a known PID and confirms the
    /// protocol with a read-only PID query before anything else. On a
    /// shared receiver the paired device must report a known mouse PID.
    pub fn open(pick: &Pick) -> Option<(Corsair, DeviceInfo)> {
        let cands: Vec<(Interface, &'static Model)> = pick.interfaces(VID)
            .into_iter()
            .filter(|i| i.usage_page == VENDOR_PAGE && i.output_len >= MIN_OUT_LEN)
            .filter_map(|i| model(i.info.pid).map(|m| (i, m)))
            .collect();

        for (iface, dev) in cands {
            let Some(ch) = Channel::open(&iface.path) else {
                continue;
            };
            let mut d = Corsair { ch, model: dev, to: target(dev.link), out_len: iface.output_len, max_rate: DEFAULT_MAX_RATE };
            let Some(pid) = d.hello() else {
                continue;
            };
            let Some(mouse) = accept(dev, pid) else {
                continue;
            };
            d.model = mouse;
            return Some((d, iface.info));
        }
        None
    }

    /// PID property, retried while a wireless mouse wakes up.
    fn hello(&self) -> Option<u32> {
        for _ in 0..HELLO_TRIES {
            if let Ok(pid) = self.call(CMD_GET, PROP_PID, 0) {
                return (pid != 0).then_some(pid);
            }
            std::thread::sleep(HELLO_PAUSE);
        }
        None
    }

    /// Drops input reports that arrived since the last exchange.
    fn drain(&self) {
        for _ in 0..DRAIN_MAX {
            if self.ch.read(READ_LEN, DRAIN_WAIT).is_none() {
                return;
            }
        }
    }

    /// One request and its reply; gives up after `REPLY_WAIT`.
    fn call(&self, cmd: u8, prop: u8, value: u16) -> Result<u32, Fail> {
        let Some(req) = frame(self.to, cmd, prop, value, self.out_len) else {
            return Err(Fail::Short);
        };
        self.drain();
        if !self.ch.write(&req, WRITE_WAIT) {
            return Err(Fail::Other);
        }

        let start = Instant::now();
        while let Some(left) = REPLY_WAIT.checked_sub(start.elapsed()) {
            let Some(r) = self.ch.read(READ_LEN, left) else {
                break;
            };
            match parse_reply(&r, cmd) {
                Err(Fail::Other) => continue,
                done => return done,
            }
        }
        Err(Fail::Other)
    }

    fn get(&self, prop: u8) -> Option<u32> {
        self.call(CMD_GET, prop, 0).ok()
    }

    fn set(&self, prop: u8, value: u16) -> bool {
        self.call(CMD_SET, prop, value).is_ok()
    }

    pub fn ident(&mut self) -> Ident {
        let mut id = Ident { name: format!("Corsair {}", self.model.name), protocol: PROTOCOL.into(), ..Ident::default() };
        if let Some(v) = self.get(PROP_APP_VER) {
            id.firmware = version(v);
        }
        id
    }

    pub fn ability(&mut self) -> Ability {
        if let Some(hz) = self.get(PROP_MAX_RATE).and_then(rate_hz) {
            self.max_rate = hz;
        }
        Ability {
            dpi: Some(DpiRange::Steps { min: DPI_MIN, max: self.model.dpi_max, step: DPI_STEP }),
            rates: RATES.iter().map(|(hz, _)| *hz).filter(|hz| *hz <= self.max_rate).collect(),
            battery: self.get(PROP_BATTERY).is_some(),
            stages: None,
        }
    }

    /// `None` when the mouse does not answer (asleep or off).
    pub fn read(&mut self) -> Option<Reading> {
        // Out-of-range DPI would skew the distance statistics.
        let dpi = self.get(PROP_DPI_X)?;
        let dpi = (DPI_MIN..=self.model.dpi_max).contains(&dpi).then_some(dpi);
        let mut r = Reading { dpi, rate_hz: self.get(PROP_RATE).and_then(rate_hz), ..Reading::default() };

        let Some(level) = self.get(PROP_BATTERY).and_then(battery_pct) else {
            return Some(r);
        };
        r.battery = Some(level);
        r.charging = self.get(PROP_BATT_STATE) == Some(BATT_CHARGING);
        Some(r)
    }

    /// Same value on both axes, clamped to the model range.
    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        let v = dpi.clamp(DPI_MIN, self.model.dpi_max.max(DPI_MIN)) as u16;
        self.set(PROP_DPI_X, v) && self.set(PROP_DPI_Y, v)
    }

    pub fn set_rate(&mut self, hz: u32) -> bool {
        let Some(code) = rate_code(hz, self.max_rate) else {
            return false;
        };
        self.set(PROP_RATE, code as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello_must_match_link() {
        let cable = model(0x1B80).unwrap();
        let dongle = model(0x1B81).unwrap();
        let shared = model(0x1B65).unwrap();
        assert!(accept(cable, 0x1B80).is_some());
        assert!(accept(cable, 0x1B9E).is_none());
        // Dongle: the paired device must be the receiver's own mouse.
        assert!(accept(dongle, 0x1B80).is_some());
        assert!(accept(dongle, 0x1BF0).is_none());
        assert!(accept(dongle, 0x1B3D).is_none());
        assert_eq!(accept(shared, 0x1BF0).map(|m| m.pid), Some(0x1BF0));
        assert!(accept(shared, 0x1B3D).is_none());
    }

    #[test]
    fn pids_are_unique() {
        let mut all: Vec<u16> = MODELS.iter().map(|m| m.pid).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
    }

    #[test]
    fn frame_layout() {
        let f = frame(TO_PAIRED, CMD_SET, PROP_DPI_X, 800, 65).unwrap();
        assert_eq!(f.len(), 65);
        assert_eq!(&f[..8], &[0x00, 0x09, 0x01, 0x21, 0x00, 0x20, 0x03, 0x00]);
        assert!(f[7..].iter().all(|&b| b == 0));
        let g = frame(TO_DEVICE, CMD_GET, PROP_PID, 0, 65).unwrap();
        assert_eq!(&g[..5], &[0x00, 0x08, 0x02, 0x12, 0x00]);
        assert_eq!(frame(TO_DEVICE, CMD_GET, PROP_PID, 0, 6), None);
    }

    #[test]
    fn reply_parsing() {
        assert_eq!(parse_reply(&[0x00, 0x08, 0x02, 0x00, 0xE8, 0x03, 0x00, 0x00], CMD_GET), Ok(1000));
        assert_eq!(parse_reply(&[0x00, 0x09, 0x02, 0x00, 0x7E, 0x1B, 0x00, 0x00, 0xFF], CMD_GET), Ok(0x1B7E));
        assert_eq!(parse_reply(&[0x00, 0x08, 0x01, 0x00], CMD_SET), Ok(0));
        assert_eq!(parse_reply(&[0x00, 0x08, 0x02, 0x05, 0, 0, 0, 0], CMD_GET), Err(Fail::Status(5)));
        assert_eq!(parse_reply(&[0x00, 0x08, 0x02, 0x00, 0xE8], CMD_GET), Err(Fail::Short));
        assert_eq!(parse_reply(&[0x00, 0x08, 0x02], CMD_GET), Err(Fail::Short));
        assert_eq!(parse_reply(&[0x00, 0x08, 0x01, 0x00], CMD_GET), Err(Fail::Other));
        assert_eq!(parse_reply(&[0x00], CMD_GET), Err(Fail::Other));
        assert_eq!(parse_reply(&[], CMD_GET), Err(Fail::Other));
    }

    #[test]
    fn battery_and_rates() {
        assert_eq!(battery_pct(850), Some(85));
        assert_eq!(battery_pct(1000), Some(100));
        assert_eq!(battery_pct(4), Some(0));
        assert_eq!(battery_pct(1001), None);
        assert_eq!(rate_hz(4), Some(1000));
        assert_eq!(rate_hz(0), None);
        assert_eq!(rate_code(1000, 1000), Some(4));
        assert_eq!(rate_code(8000, 1000), Some(4));
        assert_eq!(rate_code(300, 8000), Some(2));
        assert_eq!(rate_code(100, 8000), Some(1));
    }

    #[test]
    fn shared_receiver_needs_mouse() {
        assert_eq!(paired_model(0x1B7E).map(|m| m.name), Some("Dark Core RGB Pro SE"));
        assert!(paired_model(0x1BA6).is_none());
        assert!(paired_model(0x1B62).is_none());
        assert!(paired_model(0x1_1B7E).is_none());
        assert_eq!(target(Link::Cable), TO_DEVICE);
        assert_eq!(target(Link::Shared), TO_PAIRED);
    }

    #[test]
    fn firmware_version() {
        assert_eq!(version(0x0A0305), "v5.3.10");
    }
}
