//! "Help add your mouse" report: what the OS and the read-only driver
//! probe tell about one connected mouse, as Markdown for a public GitHub
//! issue. Built in English (`i18n::with_lang`). Private by construction:
//! no serial numbers, no paths, Bluetooth address masked to its last two
//! bytes.
//!
//! ```text
//!   ### Mouse report: <product> (VID:PID)     header, system, catalogue
//!   #### HID collections                     one table row per collection
//!   #### Driver probe                         one line per driver tried
//!   #### Verification                         guided checks (if run)
//!   #### Catalogue entry                      ```toml [[mouse]] block
//! ```

use super::mask_address;
use crate::devices::{self, Match, Support};
use crate::drivers::{Ability, DpiRange, Ident, Reading};
use crate::hid_link::{DeviceInfo, HidDevice, ReportLayout, Transport};
use crate::mouse::{Probe, Probed, Target};

/// GitHub refuses longer new-issue links; the body is pasted instead.
const URL_MAX: usize = 6000;
const DESKTOP_PAGE: u16 = 0x01;
const MOUSE_USAGE: u16 = 0x02;
/// Bytes of a raw descriptor per hex line.
const HEX_LINE: usize = 16;

/// Everything the report is made of.
pub struct Input<'a> {
    pub dev: &'a HidDevice,
    pub probes: &'a [Probe],
    /// Verification result lines, if the checks ran.
    pub checks: &'a [String],
    pub os: &'a str,
}

/// How the new-issue page gets the report.
#[derive(Debug, PartialEq, Eq)]
pub enum Issue {
    /// Title and body in the link.
    Prefilled(String),
    /// Too long for a link: the empty page; the body goes to the clipboard.
    Paste(String),
}

/// A string the device reports, safe in Markdown and TOML: no control
/// characters, table bars, code fences, quotes or @mentions.
fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() || matches!(c, '|' | '`' | '@' | '"' | '<' | '>') { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// "Logitech G305", else the vendor and product ids.
pub fn product(d: &DeviceInfo) -> String {
    let name = format!("{} {}", clean(&d.manufacturer), clean(&d.product));
    let name = name.trim();
    if name.is_empty() { usb_id(d) } else { name.to_string() }
}

pub fn usb_id(d: &DeviceInfo) -> String {
    format!("{:04X}:{:04X}", d.vid, d.pid)
}

pub fn title(d: &DeviceInfo) -> String {
    format!("Mouse report: {} ({})", product(d), usb_id(d))
}

pub fn transport(t: Transport) -> &'static str {
    match t {
        Transport::Usb => "USB",
        Transport::Bluetooth => "Bluetooth",
        Transport::Other => "other",
    }
}

/// "B66B18D71E00" -> "Bluetooth LE · XX:XX:XX:XX:1E:00".
fn bt_line(addr: &str) -> String {
    let pairs: Vec<&str> = (0..addr.len()).step_by(2).filter_map(|k| addr.get(k..k + 2)).collect();
    mask_address(&format!("Bluetooth LE · {}", pairs.join(":")))
}

/// The whole report as Markdown.
pub fn text(i: &Input) -> String {
    let d = &i.dev.info;
    let mut out = vec![format!("### {}", title(d)), String::new()];
    out.push(format!("- {} {} on {}", crate::APP_NAME, env!("CARGO_PKG_VERSION"), i.os));
    out.push(format!("- Manufacturer: {}", or_none(&clean(&d.manufacturer))));
    out.push(format!("- Product: {}", or_none(&clean(&d.product))));
    if d.bt_address.is_some() {
        // Windows may show the name the user gave the mouse.
        out.push("- Note: Bluetooth name as set in the OS; remove it if it is personal.".into());
    }
    out.push(format!("- USB ID: {}, release {}", usb_id(d), devices::bcd_version(d.version)));
    let link = d.bt_address.as_deref().map_or_else(|| link_of(i.dev).to_string(), bt_line);
    out.push(format!("- Connection: {link}"));
    out.push(format!("- Catalogue: {}", catalogue(d)));

    out.push(String::new());
    out.extend(collections(i.dev));
    out.push(String::new());
    out.extend(probes(i.probes));
    if !i.checks.is_empty() {
        out.push(String::new());
        out.push("#### Verification (guided checks on the real mouse)".into());
        out.extend(i.checks.iter().cloned());
    }
    out.push(String::new());
    out.extend(data_block(d, i.probes));
    out.join("\n")
}

fn or_none(s: &str) -> &str {
    if s.is_empty() { "(none)" } else { s }
}

/// Transport of the mouse collection.
fn link_of(dev: &HidDevice) -> &'static str {
    dev.collections.first().map_or("unknown", |(_, l)| transport(l.transport))
}

