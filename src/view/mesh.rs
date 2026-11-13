//! The mouse on the Buttons page: a low-poly mesh of a classic
//! right-handed mouse, seen from the front-left above, with its buttons
//! as parts; those the page can set light up.
//!
//! ```text
//!   body   a height field over a rounded footprint, 12 x 16 cells, the
//!          inner vertices nudged so the triangles look hand-made
//!   parts  1 left, 2 right (the shell in front of the seam), 3 wheel
//!          (cylinder), 4 DPI button (box), 5 and 6 side buttons (boxes)
//!   more   what the model's `Layout` adds: further side and top
//!          buttons, a thumb grid, a sniper paddle, a thumb wheel, edge
//!          buttons; parts without a number
//!   view   orthographic, painter-sorted, back faces dropped, flat shading
//! ```
//! Geometry and numbers follow `mesh()` of the design mock-up. Building
//! it is pure and done once; drawing only picks colors.

use super::{Act, Frame, th};
use crate::devices::{Hand, Layout};
use crate::gfx::{Align, Color, Rect, Seg, Size, Weight};

/// The drawing fits a box of this size; callouts end at its sides.
pub const WIDTH: f32 = 220.0;
pub const HEIGHT: f32 = 200.0;

type P3 = [f32; 3];

/// Camera: turned and tilted toward the viewer.
const YAW: f32 = -0.62;
const PITCH: f32 = 0.8;
const SCALE: f32 = 1.45;
const CENTER: (f32, f32) = (112.0, 104.0);
/// Light from the top-front-left.
const LIGHT: P3 = [-0.35, 0.45, 0.82];

/// Footprint half-width, half-length, hump height, base height.
const HALF_W: f32 = 34.0;
const HALF_L: f32 = 58.0;
const HUMP: f32 = 30.0;
const BASE: f32 = 2.0;
/// Grid of the body in (u, v), each in -1..=1.
const NU: usize = 12;
const NV: usize = 16;
/// The seam behind the two main buttons.
const SEAM_V: f32 = -0.05;
const JITTER_SEED: u64 = 5;

/// Mesh regions (the mock-up's part numbers) to the app's button order:
/// left, right, middle, back, forward, DPI. The last region is a part
/// the page has no button for.
const REGION_BUTTON: [Option<usize>; REGIONS] = [None, Some(0), Some(1), Some(2), Some(5), Some(3), Some(4), None];
const REGIONS: usize = 8;
const WHEEL: usize = 3;
const TOP_KEY: usize = 4;
const BACK_KEY: usize = 5;
const FORWARD_KEY: usize = 6;
const OTHER_PART: usize = 7;
/// Parts of one kind that are drawn; a mouse with more shows these.
const SIDE_MAX: u8 = 4;
const TOP_MAX: u8 = 3;
const EDGE_MAX: u8 = 2;
const GRID_MAX: u8 = 12;
/// The flank of the shell, in u: where side parts sit.
const FLANK_U: f32 = 0.93;
/// Buttons whose callout goes to the left side.
const LEFT_CALLOUTS: [usize; 3] = [0, 3, 4];
/// Callout labels keep this distance from each other.
const CALLOUT_STEP: f32 = 18.0;

struct Tri {
    pts: [(f32, f32); 3],
    depth: f32,
    shade: f32,
    button: Option<usize>,
    /// A button or wheel, not the shell.
    part: bool,
}

struct Callout {
    button: usize,
    anchor: (f32, f32),
    /// Height of the label and the level part of the leader.
    y: f32,
    left: bool,
}

pub struct Mesh {
    tris: Vec<Tri>,
    seams: Vec<Vec<(f32, f32)>>,
    callouts: Vec<Callout>,
}

fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: P3, b: P3) -> P3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: P3, b: P3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: P3) -> f32 {
    dot(a, a).sqrt()
}

