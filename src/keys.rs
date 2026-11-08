//! Windows virtual keys -> HID keyboard usages (what the mouse firmware
//! sends) and short display names. Pure tables, no I/O.

/// `(vk, hid usage, name)`. Letters and digits are generated below.
const TABLE: &[(u16, u8, &str)] = &[
    (0x0D, 0x28, "Enter"),
    (0x1B, 0x29, "Esc"),
    (0x08, 0x2A, "Backspace"),
    (0x09, 0x2B, "Tab"),
    (0x20, 0x2C, "Space"),
    (0xBD, 0x2D, "-"),
    (0xBB, 0x2E, "="),
    (0xDB, 0x2F, "["),
    (0xDD, 0x30, "]"),
    (0xDC, 0x31, "\\"),
    (0xBA, 0x33, ";"),
    (0xDE, 0x34, "'"),
    (0xC0, 0x35, "`"),
    (0xBC, 0x36, ","),
    (0xBE, 0x37, "."),
    (0xBF, 0x38, "/"),
    (0x14, 0x39, "Caps Lock"),
    (0x2C, 0x46, "Print Screen"),
    (0x91, 0x47, "Scroll Lock"),
    (0x13, 0x48, "Pause"),
    (0x2D, 0x49, "Insert"),
    (0x24, 0x4A, "Home"),
    (0x21, 0x4B, "Page Up"),
    (0x2E, 0x4C, "Delete"),
    (0x23, 0x4D, "End"),
    (0x22, 0x4E, "Page Down"),
    (0x27, 0x4F, "→"),
    (0x25, 0x50, "←"),
    (0x28, 0x51, "↓"),
    (0x26, 0x52, "↑"),
    (0x90, 0x53, "Num Lock"),
    (0x6F, 0x54, "Num /"),
    (0x6A, 0x55, "Num *"),
    (0x6D, 0x56, "Num -"),
    (0x6B, 0x57, "Num +"),
    (0x6E, 0x63, "Num ."),
    (0x5D, 0x65, "Menu"),
];

const VK_A: u16 = 0x41;
const VK_0: u16 = 0x30;
const VK_NUMPAD0: u16 = 0x60;
const VK_F1: u16 = 0x70;
const HID_A: u8 = 0x04;
const HID_1: u8 = 0x1E;
const HID_0: u8 = 0x27;
const HID_NUM1: u8 = 0x59;
const HID_NUM0: u8 = 0x62;
const HID_F1: u8 = 0x3A;
const HID_F13: u8 = 0x68;

/// HID usage for a virtual key, `None` for modifiers and unsupported keys.
pub fn hid_of(vk: u16) -> Option<u8> {
    match vk {
        0x41..=0x5A => Some(HID_A + (vk - VK_A) as u8),
        0x31..=0x39 => Some(HID_1 + (vk - 0x31) as u8),
        VK_0 => Some(HID_0),
        0x61..=0x69 => Some(HID_NUM1 + (vk - 0x61) as u8),
        VK_NUMPAD0 => Some(HID_NUM0),
        0x70..=0x7B => Some(HID_F1 + (vk - VK_F1) as u8),
        0x7C..=0x87 => Some(HID_F13 + (vk - 0x7C) as u8),
        _ => TABLE.iter().find(|(v, _, _)| *v == vk).map(|(_, h, _)| *h),
    }
}

/// Short name of a HID usage, e.g. `0x04` -> "A", `0x3A` -> "F1".
pub fn name_of(usage: u8) -> String {
    match usage {
        0x04..=0x1D => ((b'A' + usage - HID_A) as char).to_string(),
        0x1E..=0x26 => ((b'1' + usage - HID_1) as char).to_string(),
        HID_0 => "0".into(),
        0x59..=0x61 => format!("Num {}", usage - HID_NUM1 + 1),
        HID_NUM0 => "Num 0".into(),
        0x3A..=0x45 => format!("F{}", usage - HID_F1 + 1),
        0x68..=0x73 => format!("F{}", usage - HID_F13 + 13),
        _ => TABLE.iter().find(|(_, h, _)| *h == usage).map_or_else(|| format!("#{usage:02X}"), |(_, _, n)| n.to_string()),
    }
}

/// Modifier keys as macro modifier bits (HID usages E0..E7).
/// `extended` distinguishes right Ctrl/Alt; right Shift has its own vk.
pub fn macro_mod_of(vk: u16, extended: bool) -> Option<u8> {
    match vk {
        0x10 | 0xA0 => Some(0x02),
        0xA1 => Some(0x20),
        0x11 | 0xA2 => Some(if extended { 0x10 } else { 0x01 }),
        0xA3 => Some(0x10),
        0x12 | 0xA4 => Some(if extended { 0x40 } else { 0x04 }),
        0xA5 => Some(0x40),
        0x5B => Some(0x08),
        0x5C => Some(0x80),
        _ => None,
    }
}

/// Name of macro modifier bits, e.g. `0x01` -> "Ctrl".
pub fn mod_name(bits: u8) -> &'static str {
    match bits {
        0x01 | 0x10 => "Ctrl",
        0x02 | 0x20 => "Shift",
        0x04 => "Alt",
        0x40 => "AltGr",
        _ => "Win",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_function_keys() {
        assert_eq!(hid_of(0x41), Some(0x04));
        assert_eq!(hid_of(0x5A), Some(0x1D));
        assert_eq!(hid_of(0x31), Some(0x1E));
        assert_eq!(hid_of(0x30), Some(0x27));
        assert_eq!(hid_of(0x70), Some(0x3A));
        assert_eq!(hid_of(0x7B), Some(0x45));
        assert_eq!(hid_of(0x60), Some(0x62));
        assert_eq!(hid_of(0x10), None);
    }

    #[test]
    fn names_roundtrip() {
        for vk in (0x08..=0xDE).filter_map(hid_of) {
            assert!(!name_of(vk).starts_with('#'), "usage {vk:#x} has no name");
        }
        assert_eq!(name_of(0x04), "A");
        assert_eq!(name_of(0x45), "F12");
    }

    #[test]
    fn modifiers() {
        assert_eq!(macro_mod_of(0x11, false), Some(0x01));
        assert_eq!(macro_mod_of(0x11, true), Some(0x10));
        assert_eq!(hid_of(0x11), None);
    }
}
