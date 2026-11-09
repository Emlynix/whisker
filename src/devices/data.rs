//! Parser of the catalogue file `data/mice.toml` (format: see the
//! `devices` module doc). Hand-written, no crates; it never panics and
//! every error names its 1-based line.
//!
//! ```text
//!   text ──lines──> blocks [[brand]] / [[mouse]] ──keys──> records
//! ```

use super::{Family, Link, Support, slug};
use crate::drivers::DpiRange;
use crate::i18n::T;
use std::fmt;

/// A parse failure at a 1-based line.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ParseError {
    pub(super) line: usize,
    pub(super) msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.msg)
    }
}

/// One `[[brand]]` block.
pub(super) struct BrandRec<'a> {
    pub(super) id: &'a str,
    /// Empty when `label` names the group.
    pub(super) name: &'a str,
    pub(super) label: Option<T>,
    pub(super) in_names: bool,
    /// Family of its models unless a model says otherwise.
    pub(super) family: Family,
    /// Vendor id of "Other <brand> mouse (auto-detect)", if any.
    pub(super) auto: Option<u16>,
    /// Model words verified on hardware through the auto entry.
    pub(super) verified: Vec<&'a str>,
    /// Product ids of `auto` shared by many models (Unifying, Bolt…).
    pub(super) receivers: Vec<u16>,
}

/// One `[[mouse]]` block.
pub(super) struct MouseRec<'a> {
    pub(super) brand: &'a str,
    pub(super) model: &'a str,
    pub(super) vid: u16,
    pub(super) pids: Vec<(u16, Option<Link>)>,
    /// `None`: the brand's family.
    pub(super) family: Option<Family>,
    pub(super) support: Support,
    pub(super) dpi: Option<DpiRange>,
    pub(super) rates: Vec<u32>,
    pub(super) stage_dpi: Vec<u32>,
    pub(super) buttons: Option<u8>,
}

/// The whole file, records in file order.
pub(super) struct Data<'a> {
    pub(super) brands: Vec<BrandRec<'a>>,
    pub(super) mice: Vec<MouseRec<'a>>,
}

const BRAND_HEADER: &str = "[[brand]]";
const MOUSE_HEADER: &str = "[[mouse]]";
const HEX_PREFIX: &str = "0x";
/// `dpi = 100..26000 step 50`.
const RANGE_SEP: &str = "..";
const STEP_WORD: &str = "step";

/// Family words, e.g. `family = telink-m022`.
const FAMILIES: [(&str, Family); 11] = [
    ("telink-m022", Family::TelinkM022),
    ("hidpp", Family::Hidpp),
    ("razer", Family::Razer),
    ("eternico", Family::Eternico),
    ("sinowealth", Family::Sinowealth),
    ("steelseries", Family::SteelSeries),
    ("oemhid", Family::OemHid),
    ("roccat", Family::Roccat),
    ("corsair", Family::Corsair),
    ("asus", Family::Asus),
    ("none", Family::NoDriver),
];

const SUPPORTS: [(&str, Support); 4] =
    [("full", Support::Full), ("experimental", Support::Experimental), ("partial", Support::Partial), ("stats-only", Support::StatsOnly)];

const LINKS: [(&str, Link); 2] = [("receiver", Link::Receiver), ("cable", Link::Cable)];

/// Translated group names, e.g. `label = oem`.
const LABELS: [(&str, T); 1] = [("oem", T::BrandOem)];

const BOOLS: [(&str, bool); 2] = [("true", true), ("false", false)];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Brand,
    Mouse,
}

/// `key = value` at a line.
struct Field<'a> {
    key: &'a str,
    value: &'a str,
    line: usize,
}

struct Block<'a> {
    kind: Kind,
    line: usize,
    fields: Vec<Field<'a>>,
}

