//! The windows: the main window, one alert window per monitor, the warning strip and the start note; the tray icon;
//! and the loop that draws only what is on screen and only when something moved (nothing visible: nothing drawn).

use crate::drill;
use crate::engine::{Cue, Level, Snapshot};
use crate::gfx::*;
use crate::map::*;
use crate::paths::{self, now_ms};
use crate::places::Places;
use crate::settings::Settings;
use crate::sound;
use crate::sources::{self, Health, HistItem, Hub, Protection, WM_WAKE};
use crate::ui::{self, Action, Input, View};
use std::mem::size_of;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{w, Interface, Result, BOOL, HSTRING, PCWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const MAIN_CLASS: PCWSTR = w!("SirenDeskMain");
const OVERLAY_CLASS: PCWSTR = w!("SirenDeskOverlay");
pub const WM_SHOW_MAIN: u32 = WM_APP + 3;
const WM_TRAY: u32 = WM_APP + 2;
const ID_EDIT: isize = 77;
const MENU_OPEN: usize = 1;
const MENU_SNOOZE: usize = 2;
const MENU_DRILL: usize = 3;
const MENU_QUIT: usize = 9;

#[derive(Clone)]
pub struct Options {
    pub background: bool,
    pub mute: bool,
    pub drill: Option<ui::Drill>,
    pub exit_after: Option<u64>,
    pub test_run: bool,
}

struct Surface {
    hwnd_rt: ID2D1HwndRenderTarget,
    rt: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    px: (u32, u32),
    dpi: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Hidden,
    Full,
    Panel,
}

struct Win {
    hwnd: HWND,
    surface: Option<Surface>,
    map: MapView,
    input: Input,
    scroll: f32,
    dirty: bool,
    visible: bool,
    dpi: f32,
    w: f32,
    h: f32,
    last_draw: Option<Instant>,
}

impl Win {
    fn new(hwnd: HWND) -> Win {
        Win { hwnd, surface: None, map: MapView::new(), input: Input::default(), scroll: 0.0, dirty: true, visible: false, dpi: 96.0, w: 1.0, h: 1.0, last_draw: None }
    }

    /// Mouse position from a message's lparam, in DIPs
    fn set_mouse(&mut self, lp: LPARAM) {
        let (x, y) = ((lp.0 & 0xFFFF) as i16 as f32, ((lp.0 >> 16) & 0xFFFF) as i16 as f32);
        self.input.x = x * 96.0 / self.dpi;
        self.input.y = y * 96.0 / self.dpi;
        self.input.inside = true;
    }
}

#[derive(Clone, Copy)]
struct Monitor {
    rect: RECT,
    work: RECT,
    dpi: f32,
    primary: bool,
}

unsafe extern "system" fn monitor_proc(mon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
    let list = &mut *(data.0 as *mut Vec<Monitor>);
    let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(mon, &mut info).as_bool() {
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        list.push(Monitor { rect: info.rcMonitor, work: info.rcWork, dpi: dx as f32, primary: info.dwFlags & MONITORINFOF_PRIMARY != 0 });
    }
    BOOL(1)
}

fn monitors() -> Vec<Monitor> {
    let mut list: Vec<Monitor> = vec![];
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(monitor_proc), LPARAM(&mut list as *mut _ as isize));
    }
    list.sort_by_key(|m| !m.primary);
    list
}

/// The monitor the user is looking at: the one with the window in front
fn active_monitor(list: &[Monitor]) -> Monitor {
    unsafe {
        let fg = GetForegroundWindow();
        if !fg.0.is_null() {
            let mon = MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(mon, &mut info).as_bool() {
                if let Some(m) = list.iter().find(|m| m.rect.left == info.rcMonitor.left && m.rect.top == info.rcMonitor.top) {
                    return *m;
                }
            }
        }
    }
    list.first().copied().unwrap_or(Monitor { rect: RECT { left: 0, top: 0, right: 1920, bottom: 1080 }, work: RECT { left: 0, top: 0, right: 1920, bottom: 1040 }, dpi: 96.0, primary: true })
}

fn make_surface(gfx: &Gfx, hwnd: HWND, dpi: f32) -> Result<Surface> {
    unsafe {
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc)?;
        let px = (((rc.right - rc.left).max(1)) as u32, ((rc.bottom - rc.top).max(1)) as u32);
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
            dpiX: dpi,
            dpiY: dpi,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        // (drawn as soon as ready, not at the next refresh of a 240 Hz screen: the loop keeps its own pace)
        let hwnd_props = D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd, pixelSize: D2D_SIZE_U { width: px.0, height: px.1 }, presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY };
        let hwnd_rt = gfx.d2d.CreateHwndRenderTarget(&props, &hwnd_props)?;
        let rt: ID2D1RenderTarget = hwnd_rt.cast()?;
        let brush = rt.CreateSolidColorBrush(&rgb(0xFFFFFF).d2d(), None)?;
        Ok(Surface { hwnd_rt, rt, brush, px, dpi })
    }
}

fn set_dark_frame(hwnd: HWND) {
    unsafe {
        let on: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const i32 as *const _, 4);
    }
}

fn set_round_corners(hwnd: HWND, round: bool) {
    unsafe {
        let pref = if round { DWMWCP_ROUND } else { DWMWCP_DONOTROUND };
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &pref as *const _ as *const _, size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32);
    }
}

// ---------------------------------------------------------------- starting with Windows

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("SirenDesk");

pub fn autostart_get() -> bool {
    unsafe {
        let mut size = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok()
    }
}

pub fn autostart_set(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).is_err() {
            paths::log("autostart: the Run key would not open");
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let text = format!("\"{}\" --background", exe.display());
                let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
                let bytes = std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
                let r = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes));
                if r.is_err() {
                    paths::log(&format!("autostart not set: {r:?}"));
                }
            }
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

// ---------------------------------------------------------------- the tray icon

/// A round icon, drawn pixel by pixel: the colour says the state at a glance
fn make_icon(color: Color, alert: bool) -> Option<HICON> {
    const S: i32 = 32;
    let mut bits: Vec<u8> = Vec::with_capacity((40 + S * S * 4 + S * 4) as usize);
    let header = BITMAPINFOHEADER {
        biSize: 40,
        biWidth: S,
        biHeight: S * 2,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: 0,
        ..Default::default()
    };
    bits.extend_from_slice(unsafe { std::slice::from_raw_parts(&header as *const _ as *const u8, 40) });
    let c = (S as f32 - 1.0) / 2.0;
    for y in (0..S).rev() {
        for x in 0..S {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let d = (dx * dx + dy * dy).sqrt();
            let disc = (15.5 - d).clamp(0.0, 1.0);
            // a white core for "listening", none when it is not
            let core = if alert { 0.0 } else { (4.5 - d).clamp(0.0, 1.0) };
            let ring = ((1.2 - (d - 9.5).abs()).clamp(0.0, 1.0)) * 0.55;
            let w_mix = (core + ring).min(1.0);
            let r_ = color.r + (1.0 - color.r) * w_mix;
            let g_ = color.g + (1.0 - color.g) * w_mix;
            let b_ = color.b + (1.0 - color.b) * w_mix;
            let a = disc;
            bits.push((b_ * a * 255.0) as u8);
            bits.push((g_ * a * 255.0) as u8);
            bits.push((r_ * a * 255.0) as u8);
            bits.push((a * 255.0) as u8);
        }
    }
    bits.extend(std::iter::repeat_n(0u8, (S * 4) as usize));
    unsafe { CreateIconFromResourceEx(&bits, true, 0x00030000, S, S, LR_DEFAULTCOLOR).ok() }
}

