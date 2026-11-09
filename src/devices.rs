//! Catalogue of supported mice, brand -> model, and matching of connected
//! ones to it. Named models come from the drivers' own model tables; each
//! brand with a vendor-wide driver ends in "Other <brand> mouse".
//!
//! ```text
//!   White Shark  -> Lionel, Other White Shark mouse (auto-detect)
//!   Corsair      -> Katar Pro, …, Other Corsair mouse (auto-detect)
//!   Eternico     -> AET-MS480B (partial), AET-MVS490B (stats only)
//!   Other brand  -> Other mouse (stats only)
//! ```

use crate::drivers::Pick;
use crate::hid_link::DeviceInfo;
use crate::i18n::{T, fill, tr};
use crate::mouse::Target;
use std::sync::OnceLock;

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
    /// Roccat Kone family, checksummed settings blocks.
    Roccat,
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
            Family::Roccat => Target::Roccat,
            Family::Corsair => Target::Corsair,
            Family::Asus => Target::Asus,
            Family::NoDriver => Target::Nothing,
        }
    }
}

/// How far Whisker supports a model; ordered best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Support {
    /// Verified on real hardware.
    Full,
    /// Driver written from protocol documentation, not yet verified.
    Experimental,
    /// Some values only, read-only (e.g. battery and DPI stage).
    Partial,
    /// Recognized; statistics only.
    StatsOnly,
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

/// Kind of catalogue entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A named model with its own product ids.
    Named,
    /// "Other <brand> mouse (auto-detect)": any product of the vendor id,
    /// the driver identifies the model itself.
    Auto,
    /// "Other mouse": any brand, statistics only (`store::GENERIC_DEVICE`).
    Other,
}

#[derive(Debug)]
pub struct Model {
    /// Stable id stored in the config: "<brand>/<model>", e.g.
    /// "whiteshark/lionel", "corsair/auto"; "generic" for any other mouse.
    pub id: String,
    /// Brand id, e.g. "corsair".
    pub brand: &'static str,
    /// Model name without the brand, e.g. "Katar Pro"; empty for Auto/Other.
    pub name: &'static str,
    pub entry: Entry,
    pub vid: u16,
    /// Product ids, with the connection where known; empty = any.
    pub pids: Vec<(u16, Option<Link>)>,
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

/// A brand and its models; the Auto entry ("other <brand> mouse") last.
#[derive(Debug)]
pub struct Brand {
    pub id: &'static str,
    /// Trade name; empty when `label` names the group instead.
    pub name: &'static str,
    /// Translated name of a group that is not a trade name.
    pub label: Option<T>,
    /// The brand is part of the model names ("Dream Machines DM5 Blink").
    pub in_names: bool,
    pub models: Vec<Model>,
}

impl Brand {
    pub fn title(&self) -> String {
        self.label.map_or_else(|| self.name.to_string(), |t| tr(t).to_string())
    }

    /// Best support level among the brand's models.
    pub fn best(&self) -> Support {
        self.models.iter().map(|m| m.support).min().unwrap_or(Support::StatsOnly)
    }
}

/// Config ids of the flat catalogue before brands -> brand/model ids.
const OLD_IDS: [(&str, &str); 13] = [
    ("whiteshark-lionel", "whiteshark/lionel"),
    ("logitech-hidpp", "logitech/auto"),
    ("razer", "razer/auto"),
    ("eternico-ms480b", "eternico/aet-ms480b"),
    ("eternico-mvs490b", "eternico/aet-mvs490b"),
    ("steelseries", "steelseries/auto"),
    ("sinowealth", "sinowealth/auto"),
    ("glorious", "glorious/auto"),
    ("atk", "atk/auto"),
    ("vxe-vgn-lamzu", "vxe-vgn-lamzu/auto"),
    ("asus", "asus/auto"),
    ("corsair", "corsair/auto"),
    ("roccat", "roccat/auto"),
];

/// Current id for a stored one (old flat ids are renamed).
pub fn migrate_id(id: &str) -> &str {
    OLD_IDS.iter().find(|(old, _)| *old == id).map_or(id, |(_, new)| new)
}

/// "Katar Pro XT" -> "katar-pro-xt".
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Brand under construction; models of one name merge their product ids
/// (cable and receiver are listed apart by the drivers).
struct Builder {
    brand: Brand,
    family: Family,
    caps: Caps,
}

impl Builder {
    fn new(id: &'static str, name: &'static str, family: Family) -> Builder {
        let brand = Brand { id, name, label: None, in_names: false, models: Vec::new() };
        Builder { brand, family, caps: CAPS_BASIC }
    }

