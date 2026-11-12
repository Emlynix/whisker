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
use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const VID: u16 = 0x046D;

const REPORT_SHORT: u8 = 0x10;
const REPORT_LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
const PARAMS: usize = 16;
/// Software ids of our requests; 0 marks the device's own notifications.
const SW_FIRST: u8 = 1;
const SW_LAST: u8 = 15;
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
/// FeatureSet: getCount (fn 0), getFeatureID(index) (fn 1).
const F_FEATURE_SET: u16 = 0x0001;
/// A device has a few dozen features; each silent one costs a timeout.
const FEATURES_MAX: u8 = 64;
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
/// OnboardProfiles: getOnboardMode is fn 2.
const F_ONBOARD: u16 = 0x8100;
/// getOnboardMode: the mouse runs its stored profile.
const MODE_ONBOARD: u8 = 1;
/// getOnboardMode: software on the computer sets DPI and report rate.
const MODE_HOST: u8 = 2;

/// Function and software id as one byte: `fn<<4 | sw`.
pub fn tag(func: u8, sw: u8) -> u8 {
    (func << 4) | (sw & 0x0F)
}

/// Request frame: `[11, device, feature, tag, params…]`.
pub fn request(device: u8, feature: u8, tag: u8, params: &[u8]) -> [u8; LONG_LEN] {
    let mut f = [0u8; LONG_LEN];
    f[0] = REPORT_LONG;
    f[1] = device;
    f[2] = feature;
    f[3] = tag;
    let n = params.len().min(PARAMS);
    f[4..4 + n].copy_from_slice(&params[..n]);
    f
}

/// Reply to our request: `Some(Ok(params))`, `Some(Err(code))`, or `None`
/// for unrelated reports (notifications, other requests).
pub fn parse_reply(r: &[u8], device: u8, feature: u8, tag: u8) -> Option<Result<Vec<u8>, u8>> {
    if r.len() < 5 || !(r[0] == REPORT_LONG || r[0] == REPORT_SHORT) || r[1] != device {
        return None;
    }
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

/// The reports of one HID++ collection: the device, or a test double.
pub trait Wire {
    fn write(&self, report: &[u8], timeout: Duration) -> bool;
    fn read(&self, len: usize, timeout: Duration) -> Option<Vec<u8>>;
}

impl Wire for Channel {
    fn write(&self, report: &[u8], timeout: Duration) -> bool {
        Channel::write(self, report, timeout)
    }

    fn read(&self, len: usize, timeout: Duration) -> Option<Vec<u8>> {
        Channel::read(self, len, timeout)
    }
}

pub struct Hidpp<W: Wire = Channel> {
    ch: W,
    device: u8,
    features: HashMap<u16, u8>,
    /// Software id of the last request.
    sw: Cell<u8>,
    /// The device holds each reply until the next request arrives.
    late: Cell<bool>,
}

/// When a call sends the extra request that releases a held reply.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Nudge {
    /// Only after the device stayed silent: a prompt device gets one request.
    IfSilent,
    /// Right away: an empty receiver slot then costs one timeout, not two.
    Always,
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
                let mut d = Hidpp::on(ch, dev);
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
}

impl<W: Wire> Hidpp<W> {
    fn on(ch: W, device: u8) -> Self {
        Hidpp { ch, device, features: HashMap::new(), sw: Cell::new(SW_LAST), late: Cell::new(false) }
    }

    /// DeviceName.getDeviceType: 0 keyboard … 3 mouse.
    fn kind(&mut self) -> Option<u8> {
        let f = self.feature(F_NAME)?;
        self.call(f, 2, &[])?.first().copied()
    }

    fn call(&self, feature: u8, func: u8, params: &[u8]) -> Option<Vec<u8>> {
        self.exchange(feature, func, params, Nudge::IfSilent)
    }

