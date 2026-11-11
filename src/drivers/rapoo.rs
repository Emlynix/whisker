//! Rapoo's second generation of gaming mice (nRF54 firmware, vendor id 24AE)
//! speaks the "A5A5" register protocol of A Hub and of the hub.rapoo.cn web
//! configurator. Protocol facts from the WebHID driver study ClickSync
//! (github.com/Nuitfanee/ClickSync, `src/protocols/protocol_api_rapoo.js`,
//! GPL-2.0: used as information only, no code copied), which took them from
//! Rapoo's own configurator, and from a Whisker mouse report of the VT1 Gen 2
//! (issue #3). Experimental: written from those notes, never run on a Rapoo
//! mouse; what is still open is in `docs/rapoo.md`.
//!
//! ```text
//!  FF00:000E  output report 06 (33 B)   the frames go out
//!  FF00:000F  feature report 08 (33 B)   the answer to a register read
//!  FF00:0002  input report 07 (19 B)    status broadcast, carries the battery
//!
//!  write   06 | A5 A5 | len | addr | bank | 00 00 | data[0..len]   padded to 33
//!  read    06 | A5 A4 | len | addr | bank | 00 00 00    then GetFeature 08
//!  wake    06 | A5 A3 | 00 ...          what A Hub sends when it takes a mouse
//!  answer  08 | 01 00 00 00 | data[0..len]
//!  status  07 | 20 00 | dpi (2 B LE) | .. | stage | percent
//!
//!  bank 08 settings  0x80 report rate   0x88 DPI stages X   0xC8 DPI stages Y
//!                    0x96 stage count   0x98 active stage   0xDC..0xE2 speed
//!                    profile, one register per report rate
//!  bank 06 buttons   0x04.. one entry per button
//!  bank 00 radio     pairing, MAC and encryption keys
//! ```

use std::time::Duration;

use super::{Ability, DpiRange, Ident, Pick, Reading};
use crate::hid_link::raw::Channel;

/// Rapoo's vendor id.
pub const VID: u16 = 0x24AE;

/// Vendor usage page of the three configuration collections.
const PAGE: u16 = 0xFF00;
const USAGE_OUT: u16 = 0x000E;
const USAGE_ANSWER: u16 = 0x000F;
const USAGE_STATUS: u16 = 0x0002;

const REPORT_OUT: u8 = 0x06;
const REPORT_ANSWER: u8 = 0x08;
/// First payload byte of a status broadcast.
const MARK_STATUS: u8 = 0x20;

/// Frames and answers are 33 bytes with the report id, the broadcast 19.
const FRAME_LEN: usize = 33;
const STATUS_LEN: usize = 19;
/// Report id, `A5 op`, len, addr, bank and two zero bytes.
const HEAD_LEN: usize = 8;
/// Report id plus the four bytes an answer starts with.
const ANSWER_HEAD: usize = 5;
/// Data a frame carries behind its header.
const MAX_DATA: usize = FRAME_LEN - HEAD_LEN;

/// Second byte of every frame.
const MARK: u8 = 0xA5;
/// Write a register, read it, wake the configuration interface.
const OP_WRITE: u8 = 0xA5;
const OP_READ: u8 = 0xA4;
const OP_WAKE: u8 = 0xA3;

/// Settings bank. The button table is bank 06, the radio's bank 00.
const BANK_SETTINGS: u8 = 0x08;
const AT_RATE: u8 = 0x80;
const AT_DPI_X: u8 = 0x88;
const AT_DPI_Y: u8 = 0xC8;
const AT_STAGE: u8 = 0x98;

/// DPI stages of one axis; the firmware takes X and Y written the same.
const SLOTS: usize = 6;
const TABLE_LEN: usize = SLOTS * 2;
/// DPI slider of A Hub; the register holds 16 bits per stage.
const DPI_MIN: u32 = 50;
const DPI_MAX: u32 = 26_000;
const DPI_STEP: u32 = 10;

/// Report rates and their code in the rate register. 8000 Hz (`0x81`) lives in
/// the registers of Rapoo's 8K receiver, which this driver does not touch.
const RATES: [(u32, u8); 6] = [(125, 0x08), (250, 0x04), (500, 0x02), (1000, 0x01), (2000, 0x84), (4000, 0x82)];

