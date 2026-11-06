//! Rehearsals: real windows, real sounds, marked as a drill on every screen. Nothing leaves this PC.
//! A real alert ends a rehearsal on the spot (Engine::apply, and the id check here).

use crate::engine::{Event, Source};
use crate::model::{Kind, Threat};
use crate::paths::now_ms;
use crate::sound;
use crate::sources::Hub;
use crate::ui::Drill;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn send(hub: &Hub, id: i64, step: u32, kind: Kind, threat: Threat, places: Vec<usize>, title: &str, body: &str) {
    hub.deliver(Event {
        source: Source::Drill,
        key: format!("drill:{id}:{step}"),
        kind,
        threat,
        drill: true,
        time_s: Some(now_ms() / 1000),
        title: title.to_string(),
        body: body.to_string(),
        places,
        unknown: vec![],
        read_at: Instant::now(),
    });
}

/// Waits, ending the wait early if the rehearsal was called off
fn wait(hub: &Hub, id: i64, secs: f32) -> bool {
    let until = Instant::now() + Duration::from_secs_f32(secs);
    while Instant::now() < until {
        if hub.rehearsal_id.load(Ordering::SeqCst) != id || hub.stop.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

fn around(hub: &Hub, homes: &[usize], km: f32) -> Vec<usize> {
    (0..hub.places.list.len()).filter(|&i| homes.iter().any(|&h| hub.places.distance_km(h, i) <= km)).collect()
}

fn far_group(hub: &Hub, homes: &[usize]) -> Vec<usize> {
    // the far north, or the far south for someone who lives in the north
    let pick = ["קריית שמונה", "אילת"];
    for name in pick {
        if let Some(c) = hub.places.find(name) {
            if homes.iter().all(|&h| hub.places.distance_km(h, c) > 60.0) {
                return (0..hub.places.list.len()).filter(|&i| hub.places.distance_km(c, i) <= 16.0).collect();
            }
        }
    }
    vec![]
}

pub fn start(hub: Arc<Hub>, kind: Drill) {
    let id = hub.rehearsal_id.fetch_add(1, Ordering::SeqCst) + 1;
    hub.engine.lock().unwrap().clear_drills();
    sound::stop_siren();
    let _ = std::thread::Builder::new().name("drill".into()).spawn(move || {
        let homes = hub.engine.lock().unwrap().policy.homes.clone();
        let quiet = |hub: &Hub| {
            sound::stop_siren();
            hub.engine.lock().unwrap().silence();
        };
        let mut finished = false;
        match kind {
            Drill::Home => {
                send(&hub, id, 1, Kind::Siren, Threat::Missiles, around(&hub, &homes, 8.0), "", "");
                if wait(&hub, id, 6.0) {
                    quiet(&hub);
                    if wait(&hub, id, 16.0) {
                        send(&hub, id, 2, Kind::End, Threat::General, around(&hub, &homes, 8.0), "סיום אירוע", "תרגיל: האירוע הסתיים");
                        finished = wait(&hub, id, 8.0);
                    }
                }
            }
            Drill::Far => {
                send(&hub, id, 1, Kind::Siren, Threat::Missiles, far_group(&hub, &homes), "", "");
                finished = wait(&hub, id, 24.0);
            }
            Drill::Big => {
                send(&hub, id, 1, Kind::Siren, Threat::Missiles, around(&hub, &homes, 110.0), "", "");
                if wait(&hub, id, 6.0) {
                    quiet(&hub);
                    finished = wait(&hub, id, 20.0);
                }
            }
            Drill::Sequence => {
                send(&hub, id, 1, Kind::Pre, Threat::General, around(&hub, &homes, 70.0), "התרעה מקדימה", "תרגיל: בעקבות זיהוי שיגורים, בדקות הקרובות צפויות להתקבל התרעות באזורך");
                if wait(&hub, id, 8.0) {
                    send(&hub, id, 2, Kind::Siren, Threat::Missiles, around(&hub, &homes, 9.0), "", "");
                    if wait(&hub, id, 6.0) {
                        quiet(&hub);
                        if wait(&hub, id, 12.0) {
                            send(&hub, id, 3, Kind::End, Threat::General, around(&hub, &homes, 70.0), "סיום אירוע", "תרגיל: האירוע הסתיים, ניתן לצאת מהמרחב המוגן");
                            finished = wait(&hub, id, 8.0);
                        }
                    }
                }
            }
        }
        // (called off: whoever called it off has cleared it)
        if finished {
            hub.engine.lock().unwrap().clear_drills();
            hub.poke();
        }
    });
}

/// Called off by the user
pub fn stop(hub: &Hub) {
    hub.rehearsal_id.fetch_add(1, Ordering::SeqCst);
    sound::stop_siren();
    let mut e = hub.engine.lock().unwrap();
    e.silence();
    e.clear_drills();
}
