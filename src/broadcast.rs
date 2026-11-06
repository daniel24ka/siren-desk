//! The alerts for the live broadcast: a transparent 1920x1080 page for an OBS Browser source
//! (assets/broadcast.html) and its data, pushed the moment the engine changes. 127.0.0.1 only.
//!
//! The stream is public, so what goes out is only what Home Front Command publishes anyway: the places, the
//! kind of alert, the times. Never a rehearsal, never "my places", never the distance from home.

use crate::engine::Snapshot;
use crate::paths::{self, now_ms};
use crate::places::{project, Places};
use crate::sources::{Hub, Protection};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("../assets/broadcast.html");
const MAX_CLIENTS: usize = 16;

static CLIENTS: AtomicUsize = AtomicUsize::new(0);
static GEO: OnceLock<String> = OnceLock::new();

pub struct Config {
    pub port: u16,
    /// serve the page from this file, read on every request (for working on it)
    pub page: Option<PathBuf>,
    /// a test run: the page says so on screen
    pub test_run: bool,
}

pub fn start(hub: Arc<Hub>, cfg: Config) {
    let _ = std::thread::Builder::new().name("broadcast".into()).spawn(move || {
        let listener = match TcpListener::bind(("127.0.0.1", cfg.port)) {
            Ok(l) => l,
            Err(e) => {
                paths::log(&format!("broadcast feed not started on port {}: {e}", cfg.port));
                return;
            }
        };
        paths::log(&format!("broadcast feed on http://127.0.0.1:{}/broadcast", cfg.port));
        let cfg = Arc::new(cfg);
        for conn in listener.incoming() {
            let Ok(stream) = conn else { continue };
            if CLIENTS.load(Ordering::Relaxed) >= MAX_CLIENTS {
                continue;
            }
            let (hub, cfg) = (hub.clone(), cfg.clone());
            CLIENTS.fetch_add(1, Ordering::Relaxed);
            let _ = std::thread::Builder::new().name("broadcast-client".into()).spawn(move || {
                let _ = handle(stream, &hub, &cfg);
                CLIENTS.fetch_sub(1, Ordering::Relaxed);
            });
        }
    });
}

fn respond(s: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()
}

fn handle(mut s: TcpStream, hub: &Hub, cfg: &Config) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.set_nodelay(true)?;
    let mut req = Vec::with_capacity(1024);
    let mut buf = [0u8; 2048];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 16 * 1024 {
        let n = s.read(&mut buf)?;
        if n == 0 {
            break;
        }
        req.extend_from_slice(&buf[..n]);
    }
    let text = String::from_utf8_lossy(&req);
    let mut words = text.lines().next().unwrap_or("").split_whitespace();
    if words.next() != Some("GET") {
        return respond(&mut s, "405 Method Not Allowed", "text/plain", b"GET only");
    }
    let target = words.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/");
    match path {
        "/broadcast" | "/broadcast/" => {
            let page = cfg.page.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| PAGE.to_string());
            respond(&mut s, "200 OK", "text/html; charset=utf-8", page.as_bytes())
        }
        "/broadcast/geo" => respond(&mut s, "200 OK", "application/json", geo(&hub.places).as_bytes()),
        "/broadcast/state" => {
            let body = state(hub, cfg.test_run).to_string();
            respond(&mut s, "200 OK", "application/json; charset=utf-8", body.as_bytes())
        }
        "/broadcast/events" => events(s, hub, cfg.test_run),
        "/broadcast/backdrop.png" if cfg.test_run => match std::fs::read(paths::data_dir().join("backdrop.png")) {
            Ok(bytes) => respond(&mut s, "200 OK", "image/png", &bytes),
            Err(_) => respond(&mut s, "404 Not Found", "text/plain", b"no backdrop"),
        },
        _ => respond(&mut s, "404 Not Found", "text/plain", b"not here"),
    }
}

/// What the page needs to know right now, and nothing more
fn state_of(snap: &Snapshot, now: i64, test_run: bool, ok: bool) -> Value {
    let zones: Vec<Value> = snap
        .zones
        .iter()
        .filter(|z| !z.drill)
        .map(|z| {
            json!({
                "i": z.place,
                "n": if z.place.is_none() { Some(z.name.as_str()) } else { None },
                "k": format!("{:?}", z.kind),
                "t": format!("{:?}", z.threat),
                "a": z.alert_ms,
                "f": z.first_ms,
                "c": z.countdown_s,
            })
        })
        .collect();
    let msg = snap.message.as_ref().filter(|m| !m.drill).map(|m| {
        json!({ "title": m.title, "body": m.body.replace('*', ""), "kind": m.kind.map(|k| format!("{k:?}")), "at": m.time_ms })
    });
    json!({ "rev": snap.rev, "now": now, "test": test_run, "ok": ok, "zones": zones, "msg": msg })
}

