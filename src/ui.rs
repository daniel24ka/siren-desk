//! What each window shows, drawn fresh on each frame: the full screen, the side panel, the warning strip and the main window.
//! All of it right to left. Text states what is known and how old it is, never more.

use crate::engine::{FarMode, Level, Snapshot, Source, Zone};
use crate::gfx::*;
use crate::map::*;
use crate::model::{Kind, Threat};
use crate::paths;
use crate::places::Places;
use crate::settings::Settings;
use crate::sources::{Health, HistItem, Protection, SourceHealth};
use std::collections::BTreeMap;
use std::f32::consts::TAU;

const BG: Color = rgb(0x0B1118);
const PANEL: Color = rgb(0x111921);
const PANEL2: Color = rgb(0x19232D);
const PANEL3: Color = rgb(0x222E3A);
const LINE: Color = rgb(0x2A3744);
const TEXT: Color = rgb(0xF3F6F9);
const TEXT2: Color = rgb(0xAEBBC8);
const TEXT3: Color = rgb(0x7A8A98);
const RED: Color = rgb(0xFF3B3B);
const AMBER: Color = rgb(0xFFB020);
const GREEN: Color = rgb(0x2ED47A);
const WHITE: Color = rgb(0xFFFFFF);

#[derive(Default, Debug)]
pub struct Input {
    pub x: f32,
    pub y: f32,
    pub inside: bool,
    pub down: bool,
    pub clicked: bool,
    pub wheel: f32,
    pub hand: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drill {
    Home,
    Far,
    Big,
    Sequence,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Close,
    Mute,
    Tab(u8),
    PickHistory(usize),
    AddHome(usize),
    RemoveHome(usize),
    FarMode(FarMode),
    ToggleFarSound,
    ToggleNight,
    ToggleAllMonitors,
    ToggleAutostart,
    VolHome(i32),
    VolFar(i32),
    Drill(Drill),
    StopDrill,
    TestSound,
    Snooze(i64),
    OpenOfficial,
    ResetMap,
    ZoomIn,
    ZoomOut,
    FullScreenNow,
    CloseStrip,
}

pub struct View<'a> {
    pub places: &'a Places,
    pub snap: &'a Snapshot,
    pub health: &'a Health,
    pub settings: &'a Settings,
    pub history: &'a [HistItem],
    pub homes: &'a [usize],
    pub now_ms: i64,
    pub t: f32,
    pub siren_on: bool,
    pub output_note: Option<&'a str>,
    pub output_name: Option<&'a str>,
    pub autostart_on: bool,
    pub geo: &'a Geo,
    /// reading a stand-in for the sources: every alert on screen says so
    pub test_run: bool,
}

fn button(p: &Painter, i: &mut Input, a: R, label: &str, size: f32, normal: Color, hover: Color, fg: Color) -> bool {
    let hot = i.inside && a.contains(i.x, i.y);
    if hot {
        i.hand = true;
    }
    p.round(a, 8.0, if hot { hover } else { normal });
    p.text(label, a, st(size, 600).center().middle(), fg);
    hot && i.clicked
}

fn plain_button(p: &Painter, i: &mut Input, a: R, label: &str) -> bool {
    button(p, i, a, label, 14.0, PANEL3, rgb(0x2D3B49), TEXT)
}

fn x_button(p: &Painter, i: &mut Input, a: R, on_dark: bool) -> bool {
    let hot = i.inside && a.contains(i.x, i.y);
    if hot {
        i.hand = true;
    }
    p.round(a, 8.0, if on_dark { rgb(0x000000).with(if hot { 0.55 } else { 0.32 }) } else if hot { PANEL3 } else { PANEL2 });
    let (cx, cy, d) = (a.cx(), a.cy(), a.w * 0.2);
    p.line(cx - d, cy - d, cx + d, cy + d, WHITE, 2.0);
    p.line(cx + d, cy - d, cx - d, cy + d, WHITE, 2.0);
    hot && i.clicked
}

fn toggle(p: &Painter, i: &mut Input, a: R, label: &str, on: bool) -> bool {
    let hot = i.inside && a.contains(i.x, i.y);
    if hot {
        i.hand = true;
    }
    let sw = r(a.x, a.cy() - 11.0, 40.0, 22.0);
    p.round(sw, 11.0, if on { GREEN.mix(rgb(0x000000), 0.15) } else { PANEL3 });
    p.circle(if on { sw.right() - 11.0 } else { sw.x + 11.0 }, sw.cy(), 8.0, WHITE);
    p.text(label, r(a.x + 50.0, a.y, a.w - 50.0, a.h), st(14.0, 400).middle(), if hot { TEXT } else { TEXT2 });
    hot && i.clicked
}

fn section(p: &Painter, a: R, label: &str) {
    p.text(label, a, st(13.0, 600).middle(), TEXT3);
}

fn seconds_ago(age: f32) -> String {
    if age < 60.0 {
        format!("{age:.1} ש׳")
    } else if age < 3600.0 {
        format!("{:.0} דק׳", age / 60.0)
    } else {
        format!("{:.1} שע׳", age / 3600.0)
    }
}

fn source_name(s: Option<Source>) -> &'static str {
    match s {
        Some(Source::Sirens) | Some(Source::Messages) | Some(Source::Feed) => "צופר",
        Some(Source::Official) => "פיקוד העורף",
        Some(Source::Drill) => "תרגיל",
        None => "",
    }
}

struct Group {
    area: String,
    color: Color,
    countdown: i32,
    towns: BTreeMap<String, Vec<String>>,
    km: f32,
    count: usize,
    home: bool,
}

impl Group {
    fn text(&self) -> String {
        let mut out = Vec::with_capacity(self.towns.len());
        for (town, subs) in &self.towns {
            if subs.is_empty() || subs.iter().all(|s| s.is_empty()) {
                out.push(town.clone());
            } else {
                out.push(format!("{town} ({})", subs.join(", ")));
            }
        }
        out.join(" • ")
    }
}

