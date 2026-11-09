//! SteelSeries mice (experimental, untested on hardware). Protocol from
//! rivalcfg (github.com/flozz/rivalcfg, WTFPL): plain output reports,
//! report id 0, on one USB interface per model.
//!
//! ```text
//! output report:  00 | command… | args… | zero padding to the report size
//! e.g. Rival 3 DPI 200 as the only preset:  00 | 0B 00 | 01 01 04
//!                                                cmd    count preset code
//! Aerox-style 2.4 GHz dongles: command[0] |= 0x40, each write is answered
//! by a 64-byte reply that is read and dropped.
//! ```
//! The mice are write-only: nothing reads DPI or report rate back, only
//! battery (some models) and firmware (some models). Settings are saved to
//! flash after each change, as `rivalcfg` does.

use super::{Ability, DpiRange, Ident, Pick, Reading};
use crate::hid_link::raw::{Channel, Interface};
use std::time::Duration;

pub const VID: u16 = 0x1038;

const REPORT_ID: u8 = 0x00;
/// Set in command[0] for Aerox-style 2.4 GHz dongles.
const WIRELESS_FLAG: u8 = 0x40;
/// Large enough for any input report of these mice, including the
/// 64-byte dongle readback (ReadFile needs at least the report size).
const READ_LEN: usize = 1025;
/// rivalcfg reads replies with a 200 ms timeout.
const REPLY_WAIT: Duration = Duration::from_millis(200);
const WRITE_WAIT: Duration = Duration::from_millis(200);
/// Pause after each command; faster sequences can hang some models.
const COMMAND_DELAY: Duration = Duration::from_millis(50);
const DRAIN_WAIT: Duration = Duration::from_millis(5);
const DRAIN_MAX: usize = 8;
const VENDOR_PAGE: u16 = 0xFF00;
/// We always write a single DPI preset and select it.
const ONE_PRESET: u8 = 1;
/// Preset slots of the `xxyy` layout (unused slots are zero).
const XXYY_SLOTS: usize = 5;
/// Legacy mice hold two presets toggled by the DPI button.
const LEGACY_STAGES: [u8; 2] = [1, 2];
const FULL: u8 = 100;
const PROTOCOL: &str = "SteelSeries (experimental)";

