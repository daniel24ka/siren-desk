//! The threads that read the sources and hand every new word straight to the engine and the speakers.
//!
//!   sirens    Tzofar /notifications, once a second (the CDN holds it up to 5 s)
//!   messages  Tzofar /system-messages/id/<next>, every 2 s: early warnings and "event ended" (up to 5 s)
//!   feed      Tzofar /ios/feed, every 15 s (up to 15 s): the history, and whatever the two above missed
//!   official  Home Front Command's own live file /warningMessages/alert/Alerts.json, once a second while it
//!             answers. Since Nov 3 2026 it answers only what asks as its own page does (403 otherwise), so these
//!             requests carry the page's headers (net::request_headers; Tzofar's never do). On errors it backs off
//!             (2, 4, 8... up to 60 s; a refusal 403/429 from 15 s up to 5 minutes) and comes back to 1 s on the
//!             first good answer.
//!   official history  /warningMessages/alert/History/AlertsHistory.json (the last day, one row per place),
//!             falling back to /api/v1/AlertHistory/heb?period=lastDay: at start and every minute (every 15 s
//!             while the live file is not answering), 3 tries per round. Its rows fill the gaps the live
//!             sources left: the engine takes what is new and drops a copy of what Tzofar already told (the same
//!             place, kind and threat within its window); the history list shows them only where Tzofar's
//!             feed has no row of the same kind within OFFICIAL_DEDUP_S.
//!
//! Health is honest by construction: a source counts as read only after its answer was parsed as what it
//! should be. A failed read is never "no alert"; an answer a cache has held too long is not fresh.

use crate::engine::{Engine, Event, Source};
use crate::model::{self, Kind, Threat};
use crate::net::{Base, Http};
use crate::paths::{self, now_ms};
use crate::places::{Places, Special};
use crate::sound;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_WAKE: u32 = WM_APP + 1;

/// A source is "fine" while its last parsed answer is this recent (s)
pub const FRESH_S: f32 = 6.0;
/// Past this the screen says it plainly: an alert could be missed (s)
pub const DOWN_S: f32 = 20.0;
/// A cached answer older than this means the source behind the cache may be dead (s)
const STALE_CACHE_S: u32 = 90;

#[derive(Clone, Default, Debug)]
pub struct SourceHealth {
    pub ok_at: Option<Instant>,
    pub bad_since: Option<Instant>,
    pub err: String,
    pub last_ms: u32,
    pub cache_age: Option<u32>,
    pub n_ok: u64,
    pub n_bad: u64,
}

impl SourceHealth {
    fn good(&mut self, took_ms: u32, cache_age: Option<u32>) {
        self.ok_at = Some(Instant::now());
        self.bad_since = None;
        self.err.clear();
        self.last_ms = took_ms;
        self.cache_age = cache_age;
        self.n_ok += 1;
    }

    fn bad(&mut self, err: String) {
        if self.bad_since.is_none() {
            self.bad_since = Some(Instant::now());
        }
        self.err = err;
        self.n_bad += 1;
    }