fn groups(v: &View) -> Vec<Group> {
    let mut map: BTreeMap<String, Group> = BTreeMap::new();
    for z in v.snap.zones.iter() {
        let (area, base, sub) = match z.place {
            Some(i) => {
                let pl = &v.places.list[i];
                (v.places.area_name(pl.area).to_string(), pl.base.clone(), pl.sub.clone())
            }
            None => ("לא ברשימה".to_string(), z.name.clone(), String::new()),
        };
        let key = format!("{:?}|{area}", z.kind);
        let g = map.entry(key).or_insert_with(|| Group {
            area: area.clone(),
            color: kind_color(z),
            // (a place's seconds are rocket flight time: other alerts are "now", and an ended one has no clock)
            countdown: if z.kind == Kind::End || (z.kind == Kind::Siren && !z.threat.has_countdown()) { 0 } else { z.countdown_s },
            towns: BTreeMap::new(),
            km: f32::MAX,
            count: 0,
            home: false,
        });
        g.towns.entry(base).or_default().push(sub);
        g.km = g.km.min(z.km_from_home);
        g.count += 1;
        g.home |= z.home;
    }
    let mut out: Vec<Group> = map.into_values().collect();
    out.sort_by(|a, b| b.home.cmp(&a.home).then(a.km.partial_cmp(&b.km).unwrap_or(std::cmp::Ordering::Equal)));
    out
}

/// The list of places under alert, by alert area. Returns the height it wanted.
fn draw_groups(p: &Painter, v: &View, i: &mut Input, a: R, scroll: &mut f32) {
    let gs = groups(v);
    let hovered = i.inside && a.contains(i.x, i.y);
    if hovered && i.wheel != 0.0 {
        *scroll = (*scroll - i.wheel * 60.0).max(0.0);
        i.wheel = 0.0;
    }
    p.clip(a);
    let mut y = a.y - *scroll;
    let text_w = a.w - 28.0;
    let body = st(14.0, 400).wrap();
    for g in &gs {
        let t = g.text();
        let (_, th) = p.gfx.measure(&t, text_w, body);
        let height = 34.0 + th + 12.0;
        if y + height > a.y && y < a.bottom() {
            let card = r(a.x, y, a.w, height - 6.0);
            p.round(card, 8.0, if g.home { HOME_BLUE.with(0.14) } else { PANEL2 });
            p.round(r(card.right() - 4.0, card.y + 6.0, 4.0, card.h - 12.0), 2.0, g.color);
            let head = r(card.x + 12.0, card.y + 6.0, card.w - 26.0, 24.0);
            p.text(&g.area, head, st(15.0, 700).middle(), TEXT);
            let right = if g.countdown > 0 { format!("{} יישובים • {} שנ׳ למרחב מוגן", g.count, g.countdown) } else { format!("{} יישובים", g.count) };
            p.text(&right, head, st(13.0, 500).end().middle(), TEXT2);
            p.text(&t, r(card.x + 12.0, card.y + 31.0, text_w, th + 2.0), body, TEXT2);
        }
        y += height;
    }
    let total = y + *scroll - a.y;
    *scroll = scroll.min((total - a.h).max(0.0));
    p.unclip();
    if total > a.h {
        // there is more below
        let fade = a.bottom_part(26.0);
        p.gradient(fade, fade.x, fade.y, fade.x, fade.bottom(), PANEL.with(0.0), PANEL);
    }
}

fn drill_band(p: &Painter, a: R, size: f32) {
    p.fill(a, rgb(0xFFD60A));
    p.text("תרגיל — זו לא אזעקה אמיתית", a, st(size, 800).center().middle(), rgb(0x111111));
}

fn delay_line(v: &View) -> String {
    let mut parts = vec![paths::clock(v.now_ms)];
    if let Some(d) = v.snap.last_delay_ms {
        if v.snap.last_source != Some(Source::Drill) {
            parts.push(format!("הגיע {:.0} ש׳ אחרי תחילת ההתרעה", (d as f32 / 1000.0).max(0.0)));
        }
    }
    let src = source_name(v.snap.last_source);
    if !src.is_empty() {
        parts.push(format!("מקור: {src}"));
    }
    parts.join("  •  ")
}

fn near_line(v: &View) -> Option<String> {
    let z = v.snap.nearest_alert()?;
    if z.km_from_home == f32::MAX || z.home {
        return None;
    }
    Some(format!("הכי קרוב אליך: {} — {:.0} ק״מ", z.name, z.km_from_home))
}

fn focus_zone<'b>(snap: &'b Snapshot, now_ms: i64) -> Option<&'b Zone> {
    if let Some(h) = snap.home_zone() {
        return Some(h);
    }
    snap.zones.iter().filter(|z| z.urgent(now_ms)).max_by_key(|z| z.alert_ms).or_else(|| snap.zones.iter().max_by_key(|z| (z.kind, z.alert_ms)))
}

// ---------------------------------------------------------------- the full screen