/// Model point to (screen x, screen y, depth).
fn project(p: P3) -> P3 {
    let (cy, sy, cp, sp) = (YAW.cos(), YAW.sin(), PITCH.cos(), PITCH.sin());
    let x = (p[0] * cy - p[1] * sy) * SCALE;
    let y = (p[0] * sy + p[1] * cy) * SCALE;
    [CENTER.0 + x, CENTER.1 + y * cp - p[2] * SCALE * sp, y * sp + p[2] * SCALE * cp]
}

/// A normal that points toward the camera.
fn facing(n: P3) -> bool {
    let (cy, sy, cp, sp) = (YAW.cos(), YAW.sin(), PITCH.cos(), PITCH.sin());
    (n[0] * sy + n[1] * cy) * sp + n[2] * cp > 0.0
}

/// Half-width of the footprint at `v`: wider at the back, ends rounded
/// by a superellipse.
fn half_width(v: f32) -> f32 {
    HALF_W * (0.82 + 0.18 * (v + 1.0) / 2.0) * (1.0 - v.abs().powi(3)).max(0.0).cbrt()
}

/// The thumb groove: the left side is a tenth narrower around v = 0.15.
fn groove(u: f32, v: f32) -> f32 {
    if u >= 0.0 {
        return 1.0;
    }
    1.0 - 0.1 * (-((v - 0.15) / 0.3).powi(2)).exp()
}

/// Height of the shell: the hump sits left of centre and toward the back.
fn height(u: f32, v: f32) -> f32 {
    let up = if u < 0.0 { (u + 0.12) / 0.88 } else { (u + 0.12) / 1.12 };
    let vp = (v - 0.22) / 1.22;
    BASE + HUMP * (1.0 - vp * vp).max(0.0).powf(0.55) * (1.0 - up * up).max(0.0).powf(0.32)
}

fn shell(u: f32, v: f32) -> P3 {
    [u * half_width(v) * groove(u, v), v * HALF_L, height(u, v)]
}

/// Collects triangles that face the camera.
struct Builder {
    tris: Vec<Tri>,
    /// Sum of the screen centres of each region's triangles, and their count.
    centers: [(f32, f32, f32); REGIONS],
    light: P3,
}

/// The face of a box that is left out: it sits against the shell.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    None,
    /// +x: a part on the left flank.
    Px,
    /// -x: a part on the right flank.
    Nx,
}

/// The axis a cylinder turns around.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// The scroll wheel.
    X,
    /// A thumb wheel.
    Z,
}

/// Which flank of the shell.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Side {
    /// Sign of x on this side.
    fn sign(self) -> f32 {
        if self == Side::Left { -1.0 } else { 1.0 }
    }

    fn open(self) -> Open {
        if self == Side::Left { Open::Px } else { Open::Nx }
    }
}

impl Builder {
    /// `inside` is a point within the solid: normals point away from it.
    fn tri(&mut self, a: P3, b: P3, c: P3, region: usize, inside: P3) {
        let mut n = cross(sub(b, a), sub(c, a));
        let nl = len(n);
        if nl < 1e-6 {
            return;
        }
        if dot(n, sub(a, inside)) < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        if !facing(n) {
            return;
        }
        let shade = (dot(n, self.light) / nl).max(0.0);
        let (pa, pb, pc) = (project(a), project(b), project(c));
        if region != 0 {
            let k = &mut self.centers[region];
            (k.0, k.1, k.2) = (k.0 + (pa[0] + pb[0] + pc[0]) / 3.0, k.1 + (pa[1] + pb[1] + pc[1]) / 3.0, k.2 + 1.0);
        }
        let depth = (pa[2] + pb[2] + pc[2]) / 3.0;
        self.tris.push(Tri { pts: [(pa[0], pa[1]), (pb[0], pb[1]), (pc[0], pc[1])], depth, shade, button: REGION_BUTTON[region], part: region != 0 });
    }

