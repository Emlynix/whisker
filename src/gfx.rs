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

/// Type ramp: the only text styles of the UI. Two faces with strict
/// roles: `Ui` (Inter) for words, `Mono` (JetBrains Mono) for numbers,
/// values, the page title and tags.
///
/// ```text
///  Ui    Caption 12  notes, descriptions     Mono  Tag     10  PRO tag
///        Label   12  small captions                Key     11  key caps, scales
///        Body    13  text, rows, buttons           Fine    12  second line of a segment
///        Large   14  emphasis                      Value   13  values, segments, inputs
///        Subtitle 16 dialog titles                 AppName 12  "WHISKER", spaced
///                                                  Title   18  page title, spaced
///                                                  Hero    28  big numbers, tightened
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Size {
    Caption,
    Label,
    Body,
    Large,
    Subtitle,
    Tag,
    Key,
    Fine,
    Value,
    AppName,
    Title,
    Hero,
}

/// Font family of a text style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Face {
    Ui,
    Mono,
}

impl Size {
    pub const fn px(self) -> f32 {
        match self {
            Size::Tag => 10.0,
            Size::Key => 11.0,
            Size::Caption | Size::Label | Size::Fine | Size::AppName => 12.0,
            Size::Body | Size::Value => 13.0,
            Size::Large => 14.0,
            Size::Subtitle => 16.0,
            Size::Title => 18.0,
            Size::Hero => 28.0,
        }
    }

    /// Extra space after each character, in DIPs (negative tightens):
    /// 0.1 em for the tag and the app name, 0.04 em for the page title,
    /// -0.03 em for big numbers.
    pub const fn tracking(self) -> f32 {
        match self {
            Size::Tag => 1.0,
            Size::AppName => 1.2,
            Size::Title => 0.72,
            Size::Hero => -0.84,
            _ => 0.0,
        }
    }

    pub const fn face(self) -> Face {
        match self {
            Size::Tag | Size::Key | Size::Fine | Size::Value | Size::AppName | Size::Title | Size::Hero => Face::Mono,
            _ => Face::Ui,
        }
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
    Medium,
    Semibold,
    /// The bundled fonts end at Semibold: drawn as that.
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
