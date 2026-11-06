//! The map: land, coast, borders and all 1449 alert places, baked into the program, so it draws with no internet and
//! no tile server. Three cached layers keep a pulsing map almost free: the land only changes with the camera,
//! the alert colours with the state, and the pulse is one cached picture drawn at a changing opacity.

use crate::engine::{Snapshot, Zone};
use crate::gfx::*;
use crate::model::{Kind, Threat};
use crate::places::{project, Places};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Instant;
use windows::core::{Interface, Result};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;

pub const BASEMAP: &str = include_str!("../assets/basemap.json");

pub const SEA_TOP: Color = rgb(0x0C2036);
pub const SEA_BOTTOM: Color = rgb(0x081626);
pub const LAND: Color = rgb(0x12171D);
pub const HOME_LAND: Color = rgb(0x1E2832);
pub const ZONE_FILL: Color = rgb(0x2E3D4B);
pub const ZONE_EDGE: Color = rgb(0x5A6B7C);
pub const LAKE: Color = rgb(0x0E2233);
pub const COAST: Color = rgb(0x46627C);
pub const BORDER: Color = rgb(0x6F7F8E);
pub const HOME_BLUE: Color = rgb(0x45C2FF);

pub fn threat_color(t: Threat) -> Color {
    match t {
        Threat::Missiles | Threat::General | Threat::Blast => rgb(0xFF2E2E),
        Threat::Aircraft => rgb(0xFF7A1A),
        Threat::Infiltration => rgb(0xFFC800),
        Threat::Earthquake => rgb(0xC98B2B),
        Threat::Tsunami => rgb(0x2E8BFF),
        Threat::Hazmat => rgb(0xA855F7),
        Threat::Radiological | Threat::NonConventional => rgb(0xEE35A8),
    }
}

pub fn kind_color(z: &Zone) -> Color {
    match z.kind {
        Kind::Siren => threat_color(z.threat),
        Kind::Stay => rgb(0xE5484D),
        Kind::Near => rgb(0xFF8F40),
        Kind::Pre => rgb(0xFFB020),
        Kind::Notice => rgb(0x5B9BD5),
        Kind::End => rgb(0x2ED47A),
    }
}

/// The names kept on the map at every zoom; rank says from which zoom on
const CITIES: &[(&str, f64, f64, u8)] = &[
    ("ירושלים", 31.78, 35.21, 1),
    ("תל אביב", 32.08, 34.78, 1),
    ("חיפה", 32.80, 34.99, 1),
    ("באר שבע", 31.25, 34.79, 1),
    ("אילת", 29.56, 34.95, 1),
    ("טבריה", 32.79, 35.53, 2),
    ("נצרת", 32.70, 35.30, 2),
    ("צפת", 32.97, 35.50, 2),
    ("קריית שמונה", 33.21, 35.57, 1),
    ("עכו", 32.93, 35.08, 2),
    ("נהריה", 33.01, 35.10, 2),
    ("עפולה", 32.61, 35.29, 2),
    ("נתניה", 32.33, 34.86, 2),
    ("אשדוד", 31.80, 34.65, 2),
    ("אשקלון", 31.67, 34.57, 2),
    ("ערד", 31.26, 35.21, 2),
    ("דימונה", 31.07, 35.03, 2),
    ("מצפה רמון", 30.61, 34.80, 1),
    ("קצרין", 32.99, 35.69, 2),
    ("בית שאן", 32.50, 35.50, 2),
    ("חדרה", 32.44, 34.92, 3),
    ("שדרות", 31.52, 34.60, 3),
    ("מודיעין", 31.90, 35.01, 3),
    ("רחובות", 31.89, 34.81, 3),
    ("כפר סבא", 32.18, 34.91, 3),
    ("קריית גת", 31.61, 34.76, 3),
    ("בית שמש", 31.75, 34.99, 3),
    ("אריאל", 32.10, 35.17, 3),
    ("יטבתה", 29.90, 35.06, 3),
    ("עין גדי", 31.46, 35.39, 3),
];

