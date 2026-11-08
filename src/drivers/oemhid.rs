//! CompX "report 8" mice configured by WebHID web apps: ATK, VXE, VGN and
//! Lamzu Atlantis share one OEM protocol (as documented by OpenMouse).
//! Output report 8 out, input report 8 back, on collection FF02:0002:
//!
//! ```text
//!  0    1     2       3..4       5     6..15   16
//!  08 | cmd | status | addr (BE) | len | data | sum
//!  sum = 0x55 - (bytes 0..=15), so the whole report sums to 0x55
//!  cmd 08 read EEPROM, 07 write EEPROM, 04 battery, 10 CID/MID, 12 firmware
//! ```
//!
//! EEPROM fields are `value, 0x55 - value` pairs: 0x00 report rate,
//! 0x02 stage count, 0x04 active stage, 0x0C + 4n DPI stage n as
//! `x, y, mode nibbles (y << 4 | x), check`. ATK receivers are shared
//! between models, so the model (and with it the DPI encoding) comes from
//! CID/MID. Writes happen only on model + transport pairs that OpenMouse
//! verified on hardware; everything else is read-only.

use super::{Ability, DpiRange, Ident, Reading};
use crate::hid_link::raw::{Channel, interfaces};
use std::time::Duration;

/// ATK's own vendor id and CompX's shared ODM id (VXE, VGN, Lamzu).
pub const VIDS: [u16; 2] = [VID_ATK, VID_COMPX];
const VID_ATK: u16 = 0x373B;
const VID_COMPX: u16 = 0x3554;

const REPORT_ID: u8 = 0x08;
/// Report id + 16-byte payload.
const FRAME_LEN: usize = 17;
/// Generous input buffer; replies are `FRAME_LEN` long.
const READ_LEN: usize = 64;
const DATA: usize = 6;
const MAX_DATA: usize = 10;
const SUM: u8 = 0x55;
const STATUS_OK: u8 = 0x00;

const CONFIG_PAGE: u16 = 0xFF02;
const CONFIG_USAGE: u16 = 0x0002;

const CMD_BATTERY: u8 = 0x04;
const CMD_WRITE: u8 = 0x07;
const CMD_READ: u8 = 0x08;
const CMD_CIDMID: u8 = 0x10;
const CMD_VERSION: u8 = 0x12;

const ADDR_SYSTEM: u16 = 0x0000;
const SYSTEM_LEN: u8 = 6;
const ADDR_STAGES: u16 = 0x000C;
const STAGE_LEN: u8 = 4;
const MAX_STAGES: u8 = 8;
const MAX_PERCENT: u8 = 100;

const REPLY_WAIT: Duration = Duration::from_millis(500);
/// Lamzu echoes a write, ATK does not; either way the mouse needs a moment.
const ACK_WAIT: Duration = Duration::from_millis(250);
/// A sleeping wireless mouse may miss the first CID/MID query.
const ID_TRIES: u8 = 3;

/// Rate codes (= 1000 / interval, 2K+ as flags).
const RATES: [(u8, u32); 7] = [(0x08, 125), (0x04, 250), (0x02, 500), (0x01, 1000), (0x10, 2000), (0x20, 4000), (0x40, 8000)];
/// Lamzu cable: 0x10 is also read as 1000 Hz (vendor UI showed 1000).
const RATES_LAMZU: [(u8, u32); 5] = [(0x08, 125), (0x04, 250), (0x02, 500), (0x01, 1000), (0x10, 1000)];

/// Stage encoding families.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    /// (count + 1) * 50; above 30,000 counts of 100.
    Step50,
    /// (count + 1) * 10 up to 10,000, then 50s, above 30,000 100s.
    Ten,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Access {
    Full,
    ReadOnly,
}

/// One mouse model.
struct Spec {
    name: &'static str,
    codec: Codec,
    min: u32,
    max: u32,
    rates: &'static [(u8, u32)],
    max_hz: u32,
    access: Access,
}

/// One USB transport (cable or receiver).
struct Port {
    vid: u16,
    pid: u16,
    /// Transport ceiling, e.g. a 1K receiver.
    cap_hz: u32,
    access: Access,
    /// `None`: shared receiver, model comes from CID/MID.
    spec: Option<&'static Spec>,
}

