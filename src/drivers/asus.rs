//! ASUS ROG / TUF mice (experimental, untested on hardware).
//!
//! Sources (facts and constants only, code written from scratch):
//! - libratbag `src/asus.c`, `src/asus.h`, `src/driver-asus.c` and
//!   `data/devices/asus-*.device` (github.com/libratbag/libratbag, MIT).
//! - G-Helper `app/Peripherals/Mouse/AsusMouse.cs`
//!   (github.com/seerge/g-helper, GPL-3.0): battery reply offsets only.
//!
//! One 64-byte output report (report id 0) on the vendor collection; the
//! mouse answers each with a 64-byte input report echoing the command.
//!
//! ```text
//! request:  00 | cmd0 cmd1 | params… | zero padding to 64 bytes
//! reply:       cmd0 cmd1 | data…              (FF AA = asleep / error)
//!
//! 12 00            profile:  [11] active DPI preset (1-based),
//!                            [13..=15] firmware build, minor, major
//! 12 04 00|02      settings: DPI codes LE at [4 + 2i] (or [4 + 4i] for
//!                            the X/Y variant), rate index at [4 + 2n]
//! 12 07            battery:  [4] percent, [9] charging
//! 51 31 ii 00 vv…  set field ii: 0..n-1 = DPI preset, n = report rate
//! 50 03            save to flash
//! DPI code = dpi / unit - 1   (unit 50, or 100 on "double DPI" models)
//! ```

use super::{Ability, DpiRange, Ident, Pick, Reading};
use crate::hid_link::raw::{Channel, Interface};
use std::time::Duration;

pub const VID: u16 = 0x0B05;

const REPORT_ID: u8 = 0x00;
const PACKET_LEN: usize = 64;
/// Output report with the leading report id.
const REPORT_LEN: usize = PACKET_LEN + 1;
/// Large enough for any input report of the collection.
const READ_LEN: usize = 1025;
/// G-Helper uses a 300 ms USB timeout.
const WRITE_WAIT: Duration = Duration::from_millis(300);
const REPLY_WAIT: Duration = Duration::from_millis(300);
const DRAIN_WAIT: Duration = Duration::from_millis(5);
const DRAIN_MAX: usize = 8;

const CMD_PROFILE: [u8; 2] = [0x12, 0x00];
const CMD_SETTINGS: [u8; 2] = [0x12, 0x04];
const CMD_BATTERY: [u8; 2] = [0x12, 0x07];
const CMD_SET: [u8; 2] = [0x51, 0x31];
const CMD_SAVE: [u8; 2] = [0x50, 0x03];
/// Reply code of an unknown request or a sleeping / absent mouse.
const STATUS_ERROR: [u8; 2] = [0xFF, 0xAA];

/// Settings query parameter: combined DPI or separate X/Y values.
const SETTINGS_PLAIN: u8 = 0x00;
const SETTINGS_XY: u8 = 0x02;
/// Report rate field index = number of DPI presets + this offset.
const FIELD_RATE: u8 = 0;

/// Reply offsets.
const AT_PRESET: usize = 11;
const AT_FW: usize = 13;
const AT_DPI: usize = 4;
const AT_LEVEL: usize = 4;
const AT_CHARGING: usize = 9;
const RATE_MASK: u8 = 0x07;
const FULL: u8 = 100;

/// Report rate per index.
const RATES: [u32; 4] = [125, 250, 500, 1000];
const VENDOR_PAGE: u16 = 0xFF00;
const PREFERRED_PAGE: u16 = 0xFF01;
const DESKTOP: u16 = 0x01;
const MOUSE: u16 = 0x02;
const PROTOCOL: &str = "ASUS (experimental)";

/// DPI encoding unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Fifty = 50,
    /// libratbag `DOUBLE_DPI` quirk: the code counts 100 DPI.
    Hundred = 100,
}

/// Where the settings reply keeps the DPI codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// `12 04 00`, two bytes per preset.
    Plain,
    /// `12 04 02`, X and Y (two bytes each) per preset; X is used.
    Xy,
}