fn catalogue(d: &DeviceInfo) -> String {
    match devices::match_device(d) {
        Match::Known(m) => {
            let mut facts = vec![support(m.support).to_string()];
            facts.extend(m.dpi.as_ref().map(|r| format!("DPI {}", dpi_text(r))));
            if !m.rates.is_empty() {
                facts.push(format!("rates {}", join(&m.rates, "/")));
            }
            facts.extend(m.buttons.map(|b| format!("{b} buttons")));
            format!("{} ({})", m.full_name(), facts.join(", "))
        }
        Match::Guess(m) => format!("not listed; same vendor as {}", m.full_name()),
        Match::Unknown => "not listed".into(),
    }
}

fn support(s: Support) -> &'static str {
    match s {
        Support::Full => "full",
        Support::Experimental => "experimental",
        Support::Partial => "partial",
        Support::StatsOnly => "statistics only",
    }
}

/// Table of collections, their caps and raw descriptors.
fn collections(dev: &HidDevice) -> Vec<String> {
    let mut out = vec![
        "#### HID collections".to_string(),
        "| Usage page:usage | Interface | Link | Input / output / feature bytes | Report IDs |".to_string(),
        "|---|---|---|---|---|".to_string(),
    ];
    for (c, l) in &dev.collections {
        let usage = usage(c.usage_page, c.usage);
        let iface = l.interface.map_or_else(|| "-".to_string(), |n| n.to_string());
        let ids = if l.report_ids.is_empty() { "none".to_string() } else { hex_list(&l.report_ids) };
        let lens = format!("{} / {} / {}", l.input_len, l.output_len, l.feature_len);
        out.push(format!("| {usage} | {iface} | {} | {lens} | {ids} |", transport(l.transport)));
    }

    for (c, l) in &dev.collections {
        out.extend(caps(&usage(c.usage_page, c.usage), l));
    }
    if dev.collections.iter().all(|(_, l)| l.descriptor.is_none()) {
        out.push(String::new());
        out.push("Raw report descriptors: not available on this OS (caps above).".into());
    }
    out
}

/// Caps summary and raw descriptor of one collection.
fn caps(usage: &str, l: &ReportLayout) -> Vec<String> {
    let mut out = vec![String::new(), format!("{usage}:")];
    out.extend(l.summary.iter().map(|s| format!("- {s}")));
    let Some(bytes) = &l.descriptor else {
        return out;
    };
    out.push("```text".into());
    out.extend(bytes.chunks(HEX_LINE).map(|row| row.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")));
    out.push("```".into());
    out
}

fn usage(page: u16, usage: u16) -> String {
    let name = if (page, usage) == (DESKTOP_PAGE, MOUSE_USAGE) { " (mouse)" } else { "" };
    format!("0x{page:04X}:0x{usage:04X}{name}")
}

fn hex_list(ids: &[u8]) -> String {
    ids.iter().map(|b| format!("0x{b:02X}")).collect::<Vec<_>>().join(", ")
}

/// Driver display name in the report.
pub fn driver_name(t: &Target) -> &'static str {
    match t {
        Target::Lionel => "Telink M022 (White Shark Lionel family)",
        Target::Hidpp => "Logitech HID++ 2.0",
        Target::Razer => "Razer",
        Target::Eternico => "Eternico (listen-only)",
        Target::Sinowealth => "Sinowealth OEM",
        Target::SteelSeries => "SteelSeries",
        Target::OemHid => "ATK / VXE / VGN / Lamzu OEM",
        Target::Asus => "ASUS ROG",
        Target::Corsair => "Corsair Bragi",
        Target::Roccat => "Roccat Kone",
        Target::Bluetooth(_) | Target::Nothing => "none",
    }
}

fn probes(list: &[Probe]) -> Vec<String> {
    let mut out = vec!["#### Driver probe (identify and read requests only)".to_string()];
    if list.is_empty() {
        out.push("- No Whisker driver knows this vendor id.".into());
    }
    for p in list {
        out.push(format!("- {}: {}", driver_name(&p.target), probed(&p.got)));
    }
    out
}

fn probed(p: &Probed) -> String {
    match p {
        Probed::Refused => "did not accept the device".into(),
        Probed::Status(info, None) => format!("opened {}, no status reply", usb_id(info)),
        Probed::Status(info, Some(s)) => {
            let sensor = devices::sensor_name(s.sensor).map_or_else(|| format!("0x{:02X}", s.sensor), str::to_string);
            format!("opened {}; status: battery {} %, DPI stage {}, sensor {sensor}", usb_id(info), s.battery, s.active_stage + 1)
        }
        Probed::Driver(info, ident, ability, reading) => {
            format!("opened {}; {}; {}; {}", usb_id(info), ident_text(ident), ability_text(ability), reading_text(reading.as_ref()))
        }
    }
}

/// Name, firmware and protocol; the serial number stays out.
fn ident_text(i: &Ident) -> String {
    format!("name \"{}\", firmware \"{}\", protocol \"{}\"", clean(&i.name), clean(&i.firmware), clean(&i.protocol))
}

fn ability_text(a: &Ability) -> String {
    let dpi = a.dpi.as_ref().map_or_else(|| "DPI not settable".to_string(), |r| format!("DPI {}", dpi_text(r)));
    let rates = if a.rates.is_empty() { "rates unknown".to_string() } else { format!("rates {}", join(&a.rates, "/")) };
    let battery = if a.battery { "battery yes" } else { "battery no" };
    format!("{dpi}, {rates}, {battery}")
}

fn dpi_text(r: &DpiRange) -> String {
    match r {
        DpiRange::Steps { min, max, step } => format!("{min}..{max} step {step}"),
        DpiRange::List(v) => join(v, "/"),
    }
}

fn reading_text(r: Option<&Reading>) -> String {
    let Some(r) = r else {
        return "no reading (asleep or not answering)".into();
    };
    let show = |v: Option<u32>, unit: &str| v.map_or_else(|| "?".to_string(), |x| format!("{x}{unit}"));
    let battery = r.battery.map_or_else(|| "?".to_string(), |b| format!("{b} %"));
    format!("reading: battery {battery}, DPI {}, rate {}", show(r.dpi, ""), show(r.rate_hz, " Hz"))
}

fn join(v: &[u32], sep: &str) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(sep)
}