    /// One request and its reply.
    ///
    /// A receiver may hand each reply over only when the next request
    /// arrives (seen with an M705 on a Unifying receiver):
    ///
    /// ```text
    /// request A ──>            request A ──>
    ///           <── reply A    nudge     ──>
    /// request B ──>                      <── reply A
    ///           <── reply B    (reply to the nudge waits for the next request)
    ///   prompt device              late device
    /// ```
    /// Every request has its own software id, so a reply that comes late is
    /// never taken for the answer to the request after it.
    fn exchange(&self, feature: u8, func: u8, params: &[u8], nudge: Nudge) -> Option<Vec<u8>> {
        let tag = self.next_tag(func);
        if !self.ch.write(&request(self.device, feature, tag, params), TIMEOUT) {
            return None;
        }
        let nudged = self.late.get() || nudge == Nudge::Always;
        if nudged {
            self.nudge();
        }
        if let Some(r) = self.reply(feature, tag) {
            return r.ok();
        }
        if nudged {
            return None;
        }

        // Silent: the reply may be held back; one more request releases it.
        self.nudge();
        let r = self.reply(feature, tag)?;
        self.late.set(true);
        crate::log!("hidpp: replies come one request late");
        r.ok()
    }

    /// The tag of a new request: the next software id, `SW_FIRST..=SW_LAST`.
    fn next_tag(&self, func: u8) -> u8 {
        let sw = if self.sw.get() >= SW_LAST { SW_FIRST } else { self.sw.get() + 1 };
        self.sw.set(sw);
        tag(func, sw)
    }

    /// A ping whose only job is to make the device send what it holds.
    fn nudge(&self) {
        let tag = self.next_tag(1);
        self.ch.write(&request(self.device, ROOT, tag, &[0, 0, PING_DATA]), TIMEOUT);
    }

    /// The reply to the request `tag`; other reports are skipped.
    fn reply(&self, feature: u8, tag: u8) -> Option<Result<Vec<u8>, u8>> {
        let start = Instant::now();
        while let Some(left) = TIMEOUT.checked_sub(start.elapsed()) {
            let r = self.ch.read(LONG_LEN, left)?;
            if let Some(v) = parse_reply(&r, self.device, feature, tag) {
                return Some(v);
            }
        }
        None
    }

