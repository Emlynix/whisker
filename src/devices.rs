//! Catalogue of supported mice, brand -> model, and matching of connected
//! ones to it. Brands and models come from `data/mice.toml` (embedded);
//! each brand with a vendor-wide driver ends in "Other <brand> mouse".
//!
//! ```text
//!   White Shark  -> Lionel, Other White Shark mouse (auto-detect)
//!   Corsair      -> Katar Pro, …, Other Corsair mouse (auto-detect)
//!   Eternico     -> AET-MS480B (partial), AET-MVS490B (stats only)
//!   Other brand  -> Other mouse (stats only, in code)
//! ```
//!
//! # Data file format
//!
//! `#` starts a comment. `[[brand]]` and `[[mouse]]` start blocks of
//! `key = value` lines. Values: `"text"` (no escapes), numbers (`26000`,
//! `0x5B2F`), bare words, comma lists. Unknown keys or words are errors.
//! File order is picker order; a mouse goes under its brand's block.
//!
//! - brand: `id`, `name` or `label` (`oem`), `in_names` (`true`: the
//!   brand is part of the model names), `family`, `auto` (vendor id of
//!   "Other <brand> mouse (auto-detect)"; none = no such entry),
//!   `verified` (model words checked on hardware through that entry,
//!   e.g. `"M705"`: such a mouse gets full support, a named model whose
//!   name holds the word too), `receivers` (product ids of `auto` that
//!   many models share, e.g. Unifying `0xC52B`: they pick the auto entry,
//!   never a model, and named models may open through them).
//! - mouse: `brand`, `model`, `vid`, `support` (`full`, `experimental`,
//!   `partial`, `stats-only`) are required; `pids` (each with an optional
//!   `receiver` / `cable`; none = only by vendor id), `family` (default:
//!   the brand's), `dpi`, `rates`, `stage_dpi`, `buttons` are optional.
//! - family: `telink-m022`, `hidpp`, `razer`, `eternico`, `sinowealth`,
//!   `steelseries`, `oemhid`, `roccat`, `corsair`, `asus`, `none`.
//!
//! A ready-to-paste block (id = "<brand>/<slug of model>"):
//!
//! ```text
//! [[mouse]]
//! brand = corsair
//! model = "M65 RGB Ultra"
//! vid = 0x1B1C
//! pids = 0x1B9E cable, 0x1BB4 receiver
//! support = experimental
//! dpi = 100..26000 step 50
//! rates = 125, 250, 500, 1000
//! stage_dpi = 400, 800, 1600, 3200
//! buttons = 8
//! ```

mod data;

use crate::drivers::{DpiRange, Pick};
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
    /// DPI range from the data file (the driver still reports its own).
    pub dpi: Option<DpiRange>,
    /// Report rates in Hz from the data file; empty = unknown.
    pub rates: Vec<u32>,
    /// Physical buttons, for a future button remap page.
    pub buttons: Option<u8>,
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
    /// Models verified through the auto-detect entry ("M705").
    pub verified: Vec<&'static str>,
    /// Receiver ids of the auto vendor shared by many models.
    pub receivers: Vec<u16>,
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

/// The catalogue file, embedded at build time; order = picker order.
const MICE: &str = include_str!("../data/mice.toml");
/// "Other <brand> mouse (auto-detect)": the vendor-wide driver entry.
const AUTO_SUPPORT: Support = Support::Experimental;

/// Brand -> model catalogue, parsed once from `data/mice.toml`.
pub fn brands() -> &'static [Brand] {
    static CATALOGUE: OnceLock<Vec<Brand>> = OnceLock::new();
    CATALOGUE.get_or_init(build)
}

/// The data file's brands, then "Other brand". A broken file (the tests
/// parse it) leaves only "Other mouse" instead of failing at startup.
fn build() -> Vec<Brand> {
    let mut brands = data::parse(MICE).map_or_else(|_| Vec::new(), from_data);
    brands.push(other_brand());
    brands
}

