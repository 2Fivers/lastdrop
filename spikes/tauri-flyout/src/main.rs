//! Lastdrop tray-flyout spike: Tauri 2 + WebView2, vanilla HTML/CSS/JS frontend.
//!
//! Run normally for an interactive tray app, or `--bench <out.json> [--mode keepalive|destroy]
//! [--lowmem] [--theme light|dark]` for the scripted 50 s timeline used by the measurer.
#![windows_subsystem = "windows"]

use std::{
    ffi::c_void,
    mem::size_of,
    path::PathBuf,
    sync::{Mutex, MutexGuard, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::ScrollBarStyle,
    AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Settings3, ICoreWebView2Settings4, ICoreWebView2Settings5,
        ICoreWebView2Settings6, ICoreWebView2_19, ICoreWebView2_3,
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    },
    TrySuspendCompletedHandler,
};
use windows::{
    core::{Interface, BOOL, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Dwm::{
                DwmGetColorizationColor, DwmGetWindowAttribute, DwmSetWindowAttribute,
                DWMSBT_MAINWINDOW, DWMSBT_TRANSIENTWINDOW, DWMWA_EXTENDED_FRAME_BOUNDS,
                DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
                DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
            },
            Gdi::{ClientToScreen, GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST},
        },
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
            Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD},
            Threading::{GetCurrentProcess, GetProcessTimes},
        },
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Shell::{DefSubclassProc, SetWindowSubclass},
            WindowsAndMessaging::{
                CalculatePopupWindowPosition, GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowRect,
                GetForegroundWindow, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE,
                HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                SWP_NOZORDER, SW_HIDE, SW_SHOW, SW_SHOWNOACTIVATE, TPM_BOTTOMALIGN,
                TPM_CENTERALIGN, TPM_VERTICAL, TPM_WORKAREA, WA_INACTIVE, WM_ACTIVATE,
                WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
            },
        },
    },
};

const LABEL: &str = "flyout";
const TRAY_ID: &str = "lastdrop";
const W_LOGICAL: f64 = 360.0;
const H_LOGICAL: f64 = 440.0;
const GAP_LOGICAL: f64 = 12.0;
const OPT_LEVEL: &str = "s";

// ---------------------------------------------------------------------------------------------
// config + shared state
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    KeepAlive,
    Destroy,
}

struct Config {
    mode: Mode,
    bench_out: Option<PathBuf>,
    lowmem: bool,
    theme: Option<tauri::Theme>,
    /// time compression for quick smoke tests only (measurer uses the default 1.0)
    bench_scale: f64,
}

