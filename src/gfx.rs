//! Drawing vocabulary of the view: geometry, colors, text style. The
//! drawing surface `Gfx` comes from the platform backend and works in
//! device-independent pixels.

pub use crate::platform::gfx::Gfx;

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Shrinks by `d` on every side.
    pub fn inset(&self, d: f32) -> Rect {
        Rect::new(self.x + d, self.y + d, self.w - 2.0 * d, self.h - 2.0 * d)
    }
}

/// `0xRRGGBB` with alpha `0..=255`.
#[derive(Clone, Copy, PartialEq)]
pub struct Color(pub u32, pub u8);

impl Color {
    pub const fn rgb(v: u32) -> Self {
        Color(v, 255)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Normal,
    Semibold,
    Bold,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
    /// Top-left, wrapped at word boundaries.
    Wrap,
}
