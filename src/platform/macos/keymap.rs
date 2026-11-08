//! macOS virtual key codes (`kVK_*`, layout independent positions) to the
//! Windows virtual keys the core speaks (`shell::KeyEvent::vk`).
//! Pure tables, no I/O.

/// `(mac key code, vk, extended)`.
const TABLE: &[(u16, u16, bool)] = &[
    // Letters (ANSI positions).
    (0x00, 0x41, false), // A
    (0x0B, 0x42, false), // B
    (0x08, 0x43, false), // C
    (0x02, 0x44, false), // D
    (0x0E, 0x45, false), // E
    (0x03, 0x46, false), // F
    (0x05, 0x47, false), // G
    (0x04, 0x48, false), // H
    (0x22, 0x49, false), // I
    (0x26, 0x4A, false), // J
    (0x28, 0x4B, false), // K
    (0x25, 0x4C, false), // L
    (0x2E, 0x4D, false), // M
    (0x2D, 0x4E, false), // N
    (0x1F, 0x4F, false), // O
    (0x23, 0x50, false), // P
    (0x0C, 0x51, false), // Q
    (0x0F, 0x52, false), // R
    (0x01, 0x53, false), // S
    (0x11, 0x54, false), // T
    (0x20, 0x55, false), // U
    (0x09, 0x56, false), // V
    (0x0D, 0x57, false), // W
    (0x07, 0x58, false), // X
    (0x10, 0x59, false), // Y
    (0x06, 0x5A, false), // Z
    // Digit row.
    (0x1D, 0x30, false),
    (0x12, 0x31, false),
    (0x13, 0x32, false),
    (0x14, 0x33, false),
    (0x15, 0x34, false),
    (0x17, 0x35, false),
    (0x16, 0x36, false),
    (0x1A, 0x37, false),
    (0x1C, 0x38, false),
    (0x19, 0x39, false),
    // Punctuation.
    (0x1B, 0xBD, false), // -
    (0x18, 0xBB, false), // =
    (0x21, 0xDB, false), // [
    (0x1E, 0xDD, false), // ]
    (0x2A, 0xDC, false), // \
    (0x29, 0xBA, false), // ;
    (0x27, 0xDE, false), // '
    (0x32, 0xC0, false), // `
    (0x2B, 0xBC, false), // ,
    (0x2F, 0xBE, false), // .
    (0x2C, 0xBF, false), // /
    // Editing and navigation.
    (0x24, 0x0D, false), // Return
    (0x4C, 0x0D, true),  // Keypad Enter
    (0x35, 0x1B, false), // Escape
    (0x33, 0x08, false), // Delete (backspace)
    (0x30, 0x09, false), // Tab
    (0x31, 0x20, false), // Space
    (0x39, 0x14, false), // Caps Lock
    (0x72, 0x2D, true),  // Help = Insert position
    (0x73, 0x24, true),  // Home
    (0x74, 0x21, true),  // Page Up
    (0x75, 0x2E, true),  // Forward Delete
    (0x77, 0x23, true),  // End
    (0x79, 0x22, true),  // Page Down
    (0x7B, 0x25, true),  // ←
    (0x7C, 0x27, true),  // →
    (0x7D, 0x28, true),  // ↓
    (0x7E, 0x26, true),  // ↑
    // Keypad.
    (0x52, 0x60, false),
    (0x53, 0x61, false),
    (0x54, 0x62, false),
    (0x55, 0x63, false),
    (0x56, 0x64, false),
    (0x57, 0x65, false),
    (0x58, 0x66, false),
    (0x59, 0x67, false),
    (0x5B, 0x68, false),
    (0x5C, 0x69, false),
    (0x43, 0x6A, false), // *
    (0x45, 0x6B, false), // +
    (0x4E, 0x6D, false), // -
    (0x41, 0x6E, false), // .
    (0x4B, 0x6F, true),  // /
    (0x47, 0x90, false), // Clear = Num Lock position
    // Function keys.
    (0x7A, 0x70, false),
    (0x78, 0x71, false),
    (0x63, 0x72, false),
    (0x76, 0x73, false),
    (0x60, 0x74, false),
    (0x61, 0x75, false),
    (0x62, 0x76, false),
    (0x64, 0x77, false),
    (0x65, 0x78, false),
    (0x6D, 0x79, false),
    (0x67, 0x7A, false),
    (0x6F, 0x7B, false),
    (0x69, 0x7C, false),
    (0x6B, 0x7D, false),
    (0x71, 0x7E, false),
    (0x6A, 0x7F, false),
    (0x40, 0x80, false),
    (0x4F, 0x81, false),
    (0x50, 0x82, false),
    (0x5A, 0x83, false),
];

/// One modifier key: mac key code, vk, extended, device-dependent flag bit
/// (`NX_DEVICE*KEYMASK`) that is set while that very key is held.
const MODIFIERS: &[(u16, u16, bool, usize)] = &[
    (0x38, 0xA0, false, 0x0002), // Left Shift
    (0x3C, 0xA1, false, 0x0004), // Right Shift
    (0x3B, 0x11, false, 0x0001), // Left Control
    (0x3E, 0x11, true, 0x2000),  // Right Control
    (0x3A, 0x12, false, 0x0020), // Left Option
    (0x3D, 0x12, true, 0x0040),  // Right Option
    (0x37, 0x5B, false, 0x0008), // Left Command = Win
    (0x36, 0x5C, false, 0x0010), // Right Command
];

/// `(vk, extended)` of a non-modifier key.
pub fn vk_of(code: u16) -> Option<(u16, bool)> {
    TABLE.iter().find(|k| k.0 == code).map(|k| (k.1, k.2))
}

/// `(vk, extended, down)` for a modifier key event, given the raw
/// modifier flags after the change.
pub fn modifier_of(code: u16, flags: usize) -> Option<(u16, bool, bool)> {
    let m = MODIFIERS.iter().find(|m| m.0 == code)?;
    Some((m.1, m.2, flags & m.3 != 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        for (i, a) in TABLE.iter().enumerate() {
            assert!(TABLE[i + 1..].iter().all(|b| b.0 != a.0), "duplicate code {:#x}", a.0);
        }
    }

    #[test]
    fn right_option_is_extended_alt() {
        assert_eq!(modifier_of(0x3D, 0x40), Some((0x12, true, true)));
        assert_eq!(modifier_of(0x3D, 0x20), Some((0x12, true, false)));
    }
}