    fn quad(&mut self, q: [P3; 4], region: usize, inside: P3) {
        self.tri(q[0], q[1], q[2], region, inside);
        self.tri(q[0], q[2], q[3], region, inside);
    }

    /// A box around `c` with half-sizes `h`, without its `open` face.
    fn block(&mut self, c: P3, h: P3, region: usize, open: Open) {
        let v = |dx: f32, dy: f32, dz: f32| [c[0] + dx * h[0], c[1] + dy * h[1], c[2] + dz * h[2]];
        let top = [v(-1.0, -1.0, 1.0), v(1.0, -1.0, 1.0), v(1.0, 1.0, 1.0), v(-1.0, 1.0, 1.0)];
        let nx = [v(-1.0, -1.0, -1.0), v(-1.0, 1.0, -1.0), v(-1.0, 1.0, 1.0), v(-1.0, -1.0, 1.0)];
        let px = [v(1.0, -1.0, -1.0), v(1.0, -1.0, 1.0), v(1.0, 1.0, 1.0), v(1.0, 1.0, -1.0)];
        let ny = [v(-1.0, -1.0, -1.0), v(-1.0, -1.0, 1.0), v(1.0, -1.0, 1.0), v(1.0, -1.0, -1.0)];
        let py = [v(-1.0, 1.0, -1.0), v(1.0, 1.0, -1.0), v(1.0, 1.0, 1.0), v(-1.0, 1.0, 1.0)];
        for face in [top, ny, py] {
            self.quad(face, region, c);
        }
        if open != Open::Nx {
            self.quad(nx, region, c);
        }
        if open != Open::Px {
            self.quad(px, region, c);
        }
    }

    /// A cylinder around `c` turning on `axis`: a wheel.
    fn cylinder(&mut self, c: P3, radius: f32, half: f32, axis: Axis, region: usize, sides: usize) {
        let p = |a: f32, w: f32| match axis {
            Axis::X => [c[0] + w, c[1] + radius * a.cos(), c[2] + radius * a.sin()],
            Axis::Z => [c[0] + radius * a.cos(), c[1] + radius * a.sin(), c[2] + w],
        };
        let hub = |w: f32| if axis == Axis::X { [c[0] + w, c[1], c[2]] } else { [c[0], c[1], c[2] + w] };
        for k in 0..sides {
            let a0 = std::f32::consts::TAU * k as f32 / sides as f32;
            let a1 = std::f32::consts::TAU * (k + 1) as f32 / sides as f32;
            self.quad([p(a0, -half), p(a0, half), p(a1, half), p(a1, -half)], region, c);
            for w in [-half, half] {
                self.tri(hub(w), p(a0, w), p(a1, w), region, c);
            }
        }
    }

    /// `count` side buttons in a row along one flank, front to back.
    ///
    /// ```text
    ///   v: -0.21 ........ 0.31     two: centres -0.08 and 0.18
    ///      [ forward ][ back ]
    /// ```
    fn side_keys(&mut self, side: Side, count: u8) {
        const FROM_V: f32 = -0.21;
        const SPAN_V: f32 = 0.52;
        const GAP: f32 = 1.54;
        let n = count.min(SIDE_MAX);
        let slot = SPAN_V / n.max(1) as f32;
        for k in 0..n {
            let q = shell(side.sign() * FLANK_U, FROM_V + slot * (k as f32 + 0.5));
            // The page's Forward and Back are the first two on the left.
            let region = match (side, k) {
                (Side::Left, 0) => FORWARD_KEY,
                (Side::Left, 1) => BACK_KEY,
                _ => OTHER_PART,
            };
            self.block([q[0] + side.sign(), q[1], q[2]], [1.2, slot * HALF_L / 2.0 - GAP, 1.8], region, side.open());
        }
    }

