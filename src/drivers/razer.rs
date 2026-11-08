//! Razer mice (protocol as documented by OpenRazer). 90-byte feature
//! report, report id 0:
//!
//! ```text
//!  0       1    2..3       4      5     6      7    8..87   88   89
//! status | tid | remaining | proto | size | class | id | args | crc | 0
//! crc = xor of bytes 2..=87; reply status 02 = ok, 01 = busy
//! ```

use super::{Ability, DpiRange, Ident, Reading};
use crate::hid_link::raw::{Channel, interfaces};
use std::time::Duration;

pub const VID: u16 = 0x1532;

const REPORT_LEN: usize = 90;
/// With the leading report id byte.
const BUF_LEN: usize = REPORT_LEN + 1;
const ARGS: usize = 80;
const STATUS_OK: u8 = 0x02;
const STATUS_BUSY: u8 = 0x01;
const VARSTORE: u8 = 0x01;
const REPLY_WAIT: Duration = Duration::from_millis(30);
const BUSY_RETRIES: usize = 5;
/// Transaction ids used by different generations, newest first.
const TRANSACTION_IDS: [u8; 3] = [0x1F, 0xFF, 0x3F];

/// Report rate codes (= interval in ms).
const RATES: [(u32, u8); 3] = [(125, 0x08), (500, 0x02), (1000, 0x01)];
const DEFAULT_DPI: DpiRange = DpiRange::Steps { min: 100, max: 16000, step: 50 };

/// Request: `[report id 0, report…]` ready for HidD_SetFeature.
pub fn request(tid: u8, class: u8, id: u8, size: u8, args: &[u8]) -> [u8; BUF_LEN] {
    let mut b = [0u8; BUF_LEN];
    let r = &mut b[1..];
    r[1] = tid;
    r[5] = size;
    r[6] = class;
    r[7] = id;
    let n = args.len().min(ARGS);
    r[8..8 + n].copy_from_slice(&args[..n]);
    r[88] = crc(r);
    b
}

/// XOR of bytes 2..=87 of the 90-byte report.
pub fn crc(r: &[u8]) -> u8 {
    r[2..88].iter().fold(0, |a, b| a ^ b)
}

/// Reply args if the status is OK and it answers `class`/`id`.
pub fn parse_reply(b: &[u8; BUF_LEN], class: u8, id: u8) -> Result<[u8; ARGS], u8> {
    let r = &b[1..];
    if r[0] != STATUS_OK {
        return Err(r[0]);
    }
    if r[6] != class || r[7] != id {
        return Err(0);
    }
    let mut a = [0u8; ARGS];
    a.copy_from_slice(&r[8..88]);
    Ok(a)
}

pub struct Razer {
    ch: Channel,
    tid: u8,
}

impl Razer {
    /// Opens the collection that carries the 90-byte feature report and
    /// finds the transaction id this model answers to.
    pub fn open() -> Option<(Razer, crate::hid_link::DeviceInfo)> {
        const DESKTOP: u16 = 0x01;
        const MOUSE: u16 = 0x02;
        let all = interfaces(VID);
        // Keyboards share the protocol: prefer a product that is also a mouse.
        let is_mouse = |pid: u16| all.iter().any(|i| i.info.pid == pid && i.usage_page == DESKTOP && i.usage == MOUSE);
        let mut control: Vec<_> = all.iter().filter(|i| i.feature_len == BUF_LEN).collect();
        control.sort_by_key(|i| !is_mouse(i.info.pid));
        let iface = (*control.first()?).clone();
        let ch = Channel::open(&iface.path)?;
        let mut d = Razer { ch, tid: TRANSACTION_IDS[0] };
        let tid = TRANSACTION_IDS.into_iter().find(|&t| {
            d.tid = t;
            d.call(0x00, 0x81, 2, &[]).is_ok()
        })?;
        d.tid = tid;
        Some((d, iface.info))
    }

