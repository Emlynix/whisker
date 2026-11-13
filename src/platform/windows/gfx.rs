//! Drawing driver over Direct2D + DirectWrite (both ship with Windows).
//! Works in device-independent pixels; the render target handles DPI.

use crate::gfx::{Align, Color, Face, Rect, Seg, Size, Weight};
use std::collections::HashMap;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows::core::{Interface, PCWSTR, w};
use windows_numerics::Vector2;

/// The bundled families (`crate::fonts`), and the system ones used when
/// they cannot be loaded (Windows 10 before 1703).
const FONT_UI: PCWSTR = w!("Inter");
const FONT_MONO: PCWSTR = w!("JetBrains Mono");
const FALLBACK_UI: PCWSTR = w!("Segoe UI");
const FALLBACK_MONO: PCWSTR = w!("Consolas");
const LOCALE: PCWSTR = w!("sk-SK");
const BASE_DPI: f32 = 96.0;

fn rect(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.x + r.w, bottom: r.y + r.h }
}

fn color(c: Color) -> D2D1_COLOR_F {
    let ch = |shift: u32| ((c.0 >> shift) & 0xFF) as f32 / 255.0;
    D2D1_COLOR_F { r: ch(16), g: ch(8), b: ch(0), a: c.1 as f32 / 255.0 }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// The bundled fonts as a DirectWrite collection, read straight from
/// the exe's memory; `None` where the system cannot do that.
fn bundled_fonts(dw: &IDWriteFactory) -> Option<IDWriteFontCollection> {
    let dw5 = dw.cast::<IDWriteFactory5>().ok()?;
    unsafe {
        let loader = dw5.CreateInMemoryFontFileLoader().ok()?;
        dw5.RegisterFontFileLoader(&loader).ok()?;
        let builder = dw5.CreateFontSetBuilder().ok()?;
        for (_, data) in crate::fonts::FILES {
            // The bytes are part of the exe: they outlive every use.
            let file = loader.CreateInMemoryFontFileReference(dw, data.as_ptr().cast(), data.len() as u32, None).ok()?;
            builder.AddFontFile(&file).ok()?;
        }
        let set = builder.CreateFontSet().ok()?;
        dw5.CreateFontCollectionFromFontSet(&set).ok()?.cast().ok()
    }
}

/// Bytes of a pixel of the memory target: B, G, R, A.
const PIXEL_BYTES: usize = 4;

/// Memory target of an off-window `Gfx`: a top-down 32-bit bitmap in a
/// memory DC, drawn to through a DC render target.
///
/// ```text
///  Gfx calls -> ID2D1DCRenderTarget -> memory DC -> DIB section `bits`
/// ```
struct Memory {
    rt: ID2D1DCRenderTarget,
    dc: HDC,
    bitmap: HBITMAP,
    /// What the DC held before `bitmap`, put back before it is deleted.
    old: HGDIOBJ,
    bits: *mut u8,
    w: u32,
    h: u32,
    scale: f32,
}

impl Drop for Memory {
    fn drop(&mut self) {
        // SAFETY: the handles were made by `Memory::new` and are freed once.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

impl Memory {
    fn new(d2d: &ID2D1Factory, w: u32, h: u32, scale: f32) -> Option<Memory> {
        // A side GDI cannot hold would make a bitmap of another size.
        let (width, height) = (i32::try_from(w).ok().filter(|v| *v > 0)?, i32::try_from(h).ok().filter(|v| *v > 0)?);
        let header = BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            // Negative: the first row is the top one.
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        };
        let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: BASE_DPI * scale,
            dpiY: BASE_DPI * scale,
            ..Default::default()
        };

        // SAFETY: plain GDI calls; every handle made here is owned by the
        // returned value or freed on the way out.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return None;
            }
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return None;
            };
            let old = SelectObject(dc, bitmap.into());
            let bound = d2d.CreateDCRenderTarget(&props).ok().filter(|rt| {
                let area = RECT { left: 0, top: 0, right: width, bottom: height };
                rt.BindDC(dc, &area).is_ok()
            });
            let Some(rt) = bound else {
                SelectObject(dc, old);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(dc);
                return None;
            };
            // Over a clear ground ClearType would leave color fringes.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Some(Memory { rt, dc, bitmap, old, bits: bits.cast(), w, h, scale })
        }
    }
}

