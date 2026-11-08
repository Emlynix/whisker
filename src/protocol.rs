//! Wire format of the Lionel mouse configuration channel.
//!
//! Recovered from the vendor app (`LIONEL BLACK.exe`). Every message is a
//! 33-byte HID feature report on usage page 0xFF00:
//!
//! ```text
//!  0     1     2     3     4     5      6     7 ..
//! +-----+-----+-----+-----+-----+------+-----+---------------+
//! | 0C  | 01  | cmd | 00  | seq | 01   | len | data[len]     |
//! +-----+-----+-----+-----+-----+------+-----+---------------+
//! ```
//!
//! Status is read by writing `cmd = 0x20` and reading the report back.
//! Pure code only: no I/O here.

mod buttons;
pub use buttons::*;

pub const REPORT_ID: u8 = 0x0C;
pub const REPORT_LEN: usize = 33;
pub const VENDOR_ID: u16 = 0x248A;
pub const PID_WIRED: u16 = 0x5B2E;
pub const CONFIG_USAGE_PAGE: u16 = 0xFF00;

pub const STAGE_COUNT: usize = 6;

const TARGET_DEVICE: u8 = 0x01;
const CHUNK_SINGLE: u8 = 0x01;
const DATA_OFFSET: usize = 7;

const CMD_DPI: u8 = 0x05;
const CMD_POLLING: u8 = 0x07;
const CMD_STATUS: u8 = 0x20;

const DPI_PAYLOAD_LEN: u8 = 0x12;
const STATUS_SIGNATURE: &[u8; 4] = b"M022";
const DPI_UNIT: u32 = 50;
/// Sensor assumed until the mouse reports one (White Shark Lionel).
const DEFAULT_SENSOR: u8 = 0x25;

mod sensors;
pub use sensors::{SENSORS, Sensor};

/// Sensor of the connected mouse; picks the DPI table.
static CURRENT_SENSOR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(DEFAULT_SENSOR);

