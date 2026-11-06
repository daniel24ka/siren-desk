// (a test build keeps its console; so does a snapshot, see below)
#![cfg_attr(not(test), windows_subsystem = "windows")]
#![allow(dead_code)]
#![allow(static_mut_refs)]

mod app;
mod broadcast;
mod drill;
mod engine;
mod gfx;
mod map;
mod model;
mod net;
mod paths;
mod places;
mod settings;
mod snapshot;
mod sound;
mod sources;
mod ui;

use engine::Engine;
use net::Base;
use places::Places;
use settings::Settings;
use sources::Hub;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING};
use windows::Win32::Foundation::*;
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
use windows::Win32::System::JobObjects::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const TZOFAR: &str = "https://api.tzevaadom.co.il";
const TZOFAR_SITE: &str = "https://www.tzevaadom.co.il";
const OFFICIAL: &str = "https://www.oref.org.il";

#[derive(Default, Debug)]
struct Args {
    child: bool,
    no_supervisor: bool,
    background: bool,
    mute: bool,
    drill: Option<ui::Drill>,
    exit_after: Option<u64>,
    data_dir: Option<PathBuf>,
    base: Option<String>,
    snapshot: Option<(String, String, Option<(u32, u32)>)>,
    broadcast_port: Option<u16>,
    broadcast_page: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--child" => a.child = true,
            "--no-supervisor" => a.no_supervisor = true,
            "--background" => a.background = true,
            "--mute" => a.mute = true,
            "--drill" => {
                a.drill = match it.next().as_deref() {
                    Some("home") => Some(ui::Drill::Home),
                    Some("far") => Some(ui::Drill::Far),
                    Some("big") => Some(ui::Drill::Big),
                    Some("sequence") => Some(ui::Drill::Sequence),
                    _ => None,
                }
            }
            "--exit-after" => a.exit_after = it.next().and_then(|s| s.parse().ok()),
            "--data-dir" => a.data_dir = it.next().map(PathBuf::from),
            "--base" => a.base = it.next(),
            "--broadcast-port" => a.broadcast_port = it.next().and_then(|s| s.parse().ok()),
            "--broadcast-page" => a.broadcast_page = it.next().map(PathBuf::from),
            "--snapshot" => {
                let scene = it.next().unwrap_or_default();
                let out = it.next().unwrap_or_default();
                let rest: Vec<String> = it.by_ref().collect();
                let size = match rest.as_slice() {
                    [w, h, ..] => w.parse().ok().zip(h.parse().ok()),
                    _ => None,
                };
                a.snapshot = Some((scene, out, size));
            }
            _ => {}
        }
    }
    a
}

/// Keeps the program alive: it runs as a child of this small process, and a crash brings it straight back.
/// Only a clean exit (the user's "quit") ends this. The child dies with this process too.
fn supervise() -> i32 {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(_) => return 1,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let job = unsafe {
        let job = CreateJobObjectW(None, windows::core::PCWSTR::null()).ok();
        if let Some(j) = job {
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let _ = SetInformationJobObject(
                j,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
        }
        job
    };
    let mut quick_deaths = 0u32;
    loop {
        let started = Instant::now();
        let mut cmd = std::process::Command::new(&exe);
        cmd.args(&args).arg("--child");
        let status = match cmd.spawn() {
            Ok(mut child) => {
                if let Some(j) = job {
                    use std::os::windows::io::AsRawHandle;
                    unsafe {
                        let _ = AssignProcessToJobObject(j, HANDLE(child.as_raw_handle()));
                    }
                }
                child.wait()
            }
            Err(e) => Err(e),
        };
        match status {
            Ok(s) if s.success() => return 0,
            other => paths::log(&format!("the program ended unexpectedly ({other:?}): starting it again")),
        }
        if started.elapsed() < Duration::from_secs(20) {
            quick_deaths += 1;
        } else {
            quick_deaths = 0;
        }
        // a crash at every start must not spin the CPU, and must not give up either
        std::thread::sleep(Duration::from_secs(1u64 << quick_deaths.min(5)));
    }
}

fn main() {
    let args = parse_args();
    if let Some((scene, out, size)) = &args.snapshot {
        unsafe {
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        }
        match snapshot::render(scene, out, *size) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("snapshot failed: {e}");
                let _ = std::fs::write(format!("{out}.error.txt"), e.to_string());
                std::process::exit(1);
            }
        }
    }
    if let Some(d) = &args.data_dir {
        paths::set_data_dir(d.clone());
    }
    let _ = paths::data_dir();
    std::panic::set_hook(Box::new(|info| {
        paths::log(&format!("PANIC: {info}"));
    }));
    if !args.child && !args.no_supervisor {
        // one copy: a second start only opens the first one's window
        let name = match &args.data_dir {
            Some(d) => format!("Local\\SirenDesk-{}", d.display().to_string().replace(['\\', ':', '/'], "_")),
            None => "Local\\SirenDesk".to_string(),
        };
        let mutex = unsafe { CreateMutexW(None, true, &HSTRING::from(name)) };
        let first = unsafe { GetLastError() } != ERROR_ALREADY_EXISTS;
        if !first || mutex.is_err() {
            unsafe {
                if let Ok(h) = FindWindowW(app::MAIN_CLASS, None) {
                    let _ = PostMessageW(Some(h), app::WM_SHOW_MAIN, WPARAM(0), LPARAM(0));
                }
            }
            return;
        }
        std::process::exit(supervise());
    }
    app::raise_process();
    paths::prune_logs();
    let settings = Settings::load();
    let places = Arc::new(Places::load(paths::data_dir()));
    let policy = settings.policy(&places, args.mute);
    sound::init(settings.vol_home, settings.vol_far);
    sound::warm_up();
    let (tzofar, site, official) = match &args.base {
        Some(b) => (Base::parse(b), Base::parse(b), Base::parse(b).as_browser()),
        None => (Base::parse(TZOFAR), Base::parse(TZOFAR_SITE), Base::parse(OFFICIAL).as_browser()),
    };
    let hub = Arc::new(Hub {
        engine: Mutex::new(Engine::new(places.clone(), policy)),
        health: Mutex::new(Default::default()),
        history: Mutex::new(vec![]),
        tzofar_history: Mutex::new(vec![]),
        official_history: Mutex::new(vec![]),
        places,
        tzofar,
        tzofar_site: site,
        official,
        hwnd: AtomicIsize::new(0),
        stop: AtomicBool::new(false),
        top_message: AtomicI64::new(0),
        rehearsal_id: AtomicI64::new(0),
    });
    // the live broadcast's feed: on 8150 for the real program; a test run only on a port it is given
    let test_run = args.data_dir.is_some() || args.base.is_some();
    if let Some(port) = args.broadcast_port.or(if test_run { None } else { Some(8150) }) {
        broadcast::start(hub.clone(), broadcast::Config { port, page: args.broadcast_page.clone(), test_run });
    }
    let options = app::Options { background: args.background, mute: args.mute, drill: args.drill, exit_after: args.exit_after, test_run: args.data_dir.is_some() || args.base.is_some() };
    match app::run(hub, settings, options) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            paths::log(&format!("the program could not start: {e}"));
            unsafe {
                let text = HSTRING::from(format!("אזעקות לא הצליח לעלות:\n{e}"));
                MessageBoxW(None, &text, w!("אזעקות"), MB_OK | MB_ICONERROR);
            }
            std::process::exit(3);
        }
    }
}
