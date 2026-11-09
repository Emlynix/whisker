//! Drawing driver over Cairo (shapes) and PangoCairo (text) on the
//! drawing area's context. GTK hands the context in logical pixels with
//! the monitor scale already applied, so coordinates are DIPs as is.

use crate::gfx::{Align, Color, Rect, Seg, Size, Weight};
use gtk::cairo::{self, LineCap, LineJoin};
use gtk::prelude::*;
use gtk::pango::{self, AttrInt, AttrList, EllipsizeMode, FontDescription, Layout, WrapMode};
use std::f64::consts::{FRAC_PI_2, PI};

/// Used when GTK reports no UI font.
const FALLBACK_FAMILY: &str = "Sans";

/// `(r, g, b, a)` in `0.0..=1.0`.
fn channels(c: Color) -> (f64, f64, f64, f64) {
    let ch = |shift: u32| ((c.0 >> shift) & 0xFF) as f64 / 255.0;
    (ch(16), ch(8), ch(0), c.1 as f64 / 255.0)
}

/// Pango units from DIPs.
fn units(v: f32) -> i32 {
    (v as f64 * pango::SCALE as f64).round() as i32
}

fn dips(v: i32) -> f32 {
    v as f32 / pango::SCALE as f32
}

pub struct Gfx {
    cr: Option<cairo::Context>,
    /// Text context of the current frame (font options of the target).
    text: Option<pango::Context>,
    width: f32,
    height: f32,
    /// Desktop UI font family, read once.
    family: Option<String>,
}

impl Gfx {
    pub fn new() -> Self {
        Self { cr: None, text: None, width: 0.0, height: 0.0, family: None }
    }

    /// Starts a frame on the context GTK prepared for `draw`.
    pub fn begin(&mut self, cr: &cairo::Context, w: f32, h: f32) {
        (self.width, self.height) = (w, h);
        self.text = Some(pangocairo::functions::create_context(cr));
        self.cr = Some(cr.clone());
    }

    pub fn end(&mut self) {
        self.cr = None;
        self.text = None;
    }

    /// Re-reads the UI font while the window is hidden (it may change).
    pub fn release(&mut self) {
        self.family = None;
    }

    fn set_fill(&self, c: Color) -> Option<&cairo::Context> {
        let cr = self.cr.as_ref()?;
        let (r, g, b, a) = channels(c);
        cr.set_source_rgba(r, g, b, a);
        Some(cr)
    }

    fn set_stroke(&self, width: f32, c: Color) -> Option<&cairo::Context> {
        let cr = self.set_fill(c)?;
        cr.set_line_width(width as f64);
        cr.set_line_join(LineJoin::Miter);
        cr.set_line_cap(LineCap::Butt);
        Some(cr)
    }

    pub fn clear(&self, c: Color) {
        let Some(cr) = self.set_fill(c) else {
            return;
        };
        cr.rectangle(0.0, 0.0, self.width as f64, self.height as f64);
        let _ = cr.fill();
    }

    /// Rounded rect path; the radius is clamped to half the shorter side.
    fn rounded(cr: &cairo::Context, r: Rect, radius: f32) {
        let (x, y, w, h) = (r.x as f64, r.y as f64, r.w.max(0.0) as f64, r.h.max(0.0) as f64);
        let k = (radius as f64).min(w / 2.0).min(h / 2.0).max(0.0);
        cr.new_path();
        if k == 0.0 {
            cr.rectangle(x, y, w, h);
            return;
        }
        // Clockwise from the top-right corner.
        cr.arc(x + w - k, y + k, k, -FRAC_PI_2, 0.0);
        cr.arc(x + w - k, y + h - k, k, 0.0, FRAC_PI_2);
        cr.arc(x + k, y + h - k, k, FRAC_PI_2, PI);
        cr.arc(x + k, y + k, k, PI, PI + FRAC_PI_2);
        cr.close_path();
    }

    pub fn fill(&self, r: Rect, radius: f32, c: Color) {
        let Some(cr) = self.set_fill(c) else {
            return;
        };
        Self::rounded(cr, r, radius);
        let _ = cr.fill();
    }

    pub fn stroke(&self, r: Rect, radius: f32, width: f32, c: Color) {
        let Some(cr) = self.set_stroke(width, c) else {
            return;
        };
        // Inset by half the stroke so the border stays inside `r`.
        Self::rounded(cr, r.inset(width / 2.0), radius);
        let _ = cr.stroke();
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        let Some(cr) = self.set_fill(c) else {
            return;
        };
        cr.new_path();
        cr.arc(cx as f64, cy as f64, radius.max(0.0) as f64, 0.0, 2.0 * PI);
        let _ = cr.fill();
    }

