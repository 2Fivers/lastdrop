// Lastdrop flyout spike: Slint 1.18 (winit backend) + tray-icon, Windows 11 only.
//
// Window lifecycle (keep-alive): the Slint window is created once at startup off-screen and
// immediately hidden with Win32. Afterwards it is shown/hidden with ShowWindow on the HWND
// (SW_SHOWNOACTIVATE for the bench, SW_SHOW + SetForegroundWindow for a tray click), because
// winit's set_visible(true) always activates the window after its first show.
#![windows_subsystem = "windows"]

slint::include_modules!();

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::winit;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, VecModel};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use windows::core::{BOOL, PCSTR, PCWSTR, w};
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmGetColorizationColor, DwmGetWindowAttribute,
    DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW,
    DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_CLOAK, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DWM_SYSTEMBACKDROP_TYPE,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, RedrawWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, RDW_INTERNALPAINT, RDW_INVALIDATE};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------------------------------------
// Process-wide state (UI thread only). Only Cells / short borrows: the subclass proc can re-enter
// while Win32 calls (ShowWindow, SetWindowPos, ...) are in flight.
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct State {
    hwnd: Cell<isize>,
    visible: Cell<bool>,
    auto_hide: Cell<bool>,
    bench: Cell<bool>,
    last_auto_hide: Cell<Option<Instant>>,
    show_count: Cell<u32>,
    pending_paint: Cell<Option<(u32, Instant)>>,
    pending_render: Cell<Option<(u32, Instant)>>,
    lat_paint: RefCell<[Option<f64>; 2]>,
    lat_render: RefCell<[Option<f64>; 2]>,
    phases: RefCell<Vec<(String, u64)>>,
    rect: Cell<Option<(i32, i32, i32, i32)>>,
    tray_rect: Cell<Option<(i32, i32, i32, i32)>>,
    backdrop: RefCell<String>,
    gfx_api: RefCell<String>,
    notes: RefCell<Vec<String>>,
    ui: RefCell<Option<slint::Weak<Flyout>>>,
    tray: RefCell<Option<TrayIcon>>,
    backdrop_kind: Cell<i32>,
    force_theme: Cell<i32>, // 0 = system, 1 = light, 2 = dark (testing only)
    no_active_look: Cell<bool>,
    single_position: Cell<bool>,
    cloak_mode: Cell<bool>,
    paints: Cell<u32>,
    renders: Cell<u32>,
}

thread_local! {
    static ST: State = State::default();
}

fn epoch_ms_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn phase(name: &str) {
    let t = epoch_ms_now();
    ST.with(|s| s.phases.borrow_mut().push((name.to_string(), t)));
}

fn note(n: impl Into<String>) {
    let n = n.into();
    ST.with(|s| s.notes.borrow_mut().push(n));
}

fn hwnd() -> Option<HWND> {
    let h = ST.with(|s| s.hwnd.get());
    (h != 0).then(|| HWND(h as *mut _))
}

fn ui() -> Option<Flyout> {
    ST.with(|s| s.ui.borrow().as_ref().and_then(|w| w.upgrade()))
}

fn process_creation_epoch_ms() -> u64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u).is_ok() {
            let ft = ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64;
            return ft.saturating_sub(116_444_736_000_000_000) / 10_000;
        }
    }
    epoch_ms_now()
}

// ---------------------------------------------------------------------------------------------
// Theme: AppsUseLightTheme / SystemUsesLightTheme + accent from UISettings
// ---------------------------------------------------------------------------------------------

fn reg_dword(subkey: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, subkey, value, RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut size))
    };
    r.is_ok().then_some(data)
}

#[derive(Clone, Copy)]
struct ThemeInfo {
    apps_dark: bool,
    taskbar_dark: bool,
    accent_fill: slint::Color,   // AccentFillColorDefault for the app theme
    accent_tray: slint::Color,   // accent tuned for the taskbar theme
}

fn ui_color(kind: windows::UI::ViewManagement::UIColorType) -> Option<slint::Color> {
    let settings = windows::UI::ViewManagement::UISettings::new().ok()?;
    let c = settings.GetColorValue(kind).ok()?;
    Some(slint::Color::from_argb_u8(255, c.R, c.G, c.B))
}

