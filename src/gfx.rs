//! Direct2D and DirectWrite under a small drawing vocabulary. DirectWrite does the Hebrew: right-to-left,
//! punctuation and mixed digits come out right, at any DPI. Everything is in DIPs.

use std::cell::RefCell;
use std::collections::HashMap;
use windows::core::{Interface, Result, HSTRING};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const fn rgb(hex: u32) -> Color {
    Color { r: ((hex >> 16) & 255) as f32 / 255.0, g: ((hex >> 8) & 255) as f32 / 255.0, b: (hex & 255) as f32 / 255.0, a: 1.0 }
}

impl Color {
    pub const fn with(self, a: f32) -> Color {
        Color { r: self.r, g: self.g, b: self.b, a }
    }

    pub fn mix(self, o: Color, t: f32) -> Color {
        Color { r: self.r + (o.r - self.r) * t, g: self.g + (o.g - self.g) * t, b: self.b + (o.b - self.b) * t, a: self.a + (o.a - self.a) * t }
    }

    pub fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.r, g: self.g, b: self.b, a: self.a }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const fn r(x: f32, y: f32, w: f32, h: f32) -> R {
    R { x, y, w, h }
}

impl R {
    pub fn right(self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }
    pub fn cx(self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn inset(self, d: f32) -> R {
        r(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    pub fn inset_xy(self, dx: f32, dy: f32) -> R {
        r(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    pub fn contains(self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
    pub fn top_part(self, h: f32) -> R {
        r(self.x, self.y, self.w, h.min(self.h))
    }
    pub fn bottom_part(self, h: f32) -> R {
        r(self.x, self.bottom() - h.min(self.h), self.w, h.min(self.h))
    }
    pub fn left_part(self, w: f32) -> R {
        r(self.x, self.y, w.min(self.w), self.h)
    }
    pub fn right_part(self, w: f32) -> R {
        r(self.right() - w.min(self.w), self.y, w.min(self.w), self.h)
    }
    /// what is left below a slice taken from the top
    pub fn below(self, h: f32) -> R {
        r(self.x, self.y + h, self.w, (self.h - h).max(0.0))
    }
    pub fn above(self, h: f32) -> R {
        r(self.x, self.y, self.w, (self.h - h).max(0.0))
    }
    pub fn d2d(self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.x + self.w, bottom: self.y + self.h }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Align {
    /// the reading direction's start: the right edge
    Start,
    Center,
    /// the left edge
    End,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Face {
    Ui,
    Num,
}

#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub face: Face,
    pub size: f32,
    pub weight: u16,
    pub align: Align,
    pub middle: bool,
    pub wrap: bool,
}

pub const fn st(size: f32, weight: u16) -> Style {
    Style { face: Face::Ui, size, weight, align: Align::Start, middle: false, wrap: false }
}

impl Style {
    pub const fn center(mut self) -> Style {
        self.align = Align::Center;
        self
    }
    pub const fn end(mut self) -> Style {
        self.align = Align::End;
        self
    }
    pub const fn middle(mut self) -> Style {
        self.middle = true;
        self
    }
    pub const fn wrap(mut self) -> Style {
        self.wrap = true;
        self
    }
    pub const fn num(mut self) -> Style {
        self.face = Face::Num;
        self
    }
}

#[derive(PartialEq, Eq, Hash, Clone)]
struct FormatKey {
    face: Face,
    size10: u32,
    weight: u16,
    align: Align,
    middle: bool,
    wrap: bool,
}

#[derive(PartialEq, Eq, Hash, Clone)]
struct LayoutKey {
    format: FormatKey,
    w: u32,
    h: u32,
    text: String,
}

pub struct Gfx {
    pub d2d: ID2D1Factory,
    pub dw: IDWriteFactory,
    pub ui_family: HSTRING,
    pub num_family: HSTRING,
    pub round_caps: ID2D1StrokeStyle,
    pub dashed: ID2D1StrokeStyle,
    formats: RefCell<HashMap<FormatKey, IDWriteTextFormat>>,
    layouts: RefCell<HashMap<LayoutKey, IDWriteTextLayout>>,
}

fn font_exists(dw: &IDWriteFactory, name: &str) -> bool {
    unsafe {
        let mut fonts: Option<IDWriteFontCollection> = None;
        if dw.GetSystemFontCollection(&mut fonts, false).is_err() {
            return false;
        }
        let Some(fonts) = fonts else { return false };
        let mut index = 0u32;
        let mut exists = windows::core::BOOL(0);
        fonts.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists).is_ok() && exists.as_bool()
    }
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let round = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            let round_caps = d2d.CreateStrokeStyle(&round, None)?;
            let dash = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_FLAT,
                endCap: D2D1_CAP_STYLE_FLAT,
                dashCap: D2D1_CAP_STYLE_FLAT,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_DASH,
                dashOffset: 0.0,
            };
            let dashed = d2d.CreateStrokeStyle(&dash, None)?;
            // Rubik reads best at alert sizes where it is installed; Segoe UI is on every Windows
            let ui = if font_exists(&dw, "Rubik") { "Rubik" } else { "Segoe UI" };
            let num = if font_exists(&dw, "Bahnschrift") { "Bahnschrift" } else { ui };
            Ok(Gfx {
                d2d,
                dw,
                ui_family: HSTRING::from(ui),
                num_family: HSTRING::from(num),
                round_caps,
                dashed,
                formats: RefCell::new(HashMap::new()),
                layouts: RefCell::new(HashMap::new()),
            })
        }
    }

    fn format(&self, key: &FormatKey) -> Result<IDWriteTextFormat> {
        if let Some(f) = self.formats.borrow().get(key) {
            return Ok(f.clone());
        }
        unsafe {
            let family = match key.face {
                Face::Ui => &self.ui_family,
                Face::Num => &self.num_family,
            };
            let f = self.dw.CreateTextFormat(
                family,
                None,
                DWRITE_FONT_WEIGHT(key.weight as i32),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                key.size10 as f32 / 10.0,
                &HSTRING::from("he-IL"),
            )?;
            f.SetReadingDirection(DWRITE_READING_DIRECTION_RIGHT_TO_LEFT)?;
            f.SetTextAlignment(match key.align {
                Align::Start => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::End => DWRITE_TEXT_ALIGNMENT_TRAILING,
            })?;
            f.SetParagraphAlignment(if key.middle { DWRITE_PARAGRAPH_ALIGNMENT_CENTER } else { DWRITE_PARAGRAPH_ALIGNMENT_NEAR })?;
            f.SetWordWrapping(if key.wrap { DWRITE_WORD_WRAPPING_WRAP } else { DWRITE_WORD_WRAPPING_NO_WRAP })?;
            if !key.wrap {
                // (each format its own sign: one made from the digits' font draws boxes in Hebrew text)
                let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                let sign = self.dw.CreateEllipsisTrimmingSign(&f)?;
                f.SetTrimming(&trimming, &sign)?;
            }
            self.formats.borrow_mut().insert(key.clone(), f.clone());
            Ok(f)
        }
    }

    pub fn layout(&self, text: &str, w: f32, h: f32, s: Style) -> Result<IDWriteTextLayout> {
        let format = FormatKey { face: s.face, size10: (s.size * 10.0) as u32, weight: s.weight, align: s.align, middle: s.middle, wrap: s.wrap };
        let key = LayoutKey { format, w: w.max(1.0).round() as u32, h: h.max(1.0).round() as u32, text: text.to_string() };
        if let Some(l) = self.layouts.borrow().get(&key) {
            return Ok(l.clone());
        }
        let f = self.format(&key.format)?;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let l = unsafe { self.dw.CreateTextLayout(&wide, &f, key.w as f32, key.h as f32)? };
        let mut map = self.layouts.borrow_mut();
        if map.len() > 6000 {
            map.clear();
        }
        map.insert(key, l.clone());
        Ok(l)
    }

    /// (width, height) the text really takes
    pub fn measure(&self, text: &str, max_w: f32, s: Style) -> (f32, f32) {
        let Ok(l) = self.layout(text, max_w, 10000.0, s) else { return (0.0, 0.0) };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            if l.GetMetrics(&mut m).is_err() {
                return (0.0, 0.0);
            }
        }
        (m.widthIncludingTrailingWhitespace, m.height)
    }

    /// A path from point lists; closed figures fill, open ones only stroke
    pub fn path<'a>(&self, figures: impl Iterator<Item = &'a [(f32, f32)]>, closed: bool) -> Result<ID2D1PathGeometry> {
        unsafe {
            let path = self.d2d.CreatePathGeometry()?;
            let sink = path.Open()?;
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            let mut buf: Vec<Vector2> = vec![];
            for fig in figures {
                if fig.len() < 2 {
                    continue;
                }
                sink.BeginFigure(v2(fig[0].0, fig[0].1), if closed { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
                buf.clear();
                buf.extend(fig[1..].iter().map(|p| v2(p.0, p.1)));
                sink.AddLines(&buf);
                sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            }
            sink.Close()?;
            Ok(path)
        }
    }

    pub fn clear_text_cache(&self) {
        self.layouts.borrow_mut().clear();
    }
}

pub const fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

pub fn identity() -> Matrix3x2 {
    Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 }
}

pub fn translate(x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: x, M32: y }
}

pub fn scale_translate(s: f32, x: f32, y: f32) -> Matrix3x2 {
    Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: x, M32: y }
}