/// One exchange at a time; A Hub leaves 12 ms between its frames.
const GAP: Duration = Duration::from_millis(15);
const WRITE_WAIT: Duration = Duration::from_millis(1000);
/// The battery comes with a broadcast only: how many of them to wait for.
const STATUS_TRIES: usize = 3;
const STATUS_WAIT: Duration = Duration::from_millis(250);

pub struct Rapoo {
    out: Channel,
    answer: Channel,
    /// Collection of the status broadcast; a firmware without it is no fault.
    status: Option<Channel>,
    /// DPI stages as the mouse last reported them, and which is active.
    stages: Option<[u32; SLOTS]>,
    active: usize,
}

impl Rapoo {
    /// Opens the three vendor collections of a Rapoo mouse. Sends nothing.
    pub fn open(pick: &Pick) -> Option<(Rapoo, crate::hid_link::DeviceInfo)> {
        let ifaces = pick.interfaces(VID);
        let usage = |u: u16| ifaces.iter().find(|i| i.usage_page == PAGE && i.usage == u);
        let w = usage(USAGE_OUT).filter(|i| i.output_len >= FRAME_LEN)?;
        let a = usage(USAGE_ANSWER).filter(|i| i.feature_len >= FRAME_LEN)?;
        let info = w.info.clone();
        let out = Channel::open(&w.path)?;
        let answer = Channel::open(&a.path)?;
        let status = usage(USAGE_STATUS).and_then(|i| Channel::open(&i.path));
        Some((Rapoo { out, answer, status, stages: None, active: 0 }, info))
    }

    /// No query names a Rapoo model: the catalogue entry names the mouse.
    pub fn ident(&mut self) -> Ident {
        Ident { protocol: "Rapoo A5A5".into(), ..Ident::default() }
    }

    /// The slider and the six stages of A Hub, the rates of the rate register,
    /// the battery of the status broadcast.
    pub fn ability(&mut self) -> Ability {
        Ability {
            dpi: Some(DpiRange::Steps { min: DPI_MIN, max: DPI_MAX, step: DPI_STEP }),
            rates: RATES.iter().map(|(hz, _)| *hz).collect(),
            battery: true,
            stages: None,
        }
    }

    /// Report rate, DPI of the active stage and battery. `None` when the mouse
    /// answers nothing at all (asleep, off, or a different protocol).
    pub fn read(&mut self) -> Option<Reading> {
        let rate = self.register(AT_RATE, BANK_SETTINGS, 1).and_then(|d| rate_hz(d[0]));
        let stage = self.register(AT_STAGE, BANK_SETTINGS, 1).and_then(|d| (d[0] < SLOTS as u8).then_some(d[0] as usize));
        let table = self.register(AT_DPI_X, BANK_SETTINGS, TABLE_LEN).as_deref().and_then(parse_table);
        if let (Some(t), Some(i)) = (table, stage) {
            self.stages = Some(t);
            self.active = i;
        }
        let dpi = match (table, stage) {
            (Some(t), Some(i)) => Some(t[i]),
            _ => None,
        };
        let battery = self.battery();
        (rate.is_some() || dpi.is_some() || battery.is_some()).then_some(Reading { battery, dpi, rate_hz: rate, ..Reading::default() })
    }

