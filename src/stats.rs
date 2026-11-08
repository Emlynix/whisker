//! Usage statistics: clicks, wheel, distance and measured report rate,
//! taken from Raw Input of the Lionel mouse only. Battery history is
//! sampled from mouse status. Persisted to `%APPDATA%\Whisker\stats.txt`.

use crate::store;
use crate::win;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use store::STATS_FILE;
const METERS_PER_INCH: f64 = 0.0254;

const BATTERY_SAMPLE_EVERY: u64 = 5 * 60;
const BATTERY_KEEP_SECS: u64 = 14 * 24 * 3600;

/// Report-rate window; only windows with steady motion count.
const RATE_WINDOW: Duration = Duration::from_millis(250);
const RATE_MIN_EVENTS: u32 = 20;
const RATE_HOLD: Duration = Duration::from_secs(3);

const MOUSE_DESKTOP: u16 = 0x01;
const USAGE_MOUSE: u16 = 0x02;
const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;

const BTN_LEFT: u16 = 0x0001;
const BTN_RIGHT: u16 = 0x0004;
const BTN_MIDDLE: u16 = 0x0010;
const BTN_4: u16 = 0x0040;
const BTN_5: u16 = 0x0100;
const BTN_WHEEL: u16 = 0x0400;

#[derive(Clone, Copy, Default)]
pub struct Counters {
    pub left: u64,
    pub right: u64,
    pub middle: u64,
    pub side: u64,
    pub wheel: u64,
    pub distance_m: f64,
}

impl Counters {
    fn add(&mut self, o: &Counters) {
        self.left += o.left;
        self.right += o.right;
        self.middle += o.middle;
        self.side += o.side;
        self.wheel += o.wheel;
        self.distance_m += o.distance_m;
    }

    pub fn clicks(&self) -> u64 {
        self.left + self.right + self.middle + self.side
    }
}

#[derive(Clone, Copy)]
pub struct BatterySample {
    pub t: u64,
    pub pct: u8,
    pub charging: bool,
}

/// Text format, one record per line:
/// `day=2026-11-08 left right middle side wheel meters` and `bat=unix pct charging`.
#[derive(Default)]
struct StatsFile {
    days: BTreeMap<String, Counters>,
    battery: Vec<BatterySample>,
}

impl StatsFile {
    fn parse(text: &str) -> Self {
        let mut f = StatsFile::default();
        for (k, v) in store::pairs(text) {
            let p: Vec<&str> = v.split(' ').collect();
            match (k, p.as_slice()) {
                ("day", [d, l, r, m, s, w, dist]) => {
                    let n = |x: &str| x.parse().unwrap_or(0);
                    let c = Counters { left: n(l), right: n(r), middle: n(m), side: n(s), wheel: n(w), distance_m: dist.parse().unwrap_or(0.0) };
                    f.days.insert(d.to_string(), c);
                }
                ("bat", [t, pct, chg]) => {
                    if let (Ok(t), Ok(pct)) = (t.parse(), pct.parse()) {
                        f.battery.push(BatterySample { t, pct, charging: *chg == "1" });
                    }
                }
                _ => {}
            }
        }
        f
    }

