//! Sinowealth-based OEM mice (Glorious, Genesis, Marvo, Mad Dog, ...).
//! Protocol as documented by libratbag `driver-sinowealth.c` (MIT),
//! collection split on Windows as used by OpenRGB. Experimental: never
//! run on hardware.
//!
//! ```text
//!  command collection (feature 05, 6 B)   config collection (feature 04|06, 520 B)
//!
//!  firmware: SET 05 01 ..  GET 05 -> 05 01 'V' '1' '0' '3'
//!  read:     SET 05 11 ..  ─────────> GET 04 -> config block
//!  write:                             SET 04 11 00 LL <rest as read>
//!                                     LL = config size - 8
//!
//!  block: 0 id | 1 cmd | 3 LL (0 on read) | 9 sensor
//!         10 flags:4 rate:4 | 11 active:4 count:4 | 12 disabled slots
//!         13..29 eight DPI slots (one byte, or X,Y pairs when flag XY)
//! ```
//!
//! Vendor and product ids are reused by unrelated devices (OpenRGB
//! bricked keyboards that way), so a mouse is only driven when its
//! firmware version is in [`MODELS`].

use super::{Ability, DpiRange, Ident, Reading};
use crate::hid_link::DeviceInfo;
use crate::hid_link::raw::{Channel, Interface, interfaces};
use std::time::{Duration, Instant};

/// Sinowealth's own id, and the one Glorious moved to (Model O Eternal).
pub const VIDS: [u16; 2] = [0x258A, 0x3794];

const CMD_ID: u8 = 0x05;
const CMD_LEN: usize = 6;
/// Short and long config report ids (libratbag `is_long`).
const CONFIG_IDS: [u8; 2] = [0x04, 0x06];
/// Config report with its id byte.
pub const BLOCK_LEN: usize = 520;
/// Bytes before the payload; `config_write` counts what follows.
const HEADER_LEN: usize = 8;

const CMD_FIRMWARE: u8 = 0x01;
const CMD_PROFILE: u8 = 0x02;
/// Config command of profile 1, 2, 3.
const CMD_CONFIG: [u8; 3] = [0x11, 0x21, 0x31];

const FW_AT: usize = 2;
const FW_LEN: usize = 4;

const AT_WRITE: usize = 3;
const AT_SENSOR: usize = 9;
const AT_RATE: usize = 10;
const AT_STAGE: usize = 11;
const AT_DISABLED: usize = 12;
const AT_DPI: usize = 13;
const SLOTS: usize = 8;
const LOW: u8 = 0x0F;
const FLAG_XY: u8 = 0x80;

/// Raw rate code (low nibble of byte 10) to Hz.
const RATES: [(u32, u8); 4] = [(125, 0x1), (250, 0x2), (500, 0x3), (1000, 0x4)];
const DPI_MIN: u32 = 100;
const DPI_STEP: u32 = 100;

const VENDOR_PAGE: u16 = 0xFF00;
const DESKTOP: u16 = 0x01;
const MOUSE: u16 = 0x02;
/// OpenRGB: a read right after a write returns zeros from 0x2C on.
const SETTLE: Duration = Duration::from_millis(100);
const TAIL_AT: usize = 0x2C;

/// Sensor id at byte 9 of the block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sensor {
    Pmw3360,
    Pmw3212,
    Pmw3327,
    Pmw3389,
}

impl Sensor {
    fn from_raw(raw: u8) -> Option<Sensor> {
        match raw {
            0x06 => Some(Sensor::Pmw3360),
            0x08 => Some(Sensor::Pmw3212),
            0x0E => Some(Sensor::Pmw3327),
            0x0F => Some(Sensor::Pmw3389),
            _ => None,
        }
    }

    pub fn max_dpi(self) -> u32 {
        match self {
            Sensor::Pmw3360 => 12000,
            Sensor::Pmw3212 => 7200,
            Sensor::Pmw3327 => 10200,
            Sensor::Pmw3389 => 16000,
        }
    }

    /// PMW3360/3327 store `dpi / 100 - 1`, the others `dpi / 100`.
    fn bias(self) -> u32 {
        match self {
            Sensor::Pmw3360 | Sensor::Pmw3327 => 1,
            Sensor::Pmw3212 | Sensor::Pmw3389 => 0,
        }
    }

    /// Raw slot value to DPI, e.g. 3360: 7 -> 800.
    pub fn dpi(self, raw: u8) -> u32 {
        (raw as u32 + self.bias()) * DPI_STEP
    }

