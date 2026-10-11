//! The readout thread: its two windows, the WinEvent hooks, the occupancy thread's answers, the
//! mailbox from the main thread, placement, drawing and pointer input (R§6, R§10–R§12).
//!
//! Re-entrancy: the thread's state lives in a thread-local `RefCell`. Window procedures borrow it
//! with `try_borrow_mut`; a message that arrives while it is borrowed (one our own Win32 call sent
//! synchronously, such as `WM_WINDOWPOSCHANGING` from `SetWindowPos` or `WM_CAPTURECHANGED` from
//! `ReleaseCapture`) goes to `DefWindowProc`. WinEvent callbacks never borrow the state: they
//! only (re)arm debounce timers and re-assert the z-order through handles kept in a `Cell`.
//!
//! Locks: the mailbox lock is held for a swap, the UIA data lock for a copy — never across a
//! Win32 call (ARCHITECTURE "Messages between threads").

use std::cell::{Cell, RefCell};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use lastdrop_core::consts::HOVER_LEAVE_GRACE;
use lastdrop_core::readout::{
    ContentForm, ReadoutForm, ReadoutLayout, ReadoutModel, WidthInputs, compute_layout,
};
use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::Controls::{HOVER_DEFAULT, WM_MOUSEHOVER, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SetCapture, TME_HOVER, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
    DispatchMessageW, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_REORDER, EVENT_SYSTEM_FOREGROUND,
    GetDesktopWindow, GetMessageW, GetWindowThreadProcessId, HWND_TOPMOST, IDC_ARROW, IsChild,
    IsWindow, KillTimer, LoadCursorW, MA_NOACTIVATE, MSG, MSGFLT_ALLOW, OBJID_WINDOW,
    PBT_APMRESUMEAUTOMATIC, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SPI_SETHIGHCONTRAST, SPI_SETWORKAREA, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SetTimer, SetWindowPos, ShowWindow,
    TranslateMessage, UnregisterClassW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_APP,
    WM_CAPTURECHANGED, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_DWMCOLORIZATIONCOLORCHANGED,
    WM_GETOBJECT, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_POWERBROADCAST, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE,
    WM_THEMECHANGED, WM_TIMECHANGE, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use super::frame::{self, FrameInput, Plate};
use super::layered::Surface;
use super::uia::{self, UiaData, UiaRow};
use super::{EventSink, ReadoutConfig, ReadoutEvent, SystemEvent, gather};
use crate::flyout_pos::MonitorInfo;
use crate::geom::{Point, Rect};
use crate::occupancy::{OccupancyHandle, OccupancyResult};
use crate::placement::{
    AppButtonExtents, HiddenReason, Placement, PlacementPrefs, TaskbarAlignment, TaskbarEdge, place,
};
use crate::render::{Canvas, RenderFactory, TextMeasurer};
use crate::theme::{self, ThemeInfo};

/// The main thread's mailbox has something (model, theme, enabled).
pub(super) const WM_MAILBOX: u32 = WM_APP + 1;
/// Destroy the windows and end the thread.
pub(super) const WM_STOP: u32 = WM_APP + 2;
/// An occupancy answer: `wParam` 1 = measured (`lParam` = rightmost right, `isize::MIN` = no
/// buttons), 0 = unknown.
const WM_OCCUPANCY: u32 = WM_APP + 3;
/// UI Automation `Invoke` on the root.
const WM_INVOKED: u32 = WM_APP + 4;

/// Timers of the host window.
const T_PLACE: usize = 1;
const T_MEASURE: usize = 2;
const T_FIRST_MEASURE: usize = 3;
const T_FIND_TASKBAR: usize = 4;
const T_LEAVE: usize = 5;

/// Debounce after taskbar location changes (**not from the spec**): the slide of an auto-hide
/// taskbar and a re-flowing task list send bursts of events; place once they settle.
const PLACE_DEBOUNCE_MS: u32 = 40;
/// After z-order and foreground changes: re-check full screen and re-assert topmost quickly.
const ZORDER_DEBOUNCE_MS: u32 = 10;
/// Debounce of app-button measurements (**not from the spec**).
const MEASURE_DEBOUNCE_MS: u32 = 120;
/// Retry after one unknown measurement before floating (**not from the spec**).
const UNKNOWN_RETRY_MS: u32 = 500;
/// How long the readout stays hidden waiting for the first measurement after start-up or an
/// Explorer restart before it floats (**not from the spec**; a cold UI Automation connection to
/// Explorer took 1.4 s on the development machine).
const FIRST_MEASURE_WAIT_MS: u32 = 3000;
/// While there is no taskbar (Explorer not running), look for it again this often, in case
/// `TaskbarCreated` is missed (**not from the spec**).
const FIND_TASKBAR_RETRY_MS: u32 = 2000;

const HOST_CLASS: PCWSTR = w!("LastdropReadoutHost");
const READOUT_CLASS: PCWSTR = w!("LastdropReadout");

/// What the main thread hands over (last value wins).
#[derive(Default)]
pub(super) struct Slot {
    pub model: Option<Arc<ReadoutModel>>,
    pub theme: Option<ThemeInfo>,
    pub enabled: Option<bool>,
}

/// The one-slot mailbox between the main thread and the readout thread.
#[derive(Default)]
pub(super) struct Mailbox {
    slot: Mutex<Slot>,
}

impl Mailbox {
    fn lock(&self) -> MutexGuard<'_, Slot> {
        // Plain values: a panic elsewhere cannot leave them inconsistent.
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Changes the slot (the lock is held only for `f`).
    pub(super) fn put(&self, f: impl FnOnce(&mut Slot)) {
        f(&mut self.lock());
    }

    fn take(&self) -> Slot {
        std::mem::take(&mut *self.lock())
    }
}

/// Window handles the WinEvent callbacks need without borrowing the state.
#[derive(Clone, Copy, Default)]
struct Handles {
    host: isize,
    readout: isize,
    taskbar: isize,
    visible: bool,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    static HANDLES: Cell<Handles> = const {
        Cell::new(Handles { host: 0, readout: 0, taskbar: 0, visible: false })
    };
    static PROVIDER: RefCell<Option<uia::Provider>> = const { RefCell::new(None) };
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

fn raw(h: HWND) -> isize {
    h.0 as isize
}

fn update_handles(f: impl FnOnce(&mut Handles)) {
    HANDLES.with(|cell| {
        let mut h = cell.get();
        f(&mut h);
        cell.set(h);
    });
}

/// Runs `f` on the state unless it is already borrowed (a re-entrant message) or gone.
fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

/// (Re)arms a one-shot debounce timer on the host window.
fn arm(host: isize, id: usize, ms: u32) {
    // SAFETY: a timer on our own window; re-arming replaces it.
    unsafe { SetTimer(Some(hwnd(host)), id, ms, None) };
}

fn disarm(host: isize, id: usize) {
    // SAFETY: as above; an unset timer only makes the call fail.
    let _ = unsafe { KillTimer(Some(hwnd(host)), id) };
}

/// Puts the readout back on top of the topmost band (R§6.1 "Z-order").
fn reassert_topmost(handles: Handles) {
    if !handles.visible || handles.readout == 0 {
        return;
    }
    // SAFETY: our own window; no activation, no move, no resize.
    let _ = unsafe {
        SetWindowPos(
            hwnd(handles.readout),
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        )
    };
}

/// The thread body (ARCHITECTURE "Threads": readout).
pub(super) fn run(
    config: ReadoutConfig,
    sink: EventSink,
    mailbox: Arc<Mailbox>,
    ready: SyncSender<Result<isize, String>>,
) {
    // SAFETY: COM for this thread: an STA for Direct2D/WIC and the UIA provider.
    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if com.is_err() {
        let _ = ready.send(Err(format!("COM initialization failed: {com:?}")));
        return;
    }
    // SAFETY: only this thread's windows become per-monitor aware (v2): all geometry is in
    // device pixels, as the taskbar's.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    match Host::create(config, sink, mailbox) {
        Ok((host, provider)) => {
            let host_raw = raw(host.host_hwnd);
            let readout_raw = raw(host.readout_hwnd);
            update_handles(|h| {
                h.host = host_raw;
                h.readout = readout_raw;
            });
            PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
            HOST.with(|h| *h.borrow_mut() = Some(host));
            let _ = ready.send(Ok(host_raw));
        }
        Err(e) => {
            let _ = ready.send(Err(format!("readout windows: {e}")));
            // SAFETY: balances CoInitializeEx; nothing COM is alive on this thread.
            unsafe { CoUninitialize() };
            return;
        }
    }
    with_host(Host::start);

    let mut msg = MSG::default();
    loop {
        // SAFETY: the standard message loop; `msg` is a valid local.
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if got.0 <= 0 {
            break;
        }
        // SAFETY: as above.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    let host = HOST.with(|h| h.borrow_mut().take());
    let provider = PROVIDER.with(|p| p.borrow_mut().take());
    if let Some(host) = host {
        host.teardown(provider.as_ref());
    }
    drop(provider);
    update_handles(|h| *h = Handles::default());
    // SAFETY: balances CoInitializeEx; every COM object of this thread has been released above.
    unsafe { CoUninitialize() };
}

/// The pointer over the readout (R§10).
#[derive(Clone, Copy, Debug, Default)]
struct Pointer {
    inside: bool,
    pressed: bool,
    tracking: bool,
    /// `Hover` was sent and `Leave` not yet.
    hover_sent: bool,
    /// The second press of a double click: its button-up is not a click (R§10 "treated as one
    /// click").
    suppress_up: bool,
}

/// Where the readout is drawn now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Shown {
    rect: Rect,
    form: ReadoutForm,
    floating: bool,
}

/// What a frame's pixels depend on (R§12 "skip identical frames").
#[derive(Clone)]
struct Content {
    model: Arc<ReadoutModel>,
    layout: ReadoutLayout,
    form: ReadoutForm,
    floating: bool,
    plate: Plate,
    theme: ThemeInfo,
}

impl Content {
    fn same(&self, other: &Content) -> bool {
        (Arc::ptr_eq(&self.model, &other.model) || *self.model == *other.model)
            && self.layout == other.layout
            && self.form == other.form
            && self.floating == other.floating
            && self.plate == other.plate
            && self.theme == other.theme
    }
}

/// The last `PlacementChanged` sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Report {
    placement: Placement,
    form: ReadoutForm,
    monitor: MonitorInfo,
    taskbar: Rect,
    edge: TaskbarEdge,
}