    fn model(&mut self, name: &'static str, vid: u16, pid: Option<u16>) {
        let id = format!("{}/{}", self.brand.id, slug(name));
        if let Some(m) = self.brand.models.iter_mut().find(|m| m.id == id && m.vid == vid) {
            if let Some(p) = pid.filter(|p| !m.pids.iter().any(|(q, _)| q == p)) {
                m.pids.push((p, None));
            }
            return;
        }
        let pids = pid.map(|p| (p, None)).into_iter().collect();
        let (family, caps) = (self.family, self.caps);
        let support = Support::Experimental;
        self.brand.models.push(Model { id, brand: self.brand.id, name, entry: Entry::Named, vid, pids, family, support, caps, stage_dpi: &[] });
    }

    /// Adds "Other <brand> mouse (auto-detect)", the vendor-wide driver entry.
    fn auto(mut self, vid: u16) -> Brand {
        let (family, caps) = (self.family, self.caps);
        let id = format!("{}/auto", self.brand.id);
        let support = Support::Experimental;
        self.brand.models.push(Model { id, brand: self.brand.id, name: "", entry: Entry::Auto, vid, pids: Vec::new(), family, support, caps, stage_dpi: &[] });
        self.brand
    }
}

const WHITE_SHARK_VID: u16 = 0x248A;
const LIONEL_PIDS: [(u16, Option<Link>); 2] = [(0x5B2F, Some(Link::Receiver)), (0x5B2E, Some(Link::Cable))];
const APPLE_VID: u16 = 0x05AC;
const MVS490B_PID: u16 = 0x024F;
const MS480B_STAGES: [u32; 4] = [1200, 2400, 3200, 6400];

/// Brand -> model catalogue, built once from the drivers' model tables.
pub fn brands() -> &'static [Brand] {
    static CATALOGUE: OnceLock<Vec<Brand>> = OnceLock::new();
    CATALOGUE.get_or_init(build)
}

fn build() -> Vec<Brand> {
    use crate::drivers::{asus, corsair, eternico, hidpp, oemhid, razer, roccat, sinowealth, steelseries};
    const GLORIOUS: &str = "Glorious ";
    const ROG: &str = "ROG ";
    const ATK: &str = "ATK ";

    // White Shark: the Lionel is the only Telink M022 model known by name.
    let mut ws = Builder::new("whiteshark", "White Shark", Family::TelinkM022);
    ws.caps = CAPS_ALL;
    ws.model("Lionel", WHITE_SHARK_VID, None);
    let lionel = &mut ws.brand.models[0];
    (lionel.pids, lionel.support) = (LIONEL_PIDS.to_vec(), Support::Full);

    let mut ss = Builder::new("steelseries", "SteelSeries", Family::SteelSeries);
    for (n, p) in steelseries::listed() {
        ss.model(n, steelseries::VID, Some(p));
    }
    let mut co = Builder::new("corsair", "Corsair", Family::Corsair);
    for (n, p) in corsair::listed() {
        co.model(n, corsair::VID, Some(p));
    }
    let mut rog = Builder::new("asus", "ASUS ROG", Family::Asus);
    for (n, p) in asus::listed() {
        rog.model(n.strip_prefix(ROG).unwrap_or(n), asus::VID, Some(p));
    }
    let mut ro = Builder::new("roccat", "Roccat", Family::Roccat);
    for m in roccat::MODELS {
        ro.model(m.name, roccat::VID, Some(m.pid));
    }
    // Sinowealth OEM firmware: Glorious, and other brands under one group.
    let mut gl = Builder::new("glorious", "Glorious", Family::Sinowealth);
    let mut oem = Builder::new("sinowealth", "", Family::Sinowealth);
    (oem.brand.label, oem.brand.in_names) = (Some(T::BrandOem), true);
    for m in sinowealth::MODELS {
        match m.name.strip_prefix(GLORIOUS) {
            Some(n) => gl.model(n, m.vid, Some(m.pid)),
            None => oem.model(m.name, m.vid, Some(m.pid)),
        }
    }
    let mut atk = Builder::new("atk", "ATK", Family::OemHid);
    let mut vxe = Builder::new("vxe-vgn-lamzu", "VXE / VGN / Lamzu", Family::OemHid);
    vxe.brand.in_names = true;
    for (n, vid, pid) in oemhid::listed() {
        match n.strip_prefix(ATK) {
            Some(n) => atk.model(n, vid, pid),
            None => vxe.model(n, vid, pid),
        }
    }

    // Eternico: two receivers, no vendor-wide driver.
    let mut et = Builder::new("eternico", "Eternico", Family::Eternico);
    et.caps = CAPS_STATUS;
    et.model("AET-MS480B", eternico::VID, Some(eternico::PID));
    et.model("AET-MVS490B", APPLE_VID, Some(MVS490B_PID));
    if let [ms, mvs] = &mut et.brand.models[..] {
        // Listen-only: battery and DPI stage, nothing to set.
        (ms.support, ms.stage_dpi, ms.pids[0].1) = (Support::Partial, &MS480B_STAGES, Some(Link::Receiver));
        // Reuses an Apple keyboard id; only its mouse collection is matched.
        // Link state only: statistics, plus a Bluetooth battery where any.
        (mvs.support, mvs.family, mvs.caps, mvs.pids[0].1) = (Support::StatsOnly, Family::NoDriver, CAPS_NONE, Some(Link::Receiver));
    }

    let mut other = Builder::new("other", "", Family::NoDriver).brand;
    other.label = Some(T::OtherBrand);
    other.models.push(Model {
        id: crate::store::GENERIC_DEVICE.into(),
        brand: "other",
        name: "",
        entry: Entry::Other,
        vid: 0,
        pids: Vec::new(),
        family: Family::NoDriver,
        support: Support::StatsOnly,
        caps: CAPS_NONE,
        stage_dpi: &[],
    });

    vec![
        ws.auto(WHITE_SHARK_VID),
        Builder::new("logitech", "Logitech", Family::Hidpp).auto(hidpp::VID),
        Builder::new("razer", "Razer", Family::Razer).auto(razer::VID),
        ss.auto(steelseries::VID),
        co.auto(corsair::VID),
        rog.auto(asus::VID),
        ro.auto(roccat::VID),
        gl.auto(sinowealth::VIDS[1]),
        et.brand,
        atk.auto(oemhid::VIDS[0]),
        vxe.auto(oemhid::VIDS[1]),
        oem.auto(sinowealth::VIDS[0]),
        other,
    ]
}