    /// The buttons behind the wheel; returns the v where they start and
    /// end (the middle seam leaves that out).
    fn top_keys(&mut self, count: u8) -> Option<(f32, f32)> {
        const ONE_V: f32 = -0.25;
        const FROM_V: f32 = -0.37;
        const SPAN_V: f32 = 0.3;
        let n = count.min(TOP_MAX);
        if n == 0 {
            return None;
        }
        let slot = if n == 1 { 0.12 } else { SPAN_V / n as f32 };
        let first = if n == 1 { ONE_V } else { FROM_V + slot / 2.0 };
        for k in 0..n {
            let q = shell(0.0, first + slot * k as f32);
            let region = if k == 0 { TOP_KEY } else { OTHER_PART };
            self.block([0.0, q[1], q[2] + 0.4], [2.4, slot * HALF_L / 2.0 - 0.5, 1.2], region, Open::None);
        }
        Some((first - slot / 2.0, first + slot * (n as f32 - 0.5)))
    }

    /// A thumb grid on a flank: rows of small buttons, top row first.
    ///
    /// ```text
    ///   12 -> 3 rows of 4      7, 8 -> 2 rows of 4      6 -> 2 rows of 3
    /// ```
    fn grid_keys(&mut self, side: Side, count: u8) {
        const FROM_V: f32 = -0.2;
        const SPAN_V: f32 = 0.56;
        /// Rows go down the flank: toward the edge of the footprint.
        const ROW_U: [f32; 3] = [0.9, 0.955, 0.99];
        const LOWEST_Z: f32 = 3.2;
        let count = count.min(GRID_MAX);
        let rows: u8 = if count > 8 { 3 } else { 2 };
        let cols = count.div_ceil(rows).max(1);
        let slot = SPAN_V / cols as f32;
        for k in 0..count.min(rows * cols) {
            let (row, col) = ((k / cols) as usize, k % cols);
            let q = shell(side.sign() * ROW_U[row], FROM_V + slot * (col as f32 + 0.5));
            self.block([q[0] + 0.8 * side.sign(), q[1], q[2].max(LOWEST_Z)], [1.0, slot * HALF_L / 2.0 - 0.8, 1.5], OTHER_PART, side.open());
        }
    }
}

/// The body's grid in (u, v), its inner points nudged by up to a quarter
/// of a cell; points near the seam are pulled onto it.
fn grid() -> Vec<Vec<(f32, f32)>> {
    const MODULUS: u64 = 2_147_483_647;
    let mut seed = JITTER_SEED;
    // Lehmer generator, as in the mock-up: the same mesh every time.
    let mut rnd = || {
        seed = seed * 16_807 % MODULUS;
        seed as f32 / MODULUS as f32
    };
    let mut rows = Vec::new();
    for j in 0..=NV {
        let mut row = Vec::new();
        for i in 0..=NU {
            let mut u = -1.0 + 2.0 * i as f32 / NU as f32;
            let mut v = -1.0 + 2.0 * j as f32 / NV as f32;
            if i > 0 && i < NU {
                u += (rnd() - 0.5) * 0.5 / NU as f32 * 2.0;
            }
            if j > 0 && j < NV {
                v += (rnd() - 0.5) * 0.5 / NV as f32 * 2.0;
            }
            if (v - SEAM_V).abs() < 0.07 {
                v = SEAM_V;
            }
            row.push((u, v));
        }
        rows.push(row);
    }
    rows
}

/// A line on the shell, lifted a little so it is not buried in it.
fn seam(from: (f32, f32), to: (f32, f32), steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|k| {
            let t = k as f32 / steps as f32;
            let p = shell(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            let q = project([p[0], p[1], p[2] + 0.3]);
            (q[0], q[1])
        })
        .collect()
}