/// Parses the catalogue file.
pub(super) fn parse(src: &str) -> Result<Data<'_>, ParseError> {
    let mut data = Data { brands: Vec::new(), mice: Vec::new() };
    for block in blocks(src)? {
        match block.kind {
            Kind::Brand => {
                let b = brand(&block)?;
                if data.brands.iter().any(|o| o.id == b.id) {
                    return Err(fail(block.line, format!("duplicate brand {}", b.id)));
                }
                data.brands.push(b);
            }
            Kind::Mouse => {
                let m = mouse(&block)?;
                check_mouse(&data, &m, block.line)?;
                data.mice.push(m);
            }
        }
    }
    Ok(data)
}

/// The brand must be declared above; one model name once per brand; no
/// shared receiver as a model's own id (it would name the wrong mouse).
fn check_mouse(data: &Data, m: &MouseRec, line: usize) -> Result<(), ParseError> {
    let Some(brand) = data.brands.iter().find(|b| b.id == m.brand) else {
        return Err(fail(line, format!("brand {} is not declared above", m.brand)));
    };
    let shared = m.pids.iter().find(|(p, _)| brand.auto == Some(m.vid) && brand.receivers.contains(p));
    if let Some((p, _)) = shared {
        return Err(fail(line, format!("{HEX_PREFIX}{p:04X} is a shared receiver of {}", m.brand)));
    }
    let id = slug(m.model);
    if id.is_empty() {
        return Err(fail(line, "model name has no letters or digits".into()));
    }
    if data.mice.iter().any(|o| o.brand == m.brand && slug(o.model) == id) {
        return Err(fail(line, format!("duplicate model {}/{id}", m.brand)));
    }
    Ok(())
}

fn fail(line: usize, msg: String) -> ParseError {
    ParseError { line, msg }
}

/// Splits the text into blocks of fields; comments and blank lines go.
fn blocks(src: &str) -> Result<Vec<Block<'_>>, ParseError> {
    let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
    let mut out: Vec<Block> = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let line = i + 1;
        let text = strip_comment(raw).trim();
        if text.is_empty() {
            continue;
        }

        if text.starts_with('[') {
            let kind = match text {
                BRAND_HEADER => Kind::Brand,
                MOUSE_HEADER => Kind::Mouse,
                _ => return Err(fail(line, format!("unknown block {text}"))),
            };
            out.push(Block { kind, line, fields: Vec::new() });
            continue;
        }

        let Some((key, value)) = text.split_once('=') else {
            return Err(fail(line, "expected key = value".into()));
        };
        let Some(block) = out.last_mut() else {
            return Err(fail(line, "key outside a [[brand]] or [[mouse]] block".into()));
        };
        let (key, value) = (key.trim(), value.trim());
        if value.is_empty() {
            return Err(fail(line, format!("{key} has no value")));
        }
        if block.fields.iter().any(|f| f.key == key) {
            return Err(fail(line, format!("duplicate key {key}")));
        }
        block.fields.push(Field { key, value, line });
    }
    Ok(out)
}

/// Text before a `#` that is not inside quotes.
fn strip_comment(s: &str) -> &str {
    let mut quoted = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &s[..i],
            _ => {}
        }
    }
    s
}

fn brand<'a>(block: &Block<'a>) -> Result<BrandRec<'a>, ParseError> {
    let (mut id, mut name, mut label, mut in_names, mut family, mut auto) = (None, None, None, false, None, None);
    let (mut verified, mut receivers) = (Vec::new(), Vec::new());
    for f in &block.fields {
        let at = |msg: String| fail(f.line, format!("{}: {msg}", f.key));
        let v = f.value;
        match f.key {
            "id" => id = Some(word(v).map_err(at)?),
            "name" => name = Some(string(v).map_err(at)?),
            "label" => label = Some(lookup(&LABELS, v).map_err(at)?),
            "in_names" => in_names = lookup(&BOOLS, v).map_err(at)?,
            "family" => family = Some(lookup(&FAMILIES, v).map_err(at)?),
            "auto" => auto = Some(int(v).map_err(at)?),
            "verified" => verified = list(v, string).map_err(at)?,
            "receivers" => receivers = list(v, int).map_err(at)?,
            key => return Err(fail(f.line, format!("unknown brand key {key}"))),
        }
    }

    let missing = |key: &str| fail(block.line, format!("[[brand]] without {key}"));
    let id = id.ok_or_else(|| missing("id"))?;
    let family = family.ok_or_else(|| missing("family"))?;
    if name.is_none() && label.is_none() {
        return Err(missing("name or label"));
    }
    // Receivers are products of the auto entry's vendor.
    if auto.is_none() && !receivers.is_empty() {
        return Err(missing("auto (receivers need it)"));
    }
    Ok(BrandRec { id, name: name.unwrap_or(""), label, in_names, family, auto, verified, receivers })
}