struct Tray {
    hwnd: HWND,
    icons: [Option<HICON>; 4],
    shown: i32,
    tip: String,
    added: bool,
}

impl Tray {
    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW { cbSize: size_of::<NOTIFYICONDATAW>() as u32, hWnd: self.hwnd, uID: 1, ..Default::default() }
    }

    fn add(&mut self) {
        let mut d = self.data();
        d.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        d.uCallbackMessage = WM_TRAY;
        d.hIcon = self.icons[1].unwrap_or_default();
        self.write_tip(&mut d);
        unsafe {
            self.added = Shell_NotifyIconW(NIM_ADD, &d).as_bool();
        }
    }

    fn write_tip(&self, d: &mut NOTIFYICONDATAW) {
        for (slot, u) in d.szTip.iter_mut().zip(self.tip.encode_utf16().take(126)) {
            *slot = u;
        }
    }

    fn update(&mut self, state: i32, tip: &str) {
        if state == self.shown && tip == self.tip {
            return;
        }
        self.shown = state;
        self.tip = tip.to_string();
        let mut d = self.data();
        d.uFlags = NIF_ICON | NIF_TIP;
        d.hIcon = self.icons[state as usize].unwrap_or_default();
        self.write_tip(&mut d);
        unsafe {
            if !Shell_NotifyIconW(NIM_MODIFY, &d).as_bool() {
                self.added = false;
            }
        }
        if !self.added {
            self.add();
        }
    }

    fn remove(&self) {
        let d = self.data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
        }
    }
}

// ---------------------------------------------------------------- the speakers' state

/// Where sound goes now, by the name Windows shows. A siren sent to a TV that is off or a virtual cable is silent.
fn output_device() -> Option<String> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        let store = device.OpenPropertyStore(STGM_READ).ok()?;
        let name = store.GetValue(&PKEY_Device_FriendlyName).ok()?.to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }
}

/// A warning to show when a siren could not be heard. Needs COM on this thread.
fn output_warning() -> Option<String> {
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let device = match devices.GetDefaultAudioEndpoint(eRender, eConsole) {
            Ok(d) => d,
            Err(_) => return Some("אין התקן שמע פעיל: לא תשמע סירנה".to_string()),
        };
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        if volume.GetMute().ok()?.as_bool() {
            return Some("המחשב מושתק: לא תשמע סירנה".to_string());
        }
        let level = volume.GetMasterVolumeLevelScalar().ok()?;
        if level < 0.15 {
            return Some(format!("עוצמת המחשב {:.0}%: הסירנה תישמע חלש", level * 100.0));
        }
        None
    }
}

// ---------------------------------------------------------------- the program

pub struct App {
    hub: Arc<Hub>,
    gfx: Gfx,
    geo: Geo,
    places: Arc<Places>,
    settings: Settings,
    options: Options,
    homes: Vec<usize>,
    snap: Snapshot,
    health: Health,
    history: Vec<HistItem>,
    main: Win,
    alerts: Vec<Win>,
    strip: Win,
    note: Win,
    tray: Tray,
    mode: Mode,
    panel_monitor: Option<Monitor>,
    edit: HWND,
    edit_font: HFONT,
    edit_brush: HBRUSH,
    edit_rect: Option<R>,
    search: String,
    tab: u8,
    picked: Option<usize>,
    hover_place: Option<usize>,
    drag: Option<(f32, f32)>,
    seq_seen: u64,
    t0: Instant,
    last_tick: Instant,
    last_slow: Instant,
    output_note: Option<String>,
    output_name: Option<String>,
    autostart_on: bool,
    timer: HANDLE,
    taskbar_created: u32,
    strip_hidden_for_outage: bool,
    outage_since: Option<Instant>,
    down_toned: bool,
    note_until: Option<Instant>,
    measure_first_frame: Option<Instant>,
    frame_ms_max: f32,
    frame_ms_sum: f32,
    frame_count: u32,
    started_ms: i64,
    quitting: bool,
    display_held: bool,
    last_topmost: Instant,
}

static mut APP: *mut App = std::ptr::null_mut();

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if !APP.is_null() {
        return (*APP).on_message(hwnd, msg, wp, lp);
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

fn register_classes() -> Result<()> {
    unsafe {
        let inst: HINSTANCE = GetModuleHandleW(None)?.into();
        let icon = LoadIconW(Some(inst), PCWSTR(1 as *const u16)).unwrap_or_default();
        let dark = CreateSolidBrush(COLORREF(0x00180B0B));
        let main = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: inst,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: dark,
            lpszClassName: MAIN_CLASS,
            hIcon: icon,
            hIconSm: icon,
            ..Default::default()
        };
        RegisterClassExW(&main);
        let overlay = WNDCLASSEXW { lpszClassName: OVERLAY_CLASS, ..main };
        RegisterClassExW(&overlay);
    }
    Ok(())
}

fn popup(title: PCWSTR) -> Result<HWND> {
    unsafe {
        let inst: HINSTANCE = GetModuleHandleW(None)?.into();
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            OVERLAY_CLASS,
            title,
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(inst),
            None,
        )
    }
}

impl App {
    fn window_kind(&mut self, hwnd: HWND) -> Option<&mut Win> {
        if self.main.hwnd == hwnd {
            return Some(&mut self.main);
        }
        if self.strip.hwnd == hwnd {
            return Some(&mut self.strip);
        }
        if self.note.hwnd == hwnd {
            return Some(&mut self.note);
        }
        self.alerts.iter_mut().find(|w| w.hwnd == hwnd)
    }