pub fn draw_full(p: &Painter, v: &View, map: &mut MapView, i: &mut Input, w: f32, h: f32, scroll: &mut f32, out: &mut Vec<Action>) {
    let now = v.now_ms;
    let focus = focus_zone(v.snap, now);
    let kind = focus.map(|z| z.kind).unwrap_or(Kind::End);
    let threat = focus.map(|z| z.threat).unwrap_or(Threat::General);
    let urgent = focus.map(|z| z.urgent(now)).unwrap_or(false);
    let drill = v.snap.zones.iter().any(|z| z.drill) || v.test_run;
    let accent = match kind {
        Kind::Siren | Kind::Stay => focus.map(kind_color).unwrap_or(RED),
        Kind::Pre | Kind::Near => AMBER,
        Kind::Notice => rgb(0x5B9BD5),
        Kind::End => GREEN,
    };
    let wave = 0.5 + 0.5 * (v.t * TAU / 1.25).sin();
    let all = r(0.0, 0.0, w, h);
    p.fill(all, accent.mix(rgb(0x05060A), 0.9));
    let glow = if urgent { 0.30 + 0.34 * wave } else { 0.2 };
    p.radial(all, w * 0.5, h * 0.5, w * 0.78, h * 0.92, &[(0.42, accent.with(0.0)), (1.0, accent.with(glow))]);
    let mut area = all;
    if drill {
        let band = area.top_part(56.0);
        drill_band(p, band, 26.0);
        area = area.below(56.0);
    }
    let area = area.inset(24.0);
    let map_w = (area.w * 0.57).round();
    let map_rect = area.left_part(map_w);
    let col = r(map_rect.right() + 28.0, area.y, area.w - map_w - 28.0, area.h);
    p.fill(map_rect, BG);
    let _ = map.draw(p.rt, p.brush, p.gfx, v.geo, v.places, v.snap, v.homes, map_rect, now, v.t, MapOpts { inset: true, ripples: urgent, city_labels: true });
    p.stroke(map_rect, accent.with(0.65 + if urgent { 0.35 * wave } else { 0.0 }), 2.0);

    // top row: buttons at the far end, the time at the start
    let top = col.top_part(44.0);
    if x_button(p, i, top.left_part(44.0), true) {
        out.push(Action::Close);
    }
    if v.siren_on && button(p, i, r(top.x + 54.0, top.y, 110.0, 44.0), "השתק", 16.0, rgb(0x000000).with(0.32), rgb(0x000000).with(0.55), WHITE) {
        out.push(Action::Mute);
    }
    if drill && button(p, i, r(top.x + 174.0, top.y, 150.0, 44.0), "סיום תרגיל", 16.0, rgb(0x000000).with(0.32), rgb(0x000000).with(0.55), WHITE) {
        out.push(Action::StopDrill);
    }
    p.text(&paths::clock(now), top, st(22.0, 500).num().middle(), TEXT2);

    let mut y = top.bottom() + 12.0;
    // the headline, as large as it fits
    let name = focus.map(|z| z.name.as_str()).unwrap_or("");
    let is_home = focus.map(|z| z.home).unwrap_or(false);
    let title = match kind {
        Kind::Siren if is_home && urgent => format!("אזעקה ב{name}"),
        Kind::Siren if is_home => "הישארו במרחב המוגן".to_string(),
        Kind::Siren => threat.title().to_string(),
        Kind::Stay => "המשיכו לשהות במרחב המוגן".to_string(),
        Kind::Near => "הישארו בקרבת המרחב המוגן".to_string(),
        Kind::Pre => "התרעה מקדימה".to_string(),
        Kind::Notice => "הודעת פיקוד העורף".to_string(),
        Kind::End => "האירוע הסתיים".to_string(),
    };
    let mut size = 92.0;
    while size > 40.0 && p.gfx.measure(&title, 4000.0, st(size, 800)).0 > col.w {
        size -= 4.0;
    }
    let title_h = size * 1.28;
    p.text(&title, r(col.x, y, col.w, title_h), st(size, 800).middle(), WHITE);
    y += title_h + 2.0;
    let sub = match kind {
        Kind::Siren if is_home && !urgent => format!("אזעקה ב{name} • {}", threat.title()),
        Kind::Siren if is_home => threat.title().to_string(),
        Kind::Siren => format!("{} יישובים באזעקה", v.snap.count(Kind::Siren)),
        Kind::End => format!("הודעת סיום עבור {name}"),
        _ => name.to_string(),
    };
    p.text(&sub, r(col.x, y, col.w, 46.0), st(30.0, 500).middle(), rgb(0xFFFFFF).with(0.82));
    y += 56.0;

    if kind == Kind::Siren {
        // what to do, the widest thing on the screen
        let bar = r(col.x, y, col.w, 88.0);
        let fill = if urgent { accent.mix(WHITE, 0.1 * wave) } else { accent.mix(rgb(0x000000), 0.45) };
        p.round(bar, 12.0, fill);
        let action = if urgent { threat.action() } else { "עד הודעת סיום מפיקוד העורף" };
        let mut s2 = 42.0;
        while s2 > 22.0 && p.gfx.measure(action, 4000.0, st(s2, 800)).0 > bar.w - 30.0 {
            s2 -= 2.0;
        }
        p.text(action, bar, st(s2, 800).center().middle(), WHITE);
        y += 88.0 + 18.0;
    } else if let Some(m) = v.snap.message.as_ref().filter(|m| !m.body.is_empty()) {
        let (_, th) = p.gfx.measure(&m.body, col.w, st(24.0, 400).wrap());
        p.text(&m.body, r(col.x, y, col.w, th + 4.0), st(24.0, 400).wrap(), TEXT);
        y += th + 18.0;
    }

    // the clock of the event
    let footer_h = 30.0;
    let rest_bottom = col.bottom() - footer_h;
    let big_h = ((rest_bottom - y) * 0.62).min(col.w * 0.62).max(0.0);
    if let Some(z) = focus {
        let cx = col.cx();
        let radius = (big_h / 2.0 - 8.0).max(10.0);
        let cy = y + big_h / 2.0;
        match kind {
            Kind::Siren if z.left_s(now).is_some() && z.left_s(now) != Some(0) => {
                let left = z.left_s(now).unwrap_or(0);
                let exact = (z.countdown_s as f32 - (now - z.alert_ms) as f32 / 1000.0).max(0.0);
                let total = z.countdown_s.max(1) as f32;
                p.ring(cx, cy, radius, WHITE.with(0.13), 16.0);
                p.arc(cx, cy, radius, exact / total, if left <= 15 { WHITE } else { accent.mix(WHITE, 0.25) }, 16.0);
                let digits = st(radius * 1.05, 700).num().center().middle();
                p.text(&format!("{left}"), r(cx - radius, cy - radius * 0.72, radius * 2.0, radius * 1.2), digits, WHITE);
                p.text("שניות", r(cx - radius, cy + radius * 0.42, radius * 2.0, radius * 0.34), st((radius * 0.17).max(14.0), 500).center().middle(), TEXT2);
            }
            Kind::Siren | Kind::Stay | Kind::Near | Kind::Pre => {
                let since = (now - z.alert_ms) / 1000;
                p.ring(cx, cy, radius, WHITE.with(0.1), 6.0);
                p.text(&paths::elapsed(since), r(cx - radius, cy - radius * 0.55, radius * 2.0, radius * 0.9), st(radius * 0.58, 600).num().center().middle(), WHITE);
                let label = if kind == Kind::Pre { "מאז ההתרעה" } else { "מאז תחילת האזעקה" };
                p.text(label, r(cx - radius, cy + radius * 0.3, radius * 2.0, radius * 0.3), st((radius * 0.14).max(13.0), 500).center().middle(), TEXT2);
            }
            Kind::End | Kind::Notice => {
                p.ring(cx, cy, radius * 0.7, GREEN, 8.0);
                let d = radius * 0.3;
                p.line(cx - d, cy + d * 0.05, cx - d * 0.2, cy + d * 0.75, GREEN, 10.0);
                p.line(cx - d * 0.2, cy + d * 0.75, cx + d, cy - d * 0.6, GREEN, 10.0);
            }
        }
    }
    y += big_h + 10.0;
    // who else
    let others = v.snap.zones.iter().filter(|z| !z.home && z.kind != Kind::End).count();
    if others > 0 && rest_bottom - y > 40.0 {
        let line = match near_line(v) {
            Some(n) => format!("עוד {others} יישובים  •  {n}"),
            None => format!("עוד {others} יישובים"),
        };
        p.text(&line, r(col.x, y, col.w, 26.0), st(17.0, 600).middle(), TEXT);
        y += 30.0;
        let list = r(col.x, y, col.w, rest_bottom - y - 6.0);
        if list.h > 40.0 {
            draw_groups_on_glass(p, v, i, list, scroll);
        }
    }
    p.text(&delay_line(v), col.bottom_part(footer_h), st(14.0, 400).middle(), TEXT2);
    if let Some(trouble) = v.health.trouble() {
        p.text(&trouble, r(col.x, col.bottom() - footer_h - 22.0, col.w, 22.0), st(14.0, 600).middle(), AMBER);
    }
}