/// The readout thread's state.
struct Host {
    config: ReadoutConfig,
    sink: EventSink,
    mailbox: Arc<Mailbox>,
    hinstance: HINSTANCE,
    host_hwnd: HWND,
    readout_hwnd: HWND,
    taskbar_created: u32,
    factory: RenderFactory,
    measurer: TextMeasurer,
    canvas: Option<Canvas>,
    surface: Option<Surface>,
    occupancy: Option<OccupancyHandle>,
    hooks: Vec<HWINEVENTHOOK>,
    taskbar: Option<HWND>,
    alignment: TaskbarAlignment,
    model: Option<Arc<ReadoutModel>>,
    theme: ThemeInfo,
    enabled: bool,
    /// The last measured app-button extents; `None` = unknown (floating).
    buttons: Option<AppButtonExtents>,
    /// A measurement answered since the taskbar was found.
    buttons_answered: bool,
    /// The first-measurement wait is over: unknown extents float instead of hiding.
    buttons_wait_over: bool,
    unknown_streak: u32,
    layout: Option<(WidthInputs, u64, ReadoutLayout)>,
    shown: Option<Shown>,
    visible: bool,
    last_content: Option<Content>,
    last_rect: Option<Rect>,
    last_uia: Option<UiaData>,
    last_report: Option<Report>,
    pointer: Pointer,
    announce_pending: bool,
}

