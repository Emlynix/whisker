//! Report descriptor parser (HID 1.11, 6.2.2): the top-level collections
//! of a device and the byte length of their reports, as Windows reports
//! them per collection (`HIDP_CAPS`): longest report of a kind plus one
//! byte for the report id (also when unnumbered), 0 when there is none.
//! hidraw gives one descriptor per device; drivers pick collections by
//! usage and these lengths, as on Windows. Pure, no I/O.
//!
//! ```text
//!   05 01  09 02  A1 01  85 02 … 81 02 … C0    page 1, usage 2, Application
//!   ^page  ^usage ^coll. ^id     ^input  ^end   -> Collection { 0x01, 0x02, … }
//! ```

/// Item types in bits 2..3 of the prefix.
const TYPE_MAIN: u8 = 0;
const TYPE_GLOBAL: u8 = 1;
const TYPE_LOCAL: u8 = 2;
/// Main item tags.
const TAG_INPUT: u8 = 0x8;
const TAG_OUTPUT: u8 = 0x9;
const TAG_FEATURE: u8 = 0xB;
const TAG_COLLECTION: u8 = 0xA;
const TAG_END_COLLECTION: u8 = 0xC;
/// Global item tags.
const TAG_USAGE_PAGE: u8 = 0x0;
const TAG_REPORT_SIZE: u8 = 0x7;
const TAG_REPORT_ID: u8 = 0x8;
const TAG_REPORT_COUNT: u8 = 0x9;
const TAG_PUSH: u8 = 0xA;
const TAG_POP: u8 = 0xB;
/// Local item tag.
const TAG_USAGE: u8 = 0x0;
const LONG_ITEM: u8 = 0xFE;
/// Room for the report id byte in each length.
const ID_BYTE: usize = 1;
/// Push/pop depth kept; deeper pushes are ignored (bad descriptor).
const STACK_MAX: usize = 16;

/// One top-level collection with Windows-style report lengths.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Collection {
    pub usage_page: u16,
    pub usage: u16,
    pub input_len: usize,
    pub output_len: usize,
    pub feature_len: usize,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u16,
    size: u32,
    count: u32,
    id: u8,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Input,
    Output,
    Feature,
}

/// Bits per `(kind, report id)` of one collection.
#[derive(Default)]
struct Bits(Vec<(Kind, u8, u32)>);

impl Bits {
    fn add(&mut self, kind: Kind, id: u8, bits: u32) {
        match self.0.iter_mut().find(|(k, i, _)| (*k, *i) == (kind, id)) {
            Some(e) => e.2 = e.2.saturating_add(bits),
            None => self.0.push((kind, id, bits)),
        }
    }

    /// Longest report of `kind` in bytes plus the id byte; 0 when none.
    fn len(&self, kind: Kind) -> usize {
        let Some(max) = self.0.iter().filter(|(k, _, _)| *k == kind).map(|e| e.2).max() else {
            return 0;
        };
        (max as usize).div_ceil(8) + ID_BYTE
    }
}

/// Little-endian unsigned item data.
fn value(data: &[u8]) -> u32 {
    data.iter().rev().fold(0, |v, &b| (v << 8) | b as u32)
}