    /// DPI of the active stage: the X and the Y table written one after the
    /// other, which is what A Hub does. The mouse is not read back here; the
    /// next poll shows the value it took.
    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        let dpi = dpi.clamp(DPI_MIN, DPI_MAX);
        // Without a table the mouse reported, a write would overwrite the other
        // stages with values of ours.
        let Some(mut stages) = self.stages else {
            return false;
        };
        stages[self.active] = dpi;
        let bytes = table_bytes(&stages);
        self.wake();
        if !self.poke(AT_DPI_X, BANK_SETTINGS, &bytes) || !self.poke(AT_DPI_Y, BANK_SETTINGS, &bytes) {
            return false;
        }
        self.stages = Some(stages);
        true
    }

    /// Report rate. Every rate has its own speed-profile register
    /// (`0xDC..0xE2`), so nothing else goes with it.
    pub fn set_rate(&mut self, hz: u32) -> bool {
        let Some(code) = rate_code(hz) else {
            return false;
        };
        self.wake();
        self.poke(AT_RATE, BANK_SETTINGS, &[code])
    }

    /// `A5 A3` with no data: the frame A Hub sends when it takes a mouse. Sent
    /// before a write only. Reading registers does not need it, and a probe of
    /// an unknown mouse writes nothing.
    fn wake(&mut self) {
        let _ = self.out.write(&frame(OP_WAKE, 0, 0, &[]), WRITE_WAIT);
    }

    /// One register: the `A5 A4` frame out, then the answer on feature report
    /// 08. The mouse writes its answer into that report; it is no input report.
    fn register(&mut self, addr: u8, bank: u8, len: usize) -> Option<Vec<u8>> {
        if !self.out.write(&frame(OP_READ, addr, bank, &[]), WRITE_WAIT) {
            return None;
        }
        std::thread::sleep(GAP);
        let mut buf = [0u8; FRAME_LEN];
        buf[0] = REPORT_ANSWER;
        if !self.answer.get_feature(&mut buf) {
            return None;
        }
        answer(&buf, len)
    }

    /// `A5 A5` write of a register; the mouse does not acknowledge it.
    fn poke(&mut self, addr: u8, bank: u8, data: &[u8]) -> bool {
        let ok = self.out.write(&frame(OP_WRITE, addr, bank, data), WRITE_WAIT);
        std::thread::sleep(GAP);
        ok
    }

    /// Battery percent of a status broadcast, which the receiver sends on its own.
    fn battery(&mut self) -> Option<u8> {
        let ch = self.status.as_mut()?;
        for _ in 0..STATUS_TRIES {
            if let Some(p) = ch.read(STATUS_LEN, STATUS_WAIT).as_deref().and_then(broadcast) {
                return Some(p);
            }
        }
        None
    }
}

/// `[06 | A5 op | len | addr | bank | 00 00 | data[0..len]]`, padded to 33 bytes.
fn frame(op: u8, addr: u8, bank: u8, data: &[u8]) -> [u8; FRAME_LEN] {
    let n = data.len().min(MAX_DATA);
    let mut b = [0u8; FRAME_LEN];
    b[0] = REPORT_OUT;
    b[1] = MARK;
    b[2] = op;
    b[3] = n as u8;
    b[4] = addr;
    b[5] = bank;
    b[HEAD_LEN..HEAD_LEN + n].copy_from_slice(&data[..n]);
    b
}

/// Data behind the header of an answer. The values are checked per register
/// after this, so an answer that was never written reads as zeros.
fn answer(r: &[u8], len: usize) -> Option<Vec<u8>> {
    (r.len() >= ANSWER_HEAD + len).then(|| r[ANSWER_HEAD..ANSWER_HEAD + len].to_vec())
}

/// Battery percent of a status broadcast. `0x20` starts its payload, whether
/// or not a report id stands in front of it.
fn broadcast(r: &[u8]) -> Option<u8> {
    (r.len() >= 9 && r[1] == MARK_STATUS).then(|| r[8].min(100))
}

fn rate_hz(code: u8) -> Option<u32> {
    RATES.iter().find(|(_, c)| *c == code).map(|(hz, _)| *hz)
}

fn rate_code(hz: u32) -> Option<u8> {
    RATES.iter().find(|(x, _)| *x == hz).map(|(_, c)| *c)
}

/// The six DPI stages of 16 bits each, little endian. A stage of zero is no
/// stage: an answer of zeros reads as one.
fn parse_table(b: &[u8]) -> Option<[u32; SLOTS]> {
    if b.len() < TABLE_LEN {
        return None;
    }
    let mut out = [0u32; SLOTS];
    for (i, v) in out.iter_mut().enumerate() {
        let x = u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
        if x == 0 {
            return None;
        }
        *v = x as u32;
    }
    Some(out)
}