/// How DPI maps to the byte(s) sent.
#[derive(Clone, Copy, Debug)]
enum Scale {
    /// Sensor lookup table: `codes[i]` is the code for `min + i * step`.
    Table { min: u32, step: u32, codes: &'static [u8] },
    /// Linear: code = `base + (dpi - min) / step`.
    Linear { min: u32, max: u32, step: u32, base: u16 },
}

/// Layout of the X/Y codes of an X/Y preset command.
#[derive(Clone, Copy, Debug)]
enum XyLayout {
    /// `x1 y1 x2 y2 …`
    Xyxy,
    /// `x1 … x5 y1 … y5`
    Xxyy,
}

/// DPI command family.
#[derive(Clone, Copy, Debug)]
enum Dpi {
    /// `cmd, count, preset, code` (code is `width` bytes, little endian).
    Preset { cmd: &'static [u8], first: u8, width: usize, scale: Scale },
    /// `cmd, count, preset, x/y codes` (1 byte each).
    PresetXy { cmd: &'static [u8], first: u8, layout: XyLayout, scale: Scale },
    /// One command per stage: `cmd, stage, code, suffix`.
    Stages { cmd: &'static [u8], suffix: &'static [u8], scale: Scale },
}

/// Battery query and reply format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Battery {
    None,
    /// `92` → `_ BB`: bit 7 charging, level = ((BB & 0x7F) - 1) * 5.
    Aerox,
    /// `AA 01` → `LL _ CC`: level %, CC != 0 charging.
    Rival,
}

/// How the mouse is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Cable,
    /// 2.4 GHz dongle using the cable command set.
    Radio,
    /// Aerox-style 2.4 GHz dongle: wireless flag and readback.
    RadioFlag,
}

struct Model {
    name: &'static str,
    pids: &'static [u16],
    /// USB interface number (Windows path `mi_XX`).
    iface: u8,
    link: Link,
    dpi: Dpi,
    rate_cmd: &'static [u8],
    rates: &'static [(u32, u8); 4],
    save: &'static [u8],
    battery: Battery,
    /// Command and reply length.
    firmware: Option<(&'static [u8], usize)>,
}

/// TrueMove Core codes, 200..=8500 step 100. The PMW3326 table is the
/// first 71 entries (200..=7200).
const CORE_CODES: [u8; 84] = [
    0x04, 0x06, 0x08, 0x0B, 0x0D, 0x0F, 0x12, 0x14, 0x16, 0x19, 0x1B, 0x1D, 0x20, 0x22, 0x24, 0x27, //
    0x29, 0x2B, 0x2E, 0x30, 0x32, 0x34, 0x37, 0x39, 0x3B, 0x3E, 0x40, 0x42, 0x45, 0x47, 0x49, 0x4C, //
    0x4E, 0x50, 0x53, 0x55, 0x57, 0x5A, 0x5C, 0x5E, 0x61, 0x63, 0x65, 0x68, 0x6A, 0x6C, 0x6F, 0x71, //
    0x73, 0x76, 0x78, 0x7A, 0x7D, 0x7F, 0x81, 0x84, 0x86, 0x88, 0x8B, 0x8D, 0x8F, 0x92, 0x94, 0x96, //
    0x99, 0x9B, 0x9D, 0xA0, 0xA2, 0xA4, 0xA7, 0xA9, 0xAB, 0xAD, 0xB0, 0xB2, 0xB4, 0xB7, 0xB9, 0xBC, //
    0xBE, 0xC0, 0xC3, 0xC5,
];
const PMW3326_LEN: usize = 71;

/// TrueMove Air codes, 100..=18000 step 100.
const AIR_CODES: [u8; 180] = [
    0x00, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x10, 0x11, 0x12, //
    0x13, 0x14, 0x16, 0x17, 0x18, 0x19, 0x1A, 0x1B, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x23, 0x25, 0x26, //
    0x27, 0x28, 0x29, 0x2A, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x32, 0x33, 0x34, 0x35, 0x36, 0x38, 0x39, //
    0x3A, 0x3B, 0x3C, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x44, 0x45, 0x46, 0x47, 0x48, 0x4A, 0x4B, 0x4C, //
    0x4D, 0x4E, 0x50, 0x51, 0x52, 0x53, 0x54, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x5C, 0x5D, 0x5E, 0x5F, //
    0x60, 0x62, 0x63, 0x64, 0x65, 0x66, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6E, 0x6F, 0x70, 0x71, 0x72, //
    0x74, 0x75, 0x76, 0x77, 0x78, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E, 0x80, 0x81, 0x82, 0x83, 0x84, 0x86, //
    0x87, 0x88, 0x89, 0x8A, 0x8C, 0x8D, 0x8E, 0x8F, 0x90, 0x92, 0x93, 0x94, 0x95, 0x96, 0x98, 0x99, //
    0x9A, 0x9B, 0x9C, 0x9E, 0x9F, 0xA0, 0xA1, 0xA2, 0xA4, 0xA5, 0xA6, 0xA7, 0xA8, 0xAA, 0xAB, 0xAC, //
    0xAD, 0xAE, 0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xBB, 0xBC, 0xBD, //
    0xBF, 0xC0, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCF, 0xD0, 0xD1, //
    0xD2, 0xD3, 0xD5, 0xD6,
];

const CORE: Scale = Scale::Table { min: 200, step: 100, codes: &CORE_CODES };
const PMW3326: Scale = Scale::Table { min: 200, step: 100, codes: CORE_CODES.split_at(PMW3326_LEN).0 };
const AIR: Scale = Scale::Table { min: 100, step: 100, codes: &AIR_CODES };
/// Prime / Sensei Ten: 50..=18000 step 50 → 0x0001..=0x0168.
const FINE: Scale = Scale::Linear { min: 50, max: 18000, step: 50, base: 1 };
/// Rival 310/600/650/700: 100..=12000 step 100 → 0x00..=0x77.
const LEGACY: Scale = Scale::Linear { min: 100, max: 12000, step: 100, base: 0 };

/// Report rate codes (Hz, code).
const RATES_A: [(u32, u8); 4] = [(125, 0x04), (250, 0x03), (500, 0x02), (1000, 0x01)];
const RATES_B: [(u32, u8); 4] = [(125, 0x03), (250, 0x02), (500, 0x01), (1000, 0x00)];

const FW_10: Option<(&[u8], usize)> = Some((&[0x10, 0x00], 2));
const FW_90: Option<(&[u8], usize)> = Some((&[0x90, 0x00], 2));

/// Aerox 3 Wireless / 5 / 5 Wireless / 9 Wireless, Prime Wireless, Rival 5.
const AIR_2D: Dpi = Dpi::Preset { cmd: &[0x2D], first: 0, width: 1, scale: AIR };

/// Aerox 3W / 5W / 9W and Prime Wireless share one command set (cable).
const fn aerox_w(name: &'static str, pids: &'static [u16], link: Link) -> Model {
    Model {
        name,
        pids,
        iface: 3,
        link,
        dpi: AIR_2D,
        rate_cmd: &[0x2B],
        rates: &RATES_B,
        save: &[0x11, 0x00],
        battery: Battery::Aerox,
        firmware: None,
    }
}

/// Two-stage legacy mice (Rival 310/600/700/710, Sensei 310).
const fn legacy(name: &'static str, pids: &'static [u16], dpi: &'static [u8], rate: &'static [u8], save: &'static [u8]) -> Model {
    Model {
        name,
        pids,
        iface: 0,
        link: Link::Cable,
        dpi: Dpi::Stages { cmd: dpi, suffix: &[0x00, 0x42], scale: LEGACY },
        rate_cmd: rate,
        rates: &RATES_A,
        save,
        battery: Battery::None,
        firmware: None,
    }
}

/// Supported models; one entry per command set and link.
const MODELS: [Model; 25] = [
    Model {
        name: "Rival 3",
        pids: &[0x1824, 0x184C],
        iface: 3,
        link: Link::Cable,
        dpi: Dpi::Preset { cmd: &[0x0B, 0x00], first: 1, width: 1, scale: CORE },
        rate_cmd: &[0x04, 0x00],
        rates: &RATES_A,
        save: &[0x09, 0x00],
        battery: Battery::None,
        firmware: FW_10,
    },
    Model {
        name: "Rival 3 Gen 2",
        pids: &[0x1870],
        iface: 3,
        link: Link::Cable,
        dpi: Dpi::PresetXy { cmd: &[0x34], first: 0, layout: XyLayout::Xyxy, scale: CORE },
        rate_cmd: &[0x2B],
        rates: &RATES_A,
        save: &[0x11, 0x00],
        battery: Battery::None,
        firmware: None,
    },
    Model {
        name: "Rival 3 Wireless",
        pids: &[0x1830],
        iface: 3,
        link: Link::Radio,
        dpi: Dpi::Preset { cmd: &[0x20], first: 1, width: 2, scale: AIR },
        rate_cmd: &[0x17],
        rates: &RATES_B,
        save: &[0x09],
        battery: Battery::Rival,
        firmware: FW_90,
    },
    Model {
        name: "Rival 3 Wireless Gen 2",
        pids: &[0x1872],
        iface: 3,
        link: Link::Radio,
        dpi: Dpi::PresetXy { cmd: &[0x2C], first: 1, layout: XyLayout::Xxyy, scale: AIR },
        rate_cmd: &[0x17],
        rates: &RATES_B,
        save: &[0x09],
        battery: Battery::Rival,
        firmware: None,
    },
    Model {
        name: "Rival 5",
        pids: &[0x183C, 0x183E],
        iface: 0,
        link: Link::Cable,
        dpi: AIR_2D,
        rate_cmd: &[0x2B],
        rates: &RATES_A,
        save: &[0x11, 0x00],
        battery: Battery::None,
        firmware: None,
    },
    Model {
        name: "Aerox 3",
        pids: &[0x1836],
        iface: 3,
        link: Link::Cable,
        dpi: Dpi::Preset { cmd: &[0x2D], first: 1, width: 1, scale: CORE },
        rate_cmd: &[0x2B],
        rates: &RATES_A,
        save: &[0x11, 0x00],
        battery: Battery::None,
        firmware: None,
    },
    aerox_w("Aerox 3 Wireless", &[0x183A, 0x187A], Link::Cable),
    aerox_w("Aerox 3 Wireless", &[0x1838, 0x1878], Link::RadioFlag),
    Model {
        name: "Aerox 5",
        pids: &[0x1850],
        iface: 3,
        link: Link::Cable,
        dpi: AIR_2D,
        rate_cmd: &[0x2B],
        rates: &RATES_A,
        save: &[0x11, 0x00],
        battery: Battery::None,
        firmware: None,
    },
    aerox_w("Aerox 5 Wireless", &[0x1854, 0x185E, 0x1862], Link::Cable),
    aerox_w("Aerox 5 Wireless", &[0x1852, 0x185C, 0x1860], Link::RadioFlag),
    aerox_w("Aerox 9 Wireless", &[0x185A, 0x1876], Link::Cable),
    aerox_w("Aerox 9 Wireless", &[0x1858, 0x1874], Link::RadioFlag),
    aerox_w("Prime Wireless", &[0x1842, 0x184A], Link::Cable),
    aerox_w("Prime Wireless", &[0x1840, 0x1848], Link::RadioFlag),
    Model {
        name: "Prime / Prime+",
        pids: &[0x182E, 0x182A, 0x1856, 0x182C],
        iface: 0,
        link: Link::Cable,
        dpi: Dpi::Preset { cmd: &[0x61], first: 0, width: 2, scale: FINE },
        rate_cmd: &[0x5D],
        rates: &RATES_A,
        save: &[0x59],
        battery: Battery::None,
        firmware: None,
    },
    Model {
        name: "Prime Mini",
        pids: &[0x184D],
        iface: 3,
        link: Link::Cable,
        dpi: Dpi::Preset { cmd: &[0x2D], first: 0, width: 2, scale: FINE },
        rate_cmd: &[0x2B],
        rates: &RATES_A,
        save: &[0x11, 0x00],
        battery: Battery::None,
        firmware: None,
    },
    Model {
        name: "Sensei Ten",
        pids: &[0x1832, 0x1834],
        iface: 0,
        link: Link::Cable,
        dpi: Dpi::Preset { cmd: &[0x55, 0x00], first: 1, width: 2, scale: FINE },
        rate_cmd: &[0x54, 0x00],
        rates: &RATES_A,
        save: &[0x59, 0x00],
        battery: Battery::None,
        firmware: FW_90,
    },
    Model {
        name: "Rival 110",
        pids: &[0x1729, 0x1816],
        iface: 0,
        link: Link::Cable,
        dpi: Dpi::Stages { cmd: &[0x03], suffix: &[], scale: PMW3326 },
        rate_cmd: &[0x04, 0x00],
        rates: &RATES_A,
        save: &[0x09, 0x00],
        battery: Battery::None,
        firmware: None,
    },
    Model { firmware: FW_90, ..legacy("Rival 310", &[0x1720, 0x171E, 0x1736], &[0x53, 0x00], &[0x54, 0x00], &[0x59, 0x00]) },
    Model { firmware: FW_90, ..legacy("Sensei 310", &[0x1722], &[0x53, 0x00], &[0x54, 0x00], &[0x59, 0x00]) },
    legacy("Rival 600", &[0x1724, 0x172E], &[0x03, 0x00], &[0x04, 0x00], &[0x09, 0x00]),
    Model { firmware: FW_10, ..legacy("Rival 700 / 710", &[0x1700, 0x1730], &[0x03, 0x00], &[0x04, 0x00], &[0x09, 0x00]) },
    Model {
        dpi: Dpi::Stages { cmd: &[0x15], suffix: &[], scale: LEGACY },
        battery: Battery::Rival,
        ..legacy("Rival 650 Wireless", &[0x172B], &[], &[0x17], &[0x09])
    },
    Model {
        dpi: Dpi::Stages { cmd: &[0x15], suffix: &[], scale: LEGACY },
        battery: Battery::Rival,
        link: Link::Radio,
        ..legacy("Rival 650 Wireless", &[0x1726], &[], &[0x17], &[0x09])
    },
];

fn model(pid: u16) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.pids.contains(&pid))
}