fn read_theme() -> ThemeInfo {
    use windows::UI::ViewManagement::UIColorType;
    let personalize = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let apps_dark = match ST.with(|s| s.force_theme.get()) {
        1 => false,
        2 => true,
        _ => reg_dword(personalize, w!("AppsUseLightTheme")).map(|v| v == 0).unwrap_or(false),
    };
    let taskbar_dark = reg_dword(personalize, w!("SystemUsesLightTheme")).map(|v| v == 0).unwrap_or(true);
    let dwm_fallback = || {
        let mut argb = 0u32;
        let mut opaque = BOOL::default();
        if unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.is_ok() {
            slint::Color::from_argb_u8(255, (argb >> 16) as u8, (argb >> 8) as u8, argb as u8)
        } else {
            slint::Color::from_rgb_u8(0x00, 0x5f, 0xb8)
        }
    };
    let pick = |dark: bool| {
        ui_color(if dark { UIColorType::AccentLight2 } else { UIColorType::AccentDark1 }).unwrap_or_else(dwm_fallback)
    };
    ThemeInfo { apps_dark, taskbar_dark, accent_fill: pick(apps_dark), accent_tray: pick(taskbar_dark) }
}

fn apply_theme() {
    let t = read_theme();
    if let Some(ui) = ui() {
        let g = ui.global::<Theme>();
        g.set_dark(t.apps_dark);
        g.set_accent(t.accent_fill);
    }
    if let Some(h) = hwnd() {
        let dark = BOOL::from(t.apps_dark);
        unsafe {
            let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        }
    }
    let icon = ring_icon(0.42, t.accent_tray, t.taskbar_dark);
    ST.with(|s| {
        if let Some(tray) = s.tray.borrow().as_ref() {
            let _ = tray.set_icon(Some(icon));
        }
    });
}

