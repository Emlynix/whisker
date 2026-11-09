//! Generic top-down mouse drawing: Buttons page (with numbered button
//! markers), Overview hero and the detect dialog. Drawn in a 100 x 150
//! unit box, scaled into the target rect.
//!
//! ```text
//!          .--+--.
//!        /  1 |3| 2 \       1 left   2 right   3 wheel
//!       |     |6|    |      6 DPI button behind the wheel
//!      5|----------- |      5 forward, 4 back (left side)
//!      4|            |
//!        \__________/
//! ```
//!
//! Marker numbers are button indices + 1, as listed on the Buttons page.

use super::{Act, Frame, th};
use crate::gfx::{Align, Color, Rect, Seg, Size, Weight};

const UNIT_W: f32 = 100.0;
const UNIT_H: f32 = 150.0;
const OUTLINE: f32 = 1.5;
const MARK_R: f32 = 10.0;

/// Button markers on the drawing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Marks {
    None,
    /// Clickable markers ①–⑥; `lit` is the hovered or focused button.
    Buttons { lit: Option<usize> },
}

/// Marker centers per button index: left, right, middle, back, forward, DPI.
const MARKERS: [(f32, f32); 6] = [(27.0, 30.0), (73.0, 30.0), (50.0, 22.0), (17.0, 96.0), (17.0, 70.0), (50.0, 48.0)];
const MIDDLE: usize = 2;
const BACK: usize = 3;
const FORWARD: usize = 4;
const DPI: usize = 5;

/// Body outline, clockwise from the front center.
const BODY: [Seg; 8] = [
    Seg::Move(50.0, 2.0),
    Seg::Curve(76.0, 2.0, 92.0, 18.0, 94.0, 48.0),
    Seg::Curve(97.0, 80.0, 98.0, 110.0, 90.0, 130.0),
    Seg::Curve(82.0, 146.0, 66.0, 150.0, 50.0, 150.0),
    Seg::Curve(34.0, 150.0, 18.0, 146.0, 10.0, 130.0),
    Seg::Curve(2.0, 110.0, 3.0, 80.0, 6.0, 48.0),
    Seg::Curve(8.0, 18.0, 24.0, 2.0, 50.0, 2.0),
    Seg::Close,
];

/// Left button area; the right one is its mirror image.
const LEFT_KEY: [Seg; 5] = [
    Seg::Move(50.0, 2.0),
    Seg::Curve(24.0, 2.0, 8.0, 18.0, 6.0, 48.0),
    Seg::Line(5.0, 52.0),
    Seg::Curve(15.0, 56.0, 32.5, 58.0, 50.0, 58.0),
    Seg::Close,
];

/// Line between the buttons and the palm rest.
const SPLIT: [Seg; 2] = [Seg::Move(5.0, 52.0), Seg::Curve(25.0, 60.0, 75.0, 60.0, 95.0, 52.0)];

/// Small parts as unit rects: wheel, DPI button, side buttons.
const WHEEL: Rect = Rect::new(44.5, 11.0, 11.0, 24.0);
const DPI_KEY: Rect = Rect::new(45.5, 41.0, 9.0, 12.0);
const FORWARD_KEY: Rect = Rect::new(1.0, 62.0, 7.0, 16.0);
const BACK_KEY: Rect = Rect::new(1.0, 84.0, 7.0, 16.0);

/// Unit box -> screen: scale and offset of the largest fit centered in `r`.
#[derive(Clone, Copy)]
struct Fit {
    k: f32,
    x: f32,
    y: f32,
}

impl Fit {
    fn new(r: Rect) -> Fit {
        let k = (r.w / UNIT_W).min(r.h / UNIT_H);
        Fit { k, x: r.x + (r.w - UNIT_W * k) / 2.0, y: r.y + (r.h - UNIT_H * k) / 2.0 }
    }

