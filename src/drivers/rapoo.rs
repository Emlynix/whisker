//! Rapoo's second generation of gaming mice (nRF54 firmware, vendor id 24AE)
//! speaks the "A5A5" register protocol of A Hub and of the hub.rapoo.cn web
//! configurator. Protocol facts from the WebHID driver study ClickSync
//! (github.com/Nuitfanee/ClickSync, `src/protocols/protocol_api_rapoo.js`,
//! GPL-2.0: used as information only, no code copied), which took them from
//! Rapoo's own configurator, and from a Whisker mouse report of the VT1 Gen 2
//! (issue #3). Measured now against a connected and awake VT1 Gen 2 (24AE:
//! 1415): every register of bank 08 that was asked answered, and the frame
//! layout the driver uses is the one the Rapoo configurator uses. What is
//! still open is in `docs/rapoo.md`.
//!
//! ```text
//!  FF00:000E  output report 06 (33 B)    the frames go out
//!  FF00:000F  feature report 08 (33 B)   the answer to a register read
//!  FF00:0002  input report 07 (19 B)     status broadcast, carries the battery
//!
//!  write   06 | A5 A5 | len | addr | bank | 00 00 | data[0..len]
//!  read    06 | A5 A4 | len | addr | bank | 00 00 00   then GetFeature 08
//!  wake    06 | A5 A3 | 00 ...           what A Hub sends when it takes a mouse
//!  answer  01 | 00 00 00 | data[0..len]  four header bytes, then the values
//!  status  07 | head (2 B) | dpi (2 B LE) | .. | stage | percent | .. | time
//!
//!  bank 08 settings  0x80 report rate   0x81 key scan rate  0x84 lift-off
//!                    0x85 motion sync   0x88..0x93 DPI stages, six of 2 B
//!                    0x96 stages in use - 1   0x98 stage selected
//!                    0xC0 debounce  0xC1 lift-off delay  0xC2 sleep, minutes
//!                    0xC3 ripple and linear  0xC4 angle  0xC5 glass mode
//!                    0xC8..0xD3 DPI stages Y  0xDC..0xE2 performance mode,
//!                    one register per report rate
//!  bank 06 buttons   0x04.. one entry per button
//!  bank 00 radio     pairing, MAC and encryption keys
//! ```
//!
//! A read of one byte answers with four, a table read with the table that was
//! asked for, and a read that asks for more than the register holds is not
//! answered at all. DPI stages are 16 bits little endian, and a mouse with two
//! stages in use answers six of them, the ones it does not use filled with the
//! 800 that A Hub pads to.
//!
//! A report goes out with the length its collection declares, so the driver
//! reads the sizes off the collections instead of assuming 33 bytes. The `len`
//! of a read frame is the length of the answer wanted.

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
/// Report id of the collection the status broadcasts arrive on.
const REPORT_BROADCAST: u8 = 0x07;

/// Frames and answers are 33 bytes with the report id, and so are the
/// broadcasts of the VT1 Gen 2; a driver never assumes a size, it takes the
/// one the collection reports.
const FRAME_LEN: usize = 33;
/// Report id, `A5 op`, len, addr, bank and two zero bytes.
const HEAD_LEN: usize = 8;
/// The four bytes an answer starts with, before the register values: `01 00
/// 00 00`. The mouse does not put a report id in front of them, so they are
/// the first four bytes of what a feature read returns.
const ANSWER_HEAD: usize = 4;
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
const AT_COUNT: u8 = 0x96;
const AT_STAGE: u8 = 0x98;
/// A new report rate needs a moment before the mouse answers anything else.
const RATE_SETTLE: Duration = Duration::from_millis(120);

/// DPI stages of one axis; the firmware takes X and Y written the same.
const SLOTS: usize = 6;
/// DPI slider of A Hub; the register holds 16 bits per stage.
const DPI_MIN: u32 = 50;
const DPI_MAX: u32 = 26_000;
const DPI_STEP: u32 = 10;

/// Report rates and their code in the rate register. The high three need a
/// receiver that can carry them, and a mouse refuses a rate it cannot do.
const RATES: [(u32, u8); 7] = [(125, 0x08), (250, 0x04), (500, 0x02), (1000, 0x01), (2000, 0x84), (4000, 0x82), (8000, 0x81)];

/// One exchange at a time; A Hub leaves 12 ms between its frames.
const GAP: Duration = Duration::from_millis(15);
const WRITE_WAIT: Duration = Duration::from_millis(1000);
/// Battery percent needs the id, the `0x20` marker and its own byte.
const STATUS_MIN: usize = 9;
/// How often one register read asks the mouse for its answer. The receiver
/// needs a moment for the mouse, so it is worth asking a few times.
const ANSWER_TRIES: usize = 6;
/// How often the read request itself goes out again when nothing answered it.
/// Kept low because a sleeping mouse answers none of them.
const ASK_TRIES: usize = 2;
/// The battery comes with a broadcast only: how many of them to read at a poll.
const STATUS_TRIES: usize = 3;
const STATUS_WAIT: Duration = Duration::from_millis(150);