/// Lets the tray context menu (a Win32 popup menu owned by tray-icon/muda) follow dark mode.
/// uxtheme ordinals 135 = SetPreferredAppMode, 136 = FlushMenuThemes (undocumented, stable since 1903).
fn allow_dark_menus() {
    unsafe {
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(ux, PCSTR(135usize as *const u8)) {
            let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set_mode(1); // AllowDark
        }
        if let Some(f) = GetProcAddress(ux, PCSTR(136usize as *const u8)) {
            let flush: extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Tray icon: 32x32 RGBA ring gauge drawn at runtime (4x4 supersampled)
// ---------------------------------------------------------------------------------------------

fn ring_icon(fraction: f32, accent: slint::Color, taskbar_dark: bool) -> tray_icon::Icon {
    tray_icon::Icon::from_rgba(ring_rgba(fraction, accent, taskbar_dark), 32, 32).expect("icon")
}

fn ring_rgba(fraction: f32, accent: slint::Color, taskbar_dark: bool) -> Vec<u8> {
    const N: usize = 32;
    const SS: usize = 4;
    let (cx, cy) = (16.0f32, 16.0f32);
    let (r_out, r_in) = (14.5f32, 9.5f32);
    let r_mid = (r_out + r_in) / 2.0;
    let half_w = (r_out - r_in) / 2.0;
    let sweep = fraction.clamp(0.0, 1.0) * std::f32::consts::TAU;
    let track = if taskbar_dark { [255u8, 255, 255, 70] } else { [0u8, 0, 0, 60] };
    let fg = [accent.red(), accent.green(), accent.blue(), 255u8];
    // arc end caps (rounded)
    let cap = |a: f32| (cx + r_mid * a.sin(), cy - r_mid * a.cos());
    let (c0, c1) = (cap(0.0), cap(sweep));

    let mut buf = vec![0u8; N * N * 4];
    for py in 0..N {
        for px in 0..N {
            let (mut cov_fg, mut cov_track) = (0.0f32, 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    let (dx, dy) = (x - cx, y - cy);
                    let d = (dx * dx + dy * dy).sqrt();
                    let mut ang = dx.atan2(-dy); // 0 at 12 o'clock, clockwise
                    if ang < 0.0 {
                        ang += std::f32::consts::TAU;
                    }
                    let in_ring = d >= r_in && d <= r_out;
                    let near = |c: (f32, f32)| ((x - c.0).powi(2) + (y - c.1).powi(2)).sqrt() <= half_w;
                    if (in_ring && ang <= sweep) || near(c0) || near(c1) {
                        cov_fg += 1.0;
                    } else if in_ring {
                        cov_track += 1.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            let (a_fg, a_tr) = (cov_fg / n, cov_track / n * track[3] as f32 / 255.0);
            // fg over track (straight alpha out)
            let a = a_fg + a_tr * (1.0 - a_fg);
            let i = (py * N + px) * 4;
            if a > 0.0 {
                for ch in 0..3 {
                    let v = (fg[ch] as f32 * a_fg + track[ch] as f32 * a_tr * (1.0 - a_fg)) / a;
                    buf[i + ch] = v.round().clamp(0.0, 255.0) as u8;
                }
                buf[i + 3] = (a * 255.0).round() as u8;
            }
        }
    }
    buf
}

/// Testing aid: write the tray icon RGBA as an (uncompressed) PNG.
fn write_png(path: &str, rgba: &[u8], w: u32, h: u32) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for y in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    // zlib stream with stored (uncompressed) deflate blocks
    let mut z = vec![0x78u8, 0x01];
    for (i, block) in raw.chunks(65535).enumerate() {
        let last = (i + 1) * 65535 >= raw.len();
        z.push(last as u8);
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out)
}

// ---------------------------------------------------------------------------------------------
// Positioning: Shell_NotifyIconGetRect (via tray-icon) + CalculatePopupWindowPosition
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Anchor {
    Icon(RECT),
    Point(POINT),
    PrimaryCorner,
}

fn tray_rect_to_win(r: &tray_icon::Rect) -> RECT {
    let (x, y) = (r.position.x.round() as i32, r.position.y.round() as i32);
    RECT { left: x, top: y, right: x + r.size.width as i32, bottom: y + r.size.height as i32 }
}

fn position_window(h: HWND, anchor: Anchor) {
    unsafe {
        let mut wr = RECT::default();
        let _ = GetWindowRect(h, &mut wr);
        let size = SIZE { cx: wr.right - wr.left, cy: wr.bottom - wr.top };

        let probe = match anchor {
            Anchor::Icon(r) => POINT { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 },
            Anchor::Point(p) => p,
            Anchor::PrimaryCorner => POINT { x: 0, y: 0 },
        };
        let mon = MonitorFromPoint(probe, if matches!(anchor, Anchor::PrimaryCorner) { MONITOR_DEFAULTTOPRIMARY } else { MONITOR_DEFAULTTONEAREST });
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let work = mi.rcWork;
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let gap = (12.0 * dpi_x as f64 / 96.0).round() as i32;

        // Exclusion rect = icon rect (or a 1px rect at the point) extended up so the flyout's
        // bottom edge sits `gap` px above the taskbar, like the system flyouts.
        let (anchor_x, mut excl) = match anchor {
            Anchor::Icon(r) => ((r.left + r.right) / 2, r),
            Anchor::Point(p) => (p.x, RECT { left: p.x, top: p.y, right: p.x + 1, bottom: p.y + 1 }),
            Anchor::PrimaryCorner => {
                let x = work.right - gap - size.cx / 2;
                (x, RECT { left: x, top: work.bottom, right: x + 1, bottom: work.bottom + 1 })
            }
        };
        excl.top = excl.top.min(work.bottom) - gap;
        let pt = POINT { x: anchor_x, y: excl.top };
        let mut out = RECT::default();
        let flags = (TPM_CENTERALIGN | TPM_BOTTOMALIGN | TPM_VERTICAL | TPM_WORKAREA).0;
        let (mut x, mut y) = if CalculatePopupWindowPosition(&pt, &size, flags, Some(&excl), &mut out).is_ok() {
            (out.left, out.top)
        } else {
            (anchor_x - size.cx / 2, excl.top - size.cy)
        };
        // keep the 12px margin from the work-area edges too (Quick Settings sits 12px from the right edge)
        x = x.clamp(work.left + gap, (work.right - gap - size.cx).max(work.left + gap));
        y = y.clamp(work.top + gap, (work.bottom - gap - size.cy).max(work.top + gap));
        let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
    }
}

fn visible_rect(h: HWND) -> (i32, i32, i32, i32) {
    let mut r = RECT::default();
    unsafe {
        if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).is_err() {
            let _ = GetWindowRect(h, &mut r);
        }
    }
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

// ---------------------------------------------------------------------------------------------
// Show / hide
// ---------------------------------------------------------------------------------------------

fn show_flyout(activate: bool, anchor: Anchor) {
    let Some(h) = hwnd() else {
        // window not created yet (slow GPU init): try again shortly
        Timer::single_shot(Duration::from_millis(20), move || show_flyout(activate, anchor));
        return;
    };
    let start = Instant::now();
    let n = ST.with(|s| {
        let n = s.show_count.get() + 1;
        s.show_count.set(n);
        n
    });
    phase(&format!("show{n}"));
    position_window(h, anchor);
    // If that move crossed onto a monitor with another DPI, winit has resized the window
    // (WM_DPICHANGED is handled synchronously); place it again with the new size.
    if !ST.with(|s| s.single_position.get()) {
        position_window(h, anchor);
    }
    if let Some(ui) = ui() {
        // force a full repaint of the (possibly stale) surface on the next frame
        ui.set_repaint_tick(!ui.get_repaint_tick());
    }
    ST.with(|s| {
        s.pending_paint.set(Some((n, start)));
        s.pending_render.set(Some((n, start)));
        s.visible.set(true);
    });
    unsafe {
        if ST.with(|s| s.cloak_mode.get()) {
            set_cloak(h, false);
        } else {
            let _ = ShowWindow(h, if activate { SW_SHOW } else { SW_SHOWNOACTIVATE });
        }
        if activate {
            let _ = SetForegroundWindow(h);
        }
        // Re-showing a hidden window does not always produce a WM_PAINT (DWM keeps the old
        // surface); ask for one so Slint renders right away instead of on its throttle timer.
        let _ = RedrawWindow(Some(h), None, None, RDW_INVALIDATE | RDW_INTERNALPAINT);
    }
    let r = visible_rect(h);
    ST.with(|s| s.rect.set(Some(r)));
    if let Some(tr) = ST.with(|s| s.tray.borrow().as_ref().and_then(|t| t.rect())) {
        let tr = tray_rect_to_win(&tr);
        ST.with(|s| s.tray_rect.set(Some((tr.left, tr.top, tr.right - tr.left, tr.bottom - tr.top))));
    }
}

fn set_cloak(h: HWND, on: bool) {
    let v = BOOL::from(on);
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_CLOAK, &v as *const _ as *const _, 4);
    }
}

fn hide_flyout() {
    let Some(h) = hwnd() else { return };
    let was = ST.with(|s| s.visible.replace(false));
    unsafe {
        if ST.with(|s| s.cloak_mode.get()) {
            // stays a visible USER window (so GPU swapchains keep presenting), but DWM doesn't draw
            // it; park it off-screen so it can't take clicks either
            set_cloak(h, true);
            let _ = SetWindowPos(h, None, -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER);
        } else {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
    if was && ST.with(|s| s.bench.get()) {
        let n = ST.with(|s| s.show_count.get());
        phase(&format!("hide{n}"));
    }
}

fn on_tray_left_click(anchor: Anchor) {
    if ST.with(|s| s.visible.get()) {
        hide_flyout();
        return;
    }
    // The mouse-down on the taskbar already deactivated (and auto-hid) the flyout: this click
    // was meant to close it, so don't reopen.
    if let Some(t) = ST.with(|s| s.last_auto_hide.get()) {
        if t.elapsed() < Duration::from_millis(400) {
            return;
        }
    }
    show_flyout(true, anchor);
}

// ---------------------------------------------------------------------------------------------
// Window subclass: first-frame timing (WM_PAINT), auto-hide (WM_ACTIVATE), active-look backdrop
// (WM_NCACTIVATE), theme changes (WM_SETTINGCHANGE)
// ---------------------------------------------------------------------------------------------

fn on_painted() {
    let now = Instant::now();
    ST.with(|s| s.paints.set(s.paints.get() + 1));
    let done = ST.with(|s| {
        if let Some((n, t)) = s.pending_paint.take() {
            let ms = now.duration_since(t).as_secs_f64() * 1000.0;
            if (1..=2).contains(&n) {
                s.lat_paint.borrow_mut()[(n - 1) as usize] = Some(ms);
            }
            Some(n)
        } else {
            None
        }
    });
    if let Some(n) = done {
        phase(&format!("firstFramePresented{n}"));
        detect_gfx_modules();
    }
}

unsafe extern "system" fn subclass_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    unsafe {
        match msg {
            WM_PAINT => {
                let r = DefSubclassProc(h, msg, wp, lp);
                on_painted();
                return r;
            }
            WM_NCACTIVATE if !ST.with(|s| s.no_active_look.get()) => {
                // Keep DWM drawing the backdrop in its active state even when the flyout is shown
                // without activation (bench) - real flyouts are always active while visible.
                return DefSubclassProc(h, msg, WPARAM(1), lp);
            }
            WM_ACTIVATE => {
                let inactive = (wp.0 & 0xffff) as u32 == WA_INACTIVE;
                if inactive && ST.with(|s| s.auto_hide.get() && s.visible.get()) {
                    ST.with(|s| s.last_auto_hide.set(Some(Instant::now())));
                    Timer::single_shot(Duration::ZERO, hide_flyout);
                }
            }
            WM_SETTINGCHANGE => {
                if lp.0 != 0 {
                    let s = PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default();
                    if s == "ImmersiveColorSet" {
                        Timer::single_shot(Duration::ZERO, apply_theme);
                    }
                }
            }
            WM_DWMCOLORIZATIONCOLORCHANGED | WM_THEMECHANGED => {
                Timer::single_shot(Duration::ZERO, apply_theme);
            }
            WM_NCDESTROY => {
                let _ = RemoveWindowSubclass(h, Some(subclass_proc), 1);
            }
            _ => {}
        }
        DefSubclassProc(h, msg, wp, lp)
    }
}

fn detect_gfx_modules() {
    let mut found = Vec::new();
    for (name, label) in [
        (w!("d3d12.dll"), "d3d12"),
        (w!("dcomp.dll"), "dcomp"),
        (w!("d3d11.dll"), "d3d11"),
        (w!("vulkan-1.dll"), "vulkan"),
        (w!("opengl32.dll"), "opengl32"),
        (w!("nvoglv64.dll"), "nvoglv64"),
    ] {
        if unsafe { GetModuleHandleW(name) }.is_ok() {
            found.push(label);
        }
    }
    ST.with(|s| {
        let mut g = s.gfx_api.borrow_mut();
        let mods = format!("modules: {}", if found.is_empty() { "none".to_string() } else { found.join("+") });
        if !g.contains("modules:") {
            if !g.is_empty() {
                g.push_str("; ");
            }
            g.push_str(&mods);
        }
    });
}

/// Runs once the winit window exists: Win32/DWM styling, subclass, then hide (keep-alive).
fn setup_native_window(h: HWND) {
    ST.with(|s| s.hwnd.set(h.0 as isize));
    unsafe {
        let _ = SetWindowSubclass(h, Some(subclass_proc), 1, 0);

        // Tool window: no taskbar button, not in Alt+Tab. Drop the system menu / min / max boxes so
        // DWM never draws caption buttons into the extended frame.
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let ex = (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
        let st = GetWindowLongPtrW(h, GWL_STYLE);
        let st = st & !((WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0) as isize);
        SetWindowLongPtrW(h, GWL_STYLE, st);

        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, 4);

        let kind = ST.with(|s| s.backdrop_kind.get());
        let (bd, label): (DWM_SYSTEMBACKDROP_TYPE, &str) = match kind {
            1 => (DWMSBT_MAINWINDOW, "mica"),
            2 => (DWMSBT_NONE, "none"),
            _ => (DWMSBT_TRANSIENTWINDOW, "acrylic"),
        };
        let res = DwmSetWindowAttribute(h, DWMWA_SYSTEMBACKDROP_TYPE, &bd as *const _ as *const _, 4);
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let ext = DwmExtendFrameIntoClientArea(h, &margins);
        let backdrop = match (&res, &ext) {
            (Ok(()), Ok(())) => label.to_string(),
            _ => {
                note(format!("backdrop setup failed: {res:?} / {ext:?}"));
                "none".to_string()
            }
        };
        ST.with(|s| *s.backdrop.borrow_mut() = backdrop);

        let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        if ST.with(|s| s.cloak_mode.get()) {
            set_cloak(h, true);
        } else {
            let _ = ShowWindow(h, SW_HIDE);
        }
    }
    ST.with(|s| s.visible.set(false));
    apply_theme();
    phase("windowPrecreatedHidden");
}

// ---------------------------------------------------------------------------------------------
// Mock data
// ---------------------------------------------------------------------------------------------

fn spark_paths(values: &[f64], lo: f64, hi: f64) -> (SharedString, SharedString) {
    let n = values.len();
    let pts: Vec<(f64, f64)> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = 1.0 + 98.0 * i as f64 / (n - 1) as f64;
            let y = 38.0 - 34.0 * ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
            (x, y)
        })
        .collect();
    let mut line = String::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        line.push_str(&format!("{}{x:.2} {y:.2} ", if i == 0 { "M " } else { "L " }));
    }
    let area = format!("{line}L {:.2} 40 L {:.2} 40 Z", pts[n - 1].0, pts[0].0);
    (line.trim_end().into(), area.into())
}