const F1_ULTIMATE: Spec = Spec { name: "ATK F1 Ultimate 2.0", codec: Codec::Ten, min: 10, max: 42000, rates: &RATES, max_hz: 8000, access: Access::Full };
const X1_PRO_MAX: Spec = Spec { name: "ATK X1 Pro Max", codec: Codec::Step50, min: 50, max: 36000, rates: &RATES, max_hz: 8000, access: Access::Full };
const A9_PLUS_NK: Spec = Spec { name: "ATK A9 Plus Nearlink", codec: Codec::Step50, min: 100, max: 30000, rates: &RATES, max_hz: 1000, access: Access::Full };
const R1_PRO_MAX: Spec = Spec { name: "VXE R1 Pro Max", codec: Codec::Step50, min: 100, max: 30000, rates: &RATES, max_hz: 1000, access: Access::Full };
const VGN_F2: Spec = Spec { name: "VGN Dragonfly F2 Master+", codec: Codec::Step50, min: 50, max: 26000, rates: &RATES, max_hz: 8000, access: Access::ReadOnly };
const ATLANTIS: Spec = Spec { name: "Lamzu Atlantis", codec: Codec::Step50, min: 50, max: 26000, rates: &RATES_LAMZU, max_hz: 1000, access: Access::Full };
const ATLANTIS_4K: Spec = Spec { name: "Lamzu Atlantis", codec: Codec::Step50, min: 50, max: 26000, rates: &RATES, max_hz: 4000, access: Access::ReadOnly };

/// Models by GetMouseCIDMID (command 0x10) reply.
const MODELS: [((u8, u8), &Spec); 4] = [((1, 8), &F1_ULTIMATE), ((2, 39), &X1_PRO_MAX), ((2, 83), &A9_PLUS_NK), ((2, 27), &R1_PRO_MAX)];

/// Known transports. Unlisted ids (R1 SE+, Pulsar, …) are not opened.
const PORTS: [Port; 13] = [
    Port { vid: VID_ATK, pid: 0x1017, cap_hz: 8000, access: Access::Full, spec: None },
    Port { vid: VID_ATK, pid: 0x101B, cap_hz: 8000, access: Access::Full, spec: None },
    Port { vid: VID_ATK, pid: 0x101A, cap_hz: 1000, access: Access::Full, spec: None },
    Port { vid: VID_ATK, pid: 0x1115, cap_hz: 1000, access: Access::Full, spec: None },
    Port { vid: VID_ATK, pid: 0x10C9, cap_hz: 1000, access: Access::Full, spec: None },
    Port { vid: VID_ATK, pid: 0x11D9, cap_hz: 8000, access: Access::Full, spec: None },
    Port { vid: VID_COMPX, pid: 0xF58C, cap_hz: 1000, access: Access::Full, spec: None },
    // Pro Max receiver writes need a full 10-byte system row; not ported.
    Port { vid: VID_COMPX, pid: 0xF58A, cap_hz: 1000, access: Access::ReadOnly, spec: None },
    Port { vid: VID_COMPX, pid: 0xFB57, cap_hz: 8000, access: Access::ReadOnly, spec: Some(&VGN_F2) },
    Port { vid: VID_COMPX, pid: 0xFB56, cap_hz: 8000, access: Access::ReadOnly, spec: Some(&VGN_F2) },
    Port { vid: VID_COMPX, pid: 0xF50F, cap_hz: 1000, access: Access::Full, spec: Some(&ATLANTIS) },
    Port { vid: VID_COMPX, pid: 0xF50D, cap_hz: 1000, access: Access::ReadOnly, spec: Some(&ATLANTIS) },
    // The 4K receiver re-enumerates on a rate change.
    Port { vid: VID_COMPX, pid: 0xF510, cap_hz: 4000, access: Access::ReadOnly, spec: Some(&ATLANTIS_4K) },
];

fn port(vid: u16, pid: u16) -> Option<&'static Port> {
    PORTS.iter().find(|p| p.vid == vid && p.pid == pid)
}

fn model(cid: u8, mid: u8) -> Option<&'static Spec> {
    MODELS.iter().find(|(id, _)| *id == (cid, mid)).map(|(_, s)| *s)
}

/// 0x55 minus the sum of report bytes 0..=15.
fn checksum(b: &[u8]) -> u8 {
    b[..FRAME_LEN - 1].iter().fold(SUM, |a, x| a.wrapping_sub(*x))
}

