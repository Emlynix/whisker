//! Usage statistics: clicks, wheel, distance and measured report rate,
//! taken from the platform input hook (Raw Input on Windows) of the
//! configured mouse only. Battery history is
//! sampled from mouse status. Persisted to `%APPDATA%\Whisker\stats.txt`.

use crate::platform::input;
use crate::store;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use store::STATS_FILE;
const METERS_PER_INCH: f64 = 0.0254;

const BATTERY_SAMPLE_EVERY: u64 = 5 * 60;
const BATTERY_KEEP_SECS: u64 = 14 * 24 * 3600;

/// Report-rate window; only windows with steady motion count.
const RATE_WINDOW: Duration = Duration::from_millis(250);
const RATE_MIN_EVENTS: u32 = 20;
const RATE_HOLD: Duration = Duration::from_secs(3);

/// One mouse report from the platform input hook.
pub struct MouseInput {
    /// Backend id of the reporting device, stable while it is connected.
    pub dev: usize,
    /// Button went down in this report (side = button 4 or 5).
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub side: bool,
    /// Vertical wheel turned.
    pub wheel: bool,
    /// Relative motion in sensor counts; 0 for absolute pointers.
    pub dx: i32,
    pub dy: i32,
}

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
    /// Input device id -> counts toward the statistics.
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
    /// Loads saved stats and starts the platform input hook.
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

        input::start(on_input);
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
        let today = local_date(0);
        let week_start = local_date(6);

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

/// Local `(month, day)` `days_back` days ago, for chart labels.
pub fn month_day(days_back: i64) -> (i64, i64) {
    let (y, m, d) = crate::platform::today();
    let (_, m, d) = civil_from_days(days_from_civil(y, m, d) - days_back);
    (m, d)
}

/// Local date `days_back` days ago as `YYYY-MM-DD`, e.g. `local_date(0)` is today.
fn local_date(days_back: i64) -> String {
    let (y, m, d) = crate::platform::today();
    let (y, m, d) = civil_from_days(days_from_civil(y, m, d) - days_back);
    format!("{y:04}-{m:02}-{d:02}")
}

// Gregorian calendar <-> day number (H. Hinnant's algorithms).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    (y, m, d)
}

// ---- Counting --------------------------------------------------------------

/// Device of the wanted vendor (or any with `None`).
fn is_counted(dev: usize, vendor: Option<u16>) -> bool {
    let Some(vid) = vendor else {
        return true;
    };
    input::has_vendor(dev, vid)
}

fn on_input(m: &MouseInput) {
    let Some(state) = STATE.get() else {
        return;
    };
    let mut s = state.lock().unwrap();

    let vendor = s.vendor;
    let ours = *s.ours.entry(m.dev).or_insert_with(|| is_counted(m.dev, vendor));
    if !ours {
        return;
    }

    let moved = m.dx != 0 || m.dy != 0;
    let dpi = s.dpi as f64;

    let day = s.file.days.entry(local_date(0)).or_default();
    day.left += m.left as u64;
    day.right += m.right as u64;
    day.middle += m.middle as u64;
    day.side += m.side as u64;
    day.wheel += m.wheel as u64;

    if moved {
        let counts = ((m.dx as f64).powi(2) + (m.dy as f64).powi(2)).sqrt();
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
    fn calendar_roundtrip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        let d = days_from_civil(2026, 3, 1);
        assert_eq!(civil_from_days(d - 1), (2026, 2, 28));
        assert_eq!(civil_from_days(days_from_civil(2024, 3, 1) - 1), (2024, 2, 29));
    }

    #[test]
    fn eta_needs_enough_data() {
        assert!(hours_left(&[s(0, 80), s(1, 79)]).is_none());
        assert!(hours_left(&[]).is_none());
    }
}