pub struct Rapoo {
    out: Channel,
    /// Size of a report on the collection the frames go out on.
    out_len: usize,
    answer: Channel,
    /// Size of the feature report an answer comes back in.
    answer_len: usize,
    /// Collection of the status broadcasts, with the size of one.
    status: Option<(Channel, usize)>,
    /// DPI stages as the mouse last reported them, how many it uses and which
    /// of them is active.
    stages: Option<[u32; SLOTS]>,
    count: usize,
    active: usize,
    /// What the polls answered last, so a mouse that answers slowly does not
    /// blink out of its cards.
    rate: Option<u32>,
    /// What the last status broadcast named: the percent, the DPI, the stage
    /// and the rate it is using. A mouse sends these on its own, so they are
    /// what a poll can fall back on when a register read went unanswered.
    pushed: Option<Broadcast>,
}

/// What a status broadcast of report 07 carries. The percent sits at the same
/// place in the payload on every mouse of this generation; the DPI, the stage
/// and the two rate codes are as they were read off a VT1 Gen 2: `10 01 | dpi
/// (2 B LE) | dpi (2 B LE) | stage | percent | 00 | scan | rate | .. | timer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Broadcast {
    pub battery: u8,
    pub dpi: u16,
    pub stage: u8,
    /// Report rate as its code in the broadcast, when one is recognised there.
    pub rate: Option<u32>,
    /// Last byte of the payload. It fell by eleven between two broadcasts
    /// eleven seconds apart and was 197 in a later run, so it counts something
    /// that restarts: what exactly is open.
    pub timer: u8,
}

impl Rapoo {
    /// Opens the three vendor collections of a Rapoo mouse. Sends nothing.
    pub fn open(pick: &Pick) -> Option<(Rapoo, crate::hid_link::DeviceInfo)> {
        let ifaces = pick.interfaces(VID);
        let usage = |u: u16| ifaces.iter().find(|i| i.usage_page == PAGE && i.usage == u);
        let w = usage(USAGE_OUT).filter(|i| i.output_len >= HEAD_LEN)?;
        let a = usage(USAGE_ANSWER).filter(|i| i.feature_len >= ANSWER_HEAD)?;
        let info = w.info.clone();
        let out = Channel::open(&w.path)?;
        let answer = Channel::open(&a.path)?;
        let status = usage(USAGE_STATUS).filter(|i| i.input_len >= STATUS_MIN).and_then(|i| Channel::open(&i.path).map(|c| (c, i.input_len)));
        Some((Rapoo { out, out_len: w.output_len, answer, answer_len: a.feature_len, status, stages: None, count: SLOTS, active: 0, rate: None, pushed: None }, info))
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
        // The stage count decides how many bytes a table read has to ask for.
        let count = self.register(AT_COUNT, BANK_SETTINGS, 1).map(|d| (d[0] as usize + 1).clamp(1, SLOTS));
        let stage = self.register(AT_STAGE, BANK_SETTINGS, 1).and_then(|d| usize::try_from(d[0]).ok()).filter(|i| *i < SLOTS);
        let used = count.unwrap_or(SLOTS);
        let table = self.register(AT_DPI_X, BANK_SETTINGS, used * 2).as_deref().and_then(|b| parse_table(b, used));
        if let (Some(t), Some(c)) = (table, count) {
            self.stages = Some(t);
            self.count = c;
            self.active = stage.unwrap_or(0).min(c - 1);
        }
        self.listen();
        // What the registers answered, or, when they did not, what the mouse
        // broadcast on its own.
        let dpi = self.stages.map(|t| t[self.active]).or_else(|| self.pushed.map(|b| b.dpi as u32));
        let stage = self.stages.map(|_| self.active as u8 + 1).or_else(|| self.pushed.map(|b| b.stage + 1));
        let rate = self.rate();
        (dpi.is_some() || rate.is_some() || self.pushed.is_some()).then_some(Reading { battery: self.pushed.map(|b| b.battery), dpi, rate_hz: rate, stage, ..Reading::default() })
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
        let active = self.active.min(self.count - 1);
        stages[active] = dpi;
        let bytes = table_bytes(&stages[..self.count]);
        self.wake();
        if !self.poke(AT_DPI_X, BANK_SETTINGS, &bytes) || !self.poke(AT_DPI_Y, BANK_SETTINGS, &bytes) {
            return false;
        }
        // A Hub writes the number of stages and the active one along with them.
        let _ = self.poke(AT_COUNT, BANK_SETTINGS, &[(self.count - 1) as u8]);
        let _ = self.poke(AT_STAGE, BANK_SETTINGS, &[active as u8]);
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
        let ok = self.poke(AT_RATE, BANK_SETTINGS, &[code]);
        // The mouse needs a moment to switch over to what it reports at.
        std::thread::sleep(RATE_SETTLE);
        self.rate = if ok { Some(hz) } else { self.rate };
        ok
    }