impl Config {
    fn variant(&self) -> String {
        let mut v = String::from(match self.mode {
            Mode::KeepAlive => "tauri-keepalive",
            Mode::Destroy => "tauri-destroy",
        });
        if self.lowmem {
            v.push_str("-lowmem");
        }
        v
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Phase {
    name: String,
    epoch_ms: f64,
}

#[derive(Serialize, Default, Clone)]
struct Latency {
    cold: Option<f64>,
    warm: Option<f64>,
}

#[derive(Serialize, Clone, Copy, Default)]
struct RectPx {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ShowDetail {
    label: String,
    seq: u32,
    total_ms: f64,
    window_build_ms: Option<f64>,
    page_ready_ms: Option<f64>,
    js_total_ms: Option<f64>,
    js_visible_to_frame_ms: Option<f64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProcSample {
    at: String,
    t_ms: f64,
    count: usize,
    pids: Vec<u32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Report {
    variant: String,
    pid: u32,
    t0_epoch_ms: f64,
    process_start_epoch_ms: f64,
    t0_rebased: bool,
    phases: Vec<Phase>,
    show_latency_ms: Latency,
    flyout_rect_px: Option<RectPx>,
    backdrop: String,
    renderer: String,
    notes: String,
    // extras (not required by the measurer)
    complete: bool,
    show_detail: Vec<ShowDetail>,
    webview_processes: Vec<ProcSample>,
    window_rect_px: Option<RectPx>,
    client_rect_px: Option<RectPx>,
    viewport: String,
    positioning: String,
    dpi: u32,
    window_ex_style: String,
    focus_stolen: Option<bool>,
    theme: String,
    accent: String,
    accent_source: String,
    webview2_version: String,
    user_data_folder: String,
    opt_level: String,
}

struct Pending {
    seq: u32,
    label: String,
    start: Instant,
    activate: bool,
    build_ms: Option<f64>,
    page_ready_ms: Option<f64>,
}

struct State {
    visible: bool,
    destroying: bool,
    page_ready: bool,
    seq: u32,
    pending: Option<Pending>,
    last_deactivate_hide: Option<Instant>,
    palette: Vec<(&'static str, [u8; 3])>,
    dark: bool,
    report: Report,
}

static CFG: OnceLock<Config> = OnceLock::new();
static T0: OnceLock<Instant> = OnceLock::new();
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
static APP: OnceLock<AppHandle> = OnceLock::new();

fn cfg() -> &'static Config {
    CFG.get().expect("config")
}

/// Never hold this guard across Win32/Tauri calls: those can re-enter (WM_ACTIVATE, IPC).
fn st() -> MutexGuard<'static, State> {
    STATE.get().expect("state").lock().unwrap_or_else(|e| e.into_inner())
}

fn epoch_ms_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

fn phase(name: &str) {
    if cfg().bench_out.is_none() {
        return; // interactive mode: keep no history
    }
    st().report.phases.push(Phase { name: name.to_string(), epoch_ms: epoch_ms_now() });
}

fn note(s: &str) {
    let mut g = st();
    if !g.report.notes.is_empty() {
        g.report.notes.push(' ');
    }
    g.report.notes.push_str(s);
}

fn process_start_epoch_ms() -> Option<f64> {
    unsafe {
        let (mut c, mut e, mut k, mut u) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).ok()?;
        let ticks = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64; // 100 ns since 1601
        Some(ticks as f64 / 10_000.0 - 11_644_473_600_000.0)
    }
}

fn parse_args() -> Config {
    let mut c = Config { mode: Mode::KeepAlive, bench_out: None, lowmem: false, theme: None, bench_scale: 1.0 };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let (key, inline) = match a.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (a.clone(), None),
        };
        let mut val = || inline.clone().or_else(|| it.next());
        match key.as_str() {
            "--bench" => c.bench_out = val().map(PathBuf::from),
            "--mode" => {
                c.mode = match val().as_deref() {
                    Some("destroy") => Mode::Destroy,
                    _ => Mode::KeepAlive,
                }
            }
            "--lowmem" => c.lowmem = true,
            "--bench-scale" => {
                c.bench_scale = val().and_then(|v| v.parse().ok()).filter(|v: &f64| *v > 0.0).unwrap_or(1.0)
            }
            "--theme" => {
                c.theme = match val().as_deref() {
                    Some("dark") => Some(tauri::Theme::Dark),
                    Some("light") => Some(tauri::Theme::Light),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    c
}

// ---------------------------------------------------------------------------------------------
// system theme + accent
// ---------------------------------------------------------------------------------------------

fn reg_dword(name: &str) -> Option<u32> {
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let val: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let mut data: u32 = 0;
    let mut len = size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(val.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut c_void),
            Some(&mut len),
        )
    };
    r.is_ok().then_some(data)
}

fn apps_dark() -> bool {
    match cfg().theme {
        Some(tauri::Theme::Dark) => true,
        Some(_) => false,
        None => reg_dword("AppsUseLightTheme").map(|v| v == 0).unwrap_or(false),
    }
}

fn taskbar_dark() -> bool {
    reg_dword("SystemUsesLightTheme").map(|v| v == 0).unwrap_or(true)
}

/// WinUI accent ramp from UISettings (WinRT); falls back to the DWM colorization colour.
fn read_palette() -> (Vec<(&'static str, [u8; 3])>, &'static str) {
    use windows::UI::ViewManagement::{UIColorType, UISettings};
    let ui = || -> windows::core::Result<Vec<(&'static str, [u8; 3])>> {
        let s = UISettings::new()?;
        let get = |t: UIColorType| s.GetColorValue(t).map(|c| [c.R, c.G, c.B]);
        Ok(vec![
            ("accent", get(UIColorType::Accent)?),
            ("accent-dark1", get(UIColorType::AccentDark1)?),
            ("accent-dark2", get(UIColorType::AccentDark2)?),
            ("accent-dark3", get(UIColorType::AccentDark3)?),
            ("accent-light1", get(UIColorType::AccentLight1)?),
            ("accent-light2", get(UIColorType::AccentLight2)?),
            ("accent-light3", get(UIColorType::AccentLight3)?),
        ])
    };
    match ui() {
        Ok(v) => (v, "UISettings.GetColorValue"),
        Err(_) => {
            let mut argb = 0u32;
            let mut opaque = BOOL(0);
            let c = if unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.is_ok() {
                [(argb >> 16) as u8, (argb >> 8) as u8, argb as u8]
            } else {
                [0x00, 0x78, 0xd4]
            };
            let names = ["accent", "accent-dark1", "accent-dark2", "accent-dark3", "accent-light1", "accent-light2", "accent-light3"];
            (names.iter().map(|n| (*n, c)).collect(), "DwmGetColorizationColor")
        }
    }
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn palette_get(p: &[(&'static str, [u8; 3])], name: &str) -> [u8; 3] {
    p.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).unwrap_or([0, 0x78, 0xd4])
}

fn init_script() -> String {
    let (palette, theme) = {
        let s = st();
        let mut m = serde_json::Map::new();
        for (k, c) in &s.palette {
            m.insert((*k).to_string(), serde_json::Value::String(hex(*c)));
        }
        (serde_json::Value::Object(m), cfg().theme.map(|t| if t == tauri::Theme::Dark { "dark" } else { "light" }))
    };
    format!(
        "window.__LASTDROP_INIT={};",
        serde_json::json!({ "palette": palette, "theme": theme })
    )
}

// ---------------------------------------------------------------------------------------------
// tray icon: 32x32 RGBA ring gauge drawn at runtime (4x4 supersampled)
// ---------------------------------------------------------------------------------------------

fn draw_ring(pct: f32, fg: [u8; 3], track: [u8; 3], track_a: f32) -> Vec<u8> {
    const N: usize = 32;
    const SS: usize = 4;
    let c = N as f32 / 2.0;
    let (r_mid, hw) = (12.0f32, 3.0f32);
    let sweep = pct * std::f32::consts::TAU;
    let (ex, ey) = (r_mid * sweep.sin(), -r_mid * sweep.cos());
    let mut out = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let (mut a, mut t) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let px = x as f32 + (sx as f32 + 0.5) / SS as f32 - c;
                    let py = y as f32 + (sy as f32 + 0.5) / SS as f32 - c;
                    let d = (px * px + py * py).sqrt();
                    let band = (d - r_mid).abs() <= hw;
                    let mut ang = px.atan2(-py);
                    if ang < 0.0 {
                        ang += std::f32::consts::TAU;
                    }
                    let cap0 = (px * px + (py + r_mid) * (py + r_mid)).sqrt() <= hw;
                    let cap1 = ((px - ex).powi(2) + (py - ey).powi(2)).sqrt() <= hw;
                    if (band && ang <= sweep) || cap0 || cap1 {
                        a += 1;
                    } else if band {
                        t += 1;
                    }
                }
            }
            let a = a as f32 / (SS * SS) as f32;
            let ta = t as f32 / (SS * SS) as f32 * track_a;
            let al = a + ta * (1.0 - a);
            let i = (y * N + x) * 4;
            if al > 0.0 {
                for k in 0..3 {
                    out[i + k] =
                        ((fg[k] as f32 * a + track[k] as f32 * ta * (1.0 - a)) / al).round() as u8;
                }
            }
            out[i + 3] = (al * 255.0).round() as u8;
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// window chrome + positioning (Win32)
// ---------------------------------------------------------------------------------------------

unsafe extern "system" fn flyout_subclass(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_ACTIVATE && (wp.0 & 0xffff) as u32 == WA_INACTIVE && cfg().bench_out.is_none() {
        if let Some(app) = APP.get() {
            if let Ok(mut s) = STATE.get().expect("state").try_lock() {
                if s.visible {
                    s.last_deactivate_hide = Some(Instant::now());
                }
            }
            defer_main(app.clone(), |a| hide(&a, "hide_deactivate"));
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

/// Tool window (no taskbar / Alt+Tab), Win11 round corners, system acrylic backdrop.
unsafe fn apply_native_chrome(hwnd: HWND, dark: bool) -> &'static str {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let ex = (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        );
        let dark_v = BOOL(dark as i32);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark_v as *const _ as *const c_void,
            size_of::<BOOL>() as u32,
        );
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const c_void,
            size_of_val(&pref) as u32,
        );
        let acrylic = DWMSBT_TRANSIENTWINDOW;
        let backdrop = if DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &acrylic as *const _ as *const c_void,
            size_of_val(&acrylic) as u32,
        )
        .is_ok()
        {
            "acrylic"
        } else {
            let mica = DWMSBT_MAINWINDOW;
            if DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &mica as *const _ as *const c_void,
                size_of_val(&mica) as u32,
            )
            .is_ok()
            {
                "mica"
            } else {
                "none"
            }
        };
        let _ = SetWindowSubclass(hwnd, Some(flyout_subclass), 0x1a57, 0);
        backdrop
    }
}

fn anchor_rect(app: &AppHandle) -> (RECT, &'static str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(Some(r)) = tray.rect() {
            let p = r.position.to_physical::<i32>(1.0);
            let s = r.size.to_physical::<i32>(1.0);
            if s.width > 0 && s.height > 0 {
                return (
                    RECT { left: p.x, top: p.y, right: p.x + s.width, bottom: p.y + s.height },
                    "Shell_NotifyIconGetRect",
                );
            }
        }
    }
    let mut pt = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut pt);
    }
    (RECT { left: pt.x, top: pt.y, right: pt.x + 1, bottom: pt.y + 1 }, "cursor")
}