/// Draws on one render target: the window's or a layer's
pub struct Painter<'a> {
    pub rt: &'a ID2D1RenderTarget,
    pub gfx: &'a Gfx,
    pub brush: &'a ID2D1SolidColorBrush,
}

impl<'a> Painter<'a> {
    fn b(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&c.d2d()) };
        self.brush
    }

    pub fn fill(&self, a: R, c: Color) {
        unsafe { self.rt.FillRectangle(&a.d2d(), self.b(c)) }
    }

    pub fn stroke(&self, a: R, c: Color, w: f32) {
        unsafe { self.rt.DrawRectangle(&a.d2d(), self.b(c), w, None) }
    }

    pub fn round(&self, a: R, radius: f32, c: Color) {
        let rr = D2D1_ROUNDED_RECT { rect: a.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillRoundedRectangle(&rr, self.b(c)) }
    }

    pub fn round_stroke(&self, a: R, radius: f32, c: Color, w: f32) {
        let rr = D2D1_ROUNDED_RECT { rect: a.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt.DrawRoundedRectangle(&rr, self.b(c), w, None) }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color, w: f32) {
        unsafe { self.rt.DrawLine(v2(x0, y0), v2(x1, y1), self.b(c), w, &self.gfx.round_caps) }
    }

    pub fn circle(&self, cx: f32, cy: f32, radius: f32, c: Color) {
        let e = D2D1_ELLIPSE { point: v2(cx, cy), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillEllipse(&e, self.b(c)) }
    }

    pub fn ring(&self, cx: f32, cy: f32, radius: f32, c: Color, w: f32) {
        let e = D2D1_ELLIPSE { point: v2(cx, cy), radiusX: radius, radiusY: radius };
        unsafe { self.rt.DrawEllipse(&e, self.b(c), w, None) }
    }

    /// A clockwise arc from the top, `frac` of the circle
    pub fn arc(&self, cx: f32, cy: f32, radius: f32, frac: f32, c: Color, w: f32) {
        let frac = frac.clamp(0.0, 1.0);
        if frac <= 0.0 {
            return;
        }
        if frac >= 0.9999 {
            self.ring(cx, cy, radius, c, w);
            return;
        }
        let angle = frac * std::f32::consts::TAU;
        let end = v2(cx + radius * angle.sin(), cy - radius * angle.cos());
        unsafe {
            let Ok(path) = self.gfx.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = path.Open() else { return };
            sink.BeginFigure(v2(cx, cy - radius), D2D1_FIGURE_BEGIN_HOLLOW);
            let seg = D2D1_ARC_SEGMENT {
                point: end,
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            };
            sink.AddArc(&seg);
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            let _ = sink.Close();
            self.rt.DrawGeometry(&path, self.b(c), w, &self.gfx.round_caps);
        }
    }

    pub fn text(&self, s: &str, a: R, style: Style, c: Color) {
        if s.is_empty() || a.w < 1.0 || a.h < 1.0 {
            return;
        }
        if let Ok(l) = self.gfx.layout(s, a.w, a.h, style) {
            unsafe { self.rt.DrawTextLayout(v2(a.x, a.y), &l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_CLIP) }
        }
    }

    /// Readable over anything: a dark outline under the letters
    pub fn text_halo(&self, s: &str, a: R, style: Style, c: Color, halo: Color) {
        if let Ok(l) = self.gfx.layout(s, a.w, a.h, style) {
            unsafe {
                let brush = self.b(halo);
                for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-0.8, -0.8), (0.8, 0.8), (-0.8, 0.8), (0.8, -0.8)] {
                    self.rt.DrawTextLayout(v2(a.x + dx, a.y + dy), &l, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
                self.rt.DrawTextLayout(v2(a.x, a.y), &l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
    }

    pub fn clip(&self, a: R) {
        unsafe { self.rt.PushAxisAlignedClip(&a.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) }
    }

    pub fn unclip(&self) {
        unsafe { self.rt.PopAxisAlignedClip() }
    }

    fn stops(&self, list: &[(f32, Color)]) -> Result<ID2D1GradientStopCollection> {
        let stops: Vec<D2D1_GRADIENT_STOP> = list.iter().map(|(p, c)| D2D1_GRADIENT_STOP { position: *p, color: c.d2d() }).collect();
        unsafe { self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) }
    }

    /// `a` filled from `from` at (x0,y0) to `to` at (x1,y1)
    pub fn gradient(&self, a: R, x0: f32, y0: f32, x1: f32, y1: f32, from: Color, to: Color) {
        unsafe {
            let Ok(stops) = self.stops(&[(0.0, from), (1.0, to)]) else { return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(x0, y0), endPoint: v2(x1, y1) };
            if let Ok(brush) = self.rt.CreateLinearGradientBrush(&props, None, &stops) {
                self.rt.FillRectangle(&a.d2d(), &brush);
            }
        }
    }

    pub fn gradient_round(&self, a: R, radius: f32, x0: f32, y0: f32, x1: f32, y1: f32, from: Color, to: Color) {
        unsafe {
            let Ok(stops) = self.stops(&[(0.0, from), (1.0, to)]) else { return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(x0, y0), endPoint: v2(x1, y1) };
            if let Ok(brush) = self.rt.CreateLinearGradientBrush(&props, None, &stops) {
                let rr = D2D1_ROUNDED_RECT { rect: a.d2d(), radiusX: radius, radiusY: radius };
                self.rt.FillRoundedRectangle(&rr, &brush);
            }
        }
    }

    /// An oval glow: `center` in the middle to `edge` at the rim of the oval
    pub fn radial(&self, a: R, cx: f32, cy: f32, rx: f32, ry: f32, list: &[(f32, Color)]) {
        unsafe {
            let Ok(stops) = self.stops(list) else { return };
            let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center: v2(cx, cy),
                gradientOriginOffset: v2(0.0, 0.0),
                radiusX: rx,
                radiusY: ry,
            };
            if let Ok(brush) = self.rt.CreateRadialGradientBrush(&props, None, &stops) {
                self.rt.FillRectangle(&a.d2d(), &brush);
            }
        }
    }

    pub fn fill_geo(&self, g: &ID2D1Geometry, c: Color) {
        unsafe { self.rt.FillGeometry(g, self.b(c), None) }
    }

    pub fn stroke_geo(&self, g: &ID2D1Geometry, c: Color, w: f32) {
        unsafe { self.rt.DrawGeometry(g, self.b(c), w, &self.gfx.round_caps) }
    }

    pub fn stroke_geo_dashed(&self, g: &ID2D1Geometry, c: Color, w: f32) {
        unsafe { self.rt.DrawGeometry(g, self.b(c), w, &self.gfx.dashed) }
    }

    pub fn transform(&self, m: &Matrix3x2) {
        unsafe { self.rt.SetTransform(m) }
    }
}

/// A geometry as the base type the draw calls take
pub fn geo(p: &ID2D1PathGeometry) -> ID2D1Geometry {
    p.cast().expect("a path is a geometry")
}
