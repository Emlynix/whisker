//! Color themes. `System` follows the Windows light/dark app setting.

use crate::gfx::Color;
use crate::win::wide;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};

const PERSONALIZE_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
const LIGHT_VALUE: &str = "AppsUseLightTheme";

pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub bg: Color,
    pub side: Color,
    pub card: Color,
    pub card_hi: Color,
    pub border: Color,
    pub hover: Color,
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub accent_hover: Color,
    pub on_accent: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    pub scrim: Color,
}

/// Builds a theme from `[bg, side, card, card_hi, border, hover, text, muted,
/// faint, accent, accent_hover, on_accent, good, warn, bad]`.
const fn make(id: &'static str, name: &'static str, dark: bool, c: [u32; 15]) -> Theme {
    let rgb = Color::rgb;
    Theme {
        id,
        name,
        dark,
        bg: rgb(c[0]),
        side: rgb(c[1]),
        card: rgb(c[2]),
        card_hi: rgb(c[3]),
        border: rgb(c[4]),
        hover: rgb(c[5]),
        text: rgb(c[6]),
        muted: rgb(c[7]),
        faint: rgb(c[8]),
        accent: rgb(c[9]),
        accent_soft: Color(c[9], if dark { 0x26 } else { 0x1c }),
        accent_hover: rgb(c[10]),
        on_accent: rgb(c[11]),
        good: rgb(c[12]),
        warn: rgb(c[13]),
        bad: rgb(c[14]),
        scrim: Color(0x000000, if dark { 0xa0 } else { 0x60 }),
    }
}

pub static THEMES: [Theme; 9] = [
    make("dark", "Dark", true, [
        0x0c0f13, 0x10141a, 0x161b22, 0x1c2330, 0x232b36, 0x1a212b, 0xe8ecf2, 0x8b95a5,
        0x5d6675, 0x3ab8ff, 0x5cc6ff, 0x06121c, 0x3ddc84, 0xffb020, 0xff4d4f,
    ]),
    make("light", "Light", false, [
        0xf3f5f8, 0xe8ecf1, 0xffffff, 0xeef3f9, 0xd9dfe7, 0xe4e9ef, 0x1a1f27, 0x5b6573,
        0x98a2b0, 0x0a84d6, 0x2b97e0, 0xffffff, 0x1e9e5a, 0xc77d00, 0xd93a3c,
    ]),
    make("tokyo", "Tokyo Night", true, [
        0x1a1b26, 0x16161e, 0x1f2335, 0x292e42, 0x2f3549, 0x24283b, 0xc0caf5, 0x9aa5ce,
        0x565f89, 0x7aa2f7, 0x94b6fa, 0x1a1b26, 0x9ece6a, 0xe0af68, 0xf7768e,
    ]),
    make("dracula", "Dracula", true, [
        0x282a36, 0x21222c, 0x303341, 0x3a3d4f, 0x44475a, 0x363949, 0xf8f8f2, 0xc0c4d6,
        0x6272a4, 0xbd93f9, 0xcbabfa, 0x282a36, 0x50fa7b, 0xffb86c, 0xff5555,
    ]),
    make("monokai", "Monokai", true, [
        0x272822, 0x1e1f1c, 0x2f302a, 0x3a3b34, 0x49483e, 0x34352e, 0xf8f8f2, 0xa59f85,
        0x75715e, 0x66d9ef, 0x8be3f3, 0x272822, 0xa6e22e, 0xfd971f, 0xf92672,
    ]),
    make("solarized", "Solarized Dark", true, [
        0x002b36, 0x00252f, 0x073642, 0x0a4250, 0x0d4a58, 0x08404d, 0xeee8d5, 0x93a1a1,
        0x586e75, 0x268bd2, 0x4aa0dc, 0xfdf6e3, 0x859900, 0xb58900, 0xdc322f,
    ]),
    make("nord", "Nord", true, [
        0x2e3440, 0x292e39, 0x3b4252, 0x434c5e, 0x4c566a, 0x404859, 0xeceff4, 0xc5ccd8,
        0x7b88a1, 0x88c0d0, 0x9fcdda, 0x2e3440, 0xa3be8c, 0xebcb8b, 0xbf616a,
    ]),
    make("catppuccin", "Catppuccin", true, [
        0x1e1e2e, 0x181825, 0x262637, 0x313244, 0x3a3b52, 0x2b2c3f, 0xcdd6f4, 0xa6adc8,
        0x6c7086, 0xcba6f7, 0xd8bdf9, 0x1e1e2e, 0xa6e3a1, 0xf9e2af, 0xf38ba8,
    ]),
    make("gruvbox", "Gruvbox", true, [
        0x282828, 0x1d2021, 0x32302f, 0x3c3836, 0x504945, 0x383432, 0xebdbb2, 0xbdae93,
        0x7c6f64, 0x83a598, 0x9cb8ad, 0x1d2021, 0xb8bb26, 0xfabd2f, 0xfb4934,
    ]),
];

const DARK: usize = 0;
const LIGHT: usize = 1;

/// User's pick: follow Windows, or a fixed theme by index into `THEMES`.
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

/// Applies a choice; `System` reads the Windows setting now.
pub fn apply(c: Choice) {
    let i = match c {
        Choice::Fixed(i) => i.min(THEMES.len() - 1),
        Choice::System if os_prefers_light() => LIGHT,
        Choice::System => DARK,
    };
    CURRENT.store(i, Ordering::Relaxed);
}

fn os_prefers_light() -> bool {
    let (key, name) = (wide(PERSONALIZE_KEY), wide(LIGHT_VALUE));
    let mut value: u32 = 0;
    let mut size = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&mut value as *mut u32).cast(), &mut size)
    };
    r == 0 && value == 1
}