/// Registers a window class; an existing registration (a previous readout thread) is fine.
fn register_class(
    hinstance: HINSTANCE,
    name: PCWSTR,
    proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    double_clicks: bool,
) -> windows::core::Result<()> {
    // SAFETY: a system cursor.
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class = WNDCLASSEXW {
        cbSize: u32::try_from(size_of::<WNDCLASSEXW>()).unwrap_or(80),
        style: if double_clicks {
            CS_DBLCLKS
        } else {
            Default::default()
        },
        lpfnWndProc: Some(proc_),
        hInstance: hinstance,
        hCursor: cursor,
        lpszClassName: name,
        ..Default::default()
    };
    // SAFETY: `class` is valid for the call; the name is a static string.
    if unsafe { RegisterClassExW(&class) } == 0 {
        // SAFETY: plain query.
        let error = unsafe { GetLastError() };
        if error != ERROR_CLASS_ALREADY_EXISTS {
            return Err(error.into());
        }
    }
    Ok(())
}

/// Posts an occupancy answer to the host window (runs on the occupancy thread).
fn post_occupancy(host: isize, result: OccupancyResult) {
    let (wparam, lparam) = match result {
        OccupancyResult::Measured(e) => (1, e.rightmost_right.map_or(isize::MIN, |x| x as isize)),
        OccupancyResult::Unknown => (0, 0),
    };
    // SAFETY: posting to the readout thread's window; a stale handle only makes the call fail.
    let _ = unsafe {
        PostMessageW(
            Some(hwnd(host)),
            WM_OCCUPANCY,
            WPARAM(wparam),
            LPARAM(lparam),
        )
    };
}

fn decode_occupancy(wparam: WPARAM, lparam: LPARAM) -> OccupancyResult {
    if wparam.0 == 0 {
        return OccupancyResult::Unknown;
    }
    let rightmost_right = if lparam.0 == isize::MIN {
        None
    } else {
        i32::try_from(lparam.0).ok()
    };
    OccupancyResult::Measured(AppButtonExtents { rightmost_right })
}

