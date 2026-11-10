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

/// Type ramp (Fluent 2): the only text sizes of the UI.
///
/// ```text
///  Caption 12   notes, axis labels      Subtitle 20  dialog titles, tile values
///  Label   12   UPPER-CASE captions     Title    28  page titles, card values
///  Body    14   text, rows, buttons     Hero     40  battery level
///  Large   16   emphasis, tile titles
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Size {
    Caption,
    /// Upper-case section caption: Semibold, letter-spaced.
    Label,
    Body,
    Large,
    Subtitle,
    Title,
    Hero,
}

/// Optical size of the UI font: Text cut for small sizes, Display from 20 px.
/// Linux: one family (Pango).
#[cfg_attr(target_os = "linux", allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Text,
    Display,
}

impl Size {
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    const DISPLAY_FROM: f32 = 20.0;
    const LABEL_TRACKING: f32 = 0.4;

    pub const fn px(self) -> f32 {
        match self {
            Size::Caption | Size::Label => 12.0,
            Size::Body => 14.0,
            Size::Large => 16.0,
            Size::Subtitle => 20.0,
            Size::Title => 28.0,
            Size::Hero => 40.0,
        }
    }

    /// Extra space after each character, in DIPs.
    pub const fn tracking(self) -> f32 {
        match self {
            Size::Label => Self::LABEL_TRACKING,
            _ => 0.0,
        }
    }

    #[cfg_attr(target_os = "linux", allow(dead_code))]
    pub fn face(self) -> Face {
        if self.px() >= Self::DISPLAY_FROM { Face::Display } else { Face::Text }
    }
}

/// Outline step of a path (`Gfx::fill_path`, `Gfx::stroke_path`) in DIPs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Seg {
    /// Starts a new figure.
    Move(f32, f32),
    Line(f32, f32),
    /// Cubic Bezier: two control points, then the end point.
    Curve(f32, f32, f32, f32, f32, f32),
    /// Closes the figure back to its `Move`.
    Close,
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