/// How the mouse is attached; only wireless models report a battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Cable,
    Wireless,
}

struct Model {
    name: &'static str,
    /// Cable and dongle PIDs.
    pids: &'static [u16],
    min: u32,
    max: u32,
    step: u32,
    unit: Unit,
    /// DPI presets (2 or 4).
    presets: u8,
    layout: Layout,
    link: Link,
}

const fn m(name: &'static str, pids: &'static [u16], dpi: (u32, u32, u32), unit: Unit, presets: u8, layout: Layout, link: Link) -> Model {
    Model { name, pids, min: dpi.0, max: dpi.1, step: dpi.2, unit, presets, layout, link }
}

use Layout::{Plain, Xy};
use Link::{Cable, Wireless};
use Unit::{Fifty, Hundred};

/// From libratbag device files. Omni receiver mice (0x1ACE, Strix Impact
/// III Wireless) are left out: they need report id 3 and a model probe.
const MODELS: [Model; 21] = [
    m("ROG GX860 Buzzard", &[0x1816], (100, 8200, 100), Fifty, 2, Plain, Cable),
    m("ROG Chakram Core", &[0x1958], (100, 16000, 100), Hundred, 4, Plain, Cable),
    m("ROG Chakram", &[0x18E3, 0x18E5], (100, 16000, 100), Hundred, 4, Plain, Wireless),
    m("ROG Chakram X", &[0x1A18, 0x1A1A], (100, 36000, 100), Hundred, 4, Xy, Wireless),
    m("ROG Gladius II", &[0x1845], (100, 12000, 100), Fifty, 2, Plain, Cable),
    m("ROG Gladius II Origin", &[0x1877], (100, 12000, 100), Hundred, 2, Plain, Cable),
    m("ROG Gladius II Origin PNK LTD", &[0x18CD], (100, 12000, 100), Hundred, 2, Plain, Cable),
    m("ROG Harpe Wireless", &[0x1A92], (100, 36000, 50), Fifty, 4, Plain, Wireless),
    m("ROG Keris", &[0x195C], (100, 16000, 100), Hundred, 4, Plain, Cable),
    m("ROG Keris Wireless", &[0x195E, 0x1960], (100, 16000, 100), Hundred, 4, Plain, Wireless),
    m("ROG Keris Wireless AimPoint", &[0x1A66, 0x1A68], (100, 36000, 50), Fifty, 4, Plain, Wireless),
    m("ROG Pugio", &[0x1846], (100, 7200, 100), Fifty, 2, Plain, Cable),
    m("ROG Spatha X", &[0x1979, 0x1977], (100, 19000, 50), Fifty, 4, Xy, Wireless),
    m("ROG Strix Carry", &[0x18B4], (100, 7200, 100), Fifty, 2, Plain, Wireless),
    m("ROG Strix Impact", &[0x1847], (100, 5000, 100), Fifty, 2, Plain, Cable),
    m("ROG Strix Impact II", &[0x18E1], (200, 6200, 100), Fifty, 4, Plain, Cable),
    m("ROG Strix Impact II Wireless", &[0x1947, 0x1949], (100, 16000, 100), Fifty, 4, Plain, Wireless),
    m("ROG Strix Impact III", &[0x1A88], (100, 12000, 50), Fifty, 4, Plain, Cable),
    m("TUF Gaming M3 Gen II", &[0x1A9B], (100, 8000, 100), Fifty, 4, Plain, Cable),
    m("TUF Gaming M4 Air", &[0x1A03], (100, 16000, 100), Fifty, 4, Plain, Cable),
    m("TUF Gaming Mini WL", &[0x1C56, 0x1C57], (100, 12000, 100), Fifty, 4, Xy, Wireless),
];

fn model(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pids.contains(&pid))
}

/// Output report `[00, cmd0, cmd1, params…, 0…]` of 65 bytes; `None` if
/// the parameters do not fit.
fn frame(cmd: [u8; 2], params: &[u8]) -> Option<[u8; REPORT_LEN]> {
    let end = 1 + cmd.len() + params.len();
    if end > REPORT_LEN {
        return None;
    }
    let mut r = [0u8; REPORT_LEN];
    r[0] = REPORT_ID;
    r[1..3].copy_from_slice(&cmd);
    r[3..end].copy_from_slice(params);
    Some(r)
}