    /// DPI to raw slot value, clamped and rounded down to the step.
    pub fn raw(self, dpi: u32) -> u8 {
        let dpi = dpi.clamp(DPI_MIN, self.max_dpi());
        (dpi / DPI_STEP - self.bias()) as u8
    }
}

/// One supported firmware (libratbag `data/devices/*.device`).
pub struct Model {
    pub vid: u16,
    pub pid: u16,
    /// Firmware version string the mouse reports.
    pub fw: &'static str,
    pub name: &'static str,
    /// Config size to write back; `None` = unknown, read-only.
    write_len: Option<usize>,
    profiles: u8,
}

const SHORT: Option<usize> = Some(123);
const NORMAL: Option<usize> = Some(131);

/// Sizes: 131 from gloriousctl / OpenRGB (Glorious, DM5) and the 0029
/// device file, 123 from the 0051 and 1007 device files.
pub const MODELS: &[Model] = &[
    Model { vid: 0x258A, pid: 0x0036, fw: "V103", name: "Glorious Model O/O-", write_len: NORMAL, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0033, fw: "V102", name: "Glorious Model D/D-", write_len: NORMAL, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0027, fw: "V102", name: "Glorious Model O (old firmware)", write_len: NORMAL, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0027, fw: "3106", name: "Dream Machines DM5 Blink", write_len: NORMAL, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0027, fw: "3110", name: "Genesis Xenon 770", write_len: None, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0027, fw: "V161", name: "G-Wolves Hati HT-M Wired", write_len: None, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0029, fw: "V127", name: "Machenike M620", write_len: NORMAL, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0029, fw: "V287", name: "Mad Dog GM905", write_len: NORMAL, profiles: 1 },
    Model { vid: 0x258A, pid: 0x0051, fw: "2642", name: "T-Dagger Imperial T-TGM310", write_len: SHORT, profiles: 2 },
    Model { vid: 0x258A, pid: 0x1007, fw: "9677", name: "Marvo Scorpion G961", write_len: SHORT, profiles: 1 },
    Model { vid: 0x3794, pid: 0xA000, fw: "1009", name: "Glorious Model O Eternal", write_len: None, profiles: 1 },
];

/// The settings of a validated block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub sensor: Sensor,
    pub rate_hz: u32,
    pub dpi: u32,
    /// Slot (0..8) of the active DPI stage.
    slot: usize,
    xy: bool,
}

/// Firmware version from a `05 01` reply, e.g. `05 01 56 31 30 33` -> "V103".
pub fn parse_fw(r: &[u8]) -> Option<String> {
    let v = r.get(FW_AT..FW_AT + FW_LEN)?;
    if r[0] != CMD_ID || r[1] != CMD_FIRMWARE || !v.iter().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(String::from_utf8_lossy(v).into_owned())
}

/// Slot of the `active`-th (1-based) enabled slot; checks `count`.
fn active_slot(b: &[u8]) -> Option<usize> {
    let count = (b[AT_STAGE] & LOW) as usize;
    let active = (b[AT_STAGE] >> 4) as usize;
    let enabled: Vec<usize> = (0..SLOTS).filter(|i| b[AT_DISABLED] & (1 << i) == 0).collect();
    if count == 0 || enabled.len() != count || active == 0 || active > count {
        return None;
    }
    Some(enabled[active - 1])
}

/// Byte offsets of the X and Y value of `slot`.
fn slot_at(slot: usize, xy: bool) -> (usize, usize) {
    if xy {
        return (AT_DPI + 2 * slot, AT_DPI + 2 * slot + 1);
    }
    (AT_DPI + slot, AT_DPI + slot)
}

/// Validates a block read with report `id` after command `cmd`.
///
/// ```text
/// 04 11 00 00 .. .. .. .. .. 06 04 26 C0 03 07 0F ..
///             3360 ─┘ 1000 Hz ┘  │  └ slots 6,7 off
///                 stage 2 of 6 ──┘   slot 1 = 07 -> 800 DPI
/// ```
pub fn decode(b: &[u8], id: u8, cmd: u8) -> Option<Config> {
    if b.len() != BLOCK_LEN || b[0] != id || b[1] != cmd || b[AT_WRITE] != 0 {
        return None;
    }
    let sensor = Sensor::from_raw(b[AT_SENSOR])?;
    let code = b[AT_RATE] & LOW;
    let rate_hz = RATES.iter().find(|(_, c)| *c == code)?.0;
    let xy = b[AT_RATE] & FLAG_XY != 0;
    let slot = active_slot(b)?;

    let (x, _) = slot_at(slot, xy);
    let dpi = sensor.dpi(b[x]);
    if dpi < DPI_MIN || dpi > sensor.max_dpi() {
        return None;
    }
    Some(Config { sensor, rate_hz, dpi, slot, xy })
}