/// CalculatePopupWindowPosition above the icon, 12 px (scaled) above the work area edge.
fn popup_rect(anchor: RECT) -> (RECT, u32) {
    unsafe {
        let mon = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f64 / 96.0;
        let w = (W_LOGICAL * scale).round() as i32;
        let h = (H_LOGICAL * scale).round() as i32;
        let gap = (GAP_LOGICAL * scale).round() as i32;
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let work = mi.rcWork;
        let top = anchor.top.min(work.bottom) - gap;
        let excl = RECT { left: anchor.left, top, right: anchor.right, bottom: anchor.bottom.max(top + 1) };
        let pt = POINT { x: (anchor.left + anchor.right) / 2, y: top };
        let size = SIZE { cx: w, cy: h };
        let mut out = RECT::default();
        let flags = TPM_CENTERALIGN | TPM_BOTTOMALIGN | TPM_VERTICAL | TPM_WORKAREA;
        if CalculatePopupWindowPosition(&pt, &size, flags.0, Some(&excl), &mut out).is_err() {
            out = RECT { left: pt.x - w / 2, top: pt.y - h, right: pt.x - w / 2 + w, bottom: pt.y };
        }
        let x = out.left.clamp(work.left + gap, (work.right - gap - w).max(work.left + gap));
        let y = out.top.clamp(work.top + gap, (work.bottom - gap - h).max(work.top + gap));
        (RECT { left: x, top: y, right: x + w, bottom: y + h }, dx)
    }
}