fn draw_groups_on_glass(p: &Painter, v: &View, i: &mut Input, a: R, scroll: &mut f32) {
    p.round(a, 8.0, rgb(0x000000).with(0.28));
    draw_groups_inner(p, v, i, a.inset(6.0), scroll);
}

fn draw_groups_inner(p: &Painter, v: &View, i: &mut Input, a: R, scroll: &mut f32) {
    draw_groups(p, v, i, a, scroll);
}

// ---------------------------------------------------------------- the side panel

fn panel_look(v: &View) -> (Color, String, String) {
    let snap = v.snap;
    let now = v.now_ms;
    let focus = focus_zone(snap, now);
    let Some(z) = focus else { return (TEXT3, "אין אזעקות".to_string(), String::new()) };
    let sirens = snap.count(Kind::Siren);
    match z.kind {
        Kind::Siren => {
            let title = z.threat.title().to_string();
            let sub = if sirens == 1 { z.name.clone() } else { format!("{sirens} יישובים") };
            (threat_color(z.threat), title, format!("{sub}  •  לפני {}", paths::elapsed((now - z.alert_ms) / 1000)))
        }
        Kind::Pre => {
            let title = snap.message.as_ref().map(|m| m.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "התרעה מקדימה".to_string());
            let n = snap.count(Kind::Pre);
            (AMBER, title, if z.home { format!("גם ב{}  •  {n} יישובים", z.name) } else { format!("{n} יישובים") })
        }
        Kind::Stay => (rgb(0xE5484D), "המשיכו לשהות במרחב המוגן".to_string(), z.name.clone()),
        Kind::Near => (rgb(0xFF8F40), "הישארו בקרבת המרחב המוגן".to_string(), z.name.clone()),
        Kind::Notice => (rgb(0x5B9BD5), "הודעת פיקוד העורף".to_string(), z.name.clone()),
        Kind::End => (GREEN, "האירוע הסתיים".to_string(), if z.home { format!("גם ב{}", z.name) } else { format!("{} יישובים", snap.count(Kind::End)) }),
    }
}

pub fn draw_panel(p: &Painter, v: &View, map: &mut MapView, i: &mut Input, w: f32, h: f32, scroll: &mut f32, out: &mut Vec<Action>) {
    let all = r(0.0, 0.0, w, h);
    p.fill(all, PANEL);
    let drill = v.snap.zones.iter().any(|z| z.drill) || v.test_run;
    let (accent, title, sub) = panel_look(v);
    let urgent = v.snap.zones.iter().any(|z| z.urgent(v.now_ms));
    let wave = 0.5 + 0.5 * (v.t * TAU / 1.25).sin();
    let mut y = 0.0;
    let head_h = 112.0;
    let head = r(0.0, 0.0, w, head_h);
    let bright = if urgent { 0.12 * wave } else { 0.0 };
    p.gradient(head, 0.0, 0.0, w, 0.0, accent.mix(rgb(0x000000), 0.55), accent.mix(WHITE, bright));
    y += head_h;
    let text_side = head.inset_xy(18.0, 10.0);
    let mut size = 34.0;
    while size > 20.0 && p.gfx.measure(&title, 4000.0, st(size, 800)).0 > w - 90.0 {
        size -= 2.0;
    }
    p.text(&title, r(text_side.x + 56.0, text_side.y, text_side.w - 56.0, 48.0), st(size, 800).middle(), WHITE);
    p.text(&sub, r(text_side.x, text_side.y + 50.0, text_side.w, 24.0), st(17.0, 600).middle(), WHITE.with(0.92));
    if let Some(line) = near_line(v) {
        p.text(&line, r(text_side.x, text_side.y + 74.0, text_side.w, 20.0), st(14.0, 500).middle(), WHITE.with(0.85));
    }
    if x_button(p, i, r(10.0, 10.0, 38.0, 38.0), true) {
        out.push(Action::Close);
    }
    if drill {
        drill_band(p, r(0.0, y, w, 36.0), 17.0);
        y += 36.0;
    }
    if let Some(trouble) = v.health.trouble() {
        p.fill(r(0.0, y, w, 26.0), rgb(0x5A1414));
        p.text(&trouble, r(10.0, y, w - 20.0, 26.0), st(13.0, 600).middle(), WHITE);
        y += 26.0;
    }
    if let Some(m) = v.snap.message.as_ref().filter(|m| !m.body.is_empty() && m.kind != Some(Kind::Siren) && v.snap.zones.iter().any(|z| z.kind != Kind::Siren)) {
        let style = st(15.0, 400).wrap();
        let (_, th) = p.gfx.measure(&m.body, w - 28.0, style);
        let th = th.min(84.0);
        p.fill(r(0.0, y, w, th + 12.0), PANEL2);
        p.text(&m.body, r(14.0, y + 6.0, w - 28.0, th), style, TEXT);
        y += th + 12.0;
    }
    let footer_h = 30.0;
    let body_h = h - y - footer_h;
    let map_h = (body_h * 0.60).round();
    let map_rect = r(0.0, y, w, map_h);
    let _ = map.draw(p.rt, p.brush, p.gfx, v.geo, v.places, v.snap, v.homes, map_rect, v.now_ms, v.t, MapOpts { inset: true, ripples: urgent, city_labels: true });
    y += map_h;
    draw_groups(p, v, i, r(8.0, y + 8.0, w - 16.0, body_h - map_h - 10.0), scroll);
    let footer = r(0.0, h - footer_h, w, footer_h);
    p.fill(footer, rgb(0x0C1319));
    p.text(&delay_line(v), footer.inset_xy(12.0, 0.0), st(12.0, 400).middle(), TEXT3);
    if button(p, i, r(6.0, footer.y + 3.0, 92.0, 24.0), "מסך מלא", 12.0, PANEL3, rgb(0x2D3B49), TEXT) {
        out.push(Action::FullScreenNow);
    }
    p.stroke(all, accent.with(0.8), 1.5);
}

