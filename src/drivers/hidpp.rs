//! Logitech HID++ 2.0 (Lightspeed, Bolt, Unifying receivers and cable).
//! Spec background: Solaar / libratbag docs.
//!
//! ```text
//! long report (20 B):  11 | device | feature index | fn<<4 | sw | params[16]
//! error:               11 | device | FF | feature index | fn<<4|sw | code
//! ```
//! Feature indexes are per device and looked up through the root feature.

use super::{Ability, DpiRange, Ident, Pick, Reading, percent_from_mv};
use crate::hid_link::raw::Channel;
use std::collections::HashMap;
use std::time::Duration;

pub const VID: u16 = 0x046D;

const REPORT_SHORT: u8 = 0x10;
const REPORT_LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
const PARAMS: usize = 16;
const SW_ID: u8 = 0x0A;
const DIRECT: u8 = 0xFF;
const ERROR_IDX: u8 = 0xFF;
const HIDPP10_ERROR: u8 = 0x8F;
const VENDOR_PAGE: u16 = 0xFF00;
const USAGE_LONG: u16 = 0x02;
const TIMEOUT: Duration = Duration::from_millis(300);
const PING_DATA: u8 = 0x5A;
/// DeviceName.getDeviceType kinds that are pointing devices:
/// 3 mouse, 4 trackpad, 5 trackball.
const POINTER_KINDS: [u8; 3] = [3, 4, 5];
const DEVICE_SLOTS: [u8; 7] = [DIRECT, 1, 2, 3, 4, 5, 6];

const ROOT: u8 = 0x00;
const F_FW: u16 = 0x0003;
const F_NAME: u16 = 0x0005;
/// x0003 getFwInfo: entity type in the low nibble; 0 = main application.
const FW_TYPE_MASK: u8 = 0x0F;
const FW_MAIN: u8 = 0;
/// Entities to ask when getDeviceInfo does not answer.
const FW_ENTITIES: u8 = 1;
/// Real devices have 2–4 firmware entities (bootloader, app, radio…).
const FW_MAX_ENTITIES: u8 = 8;
const F_BATTERY_LEVEL: u16 = 0x1000;
const F_BATTERY_VOLTAGE: u16 = 0x1001;
const F_BATTERY_UNIFIED: u16 = 0x1004;
const F_DPI: u16 = 0x2201;
const F_RATE: u16 = 0x8060;

/// Request frame: `[11, device, feature, fn<<4|sw, params…]`.
pub fn request(device: u8, feature: u8, func: u8, params: &[u8]) -> [u8; LONG_LEN] {
    let mut f = [0u8; LONG_LEN];
    f[0] = REPORT_LONG;
    f[1] = device;
    f[2] = feature;
    f[3] = (func << 4) | SW_ID;
    let n = params.len().min(PARAMS);
    f[4..4 + n].copy_from_slice(&params[..n]);
    f
}

/// Reply to our request: `Some(Ok(params))`, `Some(Err(code))`, or `None`
/// for unrelated reports (notifications, other requests).
pub fn parse_reply(r: &[u8], device: u8, feature: u8, func: u8) -> Option<Result<Vec<u8>, u8>> {
    if r.len() < 5 || !(r[0] == REPORT_LONG || r[0] == REPORT_SHORT) || r[1] != device {
        return None;
    }
    let tag = (func << 4) | SW_ID;
    if r[2] == ERROR_IDX && r[3] == feature && r[4] == tag {
        return Some(Err(r.get(5).copied().unwrap_or(0)));
    }
    if r[2] == HIDPP10_ERROR {
        return Some(Err(r.get(4).copied().unwrap_or(0)));
    }
    (r[2] == feature && r[3] == tag).then(|| Ok(r[4..].to_vec()))
}