/// Records -> brands; each brand with `auto` ends in its Auto entry.
fn from_data(d: data::Data<'static>) -> Vec<Brand> {
    let data::Data { brands: recs, mice } = d;
    let mut brands: Vec<Brand> = recs
        .iter()
        .map(|r| Brand { id: r.id, name: r.name, label: r.label, in_names: r.in_names, verified: r.verified.clone(), receivers: r.receivers.clone(), models: Vec::new() })
        .collect();

    for m in mice {
        let Some(i) = recs.iter().position(|r| r.id == m.brand) else {
            continue;
        };
        let family = m.family.unwrap_or(recs[i].family);
        // A model checked on hardware through the auto entry is full here too.
        let support = if names_any(&recs[i].verified, m.model) { Support::Full } else { m.support };
        brands[i].models.push(Model {
            id: format!("{}/{}", m.brand, slug(m.model)),
            brand: recs[i].id,
            name: m.model,
            entry: Entry::Named,
            vid: m.vid,
            pids: m.pids,
            family,
            support,
            caps: caps_of(family),
            stage_dpi: Vec::leak(m.stage_dpi),
            dpi: m.dpi,
            rates: m.rates,
            buttons: m.buttons,
        });
    }

    for (b, r) in brands.iter_mut().zip(&recs) {
        let Some(vid) = r.auto else {
            continue;
        };
        b.models.push(Model {
            id: format!("{}/auto", r.id),
            brand: r.id,
            name: "",
            entry: Entry::Auto,
            vid,
            pids: Vec::new(),
            family: r.family,
            support: AUTO_SUPPORT,
            caps: caps_of(r.family),
            stage_dpi: &[],
            dpi: None,
            rates: Vec::new(),
            buttons: None,
        });
    }
    brands
}

/// Pages a family's driver can fill.
fn caps_of(f: Family) -> Caps {
    match f {
        Family::TelinkM022 => CAPS_ALL,
        Family::Eternico => CAPS_STATUS,
        Family::NoDriver => CAPS_NONE,
        _ => CAPS_BASIC,
    }
}

/// "Other brand › Other mouse": any mouse, statistics only.
fn other_brand() -> Brand {
    let other = Model {
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
        dpi: None,
        rates: Vec::new(),
        buttons: None,
    };
    Brand { id: "other", name: "", label: Some(T::OtherBrand), in_names: false, verified: Vec::new(), receivers: Vec::new(), models: vec![other] }
}

/// Distinct families of the catalogue entries with this vendor id, in
/// catalogue order; e.g. 0x05AC -> [NoDriver] (the add-a-mouse wizard).
pub fn families_for(vid: u16) -> Vec<Family> {
    let mut out = Vec::new();
    for m in models().filter(|m| m.vid == vid) {
        if !out.contains(&m.family) {
            out.push(m.family);
        }
    }
    out
}

/// The named entry a driver table row is listed as: same vendor and
/// product id (`None`: listed without one) and a name the driver's ends
/// with ("ROG Chakram X" -> "Chakram X"). Driver tests pin their tables.
#[cfg(test)]
pub fn listed_as(vid: u16, pid: Option<u16>, name: &str) -> Option<&'static Model> {
    let pid_ok = |m: &Model| pid.map_or(m.pids.is_empty(), |p| m.pids.iter().any(|(q, _)| *q == p));
    models().find(|m| m.entry == Entry::Named && m.vid == vid && pid_ok(m) && name.ends_with(m.name))
}