/// Top-level collections of a raw report descriptor, in order. Stops at a
/// truncated item, so garbage never panics.
pub fn collections(desc: &[u8]) -> Vec<Collection> {
    let mut tops: Vec<(Collection, Bits)> = Vec::new();
    let mut g = Globals::default();
    let mut stack: Vec<Globals> = Vec::new();
    let mut usages: Vec<(Option<u16>, u16)> = Vec::new();
    let mut depth = 0usize;
    let mut at = 0;

    while let Some(&prefix) = desc.get(at) {
        // Long item: prefix, data size, tag, data. Carries nothing we use.
        if prefix == LONG_ITEM {
            let Some(&len) = desc.get(at + 1) else {
                break;
            };
            at += 3 + len as usize;
            continue;
        }
        let len = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let Some(data) = desc.get(at + 1..at + 1 + len) else {
            break;
        };
        at += 1 + len;
        let (kind, tag, v) = ((prefix >> 2) & 0x03, prefix >> 4, value(data));

        match (kind, tag) {
            (TYPE_GLOBAL, TAG_USAGE_PAGE) => g.page = v as u16,
            (TYPE_GLOBAL, TAG_REPORT_SIZE) => g.size = v,
            (TYPE_GLOBAL, TAG_REPORT_COUNT) => g.count = v,
            (TYPE_GLOBAL, TAG_REPORT_ID) => g.id = v as u8,
            (TYPE_GLOBAL, TAG_PUSH) if stack.len() < STACK_MAX => stack.push(g),
            (TYPE_GLOBAL, TAG_POP) => g = stack.pop().unwrap_or(g),
            // A 4-byte usage carries its own page in the high half.
            (TYPE_LOCAL, TAG_USAGE) if len == 4 => usages.push((Some((v >> 16) as u16), v as u16)),
            (TYPE_LOCAL, TAG_USAGE) => usages.push((None, v as u16)),
            (TYPE_MAIN, TAG_COLLECTION) => {
                if depth == 0 {
                    let (page, usage) = usages.first().map_or((g.page, 0), |&(p, u)| (p.unwrap_or(g.page), u));
                    tops.push((Collection { usage_page: page, usage, ..Collection::default() }, Bits::default()));
                }
                depth += 1;
            }
            (TYPE_MAIN, TAG_END_COLLECTION) => depth = depth.saturating_sub(1),
            (TYPE_MAIN, TAG_INPUT | TAG_OUTPUT | TAG_FEATURE) => {
                let report = match tag {
                    TAG_INPUT => Kind::Input,
                    TAG_OUTPUT => Kind::Output,
                    _ => Kind::Feature,
                };
                if let Some((_, bits)) = tops.last_mut() {
                    bits.add(report, g.id, g.size.saturating_mul(g.count));
                }
            }
            _ => {}
        }
        // Local items apply to the next main item only.
        if kind == TYPE_MAIN {
            usages.clear();
        }
    }

    tops.into_iter()
        .map(|(c, bits)| Collection { input_len: bits.len(Kind::Input), output_len: bits.len(Kind::Output), feature_len: bits.len(Kind::Feature), ..c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mouse (id 2: 5 buttons + 3 pad bits, X/Y 16 bit) plus a vendor
    /// collection (id 5, 32-byte feature) with a long item in between;
    /// the same bytes as the `hid_link` test.
    const MOUSE_DESC: [u8; 76] = [
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00,
        0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
        0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x06, 0xC0, 0xC0, 0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1,
        0x01, 0xFE, 0x01, 0x10, 0x85, 0x85, 0x05, 0x95, 0x20, 0x75, 0x08, 0x09, 0x02, 0xB1, 0x02, 0xC0,
    ];

    #[test]
    fn mouse_and_vendor_collections() {
        let c = collections(&MOUSE_DESC);
        assert_eq!(c.len(), 2);
        // 8 button bits + 32 motion bits = 5 bytes, + id byte.
        assert_eq!(c[0], Collection { usage_page: 0x01, usage: 0x02, input_len: 6, output_len: 0, feature_len: 0 });
        // 32-byte feature + id byte: the Lionel config collection's 33.
        assert_eq!(c[1], Collection { usage_page: 0xFF00, usage: 0x01, input_len: 0, output_len: 0, feature_len: 33 });
    }

    #[test]
    fn unnumbered_report_still_counts_the_id_byte() {
        // Usage page 1, usage 2, Application; 8 x 8-bit input.
        let d = [0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x75, 0x08, 0x95, 0x08, 0x81, 0x02, 0xC0];
        assert_eq!(collections(&d)[0].input_len, 9);
    }

    #[test]
    fn extended_usage_carries_its_page() {
        // Usage 0xFF00:0001 as one 4-byte item.
        let d = [0x0B, 0x01, 0x00, 0x00, 0xFF, 0xA1, 0x01, 0xC0];
        let c = &collections(&d)[0];
        assert_eq!((c.usage_page, c.usage), (0xFF00, 0x01));
    }

    #[test]
    fn garbage_never_panics() {
        assert!(collections(&[0xA1]).is_empty());
        assert!(collections(&[0xFE, 0xFF]).is_empty());
        let noise: Vec<u8> = (0..4096u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        for n in 0..noise.len() {
            collections(&noise[n..]);
        }
    }
}