// ---------------------------------------------------------------- the warning strip

pub fn draw_strip(p: &Painter, v: &View, i: &mut Input, w: f32, h: f32, out: &mut Vec<Action>) {
    let all = r(0.0, 0.0, w, h);
    let down = v.health.protection() == Protection::Down;
    let c = if down { rgb(0xB01818) } else { rgb(0x8A5A00) };
    p.fill(all, c);
    let t = v.health.trouble().unwrap_or_default();
    let body = all.inset_xy(14.0, 0.0);
    p.text(&t, r(body.x + 40.0, body.y, body.w - 40.0, body.h), st(16.0, 700).middle(), WHITE);
    if x_button(p, i, r(8.0, (h - 32.0) / 2.0, 32.0, 32.0), true) {
        out.push(Action::CloseStrip);
    }
}

// ---------------------------------------------------------------- the short note at start

pub fn draw_note(p: &Painter, v: &View, w: f32, h: f32) {
    let all = r(0.0, 0.0, w, h);
    p.fill(all, PANEL);
    p.stroke(all, LINE, 1.0);
    let (c, text) = match v.health.protection() {
        Protection::Ok => (GREEN, "אזעקות פועל ומאזין"),
        Protection::Weak => (AMBER, "אזעקות מתחבר…"),
        Protection::Down => (RED, "אזעקות: אין קשר למקור"),
    };
    p.circle(w - 24.0, h / 2.0, 7.0, c);
    p.text(text, r(12.0, 0.0, w - 48.0, h), st(16.0, 600).middle(), TEXT);
}

// ---------------------------------------------------------------- the main window

pub struct MainFrame {
    pub map_rect: R,
    pub edit_rect: Option<R>,
}

fn status_dot(p: &Painter, cx: f32, cy: f32, c: Color, t: f32) {
    let breath = 0.5 + 0.5 * (t * 2.0).sin();
    p.circle(cx, cy, 9.0 + breath * 2.0, c.with(0.2));
    p.circle(cx, cy, 5.5, c);
}

fn source_row(p: &Painter, a: R, name: &str, h: &SourceHealth, detail: String) {
    let ok = h.err.is_empty() && h.ok_at.is_some();
    let c = if ok { GREEN } else if h.ok_at.is_some() { AMBER } else { RED };
    p.circle(a.right() - 8.0, a.cy(), 4.5, c);
    p.text(name, r(a.x, a.y, a.w - 22.0, a.h), st(13.5, 500).middle(), TEXT);
    p.text(&detail, a, st(12.5, 400).end().middle(), TEXT3);
}

fn health_detail(h: &SourceHealth) -> String {
    match h.age_s() {
        Some(age) if h.err.is_empty() => format!("לפני {}  •  {} מ״ש", seconds_ago(age), h.last_ms),
        _ if !h.err.is_empty() => h.err.chars().take(44).collect(),
        _ => "עוד לא ענה".to_string(),
    }
}

pub fn draw_main(p: &Painter, v: &View, map: &mut MapView, i: &mut Input, w: f32, h: f32, tab: u8, search: &str, side_scroll: &mut f32, picked: Option<usize>, hover_place: Option<usize>, out: &mut Vec<Action>) -> MainFrame {
    let all = r(0.0, 0.0, w, h);
    p.fill(all, BG);
    let side_w = 440.0f32.min(w * 0.45);
    let side = all.right_part(side_w);
    let map_rect = all.left_part(w - side_w);
    let urgent = v.snap.zones.iter().any(|z| z.urgent(v.now_ms));
    let _ = map.draw(p.rt, p.brush, p.gfx, v.geo, v.places, v.snap, v.homes, map_rect, v.now_ms, v.t, MapOpts { inset: true, ripples: urgent, city_labels: true });
    // map controls
    let c = r(map_rect.x + 12.0, map_rect.y + 12.0, 38.0, 38.0);
    if button(p, i, c, "+", 22.0, PANEL.with(0.92), PANEL3, TEXT) {
        out.push(Action::ZoomIn);
    }
    if button(p, i, r(c.x, c.y + 44.0, 38.0, 38.0), "−", 22.0, PANEL.with(0.92), PANEL3, TEXT) {
        out.push(Action::ZoomOut);
    }
    if map.user_until.is_some() && button(p, i, r(c.x, c.y + 88.0, 38.0, 38.0), "⌂", 20.0, PANEL.with(0.92), PANEL3, TEXT) {
        out.push(Action::ResetMap);
    }
    // legend
    let leg = [(rgb(0xFF2E2E), "רקטות"), (rgb(0xFF7A1A), "כלי טיס"), (rgb(0xFFC800), "מחבלים"), (AMBER.mix(rgb(0xFFFFFF), 0.25), "מקדימה"), (GREEN, "סיום")];
    let lw = 76.0 * leg.len() as f32;
    let legend = r(map_rect.right() - lw - 10.0, map_rect.bottom() - 34.0, lw, 26.0);
    p.round(legend, 6.0, PANEL.with(0.85));
    for (k, (col, name)) in leg.iter().enumerate() {
        let cell = r(legend.right() - 76.0 * (k as f32 + 1.0), legend.y, 76.0, 26.0);
        p.round(r(cell.right() - 15.0, cell.cy() - 5.0, 10.0, 10.0), 2.0, *col);
        p.text(name, r(cell.x, cell.y, cell.w - 19.0, cell.h), st(11.5, 400).middle(), TEXT2);
    }
    // what is under the mouse
    if let Some(place) = hover_place {
        let pl = &v.places.list[place];
        let line2 = format!("{}  •  {} שניות למרחב מוגן", v.places.area_name(pl.area), pl.countdown);
        let tw = p.gfx.measure(&pl.name, 500.0, st(14.0, 700)).0.max(p.gfx.measure(&line2, 500.0, st(12.0, 400)).0);
        let b = r((i.x + 14.0).min(map_rect.right() - tw - 30.0), (i.y + 16.0).min(map_rect.bottom() - 60.0), tw + 22.0, 50.0);
        p.round(b, 7.0, PANEL.with(0.97));
        p.round_stroke(b, 7.0, LINE, 1.0);
        p.text(&pl.name, r(b.x + 11.0, b.y + 5.0, b.w - 22.0, 22.0), st(14.0, 700).middle(), TEXT);
        p.text(&line2, r(b.x + 11.0, b.y + 26.0, b.w - 22.0, 18.0), st(12.0, 400).middle(), TEXT2);
    }

    // the side column
    p.fill(side, PANEL);
    p.line(side.x, 0.0, side.x, h, LINE, 1.0);
    let col = side.inset_xy(16.0, 0.0);
    let mut y = 12.0;
    p.text("אזעקות", r(col.x, y, col.w, 34.0), st(26.0, 800).middle(), TEXT);
    p.text(&paths::clock(v.now_ms), r(col.x, y, col.w, 34.0), st(18.0, 400).num().end().middle(), TEXT2);
    y += 40.0;
    // the state of the watch: first and in plain words
    let prot = v.health.protection();
    let card_h = if v.output_note.is_some() { 84.0 } else { 62.0 };
    let card = r(col.x, y, col.w, card_h);
    let (c, line1) = match prot {
        Protection::Ok => (GREEN, "מאזין לאזעקות — הכל תקין".to_string()),
        Protection::Weak => (AMBER, v.health.trouble().unwrap_or_default()),
        Protection::Down => (RED, v.health.trouble().unwrap_or_default()),
    };
    p.round(card, 10.0, c.mix(PANEL, 0.86));
    p.round_stroke(card, 10.0, c.with(0.55), 1.0);
    status_dot(p, card.right() - 20.0, card.y + 22.0, c, v.t);
    p.text(&line1, r(card.x + 10.0, card.y + 6.0, card.w - 44.0, 32.0), st(15.0, 700).middle(), TEXT);
    let line2 = match v.health.sirens.age_s() {
        Some(age) => {
            let cache = v.health.sirens.cache_age.map(|c| format!("  •  מטמון {c} ש׳")).unwrap_or_default();
            format!("קריאה אחרונה לפני {}{}", seconds_ago(age), cache)
        }
        None => "עוד אין תשובה מהמקור".to_string(),
    };
    p.text(&line2, r(card.x + 10.0, card.y + 34.0, card.w - 44.0, 22.0), st(12.5, 400).middle(), TEXT2);
    if let Some(n) = v.output_note {
        p.text(n, r(card.x + 10.0, card.y + 56.0, card.w - 20.0, 22.0), st(13.0, 600).middle(), AMBER);
    }
    y += card_h + 10.0;

    // tabs
    let names = ["מצב", "היסטוריה", "הגדרות"];
    let tab_w = col.w / 3.0;
    for (k, name) in names.iter().enumerate() {
        let a = r(col.right() - tab_w * (k as f32 + 1.0), y, tab_w, 36.0);
        let on = tab == k as u8;
        let hot = i.inside && a.contains(i.x, i.y);
        if hot {
            i.hand = true;
            if i.clicked && !on {
                out.push(Action::Tab(k as u8));
            }
        }
        p.text(name, a, st(14.5, if on { 700 } else { 500 }).center().middle(), if on { TEXT } else if hot { TEXT2 } else { TEXT3 });
        p.fill(r(a.x + 6.0, a.bottom() - 2.0, a.w - 12.0, 2.0), if on { HOME_BLUE } else { LINE });
    }
    y += 44.0;
    let body = r(col.x, y, col.w, h - y - 8.0);
    let mut frame = MainFrame { map_rect, edit_rect: None };
    match tab {
        0 => tab_state(p, v, i, body, side_scroll, out),
        1 => tab_history(p, v, i, body, side_scroll, picked, out),
        _ => tab_settings(p, v, i, body, search, &mut frame, out),
    }
    frame
}