/// Selects the DPI table by the sensor id from the status reply.
/// Unknown ids keep the current table.
pub fn set_sensor(id: u8) {
    if SENSORS.iter().any(|s| s.id == id) {
        CURRENT_SENSOR.store(id, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn sensor() -> &'static Sensor {
    let id = CURRENT_SENSOR.load(std::sync::atomic::Ordering::Relaxed);
    SENSORS.iter().find(|s| s.id == id).unwrap_or(&SENSORS[0])
}

/// Number of DPI steps of the current sensor, e.g. 59 for PAW3325.
pub fn dpi_steps() -> usize {
    sensor().values.len()
}

/// DPI value of a table index, e.g. `dpi_of(0) == 200`, `dpi_of(58) == 10000` (PAW3325).
pub fn dpi_of(index: usize) -> u32 {
    let v = sensor().values;
    v[index.min(v.len() - 1)] as u32 * DPI_UNIT
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PollingRate {
    Hz125,
    Hz250,
    Hz500,
    Hz1000,
}

impl PollingRate {
    pub const ALL: [PollingRate; 4] = [Self::Hz125, Self::Hz250, Self::Hz500, Self::Hz1000];

    pub fn hz(self) -> u32 {
        match self {
            Self::Hz125 => 125,
            Self::Hz250 => 250,
            Self::Hz500 => 500,
            Self::Hz1000 => 1000,
        }
    }

    /// Report interval in milliseconds, which is what the firmware expects.
    fn interval_ms(self) -> u8 {
        (1000 / self.hz()) as u8
    }
}

/// DPI stage setup. `stages` hold table indices, `enabled` is a bit mask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DpiConfig {
    pub stages: [usize; STAGE_COUNT],
    pub enabled: u8,
    pub active: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Charge {
    Discharging,
    Charging,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub battery: u8,
    pub charge: Charge,
    /// Active DPI stage, 0-based. Changes when the DPI button is pressed.
    pub active_stage: usize,
    pub sensor: u8,
    /// Whole reply, shown as diagnostics.
    pub raw: [u8; REPORT_LEN],
}

fn frame(cmd: u8, seq: u8, data: &[u8]) -> [u8; REPORT_LEN] {
    let mut f = [0u8; REPORT_LEN];
    f[0] = REPORT_ID;
    f[1] = TARGET_DEVICE;
    f[2] = cmd;
    f[4] = seq;

    if data.is_empty() {
        return f;
    }

    f[5] = CHUNK_SINGLE;
    f[6] = data.len() as u8;
    f[DATA_OFFSET..DATA_OFFSET + data.len()].copy_from_slice(data);
    f
}

pub fn status_request(seq: u8) -> [u8; REPORT_LEN] {
    frame(CMD_STATUS, seq, &[])
}

pub fn polling_frame(rate: PollingRate, seq: u8) -> [u8; REPORT_LEN] {
    frame(CMD_POLLING, seq, &[rate.interval_ms()])
}

/// Builds the DPI frame: `[(active+1)<<4 | count, mask, 6 x u16 LE code, pad]`.
pub fn dpi_frame(cfg: &DpiConfig, seq: u8) -> [u8; REPORT_LEN] {
    let mut data = [0u8; DPI_PAYLOAD_LEN as usize];
    let count = cfg.enabled.count_ones() as u8;

    data[0] = (((cfg.active as u8) + 1) << 4) | (count & 0x0F);
    data[1] = cfg.enabled;

    let s = sensor();
    for (i, &index) in cfg.stages.iter().enumerate() {
        let code = s.first_code + index.min(s.values.len() - 1) as u16;
        data[2 + i * 2..4 + i * 2].copy_from_slice(&code.to_le_bytes());
    }

    frame(CMD_DPI, seq, &data)
}

// ---- lighting and other settings (see docs/protocol.md) ---------------------
// A wireless mouse picks these up when it wakes, i.e. on the next movement.

pub const DPI_COLOR_SLOTS: usize = 8;
pub const BREATHING_MAX: usize = 7;
pub const COLORFUL_SLOTS: usize = 7;
pub const BRIGHTNESS_MAX: u8 = 4;
pub const SPEED_MAX: u8 = 3;

/// Lighting effects offered by the vendor app, with their wire ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Off = 0,
    Streaming = 1,
    Breathing = 2,
    Steady = 3,
    Neon = 4,
    Snake = 5,
    ColorfulSteady = 6,
    Wave = 9,
}

impl Effect {
    pub const ALL: [Effect; 8] = [
        Effect::Off, Effect::Streaming, Effect::Breathing, Effect::Steady,
        Effect::Neon, Effect::Snake, Effect::ColorfulSteady, Effect::Wave,
    ];

    pub fn id(self) -> u8 {
        self as u8
    }

    pub fn from_id(id: u8) -> Option<Effect> {
        Effect::ALL.into_iter().find(|e| e.id() == id)
    }

    /// Effects whose animation speed matters (vendor UI hides it for the rest).
    pub fn has_speed(self) -> bool {
        !matches!(self, Effect::Off | Effect::Steady | Effect::ColorfulSteady)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Forward = 0,
    Reverse = 1,
}

/// Lighting setup. Colors are `0xRRGGBB`; brightness 1..=4, speed 1..=3.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lighting {
    pub effect: Effect,
    pub brightness: u8,
    pub speed: u8,
    pub direction: Direction,
    pub steady: u32,
    pub breathing: Vec<u32>,
    pub colorful: [u32; COLORFUL_SLOTS],
}

impl Default for Lighting {
    /// Vendor defaults: rainbow streaming, medium brightness and speed.
    fn default() -> Self {
        Self {
            effect: Effect::Streaming,
            brightness: 2,
            speed: 2,
            direction: Direction::Forward,
            steady: 0xFF00FF,
            breathing: vec![0xFF0000, 0x00FF00, 0x0000FF],
            colorful: [0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0x00FFFF, 0xFA00FA, 0xFFFFFF],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lod {
    Mm1 = 1,
    Mm2 = 2,
}

/// Sleep timer and whether the lights go dark while the mouse moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Power {
    pub sleep_min: u8,
    pub dark_while_moving: bool,
}

const CMD_DPI_COLORS: u8 = 0x06;
const CMD_LIGHTING: u8 = 0x08;
const CMD_LOD: u8 = 0x09;
const CMD_DEBOUNCE: u8 = 0x0A;
const CMD_POWER: u8 = 0x0B;

fn push_rgb(out: &mut Vec<u8>, c: u32) {
    out.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8]);
}

/// `[effect, brightness<<4 | speed, params…]`; params depend on the effect.
pub fn lighting_frame(l: &Lighting, seq: u8) -> [u8; REPORT_LEN] {
    let b = l.brightness.clamp(1, BRIGHTNESS_MAX);
    let s = l.speed.clamp(1, SPEED_MAX);
    let mut d = vec![l.effect.id(), (b << 4) | s];

    match l.effect {
        Effect::Streaming => d.push(l.direction as u8),
        Effect::Steady => push_rgb(&mut d, l.steady),
        Effect::Breathing => {
            let colors: Vec<u32> = l.breathing.iter().copied().filter(|&c| c != 0).take(BREATHING_MAX).collect();
            d.push(colors.len() as u8);
            colors.iter().for_each(|&c| push_rgb(&mut d, c));
        }
        Effect::ColorfulSteady => l.colorful.iter().for_each(|&c| push_rgb(&mut d, c)),
        _ => {}
    }
    frame(CMD_LIGHTING, seq, &d)
}

/// Colors of the DPI indicator LED, one per stage (8 slots, 6 used).
pub fn dpi_colors_frame(colors: &[u32; DPI_COLOR_SLOTS], seq: u8) -> [u8; REPORT_LEN] {
    let mut d = Vec::with_capacity(DPI_COLOR_SLOTS * 3);
    colors.iter().for_each(|&c| push_rgb(&mut d, c));
    frame(CMD_DPI_COLORS, seq, &d)
}

/// Button debounce in milliseconds (vendor default 12).
pub fn debounce_frame(ms: u8, seq: u8) -> [u8; REPORT_LEN] {
    frame(CMD_DEBOUNCE, seq, &[ms])
}

pub fn lod_frame(lod: Lod, seq: u8) -> [u8; REPORT_LEN] {
    frame(CMD_LOD, seq, &[lod as u8, 0])
}

pub fn power_frame(p: &Power, seq: u8) -> [u8; REPORT_LEN] {
    frame(CMD_POWER, seq, &[p.sleep_min, p.dark_while_moving as u8])
}

/// Parses a status reply. `None` when the mouse is off or out of range.
pub fn parse_status(r: &[u8]) -> Option<Status> {
    if r.len() < 0x16 || r[0] != REPORT_ID || r[2] != CMD_STATUS || r[3] != 0 {
        return None;
    }
    if &r[8..12] != STATUS_SIGNATURE {
        return None;
    }

    let charge = match r[0x13] {
        1 => Charge::Charging,
        2 => Charge::Full,
        _ => Charge::Discharging,
    };
    let battery = if charge == Charge::Full { 100 } else { r[0x12].min(100) };
    let active_stage = ((r[0x10] >> 4) as usize).saturating_sub(1).min(STAGE_COUNT - 1);

    let mut raw = [0u8; REPORT_LEN];
    let n = r.len().min(REPORT_LEN);
    raw[..n].copy_from_slice(&r[..n]);
    Some(Status { battery, charge, active_stage, sensor: r[0x0F], raw })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live reply captured from the user's mouse (60 %, stage 2 of 6, PAW3325).
    const CAPTURED: &str = "0c 01 20 00 00 01 10 00 4d 30 32 32 25 01 00 25 26 ff 3c 00 ff 00 00 00 37 f8 60 38 ff c7 67 09 6d";

    fn hex(s: &str) -> Vec<u8> {
        s.split(' ').map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    #[test]
    fn parses_captured_status() {
        let s = parse_status(&hex(CAPTURED)).unwrap();
        assert_eq!((s.battery, s.charge, s.active_stage, s.sensor), (60, Charge::Discharging, 1, 0x25));
        assert_eq!(s.raw.to_vec(), hex(CAPTURED));
    }

    #[test]
    fn rejects_missing_signature() {
        let mut r = hex(CAPTURED);
        r[8] = 0;
        assert!(parse_status(&r).is_none());
    }

    #[test]
    fn full_charge_reads_100() {
        let mut r = hex(CAPTURED);
        r[0x13] = 2;
        assert_eq!(parse_status(&r).unwrap().battery, 100);
    }

    #[test]
    fn dpi_frame_matches_verified_capture() {
        // Frame sent and accepted by the mouse during protocol verification.
        let cfg = DpiConfig { stages: [6, 12, 22, 30, 51, 58], enabled: 0x3F, active: 1 };
        let expected = hex("0c 01 05 00 00 01 12 26 3f 07 00 0d 00 17 00 1f 00 34 00 3b 00 00 00 00 00 00 00 00 00 00 00 00 00");
        assert_eq!(dpi_frame(&cfg, 0).to_vec(), expected);
    }

    #[test]
    fn polling_frame_uses_interval() {
        let f = polling_frame(PollingRate::Hz1000, 1);
        assert_eq!(&f[..8], &[0x0c, 0x01, 0x07, 0x00, 0x01, 0x01, 0x01, 0x01]);
        assert_eq!(polling_frame(PollingRate::Hz125, 0)[7], 8);
    }

    fn data(f: &[u8; REPORT_LEN]) -> &[u8] {
        &f[DATA_OFFSET..DATA_OFFSET + f[6] as usize]
    }

    #[test]
    fn steady_matches_live_test() {
        // Sent to the mouse: it turned green after waking up.
        let l = Lighting { effect: Effect::Steady, brightness: 4, speed: 1, steady: 0x00FF00, ..Lighting::default() };
        let f = lighting_frame(&l, 0);
        assert_eq!(f[2], 0x08);
        assert_eq!(data(&f), &[0x03, 0x41, 0x00, 0xFF, 0x00]);
    }

    #[test]
    fn streaming_carries_direction() {
        let l = Lighting { effect: Effect::Streaming, brightness: 2, speed: 2, direction: Direction::Reverse, ..Lighting::default() };
        assert_eq!(data(&lighting_frame(&l, 0)), &[0x01, 0x22, 0x01]);
    }

    #[test]
    fn breathing_packs_colors() {
        // Spec example: red + blue, brightness 4, speed 2.
        let l = Lighting { effect: Effect::Breathing, brightness: 4, speed: 2, breathing: vec![0xFF0000, 0x0000FF], ..Lighting::default() };
        assert_eq!(data(&lighting_frame(&l, 0)), &[0x02, 0x42, 0x02, 0xFF, 0, 0, 0, 0, 0xFF]);
    }

    #[test]
    fn breathing_caps_at_seven_colors() {
        let l = Lighting { effect: Effect::Breathing, breathing: vec![0x111111; 9], ..Lighting::default() };
        let f = lighting_frame(&l, 0);
        assert_eq!(f[6] as usize, 3 + 3 * 7);
        assert_eq!(f[DATA_OFFSET + 2], 7);
    }

    #[test]
    fn dpi_colors_frame_layout() {
        let mut c = [0u32; DPI_COLOR_SLOTS];
        c[0] = 0xFF0000;
        c[1] = 0x00FF00;
        let f = dpi_colors_frame(&c, 0);
        assert_eq!((f[2], f[6]), (0x06, 0x18));
        assert_eq!(&f[7..13], &[0xFF, 0, 0, 0, 0xFF, 0]);
    }

    #[test]
    fn single_value_frames() {
        assert_eq!(data(&debounce_frame(12, 0)), &[12]);
        assert_eq!(debounce_frame(12, 0)[2], 0x0A);
        assert_eq!(data(&lod_frame(Lod::Mm2, 0)), &[2, 0]);
        assert_eq!(lod_frame(Lod::Mm1, 0)[7], 1);
        let p = Power { sleep_min: 6, dark_while_moving: true };
        assert_eq!((power_frame(&p, 0)[2], data(&power_frame(&p, 0))), (0x0B, &[6u8, 1][..]));
    }

    #[test]
    fn sensor_tables_match_vendor_app() {
        // Spot checks against the emulated vendor tables.
        let get = |id: u8| SENSORS.iter().find(|s| s.id == id).unwrap();
        assert_eq!((get(0x25).values.len(), get(0x25).first_code), (59, 1));
        assert_eq!(*get(0x95).values.last().unwrap() as u32 * DPI_UNIT, 26000);
        assert_eq!((get(0x04).first_code, get(0x04).values[0]), (0, 2));
        assert_eq!(get(0x70).values.len(), 290);
    }

    #[test]
    fn dpi_table_bounds() {
        assert_eq!(dpi_of(0), 200);
        assert_eq!(dpi_of(58), 10000);
    }
}