impl Host {
    fn create(
        config: ReadoutConfig,
        sink: EventSink,
        mailbox: Arc<Mailbox>,
    ) -> windows::core::Result<(Host, uia::Provider)> {
        // SAFETY: the module handle of this executable.
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None)? }.into();
        register_class(hinstance, HOST_CLASS, host_proc, false)?;
        register_class(hinstance, READOUT_CLASS, readout_proc, true)?;
        // A normal top-level window (never HWND_MESSAGE, which gets no broadcasts), never shown.
        // SAFETY: registered class; no parent; no create parameter.
        let host_hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                HOST_CLASS,
                w!("Lastdrop readout host"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance),
                None,
            )?
        };
        // SAFETY: as above. R§6.1: WS_POPUP, layered, tool window, topmost, no activation.
        let readout_hwnd = match unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                READOUT_CLASS,
                w!("Lastdrop"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(hinstance),
                None,
            )
        } {
            Ok(h) => h,
            Err(e) => {
                // SAFETY: the window created above, on this thread.
                let _ = unsafe { DestroyWindow(host_hwnd) };
                return Err(e);
            }
        };
        // SAFETY: a plain registration.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created != 0 {
            // An elevated Lastdrop would otherwise never hear Explorer's broadcast (UIPI).
            // SAFETY: our own window.
            let _ = unsafe {
                ChangeWindowMessageFilterEx(host_hwnd, taskbar_created, MSGFLT_ALLOW, None)
            };
        }
        let factory = RenderFactory::new()?;
        let measurer = TextMeasurer::new(&factory);
        let notify = raw(host_hwnd);
        let occupancy = match OccupancyHandle::spawn(Box::new(move |r| post_occupancy(notify, r))) {
            Ok(o) => Some(o),
            Err(e) => {
                log::warn!("occupancy thread not started ({e}); the readout will float");
                None
            }
        };
        let provider = uia::Provider::new(readout_hwnd, host_hwnd, WM_INVOKED);
        let enabled = config.enabled;
        let theme = config.theme;
        Ok((
            Host {
                config,
                sink,
                mailbox,
                hinstance,
                host_hwnd,
                readout_hwnd,
                taskbar_created,
                factory,
                measurer,
                canvas: None,
                surface: None,
                occupancy,
                hooks: Vec::new(),
                taskbar: None,
                alignment: TaskbarAlignment::Center,
                model: None,
                theme,
                enabled,
                buttons: None,
                buttons_answered: false,
                buttons_wait_over: false,
                unknown_streak: 0,
                layout: None,
                shown: None,
                visible: false,
                last_content: None,
                last_rect: None,
                last_uia: None,
                last_report: None,
                pointer: Pointer::default(),
                announce_pending: false,
            },
            provider,
        ))
    }

    fn emit(&self, event: ReadoutEvent) {
        (self.sink)(event);
    }

    fn start(&mut self) {
        self.alignment = gather::read_alignment();
        self.attach_taskbar();
        self.place_now();
    }

    // ---- taskbar, hooks, occupancy -------------------------------------------------------------

    /// Finds `Shell_TrayWnd`; while the readout is enabled, hooks its process and asks for a first
    /// measurement. A readout that is off needs neither (ARCHITECTURE goal 2: no system-wide
    /// hook and no UI Automation traffic to Explorer for nothing); the taskbar's geometry is still
    /// gathered on each placement, for the reports the hover card and flyout are placed with.
    fn attach_taskbar(&mut self) {
        self.unhook();
        self.taskbar = gather::find_taskbar();
        let taskbar_raw = self.taskbar.map_or(0, raw);
        update_handles(|h| h.taskbar = taskbar_raw);
        self.buttons = None;
        self.buttons_answered = false;
        self.buttons_wait_over = self.occupancy.is_none();
        self.unknown_streak = 0;
        let host = raw(self.host_hwnd);
        let Some(taskbar) = self.taskbar else {
            arm(host, T_FIND_TASKBAR, FIND_TASKBAR_RETRY_MS);
            return;
        };
        disarm(host, T_FIND_TASKBAR);
        if !self.enabled {
            return;
        }
        let mut pid = 0u32;
        // SAFETY: `pid` is a valid out pointer.
        unsafe { GetWindowThreadProcessId(taskbar, Some(&mut pid)) };
        self.hook(pid);
        self.request_measure();
        arm(host, T_FIRST_MEASURE, FIRST_MEASURE_WAIT_MS);
    }

    /// Out-of-context hooks: location and z-order changes in Explorer's process, foreground
    /// changes everywhere (re-assert topmost, notice full-screen apps). Never polling.
    fn hook(&mut self, explorer_pid: u32) {
        let flags = WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS;
        let specs = [
            (EVENT_OBJECT_LOCATIONCHANGE, explorer_pid),
            (EVENT_OBJECT_REORDER, explorer_pid),
            (EVENT_SYSTEM_FOREGROUND, 0),
        ];
        for (event, pid) in specs {
            // SAFETY: an out-of-context hook delivered to this thread's message loop; the
            // callback is a plain function and is unhooked before the thread ends.
            let hook =
                unsafe { SetWinEventHook(event, event, None, Some(on_win_event), pid, 0, flags) };
            if hook.is_invalid() {
                log::warn!("WinEvent hook {event:#x} not installed");
            } else {
                self.hooks.push(hook);
            }
        }
    }

    fn unhook(&mut self) {
        for hook in self.hooks.drain(..) {
            // SAFETY: a hook installed by this thread.
            let _ = unsafe { UnhookWinEvent(hook) };
        }
    }

    fn request_measure(&self) {
        if !self.enabled {
            return;
        }
        if let (Some(occupancy), Some(taskbar)) = (&self.occupancy, self.taskbar) {
            occupancy.request(raw(taskbar));
        }
    }

    fn on_occupancy(&mut self, result: OccupancyResult) {
        log::debug!("readout: app buttons {result:?}");
        match result {
            OccupancyResult::Measured(extents) => {
                self.unknown_streak = 0;
                self.buttons = Some(extents);
                self.buttons_answered = true;
            }
            OccupancyResult::Unknown => {
                self.unknown_streak += 1;
                if self.unknown_streak == 1 {
                    // One more try before floating: Explorer may just be busy.
                    arm(raw(self.host_hwnd), T_MEASURE, UNKNOWN_RETRY_MS);
                    return;
                }
                self.buttons = None;
                self.buttons_answered = true;
            }
        }
        self.place_now();
    }

    /// Explorer restarted (`TaskbarCreated`): rebuild everything that belonged to the old
    /// taskbar, in-process.
    fn rebuild(&mut self) {
        log::info!("taskbar re-created: rebuilding the readout");
        self.hide();
        self.last_content = None;
        self.last_report = None;
        self.alignment = gather::read_alignment();
        self.attach_taskbar();
        self.place_now();
    }

    // ---- placement and drawing -----------------------------------------------------------------

    fn form(&self) -> ReadoutForm {
        self.shown.map_or(ReadoutForm::Hidden, |s| s.form)
    }

    fn plate(&self) -> Plate {
        if self.pointer.pressed {
            Plate::Pressed
        } else if self.pointer.inside {
            Plate::Hover
        } else {
            Plate::None
        }
    }

    /// The layout of the current model at `scale` (R§6.3), recomputed only when the width inputs
    /// or the scale change.
    fn layout_at(&mut self, scale: f64) -> Option<ReadoutLayout> {
        let model = self.model.as_ref()?;
        if let Some((widths, bits, layout)) = &self.layout
            && *bits == scale.to_bits()
            && *widths == model.widths
        {
            return Some(*layout);
        }
        let layout = compute_layout(&model.widths, scale, &self.measurer);
        self.layout = Some((model.widths.clone(), scale.to_bits(), layout));
        Some(layout)
    }

    /// Gathers the geometry, places, draws and reports (R§4.2, R§6.1).
    fn place_now(&mut self) {
        log::trace!("readout: place");
        let host = raw(self.host_hwnd);
        // SAFETY: IsWindow accepts any value.
        let taskbar = self
            .taskbar
            .filter(|t| unsafe { IsWindow(Some(*t)) }.as_bool());
        let Some(taskbar) = taskbar else {
            if self.taskbar.is_some() {
                // Explorer went away; wait for TaskbarCreated, and look again now and then.
                self.taskbar = None;
                update_handles(|h| h.taskbar = 0);
                self.unhook();
            }
            arm(host, T_FIND_TASKBAR, FIND_TASKBAR_RETRY_MS);
            self.shown = None;
            self.hide();
            return;
        };
        let Some(geometry) = gather::gather(taskbar, self.alignment, self.buttons) else {
            self.shown = None;
            self.hide();
            return;
        };
        let scale = if geometry.dpi == 0 {
            1.0
        } else {
            f64::from(geometry.dpi) / 96.0
        };
        let content = match &self.model {
            Some(m) if self.enabled => m.content,
            _ => ContentForm::Hidden,
        };
        let layout = self.layout_at(scale);
        let waiting = !self.buttons_answered && !self.buttons_wait_over;
        let placement = match layout {
            Some(_) if waiting && content != ContentForm::Hidden => {
                // The first measurement is on its way: show nothing rather than guess.
                self.shown = None;
                self.hide();
                return;
            }
            Some(l) => place(
                &geometry,
                &l,
                &PlacementPrefs {
                    content,
                    previous: self.form(),
                },
            ),
            None => Placement::Hidden(HiddenReason::Off),
        };
        let (form, shown) = match placement {
            Placement::Docked { rect, form } => (
                form,
                Some(Shown {
                    rect,
                    form,
                    floating: false,
                }),
            ),
            Placement::Floating { rect, form } => (
                form,
                Some(Shown {
                    rect,
                    form,
                    floating: true,
                }),
            ),
            Placement::Hidden(_) => (ReadoutForm::Hidden, None),
        };
        self.shown = shown;
        self.present();
        self.report(Report {
            placement,
            form,
            monitor: MonitorInfo {
                monitor: geometry.monitor,
                work: geometry.work_area,
                dpi: geometry.dpi,
            },
            taskbar: geometry.taskbar,
            edge: geometry.edge,
        });
    }

    fn report(&mut self, report: Report) {
        if self.last_report == Some(report) {
            return;
        }
        log::debug!(
            "readout: {:?} on a {:?} taskbar",
            report.placement,
            report.edge
        );
        self.last_report = Some(report);
        self.emit(ReadoutEvent::PlacementChanged {
            placement: report.placement,
            form: report.form,
            monitor: report.monitor,
            taskbar: report.taskbar,
            edge: report.edge,
        });
    }

    /// Draws (when the content changed), pushes and shows the readout at `self.shown`, or hides
    /// it.
    fn present(&mut self) {
        let (Some(shown), Some(model), Some((_, _, layout))) =
            (self.shown, self.model.clone(), self.layout.clone())
        else {
            self.hide();
            return;
        };
        let content = Content {
            model: Arc::clone(&model),
            layout,
            form: shown.form,
            floating: shown.floating,
            plate: self.plate(),
            theme: self.theme,
        };
        let redraw = self
            .last_content
            .as_ref()
            .is_none_or(|last| !last.same(&content));
        if redraw {
            if let Err(e) = self.render(&content) {
                log::warn!("readout frame not drawn: {e}");
                self.hide();
                return;
            }
            self.last_content = Some(content);
        }
        // Raised to the top of the topmost band only when it appears or moves; otherwise only
        // the taskbar's own events re-raise it (R§6.1: "when the event concerns
        // Shell_TrayWnd; never by polling"), so a redraw never jumps over other always-on-top
        // windows or our own hover card and flyout.
        let raise = self.last_rect != Some(shown.rect) || !self.visible;
        if redraw || self.last_rect != Some(shown.rect) || !self.visible {
            let pushed = match &self.surface {
                Some(surface) => {
                    surface.present(self.readout_hwnd, (shown.rect.left, shown.rect.top))
                }
                None => Err(windows::Win32::Foundation::E_UNEXPECTED.into()),
            };
            if let Err(e) = pushed {
                log::warn!("readout not pushed to the screen: {e}");
                self.hide();
                return;
            }
            self.last_rect = Some(shown.rect);
        }
        if !self.visible {
            // SAFETY: our own window; shown without activation (R§6.1).
            let _ = unsafe { ShowWindow(self.readout_hwnd, SW_SHOWNOACTIVATE) };
            self.visible = true;
            update_handles(|h| h.visible = true);
        }
        if raise {
            reassert_topmost(HANDLES.with(Cell::get));
        }
        self.update_uia(&model, &layout, shown);
    }

    /// Renders the frame into the canvas and copies it into the surface.
    fn render(&mut self, content: &Content) -> windows::core::Result<()> {
        let (w, h) = frame::slot_size(&content.layout, content.form);
        let (w, h) = (w.unsigned_abs(), h.unsigned_abs());
        let canvas = match self.canvas.take() {
            Some(c) if c.width() == w && c.height() == h => c,
            _ => Canvas::new(&self.factory, w, h)?,
        };
        let canvas = self.canvas.insert(canvas);
        frame::draw(
            canvas,
            &self.measurer,
            &FrameInput {
                model: &content.model,
                layout: &content.layout,
                form: content.form,
                theme: &content.theme,
                plate: content.plate,
                floating: content.floating,
            },
        )?;
        let pixels = canvas.pixels()?;
        log::debug!("readout: frame {w}×{h} {:?}", content.form);
        let surface = match self.surface.take() {
            Some(s) if s.size() == (w, h) => s,
            _ => Surface::new(w, h)?,
        };
        self.surface.insert(surface).write(&pixels);
        Ok(())
    }

    fn hide(&mut self) {
        if self.visible {
            // SAFETY: our own window.
            let _ = unsafe { ShowWindow(self.readout_hwnd, SW_HIDE) };
            self.visible = false;
            update_handles(|h| h.visible = false);
        }
        self.last_rect = None;
        self.pointer.inside = false;
        self.pointer.pressed = false;
        if self.pointer.hover_sent {
            self.pointer.hover_sent = false;
            disarm(raw(self.host_hwnd), T_LEAVE);
            self.emit(ReadoutEvent::Leave);
        }
        if self
            .last_uia
            .as_ref()
            .is_some_and(|d| d.slot != Rect::default())
        {
            self.set_uia(UiaData::default());
        }
    }

    fn update_uia(&mut self, model: &ReadoutModel, layout: &ReadoutLayout, shown: Shown) {
        let rows = frame::rows_for(model, layout, shown.form)
            .into_iter()
            .map(|(row, y, _)| UiaRow {
                name: row.uia_name.clone(),
                value: row.uia_value,
                rect: Rect {
                    left: shown.rect.left,
                    top: shown.rect.top + y,
                    right: shown.rect.right,
                    bottom: shown.rect.top + y + layout.row_h,
                },
            })
            .collect();
        let name = if shown.form == ReadoutForm::Compact {
            model.uia.root_name_compact.clone()
        } else {
            model.uia.root_name.clone()
        };
        let data = UiaData {
            name,
            help: model.uia.help_text.clone(),
            rows,
            slot: shown.rect,
        };
        if self.last_uia.as_ref() != Some(&data) {
            self.set_uia(data);
        }
        if std::mem::take(&mut self.announce_pending) {
            PROVIDER.with(|p| {
                if let Ok(p) = p.try_borrow()
                    && let Some(p) = p.as_ref()
                {
                    p.announce();
                }
            });
        }
    }

    fn set_uia(&mut self, data: UiaData) {
        PROVIDER.with(|p| {
            if let Ok(p) = p.try_borrow()
                && let Some(p) = p.as_ref()
            {
                p.shared().set(data.clone());
            }
        });
        self.last_uia = Some(data);
    }

    // ---- messages --------------------------------------------------------------------------

    fn drain_mailbox(&mut self) {
        let slot = self.mailbox.take();
        if let Some(theme) = slot.theme {
            self.theme = theme;
        }
        if let Some(enabled) = slot.enabled
            && enabled != self.enabled
        {
            self.enabled = enabled;
            if enabled {
                // Hooks and a fresh measurement for the taskbar as it is now.
                self.attach_taskbar();
            } else {
                self.unhook();
                let host = raw(self.host_hwnd);
                disarm(host, T_MEASURE);
                disarm(host, T_FIRST_MEASURE);
            }
        }
        if let Some(model) = slot.model {
            if model.uia.announce {
                self.announce_pending = true;
            }
            self.model = Some(model);
        }
        // Cheap: three GetWindowRect calls; the frame is redrawn only if its content changed.
        // Also re-checks full-screen apps once a minute with the minute tick's model.
        self.place_now();
    }

    fn on_timer(&mut self, id: usize) {
        let host = raw(self.host_hwnd);
        match id {
            T_PLACE => {
                disarm(host, T_PLACE);
                self.place_now();
            }
            T_MEASURE => {
                disarm(host, T_MEASURE);
                self.request_measure();
            }
            T_FIRST_MEASURE => {
                disarm(host, T_FIRST_MEASURE);
                self.buttons_wait_over = true;
                self.place_now();
            }
            T_FIND_TASKBAR => {
                if gather::find_taskbar().is_some() {
                    self.rebuild();
                }
            }
            T_LEAVE => {
                disarm(host, T_LEAVE);
                if !self.pointer.inside && self.pointer.hover_sent {
                    self.pointer.hover_sent = false;
                    self.emit(ReadoutEvent::Leave);
                }
            }
            _ => {}
        }
    }

    fn on_setting_change(&mut self, wparam: WPARAM, lparam: LPARAM) {
        let area = if lparam.0 == 0 {
            String::new()
        } else {
            // SAFETY: WM_SETTINGCHANGE's lParam, when set, is a NUL-terminated string that lives
            // for the duration of the message.
            unsafe { PCWSTR(lparam.0 as *const u16).to_string() }.unwrap_or_default()
        };
        if area == "intl" {
            self.emit(ReadoutEvent::System(SystemEvent::Locale));
        } else if (area == "ImmersiveColorSet" || wparam.0 == SPI_SETHIGHCONTRAST.0 as usize)
            && let Some(change) = theme::classify(WM_SETTINGCHANGE, wparam, lparam)
        {
            // Only the two theme-bearing kinds of WM_SETTINGCHANGE (theme::ThemeChange) are
            // classified; the many other broadcasts ("Environment", "Policy", …) are not.
            self.emit(ReadoutEvent::System(SystemEvent::Theme(change)));
        }
        if area == "TraySettings" || wparam.0 == SPI_SETWORKAREA.0 as usize {
            // Taskbar alignment or size, or the work area (auto-hide on/off).
            self.alignment = gather::read_alignment();
            let host = raw(self.host_hwnd);
            arm(host, T_PLACE, PLACE_DEBOUNCE_MS);
            arm(host, T_MEASURE, MEASURE_DEBOUNCE_MS);
        }
    }

    fn on_host(&mut self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let host = raw(self.host_hwnd);
        match msg {
            WM_MAILBOX => self.drain_mailbox(),
            WM_STOP => {
                // SAFETY: ends this thread's message loop; teardown follows outside the borrow.
                unsafe { PostQuitMessage(0) };
            }
            WM_OCCUPANCY => self.on_occupancy(decode_occupancy(wparam, lparam)),
            WM_INVOKED => self.emit(ReadoutEvent::Invoked),
            WM_TIMER => self.on_timer(wparam.0),
            WM_DISPLAYCHANGE | WM_DPICHANGED => {
                arm(host, T_PLACE, PLACE_DEBOUNCE_MS);
                arm(host, T_MEASURE, MEASURE_DEBOUNCE_MS);
                return None;
            }
            WM_SETTINGCHANGE => {
                self.on_setting_change(wparam, lparam);
                return None;
            }
            WM_THEMECHANGED | WM_SYSCOLORCHANGE | WM_DWMCOLORIZATIONCOLORCHANGED => {
                if let Some(change) = theme::classify(msg, wparam, lparam) {
                    self.emit(ReadoutEvent::System(SystemEvent::Theme(change)));
                }
                return None;
            }
            WM_TIMECHANGE => {
                self.emit(ReadoutEvent::System(SystemEvent::TimeChanged));
                return None;
            }
            WM_POWERBROADCAST => {
                if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize {
                    self.emit(ReadoutEvent::System(SystemEvent::Resumed));
                    arm(host, T_PLACE, PLACE_DEBOUNCE_MS);
                    arm(host, T_MEASURE, MEASURE_DEBOUNCE_MS);
                }
                return Some(LRESULT(1));
            }
            m if m != 0 && m == self.taskbar_created => self.rebuild(),
            _ => return None,
        }
        Some(LRESULT(0))
    }

    // ---- pointer (R§10) --------------------------------------------------------------------

    fn hover_time(&self) -> u32 {
        self.config.hover_delay.map_or(HOVER_DEFAULT, |d| {
            u32::try_from(d.as_millis()).unwrap_or(u32::MAX).max(1)
        })
    }

    fn pointer_move(&mut self) {
        disarm(raw(self.host_hwnd), T_LEAVE);
        if !self.pointer.tracking {
            let mut flags = TME_LEAVE;
            if !self.pointer.hover_sent {
                flags |= TME_HOVER;
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: u32::try_from(size_of::<TRACKMOUSEEVENT>()).unwrap_or(24),
                dwFlags: flags,
                hwndTrack: self.readout_hwnd,
                dwHoverTime: self.hover_time(),
            };
            // SAFETY: `track` is valid for the call and names our own window.
            if unsafe { TrackMouseEvent(&mut track) }.is_ok() {
                self.pointer.tracking = true;
            }
        }
        if !self.pointer.inside {
            self.pointer.inside = true;
            self.present();
        }
    }

    fn pointer_hover(&mut self) {
        if self.pointer.hover_sent {
            return;
        }
        if let Some(shown) = self.shown {
            self.pointer.hover_sent = true;
            self.emit(ReadoutEvent::Hover { rect: shown.rect });
        }
    }

    fn pointer_leave(&mut self) {
        self.pointer.tracking = false;
        self.pointer.inside = false;
        if !self.pointer.pressed {
            self.present();
        }
        if self.pointer.hover_sent {
            let grace = u32::try_from(HOVER_LEAVE_GRACE.as_millis()).unwrap_or(100);
            arm(raw(self.host_hwnd), T_LEAVE, grace);
        }
    }

    fn screen_point(&self, lparam: LPARAM) -> Point {
        // Client coordinates are signed 16-bit halves of lParam.
        let mut pt = POINT {
            x: i32::from(lparam.0 as u16 as i16),
            y: i32::from((lparam.0 >> 16) as u16 as i16),
        };
        // SAFETY: our own window; `pt` is a valid in/out pointer.
        let _ = unsafe { ClientToScreen(self.readout_hwnd, &mut pt) };
        Point { x: pt.x, y: pt.y }
    }

    fn on_readout(&mut self, msg: u32, _wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_MOUSEMOVE => self.pointer_move(),
            WM_MOUSEHOVER => self.pointer_hover(),
            WM_MOUSELEAVE => self.pointer_leave(),
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                self.pointer.pressed = true;
                self.pointer.suppress_up = msg == WM_LBUTTONDBLCLK;
                // SAFETY: our own window, on its thread.
                unsafe { SetCapture(self.readout_hwnd) };
                self.present();
            }
            WM_LBUTTONUP => {
                let was_pressed = std::mem::take(&mut self.pointer.pressed);
                let suppress = std::mem::take(&mut self.pointer.suppress_up);
                // SAFETY: a plain query.
                if unsafe { GetCapture() } == self.readout_hwnd {
                    // SAFETY: releases our own capture, on its thread.
                    let _ = unsafe { ReleaseCapture() };
                }
                let pt = self.screen_point(lparam);
                let inside = self.shown.is_some_and(|s| s.rect.contains(pt));
                if was_pressed && !suppress && inside {
                    self.emit(ReadoutEvent::Click);
                }
                self.present();
            }
            WM_RBUTTONDOWN => {}
            WM_RBUTTONUP => {
                let pt = self.screen_point(lparam);
                self.emit(ReadoutEvent::RightClick { pt });
            }
            WM_CAPTURECHANGED => {
                if std::mem::take(&mut self.pointer.pressed) {
                    self.present();
                }
            }
            WM_DPICHANGED => {
                // Placement decides the size from the taskbar's monitor, not the suggestion.
                arm(raw(self.host_hwnd), T_PLACE, PLACE_DEBOUNCE_MS);
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    // ---- teardown ----------------------------------------------------------------------------

    fn teardown(mut self, provider: Option<&uia::Provider>) {
        self.unhook();
        let host = raw(self.host_hwnd);
        for id in [T_PLACE, T_MEASURE, T_FIRST_MEASURE, T_FIND_TASKBAR, T_LEAVE] {
            disarm(host, id);
        }
        if let Some(provider) = provider {
            provider.disconnect(self.readout_hwnd);
        }
        // SAFETY: our own windows, destroyed on their thread; the state is no longer reachable
        // from the window procedures (taken out of the thread-local).
        unsafe {
            let _ = DestroyWindow(self.readout_hwnd);
            let _ = DestroyWindow(self.host_hwnd);
        }
        if let Some(occupancy) = self.occupancy.take() {
            occupancy.shutdown();
        }
        self.canvas = None;
        self.surface = None;
        // SAFETY: our classes; they have no windows left.
        unsafe {
            let _ = UnregisterClassW(READOUT_CLASS, Some(self.hinstance));
            let _ = UnregisterClassW(HOST_CLASS, Some(self.hinstance));
        }
    }
}