pub struct Gfx {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    /// What the frame is drawn to: the window's target or the memory one.
    rt: Option<ID2D1RenderTarget>,
    /// The window's target, for `resize`.
    window: Option<ID2D1HwndRenderTarget>,
    /// Off-window drawing (`begin_offscreen`).
    memory: Option<Memory>,
    brush: Option<ID2D1SolidColorBrush>,
    formats: HashMap<(Size, Weight, Align), IDWriteTextFormat>,
    /// The bundled fonts; `None` = system fonts.
    fonts: Option<IDWriteFontCollection>,
    /// Round caps and joins for paths and polylines.
    round: Option<ID2D1StrokeStyle>,
    dpi: f32,
}

impl Gfx {
    pub fn new(hwnd: HWND) -> Option<Self> {
        let d2d = unsafe { D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()? };
        let dw = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok()? };
        let fonts = bundled_fonts(&dw);
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: D2D1_CAP_STYLE_ROUND,
            endCap: D2D1_CAP_STYLE_ROUND,
            dashCap: D2D1_CAP_STYLE_ROUND,
            lineJoin: D2D1_LINE_JOIN_ROUND,
            miterLimit: 10.0,
            ..Default::default()
        };
        let round = unsafe { d2d.CreateStrokeStyle(&props, None).ok() };
        Some(Self { hwnd, d2d, dw, rt: None, window: None, memory: None, brush: None, formats: HashMap::new(), fonts, round, dpi: BASE_DPI })
    }

    /// A `Gfx` without a window: frames go to memory (`begin_offscreen`).
    pub fn offscreen() -> Option<Self> {
        Self::new(HWND::default())
    }

    /// Starts a frame of `w` x `h` physical pixels on a clear ground;
    /// `scale` physical pixels make one DIP.
    pub fn begin_offscreen(&mut self, w: u32, h: u32, scale: f32) -> bool {
        let fits = self.memory.as_ref().is_some_and(|m| (m.w, m.h, m.scale) == (w, h, scale));
        if !fits {
            self.rt = None;
            self.brush = None;
            self.memory = Memory::new(&self.d2d, w, h, scale);
        }
        let Some(m) = &self.memory else {
            return false;
        };

        let rt: ID2D1RenderTarget = m.rt.clone().into();
        if self.brush.is_none() {
            self.brush = unsafe { rt.CreateSolidColorBrush(&color(Color::rgb(0)), None).ok() };
        }
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&color(Color(0, 0))));
        }
        self.rt = Some(rt);
        self.brush.is_some()
    }

    /// Ends the frame: its pixels as B, G, R, A bytes with the alpha
    /// multiplied in, rows from the top.
    pub fn end_offscreen(&mut self) -> Option<&[u8]> {
        let rt = self.rt.take()?;
        if unsafe { rt.EndDraw(None, None) }.is_err() {
            self.memory = None;
            self.brush = None;
            return None;
        }
        let m = self.memory.as_ref()?;
        // What GDI still holds back must be in the bitmap before it is read.
        let _ = unsafe { GdiFlush() };
        let len = (m.w as usize).checked_mul(m.h as usize)?.checked_mul(PIXEL_BYTES)?;
        // SAFETY: the DIB section holds `len` bytes and lives as long as
        // `self.memory`, which the returned borrow of `self` keeps in place.
        Some(unsafe { std::slice::from_raw_parts(m.bits, len) })
    }

    /// Frees the render target while the window is hidden; rebuilt on next paint.
    pub fn release(&mut self) {
        self.rt = None;
        self.window = None;
        self.brush = None;
        self.formats.clear();
    }

    pub fn set_dpi(&mut self, dpi: u32) {
        self.dpi = dpi as f32;
        if let Some(rt) = &self.rt {
            unsafe { rt.SetDpi(self.dpi, self.dpi) };
        }
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if let Some(rt) = &self.window {
            let _ = unsafe { rt.Resize(&D2D_SIZE_U { width: w, height: h }) };
        }
    }

    fn ensure_target(&mut self, w: u32, h: u32) -> Option<()> {
        if self.rt.is_some() {
            return Some(());
        }
        // Software rasterizer: a small window doesn't need the GPU driver stack,
        // which alone costs ~40 MB of memory.
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            dpiX: self.dpi,
            dpiY: self.dpi,
            ..Default::default()
        };
        let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: D2D_SIZE_U { width: w, height: h },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let rt = unsafe { self.d2d.CreateHwndRenderTarget(&props, &hprops).ok()? };
        let brush = unsafe { rt.CreateSolidColorBrush(&color(Color::rgb(0)), None).ok()? };
        unsafe { rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE) };

        self.rt = Some(rt.clone().into());
        self.window = Some(rt);
        self.brush = Some(brush);
        Some(())
    }

    /// Starts a frame for a client area of `w` x `h` physical pixels.
    pub fn begin(&mut self, w: u32, h: u32) -> bool {
        if self.ensure_target(w, h).is_none() {
            return false;
        }
        unsafe { self.rt.as_ref().unwrap().BeginDraw() };
        true
    }

    /// Ends the frame; a lost device drops the target so the next frame rebuilds it.
    pub fn end(&mut self) {
        let Some(rt) = &self.rt else {
            return;
        };
        if unsafe { rt.EndDraw(None, None) }.is_err() {
            self.rt = None;
            self.window = None;
            self.brush = None;
        }
    }

    fn paint(&self, c: Color, f: impl FnOnce(&ID2D1RenderTarget, &ID2D1SolidColorBrush)) {
        let (Some(rt), Some(b)) = (&self.rt, &self.brush) else {
            return;
        };
        unsafe { b.SetColor(&color(c)) };
        f(rt, b);
    }

    pub fn clear(&self, c: Color) {
        if let Some(rt) = &self.rt {
            unsafe { rt.Clear(Some(&color(c))) };
        }
    }

    pub fn fill(&self, r: Rect, radius: f32, c: Color) {
        let rr = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
        self.paint(c, |rt, b| unsafe { rt.FillRoundedRectangle(&rr, b) });
    }

    pub fn stroke(&self, r: Rect, radius: f32, width: f32, c: Color) {
        // Inset by half the stroke so the border stays inside `r`.
        let r = r.inset(width / 2.0);
        let rr = D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius };
        self.paint(c, |rt, b| unsafe { rt.DrawRoundedRectangle(&rr, b, width, None) });
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        self.paint(c, |rt, b| unsafe { rt.FillEllipse(&e, b) });
    }

    pub fn ring(&self, cx: f32, cy: f32, radius: f32, width: f32, c: Color) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radius, radiusY: radius };
        self.paint(c, |rt, b| unsafe { rt.DrawEllipse(&e, b, width, None) });
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Color) {
        let (p0, p1) = (Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 });
        self.paint(c, |rt, b| unsafe { rt.DrawLine(p0, p1, b, width, None) });
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

    pub fn fill_path(&self, segs: &[Seg], c: Color) {
        let Some(geo) = self.geometry(segs) else {
            return;
        };
        self.paint(c, |rt, b| unsafe { rt.FillGeometry(&geo, b, None) });
    }

    pub fn stroke_path(&self, segs: &[Seg], width: f32, c: Color) {
        let Some(geo) = self.geometry(segs) else {
            return;
        };
        self.paint(c, |rt, b| unsafe { rt.DrawGeometry(&geo, b, width, self.round.as_ref()) });
    }

    /// Path geometry of `segs`; figures left open are still filled.
    fn geometry(&self, segs: &[Seg]) -> Option<ID2D1PathGeometry> {
        let geo = unsafe { self.d2d.CreatePathGeometry().ok()? };
        let sink = unsafe { geo.Open().ok()? };
        let mut open = false;
        for s in segs {
            match *s {
                Seg::Move(x, y) => {
                    if open {
                        unsafe { sink.EndFigure(D2D1_FIGURE_END_OPEN) };
                    }
                    unsafe { sink.BeginFigure(pt(x, y), D2D1_FIGURE_BEGIN_FILLED) };
                    open = true;
                }
                Seg::Line(x, y) if open => unsafe { sink.AddLine(pt(x, y)) },
                Seg::Curve(x1, y1, x2, y2, x, y) if open => {
                    let b = D2D1_BEZIER_SEGMENT { point1: pt(x1, y1), point2: pt(x2, y2), point3: pt(x, y) };
                    unsafe { sink.AddBezier(&b) };
                }
                Seg::Close if open => {
                    unsafe { sink.EndFigure(D2D1_FIGURE_END_CLOSED) };
                    open = false;
                }
                // A segment before any `Move` has no start point.
                _ => {}
            }
        }
        if open {
            unsafe { sink.EndFigure(D2D1_FIGURE_END_OPEN) };
        }
        unsafe { sink.Close().ok()? };
        Some(geo)
    }

    fn format(&mut self, size: Size, weight: Weight, align: Align) -> Option<IDWriteTextFormat> {
        let key = (size, weight, align);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }

        let w = match weight {
            Weight::Normal => DWRITE_FONT_WEIGHT_NORMAL,
            Weight::Medium => DWRITE_FONT_WEIGHT_MEDIUM,
            Weight::Semibold | Weight::Bold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
        };
        let family = match (size.face(), self.fonts.is_some()) {
            (Face::Ui, true) => FONT_UI,
            (Face::Mono, true) => FONT_MONO,
            (Face::Ui, false) => FALLBACK_UI,
            (Face::Mono, false) => FALLBACK_MONO,
        };
        let f = unsafe {
            self.dw.CreateTextFormat(family, self.fonts.as_ref(), w, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size.px(), LOCALE).ok()?
        };

        let (ta, pa, wrap) = match align {
            Align::Left => (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_WORD_WRAPPING_NO_WRAP),
            Align::Center => (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_WORD_WRAPPING_NO_WRAP),
            Align::Right => (DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_WORD_WRAPPING_NO_WRAP),
            Align::Wrap => (DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_WORD_WRAPPING_WRAP),
        };
        unsafe {
            let _ = f.SetTextAlignment(ta);
            let _ = f.SetParagraphAlignment(pa);
            let _ = f.SetWordWrapping(wrap);
        }
        // A line too long for its box ends in "…" instead of spilling out.
        if align != Align::Wrap {
            let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            if let Ok(sign) = unsafe { self.dw.CreateEllipsisTrimmingSign(&f) } {
                let _ = unsafe { f.SetTrimming(&trim, &sign) };
            }
        }
        self.formats.insert(key, f.clone());
        Some(f)
    }

    pub fn text(&mut self, s: &str, r: Rect, size: Size, weight: Weight, align: Align, c: Color) {
        let Some(f) = self.format(size, weight, align) else {
            return;
        };
        let utf16: Vec<u16> = s.encode_utf16().collect();
        if size.tracking() != 0.0 {
            self.tracked(&utf16, &f, r, size.tracking(), c);
            return;
        }
        let rect = rect(r);
        self.paint(c, |rt, b| unsafe {
            rt.DrawText(&utf16, &f, &rect, b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL)
        });
    }

    /// Letter-spaced text: spacing needs a text layout, `DrawText` has none.
    fn tracked(&self, utf16: &[u16], f: &IDWriteTextFormat, r: Rect, spacing: f32, c: Color) {
        let Ok(layout) = (unsafe { self.dw.CreateTextLayout(utf16, f, r.w.max(0.0), r.h.max(0.0)) }) else {
            return;
        };
        if let Ok(spaced) = layout.cast::<IDWriteTextLayout1>() {
            let all = DWRITE_TEXT_RANGE { startPosition: 0, length: utf16.len() as u32 };
            let _ = unsafe { spaced.SetCharacterSpacing(0.0, spacing, 0.0, all) };
        }
        self.paint(c, |rt, b| unsafe { rt.DrawTextLayout(pt(r.x, r.y), &layout, b, D2D1_DRAW_TEXT_OPTIONS_NONE) });
    }
}