fn tab_state(p: &Painter, v: &View, i: &mut Input, a: R, scroll: &mut f32, out: &mut Vec<Action>) {
    let mut y = a.y;
    let active = v.snap.active();
    if active > 0 {
        let sirens = v.snap.count(Kind::Siren);
        let line = if sirens > 0 { format!("{sirens} יישובים באזעקה") } else { format!("{active} יישובים בהתרעה") };
        p.text(&line, r(a.x, y, a.w, 30.0), st(20.0, 800).middle(), RED.mix(WHITE, 0.2));
        y += 32.0;
        if let Some(n) = near_line(v) {
            p.text(&n, r(a.x, y, a.w, 22.0), st(14.0, 500).middle(), TEXT2);
            y += 24.0;
        }
        if v.snap.zones.iter().any(|z| z.drill) {
            if button(p, i, r(a.x, y, a.w, 34.0), "סיום תרגיל", 14.0, rgb(0xFFD60A), rgb(0xFFE24D), rgb(0x111111)) {
                out.push(Action::StopDrill);
            }
            y += 40.0;
        }
        let bottom = a.bottom();
        draw_groups(p, v, i, r(a.x, y, a.w, bottom - y), scroll);
        return;
    }
    // quiet
    let last = v.history.iter().find(|h| h.kind == Kind::Siren && !h.drill);
    let line = match last {
        Some(h) => format!("שקט. אזעקה אחרונה: {}", paths::day_and_time(h.time_s * 1000)),
        None => "שקט. אין אזעקות כרגע".to_string(),
    };
    p.text(&line, r(a.x, y, a.w, 26.0), st(16.0, 600).middle(), TEXT);
    y += 26.0;
    if let Some(h) = last {
        let names: Vec<&str> = h.places.iter().take(4).map(|&x| v.places.list[x].name.as_str()).collect();
        let more = if h.places.len() > 4 { format!(" ועוד {}", h.places.len() - 4) } else { String::new() };
        p.text(&format!("{}: {}{}", h.threat.title(), names.join(", "), more), r(a.x, y, a.w, 22.0), st(13.0, 400).middle(), TEXT3);
        y += 26.0;
    }
    y += 6.0;
    section(p, r(a.x, y, a.w, 22.0), "מקורות");
    y += 26.0;
    let row = 26.0;
    source_row(p, r(a.x, y, a.w, row), "צופר: אזעקות", &v.health.sirens, health_detail(&v.health.sirens));
    y += row;
    source_row(p, r(a.x, y, a.w, row), "צופר: מקדימה וסיום", &v.health.messages, health_detail(&v.health.messages));
    y += row;
    source_row(p, r(a.x, y, a.w, row), "צופר: יומן מלא", &v.health.feed, health_detail(&v.health.feed));
    y += row;
    let official = if v.health.official_live {
        health_detail(&v.health.official)
    } else if v.health.official_status == 0 {
        "עוד לא נבדק".to_string()
    } else if v.health.official_status == 403 {
        "סירב לבקשה (403) — מנסה שוב".to_string()
    } else {
        format!("HTTP {}", v.health.official_status)
    };
    let mut off = v.health.official.clone();
    if !v.health.official_live {
        off.ok_at = None;
    }
    let (dot, name) = if v.health.official_live { (GREEN, "פיקוד העורף ישירות") } else { (TEXT3, "פיקוד העורף ישירות") };
    p.circle(a.right() - 8.0, y + row / 2.0, 4.5, dot);
    p.text(name, r(a.x, y, a.w - 22.0, row), st(13.5, 500).middle(), TEXT2);
    p.text(&official, r(a.x, y, a.w, row), st(12.5, 400).end().middle(), TEXT3);
    y += row;
    let hist = &v.health.official_history;
    let hist_detail = if hist.ok_at.is_some() { health_detail(hist) } else if hist.n_bad > 0 { "לא זמין כרגע".to_string() } else { "עוד לא נבדק".to_string() };
    source_row(p, r(a.x, y, a.w, row), "פיקוד העורף: יומן", hist, hist_detail);
    y += row;
    if let Some(n) = v.output_name {
        p.circle(a.right() - 8.0, y + row / 2.0, 4.5, if v.output_note.is_some() { AMBER } else { HOME_BLUE });
        p.text("הצליל יוצא דרך", r(a.x, y, a.w - 22.0, row), st(13.5, 500).middle(), TEXT2);
        p.text(n, r(a.x, y, a.w - 130.0, row), st(12.5, 400).end().middle(), TEXT3);
        y += row;
    }
    y += 4.0;
    if let Some(s) = v.health.clock_skew_s.filter(|s| s.abs() >= 3) {
        p.text(&format!("שעון המחשב סוטה ב-{s} ש׳: הספירה לאחור תהיה פחות מדויקת"), r(a.x, y, a.w, 22.0), st(12.5, 600).middle(), AMBER);
        y += 24.0;
    }
    if !v.health.lists_note.is_empty() {
        p.text(&v.health.lists_note, r(a.x, y, a.w, 22.0), st(12.5, 500).middle(), TEXT2);
        y += 24.0;
    }
    y += 8.0;
    section(p, r(a.x, y, a.w, 22.0), "המקומות שלי");
    y += 26.0;
    if v.homes.is_empty() {
        p.text("לא נבחר מקום: מסך מלא לא יופיע. בחרו בהגדרות.", r(a.x, y, a.w, 24.0), st(13.5, 600).middle(), AMBER);
        y += 28.0;
    }
    for &hm in v.homes {
        let pl = &v.places.list[hm];
        let line = r(a.x, y, a.w, 26.0);
        p.circle(line.right() - 8.0, line.cy(), 4.5, HOME_BLUE);
        p.text(&pl.name, r(line.x, line.y, line.w - 22.0, line.h), st(14.0, 600).middle(), TEXT);
        p.text(&format!("{} שניות למרחב מוגן", pl.countdown), line, st(12.5, 400).end().middle(), TEXT2);
        y += 26.0;
    }
    y += 10.0;
    section(p, r(a.x, y, a.w, 22.0), "תרגיל: נראה בדיוק כמו אזעקה, מסומן כתרגיל");
    y += 28.0;
    let bw = (a.w - 8.0) / 2.0;
    let cells = [
        ("אזעקה אצלי", Action::Drill(Drill::Home)),
        ("אזעקה רחוקה", Action::Drill(Drill::Far)),
        ("מטח גדול", Action::Drill(Drill::Big)),
        ("מקדימה → אזעקה → סיום", Action::Drill(Drill::Sequence)),
    ];
    for (k, (label, act)) in cells.into_iter().enumerate() {
        let cx = if k % 2 == 0 { a.right() - bw } else { a.x };
        let cy = y + (k / 2) as f32 * 42.0;
        if plain_button(p, i, r(cx, cy, bw, 36.0), label) {
            out.push(act);
        }
    }
    y += 84.0;
    if y + 36.0 < a.bottom() && plain_button(p, i, r(a.x, y, a.w, 36.0), "בדיקת צליל (3 שניות)") {
        out.push(Action::TestSound);
    }
}

