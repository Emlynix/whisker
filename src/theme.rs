//! Color themes. `System` follows the OS light/dark app setting.

use crate::gfx::Color;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Colors of one theme, by role (design tokens). Themes change colors
/// only: layout, sizes and fonts are the same in all of them.
///
/// ```text
///  side  title bar, sidebar, status bar     line   hairlines, dividers
///  bg    content area                       line2  control borders
///  panel panels, dialogs                    input  inputs, selects, segments
///  sel   selected nav item, hover           track  slider and progress track
///  acc   accent as TEXT and outlines        acc_fill  accent as FILL
///  acc_on  text on acc_fill                 acc_soft  selected segment / row
/// ```
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub bg: Color,
    pub side: Color,
    pub panel: Color,
    pub sel: Color,
    pub line: Color,
    pub line2: Color,
    pub text: Color,
    /// Secondary text; passes 4.5:1 (WCAG AA) on `bg`, `side` and `panel`.
    pub muted: Color,
    pub acc: Color,
    pub acc_fill: Color,
    pub acc_on: Color,
    pub acc_soft: Color,
    pub ok: Color,
    pub input: Color,
    pub track: Color,
    pub warn: Color,
    pub bad: Color,
    pub scrim: Color,

    // Names from before the redesign, until every screen is redrawn.
    pub card: Color,
    pub card_hi: Color,
    pub border: Color,
    pub hover: Color,
    pub subtle: Color,
    /// Decoration and disabled items only, never information.
    pub faint: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub accent_hover: Color,
    pub on_accent: Color,
    pub good: Color,
}

/// The fifteen role colors plus warning and error, as `0xRRGGBB`.
struct Tokens {
    bg: u32,
    side: u32,
    panel: u32,
    sel: u32,
    line: u32,
    line2: u32,
    text: u32,
    muted: u32,
    acc: u32,
    acc_fill: u32,
    acc_on: u32,
    acc_soft: u32,
    ok: u32,
    input: u32,
    track: u32,
    warn: u32,
    bad: u32,
}

const fn make(id: &'static str, name: &'static str, dark: bool, t: Tokens) -> Theme {
    let rgb = Color::rgb;
    Theme {
        id,
        name,
        dark,
        bg: rgb(t.bg),
        side: rgb(t.side),
        panel: rgb(t.panel),
        sel: rgb(t.sel),
        line: rgb(t.line),
        line2: rgb(t.line2),
        text: rgb(t.text),
        muted: rgb(t.muted),
        acc: rgb(t.acc),
        acc_fill: rgb(t.acc_fill),
        acc_on: rgb(t.acc_on),
        acc_soft: rgb(t.acc_soft),
        ok: rgb(t.ok),
        input: rgb(t.input),
        track: rgb(t.track),
        warn: rgb(t.warn),
        bad: rgb(t.bad),
        scrim: Color(0x000000, if dark { 0xa0 } else { 0x60 }),

        card: rgb(t.panel),
        card_hi: rgb(t.panel),
        border: rgb(t.line),
        hover: rgb(t.sel),
        subtle: rgb(t.muted),
        faint: rgb(t.line2),
        accent: rgb(t.acc),
        accent_soft: rgb(t.acc_soft),
        accent_hover: rgb(t.acc),
        on_accent: rgb(t.acc_on),
        good: rgb(t.ok),
    }
}

