//! Drawing driver over Direct2D + DirectWrite (both ship with Windows).
//! Works in device-independent pixels; the render target handles DPI.

use std::collections::HashMap;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::core::w;
use windows_numerics::Vector2;

const FONT: windows::core::PCWSTR = w!("Segoe UI");
const LOCALE: windows::core::PCWSTR = w!("sk-SK");
const BASE_DPI: f32 = 96.0;

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

    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

/// `0xRRGGBB` with alpha `0..=255`.
#[derive(Clone, Copy, PartialEq)]
pub struct Color(pub u32, pub u8);

impl Color {
    pub const fn rgb(v: u32) -> Self {
        Color(v, 255)
    }

    fn d2d(self) -> D2D1_COLOR_F {
        let c = |shift: u32| ((self.0 >> shift) & 0xFF) as f32 / 255.0;
        D2D1_COLOR_F { r: c(16), g: c(8), b: c(0), a: self.1 as f32 / 255.0 }
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

pub struct Gfx {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    formats: HashMap<(u32, Weight, Align), IDWriteTextFormat>,
    dpi: f32,
}

impl Gfx {
    pub fn new(hwnd: HWND) -> Option<Self> {
        let d2d = unsafe { D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()? };
        let dw = unsafe { DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok()? };
        Some(Self { hwnd, d2d, dw, rt: None, brush: None, formats: HashMap::new(), dpi: BASE_DPI })
    }

    /// Frees the render target while the window is hidden; rebuilt on next paint.
    pub fn release(&mut self) {
        self.rt = None;
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
        if let Some(rt) = &self.rt {
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
        let brush = unsafe { rt.CreateSolidColorBrush(&Color::rgb(0).d2d(), None).ok()? };
        unsafe { rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_CLEARTYPE) };

        self.rt = Some(rt);
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
            self.brush = None;
        }
    }

    fn paint(&self, c: Color, f: impl FnOnce(&ID2D1HwndRenderTarget, &ID2D1SolidColorBrush)) {
        let (Some(rt), Some(b)) = (&self.rt, &self.brush) else {
            return;
        };
        unsafe { b.SetColor(&c.d2d()) };
        f(rt, b);
    }

    pub fn clear(&self, c: Color) {
        if let Some(rt) = &self.rt {
            unsafe { rt.Clear(Some(&c.d2d())) };
        }
    }

    pub fn fill(&self, r: Rect, radius: f32, c: Color) {
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        self.paint(c, |rt, b| unsafe { rt.FillRoundedRectangle(&rr, b) });
    }

    pub fn stroke(&self, r: Rect, radius: f32, width: f32, c: Color) {
        // Inset by half the stroke so the border stays inside `r`.
        let r = r.inset(width / 2.0);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
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

    pub fn polyline(&self, pts: &[(f32, f32)], width: f32, c: Color) {
        for w in pts.windows(2) {
            self.line(w[0].0, w[0].1, w[1].0, w[1].1, width, c);
        }
    }

    fn format(&mut self, size: f32, weight: Weight, align: Align) -> Option<IDWriteTextFormat> {
        let key = ((size * 10.0) as u32, weight, align);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }

        let w = match weight {
            Weight::Normal => DWRITE_FONT_WEIGHT_NORMAL,
            Weight::Semibold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
            Weight::Bold => DWRITE_FONT_WEIGHT_BOLD,
        };
        let f = unsafe { self.dw.CreateTextFormat(FONT, None, w, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, LOCALE).ok()? };

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
        self.formats.insert(key, f.clone());
        Some(f)
    }

    pub fn text(&mut self, s: &str, r: Rect, size: f32, weight: Weight, align: Align, c: Color) {
        let Some(f) = self.format(size, weight, align) else {
            return;
        };
        let utf16: Vec<u16> = s.encode_utf16().collect();
        let rect = r.d2d();
        self.paint(c, |rt, b| unsafe {
            rt.DrawText(&utf16, &f, &rect, b, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL)
        });
    }
}