fn table_bytes(stages: &[u32; SLOTS]) -> Vec<u8> {
    stages.iter().flat_map(|&v| (v.clamp(1, u16::MAX as u32) as u16).to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The product id of the VT1 Gen 2 receiver (issue #3).
    const VT1_GEN2: u16 = 0x1415;

    #[test]
    fn frames_are_the_wire_form() {
        let w = frame(OP_WRITE, AT_RATE, BANK_SETTINGS, &[0x82]);
        assert_eq!(w[..7], [0x06, 0xA5, 0xA5, 1, 0x80, 0x08, 0x00]);
        assert_eq!(w[HEAD_LEN], 0x82);
        assert!(w[HEAD_LEN + 1..].iter().all(|&b| b == 0));

        let r = frame(OP_READ, AT_DPI_X, BANK_SETTINGS, &[]);
        assert_eq!(r[..6], [0x06, 0xA5, 0xA4, 0, 0x88, 0x08], "a read asks with the length");

        assert_eq!(frame(OP_WAKE, 0, 0, &[])[..8], [0x06, 0xA5, 0xA3, 0, 0, 0, 0, 0]);

        let long = frame(OP_WRITE, AT_DPI_X, BANK_SETTINGS, &[7u8; 40]);
        assert_eq!((long[3] as usize, long.len()), (MAX_DATA, FRAME_LEN), "25 data bytes at most");
    }

    #[test]
    fn answers_carry_the_register_of_the_read() {
        let mut a = [0u8; FRAME_LEN];
        a[0] = REPORT_ANSWER;
        a[1..5].copy_from_slice(&[1, 0, 0, 0]);
        a[5] = 0x82;
        assert_eq!(answer(&a, 1), Some(vec![0x82]));
        assert_eq!(answer(&a, 28).map(|d| d.len()), Some(28), "28 bytes behind the header");
        assert_eq!(answer(&a, 29), None);
    }

    #[test]
    fn rate_codes_are_known_both_ways() {
        for (hz, code) in RATES {
            assert_eq!((rate_hz(code), rate_code(hz)), (Some(hz), Some(code)));
        }
        assert_eq!(rate_hz(0x81), None, "8000 Hz belongs to the 8K receiver");
        assert_eq!(rate_hz(0), None, "no answer is not 1000 Hz");
        assert_eq!(rate_code(8000), None);
    }

    #[test]
    fn dpi_tables_of_sixteen_bits() {
        let stages = [800u32, 1600, 3200, 6400, 12_800, 26_000];
        let b = table_bytes(&stages);
        assert_eq!(b.len(), TABLE_LEN);
        assert_eq!(parse_table(&b), Some(stages));
        assert_eq!(b[..2], [0x20, 0x03], "little endian");
        assert_eq!(parse_table(&b[..4]), None, "a short table is no table");
        let mut blank = b.clone();
        blank[4..6].copy_from_slice(&[0, 0]);
        assert_eq!(parse_table(&blank), None, "zeros are an unanswered read");
        assert_eq!(table_bytes(&[0, 0, 0, 0, 0, 0])[..2], [1, 0], "a stage is never written as zero");
    }

    #[test]
    fn battery_of_a_status_broadcast() {
        // 20 00 05 DC .. 01 64: 1500 DPI, stage 1, 100 percent.
        let s = [0x07, 0x20, 0x00, 0xDC, 0x05, 0x00, 0x00, 0x01, 0x64];
        assert_eq!(broadcast(&s), Some(100));
        // A report that is not a status broadcast is not a battery.
        assert_eq!(broadcast(&[0x03, 0x21, 0, 0, 0, 0, 0, 0, 50]), None);
        assert_eq!(broadcast(&[0x07, 0x20, 0, 0, 0, 0, 0, 0, 130]), Some(100), "never more than full");
    }

    #[test]
    fn the_catalogue_names_the_mouse_the_driver_opens() {
        let c = crate::devices::listed_as(VID, Some(VT1_GEN2), "VT1 Gen 2").unwrap();
        assert_eq!(c.family, crate::devices::Family::Rapoo);
        assert_eq!(c.dpi, Some(DpiRange::Steps { min: DPI_MIN, max: DPI_MAX, step: DPI_STEP }));
        assert_eq!(c.rates, RATES.iter().map(|(hz, _)| *hz).collect::<Vec<u32>>());
    }
}