    /// Root `getProtocolVersion`: answers only for a HID++ 2.0 device.
    fn ping(&self) -> bool {
        self.exchange(ROOT, 1, &[0, 0, PING_DATA], Nudge::Always).is_some_and(|p| p.first().is_some_and(|&major| major >= 2))
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

    /// Who is in charge of DPI and report rate, for the log and bug
    /// reports; `None` without onboard profiles. A read request only.
    pub fn onboard_note(&mut self) -> Option<String> {
        let f = self.feature(F_ONBOARD)?;
        let mode = self.call(f, 2, &[])?.first().copied()?;
        Some(onboard_text(mode))
    }

    /// Lines for a bug report: the feature table and the onboard mode.
    /// Read requests only.
    pub fn details(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        let features = self.feature_table();
        if !features.is_empty() {
            out.push(features_text(&features));
        }
        out.extend(self.onboard_note());
        out
    }

    /// (index, feature id) of every feature the device lists.
    fn feature_table(&mut self) -> Vec<(u8, u16)> {
        let Some(f) = self.feature(F_FEATURE_SET) else {
            return Vec::new();
        };
        let count = self.call(f, 0, &[]).and_then(|p| p.first().copied()).unwrap_or(0).min(FEATURES_MAX);

        let mut out = vec![(ROOT, 0)];
        for index in 1..=count {
            // A device that stops answering is not asked for the rest.
            let Some(p) = self.call(f, 1, &[index]).filter(|p| p.len() >= 2) else {
                break;
            };
            out.push((index, u16::from_be_bytes([p[0], p[1]])));
        }
        out
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

/// `features (index:id): 00:0000, 01:0001, 02:0003`.
fn features_text(list: &[(u8, u16)]) -> String {
    let items: Vec<String> = list.iter().map(|(index, id)| format!("{index:02X}:{id:04X}")).collect();
    format!("features (index:id): {}", items.join(", "))
}

fn onboard_text(mode: u8) -> String {
    match mode {
        MODE_ONBOARD => "onboard mode: the mouse runs its stored profile; DPI and rate set from the computer may be ignored".into(),
        MODE_HOST => "host mode: software on the computer sets DPI and rate".into(),
        other => format!("onboard mode value {other}"),
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
    use std::cell::RefCell;
    use std::collections::VecDeque;

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
    fn details_name_features_and_who_is_in_charge() {
        assert_eq!(features_text(&[(0, 0x0000), (1, 0x0001), (0x0B, 0x8100)]), "features (index:id): 00:0000, 01:0001, 0B:8100");
        // Issue #2: a G502 X in onboard mode ignores the rate we set.
        assert!(onboard_text(MODE_ONBOARD).starts_with("onboard mode"));
        assert!(onboard_text(MODE_HOST).starts_with("host mode"));
        assert_eq!(onboard_text(7), "onboard mode value 7");
    }

    #[test]
    fn request_layout() {
        let f = request(1, 0x05, tag(3, 0x0A), &[0, 0x06, 0x40]);
        assert_eq!(&f[..7], &[0x11, 0x01, 0x05, 0x3A, 0x00, 0x06, 0x40]);
    }

    #[test]
    fn reply_ok_error_and_noise() {
        let mut ok = [0u8; 20];
        ok[..6].copy_from_slice(&[0x11, 0x01, 0x05, 0x2A, 0x00, 0x03]);
        assert_eq!(parse_reply(&ok, 1, 5, 0x2A).unwrap().unwrap()[..2], [0x00, 0x03]);

        let err = [0x11, 0x01, 0xFF, 0x05, 0x2A, 0x08, 0, 0];
        assert_eq!(parse_reply(&err, 1, 5, 0x2A), Some(Err(0x08)));

        let other = [0x11, 0x01, 0x07, 0x00, 0, 0];
        assert_eq!(parse_reply(&other, 1, 5, 0x2A), None);
    }

    /// A paired M705 as the receiver relays it. `Late`: each reply leaves
    /// only when the next request arrives (seen on a Unifying receiver).
    struct FakeMouse {
        late: bool,
        held: RefCell<Option<Vec<u8>>>,
        ready: RefCell<VecDeque<Vec<u8>>>,
        writes: Cell<usize>,
    }

    const FAKE_FEATURES: [(u16, u8); 3] = [(F_FW, 2), (F_NAME, 3), (F_BATTERY_LEVEL, 6)];
    const FAKE_BATTERY: u8 = 70;

    impl FakeMouse {
        fn new(late: bool) -> Self {
            // A late device still holds the reply to somebody's last request.
            let stale = late.then(|| vec![REPORT_LONG, 1, ROOT, tag(0, 0x0A), 0]);
            FakeMouse { late, held: RefCell::new(stale), ready: RefCell::default(), writes: Cell::new(0) }
        }

        fn answer(req: &[u8]) -> Vec<u8> {
            let mut r = req.to_vec();
            let func = req[3] >> 4;
            match (req[2], func) {
                (ROOT, 0) => {
                    let id = u16::from_be_bytes([req[4], req[5]]);
                    r[4] = FAKE_FEATURES.iter().find(|f| f.0 == id).map_or(0, |f| f.1);
                }
                (ROOT, _) => r[4..7].copy_from_slice(&[4, 5, req[6]]),
                _ => r[4..7].copy_from_slice(&[FAKE_BATTERY, 0, 0]),
            }
            r
        }
    }

    impl Wire for FakeMouse {
        fn write(&self, req: &[u8], _: Duration) -> bool {
            self.writes.set(self.writes.get() + 1);
            if req[1] != 1 {
                return true;
            }
            let reply = Self::answer(req);
            if !self.late {
                self.ready.borrow_mut().push_back(reply);
                return true;
            }
            self.ready.borrow_mut().extend(self.held.replace(Some(reply)));
            true
        }

        fn read(&self, _: usize, _: Duration) -> Option<Vec<u8>> {
            self.ready.borrow_mut().pop_front()
        }
    }

    #[test]
    fn late_replies_are_not_taken_for_the_next_request() {
        for late in [false, true] {
            let mut d = Hidpp::on(FakeMouse::new(late), 1);
            assert!(d.ping(), "late {late}");
            assert_eq!(d.feature(F_NAME), Some(3), "late {late}");
            assert_eq!(d.feature(F_FW), Some(2), "late {late}");
            assert_eq!(d.feature(F_BATTERY_UNIFIED), None, "late {late}");
            assert_eq!(d.read().and_then(|r| r.battery), Some(FAKE_BATTERY), "late {late}");
        }
    }

    #[test]
    fn a_prompt_mouse_gets_one_request_per_call() {
        let mut d = Hidpp::on(FakeMouse::new(false), 1);
        let before = d.ch.writes.get();
        assert_eq!(d.feature(F_NAME), Some(3));
        assert_eq!(d.ch.writes.get(), before + 1);
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
