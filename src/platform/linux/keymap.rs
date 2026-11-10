//! Linux evdev key codes (`KEY_*`, layout independent positions) to the
//! Windows virtual keys the core speaks (`shell::KeyEvent::vk`). GDK's
//! hardware keycode is the evdev code plus `XKB_OFFSET` on X11 and
//! Wayland alike. Pure tables, no I/O.

/// X keycodes start 8 above evdev codes.
const XKB_OFFSET: u16 = 8;

/// `(evdev code, vk, extended)`.
const TABLE: &[(u16, u16, bool)] = &[
    // Letters.
    (30, 0x41, false), // A
    (48, 0x42, false), // B
    (46, 0x43, false), // C
    (32, 0x44, false), // D
    (18, 0x45, false), // E
    (33, 0x46, false), // F
    (34, 0x47, false), // G
    (35, 0x48, false), // H
    (23, 0x49, false), // I
    (36, 0x4A, false), // J
    (37, 0x4B, false), // K
    (38, 0x4C, false), // L
    (50, 0x4D, false), // M
    (49, 0x4E, false), // N
    (24, 0x4F, false), // O
    (25, 0x50, false), // P
    (16, 0x51, false), // Q
    (19, 0x52, false), // R
    (31, 0x53, false), // S
    (20, 0x54, false), // T
    (22, 0x55, false), // U
    (47, 0x56, false), // V
    (17, 0x57, false), // W
    (45, 0x58, false), // X
    (21, 0x59, false), // Y
    (44, 0x5A, false), // Z
    // Digit row: KEY_1..KEY_9 are 2..10, KEY_0 is 11.
    (11, 0x30, false),
    (2, 0x31, false),
    (3, 0x32, false),
    (4, 0x33, false),
    (5, 0x34, false),
    (6, 0x35, false),
    (7, 0x36, false),
    (8, 0x37, false),
    (9, 0x38, false),
    (10, 0x39, false),
    // Punctuation.
    (12, 0xBD, false), // -
    (13, 0xBB, false), // =
    (26, 0xDB, false), // [
    (27, 0xDD, false), // ]
    (43, 0xDC, false), // \
    (39, 0xBA, false), // ;
    (40, 0xDE, false), // '
    (41, 0xC0, false), // `
    (51, 0xBC, false), // ,
    (52, 0xBE, false), // .
    (53, 0xBF, false), // /
    (86, 0xE2, false), // 102nd key (ISO <>)
    // Editing and navigation.
    (28, 0x0D, false),  // Enter
    (96, 0x0D, true),   // Keypad Enter
    (1, 0x1B, false),   // Escape
    (14, 0x08, false),  // Backspace
    (15, 0x09, false),  // Tab
    (57, 0x20, false),  // Space
    (58, 0x14, false),  // Caps Lock
    (110, 0x2D, true),  // Insert
    (102, 0x24, true),  // Home
    (104, 0x21, true),  // Page Up
    (111, 0x2E, true),  // Delete
    (107, 0x23, true),  // End
    (109, 0x22, true),  // Page Down
    (105, 0x25, true),  // ←
    (106, 0x27, true),  // →
    (108, 0x28, true),  // ↓
    (103, 0x26, true),  // ↑
    (99, 0x2C, true),   // Print Screen (SysRq)
    (70, 0x91, false),  // Scroll Lock
    (119, 0x13, false), // Pause
    (127, 0x5D, true),  // Menu (Compose)
    // Keypad.
    (82, 0x60, false),
    (79, 0x61, false),
    (80, 0x62, false),
    (81, 0x63, false),
    (75, 0x64, false),
    (76, 0x65, false),
    (77, 0x66, false),
    (71, 0x67, false),
    (72, 0x68, false),
    (73, 0x69, false),
    (55, 0x6A, false), // *
    (78, 0x6B, false), // +
    (74, 0x6D, false), // -
    (83, 0x6E, false), // .
    (98, 0x6F, true),  // /
    (69, 0x90, true),  // Num Lock
    // Function keys: F1..F10 are 59..68, F11/F12 87/88, F13..F24 183..194.
    (59, 0x70, false),
    (60, 0x71, false),
    (61, 0x72, false),
    (62, 0x73, false),
    (63, 0x74, false),
    (64, 0x75, false),
    (65, 0x76, false),
    (66, 0x77, false),
    (67, 0x78, false),
    (68, 0x79, false),
    (87, 0x7A, false),
    (88, 0x7B, false),
    (183, 0x7C, false),
    (184, 0x7D, false),
    (185, 0x7E, false),
    (186, 0x7F, false),
    (187, 0x80, false),
    (188, 0x81, false),
    (189, 0x82, false),
    (190, 0x83, false),
    (191, 0x84, false),
    (192, 0x85, false),
    (193, 0x86, false),
    (194, 0x87, false),
    // Modifiers (left/right kept apart).
    (42, 0xA0, false),  // Left Shift
    (54, 0xA1, false),  // Right Shift
    (29, 0x11, false),  // Left Control
    (97, 0x11, true),   // Right Control
    (56, 0x12, false),  // Left Alt
    (100, 0x12, true),  // Right Alt (AltGr)
    (125, 0x5B, false), // Left Super = Win
    (126, 0x5C, false), // Right Super
];

/// `(vk, extended)` of a GDK hardware keycode.
pub fn vk_of(keycode: u16) -> Option<(u16, bool)> {
    let code = keycode.checked_sub(XKB_OFFSET)?;
    TABLE.iter().find(|k| k.0 == code).map(|k| (k.1, k.2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        for (i, a) in TABLE.iter().enumerate() {
            assert!(TABLE[i + 1..].iter().all(|b| b.0 != a.0), "duplicate code {}", a.0);
        }
    }

    #[test]
    fn x_keycodes_map() {
        // X keycode 38 = evdev 30 = A; 108 = Right Alt.
        assert_eq!(vk_of(38), Some((0x41, false)));
        assert_eq!(vk_of(108), Some((0x12, true)));
        assert_eq!(vk_of(3), None);
    }
}
