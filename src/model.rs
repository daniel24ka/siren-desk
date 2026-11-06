//! What an alert means: its kind (what to do now) and its threat (what it is about).
//!
//! The official feed's numbers mean opposite things in its live file and its history, so its alerts are
//! read by their WORDING (table assets/titles.json, hand read from the official list, is the test below).
//! Tzofar's feed carries a threat number and, for messages, an instructionType.

use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord, Serialize)]
pub enum Kind {
    End,
    Notice,
    Pre,
    Near,
    Stay,
    Siren,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize)]
pub enum Threat {
    Missiles,
    Aircraft,
    Infiltration,
    Earthquake,
    Tsunami,
    Hazmat,
    Radiological,
    NonConventional,
    Blast,
    General,
}

impl Threat {
    /// Tzofar's numbers, from its own site's code (getColorForThreat): 9 is its drill number
    pub fn from_tzofar(n: i64) -> Threat {
        match n {
            0 => Threat::Missiles,
            1 => Threat::Hazmat,
            2 => Threat::Infiltration,
            3 => Threat::Earthquake,
            4 => Threat::Tsunami,
            5 => Threat::Aircraft,
            6 => Threat::NonConventional,
            7 => Threat::Radiological,
            _ => Threat::General,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Threat::Missiles => "ירי רקטות וטילים",
            Threat::Aircraft => "חדירת כלי טיס עוין",
            Threat::Infiltration => "חדירת מחבלים",
            Threat::Earthquake => "רעידת אדמה",
            Threat::Tsunami => "חשש לצונאמי",
            Threat::Hazmat => "אירוע חומרים מסוכנים",
            Threat::Radiological => "אירוע רדיולוגי",
            Threat::NonConventional => "טיל לא קונבנציונלי",
            Threat::Blast => "סכנת פיצוץ והדף",
            Threat::General => "התרעה",
        }
    }

    /// What to do, in the official wording of each alert
    pub fn action(self) -> &'static str {
        match self {
            Threat::Missiles | Threat::NonConventional | Threat::General => "היכנסו למרחב המוגן",
            Threat::Aircraft | Threat::Blast => "היכנסו מייד למרחב המוגן",
            Threat::Infiltration => "היכנסו מייד למבנה ונעלו את הדלת",
            Threat::Earthquake => "צאו לשטח פתוח",
            Threat::Tsunami => "התרחקו מייד מחוף הים",
            Threat::Hazmat | Threat::Radiological => "הסתגרו מייד במבנה",
        }
    }

    /// Only rockets and missiles have a known flight time: the place's "time to shelter". The rest is "now".
    pub fn has_countdown(self) -> bool {
        matches!(self, Threat::Missiles | Threat::NonConventional)
    }
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Siren => "אזעקה",
            Kind::Stay => "המשיכו לשהות במרחב המוגן",
            Kind::Near => "הישארו בקרבת המרחב המוגן",
            Kind::Pre => "התרעה מקדימה",
            Kind::Notice => "הודעה",
            Kind::End => "האירוע הסתיים",
        }
    }
}

/// How long a place stays on the list with no further word. A siren holds until "event ended" normally
/// comes, ~10 minutes (ballistic missiles); an infiltration lasted 54 minutes on Oct 19 2026. An early warning
/// holds 12 minutes as on Tzofar's own site.
pub fn hold_ms(kind: Kind, threat: Threat) -> i64 {
    const MIN: i64 = 60_000;
    match kind {
        Kind::Siren => {
            if threat == Threat::Infiltration {
                60 * MIN
            } else {
                10 * MIN
            }
        }
        Kind::Stay | Kind::Near => 30 * MIN,
        Kind::Pre => 12 * MIN,
        Kind::Notice => 5 * MIN,
        Kind::End => 90_000,
    }
}

fn any(s: &str, words: &[&str]) -> bool {
    words.iter().any(|w| s.contains(w))
}

