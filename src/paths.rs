//! Where the program keeps its files, the clock, and its log.
//! Everything lives next to the exe, never in AppData: the Claude app's sandbox virtualizes AppData, and a program
//! started from it and the same program started by the user would keep two different copies.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_data_dir(p: PathBuf) {
    let _ = fs::create_dir_all(p.join("log"));
    let _ = DATA_DIR.set(p);
}

pub fn data_dir() -> &'static Path {
    DATA_DIR.get_or_init(|| {
        let p = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("data"))).unwrap_or_else(|| PathBuf::from("data"));
        let _ = fs::create_dir_all(p.join("log"));
        p
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Local {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

pub fn local_time(ms: i64) -> Local {
    // (Windows counts 100 ns from 1601)
    let ticks = (ms as u64 + 11_644_473_600_000) * 10_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut loc = SYSTEMTIME::default();
    unsafe {
        if FileTimeToSystemTime(&ft, &mut utc).is_err() || SystemTimeToTzSpecificLocalTime(None, &utc, &mut loc).is_err() {
            return Local::default();
        }
    }
    Local { year: loc.wYear, month: loc.wMonth, day: loc.wDay, hour: loc.wHour, minute: loc.wMinute, second: loc.wSecond }
}

pub fn local_hour(ms: i64) -> u8 {
    local_time(ms).hour as u8
}

pub fn clock(ms: i64) -> String {
    let t = local_time(ms);
    format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
}

pub fn day_and_time(ms: i64) -> String {
    let t = local_time(ms);
    format!("{:02}.{:02} {:02}:{:02}", t.day, t.month, t.hour, t.minute)
}

/// mm:ss, or h:mm:ss from an hour up
pub fn elapsed(seconds: i64) -> String {
    let s = seconds.max(0);
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

static LOG: OnceLock<Mutex<Option<(String, File)>>> = OnceLock::new();

/// One line in data/log/siren-YYYY-MM-DD.log: what changed, never one line per poll
pub fn log(msg: &str) {
    let now = now_ms();
    let t = local_time(now);
    let day = format!("{:04}-{:02}-{:02}", t.year, t.month, t.day);
    let cell = LOG.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = cell.lock() else { return };
    let fresh = !matches!(&*guard, Some((d, _)) if *d == day);
    if fresh {
        let path = data_dir().join("log").join(format!("siren-{day}.log"));
        *guard = OpenOptions::new().create(true).append(true).open(path).ok().map(|f| (day.clone(), f));
    }
    if let Some((_, f)) = guard.as_mut() {
        let _ = writeln!(f, "{}.{:03} {}", clock(now), now % 1000, msg);
    }
}

/// Logs older than two weeks go
pub fn prune_logs() {
    let Ok(dir) = fs::read_dir(data_dir().join("log")) else { return };
    for e in dir.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).map(|a| a.as_secs() > 14 * 86400).unwrap_or(false);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// One JSON line per alert in data/alerts.jsonl: the record to measure real delays from
pub fn record_alert(line: &str) {
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(data_dir().join("alerts.jsonl")) {
        let _ = writeln!(f, "{line}");
    }
}

/// Written whole or not at all
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}
