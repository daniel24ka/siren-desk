//! The alert places: names, alert areas, shelter time, shape. Tzofar's public lists are embedded (complete: 1449 of
//! 1449 places have a shape); its feed names them the same way and its messages use the same ids. The lists
//! change rarely (version in the feed); a newer copy in data/lists is taken over the embedded one.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const EMBEDDED_CITIES: &str = include_str!("../assets/lists/cities.json");
const EMBEDDED_POLYGONS: &str = include_str!("../assets/lists/polygons.json");

/// Kilometres from a fixed point in the middle of the country: flat enough at this size (under 0.3% error
/// from end to end), so distances are plain arithmetic and the map is a plain scale and shift.
pub fn project(lat: f64, lng: f64) -> (f32, f32) {
    (((lng - 35.0) * 94.92) as f32, (-(lat - 31.5) * 110.95) as f32)
}

pub fn unproject(x: f32, y: f32) -> (f64, f64) {
    (31.5 - y as f64 / 110.95, 35.0 + x as f64 / 94.92)
}

#[derive(Debug)]
pub struct Place {
    pub id: u32,
    pub name: String,
    /// the town of a split town ("תל אביב" of "תל אביב - מרכז העיר"); else the name itself
    pub base: String,
    /// the part of the town, if any
    pub sub: String,
    pub en: String,
    pub area: u16,
    pub countdown: u16,
    pub x: f32,
    pub y: f32,
    pub poly: Vec<(f32, f32)>,
    pub bbox: (f32, f32, f32, f32),
}

#[derive(Deserialize)]
struct RawCity {
    id: u32,
    he: String,
    #[serde(default)]
    en: Option<String>,
    #[serde(default)]
    area: Option<u16>,
    #[serde(default)]
    countdown: Option<u16>,
    #[serde(default)]
    lat: Option<f64>,
    #[serde(default)]
    lng: Option<f64>,
}

#[derive(Deserialize)]
struct RawArea {
    he: String,
    #[serde(default)]
    en: Option<String>,
}

#[derive(Deserialize)]
struct RawLists {
    cities: HashMap<String, RawCity>,
    areas: HashMap<String, RawArea>,
    #[serde(rename = "@VERSION", default)]
    version: Option<u32>,
}

/// Names in the feed that are not places (from Tzofar's own site, City.virtualCities)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Special {
    /// the whole country, home included
    AllCountry,
    /// the feed's own test
    Test,
}

pub struct Places {
    pub list: Vec<Place>,
    pub areas: HashMap<u16, String>,
    pub areas_en: HashMap<u16, String>,
    pub cities_version: u32,
    by_name: HashMap<String, usize>,
    by_norm: HashMap<String, usize>,
    by_id: HashMap<u32, usize>,
    norm_names: Vec<String>,
}

