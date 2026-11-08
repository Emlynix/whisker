//! Catalogue of supported mice and matching of connected ones to it.
//! Adding a mouse of a known protocol family is one entry in `MODELS`.

use crate::hid_link::DeviceInfo;
use crate::mouse::Target;

/// Protocol family; one driver per family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// Telink-based OEM mice, status signature "M022" (White Shark Lionel…).
    TelinkM022,
    /// Logitech HID++ 2.0.
    Hidpp,
    Razer,
    /// Eternico receiver: listen-only status (battery, DPI stage).
    Eternico,
    /// Sinowealth OEM gaming mice (Glorious, Marvo, Genesis…).
    Sinowealth,
    SteelSeries,
    /// One Chinese OEM protocol behind ATK, VXE, VGN, Lamzu web apps.
    OemHid,
    /// Corsair wireless mice, Bragi protocol.
    Corsair,
    /// ASUS ROG mice, 64-byte vendor reports.
    Asus,
    /// Recognized, but no configuration protocol is known.
    NoDriver,
}

impl Family {
    /// Families whose driver reports identity and status itself.
    pub fn has_driver(self) -> bool {
        !matches!(self, Family::TelinkM022 | Family::NoDriver)
    }

    pub fn target(self) -> Target {
        match self {
            Family::TelinkM022 => Target::Lionel,
            Family::Hidpp => Target::Hidpp,
            Family::Razer => Target::Razer,
            Family::Eternico => Target::Eternico,
            Family::Sinowealth => Target::Sinowealth,
            Family::SteelSeries => Target::SteelSeries,
            Family::OemHid => Target::OemHid,
            Family::Corsair => Target::Corsair,
            Family::Asus => Target::Asus,
            Family::NoDriver => Target::Nothing,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    /// Verified on real hardware.
    Full,
    /// Driver written from protocol documentation, not yet verified.
    Experimental,
    /// Recognized; statistics only.
    Basic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DpiUi {
    None,
    /// Lionel-style stages with indicator colors.
    Stages,
    /// One DPI value in the range the mouse reports.
    Single,
}

/// Feature switches: the UI only shows pages the model supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub dpi: DpiUi,
    pub polling: bool,
    /// Debounce, lift-off distance, sleep timer.
    pub extras: bool,
    pub lighting: bool,
    pub buttons: bool,
    pub battery: bool,
}

/// Mouse without a driver: statistics only.
pub const CAPS_NONE: Caps = Caps { dpi: DpiUi::None, polling: false, extras: false, lighting: false, buttons: false, battery: false };
const CAPS_ALL: Caps = Caps { dpi: DpiUi::Stages, polling: true, extras: true, lighting: true, buttons: true, battery: true };
/// Listen-only mice: battery and DPI stage, nothing to set.
const CAPS_STATUS: Caps = Caps { dpi: DpiUi::None, polling: false, extras: false, lighting: false, buttons: false, battery: true };
/// Driver-based mice: DPI, report rate, battery.
const CAPS_BASIC: Caps = Caps { dpi: DpiUi::Single, polling: true, extras: false, lighting: false, buttons: false, battery: true };

#[derive(Debug)]
pub struct Model {
    /// Stable id stored in the config, e.g. "whiteshark-lionel".
    pub id: &'static str,
    pub vendor: &'static str,
    pub name: &'static str,
    pub vid: u16,
    /// (product id, connection) pairs; empty = any product of the vendor.
    pub pids: &'static [(u16, Link)],
    pub family: Family,
    pub support: Support,
    pub caps: Caps,
    /// DPI of each stage for mice that only report the stage number.
    pub stage_dpi: &'static [u32],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    Receiver,
    Cable,
}

pub static MODELS: &[Model] = &[
    Model {
        id: "whiteshark-lionel",
        vendor: "White Shark",
        name: "Lionel",
        vid: 0x248A,
        pids: &[(0x5B2F, Link::Receiver), (0x5B2E, Link::Cable)],
        family: Family::TelinkM022,
        support: Support::Full,
        caps: CAPS_ALL,
        stage_dpi: &[],
    },
    Model {
        id: "logitech-hidpp",
        vendor: "Logitech",
        name: "HID++",
        vid: crate::drivers::hidpp::VID,
        pids: &[],
        family: Family::Hidpp,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "razer",
        vendor: "Razer",
        name: "",
        vid: crate::drivers::razer::VID,
        pids: &[],
        family: Family::Razer,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "eternico-ms480b",
        vendor: "Eternico",
        name: "AET-MS480B",
        vid: crate::drivers::eternico::VID,
        pids: &[(crate::drivers::eternico::PID, Link::Receiver)],
        family: Family::Eternico,
        support: Support::Experimental,
        caps: CAPS_STATUS,
        stage_dpi: &[1200, 2400, 3200, 6400],
    },
    // Vendor-wide entries below: each driver picks the exact model by
    // product id (and firmware), and names it itself.
    Model {
        id: "steelseries",
        vendor: "SteelSeries",
        name: "",
        vid: crate::drivers::steelseries::VID,
        pids: &[],
        family: Family::SteelSeries,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "sinowealth",
        vendor: "Sinowealth",
        name: "",
        vid: crate::drivers::sinowealth::VIDS[0],
        pids: &[],
        family: Family::Sinowealth,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "glorious",
        vendor: "Glorious",
        name: "",
        vid: crate::drivers::sinowealth::VIDS[1],
        pids: &[],
        family: Family::Sinowealth,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "atk",
        vendor: "ATK",
        name: "",
        vid: crate::drivers::oemhid::VIDS[0],
        pids: &[],
        family: Family::OemHid,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "vxe-vgn-lamzu",
        vendor: "VXE / VGN / Lamzu",
        name: "",
        vid: crate::drivers::oemhid::VIDS[1],
        pids: &[],
        family: Family::OemHid,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "asus",
        vendor: "ASUS ROG",
        name: "",
        vid: crate::drivers::asus::VID,
        pids: &[],
        family: Family::Asus,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    Model {
        id: "corsair",
        vendor: "Corsair",
        name: "",
        vid: crate::drivers::corsair::VID,
        pids: &[],
        family: Family::Corsair,
        support: Support::Experimental,
        caps: CAPS_BASIC,
        stage_dpi: &[],
    },
    // Receiver reuses an Apple keyboard id (05AC:024F); only its mouse
    // collection is matched, so a real Apple keyboard is never listed.
    // It broadcasts only a link state, no battery or DPI.
    Model {
        id: "eternico-mvs490b",
        vendor: "Eternico",
        name: "AET-MVS490B",
        vid: 0x05AC,
        pids: &[(0x024F, Link::Receiver)],
        family: Family::NoDriver,
        support: Support::Basic,
        caps: CAPS_NONE,
        stage_dpi: &[],
    },
];

pub fn by_id(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

/// How a connected mouse relates to the catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// VID and PID are listed.
    Known(&'static Model),
    /// Same vendor id as a listed model: probably the same family.
    Guess(&'static Model),
    Unknown,
}

pub fn match_device(d: &DeviceInfo) -> Match {
    let listed = |m: &&Model| m.vid == d.vid && (m.pids.is_empty() || m.pids.iter().any(|(p, _)| *p == d.pid));
    if let Some(m) = MODELS.iter().find(listed) {
        return Match::Known(m);
    }
    match MODELS.iter().find(|m| m.vid == d.vid) {
        Some(m) => Match::Guess(m),
        None => Match::Unknown,
    }
}

/// Models are identified by their id.
impl PartialEq for Model {
    fn eq(&self, o: &Self) -> bool {
        self.id == o.id
    }
}
impl Eq for Model {}

impl Model {
    pub fn full_name(&self) -> String {
        format!("{} {}", self.vendor, self.name).trim().to_string()
    }

    pub fn link_of(&self, pid: u16) -> Option<Link> {
        self.pids.iter().find(|(p, _)| *p == pid).map(|(_, l)| *l)
    }
}

/// PixArt sensor reported in the status reply (vendor app table).
pub fn sensor_name(code: u8) -> Option<&'static str> {
    crate::protocol::SENSORS.iter().find(|s| s.id == code).map(|s| s.name)
}

/// `bcdDevice` as a version, e.g. 0x0177 -> "1.77".
pub fn bcd_version(v: u16) -> String {
    format!("{:x}.{:02x}", v >> 8, v & 0xFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(vid: u16, pid: u16) -> DeviceInfo {
        DeviceInfo { vid, pid, ..DeviceInfo::default() }
    }

    #[test]
    fn matches_known_guess_unknown() {
        assert!(matches!(match_device(&dev(0x248A, 0x5B2F)), Match::Known(m) if m.id == "whiteshark-lionel"));
        assert!(matches!(match_device(&dev(0x248A, 0x1234)), Match::Guess(_)));
        assert!(matches!(match_device(&dev(0x046D, 0xC539)), Match::Known(m) if m.family == Family::Hidpp));
        assert_eq!(match_device(&dev(0x0458, 0x0007)), Match::Unknown);
    }

    #[test]
    fn versions_and_sensors() {
        assert_eq!(bcd_version(0x0177), "1.77");
        assert_eq!(sensor_name(0x25), Some("PixArt PAW3325"));
        assert_eq!(sensor_name(0x00), None);
    }
}