/// Sets the active stage's DPI (X and Y) in a decoded block.
pub fn encode_dpi(b: &mut [u8], c: &Config, dpi: u32) {
    let (x, y) = slot_at(c.slot, c.xy);
    let raw = c.sensor.raw(dpi);
    b[x] = raw;
    b[y] = raw;
}

/// Sets the nearest supported rate, keeping the flag nibble.
pub fn encode_rate(b: &mut [u8], hz: u32) {
    let Some((_, code)) = RATES.iter().min_by_key(|(r, _)| r.abs_diff(hz)) else {
        return;
    };
    b[AT_RATE] = (b[AT_RATE] & !LOW) | code;
}

/// False when the bytes past the settings are all zero: a premature read
/// (see `SETTLE`) that, written back, would wipe buttons, RGB and macros.
pub fn tail_intact(b: &[u8], config_len: usize) -> bool {
    b.get(TAIL_AT..config_len).is_some_and(|t| t.iter().any(|&x| x != 0))
}

/// Turns a read block into a write: id, command and payload size.
pub fn seal(b: &mut [u8], id: u8, cmd: u8, config_len: usize) {
    b[0] = id;
    b[1] = cmd;
    b[AT_WRITE] = (config_len - HEADER_LEN) as u8;
}

fn is_mouse(i: &Interface) -> bool {
    i.usage_page == DESKTOP && i.usage == MOUSE
}

/// Sends `05 cmd` padded to the collection's feature length.
fn command(ch: &Channel, len: usize, cmd: u8) -> bool {
    let mut b = vec![0u8; len];
    b[0] = CMD_ID;
    b[1] = cmd;
    ch.set_feature(&b)
}

/// Command followed by its `05` reply (libratbag `query_read`).
fn query(ch: &Channel, len: usize, cmd: u8) -> Option<Vec<u8>> {
    if !command(ch, len, cmd) {
        return None;
    }
    let mut r = vec![0u8; len];
    r[0] = CMD_ID;
    if !ch.get_feature(&mut r) || r[1] != cmd {
        return None;
    }
    Some(r)
}

pub struct Sinowealth {
    cmd: Channel,
    cmd_len: usize,
    data: Channel,
    id: u8,
    model: &'static Model,
    fw: String,
    sensor: Sensor,
    wrote: Option<Instant>,
}

impl Sinowealth {
    /// Finds a listed mouse by firmware version and reads its config
    /// block once. Sends read commands only; never writes a block.
    pub fn open() -> Option<(Sinowealth, DeviceInfo)> {
        for vid in VIDS {
            let all = interfaces(vid);
            let mut pids: Vec<u16> = MODELS.iter().filter(|m| m.vid == vid).map(|m| m.pid).collect();
            pids.sort_unstable();
            pids.dedup();
            if let Some(d) = pids.into_iter().find_map(|pid| Self::probe(&all, vid, pid)) {
                return Some(d);
            }
        }
        None
    }

    fn probe(all: &[Interface], vid: u16, pid: u16) -> Option<(Sinowealth, DeviceInfo)> {
        let ours: Vec<&Interface> = all.iter().filter(|i| i.info.pid == pid).collect();
        // Keyboards reuse the ids: only a product that is also a mouse.
        if !ours.iter().any(|i| is_mouse(i)) {
            return None;
        }
        let vendor: Vec<&Interface> = ours.into_iter().filter(|i| i.usage_page >= VENDOR_PAGE).collect();
        let data_if = vendor.iter().find(|i| i.feature_len == BLOCK_LEN)?;

        // Command collection: the 6-byte report that answers 05 01. Never
        // probe other collections of a device not yet identified.
        let cands = vendor.iter().filter(|i| i.feature_len == CMD_LEN);
        let (cmd, cmd_len, fw) = cands.into_iter().find_map(|i| {
            let ch = Channel::open(&i.path)?;
            let fw = parse_fw(&query(&ch, i.feature_len, CMD_FIRMWARE)?)?;
            Some((ch, i.feature_len, fw))
        })?;
        let model = MODELS.iter().find(|m| m.vid == vid && m.pid == pid && m.fw == fw)?;

        let data = Channel::open(&data_if.path)?;
        let mut d = Sinowealth { cmd, cmd_len, data, id: CONFIG_IDS[0], model, fw, sensor: Sensor::Pmw3360, wrote: None };
        let (_, c, _) = CONFIG_IDS.into_iter().find_map(|id| {
            d.id = id;
            d.fetch()
        })?;
        d.sensor = c.sensor;
        Some((d, data_if.info.clone()))
    }