fn mouse<'a>(block: &Block<'a>) -> Result<MouseRec<'a>, ParseError> {
    let (mut brand, mut model, mut vid, mut support) = (None, None, None, None);
    let mut m = MouseRec {
        brand: "",
        model: "",
        vid: 0,
        pids: Vec::new(),
        family: None,
        support: Support::StatsOnly,
        dpi: None,
        rates: Vec::new(),
        stage_dpi: Vec::new(),
        buttons: None,
    };
    for f in &block.fields {
        let at = |msg: String| fail(f.line, format!("{}: {msg}", f.key));
        let v = f.value;
        match f.key {
            "brand" => brand = Some(word(v).map_err(at)?),
            "model" => model = Some(string(v).map_err(at)?),
            "vid" => vid = Some(int(v).map_err(at)?),
            "pids" => m.pids = list(v, pid).map_err(at)?,
            "family" => m.family = Some(lookup(&FAMILIES, v).map_err(at)?),
            "support" => support = Some(lookup(&SUPPORTS, v).map_err(at)?),
            "dpi" => m.dpi = Some(dpi(v).map_err(at)?),
            "rates" => m.rates = list(v, int).map_err(at)?,
            "stage_dpi" => m.stage_dpi = list(v, int).map_err(at)?,
            "buttons" => m.buttons = Some(int(v).map_err(at)?),
            key => return Err(fail(f.line, format!("unknown mouse key {key}"))),
        }
    }

    let missing = |key: &str| fail(block.line, format!("[[mouse]] without {key}"));
    m.brand = brand.ok_or_else(|| missing("brand"))?;
    m.model = model.ok_or_else(|| missing("model"))?;
    m.vid = vid.ok_or_else(|| missing("vid"))?;
    m.support = support.ok_or_else(|| missing("support"))?;
    Ok(m)
}

/// `"White Shark"` -> `White Shark`; no escapes, no inner quotes.
fn string(v: &str) -> Result<&str, String> {
    let inner = v.strip_prefix('"').and_then(|s| s.strip_suffix('"'));
    match inner {
        Some(s) if !s.contains('"') => Ok(s),
        _ => Err(format!("expected \"text\", got {v}")),
    }
}

/// Bare id word: letters, digits, `-`, `_`.
fn word(v: &str) -> Result<&str, String> {
    if v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Ok(v);
    }
    Err(format!("expected a bare word, got {v}"))
}

/// Value of a bare word in a fixed table, e.g. `stats-only`.
fn lookup<V: Copy>(table: &[(&str, V)], v: &str) -> Result<V, String> {
    if let Some((_, val)) = table.iter().find(|(w, _)| *w == v) {
        return Ok(*val);
    }
    let known: Vec<&str> = table.iter().map(|(w, _)| *w).collect();
    Err(format!("unknown value {v} (known: {})", known.join(", ")))
}

/// Decimal or `0x` hex number that fits `N`, e.g. `0x5B2F`, `26000`.
fn int<N: TryFrom<u64>>(v: &str) -> Result<N, String> {
    let n = match v.strip_prefix(HEX_PREFIX) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => v.parse::<u64>(),
    };
    let n = n.map_err(|_| format!("bad number {v}"))?;
    N::try_from(n).map_err(|_| format!("number {v} out of range"))
}