impl Scale {
    /// Code for the nearest supported DPI. Tables round ties down, linear
    /// scales up (as rivalcfg does). E.g. AIR 18000 → 0xD6, FINE 100 → 2.
    fn code(&self, dpi: u32) -> u16 {
        match *self {
            Scale::Table { min, step, codes } => {
                let max = min + step * (codes.len() as u32 - 1);
                let off = dpi.clamp(min, max) - min;
                let mut i = off / step;
                if (off % step) * 2 > step {
                    i += 1;
                }
                codes[i as usize] as u16
            }
            Scale::Linear { min, max, step, base } => {
                let off = dpi.clamp(min, max) - min;
                let mut i = off / step;
                if (off % step) * 2 >= step {
                    i += 1;
                }
                base + i as u16
            }
        }
    }

    fn range(&self) -> DpiRange {
        match *self {
            Scale::Table { min, step, codes } => DpiRange::Steps { min, max: min + step * (codes.len() as u32 - 1), step },
            Scale::Linear { min, max, step, .. } => DpiRange::Steps { min, max, step },
        }
    }
}

impl Dpi {
    fn scale(&self) -> Scale {
        match *self {
            Dpi::Preset { scale, .. } | Dpi::PresetXy { scale, .. } | Dpi::Stages { scale, .. } => scale,
        }
    }
}