fn rect_px(r: RECT) -> RectPx {
    RectPx { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

/// Client rect in screen coordinates. With tao's "undecorated + shadow" style the window keeps
/// invisible resize borders (7 px left/right/bottom at 96 DPI) outside the client area, so the
/// visible flyout == client rect, not the window rect.
unsafe fn client_screen_rect(hwnd: HWND) -> RECT {
    unsafe {
        let mut cr = RECT::default();
        let _ = GetClientRect(hwnd, &mut cr);
        let mut p = POINT { x: 0, y: 0 };
        let _ = ClientToScreen(hwnd, &mut p);
        RECT { left: p.x, top: p.y, right: p.x + cr.right, bottom: p.y + cr.bottom }
    }
}

/// Move/size so that the *client* (= visible frame) lands exactly on `r`.
unsafe fn place(hwnd: HWND, r: RECT) {
    unsafe {
        for _ in 0..3 {
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let c = client_screen_rect(hwnd);
            let (l, t) = (c.left - wr.left, c.top - wr.top);
            let (rr, b) = (wr.right - c.right, wr.bottom - c.bottom);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                r.left - l,
                r.top - t,
                (r.right - r.left) + l + rr,
                (r.bottom - r.top) + t + b,
                SWP_NOACTIVATE,
            );
            // moving across monitors with a different DPI triggers WM_DPICHANGED (tao resizes and
            // the border insets change): loop until the client rect is exact
            let c = client_screen_rect(hwnd);
            if c.left == r.left && c.top == r.top && c.right == r.right && c.bottom == r.bottom {
                break;
            }
        }
    }
}

fn frame_bounds(hwnd: HWND) -> (Option<RectPx>, Option<RectPx>) {
    unsafe {
        let mut fb = RECT::default();
        let ext = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut fb as *mut _ as *mut c_void,
            size_of::<RECT>() as u32,
        )
        .ok()
        .map(|_| rect_px(fb));
        let mut wr = RECT::default();
        let win = GetWindowRect(hwnd, &mut wr).ok().map(|_| rect_px(wr));
        (ext, win)
    }
}

// ---------------------------------------------------------------------------------------------
// flyout lifecycle
// ---------------------------------------------------------------------------------------------