    unsafe fn on_message(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_WAKE => return LRESULT(0),
            WM_SHOW_MAIN => {
                self.show_main();
                return LRESULT(0);
            }
            WM_TRAY => {
                let event = (lp.0 & 0xFFFF) as u32;
                match event {
                    WM_LBUTTONUP | NIN_SELECT => self.show_main(),
                    WM_RBUTTONUP | WM_CONTEXTMENU => self.tray_menu(),
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_COMMAND => {
                let id = wp.0 & 0xFFFF;
                let code = ((wp.0 >> 16) & 0xFFFF) as u32;
                if id == ID_EDIT as usize && code == EN_CHANGE {
                    let mut buf = [0u16; 128];
                    let n = GetWindowTextW(self.edit, &mut buf);
                    self.search = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
                    self.main.dirty = true;
                }
                match id {
                    MENU_OPEN => self.show_main(),
                    MENU_SNOOZE => {
                        let now = now_ms();
                        self.run_action(Action::Snooze(if self.settings.snooze_until_ms > now { 0 } else { now + 3_600_000 }));
                    }
                    MENU_DRILL => {
                        self.run_action(Action::Drill(ui::Drill::Far));
                    }
                    MENU_QUIT => self.ask_quit(),
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT => {
                let dc = HDC(wp.0 as *mut _);
                SetTextColor(dc, COLORREF(0x00F9F6F3));
                SetBkColor(dc, COLORREF(0x002D2319));
                return LRESULT(self.edit_brush.0 as isize);
            }
            WM_QUERYENDSESSION => return LRESULT(1),
            WM_ENDSESSION => {
                if wp.0 != 0 {
                    self.tray.remove();
                    std::process::exit(0);
                }
                return LRESULT(0);
            }
            WM_POWERBROADCAST => {
                paths::log(&format!("power event {}", wp.0));
                return LRESULT(1);
            }
            WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
                self.main.dirty = true;
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            _ => {}
        }
        if msg == self.taskbar_created && msg != 0 {
            self.tray.added = false;
            self.tray.add();
            return LRESULT(0);
        }
        let is_main = hwnd == self.main.hwnd;
        match msg {
            WM_CLOSE => {
                if is_main {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    self.main.visible = false;
                    self.main.map.highlight = Highlight::default();
                    self.picked = None;
                }
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                if let Some(win) = self.window_kind(hwnd) {
                    win.dirty = true;
                }
                return LRESULT(0);
            }
            WM_MOUSEACTIVATE if !is_main => return LRESULT(MA_NOACTIVATE as isize),
            WM_SIZE => {
                let (px_w, px_h) = ((lp.0 & 0xFFFF) as u32, ((lp.0 >> 16) & 0xFFFF) as u32);
                if let Some(win) = self.window_kind(hwnd) {
                    win.w = px_w as f32 * 96.0 / win.dpi;
                    win.h = px_h as f32 * 96.0 / win.dpi;
                    win.dirty = true;
                    if let Some(s) = &mut win.surface {
                        if px_w > 0 && px_h > 0 && s.px != (px_w, px_h) {
                            if s.hwnd_rt.Resize(&D2D_SIZE_U { width: px_w, height: px_h }).is_ok() {
                                s.px = (px_w, px_h);
                            } else {
                                win.surface = None;
                            }
                            win.map.lost();
                            win.map.invalidate_fit();
                        }
                    }
                }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xFFFF) as f32;
                if let Some(win) = self.window_kind(hwnd) {
                    win.dpi = dpi;
                    win.surface = None;
                    win.map.lost();
                    win.dirty = true;
                }
                let rc = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, SWP_NOZORDER | SWP_NOACTIVATE);
                return LRESULT(0);
            }
            WM_MOUSEMOVE => {
                let drag = self.drag;
                let mut track = false;
                let mut pan = None;
                if let Some(win) = self.window_kind(hwnd) {
                    if !win.input.inside {
                        track = true;
                    }
                    let (ox, oy) = (win.input.x, win.input.y);
                    win.set_mouse(lp);
                    win.dirty = true;
                    if is_main && drag.is_some() {
                        pan = Some((win.input.x - ox, win.input.y - oy));
                    }
                }
                if let Some((dx, dy)) = pan {
                    self.main.map.user_pan(dx, dy);
                }
                if is_main {
                    self.update_hover();
                }
                if track {
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut t);
                }
                return LRESULT(0);
            }
            WM_MOUSELEAVE => {
                if let Some(win) = self.window_kind(hwnd) {
                    win.input.inside = false;
                    win.dirty = true;
                }
                if is_main {
                    self.hover_place = None;
                }
                return LRESULT(0);
            }
            WM_LBUTTONDOWN => {
                let mut start_drag = None;
                let side_w = 440.0f32.min(self.main.w * 0.45);
                let main_w = self.main.w;
                if let Some(win) = self.window_kind(hwnd) {
                    win.set_mouse(lp);
                    win.input.down = true;
                    win.dirty = true;
                    if is_main && win.input.x < main_w - side_w && win.input.x > 56.0 {
                        start_drag = Some((win.input.x, win.input.y));
                    }
                }
                if start_drag.is_some() {
                    self.drag = start_drag;
                    let _ = SetCapture(hwnd);
                }
                return LRESULT(0);
            }
            WM_LBUTTONUP => {
                if is_main && self.drag.take().is_some() {
                    let _ = ReleaseCapture();
                }
                if let Some(win) = self.window_kind(hwnd) {
                    win.set_mouse(lp);
                    win.input.down = false;
                    win.input.clicked = true;
                    win.dirty = true;
                }
                return LRESULT(0);
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
                let mut pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
                let _ = ScreenToClient(hwnd, &mut pt);
                let side_w = 440.0f32.min(self.main.w * 0.45);
                if let Some(win) = self.window_kind(hwnd) {
                    let (x, y) = (pt.x as f32 * 96.0 / win.dpi, pt.y as f32 * 96.0 / win.dpi);
                    win.input.x = x;
                    win.input.y = y;
                    win.dirty = true;
                    if is_main && x < win.w - side_w {
                        let (mw, mh) = (win.w - side_w, win.h);
                        win.map.user_zoom(mw, mh, x, y, 1.22f32.powf(delta));
                    } else {
                        win.input.wheel += delta;
                    }
                }
                return LRESULT(0);
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    if let Some(win) = self.window_kind(hwnd) {
                        if win.input.hand {
                            SetCursor(LoadCursorW(None, IDC_HAND).ok());
                            return LRESULT(1);
                        }
                    }
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }

    fn update_hover(&mut self) {
        let side_w = 440.0f32.min(self.main.w * 0.45);
        let (mw, mh) = (self.main.w - side_w, self.main.h);
        let (x, y) = (self.main.input.x, self.main.input.y);
        self.hover_place = if self.drag.is_none() && self.main.input.inside && x < mw {
            let (wx, wy) = self.main.map.cam.to_world(mw, mh, x, y);
            self.places.at(wx, wy)
        } else {
            None
        };
    }

    fn show_main(&mut self) {
        unsafe {
            let _ = ShowWindow(self.main.hwnd, if IsIconic(self.main.hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
            let _ = SetForegroundWindow(self.main.hwnd);
        }
        self.main.visible = true;
        self.main.dirty = true;
    }

    fn tray_menu(&mut self) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let snoozed = self.settings.snooze_until_ms > now_ms();
            let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("פתיחת המפה"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_DRILL, w!("תרגיל: אזעקה רחוקה"));
            let _ = AppendMenuW(menu, MF_STRING, MENU_SNOOZE, if snoozed { w!("ביטול השתקת רחוקות") } else { w!("השתק אזעקות רחוקות לשעה") });
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("יציאה"));
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.main.hwnd);
            let _ = TrackPopupMenuEx(menu, (TPM_RIGHTBUTTON | TPM_LAYOUTRTL).0, pt.x, pt.y, self.main.hwnd, None);
            let _ = PostMessageW(Some(self.main.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
        }
    }

    fn ask_quit(&mut self) {
        let answer = unsafe {
            MessageBoxW(
                Some(self.main.hwnd),
                w!("אחרי יציאה המחשב לא יציג אזעקות. לצאת?"),
                w!("אזעקות"),
                MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2 | MB_RTLREADING | MB_RIGHT,
            )
        };
        if answer == IDYES {
            self.quit();
        }
    }

    fn quit(&mut self) {
        paths::log("quit by the user");
        self.quitting = true;
        self.hub.stop.store(true, Ordering::Relaxed);
        self.tray.remove();
        unsafe { PostQuitMessage(0) };
    }

    fn save_settings(&mut self) {
        self.settings.save();
        self.homes = self.settings.homes.iter().filter_map(|n| self.places.find(n)).collect();
        let policy = self.settings.policy(&self.places, self.options.mute);
        self.hub.engine.lock().unwrap().set_policy(policy);
        self.main.dirty = true;
    }

    fn run_action(&mut self, a: Action) {
        match a {
            Action::Close => {
                self.mode = Mode::Hidden;
                self.hide_alerts();
            }
            Action::CloseStrip => {
                self.strip_hidden_for_outage = true;
                self.hide_strip();
            }
            Action::Mute => {
                sound::stop_siren();
                self.hub.engine.lock().unwrap().silence();
            }
            Action::Tab(t) => {
                self.tab = t;
                self.main.scroll = 0.0;
                if t != 1 {
                    self.picked = None;
                    self.main.map.highlight = Highlight::default();
                }
                self.search.clear();
                unsafe {
                    let _ = SetWindowTextW(self.edit, PCWSTR::null());
                }
            }
            Action::PickHistory(k) => {
                if self.picked == Some(k) {
                    self.picked = None;
                    self.main.map.highlight = Highlight::default();
                } else if let Some(h) = self.history.get(k) {
                    self.picked = Some(k);
                    let c = match h.kind {
                        crate::model::Kind::Siren => threat_color(h.threat),
                        crate::model::Kind::End => rgb(0x2ED47A),
                        crate::model::Kind::Pre => rgb(0xFFB020),
                        _ => rgb(0x5B9BD5),
                    };
                    self.main.map.highlight = Highlight { id: h.id.clone(), places: h.places.clone(), color: Some((c.r, c.g, c.b)) };
                    self.main.map.user_reset();
                }
            }
            Action::AddHome(i) => {
                let name = self.places.list[i].name.clone();
                if !self.settings.homes.contains(&name) {
                    self.settings.homes.push(name);
                }
                self.search.clear();
                unsafe {
                    let _ = SetWindowTextW(self.edit, PCWSTR::null());
                }
                self.save_settings();
            }
            Action::RemoveHome(i) => {
                let name = self.places.list[i].name.clone();
                self.settings.homes.retain(|n| *n != name);
                self.save_settings();
            }
            Action::FarMode(m) => {
                self.settings.far_mode = m;
                self.save_settings();
            }
            Action::ToggleFarSound => {
                self.settings.far_sound = !self.settings.far_sound;
                self.save_settings();
            }
            Action::ToggleNight => {
                if self.settings.quiet_from != self.settings.quiet_to {
                    self.settings.quiet_from = 0;
                    self.settings.quiet_to = 0;
                } else {
                    self.settings.quiet_from = 23;
                    self.settings.quiet_to = 7;
                }
                self.save_settings();
            }
            Action::ToggleAllMonitors => {
                self.settings.all_monitors = !self.settings.all_monitors;
                self.save_settings();
            }
            Action::ToggleAutostart => {
                self.settings.autostart = !self.autostart_on;
                autostart_set(self.settings.autostart);
                self.autostart_on = autostart_get();
                self.save_settings();
            }
            Action::VolHome(d) => {
                self.settings.vol_home = (self.settings.vol_home as i32 + d).clamp(10, 100) as u8;
                sound::init(self.settings.vol_home, self.settings.vol_far);
                self.save_settings();
            }
            Action::VolFar(d) => {
                self.settings.vol_far = (self.settings.vol_far as i32 + d).clamp(10, 100) as u8;
                sound::init(self.settings.vol_home, self.settings.vol_far);
                self.save_settings();
            }
            Action::Drill(kind) => {
                drill::start(self.hub.clone(), kind);
            }
            Action::StopDrill => {
                drill::stop(&self.hub);
                self.hub.poke();
            }
            Action::TestSound => {
                if !self.options.mute {
                    sound::play(Cue::Siren);
                    let _ = std::thread::Builder::new().name("test-sound".into()).spawn(|| {
                        std::thread::sleep(Duration::from_secs(3));
                        sound::stop_siren();
                    });
                }
            }
            Action::Snooze(until) => {
                self.settings.snooze_until_ms = until;
                self.save_settings();
            }
            Action::OpenOfficial => unsafe {
                ShellExecuteW(None, w!("open"), w!("https://www.oref.org.il/"), None, None, SW_SHOWNORMAL);
            },
            Action::ResetMap => self.main.map.user_reset(),
            Action::ZoomIn | Action::ZoomOut => {
                let side_w = 440.0f32.min(self.main.w * 0.45);
                let (mw, mh) = (self.main.w - side_w, self.main.h);
                let f = if a == Action::ZoomIn { 1.5 } else { 1.0 / 1.5 };
                self.main.map.user_zoom(mw, mh, mw / 2.0, mh / 2.0, f);
            }
            Action::FullScreenNow => self.show_full(false),
        }
        self.main.dirty = true;
        for a in &mut self.alerts {
            a.dirty = true;
        }
    }

    fn hide_alerts(&mut self) {
        for w in &mut self.alerts {
            if w.visible {
                unsafe {
                    let _ = ShowWindow(w.hwnd, SW_HIDE);
                }
                w.visible = false;
                w.input.inside = false;
                w.scroll = 0.0;
            }
        }
        if self.display_held {
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS);
            }
            self.display_held = false;
        }
    }