fn tab_history(p: &Painter, v: &View, i: &mut Input, a: R, scroll: &mut f32, picked: Option<usize>, out: &mut Vec<Action>) {
    if v.history.is_empty() {
        p.text("עוד לא התקבל יומן מהמקור", r(a.x, a.y, a.w, 26.0), st(14.0, 500).middle(), TEXT3);
        return;
    }
    let hovered = i.inside && a.contains(i.x, i.y);
    if hovered && i.wheel != 0.0 {
        *scroll = (*scroll - i.wheel * 70.0).max(0.0);
        i.wheel = 0.0;
    }
    let row_h = 56.0;
    let total = v.history.len() as f32 * (row_h + 4.0);
    *scroll = scroll.min((total - a.h).max(0.0));
    p.clip(a);
    let mut y = a.y - *scroll;
    for (k, h) in v.history.iter().enumerate() {
        if y + row_h > a.y && y < a.bottom() {
            let row = r(a.x, y, a.w, row_h);
            let hot = i.inside && row.contains(i.x, i.y) && a.contains(i.x, i.y);
            let on = picked == Some(k);
            p.round(row, 8.0, if on { PANEL3 } else if hot { PANEL2.mix(PANEL3, 0.5) } else { PANEL2 });
            let c = match h.kind {
                Kind::Siren => threat_color(h.threat),
                Kind::End => GREEN,
                Kind::Pre => AMBER,
                _ => rgb(0x5B9BD5),
            };
            p.round(r(row.right() - 4.0, row.y + 6.0, 4.0, row.h - 12.0), 2.0, c);
            let label = match h.kind {
                Kind::Siren => h.threat.title().to_string(),
                _ if !h.title.is_empty() => h.title.clone(),
                other => other.title().to_string(),
            };
            let head = r(row.x + 10.0, row.y + 6.0, row.w - 24.0, 22.0);
            p.text(&label, head, st(14.0, 700).middle(), TEXT);
            let when = paths::day_and_time(h.time_s * 1000);
            let when = if h.source == Source::Official { format!("{when} · פיקוד העורף") } else { when };
            p.text(&when, head, st(12.5, 400).end().middle(), TEXT2);
            let names: Vec<&str> = h.places.iter().take(5).map(|&x| v.places.list[x].name.as_str()).collect();
            let line = if h.places.is_empty() {
                h.body.clone()
            } else if h.places.len() > 5 {
                format!("{} ועוד {}", names.join(", "), h.places.len() - 5)
            } else {
                names.join(", ")
            };
            p.text(&line, r(row.x + 10.0, row.y + 29.0, row.w - 24.0, 20.0), st(12.5, 400).middle(), TEXT3);
            if hot {
                i.hand = true;
                if i.clicked {
                    out.push(Action::PickHistory(k));
                }
            }
        }
        y += row_h + 4.0;
    }
    p.unclip();
}