/// Run `f` on the main thread *later* (from the main thread, Tauri runs tasks inline).
fn defer_main(app: AppHandle, f: impl FnOnce(AppHandle) + Send + 'static) {
    thread::spawn(move || {
        let a2 = app.clone();
        let _ = app.run_on_main_thread(move || f(a2));
    });
}

fn build_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let c = cfg();
    let mut b = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("Lastdrop")
        .inner_size(W_LOGICAL, H_LOGICAL)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .scroll_bar_style(ScrollBarStyle::FluentOverlay)
        .disable_drag_drop_handler()
        .zoom_hotkeys_enabled(false)
        .initialization_script(init_script());
    if let Some(t) = c.theme {
        b = b.theme(Some(t));
    }
    let w = b.build()?;
    let hwnd = w.hwnd()?;
    let dark = st().dark;
    let backdrop = unsafe { apply_native_chrome(hwnd, dark) };
    st().report.backdrop = backdrop.to_string();
    // WebView2 settings that make it feel less like a browser; keep the page hidden
    // (IsVisible=false lets WebView2 stop rendering and trim) until the first show.
    let _ = w.with_webview(|pw| unsafe {
        let ctl = pw.controller();
        if let Ok(core) = ctl.CoreWebView2() {
            if let Ok(s) = core.Settings() {
                let _ = s.SetAreDefaultContextMenusEnabled(false);
                let _ = s.SetIsStatusBarEnabled(false);
                let _ = s.SetIsZoomControlEnabled(false);
                let _ = s.SetAreDevToolsEnabled(false);
                if let Ok(s3) = s.cast::<ICoreWebView2Settings3>() {
                    let _ = s3.SetAreBrowserAcceleratorKeysEnabled(false);
                }
                if let Ok(s4) = s.cast::<ICoreWebView2Settings4>() {
                    let _ = s4.SetIsGeneralAutofillEnabled(false);
                    let _ = s4.SetIsPasswordAutosaveEnabled(false);
                }
                if let Ok(s5) = s.cast::<ICoreWebView2Settings5>() {
                    let _ = s5.SetIsPinchZoomEnabled(false);
                }
                if let Ok(s6) = s.cast::<ICoreWebView2Settings6>() {
                    let _ = s6.SetIsSwipeNavigationEnabled(false);
                }
            }
        }
        let _ = ctl.SetIsVisible(false);
    });
    Ok(w)
}

fn show(app: &AppHandle, label: &str, activate: bool) {
    let start = Instant::now();
    let seq = {
        let mut s = st();
        if s.visible || s.pending.is_some() || s.destroying {
            return;
        }
        s.seq += 1;
        s.seq
    };
    phase(&format!("{label}_show_call"));
    let (build_ms, ok) = match app.get_webview_window(LABEL) {
        Some(_) => (None, true),
        None => {
            phase(&format!("{label}_window_build_start"));
            match build_window(app) {
                Ok(_) => {
                    phase(&format!("{label}_window_built"));
                    (Some(ms(start.elapsed())), true)
                }
                Err(e) => {
                    note(&format!("window build failed: {e}."));
                    (None, false)
                }
            }
        }
    };
    if !ok {
        return;
    }
    let ready = {
        let mut s = st();
        s.pending = Some(Pending {
            seq,
            label: label.to_string(),
            start,
            activate,
            build_ms,
            page_ready_ms: None,
        });
        s.page_ready
    };
    if ready {
        present(app);
    } // else: page_ready() presents once the document has loaded
}