/// Command bytes (no report id) that make `dpi` the only active value.
/// E.g. Rival 3, 200 → `[0B 00 01 01 04]`; Rival 310, 1000 →
/// `[53 00 01 09 00 42]` and `[53 00 02 09 00 42]`.
fn dpi_frames(dpi: &Dpi, value: u32) -> Vec<Vec<u8>> {
    let code = dpi.scale().code(value);
    match *dpi {
        Dpi::Preset { cmd, first, width, .. } => {
            let mut f = [cmd, &[ONE_PRESET, first]].concat();
            f.extend_from_slice(&code.to_le_bytes()[..width]);
            vec![f]
        }
        Dpi::PresetXy { cmd, first, layout, .. } => {
            let c = code as u8;
            let mut f = [cmd, &[ONE_PRESET, first]].concat();
            match layout {
                XyLayout::Xyxy => f.extend_from_slice(&[c, c]),
                XyLayout::Xxyy => {
                    let mut axis = [0u8; XXYY_SLOTS];
                    axis[0] = c;
                    f.extend_from_slice(&axis);
                    f.extend_from_slice(&axis);
                }
            }
            vec![f]
        }
        Dpi::Stages { cmd, suffix, .. } => LEGACY_STAGES.iter().map(|&s| [cmd, &[s, code as u8], suffix].concat()).collect(),
    }
}