/// Request `[08, cmd, 0, addr hi, addr lo, len, data…, sum]`.
fn frame(cmd: u8, addr: u16, len: u8, data: &[u8]) -> [u8; FRAME_LEN] {
    let mut b = [0u8; FRAME_LEN];
    b[0] = REPORT_ID;
    b[1] = cmd;
    b[3..5].copy_from_slice(&addr.to_be_bytes());
    b[5] = len;
    let n = data.len().min(MAX_DATA);
    b[DATA..DATA + n].copy_from_slice(&data[..n]);
    b[FRAME_LEN - 1] = checksum(&b);
    b
}

/// Data of a valid, successful reply to one of `cmds`.
fn parse(r: &[u8], cmds: &[u8]) -> Option<Vec<u8>> {
    if r.len() < FRAME_LEN || r[0] != REPORT_ID || !cmds.contains(&r[1]) {
        return None;
    }
    if r[2] != STATUS_OK || r[FRAME_LEN - 1] != checksum(r) {
        return None;
    }
    let n = (r[5] as usize).min(MAX_DATA);
    Some(r[DATA..DATA + n].to_vec())
}

/// Value of a `value, 0x55 - value` pair.
fn pair(v: u8, check: u8) -> Option<u8> {
    (v.wrapping_add(check) == SUM).then_some(v)
}

/// Above this, counts are of 100 (the doubled flag).
const DOUBLE_ABOVE: u32 = 30000;
/// Top of the 10-DPI range of `Codec::Ten`.
const TEN_TOP: u32 = 10000;
const FIFTY_BASE: u32 = TEN_TOP + 50;
const MODE_DOUBLED: u8 = 0x01;
const MODE_FIFTY: u8 = 0x02;

/// One axis: (value byte, mode nibble).
fn encode_axis(codec: Codec, dpi: u32) -> (u8, u8) {
    let doubled = dpi > DOUBLE_ABOVE;
    let flag = if doubled { MODE_DOUBLED } else { 0 };
    let (code, range) = match codec {
        Codec::Step50 => (dpi / if doubled { 100 } else { 50 } - 1, 0),
        Codec::Ten => {
            let v = if doubled { dpi / 2 } else { dpi };
            if v <= TEN_TOP { (v / 10 - 1, 0) } else { ((v - FIFTY_BASE) / 50, MODE_FIFTY) }
        }
    };
    ((code & 0xFF) as u8, (((code >> 8) & 0x03) as u8) << 2 | range | flag)
}

fn decode_axis(codec: Codec, byte: u8, nibble: u8) -> u32 {
    let code = (((nibble >> 2) & 0x03) as u32) << 8 | byte as u32;
    let base = match codec {
        Codec::Step50 => (code + 1) * 50,
        Codec::Ten if nibble & MODE_FIFTY != 0 => FIFTY_BASE + code * 50,
        Codec::Ten => (code + 1) * 10,
    };
    if nibble & MODE_DOUBLED != 0 { base * 2 } else { base }
}

/// Stage row with the same DPI on both axes.
fn pack_stage(codec: Codec, dpi: u32) -> [u8; 4] {
    let (b, n) = encode_axis(codec, dpi);
    let mode = n << 4 | n;
    [b, b, mode, SUM.wrapping_sub(b).wrapping_sub(b).wrapping_sub(mode)]
}

/// X-axis DPI of a stage row, `None` on a bad check byte.
fn unpack_stage(codec: Codec, s: &[u8]) -> Option<u32> {
    if s.len() < STAGE_LEN as usize || s[..4].iter().fold(0u8, |a, x| a.wrapping_add(*x)) != SUM {
        return None;
    }
    Some(decode_axis(codec, s[0], s[2] & 0x0F))
}

/// Every DPI the codec can store between `min` and `max`.
fn dpi_values(s: &Spec) -> Vec<u32> {
    let fine = match s.codec {
        Codec::Step50 => 50,
        Codec::Ten => 10,
    };
    let from = |d: u32| d.max(s.min);
    let mut v: Vec<u32> = (s.min..=s.max.min(TEN_TOP)).step_by(fine).collect();
    v.extend((from(FIFTY_BASE)..=s.max.min(DOUBLE_ABOVE)).step_by(50));
    v.extend((from(DOUBLE_ABOVE + 100)..=s.max).step_by(100));
    v
}