/// Every catalogue entry, brand by brand.
pub fn models() -> impl Iterator<Item = &'static Model> {
    brands().iter().flat_map(|b| b.models.iter())
}

/// The model before any choice: White Shark Lionel.
pub fn default_model() -> &'static Model {
    &brands()[0].models[0]
}

pub fn brand_of(m: &Model) -> &'static Brand {
    brands().iter().find(|b| b.id == m.brand).unwrap_or(&brands()[0])
}

/// A stored model id, old ids too; `None` for "other mouse" and unknown ids.
pub fn by_id(id: &str) -> Option<&'static Model> {
    let id = migrate_id(id);
    models().find(|m| m.id == id && m.entry != Entry::Other)
}

/// Any entry by id, "other mouse" included (the detect dialog).
pub fn entry(id: &str) -> Option<&'static Model> {
    let id = migrate_id(id);
    models().find(|m| m.id == id)
}

/// How a connected mouse relates to the catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// A listed product, or any product of a vendor with an Auto entry.
    Known(&'static Model),
    /// Same vendor id as a listed model: probably the same family.
    Guess(&'static Model),
    Unknown,
}

/// Named model by product id. Several models on one id (told apart by
/// firmware) or an unlisted product fall back to the brand's Auto entry.
pub fn match_device(d: &DeviceInfo) -> Match {
    let listed = |m: &&Model| m.entry == Entry::Named && m.vid == d.vid && m.pids.iter().any(|(p, _)| *p == d.pid);
    let named: Vec<&Model> = models().filter(listed).collect();
    let auto = models().find(|m| m.entry == Entry::Auto && m.vid == d.vid);
    match (named.as_slice(), auto) {
        ([one], _) => Match::Known(one),
        (_, Some(a)) => Match::Known(a),
        ([first, ..], None) => Match::Known(first),
        ([], None) => models().find(|m| m.entry == Entry::Named && m.vid == d.vid).map_or(Match::Unknown, Match::Guess),
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
    /// "Brand Model", e.g. "Corsair Katar Pro"; the brand alone for Auto.
    pub fn full_name(&self) -> String {
        let brand = brand_of(self);
        if brand.in_names && !self.name.is_empty() {
            return self.name.to_string();
        }
        format!("{} {}", brand.title(), self.name).trim().to_string()
    }

    /// Row title in the model list: the name, "Other Corsair mouse
    /// (auto-detect)" or "Other mouse".
    pub fn title(&self) -> String {
        match self.entry {
            Entry::Named => self.name.to_string(),
            Entry::Auto => fill(tr(T::OtherBrandMouse), &[&brand_of(self).title()]),
            Entry::Other => tr(T::OtherMouse).to_string(),
        }
    }

    pub fn link_of(&self, pid: u16) -> Option<Link> {
        self.pids.iter().find(|(p, _)| *p == pid).and_then(|(_, l)| *l)
    }

    /// The USB devices its driver may open; `usb` = the detected mouse
    /// the user picked, when it is of this vendor.
    pub fn pick(&self, usb: Option<(u16, u16)>) -> Pick {
        let pids = match usb {
            Some((vid, pid)) if vid == self.vid => vec![pid],
            _ => self.pids.iter().map(|(p, _)| *p).collect(),
        };
        Pick { vid: Some(self.vid), pids }
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
    fn matches_models_brands_and_unknown() {
        let known = |vid, pid| match match_device(&dev(vid, pid)) {
            Match::Known(m) => m.id.clone(),
            other => format!("{other:?}"),
        };
        assert_eq!(known(0x248A, 0x5B2F), "whiteshark/lionel");
        // Unlisted product of a vendor with a driver: its auto entry.
        assert_eq!(known(0x248A, 0x1234), "whiteshark/auto");
        assert_eq!(known(0x046D, 0xC539), "logitech/auto");
        assert_eq!(known(0x1B1C, 0x1B81), "corsair/dark-core-rgb-pro");
        // One product id, several firmwares: the driver tells them apart.
        assert_eq!(known(0x258A, 0x0027), "sinowealth/auto");
        // Eternico has no vendor-wide driver: a guess.
        assert!(matches!(match_device(&dev(eternico_vid(), 0x0001)), Match::Guess(_)));
        assert_eq!(match_device(&dev(0x0458, 0x0007)), Match::Unknown);
    }

    fn eternico_vid() -> u16 {
        crate::drivers::eternico::VID
    }

    #[test]
    fn catalogue_is_brand_then_model() {
        let names: Vec<String> = brands().iter().map(|b| b.title()).collect();
        assert_eq!(names.len(), 13);
        assert_eq!(names[0], "White Shark");
        assert_eq!(brands().last().unwrap().models[0].entry, Entry::Other);

        // Ids are unique and stable.
        let mut ids: Vec<&str> = models().map(|m| m.id.as_str()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);

        // Brands with a vendor-wide driver end in their auto entry.
        for b in brands().iter().filter(|b| !matches!(b.id, "eternico" | "other")) {
            assert_eq!(b.models.last().unwrap().entry, Entry::Auto, "{}", b.id);
        }
        // Driver tables fill the brands: cable and receiver ids merge.
        let dark_core = by_id("corsair/dark-core-rgb-pro").unwrap();
        assert_eq!(dark_core.pids.len(), 2);
        assert_eq!(by_id("asus/chakram-x").unwrap().full_name(), "ASUS ROG Chakram X");
        assert_eq!(by_id("glorious/model-o-eternal").unwrap().vid, 0x3794);
        assert!(models().any(|m| m.brand == "sinowealth" && m.full_name() == "Marvo Scorpion G961"));

        // Support per model; the brand dot shows the best one.
        assert_eq!(by_id("eternico/aet-ms480b").unwrap().support, Support::Partial);
        assert_eq!(by_id("eternico/aet-mvs490b").unwrap().support, Support::StatsOnly);
        assert_eq!(brands()[0].best(), Support::Full);
    }

    #[test]
    fn old_ids_still_resolve() {
        assert_eq!(migrate_id("razer"), "razer/auto");
        assert_eq!(migrate_id("corsair/katar-pro"), "corsair/katar-pro");
        for (old, new) in OLD_IDS {
            assert_eq!(by_id(old).map(|m| m.id.as_str()), Some(new), "{old}");
        }
        assert!(by_id(crate::store::GENERIC_DEVICE).is_none());
    }

    #[test]
    fn versions_and_sensors() {
        assert_eq!(bcd_version(0x0177), "1.77");
        assert_eq!(sensor_name(0x25), Some("PixArt PAW3325"));
        assert_eq!(sensor_name(0x00), None);
    }
}
