//! Rapoo's second generation of gaming mice (nRF54 firmware, vendor id 24AE)
//! speaks the "A5A5" register protocol of A Hub and of the hub.rapoo.cn web
//! configurator. Protocol facts from the WebHID driver study ClickSync
//! (github.com/Nuitfanee/ClickSync, `src/protocols/protocol_api_rapoo.js`,
//! GPL-2.0: used as information only, no code copied), which took them from
//! Rapoo's own configurator, and from a Whisker mouse report of the VT1 Gen 2
//! (issue #3). Experimental: probed against a connected VT1 Gen 2 receiver,
//! which takes every frame we send but has not answered a register read yet —
//! the mouse was asleep for every probe. What is open is in `docs/rapoo.md`.
//!
//! ```text
//!  FF00:000E  output report 06 (33 B)    the frames go out
//!  FF00:000F  feature report 08 (33 B)   the answer to a register read
//!  FF00:0002  input report 07 (19 B)     status broadcast, carries the battery
//!
//!  write   06 | A5 A5 | len | addr | bank | 00 00 | data[0..len]
//!  read    06 | A5 A4 | len | addr | bank | 00 00 00   then GetFeature 08
//!  wake    06 | A5 A3 | 00 ...           what A Hub sends when it takes a mouse
//!  answer  00 | A5 A4 | len | data[0..len]              four bytes of header
//!  status  07 | 20 00 | dpi (2 B LE) | .. | stage | percent
//!
//!  bank 08 settings  0x80 report rate   0x88 DPI stages X   0xC8 DPI stages Y
//!                    0x96 stage count   0x98 active stage   0xDC..0xE2 speed
//!                    profile, one register per report rate
//!  bank 06 buttons   0x04.. one entry per button
//!  bank 00 radio     pairing, MAC and encryption keys
//! ```
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
/// First payload byte of a status broadcast.
const MARK_STATUS: u8 = 0x20;

/// Frames and answers are 33 bytes with the report id, and so are the
/// broadcasts of the VT1 Gen 2; a driver never assumes a size, it takes the
/// one the collection reports.
const FRAME_LEN: usize = 33;
/// Report id, `A5 op`, len, addr, bank and two zero bytes.
const HEAD_LEN: usize = 8;
/// The four bytes an answer starts with, before the register values. The mouse
/// does not put a report id in front of them, so they are the first four bytes
/// of what a feature read returns.
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
    /// Percent of the last status broadcast: they come when the mouse sends
    /// them, so the driver keeps what it has heard.
    battery: Option<u8>,
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
        Some((Rapoo { out, out_len: w.output_len, answer, answer_len: a.feature_len, status, stages: None, count: SLOTS, active: 0, rate: None, battery: None }, info))
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
        let dpi = self.stages.map(|t| t[self.active]);
        // Only a mouse whose table we have got a stage worth showing.
        let stage = self.stages.map(|_| self.active as u8 + 1);
        let rate = self.rate();
        (rate.is_some() || dpi.is_some() || self.battery.is_some()).then_some(Reading { battery: self.battery, dpi, rate_hz: rate, stage, ..Reading::default() })
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
        self.poke(AT_RATE, BANK_SETTINGS, &[code])
    }

    /// The rate register, keeping the last value that was read.
    fn rate(&mut self) -> Option<u32> {
        self.rate = self.register(AT_RATE, BANK_SETTINGS, 1).and_then(|d| rate_hz(d[0])).or(self.rate);
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
    fn register(&mut self, addr: u8, bank: u8, len: usize) -> Option<Vec<u8>> {
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
            // A read that was never answered is all zeros. A receiver that
            // needs a moment for the mouse asks for it again on the next try.
            if buf[ANSWER_HEAD..].iter().any(|&b| b != 0) {
                return answer(&buf, len);
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
    /// until it runs out of them, and keeps the battery of the last one.
    fn listen(&mut self) {
        let Some((ch, len)) = self.status.as_mut() else {
            return;
        };
        for _ in 0..STATUS_TRIES {
            match ch.read(*len, STATUS_WAIT).as_deref().and_then(broadcast) {
                Some(p) => self.battery = Some(p),
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

/// Battery percent of a status broadcast: `20 00 dpi (2 B) .. stage percent`,
/// the percent at the eighth byte of the payload, whether or not a report id
/// stands in front of it.
fn broadcast(r: &[u8]) -> Option<u8> {
    let head = match (r.first(), r.get(1)) {
        (Some(&MARK_STATUS), _) => 0,
        (_, Some(&MARK_STATUS)) => 1,
        _ => return None,
    };
    (r.len() >= head + 8).then(|| r[head + 7].min(100))
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
    fn battery_of_a_status_broadcast() {
        // 20 00 05 DC .. 01 64: 1500 DPI, stage 1, 100 percent, id 7 in front.
        let s = [0x07, 0x20, 0x00, 0xDC, 0x05, 0x00, 0x00, 0x01, 0x64];
        assert_eq!(broadcast(&s), Some(100));
        // The same broadcast with no id in front of it.
        assert_eq!(broadcast(&s[1..]), Some(100));
        // A report that is not a status broadcast is not a battery.
        assert_eq!(broadcast(&[0x03, 0x21, 0, 0, 0, 0, 0, 0, 50]), None);
        assert_eq!(broadcast(&[0x07, 0x20, 0, 0, 0, 0, 0, 0, 130]), Some(100), "never more than full");
        assert_eq!(broadcast(&[0x07, 0x20, 0, 0, 0]), None, "too short for a percent");
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
                    let percent = broadcast(&r).map(|p| p.to_string()).unwrap_or_else(|| "?".into());
                    println!("  status: {}  battery {percent}", hex(&r));
                }
            }
        }
    }
}