fn out_but_near(s: &str) -> bool {
    for head in ["ניתן לצאת", "יציאה ממרחב"] {
        if let Some(i) = s.find(head) {
            if any(&s[i + head.len()..], &["בקרבת", "בסמוך", "בסמיכות", "להישאר"]) {
                return true;
            }
        }
    }
    false
}

fn kind_of(s: &str) -> Option<Kind> {
    if any(s, &["אין לצאת", "לא ניתן לצאת", "אסור לצאת", "אין לשוב", "לא הסתיים", "טרם הסתיים"]) {
        return Some(Kind::Stay);
    }
    if out_but_near(s) {
        return Some(Kind::Near);
    }
    if any(
        s,
        &[
            "הסתיים", "ניתן לצאת", "יכולים לצאת", "ניתן לחזור", "שחרור ממרחב", "הוסר החשש", "החשש הוסר", "נשלל", "שלילת", "חלפה",
            "התרעת שווא", "הסרת אזהרת", "הנחיות לחזרה", "הנחיות חזרה",
        ],
    ) || s.starts_with("סיום ")
    {
        return Some(Kind::End);
    }
    if any(
        s,
        &[
            "אין הנחיות לציבור", "הודעה לציבור", "חזרו לפעול", "חזר לפעול", "להישמע להנחיות", "הישמעו להנחיות", "הנחיות בעקבות",
            "יום הזיכרון", "צפירת דומיה",
        ],
    ) || s == "שיבושים בצופרים"
        || s.starts_with("שיבושים בצופרי")
    {
        return Some(Kind::Notice);
    }
    if any(
        s,
        &[
            "להמשיך לשהות במרחב", "המשך שהייה במרחב", "המשך שהייה במבנה", "המשך הסתגרות", "הישארו בשטח", "הישארו הרחק", "לשהות במרחב מוגן",
            "שהו במרחב המוגן",
        ],
    ) {
        return Some(Kind::Stay);
    }
    if any(s, &["בסמוך", "בסמיכות", "בקרבתו"]) {
        return Some(Kind::Near);
    }
    if any(s, &["בדקות הקרובות", "צפויות להתקבל", "התרעה מוקדמת", "הגברת מוכנות", "הגברת המוכנות", "התקרבו למרחב", "פיקוד העורף מעדכן"]) {
        return Some(Kind::Pre);
    }
    None
}

fn threat_of(t: &str) -> Option<Threat> {
    if any(t, &["רקטות", "טילים"]) {
        Some(Threat::Missiles)
    } else if any(t, &["כלי טיס", "כלי טייס", "כטב\"ם", "כטב״ם"]) {
        Some(Threat::Aircraft)
    } else if t.contains("מחבלים") {
        Some(Threat::Infiltration)
    } else if any(t, &["רעידת אדמה", "רעידת האדמה"]) {
        Some(Threat::Earthquake)
    } else if any(t, &["צונמי", "צונאמי"]) {
        Some(Threat::Tsunami)
    } else if t.contains("חומרים מסוכנים") {
        Some(Threat::Hazmat)
    } else if any(t, &["רדיולוגי", "גרעיני"]) {
        Some(Threat::Radiological)
    } else if t.contains("פיצוץ והדף") {
        Some(Threat::Blast)
    } else if any(t, &["לא קונבנציונלי", "כימי"]) {
        Some(Threat::NonConventional)
    } else {
        None
    }
}

pub struct Reading {
    pub kind: Kind,
    pub threat: Threat,
    pub drill: bool,
}