/// Why a reply was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// `FF AA`: asleep, absent or unsupported request.
    Device,
    /// No matching reply or the I/O failed.
    Link,
}

/// Matches an input report against `cmd`: the 64-byte payload when it
/// echoes the command, `Some(Err)` for the error code, `None` for an
/// unrelated report. Windows puts the report id first.
fn match_reply(r: &[u8], cmd: [u8; 2]) -> Option<Result<Vec<u8>, Fault>> {
    let p = match r.first() {
        Some(&REPORT_ID) => &r[1..],
        _ => r,
    };
    if p.len() < cmd.len() {
        return None;
    }
    if p[..2] == STATUS_ERROR {
        return Some(Err(Fault::Device));
    }
    if p[..2] != cmd {
        return None;
    }
    Some(Ok(p[..p.len().min(PACKET_LEN)].to_vec()))
}

/// Little endian u16 at `at`, if the reply is long enough.
fn le16(p: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*p.get(at)?, *p.get(at + 1)?]))
}

impl Model {
    fn range(&self) -> DpiRange {
        DpiRange::Steps { min: self.min, max: self.max, step: self.step }
    }

    /// Nearest supported DPI. E.g. step 100: 849 → 800, 850 → 900.
    fn snap(&self, dpi: u32) -> u32 {
        let off = dpi.clamp(self.min, self.max) - self.min;
        let step = self.step.max(1);
        let mut i = off / step;
        if (off % step) * 2 >= step {
            i += 1;
        }
        (self.min + i * step).min(self.max)
    }

    /// Wire code. E.g. 800 → 15 (unit 50) or 7 (unit 100).
    fn encode(&self, dpi: u32) -> u16 {
        let code = (self.snap(dpi) / self.unit as u32).saturating_sub(1);
        code.min(u16::MAX as u32) as u16
    }

    fn decode(&self, code: u16) -> u32 {
        (code as u32 + 1) * self.unit as u32
    }

    fn settings_param(&self) -> u8 {
        match self.layout {
            Plain => SETTINGS_PLAIN,
            Xy => SETTINGS_XY,
        }
    }

    /// DPI of preset `i` (0-based) from a settings reply.
    fn parse_dpi(&self, p: &[u8], i: u8) -> Option<u32> {
        if i >= self.presets {
            return None;
        }
        let width = match self.layout {
            Plain => 2,
            Xy => 4,
        };
        let code = le16(p, AT_DPI + i as usize * width)?;
        let dpi = self.decode(code);
        (self.min / 2..=self.max * 2).contains(&dpi).then_some(dpi)
    }

    /// Report rate from a plain settings reply.
    fn parse_rate(&self, p: &[u8]) -> Option<u32> {
        let raw = *p.get(AT_DPI + 2 * self.presets as usize)?;
        RATES.get((raw & RATE_MASK) as usize).copied()
    }

    /// `51 31 preset 00 lo hi`.
    fn dpi_params(&self, preset: u8, dpi: u32) -> [u8; 4] {
        let v = self.encode(dpi).to_le_bytes();
        [preset, 0x00, v[0], v[1]]
    }

    /// `51 31 n 00 index` for the nearest supported rate.
    fn rate_params(&self, hz: u32) -> [u8; 3] {
        let mut idx = 0u8;
        let mut best = u32::MAX;
        for (i, r) in RATES.iter().enumerate() {
            if r.abs_diff(hz) < best {
                best = r.abs_diff(hz);
                idx = i as u8;
            }
        }
        [self.presets + FIELD_RATE, 0x00, idx]
    }
}

/// Active DPI preset (0-based) from a profile reply; `None` when unknown.
fn parse_preset(p: &[u8]) -> Option<u8> {
    p.get(AT_PRESET)?.checked_sub(1)
}