fn row(label: &str, value: f32, detail: &str) -> UsageRowData {
    UsageRowData { label: label.into(), value, detail: detail.into() }
}

fn populate(ui: &Flyout) {
    ui.set_claude_rows(ModelRc::new(VecModel::from(vec![
        row("5-hour session", 0.42, "Resets in 2 h 14 min"),
        row("Weekly · all models", 0.61, "Resets Thu 09:00"),
        row("Weekly · Fable", 0.18, ""),
    ])));
    ui.set_codex_rows(ModelRc::new(VecModel::from(vec![row("Weekly", 0.05, "Resets Fri 14:26")])));
    let codex = [3.0, 5.0, 4.0, 8.0, 12.0, 9.0, 15.0, 18.0, 14.0, 20.0, 26.0, 22.0, 30.0, 5.0];
    let (l, a) = spark_paths(&codex, 0.0, 32.0);
    ui.set_codex_line(l);
    ui.set_codex_area(a);
    let meshy: Vec<f64> = (0..30)
        .map(|i| {
            let t = i as f64 / 29.0;
            let base = 2400.0 - 1160.0 * t;
            let noise = if i == 0 || i == 29 { 0.0 } else { 28.0 * (i as f64 * 1.7).sin() + 12.0 * (i as f64 * 4.3).cos() };
            base + noise
        })
        .collect();
    let (l, a) = spark_paths(&meshy, 1150.0, 2450.0);
    ui.set_meshy_line(l);
    ui.set_meshy_area(a);
    // keep the model trait import used (rows are a dynamic model, like the real app would bind)
    let _ = ui.get_claude_rows().row_count();
}

