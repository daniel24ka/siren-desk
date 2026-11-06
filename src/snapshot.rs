//! Pictures with no window, from the same drawing code: `SirenDesk --snapshot <scene> <out.png> [width height]`.
//! Each scene is real engine state at a set moment. This is how the screens are checked without touching the desktop.

use crate::engine::{Engine, Event, Policy, Snapshot, Source};
use crate::gfx::*;
use crate::map::*;
use crate::model::{Kind, Threat};
use crate::paths::now_ms;
use crate::places::Places;
use crate::settings::Settings;
use crate::sources::{Health, HistItem};
use crate::ui::{self, Input, View};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{Interface, Result, HSTRING};
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

fn event(places: &Places, key: &str, kind: Kind, threat: Threat, ago_ms: i64, now: i64, names: &[usize], title: &str, body: &str, drill: bool) -> Event {
    let _ = places;
    Event {
        source: Source::Sirens,
        key: key.to_string(),
        kind,
        threat,
        drill,
        time_s: Some((now - ago_ms) / 1000),
        title: title.to_string(),
        body: body.to_string(),
        places: names.to_vec(),
        unknown: vec![],
        read_at: Instant::now(),
    }
}

fn around(places: &Places, name: &str, km: f32) -> Vec<usize> {
    let c = places.find(name).expect("known place");
    (0..places.list.len()).filter(|&i| places.distance_km(c, i) <= km).collect()
}

struct Scene {
    window: &'static str,
    tab: u8,
    snap: Snapshot,
    health: Health,
    history: Vec<HistItem>,
}

fn healthy() -> Health {
    let mut h = Health { started: Some(Instant::now() - Duration::from_secs(600)), official_status: 403, ..Health::default() };
    for s in [&mut h.sirens, &mut h.messages, &mut h.feed] {
        s.ok_at = Some(Instant::now() - Duration::from_millis(420));
        s.last_ms = 24;
        s.cache_age = Some(2);
        s.n_ok = 100;
    }
    h
}

fn sample_history(places: &Places, now: i64) -> Vec<HistItem> {
    let item = |id: &str, ago_min: i64, kind, threat, name: &str, km: f32, title: &str, body: &str| HistItem {
        id: id.to_string(),
        source: Source::Feed,
        time_s: (now - ago_min * 60_000) / 1000,
        kind,
        threat,
        drill: false,
        places: around(places, name, km),
        unknown: vec![],
        title: title.to_string(),
        body: body.to_string(),
    };
    vec![
        item("a", 42, Kind::End, Threat::Infiltration, "אביתר", 1.0, "סיום אירוע", "אירוע חדירת מחבלים הסתיים באביתר"),
        item("b", 96, Kind::Siren, Threat::Infiltration, "אביתר", 1.0, "", ""),
        item("c", 1440, Kind::Siren, Threat::Aircraft, "ערב אל עראמשה", 8.0, "", ""),
        item("d", 2880, Kind::Siren, Threat::Missiles, "נחל עוז", 6.0, "", ""),
        item("e", 4320, Kind::Pre, Threat::General, "ירושלים - מרכז", 30.0, "התרעה מקדימה", "בדקות הקרובות צפויות להתקבל התרעות"),
        item("f", 4400, Kind::Siren, Threat::Missiles, "מלכיה", 12.0, "", ""),
    ]
}

