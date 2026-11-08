//! Tray icon content: the charge level and a battery glyph whose fill
//! shows it. Platform backends put the glyph into the notification area.
//!
//! ```text
//!   +--------------+-+
//!   |██████████    | |   green > 50 %, amber > 20 %, red below,
//!   +--------------+-+   blue while charging, empty when offline
//! ```

const SIZE: i32 = ICON_SIZE as i32;
const LOW: u8 = 20;
const MID: u8 = 50;

const GREEN: u32 = 0x3ddc84;
const AMBER: u32 = 0xffb020;
const RED: u32 = 0xff4d4f;
const BLUE: u32 = 0x3ab8ff;
const FRAME: u32 = 0xe8ecf2;
const FRAME_OFF: u32 = 0x808894;

/// Edge of the square glyph in pixels.
pub const ICON_SIZE: usize = 32;

pub enum TrayAction {
    Open,
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Offline,
    Battery { pct: u8, charging: bool },
}

/// Battery glyph as `ICON_SIZE`² pixels `0xAARRGGBB`, row by row from the top.
pub fn glyph(level: Level) -> Vec<u32> {
    let mut px = vec![0u32; (SIZE * SIZE) as usize];
    let mut put = |x: i32, y: i32, rgb: u32| px[(y * SIZE + x) as usize] = 0xFF00_0000 | rgb;

    // Body 2..27 x 8..24 with a 2 px frame, terminal 27..30 x 13..19.
    let (x0, x1, y0, y1) = (2, 27, 8, 24);
    let frame = if level == Level::Offline { FRAME_OFF } else { FRAME };
    for y in y0..y1 {
        for x in x0..x1 {
            if x < x0 + 2 || x >= x1 - 2 || y < y0 + 2 || y >= y1 - 2 {
                put(x, y, frame);
            }
        }
    }
    for y in 13..19 {
        for x in x1..x1 + 3 {
            put(x, y, frame);
        }
    }

    if let Level::Battery { pct, charging } = level {
        let color = match pct {
            _ if charging => BLUE,
            p if p <= LOW => RED,
            p if p <= MID => AMBER,
            _ => GREEN,
        };
        let inner = (x1 - 4) - (x0 + 4);
        let width = (inner * pct.max(8) as i32 / 100).max(2);
        for y in y0 + 4..y1 - 4 {
            for x in x0 + 4..x0 + 4 + width {
                put(x, y, color);
            }
        }
    }

    px
}