/// `getSensorDpiList` payload after the sensor byte: big-endian values,
/// `0xE000 | step` marks a range between neighbours, 0 ends the list.
/// E.g. `00C8 E032 3E80` = 200..16000 step 50.
pub fn parse_dpi_list(b: &[u8]) -> Option<DpiRange> {
    const STEP_MARK: u16 = 0xE000;
    let vals: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).take_while(|&v| v != 0).collect();

    if let [min, step, max, ..] = vals[..] {
        if step & STEP_MARK == STEP_MARK {
            return Some(DpiRange::Steps { min: min as u32, max: max as u32, step: (step & !STEP_MARK) as u32 });
        }
    }
    let list: Vec<u32> = vals.iter().filter(|&&v| v & STEP_MARK != STEP_MARK).map(|&v| v as u32).collect();
    (!list.is_empty()).then_some(DpiRange::List(list))
}

/// Report-rate bitmask: bit n set = (n+1) ms supported. Returns Hz ascending.
pub fn parse_rates(mask: u8) -> Vec<u32> {
    let mut hz: Vec<u32> = (0..8).filter(|n| mask & (1 << n) != 0).map(|n| 1000 / (n + 1)).collect();
    hz.sort_unstable();
    hz.dedup();
    hz
}

/// What a paired device is, by DeviceName.getDeviceType.
#[derive(Debug, PartialEq, Eq)]
enum Fit {
    Pointer,
    /// No device type feature: can't tell, usable only as a fallback.
    Unknown,
    /// Keyboard, numpad, remote…: never driven.
    Other,
}

fn fit(kind: Option<u8>) -> Fit {
    match kind {
        Some(k) if POINTER_KINDS.contains(&k) => Fit::Pointer,
        Some(_) => Fit::Other,
        None => Fit::Unknown,
    }
}

pub struct Hidpp {
    ch: Channel,
    device: u8,
    features: HashMap<u16, u8>,
}

impl Hidpp {
    /// Finds a Logitech HID++ 2.0 mouse: every long collection, direct
    /// (cable / Bluetooth) or receiver slots 1..6. A pointing device wins;
    /// a device of unknown type is the fallback; a keyboard or other known
    /// non-pointer is never opened (it must not get DPI or rate commands).
    pub fn open(pick: &Pick) -> Option<(Hidpp, crate::hid_link::DeviceInfo)> {
        let mut fallback = None;
        let candidates = pick.interfaces(VID).into_iter().filter(|i| i.usage_page == VENDOR_PAGE && i.usage == USAGE_LONG && i.output_len >= LONG_LEN);

        for iface in candidates {
            for dev in DEVICE_SLOTS {
                let Some(ch) = Channel::open(&iface.path) else {
                    break;
                };
                let mut d = Hidpp { ch, device: dev, features: HashMap::new() };
                if !d.ping() {
                    continue;
                }
                match fit(d.kind()) {
                    Fit::Pointer => return Some((d, iface.info.clone())),
                    Fit::Unknown => {
                        fallback.get_or_insert((d, iface.info.clone()));
                    }
                    Fit::Other => {}
                }
            }
        }
        fallback
    }

    /// DeviceName.getDeviceType: 0 keyboard … 3 mouse.
    fn kind(&mut self) -> Option<u8> {
        let f = self.feature(F_NAME)?;
        self.call(f, 2, &[])?.first().copied()
    }

    fn call(&self, feature: u8, func: u8, params: &[u8]) -> Option<Vec<u8>> {
        let req = request(self.device, feature, func, params);
        if !self.ch.write(&req, TIMEOUT) {
            return None;
        }
        let dev = self.device;
        self.ch.read_until(LONG_LEN, TIMEOUT, |r| parse_reply(r, dev, feature, func))?.ok()
    }

    /// Root `getProtocolVersion`: answers only for a HID++ 2.0 device.
    fn ping(&self) -> bool {
        self.call(ROOT, 1, &[0, 0, PING_DATA]).is_some_and(|p| p.first().is_some_and(|&major| major >= 2))
    }