fn build(name: &str, places: &Arc<Places>) -> Scene {
    let now = now_ms();
    let home = places.find("רחובות").unwrap();
    let policy = Policy { homes: vec![home], quiet_from: 0, quiet_to: 0, ..Policy::default() };
    let mut e = Engine::new(places.clone(), policy);
    let mut window = "main";
    let mut tab = 0;
    let mut health = healthy();
    let mut apply = |ev: Event| {
        e.apply(&ev, now);
    };
    match name {
        "main" => {}
        "main-history" => tab = 1,
        "main-settings" => tab = 2,
        "main-alert" => apply(event(places, "x", Kind::Siren, Threat::Missiles, 12_000, now, &around(places, "חיפה - מערב", 16.0), "", "", false)),
        "panel" => {
            window = "panel";
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 14_000, now, &around(places, "קריית שמונה", 18.0), "", "", false));
        }
        "panel-near" => {
            window = "panel";
            apply(event(places, "x", Kind::Siren, Threat::Aircraft, 9_000, now, &around(places, "ראש העין", 9.0), "", "", false));
        }
        "panel-pre" => {
            window = "panel";
            apply(event(places, "x", Kind::Pre, Threat::General, 20_000, now, &around(places, "רחובות", 60.0), "התרעה מקדימה", "בעקבות זיהוי שיגורים, בדקות הקרובות צפויות להתקבל התרעות באזורים דן, ירקון, השפלה, ירושלים", false));
        }
        "panel-end" => {
            window = "panel";
            let group = around(places, "נחל עוז", 10.0);
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 300_000, now, &group, "", "", false));
            apply(event(places, "y", Kind::End, Threat::General, 3_000, now, &group, "סיום אירוע", "האירוע הסתיים, ניתן לצאת מהמרחב המוגן", false));
        }
        "panel-drill" => {
            window = "panel";
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 5_000, now, &around(places, "קריית שמונה", 16.0), "", "", true));
        }
        "full" => {
            window = "full";
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 8_000, now, &around(places, "רחובות", 7.0), "", "", false));
        }
        "full-aircraft" => {
            window = "full";
            apply(event(places, "x", Kind::Siren, Threat::Aircraft, 8_000, now, &around(places, "רחובות", 7.0), "", "", false));
        }
        "full-hold" => {
            window = "full";
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 200_000, now, &around(places, "רחובות", 7.0), "", "", false));
        }
        "full-end" => {
            window = "full";
            let group = around(places, "רחובות", 7.0);
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 590_000, now, &group, "", "", false));
            apply(event(places, "y", Kind::End, Threat::General, 2_000, now, &group, "סיום אירוע", "האירוע הסתיים, ניתן לצאת מהמרחב המוגן", false));
        }
        "full-big" => {
            window = "full";
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 6_000, now, &around(places, "רחובות", 110.0), "", "", false));
        }
        "full-drill" => {
            window = "full";
            apply(event(places, "x", Kind::Siren, Threat::Missiles, 3_000, now, &around(places, "רחובות", 7.0), "", "", true));
        }
        "strip" => {
            window = "strip";
            health.sirens.ok_at = Some(Instant::now() - Duration::from_secs(95));
            health.sirens.err = "receive: 12002 timeout".to_string();
        }
        "note" => window = "note",
        other => panic!("no scene named {other}"),
    }
    let snap = e.snapshot();
    let history = sample_history(places, now);
    Scene { window, tab, snap, health, history }
}

pub fn render(name: &str, out: &str, size: Option<(u32, u32)>) -> Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let places = Arc::new(Places::embedded());
    let scene = build(name, &places);
    let (w, h) = size.unwrap_or(match scene.window {
        "full" => (1920, 1080),
        "panel" => (640, 1008),
        "strip" => (720, 52),
        "note" => (260, 46),
        _ => (1360, 880),
    });
    let gfx = Gfx::new()?;
    let geo_data = Geo::new(&gfx, &places)?;
    let settings = Settings::default();
    let homes: Vec<usize> = settings.homes.iter().filter_map(|n| places.find(n)).collect();
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap = wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnLoad)?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = gfx.d2d.CreateWicBitmapRenderTarget(&bitmap, &props)?;
        let brush = rt.CreateSolidColorBrush(&rgb(0xFFFFFF).d2d(), None)?;
        let view = View {
            places: &places,
            snap: &scene.snap,
            health: &scene.health,
            settings: &settings,
            history: &scene.history,
            homes: &homes,
            now_ms: now_ms(),
            // (the top of the pulse)
            t: 0.3125,
            siren_on: scene.window == "full",
            output_note: None,
            output_name: Some("Speakers (Realtek(R) Audio)"),
            autostart_on: true,
            geo: &geo_data,
            test_run: false,
        };
        let mut map = MapView::new();
        let mut input = Input::default();
        let mut scroll = 0.0;
        let mut actions = vec![];
        let (fw, fh) = (w as f32, h as f32);
        let started = Instant::now();
        rt.BeginDraw();
        rt.Clear(Some(&rgb(0x000000).d2d()));
        let p = Painter { rt: &rt, gfx: &gfx, brush: &brush };
        match scene.window {
            "full" => ui::draw_full(&p, &view, &mut map, &mut input, fw, fh, &mut scroll, &mut actions),
            "panel" => ui::draw_panel(&p, &view, &mut map, &mut input, fw, fh, &mut scroll, &mut actions),
            "strip" => ui::draw_strip(&p, &view, &mut input, fw, fh, &mut actions),
            "note" => ui::draw_note(&p, &view, fw, fh),
            _ => {
                let _ = ui::draw_main(&p, &view, &mut map, &mut input, fw, fh, scene.tab, "", &mut scroll, None, None, &mut actions);
            }
        }
        rt.EndDraw(None, None)?;
        let took = started.elapsed().as_secs_f32() * 1000.0;
        // to PNG
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(out), GENERIC_WRITE.0)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.expect("a frame");
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        let mut format = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut format)?;
        let source: IWICBitmapSource = bitmap.cast()?;
        frame.WriteSource(&source, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()?;
        println!("{name}: {w}x{h}, first frame drawn by software in {took:.1} ms -> {out}");
    }
    Ok(())
}