/// Builds the mesh of a mouse with the buttons of `layout`.
pub fn build(layout: &Layout) -> Mesh {
    let light = {
        let l = len(LIGHT);
        [LIGHT[0] / l, LIGHT[1] / l, LIGHT[2] / l]
    };
    let mut b = Builder { tris: Vec::new(), centers: [(0.0, 0.0, 0.0); REGIONS], light };
    let origin = [0.0, 0.0, 0.0];
    let g = grid();

    // The shell: two triangles per cell, the diagonal alternating.
    for j in 0..NV {
        for i in 0..NU {
            let (a, bb, c, d) = (g[j][i], g[j][i + 1], g[j + 1][i + 1], g[j + 1][i]);
            let mid = ((a.0 + c.0) / 2.0, (a.1 + c.1) / 2.0);
            let region = if mid.1 >= SEAM_V { 0 } else if mid.0 < 0.0 { 1 } else { 2 };
            let (pa, pb, pc, pd) = (shell(a.0, a.1), shell(bb.0, bb.1), shell(c.0, c.1), shell(d.0, d.1));
            if (i + j) % 2 == 0 {
                b.tri(pa, pb, pc, region, origin);
                b.tri(pa, pc, pd, region, origin);
            } else {
                b.tri(pa, pb, pd, region, origin);
                b.tri(pb, pc, pd, region, origin);
            }
        }
    }
    // A thin skirt down to the desk along both sides.
    for j in 0..NV {
        for i in [0, NU] {
            let (a, c) = (shell(g[j][i].0, g[j][i].1), shell(g[j + 1][i].0, g[j + 1][i].1));
            b.quad([a, c, [c[0] * 0.97, c[1] * 0.99, 0.0], [a[0] * 0.97, a[1] * 0.99, 0.0]], 0, origin);
        }
    }

    // The wheel, the buttons behind it, those on the flanks.
    const WHEEL_END_V: f32 = -0.4;
    let wheel = shell(0.0, -0.5);
    b.cylinder([0.0, wheel[1], wheel[2] + 0.8], 5.5, 2.8, Axis::X, WHEEL, 12);
    let top = b.top_keys(layout.top);

    // What the thumb has more: on the right for a left-handed mouse. A
    // grid takes the whole flank: no plain side buttons beside it.
    let thumb = if layout.hand == Hand::Left { Side::Right } else { Side::Left };
    for (side, count) in [(Side::Left, layout.left), (Side::Right, layout.right)] {
        if layout.grid == 0 || side != thumb {
            b.side_keys(side, count);
        }
    }
    b.grid_keys(thumb, layout.grid);
    if layout.sniper {
        let q = shell(thumb.sign() * FLANK_U, -0.31);
        b.block([q[0] + thumb.sign(), q[1], q[2] - 1.0], [1.2, 3.6, 1.6], OTHER_PART, thumb.open());
    }
    if layout.thumb_wheel {
        let q = shell(thumb.sign() * FLANK_U, 0.44);
        b.cylinder([q[0] + 0.6 * thumb.sign(), q[1], q[2] - 1.0], 4.2, 1.6, Axis::Z, OTHER_PART, 10);
    }
    // Edge buttons: on the left main button, by its outer edge.
    for k in 0..layout.edge.min(EDGE_MAX) {
        let q = shell(-0.74, -0.5 + 0.2 * k as f32);
        b.block([q[0], q[1], q[2] + 0.3], [1.8, 4.2, 1.0], OTHER_PART, Open::None);
    }
    // Painter's order: far first.
    b.tris.sort_by(|x, y| x.depth.total_cmp(&y.depth));

    // Seams: across behind the buttons, and along the middle with gaps
    // for the wheel and the buttons behind it.
    let mut seams = vec![seam((-0.98, SEAM_V), (0.98, SEAM_V), 24), seam((0.0, -0.99), (0.0, -0.6), 4)];
    let (keys_from, keys_to) = top.unwrap_or((SEAM_V, SEAM_V));
    for (from, to) in [(WHEEL_END_V, keys_from), (keys_to, SEAM_V)] {
        if to - from > 0.02 {
            seams.push(seam((0.0, from), (0.0, to), 2));
        }
    }

    Mesh { callouts: callouts(&b.centers), tris: b.tris, seams }
}