/// Parse error of a data-file snippet, e.g. the report wizard's block.
#[cfg(test)]
pub fn check_data(src: &str) -> Result<usize, String> {
    data::parse(src).map(|d| d.mice.len()).map_err(|e| format!("line {}: {}", e.line, e.msg))
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

/// Support of the opened mouse: an auto-detect entry is experimental
/// unless the name it reports holds a model the brand lists as verified
/// ("Logitech Marathon Mouse/Performance Plus M705" -> M705).
pub fn support_of(m: &Model, reported: &str) -> Support {
    if m.entry != Entry::Auto {
        return m.support;
    }
    if names_any(&brand_of(m).verified, reported) { Support::Full } else { m.support }
}

/// Whether `text` holds one of `words` as a whole word, any case
/// ("Marathon M705", ["M705"] -> true; "M7050" -> false).
fn names_any(words: &[&str], text: &str) -> bool {
    let parts: Vec<&str> = text.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    words.iter().any(|v| parts.iter().any(|w| w.eq_ignore_ascii_case(v)))
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

    /// Receiver ids many of the brand's mice share; empty unless this
    /// entry has the brand's auto vendor id.
    fn shared(&self) -> &'static [u16] {
        let brand = brand_of(self);
        let auto_vid = brand.models.iter().any(|m| m.entry == Entry::Auto && m.vid == self.vid);
        if auto_vid { &brand.receivers } else { &[] }
    }

    /// `d` is a shared receiver this model may be paired to, e.g.
    /// MX Master 3 and a Bolt receiver (046D:C548).
    pub fn on_receiver(&self, d: &DeviceInfo) -> bool {
        d.vid == self.vid && self.shared().contains(&d.pid)
    }

    pub fn link_of(&self, pid: u16) -> Option<Link> {
        if self.shared().contains(&pid) {
            return Some(Link::Receiver);
        }
        self.pids.iter().find(|(p, _)| *p == pid).and_then(|(_, l)| *l)
    }

    /// The USB devices its driver may open; `usb` = the detected mouse
    /// the user picked, when it is of this vendor. A named model may
    /// also sit behind a shared receiver.
    pub fn pick(&self, usb: Option<(u16, u16)>) -> Pick {
        let pids = match usb {
            Some((vid, pid)) if vid == self.vid => vec![pid],
            // Any product of the vendor.
            _ if self.entry == Entry::Auto => Vec::new(),
            _ => self.pids.iter().map(|(p, _)| *p).chain(self.shared().iter().copied()).collect(),
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

    fn known_id(vid: u16, pid: u16) -> String {
        match match_device(&dev(vid, pid)) {
            Match::Known(m) => m.id.clone(),
            other => format!("{other:?}"),
        }
    }

    const LOGITECH: u16 = 0x046D;
    const RAZER: u16 = 0x1532;
    const BOLT: u16 = 0xC548;

    #[test]
    fn razer_models_by_product_id() {
        assert_eq!(known_id(RAZER, 0x0084), "razer/deathadder-v2");
        // Cable and dongle of one model.
        assert_eq!(known_id(RAZER, 0x007A), "razer/viper-ultimate");
        assert_eq!(known_id(RAZER, 0x007B), "razer/viper-ultimate");
        let viper = by_id("razer/viper-ultimate").unwrap();
        assert_eq!((viper.link_of(0x007A), viper.link_of(0x007B)), (Some(Link::Cable), Some(Link::Receiver)));
        assert_eq!(viper.full_name(), "Razer Viper Ultimate");
        // Unlisted product: the brand's auto entry.
        assert_eq!(known_id(RAZER, 0xFFF0), "razer/auto");
    }

    #[test]
    fn logitech_cable_ids_name_the_model() {
        assert_eq!(known_id(LOGITECH, 0xC08B), "logitech/g502-se-hero");
        assert_eq!(known_id(LOGITECH, 0xC08A), "logitech/mx-vertical");
    }

    /// Many mice pair with one receiver: the receiver never names a model.
    #[test]
    fn shared_receivers_pick_the_brand_entry() {
        let brand = brands().iter().find(|b| b.id == "logitech").unwrap();
        assert!(brand.receivers.contains(&BOLT));
        for &pid in &brand.receivers {
            assert_eq!(known_id(LOGITECH, pid), "logitech/auto", "{pid:04X}");
        }

        // A named model opens through its cable id or any shared receiver.
        let master = by_id("logitech/mx-master-3").unwrap();
        let pick = master.pick(None);
        assert!(pick.allows(LOGITECH, BOLT) && pick.allows(LOGITECH, 0xB023));
        assert!(!pick.allows(LOGITECH, 0xC08B));
        assert_eq!(master.link_of(BOLT), Some(Link::Receiver));
        assert!(master.on_receiver(&dev(LOGITECH, BOLT)));
        assert!(!master.on_receiver(&dev(LOGITECH, 0xB023)));
        assert!(!master.on_receiver(&dev(RAZER, BOLT)));

        // The auto entry still takes any product of the vendor.
        assert!(by_id("logitech/auto").unwrap().pick(None).allows(LOGITECH, 0xC08B));
        assert_eq!(by_id("logitech/auto").unwrap().link_of(BOLT), Some(Link::Receiver));
    }

    #[test]
    fn verified_words_upgrade_named_models() {
        assert_eq!(by_id("logitech/marathon-m705").unwrap().support, Support::Full);
        assert_eq!(by_id("logitech/mx-master-3").unwrap().support, Support::Experimental);
        assert_eq!(by_id("razer/deathadder-v2").unwrap().support, Support::Experimental);
    }

    #[test]
    fn razer_and_logitech_list_their_models() {
        let named = |brand: &str| models().filter(|m| m.brand == brand && m.entry == Entry::Named).count();
        assert!(named("razer") >= 50, "{}", named("razer"));
        assert!(named("logitech") >= 30, "{}", named("logitech"));
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
    fn verified_models_of_auto_entries() {
        let logitech = by_id("logitech/auto").unwrap();
        assert_eq!(support_of(logitech, "Logitech Marathon Mouse/Performance Plus M705"), Support::Full);
        assert_eq!(support_of(logitech, "G502 HERO"), Support::Experimental);
        // Whole words only: "M7050" is another mouse.
        assert_eq!(support_of(logitech, "Wireless Mouse M7050"), Support::Experimental);
        assert_eq!(support_of(logitech, ""), Support::Experimental);
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
    fn embedded_data_parses() {
        if let Err(e) = data::parse(MICE) {
            panic!("data/mice.toml {e}");
        }
    }

    /// Vendor ids each driver opens; `None` = the family is not tied to
    /// one vendor table here.
    fn driver_vids(f: Family) -> Option<Vec<u16>> {
        use crate::drivers::{asus, corsair, eternico, hidpp, oemhid, razer, roccat, sinowealth, steelseries};
        let vids = match f {
            Family::TelinkM022 => vec![crate::protocol::VENDOR_ID],
            Family::Hidpp => vec![hidpp::VID],
            Family::Razer => vec![razer::VID],
            Family::Eternico => vec![eternico::VID],
            Family::Sinowealth => sinowealth::VIDS.to_vec(),
            Family::SteelSeries => vec![steelseries::VID],
            Family::OemHid => oemhid::VIDS.to_vec(),
            Family::Roccat => vec![roccat::VID],
            Family::Corsair => vec![corsair::VID],
            Family::Asus => vec![asus::VID],
            Family::NoDriver => return None,
        };
        Some(vids)
    }

    #[test]
    fn every_model_has_its_driver() {
        for m in models() {
            match driver_vids(m.family) {
                Some(vids) => assert!(vids.contains(&m.vid), "{} {:04X}", m.id, m.vid),
                None => assert_eq!(m.support, Support::StatsOnly, "{}", m.id),
            }
        }

        // Vendor-wide drivers without a model table: their auto entry.
        for f in [Family::Hidpp, Family::Razer] {
            let vid = driver_vids(f).unwrap()[0];
            assert!(models().any(|m| m.entry == Entry::Auto && m.vid == vid && m.family == f), "{f:?}");
        }
    }

    /// Families that tell models on one product id apart by firmware.
    const SHARED_PID_FAMILIES: [Family; 1] = [Family::Sinowealth];

    #[test]
    fn product_ids_are_unique() {
        let mut seen: Vec<(u16, u16, &Model)> = Vec::new();
        for m in models().filter(|m| m.entry == Entry::Named) {
            for &(pid, _) in &m.pids {
                let clash = seen.iter().find(|(v, p, _)| (*v, *p) == (m.vid, pid));
                if let Some((_, _, o)) = clash {
                    let shared = SHARED_PID_FAMILIES.contains(&m.family) && o.family == m.family;
                    assert!(shared, "{:04X}:{pid:04X} in {} and {}", m.vid, o.id, m.id);
                }
                seen.push((m.vid, pid, m));
            }
        }
    }

    #[test]
    fn families_by_vendor() {
        assert_eq!(families_for(0x05AC), [Family::NoDriver]);
        assert_eq!(families_for(0x1D57), [Family::Eternico]);
        assert_eq!(families_for(0x258A), [Family::Sinowealth]);
        assert!(families_for(0x0458).is_empty());
    }

    #[test]
    fn data_fills_the_new_fields() {
        let chakram = by_id("asus/chakram-x").unwrap();
        assert_eq!(chakram.dpi, Some(DpiRange::Steps { min: 100, max: 36000, step: 100 }));
        assert_eq!((chakram.rates.len(), chakram.buttons), (4, Some(14)));
        assert_eq!(by_id("eternico/aet-ms480b").unwrap().stage_dpi, [1200, 2400, 3200, 6400]);
    }

    /// Pinned text form of the whole catalogue.
    const SNAPSHOT: &str = include_str!("devices/snapshot.txt");
    const SNAPSHOT_PATH: &str = "src/devices/snapshot.txt";

    /// One line per brand and per model, every catalogue field.
    fn dump() -> String {
        use std::fmt::Write;
        let mut out = String::new();
        for b in brands() {
            let receivers: Vec<String> = b.receivers.iter().map(|p| format!("{p:04X}")).collect();
            let _ = writeln!(out, "brand {} name={:?} label={:?} in_names={} receivers=[{}]", b.id, b.name, b.label, b.in_names, receivers.join(","));
            for m in &b.models {
                let link = |l: Option<Link>| l.map_or(String::new(), |l| format!(":{l:?}"));
                let pids: Vec<String> = m.pids.iter().map(|(p, l)| format!("{p:04X}{}", link(*l))).collect();
                let _ = writeln!(
                    out,
                    "  {} brand={} name={:?} entry={:?} vid={:04X} pids=[{}] family={:?} support={:?} caps={:?} stage_dpi={:?}",
                    m.id,
                    m.brand,
                    m.name,
                    m.entry,
                    m.vid,
                    pids.join(","),
                    m.family,
                    m.support,
                    m.caps,
                    m.stage_dpi
                );
            }
        }
        out
    }

    #[test]
    fn catalogue_matches_snapshot() {
        assert_eq!(dump(), SNAPSHOT.replace("\r\n", "\n"));
    }

    /// Rewrites the snapshot: `cargo test write_snapshot -- --ignored`.
    #[test]
    #[ignore]
    fn write_snapshot() {
        std::fs::write(SNAPSHOT_PATH, dump()).unwrap();
    }

    #[test]
    fn versions_and_sensors() {
        assert_eq!(bcd_version(0x0177), "1.77");
        assert_eq!(sensor_name(0x25), Some("PixArt PAW3325"));
        assert_eq!(sensor_name(0x00), None);
    }

    /// Linux udev rule shipped with the app: hidraw access per vendor.
    const UDEV_RULES: &str = include_str!("../bundle/linux/70-whisker.rules");

    /// `0003:046D:*` style match of one vendor in the rules file.
    fn has_rule(vid: u16) -> bool {
        UDEV_RULES.lines().filter(|l| !l.trim_start().starts_with('#')).any(|l| l.contains(&format!(":{vid:04X}:")))
    }

    #[test]
    fn udev_rule_covers_every_driver_vendor() {
        // Every vendor a driver opens over hidraw needs a rule.
        for m in models().filter(|m| m.family != Family::NoDriver) {
            assert!(has_rule(m.vid), "{} {:04X} missing in 70-whisker.rules", m.id, m.vid);
        }
        // Vendors only counted for statistics stay closed (Apple keyboards).
        let driven = |vid| models().any(|m| m.vid == vid && m.family != Family::NoDriver);
        for m in models().filter(|m| m.family == Family::NoDriver && !driven(m.vid)) {
            assert!(!has_rule(m.vid), "{} {:04X} must not be in 70-whisker.rules", m.id, m.vid);
        }
    }
}
