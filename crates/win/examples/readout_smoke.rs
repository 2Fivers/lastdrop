//! Smoke run of the taskbar readout on the real taskbar (developer tool, not part of the app).
//!
//! Builds `ReadoutModel`s with `lastdrop-core`'s real pipeline (`stats::evaluate` →
//! `build_readout`) from **invented** Claude snapshots, docks them with the readout thread for a
//! few seconds per state, screenshots only the readout's rectangle into PNGs, renders the tray
//! icons of the same states, records the gathered taskbar geometry as JSON (rectangles, DPI and
//! flags only), then shuts the thread down. Runs for well under 20 s and never takes focus.
//!
//! ```text
//! cargo run -p lastdrop-win --example readout_smoke -- <output directory>
//! ```
#![expect(
    clippy::print_stderr,
    reason = "a developer smoke tool reports to its console"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use lastdrop_core::grammar::{English, LocaleFormat, TextContext, TimeStyle};
use lastdrop_core::jiff::tz::{self, TimeZone};
use lastdrop_core::jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{
    AccountRef, Health, Meter, MeterId, MeterKey, MeterNames, ProviderId, Snapshot, SourceKind,
    WindowLen,
};
use lastdrop_core::readout::{
    BuildContext, ReadoutModel, TrayModel, build_readout, build_tray, compute_layout,
};
use lastdrop_core::settings::{ProviderEntry, ProviderSettings, Settings};
use lastdrop_core::stats::{self, EvalInput, EvalMemory, Sample, ServiceInput, ServiceStatus};
use lastdrop_win::geom::Rect;
use lastdrop_win::occupancy::{OccupancyHandle, OccupancyResult};
use lastdrop_win::palette::Argb;
use lastdrop_win::placement::Placement;
use lastdrop_win::readout::{ReadoutConfig, ReadoutEvent, ReadoutHandle, taskbar_geometry};
use lastdrop_win::render::{RenderFactory, TextMeasurer};
use lastdrop_win::theme::{AccentShades, ThemeInfo};
use lastdrop_win::trayicon;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomationInvokePattern, TreeScope_Children,
    UIA_InvokePatternId,
};
use windows::Win32::UI::Controls::{WM_MOUSEHOVER, WM_MOUSELEAVE};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_RBUTTONDOWN,
    WM_RBUTTONUP,
};
use windows::core::w;

/// The local zone of the invented data (Sweden in October, CEST).
fn zone() -> TimeZone {
    TimeZone::fixed(tz::offset(2))
}

fn mins(m: i64) -> SignedDuration {
    SignedDuration::from_mins(m)
}

fn five_hour(used: f64, resets_at: Timestamp) -> Meter {
    Meter {
        id: MeterId::new(MeterId::FIVE_HOUR),
        names: MeterNames::new("5h", "5-hour", "5-hour session"),
        order: 0,
        used,
        limited: false,
        resets_at: Some(resets_at),
        window: WindowLen::Fixed(SignedDuration::from_hours(5)),
        starts_on_use: true,
    }
}

fn weekly(used: f64, resets_at: Timestamp) -> Meter {
    Meter {
        id: MeterId::new(MeterId::WEEKLY),
        names: MeterNames::new("Wk", "weekly", "Weekly · all models"),
        order: 1,
        used,
        limited: false,
        resets_at: Some(resets_at),
        window: WindowLen::Fixed(SignedDuration::from_hours(7 * 24)),
        starts_on_use: false,
    }
}

/// One invented state: 5-hour readings `(minutes before now, used)` and its reset, weekly use.
struct State {
    name: &'static str,
    five_hour: &'static [(i64, f64)],
    five_hour_reset_in: i64,
    weekly: f64,
}

const STATES: [State; 3] = [
    // R mock 1a: on track.
    State {
        name: "ok",
        five_hour: &[(30, 0.375), (0, 0.42)],
        five_hour_reset_in: 134,
        weekly: 0.61,
    },
    // R mock 1c: runs out before the reset (WARNING, `Out HH:MM`).
    State {
        name: "warning",
        five_hour: &[(30, 0.71), (0, 0.78)],
        five_hour_reset_in: 165,
        weekly: 0.61,
    },
    // R mock 1d: empty in 12 minutes (CRITICAL, countdown).
    State {
        name: "critical",
        five_hour: &[(30, 0.79), (0, 0.94)],
        five_hour_reset_in: 100,
        weekly: 0.61,
    },
];