/// Comma list, each item parsed by `item`; no empty items.
fn list<'a, V>(v: &'a str, item: fn(&'a str) -> Result<V, String>) -> Result<Vec<V>, String> {
    let mut out = Vec::new();
    for part in v.split(',').map(str::trim) {
        if part.is_empty() {
            return Err("empty list item".into());
        }
        out.push(item(part)?);
    }
    Ok(out)
}

/// `0x5B2F` or `0x5B2F receiver`.
fn pid(v: &str) -> Result<(u16, Option<Link>), String> {
    let mut words = v.split_whitespace();
    let id = int(words.next().unwrap_or(""))?;
    let link = words.next().map(|w| lookup(&LINKS, w)).transpose()?;
    if words.next().is_some() {
        return Err(format!("expected product id and link, got {v}"));
    }
    Ok((id, link))
}

/// `100..26000 step 50`, or a list `400, 800, 1600`.
/// More slider positions than any mouse has: a typo, not a range.
const MAX_DPI_VALUES: u32 = 10_000;

fn dpi(v: &str) -> Result<DpiRange, String> {
    let Some((min, rest)) = v.split_once(RANGE_SEP) else {
        return Ok(DpiRange::List(list(v, int)?));
    };
    let words: Vec<&str> = rest.split_whitespace().collect();
    let [max, STEP_WORD, step] = words[..] else {
        return Err(format!("expected min..max step n, got {v}"));
    };
    let (min, max, step) = (int(min.trim())?, int(max)?, int(step)?);
    if min > max || step == 0 {
        return Err(format!("empty range {v}"));
    }
    if (max - min) / step > MAX_DPI_VALUES {
        return Err(format!("too many DPI values in {v}"));
    }
    Ok(DpiRange::Steps { min, max, step })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
# comment
[[brand]]
id = acme        # trailing comment
name = "Acme #1"
family = asus
auto = 0x0B05
receivers = 0xC52B, 50504

[[mouse]]
brand = acme
model = "Rocket"
vid = 0x0B05
pids = 0x1A18 receiver, 6680 cable, 0x1A1B
support = experimental
dpi = 100..36000 step 100
rates = 125, 1000
stage_dpi = 400, 800
buttons = 14

[[mouse]]
brand = acme
model = "Anvil"
vid = 1
family = none
support = stats-only
dpi = 400, 800
"#;

    fn line_of(src: &str) -> usize {
        parse(src).err().map_or(0, |e| e.line)
    }

    #[test]
    fn parses_good_input() {
        let d = parse(GOOD).unwrap();
        let b = &d.brands[0];
        assert_eq!((b.id, b.name, b.family, b.auto, b.in_names), ("acme", "Acme #1", Family::Asus, Some(0x0B05), false));
        assert_eq!(b.receivers, [0xC52B, 0xC548]);

        let m = &d.mice[0];
        assert_eq!((m.brand, m.model, m.vid, m.support), ("acme", "Rocket", 0x0B05, Support::Experimental));
        assert_eq!(m.pids, [(0x1A18, Some(Link::Receiver)), (0x1A18, Some(Link::Cable)), (0x1A1B, None)]);
        assert_eq!(m.dpi, Some(DpiRange::Steps { min: 100, max: 36000, step: 100 }));
        assert_eq!((m.rates.as_slice(), m.stage_dpi.as_slice(), m.buttons), (&[125, 1000][..], &[400, 800][..], Some(14)));

        let n = &d.mice[1];
        assert_eq!((n.family, n.support, n.pids.len()), (Some(Family::NoDriver), Support::StatsOnly, 0));
        assert_eq!(n.dpi, Some(DpiRange::List(vec![400, 800])));
    }

    #[test]
    fn empty_input_is_empty() {
        for src in ["", "\n\n", "# only a comment\n", "\u{FEFF}"] {
            let d = parse(src).unwrap();
            assert!(d.brands.is_empty() && d.mice.is_empty());
        }
    }

    #[test]
    fn bad_input_names_the_line() {
        let brand = "[[brand]]\nid = a\nname = \"A\"\nfamily = asus\n";
        let mouse = |extra: &str| format!("{brand}[[mouse]]\nbrand = a\nmodel = \"M\"\nvid = 1\nsupport = full\n{extra}");

        // Truncated block: required keys missing, reported at its header.
        assert_eq!(line_of("[[brand]]\nid = a\n"), 1);
        assert_eq!(line_of(&format!("{brand}[[mouse]]\nbrand = a\n")), 5);

        // Bad values and keys at their own line (line 10 = first extra).
        assert_eq!(line_of(&mouse("vid2 = 1")), 10);
        assert_eq!(line_of(&mouse("buttons = 300")), 10);
        assert_eq!(line_of(&mouse("pids = 0xZZ")), 10);
        assert_eq!(line_of(&mouse("pids = 0x10, ,0x11")), 10);
        assert_eq!(line_of(&mouse("pids = 0x10 dongle")), 10);
        assert_eq!(line_of(&mouse("dpi = 900..100 step 50")), 10);
        assert_eq!(line_of(&mouse("dpi = 100..900 by 50")), 10);
        assert_eq!(line_of(&mouse("family = quantum")), 10);
        assert_eq!(line_of(&mouse("support = maybe")), 10);
        assert_eq!(line_of(&mouse("rates = 99999999999999999999999")), 10);
        assert_eq!(line_of(&mouse("support = full")), 10);
        assert_eq!(line_of(&mouse("[[mouse]]\nbrand = a\nmodel = \"M\"\nvid = 1\nsupport = full")), 10);

        // Structure.
        assert_eq!(line_of("id = a"), 1);
        assert_eq!(line_of("\n[[brands]]"), 2);
        assert_eq!(line_of(&format!("{brand}name")), 5);
        assert_eq!(line_of(&format!("{brand}[[mouse]]\nbrand = b\nmodel = \"M\"\nvid = 1\nsupport = full")), 5);
        assert_eq!(line_of("[[brand]]\nid = a\nname = \"unterminated\nfamily = asus"), 3);
        assert_eq!(line_of("[[brand]]\nid = a b\n"), 2);
    }

    #[test]
    fn shared_receivers_are_checked() {
        let brand = "[[brand]]\nid = a\nname = \"A\"\nfamily = hidpp\nauto = 0x046D\nreceivers = 0xC52B\n";
        let mouse = |vid: &str, pids: &str| format!("{brand}[[mouse]]\nbrand = a\nmodel = \"M\"\nvid = {vid}\npids = {pids}\nsupport = full\n");
        assert!(parse(&mouse("0x046D", "0xC08B cable")).is_ok());

        // Bad list item at its line; receivers belong to the auto vendor.
        assert_eq!(line_of("[[brand]]\nid = a\nname = \"A\"\nfamily = hidpp\nauto = 1\nreceivers = 0xC52B, x\n"), 6);
        assert_eq!(line_of("[[brand]]\nid = a\nname = \"A\"\nfamily = hidpp\nreceivers = 0xC52B\n"), 1);

        // A model never claims a shared receiver: the paired mouse is unknown.
        assert_eq!(line_of(&mouse("0x046D", "0xC08B, 0xC52B receiver")), 7);
        // Another vendor's product id may be equal.
        assert!(parse(&mouse("0x1532", "0xC52B")).is_ok());
    }

    #[test]
    fn garbage_never_panics() {
        let bytes: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let text = String::from_utf8_lossy(&bytes);
        assert!(parse(&text).is_err());

        // Every prefix of a good file: parses or reports a line in range.
        for end in (0..GOOD.len()).filter(|i| GOOD.is_char_boundary(*i)) {
            if let Err(e) = parse(&GOOD[..end]) {
                assert!(e.line >= 1 && e.line <= GOOD.lines().count(), "{e}");
            }
        }
        for src in ["=", "[[mouse]]\n=", "[[brand]]\nname = \"", "[[brand]]\nname = \"\"\"", "[[mouse]]\ndpi = ..", "[[mouse]]\ndpi = 1.. step"] {
            assert!(parse(src).is_err(), "{src}");
        }
    }
}
