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
pub mod rapoo;
pub mod razer;
pub mod roccat;
pub mod sinowealth;
pub mod steelseries;

/// Which USB device a driver may open: the chosen mouse's vendor and
/// product ids. Empty `pids` = any product of the vendor (auto-detect);
/// `vid: None` = any vendor the driver knows. Two mice of one vendor
/// then open the chosen one, not the first match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pick {
    pub vid: Option<u16>,
    pub pids: Vec<u16>,
}

impl Pick {
    pub fn allows(&self, vid: u16, pid: u16) -> bool {
        self.vid.is_none_or(|v| v == vid) && (self.pids.is_empty() || self.pids.contains(&pid))
    }

    /// HID collections of `vid` this pick allows.
    pub fn interfaces(&self, vid: u16) -> Vec<crate::hid_link::Interface> {
        let all = crate::hid_link::raw::interfaces(vid);
        all.into_iter().filter(|i| self.allows(i.info.vid, i.info.pid)).collect()
    }
}

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
    /// Battery comes in coarse steps (Logitech M705: 90 / 50 / 20 / 5 %)
    /// or from a voltage curve: show it as "≈ 50 %".
    pub battery_approx: bool,
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
    fn pick_limits_vendor_and_products() {
        let any = Pick::default();
        assert!(any.allows(0x1B1C, 0x1B80));
        let vendor = Pick { vid: Some(0x1B1C), pids: Vec::new() };
        assert!(vendor.allows(0x1B1C, 0x1234) && !vendor.allows(0x0B05, 0x1234));
        let chosen = Pick { vid: Some(0x1B1C), pids: vec![0x1B80] };
        assert!(chosen.allows(0x1B1C, 0x1B80) && !chosen.allows(0x1B1C, 0x1B81));
    }

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