/// Report rate command for the nearest supported rate.
/// E.g. Rival 3, 1000 Hz → `[04 00 01]`.
fn rate_frame(m: &Model, hz: u32) -> Option<Vec<u8>> {
    let (_, code) = m.rates.iter().min_by_key(|(r, _)| r.abs_diff(hz))?;
    Some([m.rate_cmd, &[*code]].concat())
}

/// Applies the link's command marking. E.g. dongle: `2D …` → `6D …`.
fn mark(link: Link, mut f: Vec<u8>) -> Vec<u8> {
    if link != Link::RadioFlag {
        return f;
    }
    if let Some(b) = f.first_mut() {
        *b |= WIRELESS_FLAG;
    }
    f
}

/// Output report `[00, data…, 0…]` of exactly `len` bytes (the size
/// WriteFile requires). `None` if the data does not fit.
fn report(data: &[u8], len: usize) -> Option<Vec<u8>> {
    if data.len() + 1 > len {
        return None;
    }
    let mut r = vec![0u8; len];
    r[0] = REPORT_ID;
    r[1..=data.len()].copy_from_slice(data);
    Some(r)
}

/// Input report payload: Windows puts the report id first; drop it when
/// it is 0 (no numbered reports), as hidapi does.
fn payload(r: &[u8]) -> &[u8] {
    match r.first() {
        Some(&REPORT_ID) => &r[1..],
        _ => r,
    }
}

/// Battery % and charging from a reply payload; `None` when out of range
/// (mouse off). E.g. Aerox `[92 85]` → 20 %, charging.
fn parse_battery(kind: Battery, p: &[u8]) -> Option<(u8, bool)> {
    const CHARGING: u8 = 0x80;
    const AEROX_STEP: u8 = 5;
    let (level, charging) = match kind {
        Battery::None => return None,
        Battery::Aerox => {
            let b = *p.get(1)?;
            ((b & !CHARGING).checked_sub(1)?.checked_mul(AEROX_STEP)?, b & CHARGING != 0)
        }
        Battery::Rival => (*p.first()?, *p.get(2)? != 0),
    };
    (level <= FULL).then_some((level, charging))
}

fn battery_cmd(kind: Battery) -> Option<(&'static [u8], usize)> {
    match kind {
        Battery::None => None,
        Battery::Aerox => Some((&[0x92], 2)),
        Battery::Rival => Some((&[0xAA, 0x01], 3)),
    }
}

/// USB interface number from a Windows HID path (`…&mi_03#…`); paths of
/// single-interface devices have none, which means interface 0.
fn iface_number(path: &str) -> u8 {
    const TAG: &str = "mi_";
    let p = path.to_lowercase();
    let Some(at) = p.find(TAG) else {
        return 0;
    };
    let digits = p.get(at + TAG.len()..at + TAG.len() + 2).unwrap_or("");
    u8::from_str_radix(digits, 16).unwrap_or(0)
}

pub struct SteelSeries {
    ch: Channel,
    model: &'static Model,
    out_len: usize,
}