/// The host window: system broadcasts, the mailbox, timers, occupancy answers.
unsafe extern "system" fn host_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if let Some(Some(result)) = with_host(|h| h.on_host(msg, wparam, lparam)) {
        return result;
    }
    // SAFETY: default processing for our own window.
    unsafe { DefWindowProcW(window, msg, wparam, lparam) }
}

/// The readout window: UIA, pointer input, DPI.
unsafe extern "system" fn readout_proc(
    window: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_GETOBJECT {
        let answered = PROVIDER.with(|p| {
            p.try_borrow()
                .ok()
                .and_then(|p| p.as_ref().and_then(|p| p.answer(window, wparam, lparam)))
        });
        if let Some(result) = answered {
            return LRESULT(result);
        }
    }
    if msg == WM_MOUSEACTIVATE {
        // Clicks never activate the readout (R§10 "not focusable").
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if let Some(Some(result)) = with_host(|h| h.on_readout(msg, wparam, lparam)) {
        return result;
    }
    // SAFETY: default processing for our own window.
    unsafe { DefWindowProcW(window, msg, wparam, lparam) }
}

/// Out-of-context WinEvent callback (runs in this thread's message loop). Borrows no state: it
/// arms debounce timers and re-asserts the z-order.
unsafe extern "system" fn on_win_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    window: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    let handles = HANDLES.with(Cell::get);
    if handles.host == 0 || handles.taskbar == 0 {
        return;
    }
    let taskbar = hwnd(handles.taskbar);
    // SAFETY: plain queries on window handles (any value is accepted).
    let in_taskbar = || window == taskbar || unsafe { IsChild(taskbar, window) }.as_bool();
    match event {
        EVENT_OBJECT_LOCATIONCHANGE => {
            // The taskbar moved or resized (auto-hide, DPI, edge), the tray grew or shrank, or
            // the task list re-flowed (app buttons came or went): re-place and re-measure.
            if id_object == OBJID_WINDOW.0 && !window.is_invalid() && in_taskbar() {
                arm(handles.host, T_PLACE, PLACE_DEBOUNCE_MS);
                arm(handles.host, T_MEASURE, MEASURE_DEBOUNCE_MS);
            }
        }
        EVENT_OBJECT_REORDER => {
            // Explorer reordered top-level windows (the taskbar raised itself) or the taskbar's
            // children: re-assert topmost now, then re-check full screen.
            // SAFETY: plain query.
            if in_taskbar() || window == unsafe { GetDesktopWindow() } {
                reassert_topmost(handles);
                arm(handles.host, T_PLACE, ZORDER_DEBOUNCE_MS);
            }
        }
        EVENT_SYSTEM_FOREGROUND => {
            if window == taskbar {
                // Clicking the taskbar raises it over the readout: put the readout back now.
                reassert_topmost(handles);
            }
            // A full-screen app may have come or gone.
            arm(handles.host, T_PLACE, ZORDER_DEBOUNCE_MS);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occupancy_answers_survive_the_message_round_trip() {
        for result in [
            OccupancyResult::Unknown,
            OccupancyResult::Measured(AppButtonExtents {
                rightmost_right: None,
            }),
            OccupancyResult::Measured(AppButtonExtents {
                rightmost_right: Some(3040),
            }),
            OccupancyResult::Measured(AppButtonExtents {
                rightmost_right: Some(-1200),
            }),
        ] {
            let (wparam, lparam) = match result {
                OccupancyResult::Measured(e) => {
                    (1, e.rightmost_right.map_or(isize::MIN, |x| x as isize))
                }
                OccupancyResult::Unknown => (0, 0),
            };
            assert_eq!(decode_occupancy(WPARAM(wparam), LPARAM(lparam)), result);
        }
    }

    #[test]
    fn the_mailbox_keeps_the_last_value() {
        let mailbox = Mailbox::default();
        mailbox.put(|s| s.enabled = Some(true));
        mailbox.put(|s| s.enabled = Some(false));
        let slot = mailbox.take();
        assert_eq!(slot.enabled, Some(false));
        assert!(slot.model.is_none());
        assert!(mailbox.take().enabled.is_none());
    }
}