    pub fn ring(&self, cx: f32, cy: f32, radius: f32, width: f32, c: Color) {
        let Some(cr) = self.set_stroke(width, c) else {
            return;
        };
        cr.new_path();
        cr.arc(cx as f64, cy as f64, radius.max(0.0) as f64, 0.0, 2.0 * PI);
        let _ = cr.stroke();
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        self.polyline(&[(x0, y0), (x1, y1)], width, c);
    }

    /// Connected line with round joins (icons, chart).
    pub fn polyline(&self, pts: &[(f32, f32)], width: f32, c: Color) {
        let Some(((x0, y0), rest)) = pts.split_first() else {
            return;
        };
        let mut segs = vec![Seg::Move(*x0, *y0)];
        segs.extend(rest.iter().map(|&(x, y)| Seg::Line(x, y)));
        self.stroke_path(&segs, width, c);
    }

    /// Replaces the context's current path with `segs`.
    fn add_path(cr: &cairo::Context, segs: &[Seg]) {
        let f = |v: f32| v as f64;
        cr.new_path();
        for s in segs {
            match *s {
                Seg::Move(x, y) => cr.move_to(f(x), f(y)),
                Seg::Line(x, y) => cr.line_to(f(x), f(y)),
                Seg::Curve(x1, y1, x2, y2, x, y) => cr.curve_to(f(x1), f(y1), f(x2), f(y2), f(x), f(y)),
                Seg::Close => cr.close_path(),
            }
        }
    }

    pub fn fill_path(&self, segs: &[Seg], c: Color) {
        let Some(cr) = self.set_fill(c) else {
            return;
        };
        Self::add_path(cr, segs);
        let _ = cr.fill();
    }

    pub fn stroke_path(&self, segs: &[Seg], width: f32, c: Color) {
        let Some(cr) = self.set_stroke(width, c) else {
            return;
        };
        cr.set_line_join(LineJoin::Round);
        cr.set_line_cap(LineCap::Round);
        Self::add_path(cr, segs);
        let _ = cr.stroke();
    }

    /// Family of the desktop UI font (`gtk-font-name`, e.g. "Ubuntu 11").
    fn family(&mut self) -> String {
        if let Some(f) = &self.family {
            return f.clone();
        }
        let name = gtk::Settings::default().and_then(|s| s.gtk_font_name()).map(|n| n.to_string());
        let family = name.and_then(|n| FontDescription::from_string(&n).family().map(|f| f.to_string())).filter(|f| !f.is_empty());
        let family = family.unwrap_or_else(|| FALLBACK_FAMILY.to_string());
        self.family = Some(family.clone());
        family
    }

    fn font(&mut self, size: Size, weight: Weight) -> FontDescription {
        let mut fd = FontDescription::new();
        fd.set_family(&self.family());
        // Absolute: the type ramp is in DIPs, independent of font DPI.
        fd.set_absolute_size(size.px() as f64 * pango::SCALE as f64);
        fd.set_weight(match weight {
            Weight::Normal => pango::Weight::Normal,
            Weight::Semibold => pango::Weight::Semibold,
            Weight::Bold => pango::Weight::Bold,
        });
        fd
    }

    pub fn text(&mut self, s: &str, r: Rect, size: Size, weight: Weight, align: Align, c: Color) {
        let font = self.font(size, weight);
        let (Some(cr), Some(ctx)) = (self.set_fill(c), self.text.as_ref()) else {
            return;
        };
        let layout = Layout::new(ctx);
        layout.set_font_description(Some(&font));
        layout.set_width(units(r.w.max(0.0)));
        if size.tracking() > 0.0 {
            let attrs = AttrList::new();
            attrs.insert(AttrInt::new_letter_spacing(units(size.tracking())));
            layout.set_attributes(Some(&attrs));
        }
        layout.set_text(s);

        if align == Align::Wrap {
            // Wraps at word boundaries inside the rect, top-left first line.
            layout.set_wrap(WrapMode::Word);
            cr.move_to(r.x as f64, r.y as f64);
            pangocairo::functions::show_layout(cr, &layout);
            return;
        }

        // One line, centered vertically; a line too long ends in "…".
        layout.set_single_paragraph_mode(true);
        layout.set_ellipsize(EllipsizeMode::End);
        layout.set_alignment(match align {
            Align::Center => pango::Alignment::Center,
            Align::Right => pango::Alignment::Right,
            _ => pango::Alignment::Left,
        });
        let (_, logical) = layout.extents();
        let y = r.y + (r.h - dips(logical.height())) / 2.0;
        cr.move_to(r.x as f64, y as f64);
        pangocairo::functions::show_layout(cr, &layout);
    }
}