    fn pt(self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.k, self.y + y * self.k)
    }

    fn rect(self, r: Rect) -> Rect {
        let (x, y) = self.pt(r.x, r.y);
        Rect::new(x, y, r.w * self.k, r.h * self.k)
    }

    /// Path in screen space; `Mirror::X` flips it around the center line.
    fn path(self, segs: &[Seg], m: Mirror) -> Vec<Seg> {
        let fx = |x: f32| if m == Mirror::X { UNIT_W - x } else { x };
        let p = |x: f32, y: f32| self.pt(fx(x), y);
        segs.iter()
            .map(|s| match *s {
                Seg::Move(x, y) => {
                    let (x, y) = p(x, y);
                    Seg::Move(x, y)
                }
                Seg::Line(x, y) => {
                    let (x, y) = p(x, y);
                    Seg::Line(x, y)
                }
                Seg::Curve(x1, y1, x2, y2, x, y) => {
                    let ((x1, y1), (x2, y2), (x, y)) = (p(x1, y1), p(x2, y2), p(x, y));
                    Seg::Curve(x1, y1, x2, y2, x, y)
                }
                Seg::Close => Seg::Close,
            })
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mirror {
    None,
    X,
}

/// Draws the mouse into `r` (aspect 2:3); markers add hits `EditButton(i)`.
pub(super) fn mouse(f: &mut Frame, r: Rect, marks: Marks) {
    let fit = Fit::new(r);
    let lit = match marks {
        Marks::Buttons { lit } => lit,
        Marks::None => None,
    };
    let (line, fill) = (th().muted, th().card_hi);

    let body = fit.path(&BODY, Mirror::None);
    f.g.fill_path(&body, fill);

    // Lit main button: soft accent over its area.
    for (i, m) in [(0, Mirror::None), (1, Mirror::X)] {
        if lit == Some(i) {
            f.g.fill_path(&fit.path(&LEFT_KEY, m), th().accent_soft);
        }
    }
    f.g.stroke_path(&fit.path(&SPLIT, Mirror::None), OUTLINE, line);
    let (cx, top) = fit.pt(50.0, 2.0);
    let (_, bottom) = fit.pt(50.0, 57.0);
    f.g.line(cx, top, cx, bottom, OUTLINE, line);

    // Small parts sit on top of the outline (side keys stick out of it).
    f.g.stroke_path(&body, OUTLINE, line);
    for (i, part) in [(MIDDLE, WHEEL), (DPI, DPI_KEY), (FORWARD, FORWARD_KEY), (BACK, BACK_KEY)] {
        let pr = fit.rect(part);
        let radius = pr.w.min(pr.h) / 2.0;
        let c = if lit == Some(i) { th().accent } else { fill };
        f.g.fill(pr, radius, c);
        f.g.stroke(pr, radius, OUTLINE, line);
    }

    if let Marks::Buttons { .. } = marks {
        markers(f, fit, lit);
    }
}

/// Numbered markers; each is a click target for its button.
fn markers(f: &mut Frame, fit: Fit, lit: Option<usize>) {
    for (i, &(x, y)) in MARKERS.iter().enumerate() {
        let (cx, cy) = fit.pt(x, y);
        let hit = Rect::new(cx - MARK_R - 2.0, cy - MARK_R - 2.0, 2.0 * MARK_R + 4.0, 2.0 * MARK_R + 4.0);
        f.button(Act::EditButton(i), hit);
        marker(f, cx, cy, i, lit == Some(i));
    }
}

/// Round number badge "1".."6"; accent when its button is lit.
pub(super) fn marker(f: &mut Frame, cx: f32, cy: f32, index: usize, lit: bool) {
    let (bg, fg): (Color, Color) = if lit { (th().accent, th().on_accent) } else { (th().card, th().text) };
    f.g.circle(cx, cy, MARK_R, bg);
    if !lit {
        f.g.ring(cx, cy, MARK_R, 1.0, th().muted);
    }
    let r = Rect::new(cx - MARK_R, cy - MARK_R, 2.0 * MARK_R, 2.0 * MARK_R);
    f.g.text(&(index + 1).to_string(), r, Size::Caption, Weight::Semibold, Align::Center, fg);
}