    fn place_alert(&mut self, index: usize, x: i32, y: i32, w_px: i32, h_px: i32, dpi: f32, round: bool) {
        let win = &mut self.alerts[index];
        win.dpi = dpi;
        if let Some(s) = &win.surface {
            if (s.dpi - dpi).abs() > 0.1 {
                win.surface = None;
                win.map.lost();
            }
        }
        set_round_corners(win.hwnd, round);
        unsafe {
            let _ = SetWindowPos(win.hwnd, Some(HWND_TOPMOST), x, y, w_px, h_px, SWP_SHOWWINDOW | SWP_NOACTIVATE);
        }
        win.visible = true;
        win.dirty = true;
        win.w = w_px as f32 * 96.0 / dpi;
        win.h = h_px as f32 * 96.0 / dpi;
    }

    fn show_full(&mut self, home_level: bool) {
        self.mode = Mode::Full;
        self.panel_monitor = None;
        let list = monitors();
        let count = if self.settings.all_monitors { list.len() } else { 1.min(list.len()) };
        while self.alerts.len() < count {
            if let Ok(h) = popup(w!("אזעקה")) {
                self.alerts.push(Win::new(h));
            } else {
                break;
            }
        }
        for (i, m) in list.iter().take(count.min(self.alerts.len())).enumerate() {
            self.place_alert(i, m.rect.left, m.rect.top, m.rect.right - m.rect.left, m.rect.bottom - m.rect.top, m.dpi, false);
        }
        for i in count..self.alerts.len() {
            let win = &mut self.alerts[i];
            if win.visible {
                unsafe {
                    let _ = ShowWindow(win.hwnd, SW_HIDE);
                }
                win.visible = false;
            }
        }
        if home_level {
            // the screen on, and in front even of a game that took the display for itself
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
                self.display_held = true;
                let nudge = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 } } };
                SendInput(&[nudge], size_of::<INPUT>() as i32);
                if let Ok(state) = SHQueryUserNotificationState() {
                    if state == QUNS_RUNNING_D3D_FULL_SCREEN {
                        paths::log("home alert over a full screen game: taking the foreground");
                        if let Some(first) = self.alerts.first() {
                            let _ = SetForegroundWindow(first.hwnd);
                        }
                    }
                }
            }
        }
    }

    fn show_panel(&mut self) {
        self.mode = Mode::Panel;
        let list = monitors();
        let m = active_monitor(&list);
        self.panel_monitor = Some(m);
        if self.alerts.is_empty() {
            return;
        }
        let scale = m.dpi / 96.0;
        let (work_w, work_h) = (m.work.right - m.work.left, m.work.bottom - m.work.top);
        let width = ((640.0 * scale) as i32).min((work_w as f32 * 0.45) as i32).max((420.0 * scale) as i32);
        let margin = (12.0 * scale) as i32;
        let x = m.work.right - width - margin;
        let y = m.work.top + margin;
        let h = work_h - margin * 2;
        self.place_alert(0, x, y, width, h, m.dpi, true);
        for win in self.alerts.iter_mut().skip(1) {
            if win.visible {
                unsafe {
                    let _ = ShowWindow(win.hwnd, SW_HIDE);
                }
                win.visible = false;
            }
        }
    }

    fn hide_strip(&mut self) {
        if self.strip.visible {
            unsafe {
                let _ = ShowWindow(self.strip.hwnd, SW_HIDE);
            }
            self.strip.visible = false;
        }
    }

    fn show_strip(&mut self) {
        if self.strip.visible {
            return;
        }
        let list = monitors();
        let m = list.first().copied().unwrap_or_else(|| active_monitor(&list));
        let scale = m.dpi / 96.0;
        let (w_px, h_px) = ((720.0 * scale) as i32, (52.0 * scale) as i32);
        let x = m.work.left + ((m.work.right - m.work.left) - w_px) / 2;
        self.strip.dpi = m.dpi;
        set_round_corners(self.strip.hwnd, true);
        unsafe {
            let _ = SetWindowPos(self.strip.hwnd, Some(HWND_TOPMOST), x, m.work.top + (10.0 * scale) as i32, w_px, h_px, SWP_SHOWWINDOW | SWP_NOACTIVATE);
        }
        self.strip.visible = true;
        self.strip.dirty = true;
        self.strip.w = w_px as f32 * 96.0 / m.dpi;
        self.strip.h = h_px as f32 * 96.0 / m.dpi;
    }

    fn show_note(&mut self) {
        let list = monitors();
        let m = list.first().copied().unwrap_or_else(|| active_monitor(&list));
        let scale = m.dpi / 96.0;
        let (w_px, h_px) = ((260.0 * scale) as i32, (46.0 * scale) as i32);
        let margin = (14.0 * scale) as i32;
        self.note.dpi = m.dpi;
        set_round_corners(self.note.hwnd, true);
        unsafe {
            let _ = SetWindowPos(self.note.hwnd, Some(HWND_TOPMOST), m.work.right - w_px - margin, m.work.bottom - h_px - margin, w_px, h_px, SWP_SHOWWINDOW | SWP_NOACTIVATE);
        }
        self.note.visible = true;
        self.note.dirty = true;
        self.note.w = w_px as f32 * 96.0 / m.dpi;
        self.note.h = h_px as f32 * 96.0 / m.dpi;
        self.note_until = Some(Instant::now() + Duration::from_secs(5));
    }

    /// New state from the engine: does it ask for the screen?
    fn take_snapshot(&mut self) {
        let snap = {
            let e = self.hub.engine.lock().unwrap();
            if e.rev() == self.snap.rev {
                None
            } else {
                Some(e.snapshot())
            }
        };
        let Some(snap) = snap else { return };
        let seq = snap.present_seq;
        let level = snap.present_level;
        let read_at = snap.present_read_at;
        self.snap = snap;
        self.main.dirty = true;
        for w in &mut self.alerts {
            w.dirty = true;
        }
        if seq != self.seq_seen {
            self.seq_seen = seq;
            self.measure_first_frame = read_at;
            let t = Instant::now();
            self.present(level);
            paths::log(&format!("presented {level:?} in {:.1} ms, {:.1} ms after the answer was read", t.elapsed().as_secs_f32() * 1000.0, read_at.map(|r| r.elapsed().as_secs_f32() * 1000.0).unwrap_or(0.0)));
        }
    }

    fn present(&mut self, level: Option<Level>) {
        let home_level = level == Some(Level::Home);
        let far_full = ui::wants_full(level, self.settings.far_mode);
        // the main window already in front shows it all, unless it is home
        let main_in_front = self.main.visible && unsafe { GetForegroundWindow() } == self.main.hwnd && !unsafe { IsIconic(self.main.hwnd) }.as_bool();
        // (news from elsewhere never takes the full screen away during a siren at home)
        let home_on = self.snap.home_zone().map(|z| z.kind != crate::model::Kind::End).unwrap_or(false);
        if far_full || (self.mode == Mode::Full && (home_on || level == Some(Level::HomeEnd))) {
            self.show_full(home_level);
        } else if main_in_front {
            self.main.dirty = true;
        } else {
            self.show_panel();
        }
    }

    /// When nothing calls for the alert window any more, it goes
    fn auto_hide(&mut self) {
        if self.mode == Mode::Hidden {
            return;
        }
        let now = now_ms();
        let snap = &self.snap;
        let home = snap.home_zone().is_some();
        let urgent = snap.zones.iter().any(|z| z.urgent(now));
        let recent = now - snap.last_new_ms < 45_000;
        let keep = match self.mode {
            Mode::Full => home || (snap.active() > 0 && (urgent || recent)),
            _ => home || ((urgent || recent) && !snap.zones.is_empty()),
        };
        if !keep {
            self.mode = Mode::Hidden;
            self.hide_alerts();
            sound::stop_siren();
        }
    }

    /// Draws one throwaway alert to a hidden window at start: fonts loaded, shaders compiled, every window's
    /// render target made, so the first real alert pays for none of it. The pictures themselves are let go.
    fn warm_up(&mut self) {
        let started = Instant::now();
        let list = monitors();
        while self.alerts.len() < list.len() {
            match popup(w!("אזעקה")) {
                Ok(h) => self.alerts.push(Win::new(h)),
                Err(_) => break,
            }
        }
        for (i, m) in list.iter().enumerate().take(self.alerts.len()) {
            self.alerts[i].dpi = m.dpi;
            let App { gfx, alerts, .. } = self;
            let _ = App::ensure_surface(gfx, &mut alerts[i]);
        }
        let now = now_ms();
        let near: Vec<usize> = match self.homes.first() {
            Some(&h) => (0..self.places.list.len()).filter(|&i| self.places.distance_km(h, i) < 12.0).collect(),
            None => (0..self.places.list.len().min(20)).collect(),
        };
        let mut engine = crate::engine::Engine::new(self.places.clone(), crate::engine::Policy { homes: self.homes.clone(), mute_all: true, ..Default::default() });
        let event = crate::engine::Event {
            source: crate::engine::Source::Drill,
            key: "warm".to_string(),
            kind: crate::model::Kind::Siren,
            threat: crate::model::Threat::Missiles,
            drill: true,
            time_s: Some(now / 1000),
            title: String::new(),
            body: String::new(),
            places: near,
            unknown: vec![],
            read_at: Instant::now(),
        };
        engine.apply(&event, now);
        let real = std::mem::replace(&mut self.snap, engine.snapshot());
        let (w0, h0) = (self.alerts[0].w, self.alerts[0].h);
        for mode in [Mode::Panel, Mode::Full] {
            self.mode = mode;
            self.alerts[0].w = 1280.0;
            self.alerts[0].h = 720.0;
            self.render_alert(0);
        }
        self.mode = Mode::Hidden;
        self.snap = real;
        let win = &mut self.alerts[0];
        win.w = w0;
        win.h = h0;
        win.map = MapView::new();
        win.dirty = true;
        win.last_draw = None;
        self.frame_ms_max = 0.0;
        self.frame_ms_sum = 0.0;
        self.frame_count = 0;
        paths::log(&format!("warmed up in {:.0} ms", started.elapsed().as_secs_f32() * 1000.0));
    }

    fn slow_work(&mut self) {
        // once in 5 seconds
        self.output_note = output_warning();
        let name = output_device();
        if name != self.output_name {
            paths::log(&format!("sound goes to: {name:?}"));
            self.output_name = name;
        }
        self.autostart_on = autostart_get();
        self.write_status();
    }

    fn write_status(&mut self) {
        let h = &self.health;
        let src = |s: &sources::SourceHealth| {
            serde_json::json!({ "ok_age_s": s.age_s(), "bad_for_s": s.bad_since.map(|b| b.elapsed().as_secs_f32()), "err": s.err,
                "ms": s.last_ms, "cache_age_s": s.cache_age, "n_ok": s.n_ok, "n_bad": s.n_bad })
        };
        let frames = if self.frame_count > 0 { self.frame_ms_sum / self.frame_count as f32 } else { 0.0 };
        let v = serde_json::json!({
            "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION"), "started_ms": self.started_ms, "now_ms": now_ms(), "clock": paths::clock(now_ms()),
            "protection": format!("{:?}", h.protection()), "trouble": h.trouble(),
            "sirens": src(&h.sirens), "messages": src(&h.messages), "feed": src(&h.feed), "official": src(&h.official), "official_history": src(&h.official_history),
            "official_status": h.official_status, "official_live": h.official_live, "clock_skew_s": h.clock_skew_s,
            "zones": self.snap.zones.len(), "active": self.snap.active(), "mode": format!("{:?}", self.mode),
            "homes": self.settings.homes, "autostart": self.autostart_on, "history": self.history.len(), "places": self.places.list.len(),
            "lists_version": self.places.cities_version, "main_visible": self.main.visible, "output_note": self.output_note, "output_device": self.output_name,
            "frame_ms_avg": frames, "frame_ms_max": self.frame_ms_max, "frames": self.frame_count, "font": self.gfx.ui_family.to_string(),
        });
        self.frame_ms_max = 0.0;
        self.frame_ms_sum = 0.0;
        self.frame_count = 0;
        let _ = paths::write_atomic(&paths::data_dir().join("status.json"), v.to_string().as_bytes());
    }

    fn tick(&mut self) {
        let stop = self.hub.engine.lock().unwrap().tick(now_ms());
        if stop {
            sound::stop_siren();
        }
        self.health = self.hub.health.lock().unwrap().clone();
        {
            let h = self.hub.history.lock().unwrap();
            if h.len() != self.history.len() || h.first().map(|x| &x.id) != self.history.first().map(|x| &x.id) {
                self.history = h.clone();
                self.picked = None;
            }
        }
        self.auto_hide();
        // the sources' state on the tray and, past a limit, on the screen
        let prot = self.health.protection();
        let alert_on = self.snap.count(crate::model::Kind::Siren) > 0;
        let (state, tip) = if alert_on {
            (3, format!("אזעקות: {} יישובים באזעקה", self.snap.count(crate::model::Kind::Siren)))
        } else {
            match prot {
                Protection::Ok => (0, "אזעקות: מאזין, הכל תקין".to_string()),
                Protection::Weak => (1, format!("אזעקות: {}", self.health.trouble().unwrap_or_default())),
                Protection::Down => (2, format!("אזעקות: {}", self.health.trouble().unwrap_or_default())),
            }
        };
        self.tray.update(state, &tip);
        if prot == Protection::Ok {
            if self.outage_since.is_some() {
                paths::log("sources: back to normal");
            }
            self.outage_since = None;
            self.strip_hidden_for_outage = false;
            self.down_toned = false;
            self.hide_strip();
        } else {
            let since = *self.outage_since.get_or_insert_with(Instant::now);
            let held = since.elapsed().as_secs();
            let show = (prot == Protection::Down && held >= 30) || held >= 90;
            if show && !self.strip_hidden_for_outage {
                if !self.strip.visible {
                    paths::log(&format!("warning strip up: {}", self.health.trouble().unwrap_or_default()));
                }
                self.show_strip();
                self.strip.dirty = true;
            }
            if prot == Protection::Down && held >= 30 && !self.down_toned {
                self.down_toned = true;
                let night = {
                    let hour = paths::local_hour(now_ms());
                    let (a, b) = (self.settings.quiet_from, self.settings.quiet_to);
                    a != b && (if a < b { hour >= a && hour < b } else { hour >= a || hour < b })
                };
                if !night && !self.options.mute {
                    sound::play_down();
                }
            }
        }
        if let Some(until) = self.note_until {
            self.note.dirty = true;
            if Instant::now() >= until {
                self.note_until = None;
                unsafe {
                    let _ = ShowWindow(self.note.hwnd, SW_HIDE);
                }
                self.note.visible = false;
            }
        }
        if self.main.visible {
            self.main.dirty = true;
        }
        // keep on top of whatever else asked for "topmost" since
        if self.mode != Mode::Hidden && self.last_topmost.elapsed() > Duration::from_secs(3) {
            self.last_topmost = Instant::now();
            for w in self.alerts.iter().filter(|w| w.visible) {
                unsafe {
                    let _ = SetWindowPos(w.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
        }
    }

    /// Frames per second each visible thing needs right now; 0: only when something changes
    fn wanted_period(&self) -> Option<Duration> {
        let now = now_ms();
        let urgent = self.snap.zones.iter().any(|z| z.urgent(now));
        let mut best: Option<u64> = None;
        let mut want = |ms: u64| best = Some(best.map_or(ms, |b: u64| b.min(ms)));
        if self.mode != Mode::Hidden {
            let moving = self.alerts.iter().any(|w| w.visible && w.map.moving);
            want(if moving || urgent { 16 } else { 40 });
        }
        if self.main.visible {
            let moving = self.main.map.moving;
            want(if moving { 16 } else if urgent { 33 } else { 250 });
        }
        best.map(Duration::from_millis)
    }

    fn render_all(&mut self) {
        let due = self.wanted_period();
        let now = Instant::now();
        let is_due = |w: &Win| match (w.last_draw, due) {
            (None, _) => true,
            (Some(t), Some(p)) => now.duration_since(t) + Duration::from_millis(2) >= p,
            (Some(_), None) => false,
        };
        if self.mode != Mode::Hidden {
            for i in 0..self.alerts.len() {
                if self.alerts[i].visible && (self.alerts[i].dirty || is_due(&self.alerts[i])) {
                    self.render_alert(i);
                }
            }
            if let Some(read_at) = self.measure_first_frame.take() {
                // the moment the alert was read to the moment its picture reached the screen
                unsafe {
                    let _ = DwmFlush();
                }
                let line = format!("first frame on screen {:.1} ms after the answer was read", read_at.elapsed().as_secs_f32() * 1000.0);
                paths::log(&line);
                paths::record_alert(&serde_json::json!({ "t": now_ms(), "first_frame_ms": read_at.elapsed().as_secs_f32() * 1000.0 }).to_string());
            }
        }
        if self.main.visible && (self.main.dirty || is_due(&self.main)) {
            self.render_main();
        }
        if self.strip.visible && self.strip.dirty {
            self.render_strip();
        }
        if self.note.visible && self.note.dirty {
            self.render_note();
        }
    }

    fn ensure_surface(gfx: &Gfx, win: &mut Win) -> bool {
        if win.surface.is_none() {
            match make_surface(gfx, win.hwnd, win.dpi) {
                Ok(s) => {
                    win.map.lost();
                    win.surface = Some(s);
                }
                Err(e) => {
                    paths::log(&format!("no render target: {e}"));
                    return false;
                }
            }
        }
        true
    }

    fn render_alert(&mut self, i: usize) {
        let started = Instant::now();
        let App { hub: _, gfx, geo, places, settings, snap, health, history, homes, alerts, output_note, output_name, autostart_on, t0, mode, options, .. } = self;
        let win = &mut alerts[i];
        if !Self::ensure_surface(gfx, win) {
            return;
        }
        let s = win.surface.as_ref().unwrap();
        let view = View {
            places,
            snap,
            health,
            settings,
            history,
            homes,
            now_ms: now_ms(),
            t: t0.elapsed().as_secs_f32(),
            siren_on: sound::siren_on(),
            output_note: output_note.as_deref(),
            output_name: output_name.as_deref(),
            autostart_on: *autostart_on,
            geo,
            test_run: options.test_run,
        };
        let mut actions = vec![];
        let result = unsafe {
            s.rt.BeginDraw();
            let p = Painter { rt: &s.rt, gfx, brush: &s.brush };
            win.input.hand = false;
            let (w, h) = (win.w, win.h);
            match mode {
                Mode::Full => ui::draw_full(&p, &view, &mut win.map, &mut win.input, w, h, &mut win.scroll, &mut actions),
                _ => ui::draw_panel(&p, &view, &mut win.map, &mut win.input, w, h, &mut win.scroll, &mut actions),
            }
            s.rt.EndDraw(None, None)
        };
        win.input.clicked = false;
        win.input.wheel = 0.0;
        win.dirty = false;
        win.last_draw = Some(Instant::now());
        if let Err(e) = result {
            // the graphics device went away (a driver reset, a sleep): everything on it is made anew
            paths::log(&format!("draw lost its target: {e}"));
            win.surface = None;
            win.map.lost();
            win.dirty = true;
        }
        self.count_frame(started);
        for a in actions {
            self.run_action(a);
        }
    }

    fn render_main(&mut self) {
        let started = Instant::now();
        let App { gfx, geo, places, settings, snap, health, history, homes, main: win, output_note, output_name, autostart_on, t0, tab, search, picked, hover_place, options, .. } = self;
        if !Self::ensure_surface(gfx, win) {
            return;
        }
        let s = win.surface.as_ref().unwrap();
        let view = View {
            places,
            snap,
            health,
            settings,
            history,
            homes,
            now_ms: now_ms(),
            t: t0.elapsed().as_secs_f32(),
            siren_on: sound::siren_on(),
            output_note: output_note.as_deref(),
            output_name: output_name.as_deref(),
            autostart_on: *autostart_on,
            geo,
            test_run: options.test_run,
        };
        let mut actions = vec![];
        let edit_rect;
        let result = unsafe {
            s.rt.BeginDraw();
            let p = Painter { rt: &s.rt, gfx, brush: &s.brush };
            win.input.hand = false;
            let (w, h) = (win.w, win.h);
            let frame = ui::draw_main(&p, &view, &mut win.map, &mut win.input, w, h, *tab, search, &mut win.scroll, *picked, *hover_place, &mut actions);
            edit_rect = frame.edit_rect;
            s.rt.EndDraw(None, None)
        };
        win.input.clicked = false;
        win.input.wheel = 0.0;
        win.dirty = false;
        win.last_draw = Some(Instant::now());
        if let Err(e) = result {
            paths::log(&format!("draw lost its target: {e}"));
            win.surface = None;
            win.map.lost();
            win.dirty = true;
        }
        self.place_edit(edit_rect);
        self.count_frame(started);
        for a in actions {
            self.run_action(a);
        }
    }

    fn place_edit(&mut self, rect: Option<R>) {
        if rect == self.edit_rect {
            return;
        }
        self.edit_rect = rect;
        unsafe {
            match rect {
                Some(a) => {
                    let k = self.main.dpi / 96.0;
                    let _ = SetWindowPos(self.edit, None, (a.x * k) as i32, (a.y * k) as i32, (a.w * k) as i32, (a.h * k) as i32, SWP_NOZORDER | SWP_SHOWWINDOW);
                }
                None => {
                    let _ = ShowWindow(self.edit, SW_HIDE);
                }
            }
        }
    }

    fn render_strip(&mut self) {
        let App { gfx, geo, places, settings, snap, health, history, homes, strip: win, output_note, output_name, autostart_on, t0, options, .. } = self;
        if !Self::ensure_surface(gfx, win) {
            return;
        }
        let s = win.surface.as_ref().unwrap();
        let view = View { places, snap, health, settings, history, homes, now_ms: now_ms(), t: t0.elapsed().as_secs_f32(), siren_on: false, output_note: output_note.as_deref(),
            output_name: output_name.as_deref(), autostart_on: *autostart_on, geo, test_run: options.test_run };
        let mut actions = vec![];
        let result = unsafe {
            s.rt.BeginDraw();
            let p = Painter { rt: &s.rt, gfx, brush: &s.brush };
            win.input.hand = false;
            ui::draw_strip(&p, &view, &mut win.input, win.w, win.h, &mut actions);
            s.rt.EndDraw(None, None)
        };
        win.input.clicked = false;
        win.dirty = false;
        if result.is_err() {
            win.surface = None;
            win.dirty = true;
        }
        for a in actions {
            self.run_action(a);
        }
    }

    fn render_note(&mut self) {
        let App { gfx, geo, places, settings, snap, health, history, homes, note: win, output_note, output_name, autostart_on, t0, options, .. } = self;
        if !Self::ensure_surface(gfx, win) {
            return;
        }
        let s = win.surface.as_ref().unwrap();
        let view = View { places, snap, health, settings, history, homes, now_ms: now_ms(), t: t0.elapsed().as_secs_f32(), siren_on: false, output_note: output_note.as_deref(),
            output_name: output_name.as_deref(), autostart_on: *autostart_on, geo, test_run: options.test_run };
        let result = unsafe {
            s.rt.BeginDraw();
            let p = Painter { rt: &s.rt, gfx, brush: &s.brush };
            ui::draw_note(&p, &view, win.w, win.h);
            s.rt.EndDraw(None, None)
        };
        win.dirty = false;
        if result.is_err() {
            win.surface = None;
            win.dirty = true;
        }
    }

    fn count_frame(&mut self, started: Instant) {
        let ms = started.elapsed().as_secs_f32() * 1000.0;
        self.frame_ms_max = self.frame_ms_max.max(ms);
        self.frame_ms_sum += ms;
        self.frame_count += 1;
    }

    fn pump(&mut self) {
        self.take_snapshot();
        if self.last_tick.elapsed() >= Duration::from_millis(1000) {
            self.last_tick = Instant::now();
            self.tick();
        }
        // (a test run tells its state every second)
        if self.last_slow.elapsed() >= Duration::from_secs(if self.options.test_run { 1 } else { 5 }) {
            self.last_slow = Instant::now();
            self.slow_work();
        }
        self.render_all();
    }

    fn next_wait_ms(&self) -> u32 {
        let to_tick = 1000u64.saturating_sub(self.last_tick.elapsed().as_millis() as u64);
        let frame = self.wanted_period().map(|p| p.as_millis() as u64).unwrap_or(u64::MAX);
        to_tick.min(frame).max(1) as u32
    }
}

fn make_edit(parent: HWND) -> Result<(HWND, HFONT, HBRUSH)> {
    unsafe {
        let inst: HINSTANCE = GetModuleHandleW(None)?.into();
        let edit = CreateWindowExW(
            WS_EX_RTLREADING | WS_EX_RIGHT,
            w!("EDIT"),
            None,
            WS_CHILD | WINDOW_STYLE(ES_AUTOHSCROLL as u32 | ES_RIGHT as u32),
            0,
            0,
            100,
            20,
            Some(parent),
            Some(HMENU(ID_EDIT as *mut _)),
            Some(inst),
            None,
        )?;
        let font = CreateFontW(
            -15,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            w!("Segoe UI"),
        );
        SendMessageW(edit, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        let brush = CreateSolidBrush(COLORREF(0x002D2319));
        Ok((edit, font, brush))
    }
}

/// The program, until the user quits. Returns the exit code.
pub fn run(hub: Arc<Hub>, settings: Settings, options: Options) -> Result<i32> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let places = hub.places.clone();
    let gfx = Gfx::new()?;
    let geo = Geo::new(&gfx, &places)?;
    register_classes()?;
    let inst: HINSTANCE = unsafe { GetModuleHandleW(None)?.into() };
    let main_hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            MAIN_CLASS,
            w!("אזעקות"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1360,
            880,
            None,
            None,
            Some(inst),
            None,
        )?
    };
    set_dark_frame(main_hwnd);
    let main_dpi = unsafe { GetDpiForWindow(main_hwnd) }.max(96) as f32;
    let (edit, edit_font, edit_brush) = make_edit(main_hwnd)?;
    let primary = popup(w!("אזעקה"))?;
    let strip = popup(w!("אזעקות: אזהרה"))?;
    let note = popup(w!("אזעקות"))?;
    hub.hwnd.store(main_hwnd.0 as isize, Ordering::Relaxed);
    let icons = [make_icon(rgb(0x1FAF5F), false), make_icon(rgb(0xE59A00), false), make_icon(rgb(0x6B7785), true), make_icon(rgb(0xE81F1F), true)];
    let timer = unsafe { CreateWaitableTimerExW(None, PCWSTR::null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0).or_else(|_| CreateWaitableTimerExW(None, PCWSTR::null(), 0, TIMER_ALL_ACCESS.0))? };
    let homes = settings.homes.iter().filter_map(|n| places.find(n)).collect();
    let mut main_win = Win::new(main_hwnd);
    main_win.dpi = main_dpi;
    let mut app = Box::new(App {
        hub: hub.clone(),
        gfx,
        geo,
        places,
        settings,
        options: options.clone(),
        homes,
        snap: Snapshot::default(),
        health: Health::default(),
        history: vec![],
        main: main_win,
        alerts: vec![Win::new(primary)],
        strip: Win::new(strip),
        note: Win::new(note),
        tray: Tray { hwnd: main_hwnd, icons, shown: -1, tip: "אזעקות".to_string(), added: false },
        mode: Mode::Hidden,
        panel_monitor: None,
        edit,
        edit_font,
        edit_brush,
        edit_rect: None,
        search: String::new(),
        tab: 0,
        picked: None,
        hover_place: None,
        drag: None,
        seq_seen: 0,
        t0: Instant::now(),
        last_tick: Instant::now() - Duration::from_secs(2),
        last_slow: Instant::now() - Duration::from_secs(4),
        output_note: None,
        output_name: None,
        autostart_on: false,
        timer,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        strip_hidden_for_outage: false,
        outage_since: None,
        down_toned: false,
        note_until: None,
        measure_first_frame: None,
        frame_ms_max: 0.0,
        frame_ms_sum: 0.0,
        frame_count: 0,
        started_ms: now_ms(),
        quitting: false,
        display_held: false,
        last_topmost: Instant::now(),
    });
    unsafe {
        APP = &mut *app;
    }
    app.tray.add();
    // (a test run must not take the place of the real program in the registry)
    if app.settings.autostart && !options.test_run {
        autostart_set(true);
    }
    app.autostart_on = autostart_get();
    app.warm_up();
    sources::start(&hub);
    if let Some(kind) = options.drill {
        drill::start(hub.clone(), kind);
    }
    if options.background {
        app.show_note();
    } else {
        app.show_main();
    }
    let exit_at = options.exit_after.map(|s| Instant::now() + Duration::from_secs(s));
    paths::log(&format!("started: pid {}, {} places, homes {:?}, font {}", std::process::id(), app.places.list.len(), app.settings.homes, app.gfx.ui_family));
    let mut msg = MSG::default();
    let code = 'outer: loop {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'outer 0;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if let Some(at) = exit_at {
            if Instant::now() >= at {
                app.tray.remove();
                app.write_status();
                break 'outer 0;
            }
        }
        app.pump();
        let wait_ms = app.next_wait_ms();
        let due: i64 = -(wait_ms as i64) * 10_000;
        unsafe {
            let _ = SetWaitableTimer(app.timer, &due, 0, None, None, false);
            MsgWaitForMultipleObjectsEx(Some(&[app.timer]), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    };
    unsafe {
        APP = std::ptr::null_mut();
    }
    hub.stop.store(true, Ordering::Relaxed);
    sound::stop_siren();
    Ok(code)
}

// ---------------------------------------------------------------- pictures with no window, to check how it looks

pub fn raise_process() {
    unsafe {
        let me = GetCurrentProcess();
        let _ = SetPriorityClass(me, ABOVE_NORMAL_PRIORITY_CLASS);
        // no "efficiency mode" and no stretched timers while in the background: this program's whole point is being on time
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
            StateMask: 0,
        };
        let _ = SetProcessInformation(me, ProcessPowerThrottling, &state as *const _ as *const _, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32);
    }
}

pub fn hstring_debug(h: &HSTRING) -> String {
    h.to_string()
}