fn present(app: &AppHandle) {
    let Some(win) = app.get_webview_window(LABEL) else { return };
    let Ok(hwnd) = win.hwnd() else { return };
    let (activate, seq, label) = {
        let s = st();
        match &s.pending {
            Some(p) => (p.activate, p.seq, p.label.clone()),
            None => return,
        }
    };
    let (anchor, how) = anchor_rect(app);
    let (r, dpi) = popup_rect(anchor);
    let lowmem = cfg().lowmem;
    let _ = win.with_webview(move |pw| unsafe {
        let ctl = pw.controller();
        if lowmem {
            if let Ok(c19) = ctl.CoreWebView2().and_then(|c| c.cast::<ICoreWebView2_19>()) {
                let _ = c19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL);
            }
        }
        let _ = ctl.SetIsVisible(true); // also resumes a TrySuspend-ed webview
    });
    let fg_before = unsafe { GetForegroundWindow() };
    unsafe {
        place(hwnd, r);
        let _ = ShowWindow(hwnd, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
        if activate {
            let _ = SetForegroundWindow(hwnd);
        }
    }
    let fg_after = unsafe { GetForegroundWindow() };
    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let (ext, wr) = frame_bounds(hwnd);
    {
        let mut s = st();
        s.visible = true;
        s.report.flyout_rect_px = ext.or(Some(rect_px(r)));
        s.report.window_rect_px = wr;
        s.report.client_rect_px = Some(rect_px(unsafe { client_screen_rect(hwnd) }));
        s.report.dpi = dpi;
        s.report.window_ex_style = format!("{ex_style:#010x}");
        if !activate {
            s.report.focus_stolen = Some(fg_before != fg_after || fg_after == hwnd);
        }
        s.report.positioning = format!(
            "{how}: icon rect ({},{})-({},{}) -> CalculatePopupWindowPosition(TPM_CENTERALIGN|TPM_BOTTOMALIGN|TPM_VERTICAL|TPM_WORKAREA), {}px gap",
            anchor.left, anchor.top, anchor.right, anchor.bottom,
            (GAP_LOGICAL * dpi as f64 / 96.0).round()
        );
        if cfg().bench_out.is_some() {
            s.report.phases.push(Phase { name: format!("{label}_shown"), epoch_ms: epoch_ms_now() });
        }
    }
    let _ = win.eval(format!("window.__lastdrop&&window.__lastdrop.measure({seq})"));
}

fn hide(app: &AppHandle, label: &str) {
    let was = {
        let mut s = st();
        let was = s.visible || s.pending.is_some();
        s.visible = false;
        s.pending = None;
        if was && cfg().mode == Mode::Destroy {
            s.destroying = true;
            s.page_ready = false;
        }
        was
    };
    if !was {
        return;
    }
    phase(label);
    let Some(win) = app.get_webview_window(LABEL) else { return };
    if let Ok(hwnd) = win.hwnd() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    match cfg().mode {
        Mode::KeepAlive => {
            let lowmem = cfg().lowmem;
            let _ = win.with_webview(move |pw| unsafe {
                let ctl = pw.controller();
                let _ = ctl.SetIsVisible(false);
                if lowmem {
                    if let Ok(core) = ctl.CoreWebView2() {
                        if let Ok(c19) = core.cast::<ICoreWebView2_19>() {
                            let _ = c19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
                        }
                        if let Ok(c3) = core.cast::<ICoreWebView2_3>() {
                            let _ = c3.TrySuspend(&TrySuspendCompletedHandler::create(Box::new(|_, _| Ok(()))));
                        }
                    }
                }
            });
        }
        Mode::Destroy => {
            // async: tauri destroys the window (and drops the WebView2 controller) from the event loop
            let _ = win.destroy();
        }
    }
}

fn toggle(app: &AppHandle) {
    let (open, recently_closed) = {
        let s = st();
        (
            s.visible || s.pending.is_some(),
            s.last_deactivate_hide.map(|t| t.elapsed() < Duration::from_millis(350)).unwrap_or(false),
        )
    };
    if open {
        hide(app, "hide_tray_click");
    } else if !recently_closed {
        // the click on the tray icon itself deactivated (and hid) the flyout: don't reopen it
        show(app, "user", true);
    }
}

fn exit_app(app: &AppHandle) {
    phase("exit");
    let _ = app.remove_tray_by_id(TRAY_ID);
    write_report(true);
    app.exit(0);
}

// ---------------------------------------------------------------------------------------------
// IPC commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
fn page_ready(app: AppHandle, visibility: Option<String>, viewport: Option<String>) {
    let pending = {
        let mut s = st();
        s.page_ready = true;
        s.report.viewport = viewport.unwrap_or_default();
        match s.pending.as_mut() {
            Some(p) => {
                p.page_ready_ms = Some(ms(p.start.elapsed()));
                true
            }
            None => false,
        }
    };
    phase(&format!("page_ready({})", visibility.unwrap_or_default()));
    if pending {
        defer_main(app, |a| present(&a));
    }
}