/// Names differ between sources in quotes (`''` vs `'`) and dashes; compared without them
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut space = false;
    for c in s.chars() {
        match c {
            '\'' | '"' | '׳' | '״' | '`' | '’' | '”' => {}
            '-' | '‐' | '‑' | '–' | '—' | ' ' | '\t' => space = true,
            _ => {
                if space && !out.is_empty() {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        }
    }
    out
}

impl Places {
    pub fn load(data_dir: &Path) -> Places {
        let cities = std::fs::read_to_string(data_dir.join("lists").join("cities.json")).ok();
        let polygons = std::fs::read_to_string(data_dir.join("lists").join("polygons.json")).ok();
        if let (Some(c), Some(p)) = (&cities, &polygons) {
            if let Ok(places) = Places::parse(c, p) {
                let embedded = Places::parse(EMBEDDED_CITIES, EMBEDDED_POLYGONS).expect("embedded lists");
                if places.list.len() >= embedded.list.len() / 2 && places.cities_version >= embedded.cities_version {
                    return places;
                }
                return embedded;
            }
        }
        Places::parse(EMBEDDED_CITIES, EMBEDDED_POLYGONS).expect("embedded lists")
    }

    pub fn embedded() -> Places {
        Places::parse(EMBEDDED_CITIES, EMBEDDED_POLYGONS).expect("embedded lists")
    }

    pub fn parse(cities_json: &str, polygons_json: &str) -> Result<Places, String> {
        let raw: RawLists = serde_json::from_str(cities_json).map_err(|e| e.to_string())?;
        let shapes: HashMap<String, Vec<[f64; 2]>> = serde_json::from_str(polygons_json).map_err(|e| e.to_string())?;
        let mut rows: Vec<(String, RawCity)> = raw.cities.into_iter().collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        let mut list = Vec::with_capacity(rows.len());
        for (name, c) in rows {
            let (lat, lng) = (c.lat.unwrap_or(0.0), c.lng.unwrap_or(0.0));
            if lat == 0.0 || lng == 0.0 {
                continue;
            }
            let (x, y) = project(lat, lng);
            let poly: Vec<(f32, f32)> = shapes
                .get(&c.id.to_string())
                .map(|v| v.iter().map(|p| project(p[0], p[1])).collect())
                .unwrap_or_default();
            let mut bbox = (x, y, x, y);
            for &(px, py) in &poly {
                bbox.0 = bbox.0.min(px);
                bbox.1 = bbox.1.min(py);
                bbox.2 = bbox.2.max(px);
                bbox.3 = bbox.3.max(py);
            }
            let (base, sub) = match name.split_once(" - ") {
                Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
                None => match name.split_once(" -") {
                    Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
                    None => (name.clone(), String::new()),
                },
            };
            list.push(Place {
                id: c.id,
                name: name.clone(),
                base,
                sub,
                en: c.en.unwrap_or_default(),
                area: c.area.unwrap_or(0),
                countdown: c.countdown.unwrap_or(0),
                x,
                y,
                poly,
                bbox,
            });
            let _ = c.he;
        }
        if list.is_empty() {
            return Err("empty list".into());
        }
        let mut areas = HashMap::new();
        let mut areas_en = HashMap::new();
        for (k, v) in raw.areas {
            if let Ok(n) = k.parse::<u16>() {
                areas.insert(n, v.he);
                if let Some(en) = v.en.filter(|e| !e.is_empty()) {
                    areas_en.insert(n, en);
                }
            }
        }
        let mut by_name = HashMap::new();
        let mut by_norm = HashMap::new();
        let mut by_id = HashMap::new();
        let mut norm_names = Vec::with_capacity(list.len());
        for (i, p) in list.iter().enumerate() {
            by_name.insert(p.name.clone(), i);
            let n = normalize(&p.name);
            by_norm.insert(n.clone(), i);
            norm_names.push(n);
            by_id.insert(p.id, i);
        }
        Ok(Places { list, areas, areas_en, cities_version: raw.version.unwrap_or(0), by_name, by_norm, by_id, norm_names })
    }

    pub fn special(name: &str) -> Option<Special> {
        let n = name.trim();
        if n == "ברחבי הארץ" || n == "כל הארץ" || n == "רעידת אדמה" || n == "תרגיל עורף לאומי" {
            Some(Special::AllCountry)
        } else if n.contains("בדיקה") {
            Some(Special::Test)
        } else {
            None
        }
    }

    /// A name from any source: exact, without quotes and dashes, or a unique beginning (Tzofar's list
    /// cuts one Ashdod name, the official list has it whole)
    pub fn find(&self, name: &str) -> Option<usize> {
        if let Some(&i) = self.by_name.get(name) {
            return Some(i);
        }
        let n = normalize(name);
        if let Some(&i) = self.by_norm.get(&n) {
            return Some(i);
        }
        if n.chars().count() < 8 {
            return None;
        }
        let mut hit = None;
        for (i, full) in self.norm_names.iter().enumerate() {
            if full.starts_with(&n) || n.starts_with(full.as_str()) {
                if hit.is_some() {
                    return None;
                }
                hit = Some(i);
            }
        }
        hit
    }

    pub fn by_id(&self, id: u32) -> Option<usize> {
        self.by_id.get(&id).copied()
    }

    pub fn in_area(&self, area: u16) -> Vec<usize> {
        self.list.iter().enumerate().filter(|(_, p)| p.area == area).map(|(i, _)| i).collect()
    }

    pub fn area_name(&self, area: u16) -> &str {
        self.areas.get(&area).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn area_name_en(&self, area: u16) -> &str {
        self.areas_en.get(&area).map(|s| s.as_str()).unwrap_or("")
    }

    /// The place whose shape holds a map point
    pub fn at(&self, x: f32, y: f32) -> Option<usize> {
        for (i, p) in self.list.iter().enumerate() {
            let b = p.bbox;
            if x < b.0 || x > b.2 || y < b.1 || y > b.3 || p.poly.len() < 3 {
                continue;
            }
            let mut inside = false;
            let mut j = p.poly.len() - 1;
            for k in 0..p.poly.len() {
                let (xk, yk) = p.poly[k];
                let (xj, yj) = p.poly[j];
                if (yk > y) != (yj > y) && x < (xj - xk) * (y - yk) / (yj - yk) + xk {
                    inside = !inside;
                }
                j = k;
            }
            if inside {
                return Some(i);
            }
        }
        None
    }

    pub fn distance_km(&self, a: usize, b: usize) -> f32 {
        let (p, q) = (&self.list[a], &self.list[b]);
        ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt()
    }

    /// For the search box: names holding what was typed, the ones that begin with it first
    pub fn search(&self, query: &str, limit: usize) -> Vec<usize> {
        let q = normalize(query);
        if q.is_empty() {
            return vec![];
        }
        let mut first = vec![];
        let mut rest = vec![];
        for (i, n) in self.norm_names.iter().enumerate() {
            if n.starts_with(&q) {
                first.push(i);
            } else if n.contains(&q) {
                rest.push(i);
            }
        }
        first.extend(rest);
        first.truncate(limit);
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_place_has_a_shape_and_a_home_in_the_list() {
        let p = Places::embedded();
        assert_eq!(p.list.len(), 1449);
        let without: Vec<&str> = p.list.iter().filter(|c| c.poly.len() < 3).map(|c| c.name.as_str()).collect();
        assert!(without.is_empty(), "no shape: {without:?}");
        let home_place = &p.list[p.find("רחובות").unwrap()];
        assert_eq!(home_place.countdown, 90);
        assert_eq!(p.area_name(home_place.area), "השפלה");
    }

    #[test]
    fn names_from_the_other_sources_are_found() {
        let p = Places::embedded();
        // the official list writes one quote where Tzofar writes two
        assert!(p.find("כפר חב'ד").is_some());
        assert!(p.find("כפר חב''ד").is_some());
        // Tzofar's list cuts this name by one letter
        assert!(p.find("אשדוד -יא,יב,טו,יז,מרינה,סיטי").is_some());
        // dashes
        assert_eq!(p.find("תל אביב-מרכז העיר"), p.find("תל אביב - מרכז העיר"));
        assert!(p.find("no such place").is_none());
        assert!(p.find("אב").is_none());
    }

    #[test]
    fn every_name_resolves_to_itself_after_normalizing() {
        let p = Places::embedded();
        for (i, c) in p.list.iter().enumerate() {
            assert_eq!(p.find(&c.name), Some(i), "{}", c.name);
            assert_eq!(p.find(&c.name.replace("''", "'")), Some(i), "{}", c.name);
        }
    }

    #[test]
    fn map_lookup_and_distance() {
        let p = Places::embedded();
        let home = p.find("רחובות").unwrap();
        let c = &p.list[home];
        assert_eq!(p.at(c.x, c.y), Some(home));
        let near = p.find("נס ציונה").unwrap();
        let d = p.distance_km(home, near);
        assert!(d > 1.0 && d < 5.0, "{d}");
        let eilat = p.find("אילת").unwrap();
        let d = p.distance_km(home, eilat);
        assert!(d > 250.0 && d < 300.0, "{d}");
        assert!(!p.search("תל אביב", 10).is_empty());
    }

    #[test]
    fn specials() {
        assert_eq!(Places::special("ברחבי הארץ"), Some(Special::AllCountry));
        assert_eq!(Places::special("בדיקה 2"), Some(Special::Test));
        assert_eq!(Places::special("רחובות"), None);
    }
}