fn tidy(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An alert of the official feed by its wording; its number only when the wording says nothing known.
/// `hist`: the number is the history's numbering, not the live file's.
pub fn read_official(cat: &str, title: &str, desc: &str, hist: bool) -> Reading {
    let n: i64 = cat.trim().parse().unwrap_or(0);
    let t = tidy(title);
    let drill = t.contains("תרגיל") || if hist { (15..=28).contains(&n) } else { n > 100 };
    let done = |kind, threat: Option<Threat>| Reading { kind, threat: threat.unwrap_or(Threat::General), drill };
    // (these two say nothing by themselves: their instruction may)
    if t == "פיקוד העורף מעדכן" || t.starts_with("הודעה לציבור") {
        if let Some(d) = kind_of(desc) {
            if d != Kind::Notice {
                return done(d, threat_of(desc));
            }
        }
    }
    if let Some(k) = kind_of(&t) {
        return done(k, threat_of(&t));
    }
    if let Some(th) = threat_of(&t) {
        return done(Kind::Siren, Some(th));
    }
    if t.contains("היכנסו") || t.contains("הסתגרו") {
        return done(Kind::Siren, Some(Threat::General));
    }
    // a wording not known yet: the number, the cautious way (a live message may be an early warning)
    if hist {
        return match n {
            13 | 27 => done(Kind::End, None),
            14 | 28 => done(Kind::Pre, None),
            1 | 15 => done(Kind::Siren, Some(Threat::Missiles)),
            2 | 16 => done(Kind::Siren, Some(Threat::Aircraft)),
            7 | 8 | 21 | 22 => done(Kind::Siren, Some(Threat::Earthquake)),
            9 | 23 => done(Kind::Siren, Some(Threat::Radiological)),
            10 | 24 => done(Kind::Siren, Some(Threat::Infiltration)),
            11 | 25 => done(Kind::Siren, Some(Threat::Tsunami)),
            12 | 26 => done(Kind::Siren, Some(Threat::Hazmat)),
            _ => done(Kind::Siren, Some(Threat::General)),
        };
    }
    match n % 100 {
        10 => done(Kind::Pre, None),
        1 => done(Kind::Siren, Some(Threat::Missiles)),
        3 => done(Kind::Siren, Some(Threat::Earthquake)),
        4 => done(Kind::Siren, Some(Threat::Radiological)),
        5 => done(Kind::Siren, Some(Threat::Tsunami)),
        6 => done(Kind::Siren, Some(Threat::Aircraft)),
        7 => done(Kind::Siren, Some(Threat::Hazmat)),
        13 => done(Kind::Siren, Some(Threat::Infiltration)),
        _ => done(Kind::Siren, Some(Threat::General)),
    }
}

/// A Tzofar message: instructionType 0 early warning, 1 event ended, anything else by its wording
pub fn read_tzofar_message(instruction_type: Option<i64>, title: &str, body: &str) -> Option<Reading> {
    let threat = threat_of(body).or_else(|| threat_of(title)).unwrap_or(Threat::General);
    let plain = |kind| Some(Reading { kind, threat, drill: title.contains("תרגיל") });
    match instruction_type {
        Some(0) => plain(Kind::Pre),
        Some(1) => plain(Kind::End),
        Some(_) => match kind_of(&tidy(title)).or_else(|| kind_of(&tidy(body))) {
            Some(k) => plain(k),
            None => plain(Kind::Notice),
        },
        // (no instruction type: news about the app itself, not an alert)
        None => None,
    }
}

/// The official feed's bytes -> text: UTF-8 or UTF-16, with or without a mark, NUL padding and the stray
/// U+0A7B some answers carry removed (each form was seen live, Oct 31 - Nov 3 2026; tv-wall/oref.py).
pub fn decode_feed(raw: &[u8]) -> String {
    fn utf16(raw: &[u8], be: bool) -> String {
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| if be { u16::from_be_bytes([c[0], c[1]]) } else { u16::from_le_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    }
    let text = if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        utf16(&raw[2..], false)
    } else if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
        utf16(&raw[2..], true)
    } else if raw.len() >= 3 && raw[0] == 0xEF && raw[1] == 0xBB && raw[2] == 0xBF {
        String::from_utf8_lossy(&raw[3..]).into_owned()
    } else if raw.len() >= 4 && raw[1] == 0 && raw[3] == 0 {
        utf16(raw, false)
    } else {
        String::from_utf8_lossy(raw).into_owned()
    };
    text.replace(['\0', '\u{FEFF}', '\u{0A7B}'], "").trim().to_string()
}

