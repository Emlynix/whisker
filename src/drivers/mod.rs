//! Drivers for mice beyond the Lionel family. Each driver owns a HID
//! channel and offers the same small surface: identify, read status,
//! set DPI and report rate.
//!
//! ```text
//!   mouse service ──> Driver::{Hidpp, Razer} ──> hid_link::raw::Channel
//! ```

pub mod asus;
pub mod corsair;
pub mod eternico;
pub mod hidpp;
pub mod oemhid;
pub mod razer;
pub mod roccat;
pub mod sinowealth;
pub mod steelseries;

/// DPI values a mouse accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DpiRange {
    Steps { min: u32, max: u32, step: u32 },
    List(Vec<u32>),
}

impl DpiRange {
    /// All selectable values in order (for a slider).
    pub fn values(&self) -> Vec<u32> {
        match self {
            DpiRange::Steps { min, max, step } => (*min..=*max).step_by((*step).max(1) as usize).collect(),
            DpiRange::List(v) => v.clone(),
        }
    }
}

/// What the opened mouse can do, learned when it is opened.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ability {
    pub dpi: Option<DpiRange>,
    /// Report rates in Hz, ascending.
    pub rates: Vec<u32>,
    pub battery: bool,
    /// Number of DPI stages, for mice that only report the stage.
    pub stages: Option<u8>,
}

/// Identity details for the Device page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ident {
    pub name: String,
    pub firmware: String,
    pub serial: String,
    pub protocol: String,
}

/// One status poll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Reading {
    pub battery: Option<u8>,
    pub charging: bool,
    pub dpi: Option<u32>,
    pub rate_hz: Option<u32>,
    /// Current DPI stage (1-based), for mice that only report the stage.
    pub stage: Option<u8>,
}

/// Battery % from a Li-ion cell voltage (rough discharge curve).
pub fn percent_from_mv(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 7] = [(4150, 100), (4000, 85), (3900, 70), (3800, 50), (3700, 30), (3600, 10), (3500, 0)];
    if mv >= CURVE[0].0 {
        return 100;
    }
    for w in CURVE.windows(2) {
        let ((hv, hp), (lv, lp)) = (w[0], w[1]);
        if mv >= lv {
            let t = (mv - lv) as f32 / (hv - lv) as f32;
            return lp + ((hp - lp) as f32 * t).round() as u8;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_range_values() {
        let r = DpiRange::Steps { min: 200, max: 400, step: 100 };
        assert_eq!(r.values(), vec![200, 300, 400]);
    }

    #[test]
    fn voltage_curve() {
        assert_eq!(percent_from_mv(4200), 100);
        assert_eq!(percent_from_mv(3750), 40);
        assert_eq!(percent_from_mv(3400), 0);
    }
}