fn state(hub: &Hub, test_run: bool) -> Value {
    let snap = hub.engine.lock().unwrap().snapshot();
    let ok = hub.health.lock().unwrap().protection() == Protection::Ok;
    state_of(&snap, now_ms(), test_run, ok)
}

/// Server-sent events: the whole state on every change (checked every 100 ms) and every 5 s besides
fn events(mut s: TcpStream, hub: &Hub, test_run: bool) -> std::io::Result<()> {
    // (the page in OBS is a local file: it reads this from another origin)
    s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: keep-alive\r\n\r\n")?;
    let mut last_rev = u64::MAX;
    let mut last_sent = Instant::now() - Duration::from_secs(60);
    loop {
        if hub.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let rev = hub.engine.lock().unwrap().rev();
        if rev != last_rev || last_sent.elapsed() >= Duration::from_secs(5) {
            let line = format!("data: {}\n\n", state(hub, test_run));
            s.write_all(line.as_bytes())?;
            s.flush()?;
            last_rev = rev;
            last_sent = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn flat_km(points: &[(f32, f32)]) -> Vec<f32> {
    points.iter().flat_map(|&(x, y)| [(x * 20.0).round() / 20.0, (y * 20.0).round() / 20.0]).collect()
}

fn rings(raw: &Value) -> Vec<Vec<f32>> {
    raw.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|ring| ring.as_array())
                .map(|ring| {
                    let pts: Vec<(f32, f32)> = ring.chunks_exact(2).filter_map(|c| Some(project(c[1].as_f64()?, c[0].as_f64()?))).collect();
                    flat_km(&pts)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every place's shape and the land around, in the map's kilometres: made once
fn geo(places: &Places) -> &'static str {
    GEO.get_or_init(|| {
        let list: Vec<Value> = places
            .list
            .iter()
            .map(|p| {
                json!({
                    "n": p.name, "b": p.base, "a": places.area_name(p.area), "e": p.en, "ae": places.area_name_en(p.area), "c": p.countdown,
                    "x": (p.x * 10.0).round() / 10.0, "y": (p.y * 10.0).round() / 10.0, "p": flat_km(&p.poly),
                })
            })
            .collect();
        let base: Value = serde_json::from_str(crate::map::BASEMAP).unwrap_or(Value::Null);
        json!({
            "places": list,
            "land": rings(&base["land"]), "home": rings(&base["home"]), "coast": rings(&base["coast"]),
            "borders": rings(&base["borders"]), "lakes": rings(&base["lakes"]),
        })
        .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, Event, Policy, Source};
    use crate::model::{Kind, Threat};

    #[test]
    fn the_public_feed_carries_no_rehearsal_and_nothing_about_home() {
        let places = Arc::new(Places::embedded());
        let home = places.find("רחובות").unwrap();
        let mut e = Engine::new(places.clone(), Policy { homes: vec![home], quiet_from: 0, quiet_to: 0, ..Policy::default() });
        let now = 1_800_000_000_000i64;
        let ev = |key: &str, drill: bool, names: &[&str]| Event {
            source: if drill { Source::Drill } else { Source::Sirens },
            key: key.into(),
            kind: Kind::Siren,
            threat: Threat::Missiles,
            drill,
            time_s: Some(now / 1000),
            title: String::new(),
            body: String::new(),
            places: names.iter().filter_map(|n| places.find(n)).collect(),
            unknown: vec![],
            read_at: Instant::now(),
        };
        e.apply(&ev("d", true, &["אילת"]), now);
        let v = state_of(&e.snapshot(), now, false, true);
        assert_eq!(v["zones"].as_array().unwrap().len(), 0, "a rehearsal went on air");
        e.apply(&ev("r", false, &["רחובות", "נס ציונה"]), now + 1000);
        let text = state_of(&e.snapshot(), now, false, true).to_string();
        assert!(!text.contains("home") && !text.contains("km"), "home leaked: {text}");
        assert_eq!(state_of(&e.snapshot(), now, false, true)["zones"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn the_map_data_is_complete() {
        let places = Places::embedded();
        let v: Value = serde_json::from_str(geo(&places)).unwrap();
        assert_eq!(v["places"].as_array().unwrap().len(), places.list.len());
        // English for the stream's viewers abroad: every place and area has it
        assert!(v["places"].as_array().unwrap().iter().all(|p| !p["e"].as_str().unwrap_or("").is_empty()));
        assert!(v["places"].as_array().unwrap().iter().all(|p| !p["ae"].as_str().unwrap_or("").is_empty()));
        assert!(v["land"].as_array().unwrap().len() > 10);
        assert!(v["coast"].as_array().unwrap().len() > 3);
    }
}