/// Real places only: the feed's own test entries ("בדיקה", "בדיקה 1"...) are not places
pub fn is_feed_test_name(name: &str) -> bool {
    name.contains("בדיקה")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Row {
        t: String,
        c: i64,
        k: String,
        th: String,
        dr: bool,
    }

    fn kind_name(k: Kind) -> &'static str {
        match k {
            Kind::Siren => "siren",
            Kind::Stay => "stay",
            Kind::Near => "near",
            Kind::Pre => "pre",
            Kind::Notice => "notice",
            Kind::End => "end",
        }
    }

    fn threat_name(t: Threat) -> &'static str {
        match t {
            Threat::Missiles => "missiles",
            Threat::Aircraft => "aircraft",
            Threat::Infiltration => "infiltration",
            Threat::Earthquake => "earthquake",
            Threat::Tsunami => "tsunami",
            Threat::Hazmat => "hazmat",
            Threat::Radiological => "radiological",
            Threat::NonConventional => "nonconventional",
            Threat::Blast => "blast",
            Threat::General => "general",
        }
    }

    /// Every official wording (93 rows, read by hand, tv-wall/android/make_titles.py) reads as the table says
    #[test]
    fn every_official_wording_reads_as_the_table_says() {
        let rows: Vec<Row> = serde_json::from_str(include_str!("../assets/titles.json")).unwrap();
        assert_eq!(rows.len(), 93);
        let mut wrong = vec![];
        for r in &rows {
            let got = read_official(&r.c.to_string(), &r.t, "", true);
            if kind_name(got.kind) != r.k {
                wrong.push(format!("{} -> {} (table: {})", r.t, kind_name(got.kind), r.k));
            }
            if !r.th.is_empty() && threat_name(got.threat) != r.th {
                wrong.push(format!("{} -> threat {} (table: {})", r.t, threat_name(got.threat), r.th));
            }
            if r.dr && !got.drill {
                wrong.push(format!("{} -> not read as a drill", r.t));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn the_flipped_numbers_do_not_decide_a_known_wording() {
        // the live feed's 13 is an infiltration alert, the history's 13 an update
        let live = read_official("13", "חדירת מחבלים - היכנסו מייד למרחב המוגן", "", false);
        assert_eq!((live.kind, live.threat), (Kind::Siren, Threat::Infiltration));
        let clear = read_official("10", "הוסר החשש לחדירת מחבלים", "", false);
        assert_eq!(clear.kind, Kind::End);
        let pre = read_official("10", "פיקוד העורף מעדכן", "בדקות הקרובות צפויות להתקבל התרעות באזורך", false);
        assert_eq!(pre.kind, Kind::Pre);
    }

    #[test]
    fn tzofar_messages() {
        let m = read_tzofar_message(Some(0), "התרעה מקדימה", "בעקבות זיהוי שיגורים").unwrap();
        assert_eq!(m.kind, Kind::Pre);
        let m = read_tzofar_message(Some(1), "סיום אירוע", "אירוע חדירת מחבלים הסתיים *באביתר*").unwrap();
        assert_eq!((m.kind, m.threat), (Kind::End, Threat::Infiltration));
        assert!(read_tzofar_message(None, "תרגיל התרעות פנימי", "").is_none());
    }

    #[test]
    fn feed_encodings() {
        assert_eq!(decode_feed(b"\xef\xbb\xbf\r\n"), "");
        assert_eq!(decode_feed(b"\xef\xbb\xbf{\"a\":1}\r\n"), "{\"a\":1}");
        let text = "{\"title\":\"אב\"}";
        let le: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let mut bom = vec![0xFF, 0xFE];
        bom.extend_from_slice(&le);
        assert_eq!(decode_feed(&bom), text);
        assert_eq!(decode_feed(&le), text);
        let be: Vec<u8> = text.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let mut bom = vec![0xFE, 0xFF];
        bom.extend_from_slice(&be);
        assert_eq!(decode_feed(&bom), text);
        assert_eq!(decode_feed(&[0u8; 20]), "");
    }
}
