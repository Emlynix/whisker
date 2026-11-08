//! Drawing driver over Core Graphics (shapes) and AppKit string drawing
//! (Core Text underneath) into the view's context. The view is flipped,
//! so coordinates are top-left DIPs (points); AppKit scales them by the
//! screen's `backingScaleFactor`, nothing to do here for DPI.

use crate::gfx::{Align, Color, Rect, Weight};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSAttributedStringNSStringDrawing, NSColor, NSFont, NSFontAttributeName, NSFontWeightBold, NSFontWeightRegular, NSFontWeightSemibold, NSForegroundColorAttributeName};
use objc2_core_foundation::{CFRetained, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGContext, CGLineJoin, CGPath};
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};
use std::collections::HashMap;

fn cg_rect(r: Rect) -> CGRect {
    CGRect::new(CGPoint::new(r.x as CGFloat, r.y as CGFloat), CGSize::new(r.w.max(0.0) as CGFloat, r.h.max(0.0) as CGFloat))
}

/// `(r, g, b, a)` in `0.0..=1.0`.
fn channels(c: Color) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    let ch = |shift: u32| ((c.0 >> shift) & 0xFF) as CGFloat / 255.0;
    (ch(16), ch(8), ch(0), c.1 as CGFloat / 255.0)
}

fn cg_color(c: Color) -> CFRetained<CGColor> {
    let (r, g, b, a) = channels(c);
    CGColor::new_srgb(r, g, b, a)
}

pub struct Gfx {
    ctx: Option<CFRetained<CGContext>>,
    width: f32,
    height: f32,
    fonts: HashMap<(u32, Weight), Retained<NSFont>>,
}

impl Gfx {
    pub fn new() -> Self {
        Self { ctx: None, width: 0.0, height: 0.0, fonts: HashMap::new() }
    }

    /// Starts a frame on the context AppKit prepared for `drawRect:`.
    pub fn begin(&mut self, ctx: CFRetained<CGContext>, w: f32, h: f32) {
        (self.width, self.height) = (w, h);
        self.ctx = Some(ctx);
    }

    pub fn end(&mut self) {
        self.ctx = None;
    }

    /// Frees cached fonts while the window is hidden.
    pub fn release(&mut self) {
        self.fonts.clear();
    }

    fn ctx(&self) -> Option<&CGContext> {
        self.ctx.as_deref()
    }

    fn set_fill(&self, c: Color) -> Option<&CGContext> {
        let ctx = self.ctx()?;
        CGContext::set_fill_color_with_color(Some(ctx), Some(&cg_color(c)));
        Some(ctx)
    }

    fn set_stroke(&self, width: f32, c: Color) -> Option<&CGContext> {
        let ctx = self.ctx()?;
        CGContext::set_stroke_color_with_color(Some(ctx), Some(&cg_color(c)));
        CGContext::set_line_width(Some(ctx), width as CGFloat);
        Some(ctx)
    }

    pub fn clear(&self, c: Color) {
        let Some(ctx) = self.set_fill(c) else {
            return;
        };
        CGContext::fill_rect(Some(ctx), cg_rect(Rect::new(0.0, 0.0, self.width, self.height)));
    }

    /// Rounded rect path; the radius is clamped, Core Graphics rejects
    /// corners larger than half a side.
    fn rounded(r: Rect, radius: f32) -> CFRetained<CGPath> {
        let k = radius.min(r.w / 2.0).min(r.h / 2.0).max(0.0) as CGFloat;
        // SAFETY: a null transform means identity.
        unsafe { CGPath::with_rounded_rect(cg_rect(r), k, k, std::ptr::null()) }
    }

    pub fn fill(&self, r: Rect, radius: f32, c: Color) {
        let Some(ctx) = self.set_fill(c) else {
            return;
        };
        CGContext::add_path(Some(ctx), Some(&Self::rounded(r, radius)));
        CGContext::fill_path(Some(ctx));
    }

    pub fn stroke(&self, r: Rect, radius: f32, width: f32, c: Color) {
        let Some(ctx) = self.set_stroke(width, c) else {
            return;
        };
        // Inset by half the stroke so the border stays inside `r`.
        CGContext::add_path(Some(ctx), Some(&Self::rounded(r.inset(width / 2.0), radius)));
        CGContext::stroke_path(Some(ctx));
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        let Some(ctx) = self.set_fill(c) else {
            return;
        };
        let r = Rect::new(cx - radius, cy - radius, 2.0 * radius, 2.0 * radius);
        CGContext::fill_ellipse_in_rect(Some(ctx), cg_rect(r));
    }

    pub fn ring(&self, cx: f32, cy: f32, radius: f32, width: f32, c: Color) {
        let Some(ctx) = self.set_stroke(width, c) else {
            return;
        };
        let r = Rect::new(cx - radius, cy - radius, 2.0 * radius, 2.0 * radius);
        CGContext::stroke_ellipse_in_rect(Some(ctx), cg_rect(r));
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        self.polyline(&[(x0, y0), (x1, y1)], width, c);
    }

    pub fn polyline(&self, pts: &[(f32, f32)], width: f32, c: Color) {
        let Some(((x0, y0), rest)) = pts.split_first() else {
            return;
        };
        let Some(ctx) = self.set_stroke(width, c) else {
            return;
        };
        CGContext::set_line_join(Some(ctx), CGLineJoin::Round);
        CGContext::begin_path(Some(ctx));
        CGContext::move_to_point(Some(ctx), *x0 as CGFloat, *y0 as CGFloat);
        for (x, y) in rest {
            CGContext::add_line_to_point(Some(ctx), *x as CGFloat, *y as CGFloat);
        }
        CGContext::stroke_path(Some(ctx));
    }

    fn font(&mut self, size: f32, weight: Weight) -> Retained<NSFont> {
        let key = ((size * 10.0) as u32, weight);
        if let Some(f) = self.fonts.get(&key) {
            return f.clone();
        }

        // SAFETY: AppKit constants, initialized when the framework loads.
        let w = unsafe {
            match weight {
                Weight::Normal => NSFontWeightRegular,
                Weight::Semibold => NSFontWeightSemibold,
                Weight::Bold => NSFontWeightBold,
            }
        };
        let f = NSFont::systemFontOfSize_weight(size as CGFloat, w);
        self.fonts.insert(key, f.clone());
        f
    }

    pub fn text(&mut self, s: &str, r: Rect, size: f32, weight: Weight, align: Align, c: Color) {
        if self.ctx.is_none() {
            return;
        }
        let font = self.font(size, weight);
        let (red, g, b, a) = channels(c);
        let color = NSColor::colorWithSRGBRed_green_blue_alpha(red, g, b, a);

        // SAFETY: AppKit constants, initialized when the framework loads.
        let keys = unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let values: [&AnyObject; 2] = [font.as_ref(), color.as_ref()];
        let attrs = NSDictionary::from_slices(&keys, &values);
        // SAFETY: the dictionary holds the value types the keys expect.
        let text = unsafe { NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &NSString::from_str(s), Some(&attrs)) };

        if align == Align::Wrap {
            // Wraps at word boundaries inside the rect, top-left first line.
            text.drawInRect(cg_rect(r));
            return;
        }

        // One line, centered vertically; `x` by alignment.
        let ext = text.size();
        let (w, h) = (ext.width as f32, ext.height as f32);
        let x = match align {
            Align::Center => r.x + (r.w - w) / 2.0,
            Align::Right => r.x + r.w - w,
            _ => r.x,
        };
        let y = r.y + (r.h - h) / 2.0;
        text.drawAtPoint(CGPoint::new(x as CGFloat, y as CGFloat));
    }
}