fn tab_settings(p: &Painter, v: &View, i: &mut Input, a: R, search: &str, frame: &mut MainFrame, out: &mut Vec<Action>) {
    let s = v.settings;
    let mut y = a.y;
    section(p, r(a.x, y, a.w, 20.0), "המקומות שלי: אזעקה כאן = מסך מלא וסירנה");
    y += 24.0;
    for &hm in v.homes {
        let line = r(a.x, y, a.w, 30.0);
        p.round(line, 7.0, PANEL2);
        p.text(&v.places.list[hm].name, r(line.x + 40.0, line.y, line.w - 52.0, line.h), st(14.0, 600).middle(), TEXT);
        if x_button(p, i, r(line.x + 3.0, line.y + 3.0, 24.0, 24.0), false) {
            out.push(Action::RemoveHome(hm));
        }
        y += 34.0;
    }
    let edit = r(a.x, y, a.w, 34.0);
    p.round(edit, 7.0, PANEL2);
    p.round_stroke(edit, 7.0, LINE, 1.0);
    frame.edit_rect = Some(edit.inset_xy(10.0, 6.0));
    if search.is_empty() {
        p.text("הוספת מקום: הקלידו שם יישוב", edit.inset_xy(10.0, 0.0), st(13.5, 400).middle(), TEXT3);
    }
    y += 38.0;
    if !search.is_empty() {
        let hits = v.places.search(search, 6);
        for hit in hits {
            let line = r(a.x, y, a.w, 28.0);
            let hot = i.inside && line.contains(i.x, i.y);
            p.round(line, 6.0, if hot { PANEL3 } else { PANEL2 });
            p.text(&v.places.list[hit].name, line.inset_xy(10.0, 0.0), st(13.5, 500).middle(), TEXT);
            p.text(v.places.area_name(v.places.list[hit].area), line.inset_xy(10.0, 0.0), st(12.0, 400).end().middle(), TEXT3);
            if hot {
                i.hand = true;
                if i.clicked {
                    out.push(Action::AddHome(hit));
                }
            }
            y += 30.0;
        }
        return;
    }
    y += 6.0;
    section(p, r(a.x, y, a.w, 20.0), "אזעקה במקום אחר");
    y += 24.0;
    let bw = (a.w - 12.0) / 3.0;
    for (k, (label, mode)) in [("פאנל בצד", FarMode::Panel), ("מסך מלא", FarMode::Full), ("רק סמל למטה", FarMode::Tray)].into_iter().enumerate() {
        let b = r(a.right() - (bw + 6.0) * (k as f32 + 1.0) + 6.0, y, bw, 34.0);
        let on = s.far_mode == mode;
        if button(p, i, b, label, 13.0, if on { HOME_BLUE.mix(rgb(0x000000), 0.45) } else { PANEL3 }, if on { HOME_BLUE.mix(rgb(0x000000), 0.3) } else { rgb(0x2D3B49) }, TEXT) && !on {
            out.push(Action::FarMode(mode));
        }
    }
    y += 42.0;
    let row = 32.0;
    if toggle(p, i, r(a.x, y, a.w, row), "צליל קצר גם לאזעקה רחוקה", s.far_sound) {
        out.push(Action::ToggleFarSound);
    }
    y += row;
    if toggle(p, i, r(a.x, y, a.w, row), "בלילה (23:00–07:00) רחוקה בלי צליל", s.quiet_from != s.quiet_to) {
        out.push(Action::ToggleNight);
    }
    y += row;
    if toggle(p, i, r(a.x, y, a.w, row), "מסך מלא על כל המסכים", s.all_monitors) {
        out.push(Action::ToggleAllMonitors);
    }
    y += row;
    if toggle(p, i, r(a.x, y, a.w, row), "הפעלה עם עליית Windows", v.autostart_on) {
        out.push(Action::ToggleAutostart);
    }
    y += row + 6.0;
    for (label, val, minus, plus) in [
        ("עוצמת סירנה ומקדימה", s.vol_home, Action::VolHome(-10), Action::VolHome(10)),
        ("עוצמת צליל לרחוקה", s.vol_far, Action::VolFar(-10), Action::VolFar(10)),
    ] {
        let line = r(a.x, y, a.w, 32.0);
        p.text(label, r(line.x + 130.0, line.y, line.w - 130.0, line.h), st(14.0, 400).middle(), TEXT2);
        if plain_button(p, i, r(line.x, line.y + 2.0, 32.0, 28.0), "−") {
            out.push(minus);
        }
        p.text(&format!("{val}%"), r(line.x + 34.0, line.y, 58.0, line.h), st(14.0, 600).center().middle(), TEXT);
        if plain_button(p, i, r(line.x + 94.0, line.y + 2.0, 32.0, 28.0), "+") {
            out.push(plus);
        }
        y += 36.0;
    }
    y += 4.0;
    let snoozed = v.settings.snooze_until_ms > v.now_ms;
    let label = if snoozed { format!("רחוקות מושתקות עד {} — ביטול", &paths::clock(v.settings.snooze_until_ms)[..5]) } else { "השתק אזעקות רחוקות לשעה".to_string() };
    if plain_button(p, i, r(a.x, y, a.w, 34.0), &label) {
        out.push(Action::Snooze(if snoozed { 0 } else { v.now_ms + 3_600_000 }));
    }
    y += 40.0;
    if plain_button(p, i, r(a.x, y, a.w, 34.0), "פתיחת אתר פיקוד העורף בדפדפן") {
        out.push(Action::OpenOfficial);
    }
    y += 42.0;
    let credit = "נתוני התרעות: צופר (tzevaadom.co.il) ופיקוד העורף ישירות (oref.org.il). יישובים: צופר. מפה: Natural Earth. האפליקציה הרשמית בטלפון נשארת הערוץ הראשי.";
    let style = st(11.5, 400).wrap();
    p.text(credit, r(a.x, y, a.w, (a.bottom() - y).max(0.0)), style, TEXT3);
}

/// How the current state should present itself, by its strongest fact
pub fn wants_full(level: Option<Level>, far_mode: FarMode) -> bool {
    match level {
        Some(Level::Home) => true,
        Some(Level::Far) | Some(Level::Near) => far_mode == FarMode::Full,
        _ => false,
    }
}