/// Ready-to-paste `[[mouse]]` block for data/mice.toml.
fn data_block(d: &DeviceInfo, list: &[Probe]) -> Vec<String> {
    let opened = list.iter().find_map(|p| match &p.got {
        Probed::Driver(_, _, a, _) => Some((p.target.clone(), Some(a))),
        Probed::Status(..) => Some((p.target.clone(), None)),
        Probed::Refused => None,
    });
    let family = opened.as_ref().map_or("none", |(t, _)| family_word(t));
    let support = if family == "none" { "stats-only" } else { "experimental" };
    let mut out = vec!["#### Catalogue entry (paste into data/mice.toml)".to_string(), "```toml".to_string()];

    // A new brand needs its own block before the mouse.
    let brand = match devices::match_device(d) {
        Match::Known(m) | Match::Guess(m) => m.brand.to_string(),
        Match::Unknown => {
            let id = slug(&d.manufacturer);
            let name = if d.manufacturer.is_empty() { id.clone() } else { clean(&d.manufacturer) };
            out.extend(["[[brand]]".to_string(), format!("id = {id}"), format!("name = \"{name}\""), format!("family = {family}"), String::new()]);
            id
        }
    };
    out.push("[[mouse]]".into());
    out.push(format!("brand = {brand}"));
    out.push(format!("model = \"{}\"", clean(&d.product)));
    out.push(format!("vid = 0x{:04X}", d.vid));
    out.push(format!("pids = 0x{:04X}", d.pid));
    out.push(format!("family = {family}"));
    out.push(format!("support = {support}"));
    if let Some(a) = opened.and_then(|(_, a)| a.cloned()) {
        if let Some(DpiRange::Steps { min, max, step }) = a.dpi {
            out.push(format!("dpi = {min}..{max} step {step}"));
        }
        if !a.rates.is_empty() {
            out.push(format!("rates = {}", join(&a.rates, ", ")));
        }
    }
    out.push("```".into());
    out
}

/// Family word of the data file for a driver.
fn family_word(t: &Target) -> &'static str {
    match t {
        Target::Lionel => "telink-m022",
        Target::Hidpp => "hidpp",
        Target::Razer => "razer",
        Target::Eternico => "eternico",
        Target::Sinowealth => "sinowealth",
        Target::SteelSeries => "steelseries",
        Target::OemHid => "oemhid",
        Target::Asus => "asus",
        Target::Corsair => "corsair",
        Target::Roccat => "roccat",
        Target::Bluetooth(_) | Target::Nothing => "none",
    }
}

/// "Dream Machines" -> "dream-machines"; "unknown" when empty.
fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            continue;
        }
        if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.is_empty() { "unknown".into() } else { out.into() }
}

/// New-issue link with title and body, or the empty page when too long.
pub fn issue(base: &str, title: &str, body: &str) -> Issue {
    let url = format!("{base}?title={}&body={}", encode(title), encode(body));
    if url.len() <= URL_MAX {
        return Issue::Prefilled(url);
    }
    Issue::Paste(base.to_string())
}

