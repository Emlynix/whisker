//! Tray icon content: the charge level and the logo, whose body fills
//! like a battery. Platform backends put the glyph into the notification
//! area.
//!
//! ```text
//!      ╲        ╱
//!       ─── ■ ───     whiskers and nose
//!      ╱        ╲
//!       ┌──┬──┐
//!       ├──┴──┤       body: fills from the bottom with the charge:
//!       │█████│       green > 50 %, amber > 20 %, red below,
//!       │█████│       blue while charging, empty when offline
//!       └─────┘
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
/// The logo's nose: the app's accent.
const NOSE: u32 = 0xff7a3d;

/// Edge of the square glyph in pixels.
pub const ICON_SIZE: usize = 32;

/// Body of the mouse: outer box, 2 px frame.
const BODY: (i32, i32, i32, i32) = (8, 24, 10, 32);
const STROKE: i32 = 2;
/// Row under the two main buttons.
const SPLIT_Y: i32 = 17;
/// The least fill shown, so 1 % is not an empty body.
const FILL_MIN_PCT: i32 = 8;

pub enum TrayAction {
    Open,
    About,
    Quit,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Offline,
    Battery { pct: u8, charging: bool },
}

/// Canvas of the glyph: `0xAARRGGBB`, row by row from the top.
struct Canvas {
    px: Vec<u32>,
}

impl Canvas {
    fn put(&mut self, x: i32, y: i32, rgb: u32) {
        if (0..SIZE).contains(&x) && (0..SIZE).contains(&y) {
            self.px[(y * SIZE + x) as usize] = 0xFF00_0000 | rgb;
        }
    }

    /// Fills `x0..x1` by `y0..y1`.
    fn rect(&mut self, (x0, x1, y0, y1): (i32, i32, i32, i32), rgb: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.put(x, y, rgb);
            }
        }
    }

    /// A 2 px thick line through the points, one pixel step at a time.
    fn trace(&mut self, pts: &[(i32, i32)], rgb: u32) {
        for pair in pts.windows(2) {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
            for k in 0..=steps {
                let (x, y) = (x0 + (x1 - x0) * k / steps, y0 + (y1 - y0) * k / steps);
                self.rect((x, x + 1, y, y + STROKE), rgb);
            }
        }
    }
}