impl SteelSeries {
    /// Picks the first known model by PID and opens the collection on its
    /// interface that takes output reports (vendor page first). Sends
    /// nothing.
    pub fn open(pick: &Pick) -> Option<(SteelSeries, crate::hid_link::DeviceInfo)> {
        let mut cands: Vec<(Interface, &'static Model)> = pick.interfaces(VID)
            .into_iter()
            .filter(|i| i.output_len > 1)
            .filter_map(|i| model(i.info.pid).map(|m| (i, m)))
            .filter(|(i, m)| iface_number(&i.path) == m.iface)
            .collect();
        cands.sort_by_key(|(i, _)| i.usage_page < VENDOR_PAGE);

        for (iface, model) in cands {
            let Some(ch) = Channel::open(&iface.path) else {
                continue;
            };
            return Some((SteelSeries { ch, model, out_len: iface.output_len }, iface.info));
        }
        None
    }

    /// Drops input reports that arrived since the last exchange.
    fn drain(&self) {
        for _ in 0..DRAIN_MAX {
            if self.ch.read(READ_LEN, DRAIN_WAIT).is_none() {
                return;
            }
        }
    }

    /// Writes one command; dongles answer each with a reply we drop.
    fn send(&self, data: Vec<u8>) -> bool {
        let data = mark(self.model.link, data);
        let Some(r) = report(&data, self.out_len) else {
            return false;
        };
        if !self.ch.write(&r, WRITE_WAIT) {
            return false;
        }
        std::thread::sleep(COMMAND_DELAY);
        if self.model.link == Link::RadioFlag {
            let _ = self.ch.read(READ_LEN, REPLY_WAIT);
        }
        true
    }

    /// Writes a query and returns the reply payload (at least `len` bytes).
    fn query(&self, cmd: &[u8], len: usize) -> Option<Vec<u8>> {
        self.drain();
        let data = mark(self.model.link, cmd.to_vec());
        let r = report(&data, self.out_len)?;
        if !self.ch.write(&r, WRITE_WAIT) {
            return None;
        }
        let reply = self.ch.read(READ_LEN, REPLY_WAIT)?;
        let p = payload(&reply);
        (p.len() >= len).then(|| p[..len].to_vec())
    }

    /// Persists the current settings.
    fn save(&self) -> bool {
        self.send(self.model.save.to_vec())
    }

    pub fn ident(&mut self) -> Ident {
        let mut id = Ident { name: format!("SteelSeries {}", self.model.name), protocol: PROTOCOL.into(), ..Ident::default() };
        let Some((cmd, len)) = self.model.firmware else {
            return id;
        };
        if let Some(v) = self.query(cmd, len) {
            let parts: Vec<String> = v.iter().map(|b| b.to_string()).collect();
            id.firmware = format!("v{}", parts.join("."));
        }
        id
    }

    pub fn ability(&mut self) -> Ability {
        Ability {
            dpi: Some(self.model.dpi.scale().range()),
            rates: self.model.rates.iter().map(|(hz, _)| *hz).collect(),
            battery: self.model.battery != Battery::None,
            stages: None,
        }
    }

    /// Battery only (DPI and rate cannot be read back). `None` when a
    /// wireless mouse does not answer (asleep or off).
    pub fn read(&mut self) -> Option<Reading> {
        let radio = self.model.link != Link::Cable;
        let Some((cmd, len)) = battery_cmd(self.model.battery) else {
            return Some(Reading::default());
        };
        let Some((level, charging)) = self.query(cmd, len).and_then(|p| parse_battery(self.model.battery, &p)) else {
            return if radio { None } else { Some(Reading::default()) };
        };
        Some(Reading { battery: Some(level), charging, ..Reading::default() })
    }

    /// Makes `dpi` (nearest supported) the only preset and saves it.
    pub fn set_dpi(&mut self, dpi: u32) -> bool {
        for f in dpi_frames(&self.model.dpi, dpi) {
            if !self.send(f) {
                return false;
            }
        }
        self.save()
    }

    pub fn set_rate(&mut self, hz: u32) -> bool {
        let Some(f) = rate_frame(self.model, hz) else {
            return false;
        };
        self.send(f) && self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(pid: u16) -> &'static Model {
        model(pid).unwrap()
    }

    /// Frames as sent (after link marking), for comparison with rivalcfg
    /// test vectors (their leading `02 00` = report type + id dropped).
    fn dpi(pid: u16, v: u32) -> Vec<Vec<u8>> {
        dpi_frames(&m(pid).dpi, v).into_iter().map(|f| mark(m(pid).link, f)).collect()
    }

    fn rate(pid: u16, hz: u32) -> Vec<u8> {
        mark(m(pid).link, rate_frame(m(pid), hz).unwrap())
    }

    #[test]
    fn pids_are_unique() {
        let mut all: Vec<u16> = MODELS.iter().flat_map(|m| m.pids.iter().copied()).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n);
    }