/// One callout per button, from the middle of its part to a number at
/// the side; labels of one side keep their distance.
fn callouts(centers: &[(f32, f32, f32); REGIONS]) -> Vec<Callout> {
    let mut anchors: Vec<(usize, (f32, f32))> =
        (1..centers.len()).filter(|&r| centers[r].2 > 0.0).filter_map(|r| REGION_BUTTON[r].map(|b| (b, (centers[r].0 / centers[r].2, centers[r].1 / centers[r].2)))).collect();
    anchors.sort_by(|a, b| a.1.1.total_cmp(&b.1.1));

    let mut out = Vec::new();
    for left in [true, false] {
        let mut last = f32::MIN;
        for &(button, anchor) in anchors.iter().filter(|(b, _)| LEFT_CALLOUTS.contains(b) == left) {
            let y = anchor.1.max(last + CALLOUT_STEP);
            last = y;
            out.push(Callout { button, anchor, y, left });
        }
    }
    out
}

/// A level line from `x0` to `x1` in 2 px dashes.
fn dashed(f: &mut Frame, x0: f32, x1: f32, y: f32, c: Color) {
    const DASH: f32 = 2.0;
    let (from, to) = (x0.min(x1), x0.max(x1));
    let mut x = from;
    while x < to {
        f.g.fill(Rect::new(x, y - 0.5, DASH.min(to - x), 1.0), 0.0, c);
        x += 2.0 * DASH;
    }
}