    fn feature(&mut self, id: u16) -> Option<u8> {
        if let Some(&i) = self.features.get(&id) {
            return (i != 0).then_some(i);
        }
        let idx = self.call(ROOT, 0, &id.to_be_bytes())?.first().copied()?;
        self.features.insert(id, idx);
        (idx != 0).then_some(idx)
    }

    pub fn ident(&mut self) -> Ident {
        let mut id = Ident { protocol: "Logitech HID++ 2.0".into(), ..Ident::default() };
        if let Some(f) = self.feature(F_NAME) {
            let len = self.call(f, 0, &[]).and_then(|p| p.first().copied()).unwrap_or(0) as usize;
            let mut name = Vec::new();
            while name.len() < len {
                let Some(chunk) = self.call(f, 1, &[name.len() as u8]) else {
                    break;
                };
                name.extend(chunk.iter().take(len - name.len()).take_while(|&&c| c != 0));
                if chunk.first() == Some(&0) {
                    break;
                }
            }
            id.name = String::from_utf8_lossy(&name).trim().to_string();
        }
        if let Some(f) = self.feature(F_FW) {
            // Entity 0 may be the bootloader (M705: "BOT"): find the application.
            // The count comes from the device: cap it, each silent entity costs a timeout.
            let count = self.call(f, 0, &[]).and_then(|p| p.first().copied()).unwrap_or(FW_ENTITIES).min(FW_MAX_ENTITIES);
            id.firmware = (0..count).find_map(|e| self.call(f, 1, &[e]).and_then(|p| main_firmware(&p))).unwrap_or_default();
        }
        id
    }

    pub fn ability(&mut self) -> Ability {
        let mut a = Ability::default();
        if let Some(f) = self.feature(F_DPI) {
            a.dpi = self.call(f, 1, &[0]).and_then(|p| parse_dpi_list(p.get(1..).unwrap_or(&[])));
        }
        if let Some(f) = self.feature(F_RATE) {
            a.rates = self.call(f, 0, &[]).and_then(|p| p.first().copied()).map(parse_rates).unwrap_or_default();
        }
        a.battery = [F_BATTERY_UNIFIED, F_BATTERY_LEVEL, F_BATTERY_VOLTAGE].iter().any(|&f| self.feature(f).is_some());
        a
    }

    /// `None` when the mouse does not answer (asleep or off).
    pub fn read(&mut self) -> Option<Reading> {
        if !self.ping() {
            return None;
        }
        let mut r = Reading::default();

        if let Some(f) = self.feature(F_BATTERY_UNIFIED) {
            if let Some(p) = self.call(f, 1, &[]).filter(|p| p.len() >= 3) {
                (r.battery, r.charging) = (Some(p[0].min(100)), matches!(p[2], 1..=3));
            }
        } else if let Some(f) = self.feature(F_BATTERY_LEVEL) {
            if let Some(p) = self.call(f, 0, &[]).filter(|p| p.len() >= 3) {
                (r.battery, r.charging) = (Some(p[0].min(100)), matches!(p[2], 1..=4));
                r.battery_approx = true;
            }
        } else if let Some(f) = self.feature(F_BATTERY_VOLTAGE) {
            if let Some(p) = self.call(f, 0, &[]).filter(|p| p.len() >= 3) {
                const CHARGING_BIT: u8 = 0x80;
                let mv = u16::from_be_bytes([p[0], p[1]]);
                (r.battery, r.charging) = (Some(percent_from_mv(mv)), p[2] & CHARGING_BIT != 0);
                r.battery_approx = true;
            }
        }
        if let Some(f) = self.feature(F_DPI) {
            r.dpi = self.call(f, 2, &[0]).filter(|p| p.len() >= 3).map(|p| u16::from_be_bytes([p[1], p[2]]) as u32);
        }
        if let Some(f) = self.feature(F_RATE) {
            r.rate_hz = self.call(f, 1, &[]).and_then(|p| p.first().copied()).filter(|&ms| ms > 0).map(|ms| 1000 / ms as u32);
        }
        Some(r)
    }

    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        let Some(f) = self.feature(F_DPI) else {
            return false;
        };
        let v = (dpi.min(u16::MAX as u32) as u16).to_be_bytes();
        self.call(f, 3, &[0, v[0], v[1]]).is_some()
    }

    pub fn set_rate(&mut self, hz: u32) -> bool {
        let Some(f) = self.feature(F_RATE) else {
            return false;
        };
        let ms = (1000 / hz.max(1)).clamp(1, 8) as u8;
        self.call(f, 2, &[ms]).is_some()
    }
}