fn models(state: &State, now: Timestamp) -> (ReadoutModel, TrayModel) {
    let reset = now + mins(state.five_hour_reset_in);
    let weekly_reset = now + mins(2 * 24 * 60 + 20 * 60);
    let latest = state.five_hour.last().map_or(0.0, |&(_, u)| u);
    let snapshot = Snapshot {
        provider: ProviderId::Claude,
        account: AccountRef::primary(),
        source: SourceKind::ClaudeOAuthUsage,
        measured_at: now,
        fetched_at: now,
        meters: vec![five_hour(latest, reset), weekly(state.weekly, weekly_reset)],
        balances: Vec::new(),
        health: Health::Ok,
    };
    let mut history: BTreeMap<MeterKey, Vec<Sample>> = BTreeMap::new();
    history.insert(
        MeterKey {
            provider: ProviderId::Claude,
            account: AccountRef::primary().key,
            meter: MeterId::new(MeterId::FIVE_HOUR),
        },
        state
            .five_hour
            .iter()
            .map(|&(ago, u)| Sample::window(now - mins(ago), u, Some(reset)))
            .collect(),
    );
    let settings = Settings {
        providers: vec![ProviderEntry::Known(ProviderSettings::new(
            ProviderId::Claude,
            true,
        ))],
        ..Settings::default()
    };
    let status = ServiceStatus {
        last_success: Some(now),
        poll_interval: Some(mins(5)),
        ..ServiceStatus::default()
    };
    let account = AccountRef::primary();
    let services = [ServiceInput {
        provider: ProviderId::Claude,
        account: &account,
        snapshot: Some(&snapshot),
        status: &status,
        fetching: false,
    }];
    let eval = stats::evaluate(
        &EvalInput {
            now,
            settings: &settings,
            services: &services,
            history: &history,
        },
        &mut EvalMemory::default(),
    );
    let locale = LocaleFormat::en_gb(zone());
    let cx = BuildContext {
        text: TextContext {
            now,
            time_style: TimeStyle::Clock,
            locale: &locale,
            strings: &English,
        },
        settings: &settings,
    };
    (build_readout(&eval, &cx), build_tray(&eval, &cx))
}

/// The taskbar theme from `SystemUsesLightTheme` (the app reads it with `theme::read`).
fn theme() -> ThemeInfo {
    let mut light = 0u32;
    let mut size = 4u32;
    // SAFETY: valid out pointers of the stated size.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut light).cast()),
            Some(&mut size),
        )
    };
    let dark = status != ERROR_SUCCESS || light == 0;
    let accent = Argb(0xFF8E_3AA7);
    ThemeInfo {
        taskbar_dark: dark,
        apps_dark: dark,
        accent: AccentShades {
            accent,
            dark1: accent,
            dark2: Argb(0xFF72_2C88),
            light2: Argb(0xFFD5_8AE8),
            light3: Argb(0xFFE7_B3F3),
        },
        transparency: true,
        high_contrast: None,
    }
}

/// Copies `rect` of the screen (layered windows included) as straight RGBA.
fn capture(rect: Rect) -> Option<Vec<u8>> {
    let (w, h) = (rect.width(), rect.height());
    if w <= 0 || h <= 0 {
        return None;
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: 40,
            biWidth: w,
            biHeight: -h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..BITMAPINFOHEADER::default()
        },
        ..BITMAPINFO::default()
    };
    // SAFETY: screen DC, a memory DC and a DIB, all released below; `bits` covers w × h × 4.
    unsafe {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let mut bits = core::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        let old = SelectObject(dc, bitmap.into());
        let copied = BitBlt(
            dc,
            0,
            0,
            w,
            h,
            Some(screen),
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        );
        let len = usize::try_from(w * h * 4).ok()?;
        let bgra = std::slice::from_raw_parts(bits.cast::<u8>(), len).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        copied.ok()?;
        Some(
            bgra.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect(),
        )
    }
}

/// An uncompressed PNG (stored deflate blocks), from the spike.
fn write_png(path: &Path, rgba: &[u8], w: u32, h: u32) -> std::io::Result<()> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(0).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let row = w as usize * 4;
    let mut raw = Vec::new();
    for y in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[y * row..(y + 1) * row]);
    }
    let mut z = vec![0x78u8, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, block) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = u16::try_from(block.len()).unwrap_or(0);
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + u32::from(byte)) % 65521;
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