#[tauri::command]
fn frame_presented(seq: u32, js_total_ms: Option<f64>, js_visible_to_frame_ms: Option<f64>) {
    let mut s = st();
    let Some(p) = s.pending.take_if(|p| p.seq == seq) else { return };
    let total = ms(p.start.elapsed());
    if cfg().bench_out.is_some() {
        s.report.phases.push(Phase { name: format!("{}_first_frame", p.label), epoch_ms: epoch_ms_now() });
    }
    match p.label.as_str() {
        "cold" => s.report.show_latency_ms.cold = Some(total),
        "warm" => s.report.show_latency_ms.warm = Some(total),
        _ => {}
    }
    if cfg().bench_out.is_none() {
        return;
    }
    s.report.show_detail.push(ShowDetail {
        label: p.label,
        seq,
        total_ms: total,
        window_build_ms: p.build_ms,
        page_ready_ms: p.page_ready_ms,
        js_total_ms,
        js_visible_to_frame_ms,
    });
}

#[tauri::command]
fn hide_flyout(app: AppHandle) {
    if cfg().bench_out.is_none() {
        defer_main(app, |a| hide(&a, "hide_escape"));
    }
}

#[tauri::command]
fn open_settings() {}

// ---------------------------------------------------------------------------------------------
// bench mode
// ---------------------------------------------------------------------------------------------

fn write_report(complete: bool) {
    let Some(path) = cfg().bench_out.clone() else { return };
    let json = {
        let mut s = st();
        s.report.complete = complete;
        serde_json::to_string_pretty(&s.report).unwrap_or_default()
    };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, &json).is_err() || std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::write(&path, &json);
        let _ = std::fs::remove_file(&tmp);
    }
}

/// msedgewebview2.exe processes that descend from this process.
fn webview_descendants() -> Vec<u32> {
    let me = std::process::id();
    let mut all: Vec<(u32, u32, bool)> = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return vec![] };
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                let end = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let name = String::from_utf16_lossy(&e.szExeFile[..end]);
                all.push((e.th32ProcessID, e.th32ParentProcessID, name.eq_ignore_ascii_case("msedgewebview2.exe")));
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    let mut tree = vec![me];
    let mut out = Vec::new();
    let mut changed = true;
    while changed {
        changed = false;
        for (pid, ppid, is_wv) in &all {
            if tree.contains(ppid) && !tree.contains(pid) {
                tree.push(*pid);
                if *is_wv {
                    out.push(*pid);
                }
                changed = true;
            }
        }
    }
    out
}

fn sample_procs(at: &str) {
    let pids = webview_descendants();
    let t_ms = ms(T0.get().unwrap().elapsed());
    st().report.webview_processes.push(ProcSample { at: at.to_string(), t_ms, count: pids.len(), pids });
}

fn bench_timeline(app: AppHandle) {
    let t0 = *T0.get().unwrap();
    let at = |s: f64| {
        let target = t0 + Duration::from_secs_f64(s * cfg().bench_scale);
        let now = Instant::now();
        if target > now {
            thread::sleep(target - now);
        }
    };
    let main = |f: fn(&AppHandle)| {
        let a = app.clone();
        let _ = app.run_on_main_thread(move || f(&a));
    };
    sample_procs("t+0.5_after_start");
    at(9.5);
    sample_procs("t+9.5_before_show");
    at(10.0);
    main(|a| show(a, "cold", false));
    at(12.0);
    write_report(false);
    at(18.0);
    sample_procs("t+18_shown_cold");
    at(20.0);
    main(|a| hide(a, "hide1"));
    at(28.0);
    sample_procs("t+28_after_hide1");
    write_report(false);
    at(30.0);
    main(|a| show(a, "warm", false));
    at(32.0);
    write_report(false);
    at(38.0);
    sample_procs("t+38_shown_warm");
    at(40.0);
    main(|a| hide(a, "hide2"));
    at(48.0);
    sample_procs("t+48_after_hide2");
    at(50.0);
    main(exit_app);
}

// ---------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------