    /// The rate register, keeping the last value that was read and falling
    /// back to the rate code the mouse named in a broadcast.
    fn rate(&mut self) -> Option<u32> {
        self.rate = self.register(AT_RATE, BANK_SETTINGS, 1).and_then(|d| rate_hz(d[0])).or_else(|| self.pushed.and_then(|b| b.rate)).or(self.rate);
        self.rate
    }

    /// `A5 A3` with no data: the frame A Hub sends when it takes a mouse. Sent
    /// before a write only. Reading registers does not need it, and a probe of
    /// an unknown mouse writes nothing.
    fn wake(&mut self) {
        let _ = self.out.write(&fit(&frame(OP_WAKE, 0, 0, &[]), self.out_len), WRITE_WAIT);
    }

    /// One register: the `A5 A4` frame out, then the answer on feature report
    /// 08. The mouse writes its answer into that report; it is no input report.
    /// A request that goes unanswered is asked again, which is what a mouse
    /// that is switching its report rate or waking up needs.
    fn register(&mut self, addr: u8, bank: u8, len: usize) -> Option<Vec<u8>> {
        for _ in 0..ASK_TRIES {
            if !self.out.write(&fit(&ask(addr, bank, len), self.out_len), WRITE_WAIT) {
                return None;
            }
            for _ in 0..ANSWER_TRIES {
                std::thread::sleep(GAP);
                let mut buf = vec![0u8; self.answer_len];
                buf[0] = REPORT_ANSWER;
                if !self.answer.get_feature(&mut buf) {
                    continue;
                }
                // A read that was never answered is all zeros.
                if buf[ANSWER_HEAD..].iter().any(|&b| b != 0) {
                    return answer(&buf, len);
                }
            }
        }
        None
    }

    /// `A5 A5` write of a register; the mouse does not acknowledge it.
    fn poke(&mut self, addr: u8, bank: u8, data: &[u8]) -> bool {
        let ok = self.out.write(&fit(&frame(OP_WRITE, addr, bank, data), self.out_len), WRITE_WAIT);
        std::thread::sleep(GAP);
        ok
    }