const COUNTRIES: &[(&str, f64, f64)] = &[
    ("לבנון", 33.78, 35.80),
    ("סוריה", 33.05, 36.55),
    ("ירדן", 31.05, 36.45),
    ("מצרים", 30.25, 33.75),
    ("הים התיכון", 32.35, 34.15),
    ("עזה", 31.43, 34.38),
];

#[derive(Deserialize)]
struct RawBasemap {
    land: Vec<Vec<f64>>,
    home: Vec<Vec<f64>>,
    coast: Vec<Vec<f64>>,
    borders: Vec<Vec<f64>>,
    lakes: Vec<Vec<f64>>,
}

fn rings(raw: &[Vec<f64>]) -> Vec<Vec<(f32, f32)>> {
    raw.iter().map(|f| f.chunks_exact(2).map(|c| project(c[1], c[0])).collect()).collect()
}

/// The map's geometry, independent of any window
pub struct Geo {
    pub land: ID2D1Geometry,
    pub home_land: ID2D1Geometry,
    pub lakes: ID2D1Geometry,
    pub coast: ID2D1Geometry,
    pub borders: ID2D1Geometry,
    pub all_zones: ID2D1Geometry,
}

impl Geo {
    pub fn new(gfx: &Gfx, places: &Places) -> Result<Geo> {
        let raw: RawBasemap = serde_json::from_str(BASEMAP).expect("embedded base map");
        let land = rings(&raw.land);
        let home = rings(&raw.home);
        let lakes = rings(&raw.lakes);
        let coast = rings(&raw.coast);
        let borders = rings(&raw.borders);
        Ok(Geo {
            land: geo(&gfx.path(land.iter().map(|v| v.as_slice()), true)?),
            home_land: geo(&gfx.path(home.iter().map(|v| v.as_slice()), true)?),
            lakes: geo(&gfx.path(lakes.iter().map(|v| v.as_slice()), true)?),
            coast: geo(&gfx.path(coast.iter().map(|v| v.as_slice()), false)?),
            borders: geo(&gfx.path(borders.iter().map(|v| v.as_slice()), false)?),
            all_zones: geo(&gfx.path(places.list.iter().map(|p| p.poly.as_slice()), true)?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub cx: f32,
    pub cy: f32,
    /// dips per km
    pub scale: f32,
}

impl Camera {
    pub fn fit(b: (f32, f32, f32, f32), w: f32, h: f32, pad: f32, max_scale: f32) -> Camera {
        let (bw, bh) = ((b.2 - b.0).max(1.0), (b.3 - b.1).max(1.0));
        let scale = ((w * (1.0 - 2.0 * pad)) / bw).min((h * (1.0 - 2.0 * pad)) / bh).min(max_scale);
        Camera { cx: (b.0 + b.2) / 2.0, cy: (b.1 + b.3) / 2.0, scale }
    }

    pub fn matrix(&self, w: f32, h: f32) -> windows_numerics::Matrix3x2 {
        scale_translate(self.scale, w / 2.0 - self.cx * self.scale, h / 2.0 - self.cy * self.scale)
    }

    pub fn to_view(&self, w: f32, h: f32, x: f32, y: f32) -> (f32, f32) {
        ((x - self.cx) * self.scale + w / 2.0, (y - self.cy) * self.scale + h / 2.0)
    }

    pub fn to_world(&self, w: f32, h: f32, vx: f32, vy: f32) -> (f32, f32) {
        ((vx - w / 2.0) / self.scale + self.cx, (vy - h / 2.0) / self.scale + self.cy)
    }
}

/// The country, north of Metula to south of Eilat
pub fn israel_box() -> (f32, f32, f32, f32) {
    let (x0, y0) = project(33.38, 34.2);
    let (x1, y1) = project(29.45, 35.95);
    (x0, y0, x1, y1)
}

struct Layer {
    rt: ID2D1BitmapRenderTarget,
    w: f32,
    h: f32,
}

impl Layer {
    fn new(parent: &ID2D1RenderTarget, w: f32, h: f32) -> Result<Layer> {
        let size = D2D_SIZE_F { width: w, height: h };
        let format = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
        let rt = unsafe { parent.CreateCompatibleRenderTarget(Some(&size), None, Some(&format), D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE)? };
        unsafe { rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE) };
        Ok(Layer { rt, w, h })
    }

    fn target(&self) -> ID2D1RenderTarget {
        self.rt.cast().expect("a bitmap target is a target")
    }

    fn blit(&self, to: &ID2D1RenderTarget, at: R, opacity: f32) {
        unsafe {
            if let Ok(bitmap) = self.rt.GetBitmap() {
                to.DrawBitmap(&bitmap, Some(&at.d2d()), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct Highlight {
    pub id: String,
    pub places: Vec<usize>,
    pub color: Option<(f32, f32, f32)>,
}

#[derive(Clone, Copy)]
pub struct MapOpts {
    pub inset: bool,
    pub ripples: bool,
    pub city_labels: bool,
}

pub struct MapView {
    pub cam: Camera,
    target: Camera,
    last: Instant,
    pub user_until: Option<Instant>,
    pub moving: bool,
    base: Option<Layer>,
    steady: Option<Layer>,
    pulse: Option<Layer>,
    base_key: Option<(Camera, u32, u32)>,
    steady_key: Option<(u64, usize, Camera, u32, u32, String)>,
    pub highlight: Highlight,
    fitted: bool,
    has_pulse: bool,
    ripples: Vec<(f32, f32)>,
}

impl MapView {
    pub fn new() -> MapView {
        let cam = Camera { cx: 0.0, cy: 0.0, scale: 1.0 };
        MapView {
            cam,
            target: cam,
            last: Instant::now(),
            user_until: None,
            moving: false,
            base: None,
            steady: None,
            pulse: None,
            base_key: None,
            steady_key: None,
            highlight: Highlight::default(),
            fitted: false,
            has_pulse: false,
            ripples: vec![],
        }
    }

    /// The render target changed: every picture on it is gone
    pub fn lost(&mut self) {
        self.base = None;
        self.steady = None;
        self.pulse = None;
        self.base_key = None;
        self.steady_key = None;
    }

    pub fn user_zoom(&mut self, w: f32, h: f32, vx: f32, vy: f32, factor: f32) {
        let (wx, wy) = self.cam.to_world(w, h, vx, vy);
        let scale = (self.cam.scale * factor).clamp(0.8, 90.0);
        self.cam.cx = wx - (vx - w / 2.0) / scale;
        self.cam.cy = wy - (vy - h / 2.0) / scale;
        self.cam.scale = scale;
        self.target = self.cam;
        self.user_until = Some(Instant::now() + std::time::Duration::from_secs(45));
    }

    pub fn user_pan(&mut self, dx: f32, dy: f32) {
        self.cam.cx -= dx / self.cam.scale;
        self.cam.cy -= dy / self.cam.scale;
        self.target = self.cam;
        self.user_until = Some(Instant::now() + std::time::Duration::from_secs(45));
    }

    pub fn user_reset(&mut self) {
        self.user_until = None;
    }

    fn pick_target(&mut self, places: &Places, snap: &Snapshot, homes: &[usize], w: f32, h: f32, now_ms: i64) -> Camera {
        let default = Camera::fit(israel_box(), w, h, 0.02, 100.0);
        let mut chosen: Vec<usize> = vec![];
        if !self.highlight.places.is_empty() {
            chosen = self.highlight.places.clone();
        } else {
            let urgent: Vec<usize> = snap.zones.iter().filter(|z| z.urgent(now_ms)).filter_map(|z| z.place).collect();
            if !urgent.is_empty() {
                chosen = urgent;
            } else {
                for pick in [Kind::Siren, Kind::Stay, Kind::Near, Kind::Pre, Kind::End] {
                    chosen = snap.zones.iter().filter(|z| z.kind == pick).filter_map(|z| z.place).collect();
                    if !chosen.is_empty() {
                        break;
                    }
                }
            }
        }
        if chosen.is_empty() {
            return default;
        }
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &i in &chosen {
            let p = &places.list[i];
            b.0 = b.0.min(p.bbox.0);
            b.1 = b.1.min(p.bbox.1);
            b.2 = b.2.max(p.bbox.2);
            b.3 = b.3.max(p.bbox.3);
        }
        // home in the picture, when it says "how close to me"
        for &hm in homes {
            let p = &places.list[hm];
            let dist = ((p.x - (b.0 + b.2) / 2.0).powi(2) + (p.y - (b.1 + b.3) / 2.0).powi(2)).sqrt();
            if dist < 70.0 {
                b.0 = b.0.min(p.x);
                b.1 = b.1.min(p.y);
                b.2 = b.2.max(p.x);
                b.3 = b.3.max(p.y);
            }
        }
        // never so close that the place loses its surroundings
        let (cx, cy) = ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0);
        let (hw, hh) = (((b.2 - b.0) / 2.0).max(14.0), ((b.3 - b.1) / 2.0).max(14.0));
        let cam = Camera::fit((cx - hw, cy - hh, cx + hw, cy + hh), w, h, 0.14, 60.0);
        if cam.scale < default.scale {
            default
        } else {
            cam
        }
    }

    /// One step of the camera toward where the news is. Returns true while it still moves.
    fn step_camera(&mut self, target: Camera) {
        let user = self.user_until.map(|t| Instant::now() < t).unwrap_or(false);
        let dt = self.last.elapsed().as_secs_f32().min(0.1);
        self.last = Instant::now();
        if !self.fitted {
            self.cam = target;
            self.target = target;
            self.fitted = true;
            self.moving = false;
            return;
        }
        if user {
            self.moving = false;
            return;
        }
        self.target = target;
        let k = 1.0 - (-dt / 0.22f32).exp();
        let c = &mut self.cam;
        let (dx, dy) = (target.cx - c.cx, target.cy - c.cy);
        let ds = (target.scale / c.scale).ln();
        if dx.abs() * c.scale < 0.4 && dy.abs() * c.scale < 0.4 && ds.abs() < 0.002 {
            *c = target;
            self.moving = false;
        } else {
            c.cx += dx * k;
            c.cy += dy * k;
            c.scale *= (ds * k).exp();
            self.moving = true;
        }
    }

    pub fn invalidate_fit(&mut self) {
        self.fitted = false;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        rt: &ID2D1RenderTarget,
        brush: &ID2D1SolidColorBrush,
        gfx: &Gfx,
        g: &Geo,
        places: &Places,
        snap: &Snapshot,
        homes: &[usize],
        vp: R,
        now_ms: i64,
        t: f32,
        opts: MapOpts,
    ) -> Result<()> {
        let (w, h) = (vp.w.max(8.0), vp.h.max(8.0));
        let target = self.pick_target(places, snap, homes, w, h, now_ms);
        self.step_camera(target);
        let (wu, hu) = (w.round() as u32, h.round() as u32);
        if self.base.as_ref().map(|l| l.w != w || l.h != h).unwrap_or(true) {
            self.base = Some(Layer::new(rt, w, h)?);
            self.steady = Some(Layer::new(rt, w, h)?);
            self.pulse = Some(Layer::new(rt, w, h)?);
            self.base_key = None;
            self.steady_key = None;
        }
        let cam = self.cam;
        if self.base_key != Some((cam, wu, hu)) {
            self.draw_base(brush, gfx, g, w, h)?;
            self.base_key = Some((cam, wu, hu));
        }
        let urgent_count = snap.zones.iter().filter(|z| z.urgent(now_ms)).count();
        let key = (snap.rev, urgent_count, cam, wu, hu, self.highlight.id.clone());
        if self.steady_key.as_ref() != Some(&key) {
            self.draw_alerts(brush, gfx, g, places, snap, homes, w, h, now_ms, opts)?;
            self.steady_key = Some(key);
        }
        if let Some(l) = &self.base {
            l.blit(rt, vp, 1.0);
        }
        if self.has_pulse {
            let wave = 0.5 + 0.5 * (t * std::f32::consts::TAU / 1.25).sin();
            if let Some(l) = &self.pulse {
                l.blit(rt, vp, 0.42 + 0.53 * wave);
            }
        }
        if let Some(l) = &self.steady {
            l.blit(rt, vp, 1.0);
        }
        // the moving parts, every frame
        let p = Painter { rt, gfx, brush };
        p.clip(vp);
        if opts.ripples {
            let phase = (t / 2.1).fract();
            for &(x, y) in &self.ripples {
                let (vx, vy) = cam.to_view(w, h, x, y);
                for k in 0..2 {
                    let f = (phase + k as f32 * 0.5).fract();
                    p.ring(vp.x + vx, vp.y + vy, 6.0 + f * 46.0, rgb(0xFF5555).with((1.0 - f) * 0.65), 1.6);
                }
            }
        }
        for &hm in homes {
            let c = &places.list[hm];
            let (vx, vy) = cam.to_view(w, h, c.x, c.y);
            let (x, y) = (vp.x + vx, vp.y + vy);
            let breath = 0.5 + 0.5 * (t * 1.6).sin();
            p.circle(x, y, 9.0 + breath * 3.0, HOME_BLUE.with(0.22));
            p.circle(x, y, 5.0, rgb(0xFFFFFF));
            p.circle(x, y, 3.4, HOME_BLUE);
        }
        p.unclip();
        Ok(())
    }

    fn draw_base(&mut self, brush: &ID2D1SolidColorBrush, gfx: &Gfx, g: &Geo, w: f32, h: f32) -> Result<()> {
        let layer = self.base.as_ref().expect("made above");
        let rt = layer.target();
        let cam = self.cam;
        unsafe {
            rt.BeginDraw();
            let p = Painter { rt: &rt, gfx, brush };
            p.gradient(r(0.0, 0.0, w, h), 0.0, 0.0, 0.0, h, SEA_TOP, SEA_BOTTOM);
            p.transform(&cam.matrix(w, h));
            let s = cam.scale;
            p.fill_geo(&g.land, LAND);
            p.fill_geo(&g.home_land, HOME_LAND);
            p.fill_geo(&g.all_zones, ZONE_FILL);
            // the places' edges: faint when the whole country is in view, clear close up
            let edge = ((s - 0.8) / 7.0).clamp(0.13, 0.62);
            p.stroke_geo(&g.all_zones, ZONE_EDGE.with(edge), 0.7 / s);
            p.fill_geo(&g.lakes, LAKE);
            p.stroke_geo(&g.lakes, COAST.with(0.8), 0.9 / s);
            p.stroke_geo(&g.coast, COAST, 1.1 / s);
            p.stroke_geo_dashed(&g.borders, BORDER.with(0.75), 1.0 / s);
            p.transform(&identity());
            rt.EndDraw(None, None)?;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_alerts(
        &mut self,
        brush: &ID2D1SolidColorBrush,
        gfx: &Gfx,
        g: &Geo,
        places: &Places,
        snap: &Snapshot,
        homes: &[usize],
        w: f32,
        h: f32,
        now_ms: i64,
        opts: MapOpts,
    ) -> Result<()> {
        let cam = self.cam;
        let matrix = cam.matrix(w, h);
        let s = cam.scale;
        // by colour, so each colour is one fill and one outline however many places it has
        let mut urgent: HashMap<u32, (Color, Vec<&[(f32, f32)]>)> = HashMap::new();
        let mut steady: HashMap<u32, (Color, Vec<&[(f32, f32)]>, f32)> = HashMap::new();
        let code = |c: Color| ((c.r * 255.0) as u32) << 16 | ((c.g * 255.0) as u32) << 8 | (c.b * 255.0) as u32;
        for z in &snap.zones {
            let Some(i) = z.place else { continue };
            let poly = places.list[i].poly.as_slice();
            let c = kind_color(z);
            if z.urgent(now_ms) {
                urgent.entry(code(c)).or_insert((c, vec![])).1.push(poly);
            } else {
                let fill = match z.kind {
                    Kind::Siren | Kind::Stay => 0.42,
                    Kind::End => 0.5,
                    Kind::Pre | Kind::Near => 0.34,
                    Kind::Notice => 0.2,
                };
                steady.entry(code(c)).or_insert((c, vec![], fill)).1.push(poly);
            }
        }
        // the pulse layer
        {
            let layer = self.pulse.as_ref().expect("made above");
            let rt = layer.target();
            unsafe {
                rt.BeginDraw();
                rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
                let p = Painter { rt: &rt, gfx, brush };
                p.transform(&matrix);
                for (c, polys) in urgent.values() {
                    let path = geo(&gfx.path(polys.iter().copied(), true)?);
                    p.fill_geo(&path, *c);
                }
                p.transform(&identity());
                rt.EndDraw(None, None)?;
            }
        }
        self.has_pulse = !urgent.is_empty();
        let layer = self.steady.as_ref().expect("made above");
        let rt = layer.target();
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        }
        let p = Painter { rt: &rt, gfx, brush };
        p.transform(&matrix);
        for (c, polys, fill) in steady.values() {
            let path = geo(&gfx.path(polys.iter().copied(), true)?);
            p.fill_geo(&path, c.with(*fill));
            p.stroke_geo(&path, c.with(0.9), 1.2 / s);
        }
        for (c, polys) in urgent.values() {
            let path = geo(&gfx.path(polys.iter().copied(), true)?);
            p.stroke_geo(&path, c.with(0.28), 6.0 / s);
            p.stroke_geo(&path, c.mix(rgb(0xFFFFFF), 0.25), 1.5 / s);
        }
        if !self.highlight.places.is_empty() {
            let c = self.highlight.color.map(|(r_, g_, b_)| Color { r: r_, g: g_, b: b_, a: 1.0 }).unwrap_or(rgb(0xB48CFF));
            let polys: Vec<&[(f32, f32)]> = self.highlight.places.iter().map(|&i| places.list[i].poly.as_slice()).collect();
            let path = geo(&gfx.path(polys.iter().copied(), true)?);
            p.fill_geo(&path, c.with(0.45));
            p.stroke_geo(&path, c, 1.3 / s);
        }
        p.transform(&identity());

        // names: home first, then what is under alert, then the country's towns where room is left
        let mut taken: Vec<R> = vec![];
        let free = |taken: &Vec<R>, a: R| !taken.iter().any(|b| a.x < b.right() && a.right() > b.x && a.y < b.bottom() && a.bottom() > b.y);
        let halo = rgb(0x05090D);
        for &hm in homes {
            let c = &places.list[hm];
            let (vx, vy) = cam.to_view(w, h, c.x, c.y);
            let style = st(14.0, 700).center();
            let (tw, th) = gfx.measure(&c.base, 300.0, style);
            let a = r(vx - tw / 2.0 - 2.0, vy + 11.0, tw + 4.0, th);
            p.text_halo(&c.base, a.inset_xy(-8.0, -1.0), style, HOME_BLUE.mix(rgb(0xFFFFFF), 0.3), halo);
            taken.push(r(vx - 8.0, vy - 8.0, 16.0, 16.0));
            taken.push(a);
        }
        // one name per town; when towns are too many for the view, one per alert area
        let mut towns: HashMap<&str, (f32, f32, u32, Color)> = HashMap::new();
        let mut areas: HashMap<&str, (f32, f32, u32, Color)> = HashMap::new();
        let mut order: Vec<(&str, f32)> = vec![];
        for z in &snap.zones {
            let Some(i) = z.place else { continue };
            if z.kind == Kind::Notice {
                continue;
            }
            let pl = &places.list[i];
            let (vx, vy) = cam.to_view(w, h, pl.x, pl.y);
            if vx < -40.0 || vy < -20.0 || vx > w + 40.0 || vy > h + 20.0 {
                continue;
            }
            let c = kind_color(z).mix(rgb(0xFFFFFF), 0.55);
            let e = towns.entry(pl.base.as_str()).or_insert_with(|| {
                order.push((pl.base.as_str(), z.km_from_home));
                (0.0, 0.0, 0, c)
            });
            e.0 += vx;
            e.1 += vy;
            e.2 += 1;
            let a = areas.entry(places.area_name(pl.area)).or_insert((0.0, 0.0, 0, c));
            a.0 += vx;
            a.1 += vy;
            a.2 += 1;
        }
        let room = (w * h / 2600.0) as usize;
        let use_areas = towns.len() > room.max(24);
        let style = st(if use_areas { 14.0 } else { 13.0 }, 600).center();
        let items: Vec<(&str, (f32, f32, u32, Color))> = if use_areas {
            let mut v: Vec<_> = areas.iter().map(|(k, a)| (*k, *a)).collect();
            v.sort_by_key(|x| std::cmp::Reverse(x.1 .2));
            v
        } else {
            order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            order.iter().map(|(k, _)| (*k, towns[k])).collect()
        };
        for (name, (sx, sy, n, c)) in items {
            if name.is_empty() {
                continue;
            }
            let (vx, vy) = (sx / n as f32, sy / n as f32);
            let (tw, th) = gfx.measure(name, 300.0, style);
            let a = r(vx - tw / 2.0 - 1.0, vy - th / 2.0, tw + 2.0, th);
            if a.x < 2.0 || a.y < 2.0 || a.right() > w - 2.0 || a.bottom() > h - 2.0 || !free(&taken, a) {
                continue;
            }
            p.text_halo(name, a.inset_xy(-8.0, -1.0), style, c, halo);
            taken.push(a);
        }
        if opts.city_labels {
            let dim = rgb(0xAEBCC9);
            let label = |taken: &mut Vec<R>, name: &str, lat: f64, lng: f64, style: Style, c: Color| {
                let (x, y) = project(lat, lng);
                let (vx, vy) = cam.to_view(w, h, x, y);
                let (tw, th) = gfx.measure(name, 300.0, style);
                let a = r(vx - tw / 2.0 - 1.0, vy - th / 2.0, tw + 2.0, th);
                if a.x < 2.0 || a.y < 2.0 || a.right() > w - 2.0 || a.bottom() > h - 2.0 || !free(taken, a) {
                    return;
                }
                p.text_halo(name, a.inset_xy(-8.0, -1.0), style, c, halo);
                taken.push(a);
            };
            for &(name, lat, lng) in COUNTRIES {
                label(&mut taken, name, lat, lng, st(15.0, 300).center(), rgb(0x6F8294));
            }
            for &(name, lat, lng, rank) in CITIES {
                let min_scale = match rank {
                    1 => 0.0,
                    2 => 1.5,
                    _ => 2.6,
                };
                if s >= min_scale {
                    let size = if rank == 1 { 13.0 } else { 12.0 };
                    label(&mut taken, name, lat, lng, st(size, 500).center(), dim);
                }
            }
            // close up, the small places around too
            if s >= 7.0 {
                let mut seen: Vec<&str> = vec![];
                for pl in &places.list {
                    let (vx, vy) = cam.to_view(w, h, pl.x, pl.y);
                    if vx < 0.0 || vy < 0.0 || vx > w || vy > h || seen.contains(&pl.base.as_str()) {
                        continue;
                    }
                    seen.push(&pl.base);
                    let style = st(11.0, 400).center();
                    let (tw, th) = gfx.measure(&pl.base, 300.0, style);
                    let a = r(vx - tw / 2.0, vy - th / 2.0, tw, th);
                    if a.x > 2.0 && a.y > 2.0 && a.right() < w - 2.0 && a.bottom() < h - 2.0 && free(&taken, a) {
                        p.text_halo(&pl.base, a.inset_xy(-8.0, -1.0), style, rgb(0x93A3B2), halo);
                        taken.push(a);
                    }
                    if taken.len() > 260 {
                        break;
                    }
                }
            }
        }

        // the whole country in a corner, when the camera is close: where this is
        let whole = Camera::fit(israel_box(), w, h, 0.02, 100.0);
        let view_km = h / s;
        let country_km = israel_box().3 - israel_box().1;
        if opts.inset && s > whole.scale * 1.5 && view_km < country_km * 0.6 && h > 300.0 {
            let ih = (h * 0.36).clamp(150.0, 300.0);
            let iw = ih * 0.46;
            let inset = r(12.0, h - ih - 12.0, iw, ih);
            p.round(inset, 6.0, rgb(0x0A1119).with(0.92));
            let c2 = Camera::fit(israel_box(), inset.w, inset.h, 0.03, 100.0);
            let m2 = scale_translate(c2.scale, inset.x + inset.w / 2.0 - c2.cx * c2.scale, inset.y + inset.h / 2.0 - c2.cy * c2.scale);
            p.clip(inset);
            p.transform(&m2);
            p.fill_geo(&g.land, LAND);
            p.fill_geo(&g.home_land, rgb(0x3A4A59));
            p.stroke_geo(&g.coast, COAST, 0.8 / c2.scale);
            let all: Vec<&[(f32, f32)]> = snap.zones.iter().filter_map(|z| z.place).map(|i| places.list[i].poly.as_slice()).collect();
            if !all.is_empty() {
                let path = geo(&gfx.path(all.iter().copied(), true)?);
                let c = snap.zones.first().map(kind_color).unwrap_or(rgb(0xFF2E2E));
                p.fill_geo(&path, c);
                p.stroke_geo(&path, c, 1.5 / c2.scale);
            }
            p.transform(&identity());
            let (a0, b0) = cam.to_world(w, h, 0.0, 0.0);
            let (a1, b1) = cam.to_world(w, h, w, h);
            let view = |x: f32, y: f32| (inset.x + inset.w / 2.0 + (x - c2.cx) * c2.scale, inset.y + inset.h / 2.0 + (y - c2.cy) * c2.scale);
            let (x0, y0) = view(a0, b0);
            let (x1, y1) = view(a1, b1);
            p.stroke(r(x0, y0, x1 - x0, y1 - y0), rgb(0xFFFFFF).with(0.9), 1.2);
            p.unclip();
            p.round_stroke(inset, 6.0, rgb(0x3A4957), 1.0);
        }
        unsafe { rt.EndDraw(None, None)? };

        // where the rings spread from: the biggest clusters still urgent
        self.ripples.clear();
        if opts.ripples {
            let mut by_area: HashMap<u16, (f32, f32, u32)> = HashMap::new();
            for z in snap.zones.iter().filter(|z| z.urgent(now_ms)) {
                if let Some(i) = z.place {
                    let pl = &places.list[i];
                    let e = by_area.entry(pl.area).or_insert((0.0, 0.0, 0));
                    e.0 += pl.x;
                    e.1 += pl.y;
                    e.2 += 1;
                }
            }
            let mut v: Vec<_> = by_area.values().copied().collect();
            v.sort_by_key(|e| std::cmp::Reverse(e.2));
            self.ripples.extend(v.iter().take(8).map(|e| (e.0 / e.2 as f32, e.1 / e.2 as f32)));
        }
        Ok(())
    }
}