/// The glyph as `ICON_SIZE`² pixels `0xAARRGGBB`, row by row from the top.
pub fn glyph(level: Level) -> Vec<u32> {
    /// Whiskers of the left side, fanning out from the nose; the right
    /// side mirrors them. Two straight ones: at 16 px more would blur.
    const WHISKERS: [&[(i32, i32)]; 2] = [&[(11, 3), (1, 0)], &[(11, 6), (1, 8)]];
    let mut c = Canvas { px: vec![0u32; (SIZE * SIZE) as usize] };
    let frame = if level == Level::Offline { FRAME_OFF } else { FRAME };
    let (x0, x1, y0, y1) = BODY;

    // The charge first: frame and split line go over it.
    if let Level::Battery { pct, charging } = level {
        let color = match pct {
            _ if charging => BLUE,
            p if p <= LOW => RED,
            p if p <= MID => AMBER,
            _ => GREEN,
        };
        let inner = (y1 - STROKE) - (y0 + STROKE);
        let height = (inner * (pct as i32).clamp(FILL_MIN_PCT, 100) / 100).max(STROKE);
        c.rect((x0 + STROKE, x1 - STROKE, y1 - STROKE - height, y1 - STROKE), color);
    }

    // Body: a frame with its corners cut, like the logo's chamfers.
    c.rect((x0 + STROKE, x1 - STROKE, y0, y0 + STROKE), frame);
    c.rect((x0 + STROKE, x1 - STROKE, y1 - STROKE, y1), frame);
    c.rect((x0, x0 + STROKE, y0 + STROKE, y1 - STROKE), frame);
    c.rect((x1 - STROKE, x1, y0 + STROKE, y1 - STROKE), frame);
    // The two buttons: a line between them and one under them.
    let mid = (x0 + x1) / 2;
    c.rect((mid - 1, mid + 1, y0, SPLIT_Y), frame);
    c.rect((x0, x1, SPLIT_Y, SPLIT_Y + STROKE), frame);

    for whisker in WHISKERS {
        c.trace(whisker, frame);
        let mirrored: Vec<(i32, i32)> = whisker.iter().map(|&(x, y)| (SIZE - 1 - x, y)).collect();
        c.trace(&mirrored, frame);
    }
    c.rect((mid - 2, mid + 2, 3, 7), NOSE);
    c.px
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pixels of `rgb` in the glyph.
    fn count(px: &[u32], rgb: u32) -> usize {
        px.iter().filter(|&&p| p == 0xFF00_0000 | rgb).count()
    }

    #[test]
    fn body_fills_with_the_charge() {
        let low = glyph(Level::Battery { pct: 30, charging: false });
        let high = glyph(Level::Battery { pct: 90, charging: false });
        assert!(count(&high, GREEN) > 2 * count(&low, AMBER));
        assert_eq!(count(&low, GREEN), 0);

        // Colors tell the state: red when low, blue while charging.
        assert!(count(&glyph(Level::Battery { pct: 10, charging: false }), RED) > 0);
        assert!(count(&glyph(Level::Battery { pct: 10, charging: true }), BLUE) > 0);
        // Nearly empty still shows a sliver.
        assert!(count(&glyph(Level::Battery { pct: 1, charging: false }), RED) > 0);
    }

    #[test]
    fn offline_is_an_empty_grey_mouse() {
        let off = glyph(Level::Offline);
        assert!(count(&off, FRAME_OFF) > 0);
        for fill in [GREEN, AMBER, RED, BLUE, FRAME] {
            assert_eq!(count(&off, fill), 0);
        }
        assert_eq!(off.len(), ICON_SIZE * ICON_SIZE);
    }

    #[test]
    fn glyph_is_symmetric() {
        let px = glyph(Level::Battery { pct: 60, charging: false });
        for y in 0..SIZE {
            for x in 0..SIZE / 2 {
                let (l, r) = (px[(y * SIZE + x) as usize], px[(y * SIZE + SIZE - 1 - x) as usize]);
                assert_eq!(l, r, "row {y}, column {x}");
            }
        }
    }
}

#[cfg(test)]
mod preview {
    use super::*;

    /// Writes the glyphs side by side as a PPM, for a look at them:
    /// `cargo test -- --ignored --nocapture tray_preview`, then open
    /// `target/tray-preview.ppm`.
    #[test]
    #[ignore]
    fn tray_preview() {
        const SCALE: usize = 8;
        const BG: [u8; 3] = [0x20, 0x22, 0x28];
        let levels = [
            Level::Battery { pct: 100, charging: false },
            Level::Battery { pct: 78, charging: false },
            Level::Battery { pct: 40, charging: false },
            Level::Battery { pct: 12, charging: false },
            Level::Battery { pct: 55, charging: true },
            Level::Offline,
        ];
        let (w, h) = (levels.len() * ICON_SIZE * SCALE, ICON_SIZE * SCALE);
        let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
        let glyphs: Vec<Vec<u32>> = levels.iter().map(|l| glyph(*l)).collect();
        for y in 0..h {
            for x in 0..w {
                let p = glyphs[x / (ICON_SIZE * SCALE)][(y / SCALE) * ICON_SIZE + (x / SCALE) % ICON_SIZE];
                let rgb = if p >> 24 == 0 { BG } else { [(p >> 16) as u8, (p >> 8) as u8, p as u8] };
                out.extend_from_slice(&rgb);
            }
        }
        std::fs::write("target/tray-preview.ppm", out).unwrap();
    }
}