/// Waits up to `timeout` for a docked placement after `since` events.
fn docked_rect(events: &Mutex<Vec<ReadoutEvent>>, timeout: Duration) -> Option<Rect> {
    let end = Instant::now() + timeout;
    loop {
        let found = events.lock().ok()?.iter().rev().find_map(|e| match e {
            ReadoutEvent::PlacementChanged { placement, .. } => Some(*placement),
            _ => None,
        });
        match found {
            Some(Placement::Docked { rect, .. } | Placement::Floating { rect, .. }) => {
                return Some(rect);
            }
            _ if Instant::now() >= end => return None,
            _ => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Posts synthetic pointer messages to the readout window (the real cursor is never moved and
/// nothing is activated) and drives its UI Automation root, then checks the events.
fn check_input_and_uia(events: &Mutex<Vec<ReadoutEvent>>) {
    // SAFETY: a lookup of our own window class.
    let Ok(readout) = (unsafe { FindWindowW(w!("LastdropReadout"), None) }) else {
        eprintln!("input: readout window not found");
        return;
    };
    let before = events.lock().map(|l| l.len()).unwrap_or(0);
    let post = |msg: u32, x: i32, y: i32| {
        let lparam = ((y as u16 as isize) << 16) | (x as u16 as isize);
        // SAFETY: posting to our own window on the readout thread.
        let _ = unsafe { PostMessageW(Some(readout), msg, WPARAM(0), LPARAM(lparam)) };
    };
    post(WM_MOUSEHOVER, 20, 20);
    post(WM_LBUTTONDOWN, 20, 20);
    post(WM_LBUTTONUP, 20, 20);
    std::thread::sleep(Duration::from_millis(100));
    // A double click is one click.
    post(WM_LBUTTONDOWN, 20, 20);
    post(WM_LBUTTONUP, 20, 20);
    post(WM_LBUTTONDBLCLK, 20, 20);
    post(WM_LBUTTONUP, 20, 20);
    post(WM_RBUTTONDOWN, 30, 10);
    post(WM_RBUTTONUP, 30, 10);
    post(WM_MOUSELEAVE, 0, 0);
    std::thread::sleep(Duration::from_millis(300));

    // SAFETY: UI Automation client calls on this (STA) thread; the provider answers on the
    // readout thread.
    let uia = unsafe {
        CoCreateInstance::<_, IUIAutomation>(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).and_then(
            |a| {
                let e = a.ElementFromHandle(readout)?;
                let name = e.CurrentName()?;
                let help = e.CurrentHelpText()?;
                let kind = e.CurrentControlType()?;
                let rows = e.FindAll(TreeScope_Children, &a.CreateTrueCondition()?)?;
                let count = rows.Length()?;
                let first_row = if count > 0 {
                    rows.GetElement(0)?.CurrentName()?.to_string()
                } else {
                    String::new()
                };
                let invoke: IUIAutomationInvokePattern =
                    e.GetCurrentPatternAs(UIA_InvokePatternId)?;
                invoke.Invoke()?;
                Ok((name.to_string(), help.to_string(), kind.0, count, first_row))
            },
        )
    };
    match uia {
        Ok((name, help, kind, count, first_row)) => {
            eprintln!("uia: {kind} {name:?} help {help:?}; {count} rows; row 1 {first_row:?}");
        }
        Err(e) => eprintln!("uia: {e}"),
    }
    std::thread::sleep(Duration::from_millis(200));
    if let Ok(list) = events.lock() {
        let kinds: Vec<String> = list[before..]
            .iter()
            .map(|e| match e {
                ReadoutEvent::Hover { .. } => "Hover".to_owned(),
                ReadoutEvent::RightClick { .. } => "RightClick".to_owned(),
                ReadoutEvent::PlacementChanged { .. } => "PlacementChanged".to_owned(),
                other => format!("{other:?}"),
            })
            .collect();
        eprintln!("input events: {kinds:?}");
    }
}

/// Prints the readout thread's log lines (with their time) and counts placements and frames,
/// so a runaway event loop shows up as a large count while nothing changes.
struct Console {
    started: Instant,
    places: AtomicUsize,
    frames: AtomicUsize,
}

impl log::Log for Console {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        let text = record.args().to_string();
        if text == "readout: place" {
            self.places.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if text.starts_with("readout: frame") {
            self.frames.fetch_add(1, Ordering::Relaxed);
        }
        let ms = self.started.elapsed().as_millis();
        eprintln!("{ms:>6} ms {:<5} {text}", record.level());
    }

    fn flush(&self) {}
}

static CONSOLE: OnceLock<Console> = OnceLock::new();

fn main() {
    let console = CONSOLE.get_or_init(|| Console {
        started: Instant::now(),
        places: AtomicUsize::new(0),
        frames: AtomicUsize::new(0),
    });
    if log::set_logger(console).is_ok() {
        log::set_max_level(log::LevelFilter::Trace);
    }
    let out: PathBuf = std::env::args_os()
        .nth(1)
        .map_or_else(std::env::temp_dir, PathBuf::from);
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("cannot create {}: {e}", out.display());
        return;
    }
    let started = Instant::now();
    // SAFETY: COM for this thread (tray icon rendering and measurement).
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let theme = theme();
    eprintln!(
        "taskbar theme: {}",
        if theme.taskbar_dark { "dark" } else { "light" }
    );

    // The real geometry with a fresh app-button measurement, recorded for tests/layouts.
    let (tx, rx) = mpsc::channel();
    match OccupancyHandle::spawn(Box::new(move |r| {
        let _ = tx.send(r);
    })) {
        Ok(occupancy) => {
            let taskbar = taskbar_geometry(None);
            let buttons = if taskbar.is_some() {
                // SAFETY: a plain lookup.
                let hwnd = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) };
                if let Ok(hwnd) = hwnd {
                    occupancy.request(hwnd.0 as isize);
                }
                match rx.recv_timeout(Duration::from_secs(4)) {
                    Ok(OccupancyResult::Measured(e)) => Some(e),
                    _ => None,
                }
            } else {
                None
            };
            occupancy.shutdown();
            if let Some(geometry) = taskbar_geometry(buttons) {
                let json = serde_json::to_string_pretty(&geometry).unwrap_or_default();
                let path = out.join("readout-smoke-geometry.json");
                if let Err(e) = std::fs::write(&path, json) {
                    eprintln!("geometry not written: {e}");
                }
                eprintln!("geometry: {geometry:?}");
                if let Ok(factory) = RenderFactory::new() {
                    let measurer = TextMeasurer::new(&factory);
                    let now = Timestamp::now();
                    let (model, _) = models(&STATES[0], now);
                    let layout =
                        compute_layout(&model.widths, f64::from(geometry.dpi) / 96.0, &measurer);
                    let json = serde_json::to_string_pretty(&layout).unwrap_or_default();
                    let _ = std::fs::write(out.join("readout-smoke-layout.json"), json);
                }
            }
        }
        Err(e) => eprintln!("occupancy thread: {e}"),
    }

    let events: Arc<Mutex<Vec<ReadoutEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let handle = match ReadoutHandle::spawn(
        ReadoutConfig {
            enabled: true,
            hover_delay: None,
            theme,
        },
        Box::new(move |e| {
            if let Ok(mut list) = sink_events.lock() {
                list.push(e);
            }
        }),
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("readout thread: {e}");
            return;
        }
    };

    let factory = RenderFactory::new();
    for state in &STATES {
        let now = Timestamp::now();
        let (model, tray) = models(state, now);
        let texts: Vec<Vec<&str>> = model.rows.iter().map(|r| r.right.texts()).collect();
        eprintln!("{}: {:?} {:?}", state.name, model.content, texts);
        handle.update(Arc::new(model));
        // Let it place and draw (the first time waits for the app-button measurement).
        let Some(rect) = docked_rect(&events, Duration::from_secs(5)) else {
            eprintln!("{}: not docked", state.name);
            continue;
        };
        std::thread::sleep(Duration::from_millis(400));
        match capture(rect) {
            Some(rgba) => {
                let path = out.join(format!("readout-smoke-{}.png", state.name));
                let (w, h) = (rect.width().unsigned_abs(), rect.height().unsigned_abs());
                match write_png(&path, &rgba, w, h) {
                    Ok(()) => eprintln!("{}: {rect:?} → {}", state.name, path.display()),
                    Err(e) => eprintln!("{}: not written: {e}", state.name),
                }
            }
            None => eprintln!("{}: capture failed", state.name),
        }
        if let Ok(factory) = &factory {
            for size in [16, 20, 24, 32] {
                match trayicon::render(&tray, size, &theme, factory) {
                    Ok(img) => {
                        let path = out.join(format!("tray-smoke-{}-{size}.png", state.name));
                        let _ = write_png(&path, &img.rgba, img.size, img.size);
                    }
                    Err(e) => eprintln!("tray {size}: {e}"),
                }
            }
        }
        if started.elapsed() > Duration::from_secs(15) {
            break;
        }
    }
    check_input_and_uia(&events);
    // Idle: nothing changes, so placements come only from real taskbar or foreground events.
    let (places, frames) = (
        console.places.load(Ordering::Relaxed),
        console.frames.load(Ordering::Relaxed),
    );
    std::thread::sleep(Duration::from_secs(4));
    eprintln!(
        "idle 4 s: {} placements, {} frames",
        console.places.load(Ordering::Relaxed) - places,
        console.frames.load(Ordering::Relaxed) - frames
    );
    handle.shutdown();
    if let Ok(list) = events.lock() {
        for e in list.iter() {
            eprintln!("event: {e:?}");
        }
    }
    eprintln!("done in {:?}", started.elapsed());
}