/// Draws the mesh with its top-left corner at (`x`, `y`); `lit` is the
/// button to highlight. The numbers at the sides are click targets.
pub fn draw(f: &mut Frame, mesh: &Mesh, x: f32, y: f32, lit: Option<usize>) {
    const LEFT_END: f32 = 18.0;
    const RIGHT_END: f32 = 202.0;
    const ELBOW: f32 = 10.0;
    let at = |p: (f32, f32)| (x + p.0, y + p.1);
    let alpha = |a: f32| (a.clamp(0.0, 1.0) * 255.0) as u8;

    for t in &mesh.tris {
        let pts = t.pts.map(at);
        let path = [Seg::Move(pts[0].0, pts[0].1), Seg::Line(pts[1].0, pts[1].1), Seg::Line(pts[2].0, pts[2].1), Seg::Close];
        // The lit part in the accent; the rest in the text color, the
        // button parts a shade stronger.
        if t.button.is_some() && t.button == lit {
            f.g.fill_path(&path, Color(th().acc_fill.0, alpha(0.3 + 0.55 * t.shade)));
            f.g.stroke_path(&path, 0.6, Color(th().acc_fill.0, alpha(0.95)));
            continue;
        }
        let part = t.part;
        f.g.fill_path(&path, Color(th().text.0, alpha(if part { 0.08 } else { 0.04 } + 0.42 * t.shade * t.shade)));
        f.g.stroke_path(&path, 0.55, Color(th().text.0, alpha(if part { 0.5 } else { 0.32 })));
    }
    for s in &mesh.seams {
        let pts: Vec<(f32, f32)> = s.iter().map(|&p| at(p)).collect();
        f.g.polyline(&pts, 1.2, Color(th().text.0, alpha(0.9)));
    }

    for c in &mesh.callouts {
        let on = lit == Some(c.button);
        let ink = if on { th().acc_fill } else { th().muted };
        let (ax, ay) = at(c.anchor);
        let level = y + c.y;
        let (elbow, end) = if c.left { (ax - ELBOW, x + LEFT_END) } else { (ax + ELBOW, x + RIGHT_END) };
        f.g.line(ax, ay, elbow, level, if on { 1.0 } else { 0.7 }, ink);
        if on {
            f.g.fill(Rect::new(elbow.min(end), level - 0.5, (elbow - end).abs(), 1.0), 0.0, ink);
        } else {
            dashed(f, elbow, end, level, ink);
        }
        f.g.fill(Rect::new(ax - 1.5, ay - 1.5, 3.0, 3.0), 0.0, if on { th().acc_fill } else { th().panel });
        f.g.stroke(Rect::new(ax - 1.5, ay - 1.5, 3.0, 3.0), 0.0, 1.0, ink);

        // The number: also a way to pick the button.
        let label = if c.left { Rect::new(x, level - 9.0, LEFT_END - 4.0, 18.0) } else { Rect::new(x + RIGHT_END + 4.0, level - 9.0, WIDTH - RIGHT_END - 4.0, 18.0) };
        f.button(Act::EditButton(c.button), Rect::new(label.x - 2.0, label.y, label.w + 4.0, label.h));
        let align = if c.left { Align::Right } else { Align::Left };
        f.g.text(&(c.button + 1).to_string(), label, Size::Key, Weight::Semibold, align, if on { th().acc } else { th().text });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_is_the_same_every_time_and_has_all_six_buttons() {
        let (a, b) = (build(&Layout::GENERIC), build(&Layout::GENERIC));
        assert_eq!(a.tris.len(), b.tris.len());
        assert!(a.tris.len() > 300, "{} triangles", a.tris.len());
        for button in 0..6 {
            assert!(a.tris.iter().any(|t| t.button == Some(button)), "button {button} has no triangles");
            assert_eq!(a.callouts.iter().filter(|c| c.button == button).count(), 1);
        }
    }

    #[test]
    fn drawing_fits_its_box_and_labels_keep_apart() {
        let m = build(&Layout::GENERIC);
        for t in &m.tris {
            for (x, y) in t.pts {
                assert!((0.0..=WIDTH).contains(&x) && (0.0..=HEIGHT).contains(&y), "({x}, {y})");
            }
        }
        for left in [true, false] {
            let mut ys: Vec<f32> = m.callouts.iter().filter(|c| c.left == left).map(|c| c.y).collect();
            ys.sort_by(f32::total_cmp);
            assert_eq!(ys.len(), 3);
            assert!(ys.windows(2).all(|w| w[1] - w[0] >= CALLOUT_STEP - 0.01));
        }
    }

    #[test]
    fn far_triangles_are_drawn_first() {
        let m = build(&Layout::GENERIC);
        assert!(m.tris.windows(2).all(|w| w[0].depth <= w[1].depth));
    }

    fn parts(l: &Layout) -> usize {
        build(l).tris.iter().filter(|t| t.part).count()
    }

    #[test]
    fn every_part_of_a_layout_is_drawn_inside_the_box() {
        let plain = Layout::PLAIN;
        let all = Layout { left: 255, right: 255, top: 255, edge: 255, grid: 255, tilt: true, thumb_wheel: true, sniper: true, hand: Hand::Right };
        for t in &build(&all).tris {
            for (x, y) in t.pts {
                assert!((0.0..=WIDTH).contains(&x) && (0.0..=HEIGHT).contains(&y), "({x}, {y})");
            }
        }

        // Each kind of part adds triangles to the plain mouse.
        let more = [
            Layout { left: 1, ..plain },
            Layout { top: 1, ..plain },
            Layout { edge: 1, ..plain },
            Layout { grid: 6, ..plain },
            Layout { thumb_wheel: true, ..plain },
            Layout { sniper: true, ..plain },
        ];
        for l in more {
            assert!(parts(&l) > parts(&plain), "{l:?}");
        }
        assert!(parts(&Layout { left: 4, ..plain }) > parts(&Layout { left: 2, ..plain }));
        assert!(parts(&Layout { grid: 12, ..plain }) > parts(&Layout { grid: 6, ..plain }));

        // No side buttons: no callouts for Back and Forward.
        assert_eq!(build(&plain).callouts.len(), 3);
        assert_eq!(build(&Layout::GENERIC).callouts.len(), 6);
    }
}