    /// Every row is in data/mice.toml with the same DPI and rates.
    #[test]
    fn catalogue_agrees() {
        use crate::devices::{Family, listed_as};
        for m in &MODELS {
            let rates: Vec<u32> = m.rates.iter().map(|(hz, _)| *hz).collect();
            for &pid in m.pids {
                let c = listed_as(VID, Some(pid), m.name).unwrap_or_else(|| panic!("{} {pid:04X} not listed", m.name));
                assert_eq!((c.family, c.dpi.as_ref(), &c.rates), (Family::SteelSeries, Some(&m.dpi.scale().range()), &rates), "{}", m.name);
            }
        }
    }

    #[test]
    fn rival3_core_table() {
        assert_eq!(dpi(0x1824, 200), vec![vec![0x0B, 0x00, 0x01, 0x01, 0x04]]);
        assert_eq!(rate(0x1824, 125), vec![0x04, 0x00, 0x04]);
        assert_eq!(rate(0x1824, 1000), vec![0x04, 0x00, 0x01]);
    }

    #[test]
    fn aerox3_core_table() {
        assert_eq!(dpi(0x1836, 200), vec![vec![0x2D, 0x01, 0x01, 0x04]]);
        assert_eq!(rate(0x1836, 250), vec![0x2B, 0x03]);
    }

    #[test]
    fn air_wired() {
        // prime_wireless_wired / rival5 vectors.
        assert_eq!(dpi(0x1842, 100), vec![vec![0x2D, 0x01, 0x00, 0x00]]);
        assert_eq!(dpi(0x1842, 300), vec![vec![0x2D, 0x01, 0x00, 0x03]]);
        assert_eq!(dpi(0x1842, 18000), vec![vec![0x2D, 0x01, 0x00, 0xD6]]);
        assert_eq!(rate(0x1842, 1000), vec![0x2B, 0x00]);
        assert_eq!(dpi(0x183C, 200), vec![vec![0x2D, 0x01, 0x00, 0x02]]);
        assert_eq!(rate(0x183C, 125), vec![0x2B, 0x04]);
    }

    #[test]
    fn air_dongle_flag() {
        // aerox5_wireless_wireless / aerox3_wireless_wireless vectors.
        assert_eq!(dpi(0x1852, 18000), vec![vec![0x6D, 0x01, 0x00, 0xD6]]);
        assert_eq!(dpi(0x1838, 200), vec![vec![0x6D, 0x01, 0x00, 0x02]]);
        assert_eq!(rate(0x1852, 125), vec![0x6B, 0x03]);
        assert_eq!(mark(Link::RadioFlag, m(0x1852).save.to_vec()), vec![0x51, 0x00]);
        assert_eq!(mark(Link::RadioFlag, battery_cmd(Battery::Aerox).unwrap().0.to_vec()), vec![0xD2]);
    }

    #[test]
    fn prime_fine_linear() {
        assert_eq!(dpi(0x182E, 100), vec![vec![0x61, 0x01, 0x00, 0x02, 0x00]]);
        assert_eq!(dpi(0x182C, 18000), vec![vec![0x61, 0x01, 0x00, 0x68, 0x01]]);
        assert_eq!(rate(0x182C, 500), vec![0x5D, 0x02]);
    }

    #[test]
    fn sensei_ten() {
        assert_eq!(dpi(0x1832, 200), vec![vec![0x55, 0x00, 0x01, 0x01, 0x04, 0x00]]);
        assert_eq!(rate(0x1832, 125), vec![0x54, 0x00, 0x04]);
    }

    #[test]
    fn rival3_gen2_xyxy() {
        assert_eq!(dpi(0x1870, 200), vec![vec![0x34, 0x01, 0x00, 0x04, 0x04]]);
    }