    pub fn age_s(&self) -> Option<f32> {
        self.ok_at.map(|t| t.elapsed().as_secs_f32())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Protection {
    /// the fast path answers and its answers are fresh
    Ok,
    /// it answered recently, or its answers look old: an alert may come late
    Weak,
    /// no answer for DOWN_S: an alert could be missed
    Down,
}

#[derive(Clone, Default, Debug)]
pub struct Health {
    pub sirens: SourceHealth,
    pub messages: SourceHealth,
    pub feed: SourceHealth,
    pub official: SourceHealth,
    pub official_history: SourceHealth,
    pub official_status: u32,
    pub official_checked_ms: i64,
    pub official_live: bool,
    pub clock_skew_s: Option<i64>,
    pub lists_note: String,
    pub started: Option<Instant>,
}

impl Health {
    pub fn protection(&self) -> Protection {
        let age = self.sirens.age_s();
        let official = if self.official_live { self.official.age_s() } else { None };
        let best = match (age, official) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        match best {
            None => {
                // (just started: not "down" before the first answer had time to come)
                let young = self.started.map(|s| s.elapsed().as_secs_f32() < DOWN_S).unwrap_or(true);
                if young {
                    Protection::Weak
                } else {
                    Protection::Down
                }
            }
            Some(a) if a >= DOWN_S => Protection::Down,
            Some(a) if a >= FRESH_S => Protection::Weak,
            Some(_) => {
                if self.sirens.cache_age.unwrap_or(0) > STALE_CACHE_S {
                    Protection::Weak
                } else {
                    Protection::Ok
                }
            }
        }
    }

    /// In plain words, for the screen; None when all is well
    pub fn trouble(&self) -> Option<String> {
        match self.protection() {
            Protection::Down => Some(match self.sirens.age_s() {
                Some(a) => format!("אין קשר למקור ההתרעות כבר {} — אזעקה לא תוצג", paths::elapsed(a as i64)),
                None => "אין קשר למקור ההתרעות — אזעקה לא תוצג".to_string(),
            }),
            Protection::Weak => {
                if self.sirens.cache_age.unwrap_or(0) > STALE_CACHE_S {
                    Some("המקור מחזיר תשובה ישנה — אזעקה עלולה להתעכב".to_string())
                } else if self.started.map(|s| s.elapsed().as_secs_f32() < DOWN_S).unwrap_or(true) && self.sirens.ok_at.is_none() {
                    Some("מתחבר למקור ההתרעות…".to_string())
                } else {
                    Some("הקשר למקור ההתרעות לא יציב — אזעקה עלולה להתעכב".to_string())
                }
            }
            Protection::Ok => {
                let far_gone = |h: &SourceHealth| h.age_s().map(|a| a > 120.0).unwrap_or(false);
                if far_gone(&self.messages) && far_gone(&self.feed) {
                    Some("התרעות מקדימות והודעות סיום לא מתקבלות כרגע".to_string())
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct HistItem {
    pub id: String,
    /// which source this row came from (Feed: Tzofar's, Official: Home Front Command's history)
    pub source: Source,
    pub time_s: i64,
    pub kind: Kind,
    pub threat: Threat,
    pub drill: bool,
    pub places: Vec<usize>,
    pub unknown: Vec<String>,
    pub title: String,
    pub body: String,
}

pub struct Hub {
    pub engine: Mutex<Engine>,
    pub health: Mutex<Health>,
    /// the list the screen shows: Tzofar's feed and the official history together (publish_history)
    pub history: Mutex<Vec<HistItem>>,
    pub tzofar_history: Mutex<Vec<HistItem>>,
    pub official_history: Mutex<Vec<HistItem>>,
    pub places: Arc<Places>,
    pub tzofar: Base,
    pub tzofar_site: Base,
    pub official: Base,
    pub hwnd: AtomicIsize,
    pub stop: AtomicBool,
    /// the highest message id the feed has shown
    pub top_message: AtomicI64,
    /// the moment the last alert was read, for the delay figures
    pub rehearsal_id: AtomicI64,
}

impl Hub {
    pub fn poke(&self) {
        let h = self.hwnd.load(Ordering::Relaxed);
        if h != 0 {
            unsafe {
                let _ = PostMessageW(Some(HWND(h as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// One new word: to the engine, to the speakers and the record, and wake the screen
    pub fn deliver(&self, ev: Event) {
        let now = now_ms();
        let d = self.engine.lock().unwrap().apply(&ev, now);
        if d.stop_siren {
            sound::stop_siren();
        }
        if d.changes == 0 {
            return;
        }
        if !ev.drill {
            // a rehearsal's thread sees this and stops
            self.rehearsal_id.fetch_add(1, Ordering::SeqCst);
        }
        if let Some(cue) = d.cue {
            sound::play(cue);
        }
        let names: Vec<String> = ev.places.iter().take(60).map(|&i| self.places.list[i].name.clone()).chain(ev.unknown.iter().cloned()).collect();
        let delay = ev.time_s.map(|t| now - t * 1000);
        let line = serde_json::json!({
            "t": now, "clock": paths::clock(now), "source": ev.source, "kind": ev.kind, "threat": ev.threat, "drill": ev.drill,
            "source_time_s": ev.time_s, "delay_ms": delay, "level": d.level, "new": d.new_places, "changes": d.changes,
            "places_total": ev.places.len() + ev.unknown.len(), "places": names, "unknown": ev.unknown,
        });
        paths::record_alert(&line.to_string());
        paths::log(&format!(
            "{:?} {:?}{} from {:?}: {} places, {} new, level {:?}, delay {:?} ms",
            ev.kind,
            ev.threat,
            if ev.drill { " (drill)" } else { "" },
            ev.source,
            ev.places.len() + ev.unknown.len(),
            d.new_places,
            d.level,
            delay
        ));
        self.poke();
    }

    /// The two histories as one list for the screen, newest first
    pub fn publish_history(&self) {
        let merged = {
            let tz = self.tzofar_history.lock().unwrap();
            let off = self.official_history.lock().unwrap();
            merge_history(&tz, &off)
        };
        *self.history.lock().unwrap() = merged;
        self.poke();
    }

    fn resolve(&self, names: &[String]) -> Option<(Vec<usize>, Vec<String>)> {
        let mut places = Vec::with_capacity(names.len());
        let mut unknown = vec![];
        let mut real = false;
        for n in names {
            match Places::special(n) {
                Some(Special::Test) => {}
                Some(Special::AllCountry) => {
                    places = (0..self.places.list.len()).collect();
                    real = true;
                }
                None => {
                    real = true;
                    match self.places.find(n) {
                        Some(i) => places.push(i),
                        None => unknown.push(n.clone()),
                    }
                }
            }
        }
        places.sort_unstable();
        places.dedup();
        if real {
            Some((places, unknown))
        } else {
            None
        }
    }
}

fn sleep_rest(started: Instant, period: Duration, hub: &Hub) {
    let mut left = period.saturating_sub(started.elapsed());
    while left > Duration::ZERO && !hub.stop.load(Ordering::Relaxed) {
        let step = left.min(Duration::from_millis(250));
        std::thread::sleep(step);
        left = left.saturating_sub(step);
    }
}

#[derive(Deserialize)]
struct TzAlert {
    #[serde(rename = "notificationId", default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    time: Option<i64>,
    #[serde(default)]
    threat: Option<i64>,
    #[serde(rename = "isDrill", default)]
    drill: Option<bool>,
    #[serde(default)]
    cities: Option<Vec<String>>,
}

#[derive(Deserialize, Clone)]
struct TzMessage {
    id: i64,
    #[serde(default)]
    time: Option<i64>,
    #[serde(rename = "titleHe", default)]
    title: Option<String>,
    #[serde(rename = "bodyHe", default)]
    body: Option<String>,
    #[serde(rename = "instructionType", default)]
    instruction_type: Option<i64>,
    #[serde(rename = "citiesIds", default)]
    cities: Option<Vec<u32>>,
    #[serde(rename = "areasIds", default)]
    areas: Option<Vec<u16>>,
}

#[derive(Deserialize)]
struct TzGroup {
    id: i64,
    #[serde(default)]
    alerts: Vec<TzAlert>,
}

#[derive(Deserialize)]
struct TzFeed {
    #[serde(rename = "alertsHistory", default)]
    alerts: Vec<TzGroup>,
    #[serde(default)]
    instructions: Vec<TzMessage>,
    #[serde(rename = "systemMessages", default)]
    system: Vec<TzMessage>,
    #[serde(rename = "citiesVersion", default)]
    cities_version: Option<u32>,
}

#[derive(Deserialize)]
struct OfficialAlert {
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    cat: Option<serde_json::Value>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    desc: Option<String>,
    #[serde(default)]
    data: Vec<String>,
}

fn text_of(v: &Option<serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Tzofar's writing for "bold": *word*
pub fn plain_body(s: &str) -> String {
    s.replace('*', "")
}

fn alert_event(hub: &Hub, a: &TzAlert, source: Source, key: String, read_at: Instant) -> Option<Event> {
    let names = a.cities.as_ref()?;
    let (places, unknown) = hub.resolve(names)?;
    let threat_n = a.threat.unwrap_or(8);
    let drill = a.drill.unwrap_or(false) || threat_n == 9 || names.iter().any(|n| n.contains("תרגיל"));
    Some(Event {
        source,
        key,
        kind: Kind::Siren,
        threat: Threat::from_tzofar(threat_n),
        drill,
        time_s: a.time,
        title: String::new(),
        body: String::new(),
        places,
        unknown,
        read_at,
    })
}

fn message_event(hub: &Hub, m: &TzMessage, source: Source, read_at: Instant) -> Option<Event> {
    let title = m.title.clone().unwrap_or_default();
    let body = plain_body(m.body.as_deref().unwrap_or(""));
    let reading = model::read_tzofar_message(m.instruction_type, &title, &body)?;
    let mut places: Vec<usize> = m.cities.iter().flatten().filter_map(|&id| hub.places.by_id(id)).collect();
    if places.is_empty() {
        for &a in m.areas.iter().flatten() {
            places.extend(hub.places.in_area(a));
        }
    }
    places.sort_unstable();
    places.dedup();
    Some(Event {
        source,
        key: format!("msg:{}", m.id),
        kind: reading.kind,
        threat: reading.threat,
        drill: reading.drill,
        time_s: m.time,
        title,
        body,
        places,
        unknown: vec![],
        read_at,
    })
}

/// /notifications: [] when quiet, else [{notificationId, time, threat, isDrill, cities}]
pub fn parse_sirens(bytes: &[u8]) -> Result<Vec<serde_json::Value>, String> {
    let v: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("not JSON: {e}"))?;
    match v {
        serde_json::Value::Array(a) => Ok(a),
        _ => Err("not a list".to_string()),
    }
}

fn sirens_loop(hub: Arc<Hub>) {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    let http = match Http::new(3000) {
        Ok(h) => h,
        Err(e) => {
            paths::log(&format!("sirens: no HTTP: {e}"));
            return;
        }
    };
    let mut was_bad = false;
    let mut period = Duration::from_millis(1000);
    while !hub.stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        match http.get(&hub.tzofar, "/notifications", None) {
            Ok(r) if r.status == 200 => {
                let read_at = Instant::now();
                match parse_sirens(&r.body) {
                    Ok(items) => {
                        for item in items {
                            let Ok(a) = serde_json::from_value::<TzAlert>(item) else { continue };
                            // (a list that grows under one id is news; an item with no id must not hide the next)
                            let key = format!("n:{}:{}:{}:{}", text_of(&a.id), a.time.unwrap_or(0), a.threat.unwrap_or(8), a.cities.as_ref().map(|c| c.len()).unwrap_or(0));
                            if let Some(ev) = alert_event(&hub, &a, Source::Sirens, key, read_at) {
                                hub.deliver(ev);
                            }
                        }
                        let mut h = hub.health.lock().unwrap();
                        h.sirens.good(r.took_ms, r.age);
                        if let Some(d) = r.date_ms {
                            h.clock_skew_s = Some((now_ms() - d - r.age.unwrap_or(0) as i64 * 1000) / 1000);
                        }
                        if was_bad {
                            paths::log("sirens: answering again");
                        }
                        was_bad = false;
                        period = Duration::from_millis(1000);
                    }
                    Err(e) => {
                        let head = String::from_utf8_lossy(&r.body[..r.body.len().min(60)]).into_owned();
                        fail(&hub, &mut was_bad, format!("{e} | {head}"));
                    }
                }
            }
            Ok(r) => {
                // a refusal is told, and not pushed harder
                if r.status == 403 || r.status == 429 {
                    period = Duration::from_millis(5000);
                }
                fail(&hub, &mut was_bad, format!("HTTP {}", r.status));
            }
            Err(e) => fail(&hub, &mut was_bad, e),
        }
        let stop_siren = hub.engine.lock().unwrap().tick(now_ms());
        if stop_siren {
            sound::stop_siren();
        }
        sleep_rest(started, period, &hub);
    }
}

fn fail(hub: &Hub, was_bad: &mut bool, err: String) {
    if !*was_bad {
        paths::log(&format!("sirens: no good answer: {err}"));
    }
    *was_bad = true;
    hub.health.lock().unwrap().sirens.bad(err);
}

fn messages_loop(hub: Arc<Hub>) {
    let http = match Http::new(4000) {
        Ok(h) => h,
        Err(e) => {
            paths::log(&format!("messages: no HTTP: {e}"));
            return;
        }
    };
    let mut next: i64 = 0;
    let mut proved_at: Option<Instant> = None;
    let mut path_ok = true;
    while !hub.stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let top = hub.top_message.load(Ordering::Relaxed);
        if top >= next {
            next = top + 1;
        }
        if next <= 1 {
            // (the feed has not told yet where the numbers stand)
            sleep_rest(started, Duration::from_millis(500), &hub);
            continue;
        }
        // "nothing new" looks the same as a dead address, so the newest known message must still be there
        if proved_at.map(|t| t.elapsed() > Duration::from_secs(600)).unwrap_or(true) && top > 0 {
            match http.get(&hub.tzofar, &format!("/system-messages/id/{top}"), None) {
                Ok(r) if r.status == 200 && serde_json::from_slice::<TzMessage>(&r.body).map(|m| m.id == top).unwrap_or(false) => {
                    if !path_ok {
                        paths::log("messages: the address works again");
                    }
                    path_ok = true;
                }
                Ok(r) => {
                    if path_ok {
                        paths::log(&format!("messages: known message {top} gives HTTP {}", r.status));
                    }
                    path_ok = false;
                }
                Err(_) => {}
            }
            proved_at = Some(Instant::now());
        }
        match http.get(&hub.tzofar, &format!("/system-messages/id/{next}"), None) {
            Ok(r) if r.status == 200 => {
                let read_at = Instant::now();
                match serde_json::from_slice::<TzMessage>(&r.body) {
                    Ok(m) => {
                        if let Some(ev) = message_event(&hub, &m, Source::Messages, read_at) {
                            hub.deliver(ev);
                        }
                        next += 1;
                        hub.health.lock().unwrap().messages.good(r.took_ms, r.age);
                        continue;
                    }
                    Err(e) => hub.health.lock().unwrap().messages.bad(format!("not a message: {e}")),
                }
            }
            Ok(r) if r.status == 404 && path_ok => hub.health.lock().unwrap().messages.good(r.took_ms, r.age),
            Ok(r) => hub.health.lock().unwrap().messages.bad(format!("HTTP {}", r.status)),
            Err(e) => hub.health.lock().unwrap().messages.bad(e),
        }
        sleep_rest(started, Duration::from_millis(2000), &hub);
    }
}

fn feed_items(hub: &Hub, feed: &TzFeed) -> Vec<HistItem> {
    let mut items = vec![];
    for g in &feed.alerts {
        // a group's alerts of one threat make one row
        let mut by_threat: HashMap<i64, (i64, bool, Vec<String>)> = HashMap::new();
        for a in &g.alerts {
            let t = a.threat.unwrap_or(8);
            let row = by_threat.entry(t).or_insert((0, false, vec![]));
            row.0 = row.0.max(a.time.unwrap_or(0));
            row.1 |= a.drill.unwrap_or(false);
            row.2.extend(a.cities.iter().flatten().cloned());
        }
        for (t, (time, drill, names)) in by_threat {
            let Some((places, unknown)) = hub.resolve(&names) else { continue };
            items.push(HistItem {
                id: format!("a{}:{t}", g.id),
                source: Source::Feed,
                time_s: time,
                kind: Kind::Siren,
                threat: Threat::from_tzofar(t),
                drill: drill || t == 9,
                places,
                unknown,
                title: String::new(),
                body: String::new(),
            });
        }
    }
    for m in feed.instructions.iter().chain(feed.system.iter()) {
        let title = m.title.clone().unwrap_or_default();
        let body = plain_body(m.body.as_deref().unwrap_or(""));
        let Some(r) = model::read_tzofar_message(m.instruction_type, &title, &body) else { continue };
        let mut places: Vec<usize> = m.cities.iter().flatten().filter_map(|&id| hub.places.by_id(id)).collect();
        places.sort_unstable();
        places.dedup();
        items.push(HistItem {
            id: format!("m{}", m.id),
            source: Source::Feed,
            time_s: m.time.unwrap_or(0),
            kind: r.kind,
            threat: r.threat,
            drill: r.drill,
            places,
            unknown: vec![],
            title,
            body,
        });
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.time_s));
    items.dedup_by(|a, b| a.id == b.id);
    items
}

fn take_feed(hub: &Hub, feed: TzFeed, read_at: Instant) {
    let top = feed.instructions.iter().chain(feed.system.iter()).map(|m| m.id).max().unwrap_or(0);
    hub.top_message.fetch_max(top, Ordering::Relaxed);
    let items = feed_items(hub, &feed);
    *hub.tzofar_history.lock().unwrap() = items;
    hub.publish_history();
    // what happened lately, oldest first: "ended" after its alert, whatever order the lists come in
    let now_s = now_ms() / 1000;
    let mut events: Vec<(i64, u8, Event)> = vec![];
    for g in &feed.alerts {
        for a in &g.alerts {
            let t = a.time.unwrap_or(0);
            if now_s - t > 3700 {
                continue;
            }
            let key = format!("feed:{}:{}:{}:{}", g.id, t, a.threat.unwrap_or(8), a.cities.as_ref().map(|c| c.len()).unwrap_or(0));
            if let Some(ev) = alert_event(hub, a, Source::Feed, key, read_at) {
                events.push((t, 0, ev));
            }
        }
    }
    for m in feed.instructions.iter() {
        let t = m.time.unwrap_or(0);
        if now_s - t > 3700 {
            continue;
        }
        if let Some(ev) = message_event(hub, m, Source::Feed, read_at) {
            events.push((t, 1, ev));
        }
    }
    events.sort_by_key(|e| (e.0, e.1));
    for (_, _, ev) in events {
        hub.deliver(ev);
    }
    if let Some(v) = feed.cities_version {
        if v > hub.places.cities_version {
            fetch_new_lists(hub, v);
        }
    }
}

/// The feed says the place lists have a newer version: keep a checked copy for the next start
fn fetch_new_lists(hub: &Hub, version: u32) {
    let already = hub.health.lock().unwrap().lists_note.clone();
    if !already.is_empty() {
        return;
    }
    let result = (|| -> Result<(), String> {
        let http = Http::new(15000)?;
        let cities = http.get(&hub.tzofar_site, &format!("/static/cities.json?v={version}"), None)?;
        let polygons = http.get(&hub.tzofar_site, &format!("/static/polygons.json?v={version}"), None)?;
        if cities.status != 200 || polygons.status != 200 {
            return Err(format!("HTTP {} / {}", cities.status, polygons.status));
        }
        let (c, p) = (String::from_utf8_lossy(&cities.body), String::from_utf8_lossy(&polygons.body));
        let checked = Places::parse(&c, &p)?;
        if checked.list.len() < hub.places.list.len() / 2 {
            return Err(format!("only {} places", checked.list.len()));
        }
        let dir = paths::data_dir().join("lists");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        paths::write_atomic(&dir.join("polygons.json"), &polygons.body).map_err(|e| e.to_string())?;
        paths::write_atomic(&dir.join("cities.json"), &cities.body).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let note = match result {
        Ok(()) => format!("רשימת יישובים חדשה (גרסה {version}) תיטען בהפעלה הבאה"),
        Err(e) => {
            paths::log(&format!("new place lists not taken: {e}"));
            format!("רשימת יישובים {version} לא נטענה")
        }
    };
    paths::log(&note);
    hub.health.lock().unwrap().lists_note = note;
}

fn feed_loop(hub: Arc<Hub>) {
    let http = match Http::new(8000) {
        Ok(h) => h,
        Err(e) => {
            paths::log(&format!("feed: no HTTP: {e}"));
            return;
        }
    };
    let mut etag: Option<String> = None;
    let mut body_ok = false;
    while !hub.stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        match http.get(&hub.tzofar, "/ios/feed", etag.as_deref()) {
            Ok(r) if r.status == 304 && body_ok => hub.health.lock().unwrap().feed.good(r.took_ms, r.age),
            Ok(r) if r.status == 200 => {
                let read_at = Instant::now();
                match serde_json::from_slice::<TzFeed>(&r.body) {
                    Ok(feed) if !feed.alerts.is_empty() || !feed.instructions.is_empty() => {
                        take_feed(&hub, feed, read_at);
                        etag = r.etag;
                        body_ok = true;
                        hub.health.lock().unwrap().feed.good(r.took_ms, r.age);
                    }
                    Ok(_) => hub.health.lock().unwrap().feed.bad("empty feed".to_string()),
                    Err(e) => hub.health.lock().unwrap().feed.bad(format!("not the feed: {e}")),
                }
            }
            Ok(r) => hub.health.lock().unwrap().feed.bad(format!("HTTP {}", r.status)),
            Err(e) => hub.health.lock().unwrap().feed.bad(e),
        }
        sleep_rest(started, Duration::from_secs(15), &hub);
    }
}

/// Parses the official live file: empty when quiet
pub fn parse_official(raw: &[u8]) -> Result<Vec<(String, String, String, String, Vec<String>)>, String> {
    let text = model::decode_feed(raw);
    if text.is_empty() || text == "[]" || text == "{}" {
        return Ok(vec![]);
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("not JSON: {e}: {}", &text.chars().take(60).collect::<String>()))?;
    let list = match v {
        serde_json::Value::Array(a) => a,
        obj @ serde_json::Value::Object(_) => vec![obj],
        _ => return Err("not an object".to_string()),
    };
    let mut out = vec![];
    for item in list {
        let a: OfficialAlert = serde_json::from_value(item).map_err(|e| e.to_string())?;
        out.push((text_of(&a.id), text_of(&a.cat), a.title.unwrap_or_default(), a.desc.unwrap_or_default(), a.data));
    }
    Ok(out)
}

/// The live file asked once a second while it answers
const OFFICIAL_LIVE_MS: u64 = 1000;
/// History rows within this of a Tzofar row of the same kind, threat and place are the same alert (s)
pub const OFFICIAL_DEDUP_S: i64 = 180;
/// History rows this recent still go to the engine: the gaps the live sources left (s)
const OFFICIAL_HISTORY_RECENT_S: i64 = 3700;

/// How long to wait after `fails` failures in a row of the live file; `refused`: the last was 403/429
pub fn official_backoff(fails: u32, refused: bool) -> Duration {
    if fails == 0 {
        return Duration::from_millis(OFFICIAL_LIVE_MS);
    }
    let n = (fails - 1).min(10);
    if refused {
        Duration::from_secs((15u64 << n).min(300))
    } else {
        Duration::from_secs((2u64 << n).min(60))
    }
}

/// One row of the official history: one place, one alert
#[derive(Clone, Debug, PartialEq)]
pub struct OfficialRow {
    pub time_s: i64,
    pub name: String,
    pub cat: String,
    pub title: String,
}

/// Days from 1970-01-01 to a civil date (proleptic Gregorian)
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// 0 = Sunday
fn weekday(days: i64) -> i64 {
    (days + 4).rem_euclid(7)
}

fn last_sunday(y: i64, m: i64) -> i64 {
    let next = if m == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, m + 1, 1) };
    let last = next - 1;
    last - weekday(last)
}

/// Israel's clock (the official history's times) -> Unix seconds, whatever this PC's own zone is.
/// Summer time (UTC+3) from the Friday before March's last Sunday 02:00 to October's last Sunday 02:00, else UTC+2.
pub fn israel_to_unix(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    let day = days_from_civil(y, mo, d);
    let local = day * 86_400 + h * 3600 + mi * 60 + s;
    let start = (last_sunday(y, 3) - 2) * 86_400 + 2 * 3600;
    let end = last_sunday(y, 10) * 86_400 + 2 * 3600;
    let offset = if local >= start && local < end { 3 } else { 2 };
    local - offset * 3600
}

/// "2026-10-19 20:31:16", "2026-10-19T20:31:16", "19.10.2026" + "20:31:16"
fn parse_israel_time(date: &str, time: Option<&str>) -> Option<i64> {
    let date = date.trim();
    let (d, t) = match time {
        Some(t) => (date.split(['T', ' ']).next()?, t.trim()),
        None => {
            let mut it = date.splitn(2, ['T', ' ']);
            (it.next()?, it.next().unwrap_or("00:00:00"))
        }
    };
    let nums = |s: &str, sep: char| -> Option<Vec<i64>> { s.split(sep).map(|x| x.trim().parse::<i64>().ok()).collect() };
    let (y, mo, da) = if d.contains('-') {
        let v = nums(d, '-')?;
        (*v.first()?, *v.get(1)?, *v.get(2)?)
    } else {
        let v = nums(d, '.')?;
        (*v.get(2)?, *v.get(1)?, *v.first()?)
    };
    let t = t.split(['.', 'Z', '+']).next()?;
    let tv = nums(t, ':')?;
    let (h, mi, s) = (*tv.first()?, *tv.get(1).unwrap_or(&0), *tv.get(2).unwrap_or(&0));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&da) || !(0..24).contains(&h) || !(0..60).contains(&mi) || !(0..61).contains(&s) {
        return None;
    }
    Some(israel_to_unix(y, mo, da, h, mi, s))
}

fn str_of(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn history_row(v: &serde_json::Value) -> Option<OfficialRow> {
    let o = v.as_object()?;
    let name = str_of(o.get("data")).trim().to_string();
    if name.is_empty() {
        return None;
    }
    let time_s = if let Some(d) = o.get("alertDate").and_then(|x| x.as_str()) {
        parse_israel_time(d, None)?
    } else if let (Some(d), Some(t)) = (o.get("date").and_then(|x| x.as_str()), o.get("time").and_then(|x| x.as_str())) {
        parse_israel_time(d, Some(t))?
    } else {
        parse_israel_time(o.get("dateTime")?.as_str()?, None)?
    };
    let title = if o.contains_key("title") { str_of(o.get("title")) } else { str_of(o.get("category_Desc")) };
    Some(OfficialRow { time_s, name, cat: str_of(o.get("category")), title: title.trim().to_string() })
}

/// The official history, either form: AlertsHistory.json's list [{alertDate, title, data, category}], or
/// /api/v1/AlertHistory's {alarmsGroupedByDate: [{key, value: [{data, date, time, category, category_Desc}]}]}.
/// Empty when quiet (the file is then a bare line break). Rows that make no sense are left out, not guessed.
pub fn parse_official_history(raw: &[u8]) -> Result<Vec<OfficialRow>, String> {
    let text = model::decode_feed(raw);
    if text.is_empty() || text == "[]" || text == "{}" {
        return Ok(vec![]);
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("not JSON: {e}: {}", &text.chars().take(60).collect::<String>()))?;
    let rows: Vec<&serde_json::Value> = match &v {
        serde_json::Value::Array(a) => a.iter().collect(),
        serde_json::Value::Object(o) => match o.get("alarmsGroupedByDate") {
            Some(serde_json::Value::Array(days)) => days.iter().filter_map(|d| d.get("value").and_then(|x| x.as_array())).flatten().collect(),
            _ => return Err("not the history".to_string()),
        },
        _ => return Err("not a list".to_string()),
    };
    let mut out: Vec<OfficialRow> = rows.into_iter().filter_map(history_row).collect();
    out.sort_by(|a, b| a.time_s.cmp(&b.time_s).then_with(|| a.name.cmp(&b.name)));
    out.dedup();
    Ok(out)
}

/// The rows of one alert: the same wording within a minute of its first row
#[derive(Clone, Debug)]
pub struct OfficialGroup {
    pub time_s: i64,
    pub cat: String,
    pub title: String,
    pub names: Vec<String>,
}

pub fn group_official(rows: &[OfficialRow]) -> Vec<OfficialGroup> {
    let mut sorted: Vec<&OfficialRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.time_s);
    let mut groups: Vec<OfficialGroup> = vec![];
    for r in sorted {
        let found = groups.iter_mut().rev().take(16).find(|g| g.cat == r.cat && g.title == r.title && r.time_s - g.time_s <= 60);
        match found {
            Some(g) => {
                if !g.names.contains(&r.name) {
                    g.names.push(r.name.clone());
                }
            }
            None => groups.push(OfficialGroup { time_s: r.time_s, cat: r.cat.clone(), title: r.title.clone(), names: vec![r.name.clone()] }),
        }
    }
    groups
}

/// The screen's list: Tzofar's rows, and the official rows Tzofar has no row for (the same kind and threat
/// within OFFICIAL_DEDUP_S sharing a place, or both with no known place), newest first
pub fn merge_history(tz: &[HistItem], off: &[HistItem]) -> Vec<HistItem> {
    let mut out: Vec<HistItem> = tz.to_vec();
    for o in off {
        let copy = tz.iter().any(|t| {
            t.kind == o.kind
                && t.threat == o.threat
                && (t.time_s - o.time_s).abs() <= OFFICIAL_DEDUP_S
                && ((t.places.is_empty() && o.places.is_empty()) || o.places.iter().any(|p| t.places.binary_search(p).is_ok()))
        });
        if !copy {
            out.push(o.clone());
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.time_s));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

fn official_items(hub: &Hub, groups: &[OfficialGroup]) -> Vec<HistItem> {
    let mut items = vec![];
    for g in groups {
        let Some((places, unknown)) = hub.resolve(&g.names) else { continue };
        let reading = model::read_official(&g.cat, &g.title, "", true);
        items.push(HistItem {
            id: format!("o{}:{}:{}", g.time_s, g.cat, g.names.len()),
            source: Source::Official,
            time_s: g.time_s,
            kind: reading.kind,
            threat: reading.threat,
            drill: reading.drill,
            places,
            unknown,
            // (a siren's row speaks by its threat, as Tzofar's do; any other row keeps its words)
            title: if reading.kind == Kind::Siren { String::new() } else { g.title.clone() },
            body: String::new(),
        });
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.time_s));
    items
}

/// Whatever the history holds that is recent and still on, to the engine, oldest first ("ended" after its alert)
fn take_official_history(hub: &Hub, groups: &[OfficialGroup], read_at: Instant) {
    let now_s = now_ms() / 1000;
    let mut events: Vec<(i64, u8, Event)> = vec![];
    for g in groups {
        if now_s - g.time_s > OFFICIAL_HISTORY_RECENT_S {
            continue;
        }
        let Some((places, unknown)) = hub.resolve(&g.names) else { continue };
        let reading = model::read_official(&g.cat, &g.title, "", true);
        let order = if reading.kind == Kind::Siren { 0 } else { 1 };
        events.push((
            g.time_s,
            order,
            Event {
                source: Source::Official,
                key: format!("offh:{}:{}:{}", g.time_s, g.cat, g.names.len()),
                kind: reading.kind,
                threat: reading.threat,
                drill: reading.drill,
                time_s: Some(g.time_s),
                title: if reading.kind == Kind::Siren { String::new() } else { g.title.clone() },
                body: String::new(),
                places,
                unknown,
                read_at,
            },
        ));
    }
    events.sort_by_key(|e| (e.0, e.1));
    for (_, _, ev) in events {
        hub.deliver(ev);
    }
}

fn official_loop(hub: Arc<Hub>) {
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
    let http = match Http::new(4000) {
        Ok(h) => h,
        Err(e) => {
            paths::log(&format!("official: no HTTP: {e}"));
            return;
        }
    };
    let mut last_status = 0u32;
    let mut fails = 0u32;
    let mut last_err = String::new();
    while !hub.stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        // (the last answer was a refusal: asked again only slowly)
        let mut refused = false;
        let result: Result<(), String> = match http.get(&hub.official, "/warningMessages/alert/Alerts.json", None) {
            Ok(r) => {
                let read_at = Instant::now();
                if r.status != last_status {
                    paths::log(&format!("official file: HTTP {} (asked as its own page)", r.status));
                    last_status = r.status;
                }
                {
                    let mut h = hub.health.lock().unwrap();
                    h.official_status = r.status;
                    h.official_checked_ms = now_ms();
                }
                refused = r.status == 403 || r.status == 429;
                if r.status == 200 {
                    match parse_official(&r.body) {
                        Ok(alerts) => {
                            {
                                let mut h = hub.health.lock().unwrap();
                                h.official.good(r.took_ms, r.age);
                                h.official_live = true;
                            }
                            for (id, cat, title, desc, names) in alerts {
                                let Some((places, unknown)) = hub.resolve(&names) else { continue };
                                let reading = model::read_official(&cat, &title, &desc, false);
                                hub.deliver(Event {
                                    source: Source::Official,
                                    key: format!("off:{id}:{cat}:{}", names.len()),
                                    kind: reading.kind,
                                    threat: reading.threat,
                                    drill: reading.drill,
                                    time_s: None,
                                    title,
                                    body: desc,
                                    places,
                                    unknown,
                                    read_at,
                                });
                            }
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    Err(format!("HTTP {}", r.status))
                }
            }
            Err(e) => {
                hub.health.lock().unwrap().official_checked_ms = now_ms();
                Err(e)
            }
        };
        match result {
            Ok(()) => {
                if fails > 0 {
                    paths::log(&format!("official file: answering again after {fails} failed reads"));
                }
                fails = 0;
                last_err.clear();
            }
            Err(e) => {
                fails += 1;
                if e != last_err {
                    paths::log(&format!("official file: no good answer: {e}"));
                    last_err = e.clone();
                }
                let mut h = hub.health.lock().unwrap();
                h.official.bad(e);
                // (one blip keeps it on the list of live sources; its age still tells how fresh it is)
                if fails >= 3 || refused {
                    h.official_live = false;
                }
            }
        }
        sleep_rest(started, official_backoff(fails, refused), &hub);
    }
}

/// One read of the official history: the file, else the API; 3 tries in all, a pause between them
fn read_official_history(http: &Http, hub: &Hub) -> Result<(Vec<OfficialRow>, u32, &'static str), String> {
    const PATHS: [(&str, &str); 2] = [
        ("/warningMessages/alert/History/AlertsHistory.json", "file"),
        ("/api/v1/AlertHistory/heb?period=lastDay&cities=", "api"),
    ];
    let mut errors = vec![];
    for attempt in 0..3usize {
        if hub.stop.load(Ordering::Relaxed) {
            break;
        }
        if attempt > 0 {
            sleep_rest(Instant::now(), Duration::from_millis(1500 * attempt as u64), hub);
        }
        let (path, name) = PATHS[attempt.min(1)];
        match http.get(&hub.official, path, None) {
            Ok(r) if r.status == 200 => match parse_official_history(&r.body) {
                Ok(rows) => return Ok((rows, r.took_ms, name)),
                Err(e) => errors.push(format!("{name}: {e}")),
            },
            Ok(r) => {
                errors.push(format!("{name}: HTTP {}", r.status));
                if r.status == 429 {
                    break;
                }
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(errors.join("; "))
}

fn official_history_loop(hub: Arc<Hub>) {
    let http = match Http::new(10000) {
        Ok(h) => h,
        Err(e) => {
            paths::log(&format!("official history: no HTTP: {e}"));
            return;
        }
    };
    let mut fails = 0u32;
    let mut was = String::new();
    while !hub.stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        match read_official_history(&http, &hub) {
            Ok((rows, took, from)) => {
                let read_at = Instant::now();
                let groups = group_official(&rows);
                let items = official_items(&hub, &groups);
                let note = format!("{from}: {} rows, {} alerts", rows.len(), items.len());
                if note != was {
                    paths::log(&format!("official history: {note}"));
                    was = note;
                }
                *hub.official_history.lock().unwrap() = items;
                hub.publish_history();
                take_official_history(&hub, &groups, read_at);
                hub.health.lock().unwrap().official_history.good(took, None);
                fails = 0;
            }
            Err(e) => {
                if fails == 0 {
                    paths::log(&format!("official history: no good answer: {e}"));
                }
                fails += 1;
                hub.health.lock().unwrap().official_history.bad(e);
            }
        }
        let live = hub.health.lock().unwrap().official_live;
        let period = if fails > 0 {
            Duration::from_secs((30u64 << (fails - 1).min(4)).min(300))
        } else if live {
            Duration::from_secs(60)
        } else {
            Duration::from_secs(15)
        };
        sleep_rest(started, period, &hub);
    }
}

pub fn start(hub: &Arc<Hub>) {
    hub.health.lock().unwrap().started = Some(Instant::now());
    for (name, run) in [
        ("sirens", sirens_loop as fn(Arc<Hub>)),
        ("feed", feed_loop as fn(Arc<Hub>)),
        ("messages", messages_loop as fn(Arc<Hub>)),
        ("official", official_loop as fn(Arc<Hub>)),
        ("official-history", official_history_loop as fn(Arc<Hub>)),
    ] {
        let hub = hub.clone();
        let _ = std::thread::Builder::new().name(name.to_string()).spawn(move || run(hub));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_and_broken_answers_of_the_sirens_address() {
        assert_eq!(parse_sirens(b"[]").unwrap().len(), 0);
        assert!(parse_sirens(b"<html>").is_err());
        assert!(parse_sirens(b"").is_err());
        assert!(parse_sirens(b"{\"a\":1}").is_err());
    }

    #[test]
    fn official_file_forms() {
        assert_eq!(parse_official(b"\xef\xbb\xbf\r\n").unwrap().len(), 0);
        let body = "{\"id\":\"133\",\"cat\":\"1\",\"title\":\"ירי רקטות וטילים\",\"data\":[\"לוד\"],\"desc\":\"היכנסו\"}";
        let a = parse_official(body.as_bytes()).unwrap();
        assert_eq!(a[0].4, vec!["לוד".to_string()]);
        let le: Vec<u8> = body.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        assert_eq!(parse_official(&le).unwrap()[0].2, "ירי רקטות וטילים");
        assert!(parse_official(b"<html>403</html>").is_err());
    }

    /// /api/v1/AlertHistory/heb?period=lastMonth as it answered on Nov 6 2026 (byte for byte, but for spacing)
    const API_SAMPLE: &str = r#"{"lastRetrievedAlarm": {"data": "ערב אל עראמשה", "date": "10.10.2026", "time": "19:57:09", "dateTime": "2026-10-10T19:57:00", "category": 2, "category_Desc": "חדירת כלי טיס עוין", "matrixId": 0, "rid": 706605}, "alarmsGroupedByDate": [{"key": "19.10.2026", "value": [{"data": "אביתר", "date": "19.10.2026", "time": "20:31:16", "dateTime": "2026-10-19T20:31:00", "category": 13, "category_Desc": "הוסר החשש לחדירת מחבלים", "matrixId": 0, "rid": 706608}, {"data": "אביתר", "date": "19.10.2026", "time": "19:37:42", "dateTime": "2026-10-19T19:37:00", "category": 10, "category_Desc": "חדירת מחבלים", "matrixId": 0, "rid": 706607}]}, {"key": "10.10.2026", "value": [{"data": "ערב אל עראמשה", "date": "10.10.2026", "time": "20:06:13", "dateTime": "2026-10-10T20:06:00", "category": 13, "category_Desc": "האירוע הסתיים", "matrixId": 0, "rid": 706606}, {"data": "ערב אל עראמשה", "date": "10.10.2026", "time": "19:57:09", "dateTime": "2026-10-10T19:57:00", "category": 2, "category_Desc": "חדירת כלי טיס עוין", "matrixId": 0, "rid": 706605}]}]}"#;

    /// AlertsHistory.json's form: one row per place, Israel's clock
    const FILE_SAMPLE: &str = r#"[{"alertDate":"2026-10-19 20:31:16","title":"הוסר החשש לחדירת מחבלים","data":"אביתר","category":13},
{"alertDate":"2026-10-19 19:37:42","title":"חדירת מחבלים","data":"אביתר","category":10},
{"alertDate":"2026-10-19 19:37:50","title":"חדירת מחבלים","data":"תפוח","category":10},
{"alertDate":"2026-10-19 19:37:42","title":"חדירת מחבלים","data":"אביתר","category":10},
{"alertDate":"bad date","title":"ירי רקטות וטילים","data":"לוד","category":1},
{"alertDate":"2026-10-19 19:00:00","title":"ירי רקטות וטילים","data":"","category":1}]"#;

    #[test]
    fn official_history_quiet_forms() {
        // each was seen live on Nov 6 2026: the history file a bare line break, the live file a BOM and one
        assert!(parse_official_history(b"\r\n").unwrap().is_empty());
        assert!(parse_official_history(b"\xef\xbb\xbf\r\n").unwrap().is_empty());
        assert!(parse_official_history(b"").unwrap().is_empty());
        assert!(parse_official_history(b"[]").unwrap().is_empty());
        assert!(parse_official_history("{\"alarmsGroupedByDate\":[]}".as_bytes()).unwrap().is_empty());
        assert!(parse_official_history(b"<html>403 Forbidden</html>").is_err());
        assert!(parse_official_history(b"{\"error\":1}").is_err());
        assert!(parse_official_history(b"42").is_err());
    }

    #[test]
    fn official_history_file_form() {
        let rows = parse_official_history(FILE_SAMPLE.as_bytes()).unwrap();
        // the row twice is one, the bad date and the empty place are left out
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "אביתר");
        assert_eq!(rows[0].title, "חדירת מחבלים");
        assert_eq!(rows[2].time_s, 1_792_431_076);
        assert_eq!(rows[2].cat, "13");
        let groups = group_official(&rows);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].names, vec!["אביתר".to_string(), "תפוח".to_string()]);
        // UTF-16 with a mark, and with a UTF-8 mark: the same rows
        let mut le = vec![0xFF, 0xFE];
        le.extend(FILE_SAMPLE.encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(parse_official_history(&le).unwrap(), rows);
        let mut bom = b"\xef\xbb\xbf".to_vec();
        bom.extend_from_slice(FILE_SAMPLE.as_bytes());
        bom.extend_from_slice(b"\r\n");
        assert_eq!(parse_official_history(&bom).unwrap(), rows);
    }

    #[test]
    fn official_history_api_form() {
        let rows = parse_official_history(API_SAMPLE.as_bytes()).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].name, "ערב אל עראמשה");
        assert_eq!(rows[0].title, "חדירת כלי טיס עוין");
        assert_eq!(rows[3].time_s, 1_792_431_076);
        // the file and the API say the same about the same alert
        let file = parse_official_history(FILE_SAMPLE.as_bytes()).unwrap();
        assert_eq!((rows[3].time_s, &rows[3].name, &rows[3].cat), (file[2].time_s, &file[2].name, &file[2].cat));
        let readings: Vec<(Kind, Threat)> = rows.iter().map(|r| {
            let x = model::read_official(&r.cat, &r.title, "", true);
            (x.kind, x.threat)
        }).collect();
        assert_eq!(readings[0], (Kind::Siren, Threat::Aircraft));
        assert_eq!(readings[2], (Kind::Siren, Threat::Infiltration));
        assert_eq!(readings[3].0, Kind::End);
    }

    #[test]
    fn israel_clock_to_unix_whatever_the_pc_zone() {
        assert_eq!(israel_to_unix(2026, 11, 6, 3, 0, 0), 1_793_926_800);
        assert_eq!(israel_to_unix(2026, 10, 19, 20, 31, 16), 1_792_431_076);
        // summer time begins the Friday before March's last Sunday, and ends October's last Sunday, at 02:00
        assert_eq!(israel_to_unix(2026, 3, 27, 1, 59, 0), 1_774_569_540);
        assert_eq!(israel_to_unix(2026, 3, 27, 3, 0, 0), 1_774_569_600);
        assert_eq!(israel_to_unix(2026, 10, 25, 1, 59, 0), 1_792_882_740);
        assert_eq!(israel_to_unix(2026, 10, 25, 2, 30, 0), 1_792_888_200);
        assert_eq!(parse_israel_time("19.10.2026", Some("20:31:16")), Some(1_792_431_076));
        assert_eq!(parse_israel_time("2026-10-19T20:31:16", None), Some(1_792_431_076));
        assert_eq!(parse_israel_time("2026-13-19 20:31:16", None), None);
    }

    #[test]
    fn the_live_file_backs_off_and_comes_back_to_once_a_second() {
        assert_eq!(official_backoff(0, false), Duration::from_secs(1));
        assert_eq!(official_backoff(1, false), Duration::from_secs(2));
        assert_eq!(official_backoff(3, false), Duration::from_secs(8));
        assert_eq!(official_backoff(30, false), Duration::from_secs(60));
        assert_eq!(official_backoff(1, true), Duration::from_secs(15));
        assert_eq!(official_backoff(3, true), Duration::from_secs(60));
        assert_eq!(official_backoff(30, true), Duration::from_secs(300));
    }

    fn item(source: Source, id: &str, t: i64, kind: Kind, threat: Threat, places: Vec<usize>) -> HistItem {
        HistItem { id: id.into(), source, time_s: t, kind, threat, drill: false, places, unknown: vec![], title: String::new(), body: String::new() }
    }

    #[test]
    fn the_official_history_fills_gaps_and_does_not_repeat_tzofar() {
        let t = 1_792_431_000;
        let tz = vec![item(Source::Feed, "a1:2", t, Kind::Siren, Threat::Infiltration, vec![3, 7])];
        let off = vec![
            item(Source::Official, "o1", t + 40, Kind::Siren, Threat::Infiltration, vec![7]),
            item(Source::Official, "o2", t + 900, Kind::Siren, Threat::Infiltration, vec![7]),
            item(Source::Official, "o3", t + 10, Kind::Siren, Threat::Missiles, vec![7]),
            item(Source::Official, "o4", t + 20, Kind::End, Threat::Infiltration, vec![9]),
        ];
        let m = merge_history(&tz, &off);
        let ids: Vec<&str> = m.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["o2", "o4", "o3", "a1:2"]);
        assert_eq!(m[0].source, Source::Official);
        assert_eq!(m[3].source, Source::Feed);
        // Tzofar down: the official rows alone
        assert_eq!(merge_history(&[], &off).len(), 4);
    }

    #[test]
    fn an_official_copy_of_a_tzofar_alert_changes_nothing_and_an_early_warning_never_overrides_an_alert() {
        use crate::engine::Policy;
        let places = Arc::new(Places::embedded());
        let lod = places.find("לוד").unwrap();
        let mut e = Engine::new(places.clone(), Policy { homes: vec![lod], quiet_from: 0, quiet_to: 0, mute_all: true, ..Policy::default() });
        let t0 = now_ms();
        let ev = |source, key: &str, kind, threat, t_ms: i64| Event {
            source, key: key.into(), kind, threat, drill: false, time_s: Some(t_ms / 1000), title: String::new(), body: String::new(),
            places: vec![lod], unknown: vec![], read_at: Instant::now(),
        };
        assert_eq!(e.apply(&ev(Source::Sirens, "n:1", Kind::Siren, Threat::Missiles, t0), t0).new_places, 1);
        let copy = ev(Source::Official, "offh:1", Kind::Siren, Threat::Missiles, t0 + 3000);
        assert_eq!(e.apply(&copy, t0 + 20_000).changes, 0);
        let pre = ev(Source::Official, "offh:2", Kind::Pre, Threat::General, t0 + 5000);
        assert_eq!(e.apply(&pre, t0 + 21_000).changes, 0);
        assert_eq!(e.snapshot().home_zone().unwrap().kind, Kind::Siren);
        assert_eq!(e.snapshot().home_zone().unwrap().source, Source::Sirens);
    }

    #[test]
    fn health_says_down_when_the_answers_stop() {
        let mut h = Health { started: Some(Instant::now() - Duration::from_secs(120)), ..Health::default() };
        assert_eq!(h.protection(), Protection::Down);
        assert!(h.trouble().unwrap().contains("אזעקה לא תוצג"));
        h.sirens.good(20, Some(2));
        assert_eq!(h.protection(), Protection::Ok);
        assert!(h.trouble().is_none());
        h.sirens.ok_at = Some(Instant::now() - Duration::from_secs(10));
        assert_eq!(h.protection(), Protection::Weak);
        h.sirens.ok_at = Some(Instant::now() - Duration::from_secs(60));
        assert_eq!(h.protection(), Protection::Down);
        // a cache serving the same answer for minutes is not fresh, whatever the status says
        h.sirens.good(20, Some(600));
        assert_eq!(h.protection(), Protection::Weak);
    }

    #[test]
    fn a_failed_read_never_counts_as_a_good_one() {
        let mut h = SourceHealth::default();
        h.bad("HTTP 403".into());
        assert!(h.ok_at.is_none());
        assert!(h.age_s().is_none());
        h.good(10, None);
        assert!(h.bad_since.is_none());
        assert!(h.age_s().unwrap() < 1.0);
    }
}