    /// Config command of the active profile.
    fn config_cmd(&self) -> Option<u8> {
        if self.model.profiles <= 1 {
            return Some(CMD_CONFIG[0]);
        }
        let r = query(&self.cmd, self.cmd_len, CMD_PROFILE)?;
        let index = (r[2] as usize).checked_sub(1)?;
        if index >= self.model.profiles as usize {
            return None;
        }
        CMD_CONFIG.get(index).copied()
    }

    /// Reads and validates the active profile's block.
    fn fetch(&mut self) -> Option<(Vec<u8>, Config, u8)> {
        if let Some(left) = self.wrote.and_then(|t| SETTLE.checked_sub(t.elapsed())) {
            std::thread::sleep(left);
        }
        let cmd = self.config_cmd()?;
        if !command(&self.cmd, self.cmd_len, cmd) {
            return None;
        }
        let mut b = vec![0u8; BLOCK_LEN];
        b[0] = self.id;
        if !self.data.get_feature(&mut b) {
            return None;
        }
        let c = decode(&b, self.id, cmd)?;
        Some((b, c, cmd))
    }

    /// Block for read-modify-write: two equal reads with an intact tail,
    /// so a glitched read is never written back to flash.
    fn fetch_stable(&mut self) -> Option<(Vec<u8>, Config, u8)> {
        let len = self.model.write_len?;
        let (a, c, cmd) = self.fetch()?;
        let (b, _, _) = self.fetch()?;
        if a != b || !tail_intact(&a, len) {
            return None;
        }
        Some((a, c, cmd))
    }

    /// Writes back a block `fetch_stable` returned (libratbag `write_configs`).
    fn store(&mut self, mut b: Vec<u8>, cmd: u8) -> bool {
        let Some(len) = self.model.write_len else {
            return false;
        };
        seal(&mut b, self.id, cmd, len);
        let ok = self.data.set_feature(&b);
        self.wrote = Some(Instant::now());
        ok
    }

    pub fn ident(&mut self) -> Ident {
        Ident {
            name: self.model.name.into(),
            firmware: self.fw.clone(),
            protocol: "Sinowealth (experimental)".into(),
            ..Ident::default()
        }
    }

    /// Read-only models (unknown config size) offer no DPI or rates.
    pub fn ability(&mut self) -> Ability {
        if self.model.write_len.is_none() {
            return Ability::default();
        }
        Ability {
            dpi: Some(DpiRange::Steps { min: DPI_MIN, max: self.sensor.max_dpi(), step: DPI_STEP }),
            rates: RATES.iter().map(|(hz, _)| *hz).collect(),
            ..Ability::default()
        }
    }

    /// `None` when the block cannot be read or does not validate.
    pub fn read(&mut self) -> Option<Reading> {
        let (_, c, _) = self.fetch()?;
        Some(Reading { dpi: Some(c.dpi), rate_hz: Some(c.rate_hz), ..Reading::default() })
    }