    /// Status broadcasts come on their own, so every poll reads the collection
    /// until it runs out of them, and keeps what the last one named.
    fn listen(&mut self) {
        let Some((ch, len)) = self.status.as_mut() else {
            return;
        };
        for _ in 0..STATUS_TRIES {
            match ch.read(*len, STATUS_WAIT).as_deref().and_then(broadcast) {
                Some(b) => self.pushed = Some(b),
                None => break,
            }
        }
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

/// A read: its `len` byte says how many bytes the answer should carry, and the
/// frame itself carries no data of its own.
fn ask(addr: u8, bank: u8, len: usize) -> [u8; FRAME_LEN] {
    let mut b = frame(OP_READ, addr, bank, &[]);
    b[3] = len.min(MAX_DATA) as u8;
    b
}

/// A frame as a report of `size` bytes, which is what a collection takes: the
/// head of the frame stays, the rest of the report is zeros.
fn fit(f: &[u8], size: usize) -> Vec<u8> {
    let n = size.min(f.len());
    let mut b = vec![0u8; size];
    b[..n].copy_from_slice(&f[..n]);
    b
}

/// Data behind the header of an answer. The values are checked per register
/// after this, so an answer that was never written reads as zeros.
fn answer(r: &[u8], len: usize) -> Option<Vec<u8>> {
    (r.len() >= ANSWER_HEAD + len).then(|| r[ANSWER_HEAD..ANSWER_HEAD + len].to_vec())
}

/// The status broadcast of report 07: `[id] | head | dpi (2 B LE) | dpi (2 B)
/// | stage | percent | .. | timer`. A Hub and ClickSync look for a `20 00`
/// head; a VT1 Gen 2 sends `10 01`, so the head is not what says a report is a
/// status - a DPI in range, a stage that exists and a percent that is a percent
/// are. A report id may or may not stand in front of the payload.
fn broadcast(r: &[u8]) -> Option<Broadcast> {
    // The id of the collection first when it stands in front of the payload,
    // the payload alone otherwise.
    let order = if r.first() == Some(&REPORT_BROADCAST) { [1usize, 0] } else { [0, 1] };
    order.into_iter().find_map(|at| {
        let p = r.get(at..)?;
        if p.len() < 8 {
            return None;
        }
        let (dpi, stage, percent) = (u16::from_le_bytes([p[2], p[3]]), p[6], p[7]);
        let x = u32::from(dpi);
        if x < DPI_MIN || x > DPI_MAX || stage >= SLOTS as u8 || percent > 100 {
            return None;
        }
        Some(Broadcast { battery: percent, dpi, stage, rate: p.get(10).copied().and_then(rate_hz), timer: *p.last()? })
    })
}

fn rate_hz(code: u8) -> Option<u32> {
    RATES.iter().find(|(_, c)| *c == code).map(|(hz, _)| *hz)
}

fn rate_code(hz: u32) -> Option<u8> {
    RATES.iter().find(|(x, _)| *x == hz).map(|(_, c)| *c)
}

/// The DPI stages of 16 bits each, little endian, for the `count` stages the
/// mouse uses. A stage of zero is no stage: an answer of zeros reads as no
/// table. Stages the mouse does not use repeat the last one it named.
fn parse_table(b: &[u8], count: usize) -> Option<[u32; SLOTS]> {
    if count == 0 || b.len() < count * 2 {
        return None;
    }
    let mut out = [0u32; SLOTS];
    for (i, v) in out.iter_mut().take(count).enumerate() {
        let x = u16::from_le_bytes([b[i * 2], b[i * 2 + 1]]);
        if x == 0 {
            return None;
        }
        *v = x as u32;
    }
    let last = out[count - 1];
    for v in out.iter_mut().skip(count) {
        *v = last;
    }
    Some(out)
}

/// The stages as they go into a table register.
fn table_bytes(stages: &[u32]) -> Vec<u8> {
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

        // A read asks for the answer it wants: the length byte counts the bytes
        // of the answer, not the data of the frame.
        assert_eq!(ask(AT_DPI_X, BANK_SETTINGS, 12)[..6], [0x06, 0xA5, 0xA4, 12, 0x88, 0x08]);
        assert_eq!(ask(AT_RATE, BANK_SETTINGS, 1)[..6], [0x06, 0xA5, 0xA4, 1, 0x80, 0x08]);
        assert_eq!(ask(AT_STAGE, BANK_SETTINGS, 99)[3], MAX_DATA as u8, "a read asks for at most a frame of data");

        assert_eq!(frame(OP_WAKE, 0, 0, &[])[..8], [0x06, 0xA5, 0xA3, 0, 0, 0, 0, 0]);

        let long = frame(OP_WRITE, AT_DPI_X, BANK_SETTINGS, &[7u8; 40]);
        assert_eq!((long[3] as usize, long.len()), (MAX_DATA, FRAME_LEN), "25 data bytes at most");

        let mut b = vec![0u8; 64];
        b[..8].copy_from_slice(&frame(OP_WAKE, 0, 0, &[])[..8]);
        assert_eq!(fit(&frame(OP_WAKE, 0, 0, &[]), 64), b, "a longer report keeps the head");
        assert_eq!(fit(&frame(OP_WRITE, AT_RATE, BANK_SETTINGS, &[1, 2, 3]), 6).len(), 6);
    }

    #[test]
    fn answers_carry_the_register_of_the_read() {
        let mut a = [0u8; FRAME_LEN];
        a[0] = REPORT_ANSWER;
        a[..4].copy_from_slice(&[1, 0, 0, 0]);
        a[4] = 0x82;
        assert_eq!(answer(&a, 1), Some(vec![0x82]));
        assert_eq!(answer(&a, 28).map(|d| d.len()), Some(28), "28 bytes behind the header");
        assert_eq!(answer(&a, 30), None);
    }

    #[test]
    fn rate_codes_are_known_both_ways() {
        for (hz, code) in RATES {
            assert_eq!((rate_hz(code), rate_code(hz)), (Some(hz), Some(code)));
        }
        assert_eq!(rate_hz(0), None, "no answer is not 1000 Hz");
        assert_eq!(rate_code(8001), None);
    }

    #[test]
    fn dpi_tables_of_sixteen_bits() {
        let stages = [800u32, 1600, 3200, 6400, 12_800, 26_000];
        let b = table_bytes(&stages);
        assert_eq!(b.len(), SLOTS * 2);
        assert_eq!(parse_table(&b, SLOTS), Some(stages));
        assert_eq!(b[..2], [0x20, 0x03], "little endian");
        assert_eq!(parse_table(&b[..4], SLOTS), None, "a short table is no table");
        // Four stages of a mouse that uses four: the other two repeat the last.
        assert_eq!(parse_table(&b[..8], 4), Some([800, 1600, 3200, 6400, 6400, 6400]));
        let mut blank = b.clone();
        blank[4..6].copy_from_slice(&[0, 0]);
        assert_eq!(parse_table(&blank, SLOTS), None, "zeros are an unanswered read");
        assert_eq!(table_bytes(&[0, 0, 0, 0, 0, 0])[..2], [1, 0], "a stage is never written as zero");
        assert_eq!(parse_table(&b, 0), None, "no stages is no table");
    }

    #[test]
    fn status_broadcasts_are_read_with_and_without_a_report_id() {
        // The broadcast a VT1 Gen 2 really sends: 91 percent, 450 DPI on each
        // axis, stage index 1, the 4000 Hz it reports at, and a last byte that
        // counted down between two broadcasts eleven seconds apart.
        let r = [0x07, 0x10, 0x01, 0xc2, 0x01, 0xc2, 0x01, 0x01, 0x5b, 0x00, 0x03, 0x82, 0x00, 0x15, 0x46, 0x01, 0x00, 0x03, 0xc2];
        assert_eq!(broadcast(&r), Some(Broadcast { battery: 91, dpi: 450, stage: 1, rate: Some(4000), timer: 0xc2 }));
        // The `20 00` head A Hub and ClickSync describe: 1500 DPI, stage 1, 100 percent.
        let s = [0x07, 0x20, 0x00, 0xDC, 0x05, 0x00, 0x00, 0x01, 0x64];
        assert_eq!(broadcast(&s).map(|b| (b.battery, b.dpi, b.stage, b.rate)), Some((100, 1500, 1, None)));
        // The same with no report id standing in front of it.
        assert_eq!(broadcast(&s[1..]).map(|b| (b.battery, b.dpi, b.stage)), Some((100, 1500, 1)));
        // Reports that are not a status broadcast stay unread.
        assert_eq!(broadcast(&[0x07, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 50]), None, "a DPI of zero is no status");
        assert_eq!(broadcast(&[0x07, 0x20, 0x00, 0xDC, 0x05, 0x00, 0x00, 0x01, 130]), None, "no percent above 100");
        assert_eq!(broadcast(&[0x07, 0x20, 0x00, 0xDC, 0x05, 0x00, 0x00, 0x09, 64]), None, "no stage that cannot exist");
        assert_eq!(broadcast(&[0x07, 0x20, 0, 0]), None, "too short for a percent");
    }

    /// The answers of the connected VT1 Gen 2, run through the parsing of the
    /// driver: 4000 Hz, two DPI stages, the second one selected, and a table
    /// the mouse fills with the 800 that A Hub pads unused stages with.
    #[test]
    fn answers_of_a_vt1_gen_2() {
        assert_eq!(rate_hz(answer(&[1, 0, 0, 0, 0x82, 0x03, 0xff, 0xff], 4).unwrap()[0]), Some(4000), "the rate register answers 4000 Hz");
        assert_eq!((answer(&[1, 0, 0, 0, 0x01, 0x00, 0x01, 0x00], 1), answer(&[1, 0, 0, 0, 0x01, 0x00, 0x02, 0x01], 1)), (Some(vec![1]), Some(vec![1])), "two stages, the second selected");
        let table = answer(&[1, 0, 0, 0, 0xb0, 0x04, 0xc2, 0x01, 0x20, 0x03, 0x20, 0x03, 0x20, 0x03, 0x20, 0x03], SLOTS * 2).unwrap();
        assert_eq!(parse_table(&table, SLOTS), Some([1200, 450, 800, 800, 800, 800]));
        assert_eq!(parse_table(&table[..4], 2), Some([1200, 450, 450, 450, 450, 450]), "read at the two stages in use");
    }

    #[test]
    fn the_catalogue_names_the_mouse_the_driver_opens() {
        let c = crate::devices::listed_as(VID, Some(VT1_GEN2), "VT1 Gen 2").unwrap();
        assert_eq!(c.family, crate::devices::Family::Rapoo);
        assert_eq!(c.dpi, Some(DpiRange::Steps { min: DPI_MIN, max: DPI_MAX, step: DPI_STEP }));
        assert_eq!(c.rates, RATES.iter().map(|(hz, _)| *hz).collect::<Vec<u32>>());
    }

    /// The collections of a connected Rapoo device, as Whisker sees them:
    /// `cargo test -- --ignored --nocapture print_rapoo_collections`.
    #[test]
    #[ignore]
    fn print_rapoo_collections() {
        for i in crate::hid_link::raw::interfaces(VID) {
            println!("{:04X}:{:04X} page {:04X} usage {:04X} out {:3} feature {:3} {:?}", i.info.vid, i.info.pid, i.usage_page, i.usage, i.output_len, i.feature_len, i.info.product);
        }
    }

    /// Everything one exchange with a connected mouse shows: the frames sent,
    /// and every report that answers them. Reads registers only. Move the mouse
    /// while it runs, so the receiver is awake and broadcasting.
    /// `cargo test -- --ignored --nocapture print_rapoo_exchange`.
    #[test]
    #[ignore]
    fn print_rapoo_exchange() {
        fn hex(b: &[u8]) -> String {
            b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
        }
        // The vendor collections of the connected mouse, and what each carries.
        let mut opened: Vec<(u16, usize, usize, usize, Channel)> = Vec::new();
        for d in crate::hid_link::mouse_devices() {
            if d.info.vid != VID {
                continue;
            }
            for (i, l) in &d.collections {
                if i.usage_page != PAGE {
                    continue;
                }
                println!("usage {:04X}: in {:3} out {:3} feature {:3} ids {:?}", i.usage, l.input_len, l.output_len, l.feature_len, l.report_ids);
                if let Some(c) = Channel::open(&i.path) {
                    opened.push((i.usage, l.input_len, l.output_len, l.feature_len, c));
                }
            }
        }
        let Some((_, _, out_len, _, out)) = opened.iter().find(|(u, _, o, _, _)| *u == USAGE_OUT && *o >= FRAME_LEN) else {
            println!("no 33-byte vendor collection to write to");
            return;
        };
        // Wake it first, as A Hub does when it takes a mouse over.
        for _ in 0..3 {
            let _ = out.write(&fit(&frame(OP_WAKE, 0, 0, &[]), *out_len), WRITE_WAIT);
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("move the mouse now and keep moving it while it reads");
        // Only answers that changed are printed: the mouse answers slowly and
        // the same buffer is read again and again.
        let mut answers: Vec<(u16, u8, Vec<u8>)> = Vec::new();
        for (addr, len) in [(AT_RATE, 1), (AT_COUNT, 1), (AT_STAGE, 1), (AT_DPI_X, SLOTS * 2)] {
            println!("read 0x{addr:02X} {len} B: {}", out.write(&fit(&ask(addr, BANK_SETTINGS, len), *out_len), WRITE_WAIT));
            let mut inputs = 0;
            for _ in 0..40 {
                std::thread::sleep(Duration::from_millis(20));
                for (u, in_len, _, feat_len, c) in &opened {
                    if *feat_len > 0 {
                        let id = if *u == USAGE_ANSWER { REPORT_ANSWER } else { 9 };
                        let mut buf = vec![0u8; *feat_len];
                        buf[0] = id;
                        if c.get_feature(&mut buf) {
                            let fresh = answers.iter().all(|(u2, i2, old)| *u2 != *u || *i2 != id || *old != buf);
                            if fresh {
                                answers.retain(|(u2, i2, _)| *u2 != *u || *i2 != id);
                                answers.push((*u, id, buf.clone()));
                                println!("  usage {u:04X} feature {id:02X}: {}", hex(&buf));
                            }
                        }
                    }
                    if *in_len > 0 && inputs < 4 {
                        if let Some(r) = c.read(*in_len, Duration::from_millis(10)) {
                            inputs += 1;
                            println!("  usage {u:04X} input: {}", hex(&r));
                        }
                    }
                }
            }
        }
        // The same read as a feature request on the answer collection itself.
        if let Some((_, _, _, feat_len, a)) = opened.iter().find(|(u, _, _, f, _)| *u == USAGE_ANSWER && *f >= FRAME_LEN) {
            let f = ask(AT_DPI_X, BANK_SETTINGS, SLOTS * 2);
            let mut buf = vec![0u8; *feat_len];
            buf[0] = REPORT_ANSWER;
            buf[1..].copy_from_slice(&f[1..*feat_len]);
            println!("set feature 8: {}", a.set_feature(&buf));
            std::thread::sleep(GAP);
            let mut back = vec![0u8; *feat_len];
            back[0] = REPORT_ANSWER;
            if a.get_feature(&mut back) {
                println!("  answer after set: {}", hex(&back));
            }
        }
        // Battery comes with a broadcast on its own: two seconds of listening.
        for (u, in_len, _, _, c) in &opened {
            if *u != USAGE_STATUS || *in_len == 0 {
                continue;
            }
            println!("listening for status broadcasts on usage {u:04X}");
            let mut heard: Vec<Vec<u8>> = Vec::new();
            for _ in 0..10 {
                if let Some(r) = c.read(*in_len, Duration::from_millis(200)) {
                    if heard.iter().any(|o| o == &r) {
                        continue;
                    }
                    heard.push(r.clone());
                    let percent = match broadcast(&r) {
                        Some(b) => format!("battery {} dpi {} stage {} at {:?} Hz, timer {:#04x}", b.battery, b.dpi, b.stage + 1, b.rate, b.timer),
                        None => "not a status broadcast".into(),
                    };
                    println!("  status: {}  {percent}", hex(&r));
                }
            }
        }
    }
    /// Bank 08 register by register with the lengths that answer, then a long
    /// listen on the broadcast collection: `cargo test -- --ignored --nocapture
    /// print_rapoo_blocks`. Reads only. Keep the mouse moving while it runs.
    #[test]
    #[ignore]
    fn print_rapoo_blocks() {
        fn hex(b: &[u8]) -> String {
            b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
        }
        let Some((mut d, _)) = Rapoo::open(&Pick::default()) else {
            println!("no Rapoo mouse");
            return;
        };
        for _ in 0..3 {
            d.wake();
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("keep the mouse moving while it reads");
        let mut raw = vec![0u8; d.answer_len];
        // A read of one byte answers with four, a table read with the table:
        // every register here is asked at the size it answers at.
        for (addr, bank, len) in [(0x80u8, 0x08u8, 4), (0x84, 8, 4), (AT_DPI_X, 8, SLOTS * 2), (0x90, 8, 4), (0x94, 8, 4), (AT_STAGE, 8, 4), (0x9C, 8, 4), (0xA0, 8, 4), (0xC0, 8, 4), (0xC2, 8, 4), (0xC4, 8, 4), (AT_DPI_Y, 8, SLOTS * 2), (0xDC, 8, 4), (0xE0, 8, 4), (0x04, 0x06, 4)] {
            let mut got: Option<Vec<u8>> = None;
            for _ in 0..3 {
                let _ = d.out.write(&fit(&ask(addr, bank, len), d.out_len), WRITE_WAIT);
                for _ in 0..8 {
                    std::thread::sleep(GAP);
                    raw = vec![0u8; d.answer_len];
                    raw[0] = REPORT_ANSWER;
                    if d.answer.get_feature(&mut raw) && raw[ANSWER_HEAD..].iter().any(|b| *b != 0) {
                        got = Some(raw.clone());
                        break;
                    }
                }
                if got.is_some() {
                    break;
                }
            }
            match got {
                Some(r) => println!("bank {bank:02X} reg 0x{addr:02X} ask {len:>2} B: {}", hex(&r[1..])),
                None => println!("bank {bank:02X} reg 0x{addr:02X} ask {len:>2} B: no answer"),
            }
        }
        let Some((ch, slen)) = d.status.take() else {
            println!("no status collection");
            return;
        };
        println!("ten seconds of broadcasts on {slen} B - keep the mouse moving");
        let end = std::time::Instant::now() + Duration::from_secs(10);
        let mut heard: Vec<Vec<u8>> = Vec::new();
        while std::time::Instant::now() < end {
            if let Some(r) = ch.read(slen, Duration::from_millis(250)) {
                let named = match broadcast(&r) {
                    Some(b) => format!("battery {} dpi {} stage {} at {:?} Hz, timer {:#04x}", b.battery, b.dpi, b.stage + 1, b.rate, b.timer),
                    None => "not a status broadcast".into(),
                };
                println!("  {slen} B: {}   {named}", hex(&r));
                heard.push(r);
            }
        }
        if heard.len() > 1 {
            let changing: Vec<usize> = (0..heard[0].len()).filter(|i| heard.iter().any(|r| r.get(*i) != heard[0].get(*i))).collect();
            println!("{} broadcasts, bytes that differ: {changing:?}", heard.len());
        }
    }

    /// The writes of the driver, on a mouse that is put back as it was found,
    /// with two register reads printed after every step so that it can be seen
    /// when the mouse stops answering. It does write to the mouse while it runs.
    /// `cargo test -- --ignored --nocapture print_rapoo_writes`.
    #[test]
    #[ignore]
    fn print_rapoo_writes() {
        fn shows(d: &mut Rapoo, what: &str) {
            println!("{what}: rate {:?}  count {:?}  stage {:?}", d.register(AT_RATE, BANK_SETTINGS, 1), d.register(AT_COUNT, BANK_SETTINGS, 1), d.register(AT_STAGE, BANK_SETTINGS, 1));
        }
        let Some((mut d, info)) = Rapoo::open(&Pick::default()) else {
            println!("no Rapoo mouse");
            return;
        };
        println!("{:04X}:{:04X} {} - keep it moving; it writes and puts back", info.vid, info.pid, info.product);
        for _ in 0..3 {
            d.wake();
            std::thread::sleep(Duration::from_millis(100));
        }
        // 1) The DPI of the stage in use, first, because that is the write
        //    Whisker is for: the table as it was read with one stage 500 up,
        //    then the value it had.
        let read = d.read();
        let (dpi, stage) = (read.as_ref().and_then(|r| r.dpi), read.as_ref().and_then(|r| r.stage));
        println!("read: dpi {dpi:?} stage {stage:?}  table 12 B {:?}  broadcast {:?}", d.register(AT_DPI_X, BANK_SETTINGS, SLOTS * 2), d.pushed);
        match dpi {
            Some(dpi) => {
                let other = (dpi + 500).min(DPI_MAX);
                let wrote = d.set_dpi(other);
                let back = d.read();
                println!("stage {stage:?} at {dpi} dpi -> wrote {other}: {wrote} -> read back dpi {:?} stage {:?}", back.as_ref().and_then(|r| r.dpi), back.as_ref().and_then(|r| r.stage));
                shows(&mut d, "after the DPI write");
                let wrote = d.set_dpi(dpi);
                let back = d.read();
                println!("  back to {dpi}: {wrote} -> read back dpi {:?} stage {:?}", back.as_ref().and_then(|r| r.dpi), back.as_ref().and_then(|r| r.stage));
                shows(&mut d, "after the DPI write back");
            }
            None => println!("no DPI answered, so no DPI write was tried"),
        }
        // 2) The report rate, and only when the rate it runs at is known: the
        //    one above it (or the one below when it is at the top), then the
        //    one it had. Nothing is written on a rate that was not read.
        let rate = d.register(AT_RATE, BANK_SETTINGS, 1).and_then(|v| rate_hz(v[0]));
        match rate {
            Some(rate) => {
                let other = RATES.iter().map(|(hz, _)| *hz).filter(|hz| *hz != rate).next_back().unwrap_or(1000);
                let wrote = d.set_rate(other);
                println!("0x80 rate {rate} -> wrote {other}: {wrote} -> read {:?}", d.register(AT_RATE, BANK_SETTINGS, 1).and_then(|v| rate_hz(v[0])));
                shows(&mut d, "after the rate write");
                let wrote = d.set_rate(rate);
                println!("  back to {rate}: {wrote} -> read {:?}", d.register(AT_RATE, BANK_SETTINGS, 1).and_then(|v| rate_hz(v[0])));
                shows(&mut d, "after the rate write back");
            }
            None => println!("0x80 rate: no answer, so no rate was written"),
        }
        // 3) The minutes of idle that put the mouse to sleep, register `0xC2`.
        //    Nobody notices them, so they are the safest way to see whether a
        //    write reached the mouse at all.
        const SLEEP: u8 = 0xC2;
        match d.register(SLEEP, BANK_SETTINGS, 1).map(|v| v[0]) {
            Some(minutes) => {
                let other = if minutes > 60 { minutes - 30 } else { minutes + 30 };
                let wrote = d.poke(SLEEP, BANK_SETTINGS, &[other]);
                println!("0xC2 sleep {minutes} min -> wrote {other}: {wrote} -> read {:?}", d.register(SLEEP, BANK_SETTINGS, 1).map(|v| v[0]));
                let wrote = d.poke(SLEEP, BANK_SETTINGS, &[minutes]);
                println!("  back to {minutes}: {wrote} -> read {:?}", d.register(SLEEP, BANK_SETTINGS, 1).map(|v| v[0]));
            }
            None => println!("0xC2 sleep: no answer"),
        }
        println!("last broadcast: {:?}", d.pushed);
    }


    /// Put the connected mouse to the report rate named in the
    /// `WHISKER_RAPOO_RATE` environment variable, printing what it reads before
    /// and after. `WHISKER_RAPOO_RATE=4000; cargo test -- --ignored --nocapture
    /// print_rapoo_rate`
    #[test]
    #[ignore]
    fn print_rapoo_rate() {
        let wanted: u32 = std::env::var("WHISKER_RAPOO_RATE").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(1000);
        let Some((mut d, info)) = Rapoo::open(&Pick::default()) else {
            println!("no Rapoo mouse");
            return;
        };
        println!("{:04X}:{:04X} {}", info.vid, info.pid, info.product);
        let before = d.register(AT_RATE, BANK_SETTINGS, 1).and_then(|v| rate_hz(v[0]));
        let wrote = d.set_rate(wanted);
        println!("0x80 rate {before:?} -> wrote {wanted}: {wrote} -> read {:?}", d.register(AT_RATE, BANK_SETTINGS, 1).and_then(|v| rate_hz(v[0])));
        println!("reading: {:?}", d.read());
    }


}