/// The design's four palettes. More can follow; a theme is only colors.
pub static THEMES: [Theme; 4] = [
    make("dark", "Dark", true, Tokens {
        bg: 0x141517, side: 0x101113, panel: 0x191a1d, sel: 0x22242a, line: 0x2c2e34, line2: 0x3e414a,
        text: 0xe5e5e3, muted: 0xa0a2a8, acc: 0xff7a3d, acc_fill: 0xff7a3d, acc_on: 0x141517, acc_soft: 0x33221a,
        ok: 0x5cc48a, input: 0x0f1012, track: 0x2c2e34, warn: 0xffb020, bad: 0xff6b6b,
    }),
    make("light", "Light", false, Tokens {
        bg: 0xeeeeec, side: 0xf7f7f5, panel: 0xfcfcfb, sel: 0xe6e6e3, line: 0xd3d3ce, line2: 0xb5b5af,
        text: 0x1b1b1a, muted: 0x5c5c58, acc: 0xc2410c, acc_fill: 0xf25a1d, acc_on: 0x1b1b1a, acc_soft: 0xfdede5,
        ok: 0x2e7d32, input: 0xffffff, track: 0xdcdcd8, warn: 0x8a5200, bad: 0xc62828,
    }),
    make("tokyo", "Tokyo Night", true, Tokens {
        bg: 0x1a1b26, side: 0x16161e, panel: 0x1f2335, sel: 0x2a2f45, line: 0x2f344c, line2: 0x414868,
        text: 0xc0caf5, muted: 0x9aa5ce, acc: 0xff9e64, acc_fill: 0xff9e64, acc_on: 0x1a1b26, acc_soft: 0x3a2e35,
        ok: 0x9ece6a, input: 0x16161e, track: 0x2f344c, warn: 0xe0af68, bad: 0xf7768e,
    }),
    make("dracula", "Dracula", true, Tokens {
        bg: 0x282a36, side: 0x21222c, panel: 0x2c2e3b, sel: 0x393b4c, line: 0x3e4052, line2: 0x50536a,
        text: 0xf8f8f2, muted: 0xb4b8cf, acc: 0xffb86c, acc_fill: 0xffb86c, acc_on: 0x21222c, acc_soft: 0x4a3f3c,
        ok: 0x50fa7b, input: 0x21222c, track: 0x44475a, warn: 0xf1fa8c, bad: 0xff5555,
    }),
];

const DARK: usize = 0;
const LIGHT: usize = 1;

/// User's pick: follow the OS, or a fixed theme by index into `THEMES`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    System,
    Fixed(usize),
}

impl Choice {
    pub fn id(self) -> &'static str {
        match self {
            Choice::System => "system",
            Choice::Fixed(i) => THEMES[i].id,
        }
    }

    pub fn from_id(id: &str) -> Choice {
        THEMES.iter().position(|t| t.id == id).map(Choice::Fixed).unwrap_or(Choice::System)
    }
}

static CURRENT: AtomicUsize = AtomicUsize::new(DARK);

pub fn current() -> &'static Theme {
    &THEMES[CURRENT.load(Ordering::Relaxed)]
}

/// Applies a choice; `System` reads the OS setting now.
pub fn apply(c: Choice) {
    let i = match c {
        Choice::Fixed(i) => i.min(THEMES.len() - 1),
        Choice::System if crate::platform::dark_mode() => DARK,
        Choice::System => LIGHT,
    };
    CURRENT.store(i, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG relative luminance of an sRGB color.
    fn luminance(c: Color) -> f64 {
        let lin = |shift: u32| {
            let v = ((c.0 >> shift) & 0xFF) as f64 / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(16) + 0.7152 * lin(8) + 0.0722 * lin(0)
    }

    fn contrast(a: Color, b: Color) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        assert!((contrast(Color::rgb(0x000000), Color::rgb(0xffffff)) - 21.0).abs() < 0.01);
        assert!((contrast(Color::rgb(0x777777), Color::rgb(0xffffff)) - 4.48).abs() < 0.01);
    }

    /// Text tokens of every theme pass WCAG AA (4.5:1) on every surface
    /// text sits on; so do accent text and labels on the accent fill.
    #[test]
    fn text_tokens_pass_wcag_aa() {
        const AA: f64 = 4.5;
        for t in &THEMES {
            let surfaces = [("bg", t.bg), ("side", t.side), ("panel", t.panel), ("input", t.input), ("sel", t.sel)];
            for (name, fg) in [("text", t.text), ("muted", t.muted)] {
                for (surface, bg) in surfaces {
                    let r = contrast(fg, bg);
                    assert!(r >= AA, "{}: {name} on {surface} is {r:.2}", t.id);
                }
            }
            // Accent text sits on panels and on the selected segment, never
            // straight on the window background.
            for (surface, bg) in [("panel", t.panel), ("acc_soft", t.acc_soft)] {
                let r = contrast(t.acc, bg);
                assert!(r >= AA, "{}: acc on {surface} is {r:.2}", t.id);
            }
            let r = contrast(t.acc_on, t.acc_fill);
            assert!(r >= AA, "{}: acc_on on acc_fill is {r:.2}", t.id);
        }
    }
}