pub struct OemHid {
    ch: Channel,
    port: &'static Port,
    spec: Option<&'static Spec>,
    tries: u8,
}

impl OemHid {
    /// Opens the FF02:0002 collection carrying report 8. Sends nothing.
    pub fn open() -> Option<(OemHid, crate::hid_link::DeviceInfo)> {
        for vid in VIDS {
            let found = interfaces(vid).into_iter().find_map(|i| {
                let p = port(i.info.vid, i.info.pid)?;
                let config = i.usage_page == CONFIG_PAGE && i.usage == CONFIG_USAGE && i.output_len == FRAME_LEN;
                config.then_some((i, p))
            });
            let Some((iface, p)) = found else {
                continue;
            };
            let ch = Channel::open(&iface.path)?;
            return Some((OemHid { ch, port: p, spec: p.spec, tries: 0 }, iface.info));
        }
        None
    }

    /// Sends a request; data of the first matching reply.
    fn call(&self, cmd: u8, addr: u16, len: u8, data: &[u8], accept: &[u8]) -> Option<Vec<u8>> {
        let req = frame(cmd, addr, len, data);
        if !self.ch.write(&req, REPLY_WAIT) {
            return None;
        }
        let want = addr.to_be_bytes();
        self.ch.read_until(READ_LEN, REPLY_WAIT, |r| {
            let d = parse(r, accept)?;
            let same = cmd != CMD_READ || (r[3..5] == want && d.len() >= len as usize);
            same.then_some(d)
        })
    }

    fn query(&self, cmd: u8) -> Option<Vec<u8>> {
        self.call(cmd, 0, 0, &[], &[cmd])
    }

    /// Some firmware answers a read with the write command id.
    fn peek(&self, addr: u16, len: u8) -> Option<Vec<u8>> {
        self.call(CMD_READ, addr, len, &[], &[CMD_READ, CMD_WRITE])
    }

    /// Writes a field, then waits for the echo (or the settle time).
    fn poke(&self, addr: u16, data: &[u8]) -> bool {
        let req = frame(CMD_WRITE, addr, data.len() as u8, data);
        if !self.ch.write(&req, REPLY_WAIT) {
            return false;
        }
        let _ = self.ch.read_until(READ_LEN, ACK_WAIT, |r| parse(r, &[CMD_WRITE]));
        true
    }