fn main() {
    let now_i = Instant::now();
    let now_e = epoch_ms_now();
    let proc_start = process_start_epoch_ms().filter(|t| *t <= now_e).unwrap_or(now_e);
    // The first launch of a freshly built/copied exe can sit ~10 s before main() (Defender scan).
    // That would collapse the whole timeline, so rebase T0 to main() entry and say so.
    let rebased = now_e - proc_start > 2000.0;
    let t0_epoch = if rebased { now_e } else { proc_start };
    let t0 = now_i
        .checked_sub(Duration::from_secs_f64((now_e - t0_epoch) / 1000.0))
        .unwrap_or(now_i);
    let _ = T0.set(t0);
    let _ = CFG.set(parse_args());
    let c = cfg();

    let report = Report {
        variant: c.variant(),
        pid: std::process::id(),
        t0_epoch_ms: t0_epoch,
        process_start_epoch_ms: proc_start,
        t0_rebased: rebased,
        phases: vec![
            Phase { name: "process_start".into(), epoch_ms: proc_start },
            Phase { name: "main_entry".into(), epoch_ms: now_e },
        ],
        show_latency_ms: Latency::default(),
        flyout_rect_px: None,
        backdrop: "none".into(),
        renderer: String::new(),
        notes: format!(
            "{}Tauri 2.12.2 / wry 0.57 / WebView2, vanilla HTML+CSS+JS (no framework, no bundler). \
             Latency = Rust Instant at the show call -> IPC receipt of frame_presented, which the page sends \
             from the 2nd requestAnimationFrame after document.visibilityState is 'visible' (includes the IPC hop). \
             {} Show uses SW_SHOWNOACTIVATE (no focus steal); auto-hide on deactivate is off in bench mode. \
             Release profile opt-level=\"{OPT_LEVEL}\", lto, codegen-units=1, strip, panic=abort.",
            if rebased {
                format!(
                    "T0 REBASED: process start -> main() took {:.0} ms (likely AV first-run scan), so the timeline runs from main() entry; re-run for clean numbers. ",
                    now_e - proc_start
                )
            } else {
                String::new()
            },
            match c.mode {
                Mode::KeepAlive => "keepalive: window+webview created hidden at start; hide = SW_HIDE + controller.IsVisible=false.",
                Mode::Destroy => "destroy: latency measured from WebviewWindowBuilder::build start; hide = SW_HIDE + window.destroy() (controller closed, environment released).",
            }
        ),
        complete: false,
        show_detail: vec![],
        webview_processes: vec![],
        window_rect_px: None,
        client_rect_px: None,
        viewport: String::new(),
        positioning: String::new(),
        dpi: 0,
        window_ex_style: String::new(),
        focus_stolen: None,
        theme: String::new(),
        accent: String::new(),
        accent_source: String::new(),
        webview2_version: String::new(),
        user_data_folder: String::new(),
        opt_level: OPT_LEVEL.into(),
    };
    let _ = STATE.set(Mutex::new(State {
        visible: false,
        destroying: false,
        page_ready: false,
        seq: 0,
        pending: None,
        last_deactivate_hide: None,
        palette: vec![],
        dark: false,
        report,
    }));

    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![page_ready, frame_presented, hide_flyout, open_settings])
        .on_window_event(|w, e| {
            if w.label() == LABEL {
                if let WindowEvent::Destroyed = e {
                    let mut s = st();
                    s.destroying = false;
                    s.page_ready = false;
                    s.visible = false;
                    drop(s);
                    phase("window_destroyed");
                }
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            let (palette, source) = read_palette();
            let dark = apps_dark();
            let tb_dark = taskbar_dark();
            let ring_color = palette_get(&palette, if tb_dark { "accent-light2" } else { "accent-dark1" });
            {
                let mut s = st();
                s.dark = dark;
                s.report.theme = if dark { "dark".into() } else { "light".into() };
                s.report.accent = hex(palette_get(&palette, "accent"));
                s.report.accent_source = source.into();
                s.report.webview2_version = tauri::webview_version().unwrap_or_default();
                s.report.renderer = format!(
                    "WebView2 {} (Chromium, GPU process compositing) via wry 0.57 / Tauri 2.12.2",
                    s.report.webview2_version
                );
                s.report.user_data_folder = app
                    .path()
                    .app_local_data_dir()
                    .map(|p| p.join("EBWebView").display().to_string())
                    .unwrap_or_default();
                s.palette = palette;
            }

            // tray icon: runtime-drawn ring gauge
            let track = if tb_dark { [255, 255, 255] } else { [0, 0, 0] };
            let rgba = draw_ring(0.42, ring_color, track, if tb_dark { 0.32 } else { 0.22 });
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(Image::new_owned(rgba, 32, 32))
                .tooltip("Lastdrop - Claude 5-hour session 42%")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| {
                    if ev.id().as_ref() == "quit" {
                        exit_app(app);
                    }
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        toggle(tray.app_handle());
                    }
                })
                .build(app)?;
            phase("tray_created");

            if cfg().mode == Mode::KeepAlive {
                build_window(&handle)?;
                phase("window_precreated_hidden");
            }
            if cfg().bench_out.is_some() {
                let h = handle.clone();
                thread::spawn(move || bench_timeline(h));
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app, event| {
        if let RunEvent::ExitRequested { code: None, api, .. } = event {
            // destroy mode: the last window going away must not end the tray app
            api.prevent_exit();
        }
    });
}