    /// DPI of the active stage, read-modify-write.
    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        if self.model.write_len.is_none() {
            return false;
        }
        let Some((mut b, c, cmd)) = self.fetch_stable() else {
            return false;
        };
        encode_dpi(&mut b, &c, dpi);
        self.store(b, cmd)
    }

    /// Report rate, read-modify-write.
    pub fn set_rate(&mut self, hz: u32) -> bool {
        if self.model.write_len.is_none() {
            return false;
        }
        let Some((mut b, _, cmd)) = self.fetch_stable() else {
            return false;
        };
        encode_rate(&mut b, hz);
        self.store(b, cmd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Glorious Model O style block: PMW3360, 1000 Hz, stage 2 of 6,
    /// slots 400/800/1600/3200/6400/12000.
    fn block() -> Vec<u8> {
        let mut b = vec![0u8; BLOCK_LEN];
        b[0] = 0x04;
        b[1] = 0x11;
        b[AT_SENSOR] = 0x06;
        b[AT_RATE] = 0x04;
        b[AT_STAGE] = 0x26;
        b[AT_DISABLED] = 0xC0;
        b[AT_DPI..AT_DPI + 6].copy_from_slice(&[3, 7, 15, 31, 63, 119]);
        b[0x35] = 0x02;
        b
    }

    #[test]
    fn decode_layout() {
        let c = decode(&block(), 0x04, 0x11).unwrap();
        assert_eq!((c.sensor, c.rate_hz, c.dpi, c.slot, c.xy), (Sensor::Pmw3360, 1000, 800, 1, false));
    }

    #[test]
    fn decode_skips_disabled_slots() {
        // Slot 0 off, stage 1 = slot 1.
        let mut b = block();
        b[AT_DISABLED] = 0xC1;
        b[AT_STAGE] = 0x15;
        assert_eq!(decode(&b, 0x04, 0x11).unwrap().slot, 1);
    }

    #[test]
    fn decode_rejects() {
        let check = |f: &dyn Fn(&mut Vec<u8>)| {
            let mut b = block();
            f(&mut b);
            decode(&b, 0x04, 0x11)
        };
        assert!(check(&|b| b[1] = 0x21).is_none());
        assert!(check(&|b| b[AT_WRITE] = 123).is_none());
        assert!(check(&|b| b[AT_SENSOR] = 0x02).is_none());
        assert!(check(&|b| b[AT_RATE] = 0x00).is_none());
        assert!(check(&|b| b[AT_STAGE] = 0x27).is_none());
        assert!(check(&|b| b[AT_STAGE] = 0x76).is_none());
        assert!(check(&|b| b[AT_DPI + 1] = 200).is_none());
        assert!(check(&|b| b.truncate(131)).is_none());
        assert!(decode(&block(), 0x06, 0x11).is_none());
    }

    #[test]
    fn sensor_encoding() {
        assert_eq!(Sensor::Pmw3360.dpi(7), 800);
        assert_eq!(Sensor::Pmw3389.dpi(8), 800);
        assert_eq!(Sensor::Pmw3327.raw(1600), 15);
        assert_eq!(Sensor::Pmw3212.raw(1600), 16);
        assert_eq!(Sensor::Pmw3212.raw(50_000), 72);
        assert_eq!(Sensor::Pmw3389.raw(0), 1);
        assert_eq!(Sensor::Pmw3360.raw(1650), 15);
    }

    #[test]
    fn dpi_changes_active_slot_only() {
        let mut b = block();
        let c = decode(&b, 0x04, 0x11).unwrap();
        encode_dpi(&mut b, &c, 1200);
        let mut want = block();
        want[AT_DPI + 1] = 11;
        assert_eq!(b, want);
    }

    #[test]
    fn dpi_xy_sets_both() {
        let mut b = block();
        b[AT_RATE] |= FLAG_XY;
        b[AT_DPI..AT_DPI + 4].copy_from_slice(&[3, 3, 7, 9]);
        let c = decode(&b, 0x04, 0x11).unwrap();
        assert_eq!(c.dpi, 800);
        encode_dpi(&mut b, &c, 2000);
        assert_eq!(&b[AT_DPI..AT_DPI + 4], &[3, 3, 19, 19]);
    }

    #[test]
    fn rate_keeps_flags() {
        let mut b = block();
        b[AT_RATE] = FLAG_XY | 0x4;
        encode_rate(&mut b, 500);
        assert_eq!(b[AT_RATE], 0x83);
        encode_rate(&mut b, 300);
        assert_eq!(b[AT_RATE], 0x82);
    }

    #[test]
    fn zeroed_tail_is_not_written() {
        let mut b = block();
        assert!(tail_intact(&b, 131));
        b[TAIL_AT..].fill(0);
        assert!(!tail_intact(&b, 131));
        assert!(!tail_intact(&b[..10], 131));
    }

    #[test]
    fn seal_header_only() {
        let mut b = block();
        seal(&mut b, 0x04, 0x11, 131);
        let mut want = block();
        want[AT_WRITE] = 0x7B;
        assert_eq!(b, want);
        seal(&mut b, 0x04, 0x21, 123);
        assert_eq!(&b[..4], &[0x04, 0x21, 0x00, 115]);
    }

    #[test]
    fn firmware_reply() {
        assert_eq!(parse_fw(&[0x05, 0x01, b'V', b'1', b'0', b'3']).as_deref(), Some("V103"));
        assert_eq!(parse_fw(&[0x05, 0x01, b'3', b'1', b'1', b'0']).as_deref(), Some("3110"));
        assert_eq!(parse_fw(&[0x05, 0x01, 0, 0, 0, 0]), None);
        assert_eq!(parse_fw(&[0x05, 0x11, b'V', b'1', b'0', b'3']), None);
        assert_eq!(parse_fw(&[0x05, 0x01]), None);
    }

    #[test]
    fn models_unique() {
        for (i, a) in MODELS.iter().enumerate() {
            assert!(MODELS[i + 1..].iter().all(|b| (a.vid, a.pid, a.fw) != (b.vid, b.pid, b.fw)));
            assert!(a.write_len.is_none_or(|n| n > HEADER_LEN));
        }
    }
}