/// Firmware `major.minor.build` from a profile reply.
fn parse_fw(p: &[u8]) -> Option<String> {
    let f = p.get(AT_FW..AT_FW + 3)?;
    Some(format!("v{}.{}.{}", f[2], f[1], f[0]))
}

/// Battery % and charging; `None` when asleep (0 % and not charging) or
/// out of range.
fn parse_battery(p: &[u8]) -> Option<(u8, bool)> {
    let level = *p.get(AT_LEVEL)?;
    let charging = *p.get(AT_CHARGING)? != 0;
    if level > FULL {
        return None;
    }
    if level == 0 && !charging {
        return None;
    }
    Some((level, charging))
}

pub struct Asus {
    ch: Channel,
    model: &'static Model,
}

impl Asus {
    /// Opens the vendor collection (64-byte output report) of a known
    /// model that also exposes a mouse collection, then checks that it
    /// answers the profile query. Nothing is sent to unknown PIDs.
    pub fn open(pick: &Pick) -> Option<(Asus, crate::hid_link::DeviceInfo)> {
        let all = pick.interfaces(VID);
        let is_mouse = |pid: u16| all.iter().any(|i| i.info.pid == pid && i.usage_page == DESKTOP && i.usage == MOUSE);
        let mut cands: Vec<(&Interface, &'static Model)> = all
            .iter()
            .filter(|i| i.usage_page >= VENDOR_PAGE && i.output_len == REPORT_LEN)
            .filter(|i| is_mouse(i.info.pid))
            .filter_map(|i| model(i.info.pid).map(|m| (i, m)))
            .collect();
        cands.sort_by_key(|(i, _)| i.usage_page != PREFERRED_PAGE);

        for (iface, model) in cands {
            let Some(ch) = Channel::open(&iface.path) else {
                continue;
            };
            let d = Asus { ch, model };
            // A sleeping wireless mouse answers FF AA: still ours.
            if d.query(CMD_PROFILE, &[]) == Err(Fault::Link) {
                continue;
            }
            return Some((d, iface.info.clone()));
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

    /// Sends one request and returns the reply payload echoing `cmd`.
    fn query(&self, cmd: [u8; 2], params: &[u8]) -> Result<Vec<u8>, Fault> {
        let r = frame(cmd, params).ok_or(Fault::Link)?;
        self.drain();
        if !self.ch.write(&r, WRITE_WAIT) {
            return Err(Fault::Link);
        }
        self.ch.read_until(READ_LEN, REPLY_WAIT, |b| match_reply(b, cmd)).unwrap_or(Err(Fault::Link))
    }

    fn settings(&self) -> Result<Vec<u8>, Fault> {
        self.query(CMD_SETTINGS, &[self.model.settings_param()])
    }

    fn save(&self) -> bool {
        self.query(CMD_SAVE, &[]).is_ok()
    }

    pub fn ident(&mut self) -> Ident {
        let mut id = Ident { name: format!("ASUS {}", self.model.name), protocol: PROTOCOL.into(), ..Ident::default() };
        if let Some(fw) = self.query(CMD_PROFILE, &[]).ok().and_then(|p| parse_fw(&p)) {
            id.firmware = fw;
        }
        id
    }

    pub fn ability(&mut self) -> Ability {
        Ability {
            dpi: Some(self.model.range()),
            rates: RATES.to_vec(),
            battery: self.model.link == Wireless,
            stages: None,
        }
    }

    /// DPI of the active preset, report rate and battery. `None` when the
    /// mouse does not answer (asleep or off).
    pub fn read(&mut self) -> Option<Reading> {
        let mut r = Reading::default();

        if self.model.link == Wireless {
            let p = self.query(CMD_BATTERY, &[]).ok()?;
            let (level, charging) = parse_battery(&p)?;
            r.battery = Some(level);
            r.charging = charging;
        }

        let preset = self.query(CMD_PROFILE, &[]).ok()?;
        let active = parse_preset(&preset).unwrap_or(0);
        let s = self.settings().ok()?;
        r.dpi = self.model.parse_dpi(&s, active);

        // The X/Y reply has no rate; ask for the plain layout.
        r.rate_hz = match self.model.layout {
            Plain => self.model.parse_rate(&s),
            Xy => self.query(CMD_SETTINGS, &[SETTINGS_PLAIN]).ok().and_then(|p| self.model.parse_rate(&p)),
        };
        Some(r)
    }

    /// Writes `dpi` (nearest supported) to the active preset and saves.
    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        let Ok(p) = self.query(CMD_PROFILE, &[]) else {
            return false;
        };
        // Unknown preset: writing and saving preset 0 would change one the
        // user did not pick.
        let Some(active) = parse_preset(&p).filter(|&i| i < self.model.presets) else {
            return false;
        };
        self.query(CMD_SET, &self.model.dpi_params(active, dpi)).is_ok() && self.save()
    }

    pub fn set_rate(&mut self, hz: u32) -> bool {
        self.query(CMD_SET, &self.model.rate_params(hz)).is_ok() && self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(pid: u16) -> &'static Model {
        model(pid).unwrap()
    }

    /// Settings payload with DPI codes at the layout's offsets.
    fn settings_reply(layout: Layout, codes: &[u16], rate: u8) -> Vec<u8> {
        let mut p = vec![0u8; PACKET_LEN];
        p[..2].copy_from_slice(&CMD_SETTINGS);
        let width = if layout == Plain { 2 } else { 4 };
        for (i, c) in codes.iter().enumerate() {
            let at = AT_DPI + i * width;
            p[at..at + 2].copy_from_slice(&c.to_le_bytes());
        }
        // The X/Y reply carries no rate.
        if layout == Plain {
            p[AT_DPI + 2 * codes.len()] = rate;
        }
        p
    }

    #[test]
    fn pids_are_unique() {
        let mut all: Vec<u16> = MODELS.iter().flat_map(|m| m.pids.iter().copied()).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
        assert!(MODELS.iter().all(|m| m.presets == 2 || m.presets == 4));
        assert!(MODELS.iter().all(|m| m.step % m.unit as u32 == 0));
    }

    /// Every row is in data/mice.toml with the same DPI and rates.
    #[test]
    fn catalogue_agrees() {
        use crate::devices::{Family, listed_as};
        for m in &MODELS {
            for &pid in m.pids {
                let c = listed_as(VID, Some(pid), m.name).unwrap_or_else(|| panic!("{} {pid:04X} not listed", m.name));
                assert_eq!((c.family, c.dpi.as_ref(), c.rates.as_slice()), (Family::Asus, Some(&m.range()), &RATES[..]), "{}", m.name);
                assert!(c.buttons.is_some_and(|b| b > 0), "{}", m.name);
            }
        }
    }

    #[test]
    fn omni_receiver_is_not_supported() {
        assert!(model(0x1ACE).is_none());
        assert!(model(0x1AD7).is_none());
    }

    #[test]
    fn frame_layout() {
        let f = frame(CMD_SET, &[0x01, 0x00, 0x0F, 0x00]).unwrap();
        assert_eq!(f.len(), 65);
        assert_eq!(&f[..7], &[0x00, 0x51, 0x31, 0x01, 0x00, 0x0F, 0x00]);
        assert!(f[7..].iter().all(|&b| b == 0));
        assert_eq!(&frame(CMD_SAVE, &[]).unwrap()[..3], &[0x00, 0x50, 0x03]);
        assert!(frame(CMD_SET, &[0u8; 63]).is_none());
        assert!(frame(CMD_SET, &[0u8; 62]).is_some());
    }

    #[test]
    fn dpi_encoding() {
        // Pugio (unit 50): 800 → 15; Keris (double DPI): 800 → 7.
        assert_eq!(md(0x1846).dpi_params(0, 800), [0, 0, 15, 0]);
        assert_eq!(md(0x195C).dpi_params(3, 800), [3, 0, 7, 0]);
        // Chakram X max 36000 → 359 = 0x0167.
        assert_eq!(md(0x1A18).dpi_params(1, 99999), [1, 0, 0x67, 0x01]);
        // Clamp and snap.
        assert_eq!(md(0x1847).encode(10), 1);
        assert_eq!(md(0x1846).snap(849), 800);
        assert_eq!(md(0x1846).snap(850), 900);
        assert_eq!(md(0x1A92).snap(825), 850);
        assert_eq!(md(0x18E1).snap(100), 200);
    }

    #[test]
    fn rate_encoding() {
        // 2-preset model: field 2; 4-preset model: field 4.
        assert_eq!(md(0x1846).rate_params(1000), [2, 0, 3]);
        assert_eq!(md(0x195C).rate_params(125), [4, 0, 0]);
        assert_eq!(md(0x195C).rate_params(300), [4, 0, 1]);
        assert_eq!(md(0x195C).rate_params(8000), [4, 0, 3]);
    }

    #[test]
    fn settings_parsing() {
        let keris = md(0x195C);
        let p = settings_reply(Plain, &[3, 7, 15, 31], 0x03);
        assert_eq!(keris.parse_dpi(&p, 1), Some(800));
        assert_eq!(keris.parse_dpi(&p, 3), Some(3200));
        assert_eq!(keris.parse_dpi(&p, 4), None);
        assert_eq!(keris.parse_rate(&p), Some(1000));

        let pugio = md(0x1846);
        let p = settings_reply(Plain, &[15, 31], 0x02);
        assert_eq!(pugio.parse_dpi(&p, 0), Some(800));
        assert_eq!(pugio.parse_rate(&p), Some(500));

        // Rate index outside the table, short reply, absurd DPI.
        let p = settings_reply(Plain, &[15, 31], 0x05);
        assert_eq!(pugio.parse_rate(&p), None);
        assert_eq!(pugio.parse_dpi(&p[..5], 0), None);
        assert_eq!(pugio.parse_dpi(&settings_reply(Plain, &[0xFFFF, 0], 0), 0), None);
    }

    #[test]
    fn xy_parsing() {
        let cx = md(0x1A18);
        let mut p = settings_reply(Xy, &[7, 15], 0);
        p[AT_DPI + 2..AT_DPI + 4].copy_from_slice(&99u16.to_le_bytes());
        assert_eq!(cx.parse_dpi(&p, 0), Some(800));
        assert_eq!(cx.parse_dpi(&p, 1), Some(1600));
    }

    #[test]
    fn reply_matching() {
        let mut r = vec![0u8; REPORT_LEN];
        r[1..3].copy_from_slice(&CMD_BATTERY);
        r[1 + AT_LEVEL] = 80;
        let p = match_reply(&r, CMD_BATTERY).unwrap().unwrap();
        assert_eq!(p.len(), PACKET_LEN);
        assert_eq!(parse_battery(&p), Some((80, false)));
        assert_eq!(match_reply(&r, CMD_PROFILE), None);
        assert_eq!(match_reply(&[0x00, 0xFF, 0xAA, 0, 0], CMD_PROFILE), Some(Err(Fault::Device)));
        assert_eq!(match_reply(&[0x00, 0x12], CMD_PROFILE), None);
        assert_eq!(match_reply(&[], CMD_PROFILE), None);
    }

    #[test]
    fn battery_parsing() {
        let mut p = vec![0u8; PACKET_LEN];
        p[AT_LEVEL] = 55;
        p[AT_CHARGING] = 1;
        assert_eq!(parse_battery(&p), Some((55, true)));
        p[AT_LEVEL] = 0;
        p[AT_CHARGING] = 0;
        assert_eq!(parse_battery(&p), None);
        p[AT_LEVEL] = 101;
        assert_eq!(parse_battery(&p), None);
        assert_eq!(parse_battery(&p[..6]), None);
    }

    #[test]
    fn profile_parsing() {
        let mut p = vec![0u8; PACKET_LEN];
        p[AT_PRESET] = 2;
        p[AT_FW..AT_FW + 3].copy_from_slice(&[3, 2, 1]);
        assert_eq!(parse_preset(&p), Some(1));
        assert_eq!(parse_fw(&p).as_deref(), Some("v1.2.3"));
        p[AT_PRESET] = 0;
        assert_eq!(parse_preset(&p), None);
        assert_eq!(parse_fw(&p[..14]), None);
    }
}