// ---------------------------------------------------------------------------------------------
// Bench output
// ---------------------------------------------------------------------------------------------

struct Bench {
    out: std::path::PathBuf,
    variant: String,
    t0_ms: u64,
    renderer: String,
}

fn jstr(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jnum(v: Option<f64>) -> String {
    v.map(|v| format!("{v:.2}")).unwrap_or_else(|| "null".into())
}

fn write_json(b: &Bench, partial: bool) {
    let json = ST.with(|s| {
        let phases = s
            .phases
            .borrow()
            .iter()
            .map(|(n, t)| format!("{{\"name\":{},\"epochMs\":{t}}}", jstr(n)))
            .collect::<Vec<_>>()
            .join(",");
        let lp = *s.lat_paint.borrow();
        let lr = *s.lat_render.borrow();
        let rect = match s.rect.get() {
            Some((x, y, w, h)) => format!("{{\"x\":{x},\"y\":{y},\"w\":{w},\"h\":{h}}}"),
            None => "null".into(),
        };
        let renderer = format!("{} [{}]", b.renderer, s.gfx_api.borrow());
        let tray_rect = match s.tray_rect.get() {
            Some((x, y, w, h)) => format!("{{\"x\":{x},\"y\":{y},\"w\":{w},\"h\":{h}}}"),
            None => "null".into(),
        };
        let mut notes = s.notes.borrow().clone();
        notes.insert(0, "showLatencyMs = show call -> return of the first WM_PAINT after show (Slint rendered + presented in that handler); showLatencyAfterRenderingNotifierMs = show call -> Slint AfterRendering notifier (GPU renderers only, fires just before present)".into());
        format!(
            "{{\n  \"variant\": {},\n  \"pid\": {},\n  \"t0EpochMs\": {},\n  \"partial\": {},\n  \"phases\": [{}],\n  \"showLatencyMs\": {{\"cold\": {}, \"warm\": {}}},\n  \"showLatencyAfterRenderingNotifierMs\": {{\"cold\": {}, \"warm\": {}}},\n  \"flyoutRectPx\": {},\n  \"trayIconRectPx\": {},\n  \"backdrop\": {},\n  \"renderer\": {},\n  \"slintVersion\": \"1.18.1\",\n  \"debugCounts\": {{\"wmPaint\": {}, \"afterRenderingNotifier\": {}}},\n  \"notes\": {}\n}}\n",
            jstr(&b.variant),
            std::process::id(),
            b.t0_ms,
            partial,
            phases,
            jnum(lp[0]),
            jnum(lp[1]),
            jnum(lr[0]),
            jnum(lr[1]),
            rect,
            tray_rect,
            jstr(&s.backdrop.borrow()),
            jstr(&renderer),
            s.paints.get(),
            s.renders.get(),
            jstr(&notes.join(" | ")),
        )
    });
    let tmp = b.out.with_extension("json.tmp");
    if std::fs::write(&tmp, json.as_bytes()).is_ok() {
        let _ = std::fs::rename(&tmp, &b.out);
    }
}

// ---------------------------------------------------------------------------------------------

fn quit_app() {
    let tray = ST.with(|s| s.tray.borrow_mut().take());
    drop(tray); // NIM_DELETE
    hide_flyout();
    let _ = slint::quit_event_loop();
}

fn main() {
    let t0_ms = process_creation_epoch_ms();
    let now_ms = epoch_ms_now();
    let t0_inst = Instant::now().checked_sub(Duration::from_millis(now_ms.saturating_sub(t0_ms))).unwrap_or_else(Instant::now);

    // ---- args
    let args: Vec<String> = std::env::args().collect();
    let mut bench_out: Option<String> = None;
    let mut variant: Option<String> = None;
    let mut backdrop_kind = 0;
    let mut timescale = 1.0f64;
    let mut force_theme = 0;
    let mut no_active_look = false;
    let mut redirection_override: Option<bool> = None;
    let mut height_override: Option<f32> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bench" => {
                bench_out = args.get(i + 1).cloned();
                i += 1;
            }
            "--variant" => {
                variant = args.get(i + 1).cloned();
                i += 1;
            }
            "--timescale" => {
                // smoke tests only: compress the 50 s bench timeline (e.g. 0.3 -> 15 s)
                timescale = args.get(i + 1).and_then(|s| s.parse::<f64>().ok()).filter(|v| *v > 0.0).unwrap_or(1.0);
                i += 1;
            }
            "--theme" => {
                // testing only: override AppsUseLightTheme
                force_theme = match args.get(i + 1).map(|s| s.as_str()) {
                    Some("light") => 1,
                    Some("dark") => 2,
                    _ => 0,
                };
                i += 1;
            }
            "--no-active-look" => no_active_look = true,
            "--single-position" => ST.with(|s| s.single_position.set(true)),
            "--hide-mode" => {
                // hide (default): ShowWindow SW_HIDE / SW_SHOWNOACTIVATE
                // cloak: window stays shown, DWMWA_CLOAK toggled and parked off-screen when hidden
                let cloak = args.get(i + 1).map(|s| s == "cloak").unwrap_or(false);
                ST.with(|s| s.cloak_mode.set(cloak));
                if cloak {
                    note("hide mode: DWM cloak");
                }
                i += 1;
            }
            "--height" => {
                // screenshots only: taller window showing all three cards (default 440 per spec)
                height_override = args.get(i + 1).and_then(|s| s.parse::<f32>().ok()).filter(|v| *v >= 200.0);
                i += 1;
            }
            "--dump-icon" => {
                // testing aid: write the runtime-drawn tray icon (light + dark taskbar) and exit
                if let Some(base) = args.get(i + 1) {
                    let t = read_theme();
                    let _ = write_png(&format!("{base}-light-taskbar.png"), &ring_rgba(0.42, t.accent_tray, false), 32, 32);
                    let _ = write_png(&format!("{base}-dark-taskbar.png"), &ring_rgba(0.42, t.accent_tray, true), 32, 32);
                }
                return;
            }
            "--redirection-bitmap" => {
                redirection_override = match args.get(i + 1).map(|s| s.as_str()) {
                    Some("on") => Some(true),
                    Some("off") => Some(false),
                    _ => None,
                };
                i += 1;
            }
            "--backdrop" => {
                backdrop_kind = match args.get(i + 1).map(|s| s.as_str()) {
                    Some("mica") => 1,
                    Some("none") => 2,
                    _ => 0,
                };
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    let bench_mode = bench_out.is_some();
    ST.with(|s| {
        s.bench.set(bench_mode);
        s.auto_hide.set(!bench_mode);
        s.backdrop_kind.set(backdrop_kind);
        s.force_theme.set(force_theme);
        s.no_active_look.set(no_active_look);
    });
    ST.with(|s| s.phases.borrow_mut().push(("processStart".into(), t0_ms)));
    phase("mainEntered");

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    allow_dark_menus();

    // Skia / FemtoVG-on-wgpu render through DX12; only a DirectComposition swapchain supports
    // per-pixel alpha, which the DWM backdrop needs. Respect an explicit override.
    if std::env::var_os("WGPU_DX12_PRESENTATION_SYSTEM").is_none() {
        unsafe { std::env::set_var("WGPU_DX12_PRESENTATION_SYSTEM", "Visual") };
    }
    let backend_env = std::env::var("SLINT_BACKEND").unwrap_or_default();
    let renderer_label = if backend_env.is_empty() { "default".to_string() } else { backend_env.to_lowercase() };
    let variant = variant.unwrap_or_else(|| format!("slint-keepalive-{}", renderer_label.trim_start_matches("winit-")));

    // Experiment switch for the wgpu (D3D12 + DirectComposition) renderers: a DComp-only window has
    // no GDI redirection surface. It does not fix Skia-on-wgpu transparency in Slint 1.18.1 (the
    // swapchain is created opaque, see BENCH.md), so it is off by default. GDI (software) and WGL
    // (OpenGL) rendering need the redirection surface.
    let no_redirection = redirection_override == Some(false);
    if no_redirection {
        note("WS_EX_NOREDIRECTIONBITMAP set (DirectComposition-only window)");
    }

    slint::BackendSelector::new()
        .with_winit_window_attributes_hook(move |attrs| {
            use winit::platform::windows::{BackdropType, CornerPreference, WindowAttributesExtWindows};
            attrs
                .with_no_redirection_bitmap(no_redirection)
                .with_active(false)
                .with_skip_taskbar(true)
                .with_decorations(false)
                .with_resizable(false)
                .with_window_level(winit::window::WindowLevel::AlwaysOnTop)
                .with_position(winit::dpi::PhysicalPosition::new(-32000, -32000))
                .with_system_backdrop(BackdropType::TransientWindow)
                .with_corner_preference(CornerPreference::Round)
        })
        .select()
        .expect("slint backend");

    let ui = Flyout::new().expect("ui");
    ST.with(|s| *s.ui.borrow_mut() = Some(ui.as_weak()));
    populate(&ui);
    if let Some(h) = height_override {
        ui.set_flyout_height(h);
        note(format!("--height {h}: not the 440 px spec size, screenshot use only"));
    }

    ui.on_refresh({
        let w = ui.as_weak();
        move || {
            if let Some(ui) = w.upgrade() {
                ui.set_updated_text("Updated just now".into());
            }
        }
    });
    ui.on_open_settings(|| {});
    ui.on_dismiss(|| {
        if !ST.with(|s| s.bench.get()) {
            hide_flyout();
        }
    });

    // AfterRendering notifier (supported by FemtoVG and Skia, not by the software renderer)
    let notifier = ui.window().set_rendering_notifier(|state, api| match state {
        slint::RenderingState::RenderingSetup => {
            let kind = match api {
                slint::GraphicsAPI::NativeOpenGL { .. } => "notifier: OpenGL",
                _ => "notifier: non-GL",
            };
            ST.with(|s| *s.gfx_api.borrow_mut() = kind.into());
        }
        slint::RenderingState::AfterRendering => {
            let now = Instant::now();
            ST.with(|s| {
                s.renders.set(s.renders.get() + 1);
                if let Some((n, t)) = s.pending_render.take() {
                    if (1..=2).contains(&n) {
                        s.lat_render.borrow_mut()[(n - 1) as usize] = Some(now.duration_since(t).as_secs_f64() * 1000.0);
                    }
                }
            });
        }
        _ => {}
    });
    if let Err(e) = notifier {
        note(format!("rendering notifier unavailable ({e:?}) - software renderer"));
    }

    // ---- tray icon (T0 + a few ms)
    let menu = Menu::new();
    let quit_item = MenuItem::new("Quit", true, None);
    let _ = menu.append(&quit_item);
    let quit_id = quit_item.id().clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == quit_id {
            let _ = slint::invoke_from_event_loop(quit_app);
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, position, .. } = e {
            let r = tray_rect_to_win(&rect);
            let anchor = if r.right > r.left { Anchor::Icon(r) } else { Anchor::Point(POINT { x: position.x as i32, y: position.y as i32 }) };
            let _ = slint::invoke_from_event_loop(move || on_tray_left_click(anchor));
        }
    }));
    let t = read_theme();
    let tray = TrayIconBuilder::new()
        .with_icon(ring_icon(0.42, t.accent_tray, t.taskbar_dark))
        .with_tooltip("Lastdrop - Claude 5-hour session 42%")
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .expect("tray icon");
    ST.with(|s| *s.tray.borrow_mut() = Some(tray));
    phase("trayCreated");

    // ---- keep-alive: create the window now (off-screen, not activated), hide it once it exists
    ui.show().expect("show");
    {
        let w = ui.as_weak();
        let _ = slint::spawn_local(async move {
            let Some(ui) = w.upgrade() else { return };
            if ui.window().winit_window().await.is_err() {
                return;
            }
            let handle = ui.window().window_handle();
            let raw = handle.window_handle().map(|h| h.as_raw());
            if let Ok(RawWindowHandle::Win32(h)) = raw {
                setup_native_window(HWND(h.hwnd.get() as *mut _));
            }
        });
    }

    // ---- bench timeline
    let bench = bench_out.map(|out| {
        std::rc::Rc::new(Bench { out: out.into(), variant, t0_ms, renderer: renderer_label.clone() })
    });
    if let Some(b) = bench.clone() {
        if timescale != 1.0 {
            note(format!("timescale {timescale} (smoke test, not a valid measurement)"));
        }
        let at = move |secs: u64, f: Box<dyn FnOnce()>| {
            let delay = (t0_inst + Duration::from_secs_f64(secs as f64 * timescale)).saturating_duration_since(Instant::now());
            Timer::single_shot(delay, f);
        };
        let primary_or_icon = || {
            ST.with(|s| s.tray.borrow().as_ref().and_then(|t| t.rect()))
                .map(|r| tray_rect_to_win(&r))
                .filter(|r| r.right > r.left && r.bottom > r.top)
                .map(Anchor::Icon)
                .unwrap_or(Anchor::PrimaryCorner)
        };
        at(10, Box::new(move || {
            let a = primary_or_icon();
            if matches!(a, Anchor::PrimaryCorner) {
                note("tray icon rect unavailable (icon probably in the overflow area) - anchored to primary work-area corner");
            }
            show_flyout(false, a);
        }));
        let b12 = b.clone();
        at(12, Box::new(move || write_json(&b12, true)));
        at(20, Box::new(hide_flyout));
        at(30, Box::new(move || show_flyout(false, primary_or_icon())));
        let b32 = b.clone();
        at(32, Box::new(move || write_json(&b32, true)));
        at(40, Box::new(hide_flyout));
        at(50, Box::new(move || {
            phase("exit");
            quit_app();
        }));
    }

    slint::run_event_loop_until_quit().expect("event loop");

    let tray = ST.with(|s| s.tray.borrow_mut().take());
    drop(tray);
    if let Some(b) = bench {
        write_json(&b, false);
    }
}
