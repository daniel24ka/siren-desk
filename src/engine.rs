//! The state of the country: which place is under what, since when, and what each new word from a source
//! asks of the screen and the speakers. No window and no network here: the source threads call apply() the moment they
//! have read an answer, so a stuck screen can never delay the sound.

use crate::model::{hold_ms, Kind, Threat};
use crate::places::Places;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
pub enum Source {
    /// Tzofar's list of alerts on now: the fast path for sirens
    Sirens,
    /// Tzofar's messages by id: the fast path for early warnings and "event ended"
    Messages,
    /// Tzofar's combined feed: slower, catches what the fast paths missed
    Feed,
    /// Home Front Command's own files: the live one, and its history filling the gaps
    Official,
    Drill,
}

/// How much this event asks for, weakest first
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord, Serialize)]
pub enum Level {
    None,
    Far,
    Near,
    HomeEnd,
    HomePre,
    Home,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cue {
    Siren,
    Near,
    Far,
    Pre,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, serde::Deserialize, Default)]
pub enum FarMode {
    #[default]
    Panel,
    Full,
    Tray,
}

#[derive(Clone, Debug)]
pub struct Policy {
    pub homes: Vec<usize>,
    pub near_km: f32,
    pub far_mode: FarMode,
    pub far_sound: bool,
    /// hour of day: far alerts make no sound from this hour to quiet_to (home always does)
    pub quiet_from: u8,
    pub quiet_to: u8,
    /// far alerts neither pop up nor sound until this time
    pub snooze_until_ms: i64,
    /// tests: nothing sounds at all
    pub mute_all: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Policy { homes: vec![], near_km: 15.0, far_mode: FarMode::Panel, far_sound: true, quiet_from: 23, quiet_to: 7, snooze_until_ms: 0, mute_all: false }
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub source: Source,
    pub key: String,
    pub kind: Kind,
    pub threat: Threat,
    pub drill: bool,
    /// the source's time of the alert (seconds)
    pub time_s: Option<i64>,
    pub title: String,
    pub body: String,
    pub places: Vec<usize>,
    pub unknown: Vec<String>,
    /// when the answer was read
    pub read_at: Instant,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ZoneKey {
    Place(usize),
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct Zone {
    pub place: Option<usize>,
    pub name: String,
    pub kind: Kind,
    pub threat: Threat,
    pub drill: bool,
    /// when this program first knew of this round
    pub first_ms: i64,
    /// when the alert began: the source's time when it can be trusted
    pub alert_ms: i64,
    pub expires_ms: i64,
    pub home: bool,
    pub countdown_s: i32,
    pub km_from_home: f32,
    pub source: Source,
}

impl Zone {
    /// seconds left to reach the shelter; None when this alert has no flight time ("now")
    pub fn left_s(&self, now_ms: i64) -> Option<i32> {
        if self.kind != Kind::Siren || !self.threat.has_countdown() {
            return None;
        }
        let left = self.countdown_s as i64 - (now_ms - self.alert_ms) / 1000;
        Some(left.max(0) as i32)
    }

    /// the first moments of a siren, when the place is still to take cover
    pub fn urgent(&self, now_ms: i64) -> bool {
        if self.kind != Kind::Siren {
            return false;
        }
        match self.left_s(now_ms) {
            Some(left) => left > 0,
            None => now_ms - self.alert_ms < 60_000,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Message {
    pub title: String,
    pub body: String,
    pub time_ms: i64,
    pub drill: bool,
    pub kind: Option<Kind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    pub level: Level,
    pub cue: Option<Cue>,
    pub show: bool,
    pub changes: usize,
    pub new_places: usize,
    pub drill: bool,
    /// the siren sound must stop now (the event ended, or a real word ended a rehearsal)
    pub stop_siren: bool,
}

impl Decision {
    fn nothing() -> Decision {
        Decision { level: Level::None, cue: None, show: false, changes: 0, new_places: 0, drill: false, stop_siren: false }
    }
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub rev: u64,
    pub zones: Vec<Zone>,
    /// bumps each time the screen should come up again; with what weight and why
    pub present_seq: u64,
    pub present_level: Option<Level>,
    pub present_drill: bool,
    pub last_new_ms: i64,
    pub message: Option<Message>,
    /// the delay of the newest alert: its source's time to the moment it was read (ms)
    pub last_delay_ms: Option<i64>,
    pub last_source: Option<Source>,
    pub present_read_at: Option<Instant>,
}

impl Snapshot {
    pub fn home_zone(&self) -> Option<&Zone> {
        let rank = |z: &&Zone| (z.kind != Kind::End, z.kind, z.alert_ms);
        self.zones.iter().filter(|z| z.home).max_by_key(rank)
    }

    pub fn count(&self, kind: Kind) -> usize {
        self.zones.iter().filter(|z| z.kind == kind).count()
    }

    pub fn active(&self) -> usize {
        self.zones.iter().filter(|z| z.kind != Kind::End).count()
    }

    pub fn nearest_alert(&self) -> Option<&Zone> {
        self.zones
            .iter()
            .filter(|z| z.kind == Kind::Siren && z.place.is_some())
            .min_by(|a, b| a.km_from_home.partial_cmp(&b.km_from_home).unwrap_or(std::cmp::Ordering::Equal))
    }
}

pub struct Engine {
    pub places: Arc<Places>,
    pub policy: Policy,
    zones: HashMap<ZoneKey, Zone>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    rev: u64,
    present_seq: u64,
    present_level: Option<Level>,
    present_drill: bool,
    present_read_at: Option<Instant>,
    last_new_ms: i64,
    message: Option<Message>,
    last_delay_ms: Option<i64>,
    last_source: Option<Source>,
    last_chime_ms: i64,
    siren_until_ms: i64,
}

fn hour_of(ms: i64) -> u8 {
    crate::paths::local_hour(ms)
}

impl Engine {
    pub fn new(places: Arc<Places>, policy: Policy) -> Engine {
        Engine {
            places,
            policy,
            zones: HashMap::new(),
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            rev: 1,
            present_seq: 0,
            present_level: None,
            present_drill: false,
            present_read_at: None,
            last_new_ms: 0,
            message: None,
            last_delay_ms: None,
            last_source: None,
            last_chime_ms: 0,
            siren_until_ms: 0,
        }
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    fn km_from_home(&self, place: usize) -> f32 {
        let mut best = f32::MAX;
        for &h in &self.policy.homes {
            best = best.min(self.places.distance_km(h, place));
        }
        best
    }

    fn remember(&mut self, key: &str) -> bool {
        if self.seen.contains(key) {
            return false;
        }
        self.seen.insert(key.to_string());
        self.seen_order.push_back(key.to_string());
        if self.seen_order.len() > 2000 {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }

    fn quiet_now(&self, now_ms: i64) -> bool {
        let (a, b, h) = (self.policy.quiet_from, self.policy.quiet_to, hour_of(now_ms));
        if a == b {
            false
        } else if a < b {
            h >= a && h < b
        } else {
            h >= a || h < b
        }
    }

    /// One piece of word from a source. Returns what it asks of the screen and the speakers.
    pub fn apply(&mut self, ev: &Event, now_ms: i64) -> Decision {
        if !self.remember(&ev.key) {
            return Decision::nothing();
        }
        // a real word ends a rehearsal on the spot
        let mut rehearsal_ended = false;
        if !ev.drill && self.zones.values().any(|z| z.drill) {
            self.zones.retain(|_, z| !z.drill);
            self.siren_until_ms = 0;
            self.rev += 1;
            rehearsal_ended = true;
        }
        let source_ms = ev.time_s.map(|t| t * 1000);
        // (a clock ahead of ours can't be trusted for "seconds left")
        let alert_ms = match source_ms {
            Some(t) if t <= now_ms + 5_000 => t.min(now_ms),
            _ => now_ms,
        };
        let hold = hold_ms(ev.kind, ev.threat);
        let mut d = Decision::nothing();
        d.stop_siren = rehearsal_ended;
        if now_ms - alert_ms >= hold {
            return d;
        }
        d.drill = ev.drill;
        let mut home_new = false;
        let mut home_touched = false;
        let mut nearest = f32::MAX;
        let keys: Vec<(ZoneKey, Option<usize>, String)> = ev
            .places
            .iter()
            .map(|&i| (ZoneKey::Place(i), Some(i), self.places.list[i].name.clone()))
            .chain(ev.unknown.iter().map(|n| (ZoneKey::Unknown(n.clone()), None, n.clone())))
            .collect();
        for (key, place, name) in keys {
            let home = place.map(|i| self.policy.homes.contains(&i)).unwrap_or(false);
            let km = place.map(|i| self.km_from_home(i)).unwrap_or(f32::MAX);
            let countdown_s = place.map(|i| self.places.list[i].countdown as i32).unwrap_or(0);
            let fresh = Zone {
                place,
                name,
                kind: ev.kind,
                threat: ev.threat,
                drill: ev.drill,
                first_ms: now_ms,
                alert_ms,
                expires_ms: alert_ms + hold,
                home,
                countdown_s,
                km_from_home: km,
                source: ev.source,
            };
            let existing = self.zones.get_mut(&key);
            match ev.kind {
                Kind::End => {
                    if let Some(z) = existing {
                        // (an "ended" older than this round of the alert belongs to the last one)
                        if z.kind != Kind::End && alert_ms + 30_000 >= z.alert_ms {
                            z.kind = Kind::End;
                            z.expires_ms = now_ms + hold;
                            d.changes += 1;
                            home_touched |= home;
                        }
                    }
                }
                Kind::Siren => match existing {
                    Some(z) if z.kind == Kind::Siren && z.threat == ev.threat && (alert_ms - z.alert_ms).abs() <= 90_000 => {
                        // the same round, listed again
                        z.expires_ms = z.expires_ms.max(alert_ms + hold);
                    }
                    Some(z) if z.kind == Kind::Siren && alert_ms + 90_000 < z.alert_ms => {}
                    // (a late copy of an alert whose "ended" already came)
                    Some(z) if z.kind == Kind::End && alert_ms <= z.alert_ms + 90_000 => {}
                    _ => {
                        self.zones.insert(key, fresh);
                        d.changes += 1;
                        d.new_places += 1;
                        home_new |= home;
                        nearest = nearest.min(km);
                    }
                },
                Kind::Stay | Kind::Near | Kind::Pre | Kind::Notice => match existing {
                    Some(z) if z.kind == Kind::Siren && matches!(ev.kind, Kind::Pre | Kind::Notice) && now_ms < z.expires_ms => {}
                    Some(z) if z.kind == ev.kind => z.expires_ms = z.expires_ms.max(alert_ms + hold),
                    Some(z) if z.kind == Kind::Siren => {
                        // from "take cover" to "stay in the shelter": the same round goes on
                        z.kind = ev.kind;
                        z.expires_ms = alert_ms + hold;
                        d.changes += 1;
                        home_touched |= home;
                    }
                    _ => {
                        self.zones.insert(key, fresh);
                        d.changes += 1;
                        d.new_places += 1;
                        home_touched |= home;
                    }
                },
            }
        }
        if !ev.body.is_empty() || !ev.title.is_empty() {
            self.message = Some(Message { title: ev.title.clone(), body: ev.body.clone(), time_ms: alert_ms, drill: ev.drill, kind: Some(ev.kind) });
        }
        if d.changes == 0 {
            return d;
        }
        self.rev += 1;
        if let Some(t) = source_ms {
            self.last_delay_ms = Some(now_ms - t);
        }
        self.last_source = Some(ev.source);
        if d.new_places > 0 {
            self.last_new_ms = now_ms;
        }
        // what it asks for
        let snoozed = now_ms < self.policy.snooze_until_ms;
        let far_shows = !snoozed && self.policy.far_mode != FarMode::Tray;
        let far_sounds = !snoozed && self.policy.far_sound && !self.quiet_now(now_ms);
        let can_chime = now_ms - self.last_chime_ms > 20_000;
        let source_drill = ev.drill && ev.source != Source::Drill;
        // (found only now, say after a restart or a sleep: the screen tells it, nothing sounds or pops up for far places)
        let late = now_ms - alert_ms > 120_000;
        match ev.kind {
            Kind::Siren if home_new && !source_drill => {
                d.level = Level::Home;
                d.show = true;
                if !late {
                    d.cue = Some(Cue::Siren);
                    let c = self.policy.homes.iter().map(|&h| self.places.list[h].countdown as i64).max().unwrap_or(30).max(30);
                    let run_s = if ev.threat.has_countdown() { c + 15 } else { 60 };
                    self.siren_until_ms = alert_ms + run_s * 1000;
                }
            }
            _ if late && !home_touched => {}
            Kind::Pre | Kind::Stay | Kind::Near if late => {
                d.level = Level::HomePre;
                d.show = true;
            }
            Kind::End if late => {}
            Kind::Siren if nearest <= self.policy.near_km && !source_drill => {
                d.level = Level::Near;
                d.show = true;
                if can_chime {
                    d.cue = Some(Cue::Near);
                }
            }
            Kind::Siren => {
                d.level = Level::Far;
                d.show = far_shows;
                if can_chime && far_sounds {
                    d.cue = Some(Cue::Far);
                }
            }
            Kind::Pre | Kind::Stay | Kind::Near if home_touched => {
                d.level = Level::HomePre;
                d.show = true;
                d.cue = Some(Cue::Pre);
            }
            Kind::Pre | Kind::Stay | Kind::Near if d.new_places > 0 => {
                d.level = Level::Far;
                d.show = far_shows;
                if can_chime && far_sounds {
                    d.cue = Some(Cue::Far);
                }
            }
            Kind::End if home_touched => {
                d.level = Level::HomeEnd;
                d.show = true;
                d.cue = Some(Cue::End);
                d.stop_siren = true;
                self.siren_until_ms = 0;
            }
            _ => {}
        }
        if self.policy.mute_all {
            d.cue = None;
        }
        if matches!(d.cue, Some(Cue::Near | Cue::Far)) {
            self.last_chime_ms = now_ms;
        }
        if d.show {
            self.present_seq += 1;
            self.present_level = Some(d.level);
            self.present_drill = ev.drill;
            self.present_read_at = Some(ev.read_at);
        }
        d
    }

    /// The clock's part. Returns true if the siren sound should stop now.
    pub fn tick(&mut self, now_ms: i64) -> bool {
        let before = self.zones.len();
        self.zones.retain(|_, z| z.expires_ms > now_ms);
        if self.zones.len() != before {
            self.rev += 1;
        }
        if self.siren_until_ms != 0 && now_ms >= self.siren_until_ms {
            self.siren_until_ms = 0;
            return true;
        }
        false
    }

    pub fn siren_due(&self) -> bool {
        self.siren_until_ms != 0
    }

    /// The user pressed "mute": the siren stops and stays stopped for this round
    pub fn silence(&mut self) {
        self.siren_until_ms = 0;
    }

    pub fn clear_drills(&mut self) -> bool {
        let before = self.zones.len();
        self.zones.retain(|_, z| !z.drill);
        if self.zones.len() != before {
            self.rev += 1;
            self.siren_until_ms = 0;
            return true;
        }
        false
    }

    pub fn set_policy(&mut self, policy: Policy) {
        self.policy = policy;
        for (key, z) in self.zones.iter_mut() {
            if let ZoneKey::Place(i) = key {
                z.home = self.policy.homes.contains(i);
                let mut best = f32::MAX;
                for &h in &self.policy.homes {
                    best = best.min(self.places.distance_km(h, *i));
                }
                z.km_from_home = best;
            }
        }
        self.rev += 1;
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut zones: Vec<Zone> = self.zones.values().cloned().collect();
        zones.sort_by(|a, b| {
            b.home
                .cmp(&a.home)
                .then(b.kind.cmp(&a.kind))
                .then(a.km_from_home.partial_cmp(&b.km_from_home).unwrap_or(std::cmp::Ordering::Equal))
                .then(a.name.cmp(&b.name))
        });
        Snapshot {
            rev: self.rev,
            zones,
            present_seq: self.present_seq,
            present_level: self.present_level,
            present_drill: self.present_drill,
            last_new_ms: self.last_new_ms,
            message: self.message.clone(),
            last_delay_ms: self.last_delay_ms,
            last_source: self.last_source,
            present_read_at: self.present_read_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000_000;

    fn engine() -> Engine {
        let places = Arc::new(Places::embedded());
        let home = places.find("רחובות").unwrap();
        Engine::new(places, Policy { homes: vec![home], quiet_from: 0, quiet_to: 0, ..Policy::default() })
    }

    fn ev(e: &Engine, key: &str, kind: Kind, threat: Threat, time_ms: i64, names: &[&str]) -> Event {
        Event {
            source: Source::Sirens,
            key: key.into(),
            kind,
            threat,
            drill: false,
            time_s: Some(time_ms / 1000),
            title: String::new(),
            body: String::new(),
            places: names.iter().filter_map(|n| e.places.find(n)).collect(),
            unknown: names.iter().filter(|n| e.places.find(n).is_none()).map(|n| n.to_string()).collect(),
            read_at: Instant::now(),
        }
    }

    #[test]
    fn a_siren_at_home_takes_the_screen_and_counts_from_the_alert() {
        let mut e = engine();
        let a = ev(&e, "a", Kind::Siren, Threat::Missiles, T0 - 4_000, &["רחובות", "נס ציונה"]);
        let d = e.apply(&a, T0);
        assert_eq!((d.level, d.cue, d.show, d.new_places), (Level::Home, Some(Cue::Siren), true, 2));
        let s = e.snapshot();
        let h = s.home_zone().unwrap();
        assert_eq!(h.name, "רחובות");
        // 90 s to the shelter, read 4 s after it began: 86 left, not 90
        assert_eq!(h.left_s(T0), Some(86));
        assert!(h.urgent(T0));
        assert!(!h.urgent(T0 + 90_000));
        assert_eq!(s.last_delay_ms.map(|d| d / 1000), Some(4));
    }

    #[test]
    fn the_same_alert_from_a_second_source_changes_nothing() {
        let mut e = engine();
        let a = ev(&e, "a", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        assert_eq!(e.apply(&a, T0).level, Level::Home);
        let seq = e.snapshot().present_seq;
        // the very same id
        assert_eq!(e.apply(&a, T0 + 1000).changes, 0);
        // the feed's copy, under its own key and a slightly different time
        let b = ev(&e, "feed:a", Kind::Siren, Threat::Missiles, T0 + 2_000, &["רחובות"]);
        let d = e.apply(&b, T0 + 12_000);
        assert_eq!((d.changes, d.show, d.cue), (0, false, None));
        assert_eq!(e.snapshot().present_seq, seq);
    }

    #[test]
    fn a_new_round_at_the_same_place_is_a_new_alert() {
        let mut e = engine();
        let a = ev(&e, "a", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        e.apply(&a, T0);
        let b = ev(&e, "b", Kind::Siren, Threat::Missiles, T0 + 200_000, &["רחובות"]);
        let d = e.apply(&b, T0 + 200_000);
        assert_eq!((d.level, d.cue), (Level::Home, Some(Cue::Siren)));
        assert_eq!(e.snapshot().home_zone().unwrap().left_s(T0 + 200_000), Some(90));
    }

    #[test]
    fn levels_by_distance() {
        let mut e = engine();
        // Nes Ziona is 4.5 km from Rehovot, Eilat 259
        let near = ev(&e, "n", Kind::Siren, Threat::Missiles, T0, &["נס ציונה"]);
        assert_eq!(e.apply(&near, T0).level, Level::Near);
        let mut e = engine();
        let far = ev(&e, "f", Kind::Siren, Threat::Aircraft, T0, &["אילת"]);
        let d = e.apply(&far, T0);
        assert_eq!((d.level, d.cue, d.show), (Level::Far, Some(Cue::Far), true));
        assert_eq!(e.snapshot().zones[0].left_s(T0), None);
    }

    #[test]
    fn early_warning_then_siren_then_end_at_home() {
        let mut e = engine();
        let pre = ev(&e, "p", Kind::Pre, Threat::General, T0, &["רחובות"]);
        let d = e.apply(&pre, T0);
        assert_eq!((d.level, d.cue), (Level::HomePre, Some(Cue::Pre)));
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0 + 120_000, &["רחובות"]);
        assert_eq!(e.apply(&s, T0 + 120_000).level, Level::Home);
        // a late early warning must not take the siren away
        let late = ev(&e, "p2", Kind::Pre, Threat::General, T0 + 121_000, &["רחובות"]);
        assert_eq!(e.apply(&late, T0 + 121_000).changes, 0);
        assert_eq!(e.snapshot().home_zone().unwrap().kind, Kind::Siren);
        let end = ev(&e, "e", Kind::End, Threat::General, T0 + 600_000, &["רחובות"]);
        let d = e.apply(&end, T0 + 600_000);
        assert_eq!((d.level, d.cue), (Level::HomeEnd, Some(Cue::End)));
        assert_eq!(e.snapshot().home_zone().unwrap().kind, Kind::End);
        e.tick(T0 + 600_000 + 91_000);
        assert!(e.snapshot().zones.is_empty());
    }

    #[test]
    fn an_old_end_message_does_not_end_a_new_alert() {
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        e.apply(&s, T0);
        let old = ev(&e, "old", Kind::End, Threat::General, T0 - 60_000, &["רחובות"]);
        assert_eq!(e.apply(&old, T0 + 1000).changes, 0);
        assert_eq!(e.snapshot().home_zone().unwrap().kind, Kind::Siren);
    }

    #[test]
    fn an_end_for_a_place_not_on_the_list_is_ignored() {
        let mut e = engine();
        let end = ev(&e, "e", Kind::End, Threat::General, T0, &["רחובות"]);
        let d = e.apply(&end, T0);
        assert_eq!((d.changes, d.show), (0, false));
        assert!(e.snapshot().zones.is_empty());
    }

    #[test]
    fn places_expire_with_no_word() {
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        e.apply(&s, T0);
        e.tick(T0 + 9 * 60_000);
        assert_eq!(e.snapshot().zones.len(), 1);
        e.tick(T0 + 10 * 60_000 + 1);
        assert_eq!(e.snapshot().zones.len(), 0);
        // an infiltration lasts an hour
        let i = ev(&e, "i", Kind::Siren, Threat::Infiltration, T0, &["רחובות"]);
        e.apply(&i, T0);
        e.tick(T0 + 59 * 60_000);
        assert_eq!(e.snapshot().zones.len(), 1);
    }

    #[test]
    fn an_alert_too_old_for_its_hold_is_not_shown() {
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0 - 11 * 60_000, &["רחובות"]);
        assert_eq!(e.apply(&s, T0).changes, 0);
    }

    #[test]
    fn a_clock_behind_the_source_does_not_give_extra_seconds() {
        let mut e = engine();
        // the source's time is 30 s ahead of this PC's clock: count from now
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0 + 30_000, &["רחובות"]);
        e.apply(&s, T0);
        assert_eq!(e.snapshot().home_zone().unwrap().left_s(T0), Some(90));
    }

    #[test]
    fn unknown_names_are_kept_not_dropped() {
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0, &["מקום חדש שלא ברשימה"]);
        let d = e.apply(&s, T0);
        assert_eq!(d.new_places, 1);
        let snap = e.snapshot();
        assert_eq!(snap.zones[0].name, "מקום חדש שלא ברשימה");
        assert_eq!(snap.zones[0].place, None);
    }

    #[test]
    fn a_real_alert_ends_a_rehearsal() {
        let mut e = engine();
        let mut drill = ev(&e, "d", Kind::Siren, Threat::Missiles, T0, &["אילת"]);
        drill.drill = true;
        drill.source = Source::Drill;
        e.apply(&drill, T0);
        assert!(e.snapshot().zones[0].drill);
        let real = ev(&e, "r", Kind::Siren, Threat::Missiles, T0 + 1000, &["חיפה - מערב"]);
        e.apply(&real, T0 + 1000);
        let s = e.snapshot();
        assert_eq!(s.zones.len(), 1);
        assert!(!s.zones[0].drill);
    }

    #[test]
    fn the_sources_own_drill_never_sounds_the_siren() {
        let mut e = engine();
        let mut drill = ev(&e, "d", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        drill.drill = true;
        let d = e.apply(&drill, T0);
        assert_eq!(d.level, Level::Far);
        assert_ne!(d.cue, Some(Cue::Siren));
    }

    #[test]
    fn far_alerts_stay_quiet_when_snoozed_and_home_never_does() {
        let mut e = engine();
        e.policy.snooze_until_ms = T0 + 3_600_000;
        let far = ev(&e, "f", Kind::Siren, Threat::Missiles, T0, &["אילת"]);
        let d = e.apply(&far, T0);
        assert_eq!((d.show, d.cue), (false, None));
        let home = ev(&e, "h", Kind::Siren, Threat::Missiles, T0 + 1000, &["רחובות"]);
        let d = e.apply(&home, T0 + 1000);
        assert_eq!((d.show, d.cue), (true, Some(Cue::Siren)));
    }

    #[test]
    fn far_chime_does_not_repeat_every_second_in_a_barrage() {
        let mut e = engine();
        let a = ev(&e, "a", Kind::Siren, Threat::Missiles, T0, &["אילת"]);
        assert_eq!(e.apply(&a, T0).cue, Some(Cue::Far));
        let b = ev(&e, "b", Kind::Siren, Threat::Missiles, T0 + 1000, &["נחל עוז"]);
        let d = e.apply(&b, T0 + 1000);
        assert_eq!((d.cue, d.show), (None, true));
    }

    #[test]
    fn an_alert_found_late_shows_at_home_and_stays_silent() {
        // the program started 3 minutes into a siren at home: stay in the shelter, no siren sound
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0 - 180_000, &["רחובות"]);
        let d = e.apply(&s, T0);
        assert_eq!((d.level, d.cue, d.show), (Level::Home, None, true));
        assert!(!e.siren_due());
        // and far away: on the map only
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0 - 180_000, &["אילת"]);
        let d = e.apply(&s, T0);
        assert_eq!((d.level, d.cue, d.show, d.changes), (Level::None, None, false, 1));
    }

    #[test]
    fn a_late_copy_does_not_bring_back_an_alert_that_ended() {
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        e.apply(&s, T0);
        let end = ev(&e, "e", Kind::End, Threat::General, T0 + 20_000, &["רחובות"]);
        e.apply(&end, T0 + 20_000);
        let copy = ev(&e, "feed:s", Kind::Siren, Threat::Missiles, T0 + 1000, &["רחובות"]);
        assert_eq!(e.apply(&copy, T0 + 25_000).changes, 0);
        assert_eq!(e.snapshot().home_zone().unwrap().kind, Kind::End);
    }

    #[test]
    fn the_siren_sound_stops_on_its_own_and_on_the_end() {
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        e.apply(&s, T0);
        assert!(!e.tick(T0 + 60_000));
        assert!(e.tick(T0 + 105_000));
        assert!(!e.tick(T0 + 106_000));
        let mut e = engine();
        let s = ev(&e, "s", Kind::Siren, Threat::Missiles, T0, &["רחובות"]);
        e.apply(&s, T0);
        let end = ev(&e, "e", Kind::End, Threat::General, T0 + 30_000, &["רחובות"]);
        let d = e.apply(&end, T0 + 30_000);
        assert!(d.stop_siren);
        assert_eq!(d.cue, Some(Cue::End));
        assert!(!e.siren_due());
    }
}