/// Percent-encoding of everything but unreserved characters (RFC 3986).
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
            continue;
        }
        out.push_str(&format!("%{b:02X}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid_link::Interface;

    fn layout(transport: Transport, ids: Vec<u8>) -> ReportLayout {
        ReportLayout { transport, interface: Some(1), input_len: 8, output_len: 0, feature_len: 33, report_ids: ids, descriptor: None, summary: vec!["input id 0x02: buttons 0x09:0x01-0x05".into()] }
    }

    fn device(bt: Option<&str>) -> HidDevice {
        let info = DeviceInfo {
            vid: 0x1234,
            pid: 0x5678,
            version: 0x0112,
            manufacturer: "Acme".into(),
            product: "Glide \"Pro\"".into(),
            serial: "SN-SECRET-42".into(),
            bt_address: bt.map(str::to_string),
        };
        let mouse = Interface { path: r"\\?\hid#vid_1234&pid_5678&mi_01#7&secret&0&0000#{guid}".into(), info: info.clone(), usage_page: 1, usage: 2, output_len: 0, feature_len: 33 };
        let mut vendor = mouse.clone();
        (vendor.usage_page, vendor.usage) = (0xFF00, 1);
        let t = if bt.is_some() { Transport::Bluetooth } else { Transport::Usb };
        HidDevice { info, collections: vec![(mouse, layout(t, vec![2])), (vendor, layout(t, vec![]))] }
    }

    fn report(dev: &HidDevice, probes: &[Probe]) -> String {
        text(&Input { dev, probes, checks: &[], os: "Windows 11 26300" })
    }

    #[test]
    fn report_keeps_private_data_out() {
        let dev = device(Some("B66B18D71E00"));
        let ident = Ident { name: "Glide".into(), firmware: "1.2".into(), serial: "IDENT-SERIAL-7".into(), protocol: "X".into() };
        let probes = [Probe { target: Target::Razer, got: Probed::Driver(dev.info.clone(), ident, Ability::default(), None) }];
        let t = report(&dev, &probes);
        assert!(!t.contains("SN-SECRET-42") && !t.contains("IDENT-SERIAL-7"), "{t}");
        assert!(!t.contains("secret") && !t.contains(r"\\?\"), "no paths: {t}");
        assert!(!t.contains("B6:6B") && !t.contains("B66B"), "{t}");
        assert!(t.contains("Bluetooth LE · XX:XX:XX:XX:1E:00"), "{t}");
        let user = std::env::var("USERNAME").unwrap_or_default();
        assert!(user.is_empty() || !t.contains(&user));
    }

    #[test]
    fn report_lists_collections_probe_and_data_block() {
        let dev = device(None);
        let a = Ability { dpi: Some(DpiRange::Steps { min: 100, max: 8000, step: 50 }), rates: vec![125, 1000], battery: true, stages: None };
        let probes = [
            Probe { target: Target::Corsair, got: Probed::Refused },
            Probe { target: Target::Razer, got: Probed::Driver(dev.info.clone(), Ident::default(), a, None) },
        ];
        let t = report(&dev, &probes);
        assert!(t.starts_with("### Mouse report: Acme Glide  Pro (1234:5678)"), "{t}");
        assert_eq!(clean("a@b|c`d\ne"), "a b c d e", "device strings are cleaned");
        assert!(t.contains("- Whisker ") && t.contains("on Windows 11 26300"));
        assert!(t.contains("| 0x0001:0x0002 (mouse) | 1 | USB | 8 / 0 / 33 | 0x02 |"), "{t}");
        assert!(t.contains("| 0xFF00:0x0001 | 1 | USB | 8 / 0 / 33 | none |"), "{t}");
        assert!(t.contains("- Corsair Bragi: did not accept the device"));
        assert!(t.contains("DPI 100..8000 step 50, rates 125/1000, battery yes"), "{t}");

        let block = t.split("```toml\n").nth(1).unwrap();
        assert!(t.contains("not available on this OS"));
        let toml = block.split("```").next().unwrap();
        assert_eq!(devices::check_data(toml), Ok(1), "the block pastes into data/mice.toml");
        assert!(block.starts_with("[[brand]]\nid = acme\nname = \"Acme\"\nfamily = razer\n\n[[mouse]]\nbrand = acme\nmodel = \"Glide  Pro\"\nvid = 0x1234\npids = 0x5678\nfamily = razer\nsupport = experimental\ndpi = 100..8000 step 50\nrates = 125, 1000\n```"), "{block}");
    }

    #[test]
    fn long_reports_are_pasted() {
        let base = "https://github.com/Emlynix/whisker/issues/new";
        let Issue::Prefilled(url) = issue(base, "Mouse report: A", "a b&c") else {
            panic!("short report goes in the link");
        };
        assert_eq!(url, format!("{base}?title=Mouse%20report%3A%20A&body=a%20b%26c"));
        assert_eq!(issue(base, "t", &"x".repeat(URL_MAX)), Issue::Paste(base.into()));
    }
}
