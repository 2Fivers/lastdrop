//! `--bench <out.json>`: the spike's measurement timeline on the real app, so
//! `tools/measure/measure.ps1` and `analyze.ps1` can compare it with the spike (ADR 0001, BENCH.md
//! of `spikes/slint-flyout`).
//!
//! T0 is the process creation time (`GetProcessTimes`). At T0 + 10 s the flyout is shown without
//! activation (cold), + 20 s hidden, + 30 s shown again (warm), + 40 s hidden, + 50 s the app
//! quits. The JSON is written at + 12 s and + 32 s (partial) and at exit: `t0EpochMs`, `phases`
//! (name + epoch ms), `showLatencyMs` (`cold`/`warm`: show call → return of the first `WM_PAINT`
//! after the show, i.e. Slint rendered and presented), `flyoutRectPx`, `trayIconRectPx`.

use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lastdrop_win::geom::Rect;
use windows::Win32::Foundation::{FILETIME, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{WM_NCDESTROY, WM_PAINT};

use crate::wiring::{self, App, with_app};

/// The subclass id of the paint watch (any value unique per window and procedure).
const PAINT_SUBCLASS_ID: usize = 0x00BE_7C11;

thread_local! {
    /// The bench the paint watch reports to (main thread only).
    static PAINT_BENCH: RefCell<Option<Rc<Bench>>> = const { RefCell::new(None) };
}

/// The measurement state of one `--bench` run (main thread only).
pub struct Bench {
    out: PathBuf,
    t0_ms: u64,
    t0: Instant,
    phases: RefCell<Vec<(String, u64)>>,
    shows: Cell<u32>,
    pending: Cell<Option<(u32, Instant)>>,
    latency: Cell<[Option<f64>; 2]>,
    paints: Cell<u32>,
    flyout_rect: Cell<Option<Rect>>,
    tray_rect: Cell<Option<Rect>>,
}

/// Milliseconds since the Unix epoch.
fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The process creation time in Unix epoch ms (now when it cannot be read).
fn process_creation_ms() -> u64 {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo handle of the current process is always valid; the four out pointers
    // point to live FILETIMEs.
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .is_ok();
    if !ok {
        return epoch_ms();
    }
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    // FILETIME counts 100 ns since 1601; the Unix epoch is 11,644,473,600 s later.
    ticks.saturating_sub(116_444_736_000_000_000) / 10_000
}

impl Bench {
    /// A bench writing to `out`, T0 = the process creation time.
    pub fn new(out: PathBuf) -> Bench {
        let t0_ms = process_creation_ms();
        let since_t0 = Duration::from_millis(epoch_ms().saturating_sub(t0_ms));
        let t0 = Instant::now()
            .checked_sub(since_t0)
            .unwrap_or_else(Instant::now);
        Bench {
            out,
            t0_ms,
            t0,
            phases: RefCell::new(vec![("processStart".to_owned(), t0_ms)]),
            shows: Cell::new(0),
            pending: Cell::new(None),
            latency: Cell::new([None, None]),
            paints: Cell::new(0),
            flyout_rect: Cell::new(None),
            tray_rect: Cell::new(None),
        }
    }

    /// Records a named phase now.
    pub fn phase(&self, name: &str) {
        self.phases.borrow_mut().push((name.to_owned(), epoch_ms()));
    }

    /// A `WM_PAINT` of the flyout returned: the first one after a show ends that show's latency.
    fn painted(&self) {
        self.paints.set(self.paints.get() + 1);
        if let Some((n, start)) = self.pending.take() {
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            let mut lat = self.latency.get();
            if let Some(slot) = lat.get_mut(n.saturating_sub(1) as usize) {
                *slot = Some(ms);
            }
            self.latency.set(lat);
            self.phase(&format!("firstFramePresented{n}"));
        }
    }

    /// Shows the flyout without activation from the tray icon (F§1.3) and starts timing.
    fn show(&self, app: &App) {
        let n = self.shows.get() + 1;
        self.shows.set(n);
        self.phase(&format!("show{n}"));
        self.pending.set(Some((n, Instant::now())));
        let anchor = match app.tray.icon_rect() {
            Some(rect) => lastdrop_win::flyout_pos::Anchor::Rect(rect),
            None => lastdrop_win::flyout_pos::Anchor::Point(lastdrop_win::geom::Point::default()),
        };
        wiring::open_flyout(app, anchor, false);
        self.flyout_rect.set(app.rt.flyout_rect.get());
        self.tray_rect.set(app.tray.icon_rect());
    }

    /// Hides the flyout.
    fn hide(&self, app: &App) {
        wiring::hide_flyout(app, false);
        self.phase(&format!("hide{}", self.shows.get()));
    }

    /// Queues the JSON for the writer thread, which replaces `out` atomically (the UI thread
    /// does no file I/O, so the timeline is not skewed by the disk). The last write at exit is
    /// queued before the writer stops, so it lands before the process ends.
    pub fn write(&self, app: &App, partial: bool) {
        let json = self.json(partial);
        app.writer
            .handle()
            .replace(self.out.clone(), json.into_bytes(), "bench output");
    }

    /// The JSON document, compatible with the spike's.
    fn json(&self, partial: bool) -> String {
        let rect = |r: Option<Rect>| match r {
            Some(r) => format!(
                "{{\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}",
                r.left,
                r.top,
                r.width(),
                r.height()
            ),
            None => "null".to_owned(),
        };
        let num = |v: Option<f64>| v.map_or_else(|| "null".to_owned(), |v| format!("{v:.2}"));
        let mut phases = String::new();
        for (i, (name, t)) in self.phases.borrow().iter().enumerate() {
            if i > 0 {
                phases.push(',');
            }
            let _ = write!(phases, "{{\"name\":{},\"epochMs\":{t}}}", json_string(name));
        }
        let lat = self.latency.get();
        format!(
            "{{\n  \"variant\": \"lastdrop-app-software\",\n  \"pid\": {},\n  \"t0EpochMs\": {},\n  \"partial\": {partial},\n  \"phases\": [{phases}],\n  \"showLatencyMs\": {{\"cold\": {}, \"warm\": {}}},\n  \"flyoutRectPx\": {},\n  \"trayIconRectPx\": {},\n  \"backdrop\": \"acrylic\",\n  \"renderer\": \"winit-software\",\n  \"slintVersion\": \"1.18.1\",\n  \"debugCounts\": {{\"wmPaint\": {}}},\n  \"notes\": {}\n}}\n",
            std::process::id(),
            self.t0_ms,
            num(lat[0]),
            num(lat[1]),
            rect(self.flyout_rect.get()),
            rect(self.tray_rect.get()),
            self.paints.get(),
            json_string(
                "showLatencyMs = show call -> return of the first WM_PAINT after the show (Slint software renderer, rendered and presented in that handler)"
            ),
        )
    }
}

/// A JSON string literal.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One step of the timeline.
type Step = Box<dyn FnOnce(&App, &Bench)>;

/// Arms the timeline (T0 + 10/12/20/30/32/40/50 s).
pub fn schedule(bench: Rc<Bench>) {
    let at = |secs: u64, f: Step, bench: Rc<Bench>| {
        let delay =
            (bench.t0 + Duration::from_secs(secs)).saturating_duration_since(Instant::now());
        slint::Timer::single_shot(delay, move || with_app(|app| f(app, &bench)));
    };
    at(10, Box::new(|app, b| b.show(app)), bench.clone());
    at(12, Box::new(|app, b| b.write(app, true)), bench.clone());
    at(20, Box::new(|app, b| b.hide(app)), bench.clone());
    at(30, Box::new(|app, b| b.show(app)), bench.clone());
    at(32, Box::new(|app, b| b.write(app, true)), bench.clone());
    at(40, Box::new(|app, b| b.hide(app)), bench.clone());
    at(
        50,
        Box::new(|_, _| {
            let _ = slint::quit_event_loop();
        }),
        bench,
    );
}

/// Watches the flyout's `WM_PAINT` to time the first frame after each show.
pub fn watch_paint(hwnd: HWND, bench: Rc<Bench>) {
    PAINT_BENCH.with(|b| *b.borrow_mut() = Some(bench));
    // SAFETY: `hwnd` is a live window of this thread; `paint_proc` matches SUBCLASSPROC and stays
    // valid for the program's life; it removes itself on WM_NCDESTROY.
    let ok = unsafe { SetWindowSubclass(hwnd, Some(paint_proc), PAINT_SUBCLASS_ID, 0) }.as_bool();
    if !ok {
        log::warn!("bench: cannot watch WM_PAINT");
    }
}

/// The paint watch's window procedure.
unsafe extern "system" fn paint_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // SAFETY: forwarding the message this procedure received to the next one in the chain.
    let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    match msg {
        WM_PAINT => {
            let bench = PAINT_BENCH.with(|b| b.borrow().clone());
            if let Some(bench) = bench {
                bench.painted();
            }
        }
        WM_NCDESTROY => {
            // SAFETY: removing this procedure's own subclass from the window being destroyed.
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(paint_proc), PAINT_SUBCLASS_ID) };
        }
        _ => {}
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_matches_the_spike_fields() {
        let b = Bench::new(PathBuf::from("unused.json"));
        b.phase("mainEntered");
        b.latency.set([Some(12.345), None]);
        b.flyout_rect.set(Some(Rect {
            left: 1548,
            top: 542,
            right: 1908,
            bottom: 1020,
        }));
        let json = b.json(true);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(v["t0EpochMs"].as_u64().is_some());
        assert_eq!(v["phases"][0]["name"], "processStart");
        assert_eq!(v["phases"][1]["name"], "mainEntered");
        assert_eq!(v["showLatencyMs"]["cold"], 12.35);
        assert!(v["showLatencyMs"]["warm"].is_null());
        assert_eq!(v["flyoutRectPx"]["w"], 360);
        assert_eq!(v["flyoutRectPx"]["h"], 478);
        assert_eq!(v["partial"], true);
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_string("a\"b\\c\n\u{1}"), "\"a\\\"b\\\\c\\n\\u0001\"");
    }
}