    fn format(&self) -> String {
        let mut out = String::new();
        for (d, c) in &self.days {
            out += &format!("day={d} {} {} {} {} {} {:.2}
", c.left, c.right, c.middle, c.side, c.wheel, c.distance_m);
        }
        for b in &self.battery {
            out += &format!("bat={} {} {}
", b.t, b.pct, b.charging as u8);
        }
        out
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Today,
    Week,
    All,
}

struct State {
    file: StatsFile,
    dpi: u32,
    dirty: bool,
    /// Raw Input handle -> counts toward the statistics.
    ours: HashMap<usize, bool>,
    /// Only devices of this vendor id count; `None` = every mouse.
    vendor: Option<u16>,
    rate_start: Instant,
    rate_count: u32,
    rate: Option<(u32, Instant)>,
}

static STATE: OnceLock<Arc<Mutex<State>>> = OnceLock::new();

#[derive(Clone)]
pub struct Stats {
    state: Arc<Mutex<State>>,
}

impl Stats {
    /// Loads saved stats and starts the Raw Input listener thread.
    pub fn start() -> Self {
        let state = Arc::new(Mutex::new(State {
            file: store::load_text(STATS_FILE).map(|t| StatsFile::parse(&t)).unwrap_or_default(),
            dpi: 1600,
            dirty: false,
            ours: HashMap::new(),
            vendor: None,
            rate_start: Instant::now(),
            rate_count: 0,
            rate: None,
        }));
        let _ = STATE.set(state.clone());

        std::thread::Builder::new()
            .name("rawinput".into())
            .spawn(listen)
            .expect("spawn raw input thread");
        Self { state }
    }

    /// Restricts counting to one vendor id (the configured mouse) or all mice.
    pub fn set_vendor(&self, vid: Option<u16>) {
        let mut s = self.state.lock().unwrap();
        if s.vendor != vid {
            s.vendor = vid;
            s.ours.clear();
        }
    }

    pub fn set_dpi(&self, dpi: u32) {
        self.state.lock().unwrap().dpi = dpi.max(1);
    }

    pub fn record_battery(&self, pct: u8, charging: bool) {
        let now = unix_now();
        let mut s = self.state.lock().unwrap();
        let samples = &mut s.file.battery;

        let due = match samples.last() {
            Some(l) => now.saturating_sub(l.t) >= BATTERY_SAMPLE_EVERY || l.charging != charging,
            None => true,
        };
        if !due {
            return;
        }

        samples.push(BatterySample { t: now, pct, charging });
        samples.retain(|b| now.saturating_sub(b.t) <= BATTERY_KEEP_SECS);
        s.dirty = true;
    }

    pub fn totals(&self, period: Period) -> Counters {
        let s = self.state.lock().unwrap();
        let today = win::local_date(0);
        let week_start = win::local_date(6);

        let mut sum = Counters::default();
        for (day, c) in &s.file.days {
            let include = match period {
                Period::Today => *day == today,
                Period::Week => *day >= week_start,
                Period::All => true,
            };
            if include {
                sum.add(c);
            }
        }
        sum
    }

    /// Last measured report rate while the mouse was moving, if recent.
    pub fn measured_hz(&self) -> Option<u32> {
        let s = self.state.lock().unwrap();
        let (hz, at) = s.rate?;
        (at.elapsed() < RATE_HOLD).then_some(hz)
    }

    pub fn battery_history(&self) -> Vec<BatterySample> {
        self.state.lock().unwrap().file.battery.clone()
    }

    pub fn save(&self) {
        let mut s = self.state.lock().unwrap();
        if !s.dirty {
            return;
        }
        store::save_text(STATS_FILE, &s.file.format());
        s.dirty = false;
    }
}

pub fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Hours of battery left from the discharge since the last charge, e.g.
/// 80 % -> 70 % over 10 h with 70 % left gives 70 h. `None` if too little data.
pub fn hours_left(samples: &[BatterySample]) -> Option<f64> {
    const MIN_SPAN_SECS: u64 = 3 * 3600;
    const MIN_DROP: u8 = 2;

    let last = samples.last()?;
    if last.charging {
        return None;
    }

    // Start of the current discharge run: after the last charge or level rise.
    let mut start = samples.len() - 1;
    while start > 0 {
        let prev = &samples[start - 1];
        if prev.charging || prev.pct < samples[start].pct {
            break;
        }
        start -= 1;
    }

    let first = &samples[start];
    let span = last.t.saturating_sub(first.t);
    let drop = first.pct.saturating_sub(last.pct);
    if span < MIN_SPAN_SECS || drop < MIN_DROP {
        return None;
    }

    let pct_per_hour = drop as f64 / (span as f64 / 3600.0);
    Some(last.pct as f64 / pct_per_hour)
}

// ---- Raw Input thread ------------------------------------------------------

fn listen() {
    let Some(hwnd) = win::message_window("WhiskerRawInput", Some(wndproc)) else {
        return;
    };

    // INPUTSINK: receive mouse input even when our window is not focused.
    let dev = RAWINPUTDEVICE {
        usUsagePage: MOUSE_DESKTOP,
        usUsage: USAGE_MOUSE,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    unsafe {
        RegisterRawInputDevices(&dev, 1, size_of::<RAWINPUTDEVICE>() as u32);
    }
    win::pump_messages();
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        if let Some(input) = unsafe { read_raw(lp) } {
            on_input(&input);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe fn read_raw(lp: LPARAM) -> Option<RAWINPUT> {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let n = unsafe { GetRawInputData(lp as HRAWINPUT, RID_INPUT, (&mut raw as *mut RAWINPUT).cast(), &mut size, header) };

    if n == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE {
        return None;
    }
    Some(raw)
}

/// Device name contains `VID_xxxx` of the wanted vendor (or any with `None`).
fn is_counted(dev: HANDLE, vendor: Option<u16>) -> bool {
    let Some(vid) = vendor else {
        return true;
    };
    let mut len = 0u32;
    unsafe {
        GetRawInputDeviceInfoW(dev, RIDI_DEVICENAME, std::ptr::null_mut(), &mut len);
    }
    let mut buf = vec![0u16; len as usize + 1];
    let n = unsafe { GetRawInputDeviceInfoW(dev, RIDI_DEVICENAME, buf.as_mut_ptr().cast(), &mut len) };
    if n == u32::MAX {
        return false;
    }
    String::from_utf16_lossy(&buf).to_uppercase().contains(&format!("VID_{vid:04X}"))
}

fn on_input(raw: &RAWINPUT) {
    let Some(state) = STATE.get() else {
        return;
    };
    let mut s = state.lock().unwrap();

    let dev = raw.header.hDevice;
    let vendor = s.vendor;
    let ours = *s.ours.entry(dev as usize).or_insert_with(|| is_counted(dev, vendor));
    if !ours {
        return;
    }

    let m = unsafe { raw.data.mouse };
    let flags = unsafe { m.Anonymous.Anonymous.usButtonFlags };
    let moved = m.usFlags & MOUSE_MOVE_ABSOLUTE == 0 && (m.lLastX != 0 || m.lLastY != 0);
    let dpi = s.dpi as f64;

    let day = s.file.days.entry(win::local_date(0)).or_default();
    day.left += (flags & BTN_LEFT != 0) as u64;
    day.right += (flags & BTN_RIGHT != 0) as u64;
    day.middle += (flags & BTN_MIDDLE != 0) as u64;
    day.side += (flags & (BTN_4 | BTN_5) != 0) as u64;
    day.wheel += (flags & BTN_WHEEL != 0) as u64;

    if moved {
        let counts = ((m.lLastX as f64).powi(2) + (m.lLastY as f64).powi(2)).sqrt();
        day.distance_m += counts / dpi * METERS_PER_INCH;
        track_rate(&mut s);
    }
    s.dirty = true;
}

/// Counts motion reports per window; a full window gives the report rate.
fn track_rate(s: &mut State) {
    s.rate_count += 1;
    let elapsed = s.rate_start.elapsed();
    if elapsed < RATE_WINDOW {
        return;
    }

    if s.rate_count >= RATE_MIN_EVENTS {
        let hz = (s.rate_count as f64 / elapsed.as_secs_f64()).round() as u32;
        let best = match s.rate {
            Some((prev, at)) if at.elapsed() < RATE_HOLD => prev.max(hz),
            _ => hz,
        };
        s.rate = Some((best, Instant::now()));
    }
    s.rate_start = Instant::now();
    s.rate_count = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(h: u64, pct: u8) -> BatterySample {
        BatterySample { t: h * 3600, pct, charging: false }
    }

    #[test]
    fn eta_from_discharge_run() {
        let v = [s(0, 80), s(5, 75), s(10, 70)];
        assert_eq!(hours_left(&v).unwrap().round(), 70.0);
    }

    #[test]
    fn eta_ignores_samples_before_charge() {
        let v = [s(0, 20), s(1, 90), s(5, 88), s(9, 86)];
        assert_eq!(hours_left(&v).unwrap().round(), 172.0);
    }

    #[test]
    fn file_roundtrip() {
        let text = "day=2026-11-08 5 4 3 2 1 12.50
bat=100 60 0
";
        assert_eq!(StatsFile::parse(text).format(), text);
    }

    #[test]
    fn eta_needs_enough_data() {
        assert!(hours_left(&[s(0, 80), s(1, 79)]).is_none());
        assert!(hours_left(&[]).is_none());
    }
}