    /// Model, from the port or (shared receivers) from CID/MID.
    fn spec(&mut self) -> Option<&'static Spec> {
        if self.spec.is_some() || self.tries >= ID_TRIES {
            return self.spec;
        }
        self.tries += 1;
        let id = self.query(CMD_CIDMID)?;
        if id.len() < 2 {
            return None;
        }
        self.tries = ID_TRIES;
        self.spec = model(id[0], id[1]);
        self.spec
    }

    /// Model if writes to it over this transport were verified.
    fn writable(&mut self) -> Option<&'static Spec> {
        let s = self.spec()?;
        (self.port.access == Access::Full && s.access == Access::Full).then_some(s)
    }

    fn ceiling(&mut self) -> u32 {
        self.spec().map_or(self.port.cap_hz, |s| s.max_hz.min(self.port.cap_hz))
    }

    fn rates(&mut self) -> Vec<u32> {
        let top = self.ceiling();
        let table = self.spec().map_or(&RATES[..], |s| s.rates);
        let mut v: Vec<u32> = table.iter().map(|(_, hz)| *hz).filter(|hz| *hz <= top).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// `[rate, stage count, active stage]` from the system row.
    fn system(&self) -> Option<[u8; 3]> {
        let s = self.peek(ADDR_SYSTEM, SYSTEM_LEN)?;
        Some([pair(s[0], s[1])?, pair(s[2], s[3])?, pair(s[4], s[5])?])
    }

    fn stage_addr(sys: [u8; 3]) -> u16 {
        let count = sys[1].clamp(1, MAX_STAGES);
        ADDR_STAGES + sys[2].min(count - 1) as u16 * STAGE_LEN as u16
    }

    pub fn ident(&mut self) -> Ident {
        let mut id = Ident { protocol: "CompX report 8".into(), ..Ident::default() };
        id.name = self.spec().map_or(String::new(), |s| s.name.into());
        // BCD: 02 18 is v2.18.
        if let Some(v) = self.query(CMD_VERSION).filter(|v| v.len() >= 2) {
            id.firmware = format!("v{:x}.{:02x}", v[0], v[1]);
        }
        id
    }

    pub fn ability(&mut self) -> Ability {
        Ability {
            dpi: self.spec().map(|s| DpiRange::List(dpi_values(s))),
            rates: self.rates(),
            battery: self.query(CMD_BATTERY).is_some(),
            stages: None,
        }
    }

    /// `None` when the mouse does not answer (asleep or off).
    pub fn read(&mut self) -> Option<Reading> {
        let sys = self.system()?;
        let mut r = Reading::default();

        let top = self.ceiling();
        let table = self.spec().map_or(&RATES[..], |s| s.rates);
        // A mouse keeps its 8K setting on a 1K receiver: report the cap.
        r.rate_hz = table.iter().find(|(code, _)| *code == sys[0]).map(|(_, hz)| (*hz).min(top));

        if let Some(s) = self.spec() {
            r.dpi = self.peek(Self::stage_addr(sys), STAGE_LEN).and_then(|d| unpack_stage(s.codec, &d));
        }

        // Percent, charging, then millivolts (the length byte may say 2).
        if let Some(b) = self.query(CMD_BATTERY).filter(|b| b.len() >= 2) {
            r.battery = (b[0] <= MAX_PERCENT).then_some(b[0]);
            r.charging = b[1] != 0;
        }
        Some(r)
    }

    /// Rewrites the active stage; confirmed by reading it back.
    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        let Some(s) = self.writable() else {
            return false;
        };
        let Some(dpi) = dpi_values(s).into_iter().min_by_key(|v| v.abs_diff(dpi)) else {
            return false;
        };
        let Some(sys) = self.system() else {
            return false;
        };

        let addr = Self::stage_addr(sys);
        if !self.poke(addr, &pack_stage(s.codec, dpi)) {
            return false;
        }
        self.peek(addr, STAGE_LEN).and_then(|d| unpack_stage(s.codec, &d)) == Some(dpi)
    }

    /// Nearest supported rate; confirmed by reading it back.
    pub fn set_rate(&mut self, hz: u32) -> bool {
        let Some(s) = self.writable() else {
            return false;
        };
        let top = self.ceiling();
        let Some(&(code, _)) = s.rates.iter().filter(|(_, r)| *r <= top).min_by_key(|(_, r)| r.abs_diff(hz)) else {
            return false;
        };

        if !self.poke(ADDR_SYSTEM, &[code, SUM.wrapping_sub(code)]) {
            return false;
        }
        self.system().is_some_and(|sys| sys[0] == code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split(' ').map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn requests_match_captures() {
        // ATK X1 Pro Max cable (OpenMouse captures/atk-x1-pro-max).
        assert_eq!(frame(CMD_CIDMID, 0, 0, &[]).to_vec(), hex("08 10 00 00 00 00 00 00 00 00 00 00 00 00 00 00 3d"));
        assert_eq!(frame(CMD_READ, 0x0C, 4, &[]).to_vec(), hex("08 08 00 00 0c 04 00 00 00 00 00 00 00 00 00 00 35"));
        assert_eq!(frame(CMD_WRITE, 0x10, 4, &[0x18, 0x18, 0x00, 0x25]).to_vec(), hex("08 07 00 00 10 04 18 18 00 25 00 00 00 00 00 00 dd"));
        assert_eq!(frame(CMD_WRITE, 0x00, 2, &[0x20, 0x35]).to_vec(), hex("08 07 00 00 00 02 20 35 00 00 00 00 00 00 00 00 ef"));
        // Lamzu Atlantis 500 Hz write (docs/lamzu-atlantis-testing.md).
        assert_eq!(frame(CMD_WRITE, 0x00, 2, &[0x02, 0x53]).to_vec(), hex("08 07 00 00 00 02 02 53 00 00 00 00 00 00 00 00 ef"));
    }

    #[test]
    fn replies_decode() {
        let id = parse(&hex("08 10 00 00 00 03 02 27 2c 00 00 00 00 00 00 00 e5"), &[CMD_CIDMID]).unwrap();
        assert_eq!(model(id[0], id[1]).unwrap().name, "ATK X1 Pro Max");

        let bat = parse(&hex("08 04 00 00 00 02 2d 01 0f 06 00 00 00 00 00 00 04"), &[CMD_BATTERY]).unwrap();
        assert_eq!(bat, vec![45, 1]);

        // 8000 Hz, two stages, stage 1 active.
        let s = parse(&hex("08 08 00 00 00 06 40 15 02 53 01 54 00 00 00 00 40"), &[CMD_READ]).unwrap();
        assert_eq!([pair(s[0], s[1]), pair(s[2], s[3]), pair(s[4], s[5])], [Some(0x40), Some(2), Some(1)]);

        // Lamzu receiver reads 250 Hz.
        let l = parse(&hex("08 08 00 00 00 02 04 51 00 00 00 00 00 00 00 00 ee"), &[CMD_READ]).unwrap();
        assert_eq!(pair(l[0], l[1]), Some(0x04));
    }

    #[test]
    fn bad_checksum_or_status_rejected() {
        let mut r = hex("08 08 00 00 0c 04 0f 0f 00 37 00 00 00 00 00 00 e0");
        assert!(parse(&r, &[CMD_READ]).is_some());
        assert!(parse(&r, &[CMD_BATTERY]).is_none());
        r[16] ^= 1;
        assert!(parse(&r, &[CMD_READ]).is_none());
        r[16] ^= 1;
        r[2] = 1;
        r[16] = r[16].wrapping_sub(1);
        assert!(parse(&r, &[CMD_READ]).is_none());
    }

    #[test]
    fn step50_stages() {
        // X1 Pro Max (PAW3950): 0f 0f 00 37 = 800, 17 17 00 27 = 1200.
        assert_eq!(unpack_stage(Codec::Step50, &[0x0F, 0x0F, 0x00, 0x37]), Some(800));
        assert_eq!(unpack_stage(Codec::Step50, &[0x17, 0x17, 0x00, 0x27]), Some(1200));
        assert_eq!(pack_stage(Codec::Step50, 1250), [0x18, 0x18, 0x00, 0x25]);
        assert_eq!(unpack_stage(Codec::Step50, &[0x0F, 0x0F, 0x00, 0x36]), None);
        for dpi in [50, 12800, 12850, 26000, 30000, 30100, 36000] {
            assert_eq!(unpack_stage(Codec::Step50, &pack_stage(Codec::Step50, dpi)), Some(dpi));
        }
    }

    #[test]
    fn ten_stages() {
        // Pulsar CrazyLight: 800 DPI = raw 0x4f, flags 0.
        assert_eq!(pack_stage(Codec::Ten, 800), [0x4F, 0x4F, 0x00, 0xB7]);
        // 10,050 switches to the 50s range (mode bit 1 on both axes).
        assert_eq!(pack_stage(Codec::Ten, 10050)[2], 0x22);
        for dpi in [10, 10000, 10050, 30000, 30100, 42000] {
            assert_eq!(unpack_stage(Codec::Ten, &pack_stage(Codec::Ten, dpi)), Some(dpi));
        }
    }

    #[test]
    fn dpi_lists() {
        let v = dpi_values(&X1_PRO_MAX);
        assert_eq!(v[..2], [50, 100]);
        assert!(v.contains(&30000) && v.contains(&30100) && !v.contains(&30050));
        assert_eq!(*v.last().unwrap(), 36000);

        let t = dpi_values(&F1_ULTIMATE);
        assert_eq!(t[..2], [10, 20]);
        assert!(t.contains(&10050) && !t.contains(&10010));
        assert_eq!(*t.last().unwrap(), 42000);
        assert_eq!(dpi_values(&A9_PLUS_NK)[0], 100);
    }

    #[test]
    fn ports_and_access() {
        assert!(port(VID_ATK, 0x101B).unwrap().spec.is_none());
        assert_eq!(port(VID_COMPX, 0xF50F).unwrap().spec.unwrap().name, "Lamzu Atlantis");
        assert_eq!(port(VID_COMPX, 0xF58A).unwrap().access, Access::ReadOnly);
        assert!(port(VID_COMPX, 0xF58F).is_none());
        // Lamzu cable reads 0x10 as 1000 Hz but writes 1000 Hz as 0x01.
        assert_eq!(RATES_LAMZU.iter().find(|(c, _)| *c == 0x10).unwrap().1, 1000);
        assert_eq!(RATES_LAMZU.iter().find(|(_, hz)| *hz == 1000).unwrap().0, 0x01);
    }
}