    #[test]
    fn rival3_wireless() {
        assert_eq!(dpi(0x1830, 100), vec![vec![0x20, 0x01, 0x01, 0x00, 0x00]]);
        assert_eq!(dpi(0x1830, 18000), vec![vec![0x20, 0x01, 0x01, 0xD6, 0x00]]);
        assert_eq!(rate(0x1830, 125), vec![0x17, 0x03]);
        let x = vec![0x2C, 0x01, 0x01, 0x02, 0, 0, 0, 0, 0x02, 0, 0, 0, 0];
        assert_eq!(dpi(0x1872, 200), vec![x]);
    }

    #[test]
    fn legacy_two_stages() {
        let r310 = vec![vec![0x53, 0x00, 0x01, 0x09, 0x00, 0x42], vec![0x53, 0x00, 0x02, 0x09, 0x00, 0x42]];
        assert_eq!(dpi(0x1720, 1000), r310);
        assert_eq!(dpi(0x1722, 12000)[0], vec![0x53, 0x00, 0x01, 0x77, 0x00, 0x42]);
        assert_eq!(dpi(0x1724, 100)[1], vec![0x03, 0x00, 0x02, 0x00, 0x00, 0x42]);
        assert_eq!(dpi(0x1730, 200)[0], vec![0x03, 0x00, 0x01, 0x01, 0x00, 0x42]);
        assert_eq!(rate(0x1720, 250), vec![0x54, 0x00, 0x03]);
        assert_eq!(rate(0x1730, 500), vec![0x04, 0x00, 0x02]);
    }

    #[test]
    fn rival650() {
        assert_eq!(dpi(0x172B, 1000), vec![vec![0x15, 0x01, 0x09], vec![0x15, 0x02, 0x09]]);
        assert_eq!(dpi(0x1726, 12000)[0], vec![0x15, 0x01, 0x77]);
        assert_eq!(rate(0x1726, 125), vec![0x17, 0x04]);
    }

    #[test]
    fn rival110_pmw3326_nearest() {
        assert_eq!(dpi(0x1729, 210)[0], vec![0x03, 0x01, 0x04]);
        assert_eq!(dpi(0x1729, 290)[1], vec![0x03, 0x02, 0x06]);
        assert_eq!(dpi(0x1729, 4000)[0], vec![0x03, 0x01, 0x5C]);
        assert_eq!(dpi(0x1729, 7200)[0], vec![0x03, 0x01, 0xA7]);
        assert_eq!(dpi(0x1729, 9000)[0], vec![0x03, 0x01, 0xA7]);
        assert_eq!(PMW3326.range(), DpiRange::Steps { min: 200, max: 7200, step: 100 });
    }

    #[test]
    fn rounding_matches_rivalcfg() {
        // matches_value_in_range rounds ties up; find_nearest_choice down.
        assert_eq!(LEGACY.code(150), 1);
        assert_eq!(LEGACY.code(149), 0);
        assert_eq!(LEGACY.code(50), 0);
        assert_eq!(CORE.code(250), 0x04);
        assert_eq!(CORE.code(251), 0x06);
    }

    #[test]
    fn battery_replies() {
        assert_eq!(parse_battery(Battery::Aerox, &[0x92, 0x85]), Some((20, true)));
        assert_eq!(parse_battery(Battery::Aerox, &[0x92, 0x15]), Some((100, false)));
        assert_eq!(parse_battery(Battery::Aerox, &[0x92, 0x00]), None);
        assert_eq!(parse_battery(Battery::Aerox, &[0x92, 0x7F]), None);
        assert_eq!(parse_battery(Battery::Rival, &[64, 0, 1]), Some((64, true)));
        assert_eq!(parse_battery(Battery::Rival, &[101, 0, 0]), None);
        assert_eq!(parse_battery(Battery::Rival, &[50]), None);
    }

    #[test]
    fn report_framing() {
        assert_eq!(report(&[0x09], 4), Some(vec![0x00, 0x09, 0x00, 0x00]));
        assert_eq!(report(&[1, 2, 3, 4], 4), None);
        assert_eq!(payload(&[0x00, 0x92, 0x85]), &[0x92, 0x85]);
        assert_eq!(payload(&[0x03, 0x01]), &[0x03, 0x01]);
    }

    #[test]
    fn windows_interface_number() {
        assert_eq!(iface_number(r"\\?\hid#vid_1038&pid_1824&mi_03#7&1b&0&0000#{4d1e55b2}"), 3);
        assert_eq!(iface_number(r"\\?\HID#VID_1038&PID_1720&MI_00&Col02#8&2a"), 0);
        assert_eq!(iface_number(r"\\?\hid#vid_1038&pid_1700#6&1"), 0);
    }
}