    fn call(&self, class: u8, id: u8, size: u8, args: &[u8]) -> Result<[u8; ARGS], u8> {
        let req = request(self.tid, class, id, size, args);
        if !self.ch.set_feature(&req) {
            return Err(0);
        }
        for _ in 0..BUSY_RETRIES {
            std::thread::sleep(REPLY_WAIT);
            let mut buf = [0u8; BUF_LEN];
            if !self.ch.get_feature(&mut buf) {
                return Err(0);
            }
            match parse_reply(&buf, class, id) {
                Err(STATUS_BUSY) => continue,
                other => return other,
            }
        }
        Err(STATUS_BUSY)
    }

    pub fn ident(&mut self) -> Ident {
        let mut id = Ident { protocol: "Razer".into(), ..Ident::default() };
        if let Ok(a) = self.call(0x00, 0x81, 2, &[]) {
            id.firmware = format!("v{}.{:02}", a[0], a[1]);
        }
        if let Ok(a) = self.call(0x00, 0x82, 0x16, &[]) {
            id.serial = String::from_utf8_lossy(&a[..0x16]).trim_end_matches('\0').trim().to_string();
        }
        id
    }

    pub fn ability(&mut self) -> Ability {
        Ability {
            dpi: Some(DEFAULT_DPI),
            rates: RATES.iter().map(|(hz, _)| *hz).collect(),
            battery: self.call(0x07, 0x80, 2, &[]).is_ok(),
        }
    }

    /// `None` when the mouse does not answer (asleep or off).
    pub fn read(&mut self) -> Option<Reading> {
        let dpi = self.call(0x04, 0x85, 7, &[VARSTORE]).ok()?;
        let mut r = Reading { dpi: Some(u16::from_be_bytes([dpi[1], dpi[2]]) as u32), ..Reading::default() };

        if let Ok(a) = self.call(0x00, 0x85, 1, &[]) {
            r.rate_hz = RATES.iter().find(|(_, code)| *code == a[0]).map(|(hz, _)| *hz);
        }
        if let Ok(a) = self.call(0x07, 0x80, 2, &[]) {
            r.battery = Some(((a[1] as u32 * 100 + 127) / 255) as u8);
            r.charging = self.call(0x07, 0x84, 2, &[]).is_ok_and(|c| c[1] != 0);
        }
        Some(r)
    }

    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        let v = (dpi.min(u16::MAX as u32) as u16).to_be_bytes();
        self.call(0x04, 0x05, 7, &[VARSTORE, v[0], v[1], v[0], v[1], 0, 0]).is_ok()
    }

    pub fn set_rate(&mut self, hz: u32) -> bool {
        let Some((_, code)) = RATES.iter().min_by_key(|(r, _)| r.abs_diff(hz)) else {
            return false;
        };
        self.call(0x00, 0x05, 1, &[*code]).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_crc_and_layout() {
        // Set DPI 800 with transaction id 0x1F.
        let b = request(0x1F, 0x04, 0x05, 7, &[0x01, 0x03, 0x20, 0x03, 0x20, 0, 0]);
        assert_eq!(&b[..11], &[0x00, 0x00, 0x1F, 0, 0, 0, 7, 0x04, 0x05, 0x01, 0x03]);
        let expect = 7 ^ 0x04 ^ 0x05 ^ 0x01 ^ 0x03 ^ 0x20 ^ 0x03 ^ 0x20;
        assert_eq!(b[89], expect);
    }

    #[test]
    fn reply_status_and_match() {
        let mut b = request(0x1F, 0x00, 0x81, 2, &[1, 2]);
        b[1] = STATUS_OK;
        assert_eq!(parse_reply(&b, 0x00, 0x81).unwrap()[..2], [1, 2]);
        assert_eq!(parse_reply(&b, 0x07, 0x80), Err(0));
        b[1] = STATUS_BUSY;
        assert_eq!(parse_reply(&b, 0x00, 0x81), Err(STATUS_BUSY));
    }
}