/// "RQM 17.01 build 0019" from a getFwInfo reply of the main
/// application; `None` for a bootloader or hardware entity.
fn main_firmware(p: &[u8]) -> Option<String> {
    if p.len() < 8 || p[0] & FW_TYPE_MASK != FW_MAIN {
        return None;
    }
    let prefix = String::from_utf8_lossy(&p[1..4]).trim().to_string();
    Some(format!("{prefix} {:02X}.{:02X} build {:02X}{:02X}", p[4], p[5], p[6], p[7]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_is_the_main_application() {
        // getFwInfo replies of an M705: bootloader first, then the application.
        let boot = [1, b'B', b'O', b'T', 0x59, 0x00, 0x00, 0x10];
        let app = [0, b'R', b'Q', b'M', 0x17, 0x01, 0x00, 0x19];
        assert_eq!(main_firmware(&boot), None);
        assert_eq!(main_firmware(&app).as_deref(), Some("RQM 17.01 build 0019"));
        assert_eq!(main_firmware(&app[..4]), None);
    }

    #[test]
    fn request_layout() {
        let f = request(1, 0x05, 3, &[0, 0x06, 0x40]);
        assert_eq!(&f[..7], &[0x11, 0x01, 0x05, 0x3A, 0x00, 0x06, 0x40]);
    }

    #[test]
    fn reply_ok_error_and_noise() {
        let mut ok = [0u8; 20];
        ok[..6].copy_from_slice(&[0x11, 0x01, 0x05, 0x2A, 0x00, 0x03]);
        assert_eq!(parse_reply(&ok, 1, 5, 2).unwrap().unwrap()[..2], [0x00, 0x03]);

        let err = [0x11, 0x01, 0xFF, 0x05, 0x2A, 0x08, 0, 0];
        assert_eq!(parse_reply(&err, 1, 5, 2), Some(Err(0x08)));

        let other = [0x11, 0x01, 0x07, 0x00, 0, 0];
        assert_eq!(parse_reply(&other, 1, 5, 2), None);
    }

    #[test]
    fn dpi_list_range_and_discrete() {
        let range = [0x00, 0xC8, 0xE0, 0x32, 0x3E, 0x80, 0, 0];
        assert_eq!(parse_dpi_list(&range), Some(DpiRange::Steps { min: 200, max: 16000, step: 50 }));
        let list = [0x01, 0x90, 0x03, 0x20, 0x06, 0x40, 0, 0];
        assert_eq!(parse_dpi_list(&list), Some(DpiRange::List(vec![400, 800, 1600])));
    }

    #[test]
    fn keyboards_are_never_opened() {
        const KEYBOARD: u8 = 0;
        const MOUSE: u8 = 3;
        assert_eq!(fit(Some(MOUSE)), Fit::Pointer);
        assert_eq!(fit(Some(KEYBOARD)), Fit::Other);
        assert_eq!(fit(None), Fit::Unknown);
    }

    #[test]
    fn rate_mask() {
        // 1, 2, 4, 8 ms.
        assert_eq!(parse_rates(0b1000_1011), vec![125, 250, 500, 1000]);
    }
}
