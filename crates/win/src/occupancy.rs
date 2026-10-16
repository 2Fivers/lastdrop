//! The occupancy thread: a UI Automation client (MTA) that measures where the taskbar's app
//! buttons are, with connection and transaction timeouts, so the readout never covers them
//! (ARCHITECTURE "Threads", ROADMAP "Open questions").
//!
//! Requested by the readout thread after taskbar location changes (debounced); answers through
//! its sink. It does nothing else. When UI Automation fails or times out, the answer is
//! [`OccupancyResult::Unknown`] and placement floats the readout instead of guessing.
//!
//! COM and the automation object are set up on the first request, not when the thread starts:
//! a readout that is off never asks, so UI Automation is never loaded into the process for it.
//!
//! What counts as an app button (recorded on Windows 11 25H2): every UIA `Button` under
//! `Shell_TrayWnd` — Start, Search, Task View, Widgets and the `Taskbar.TaskListButtonAutomationPeer`
//! app buttons — except the notification area's (`SystemTray.*` classes, or anything starting at or
//! right of `TrayNotifyWnd`'s left edge).
//!
//! A secondary taskbar (`Shell_SecondaryTrayWnd`, every-taskbar-spec §3) has no `TrayNotifyWnd`:
//! its right end holds the clock and Show desktop. There, the buttons right of the rightmost
//! `Taskbar.TaskListButton*` (or, without one, every button in the right half that is not an app
//! button) are that end, and their leftmost left edge is reported as `tray_left`, whatever their
//! classes are. One `FindAllBuildCache` with a cache request for
//! `BoundingRectangle`, `ControlType` and `ClassName` and `AutomationElementMode_None`, so a
//! measurement is one cross-process transaction and holds no element references.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    AutomationElementMode_None, CUIAutomation, CUIAutomation8, IUIAutomation, IUIAutomation2,
    IUIAutomationCacheRequest, IUIAutomationCondition, TreeScope_Descendants,
    UIA_BoundingRectanglePropertyId, UIA_ButtonControlTypeId, UIA_ClassNamePropertyId,
    UIA_ControlTypePropertyId,
};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowRect, IsWindow};
use windows::core::{Interface, w};

use crate::placement::AppButtonExtents;

/// `IUIAutomation2::put_ConnectionTimeout` (**not from the spec**): how long UI Automation waits
/// for Explorer to accept the connection before the measurement counts as unknown.
pub const CONNECTION_TIMEOUT: Duration = Duration::from_millis(500);

/// `IUIAutomation2::put_TransactionTimeout` (**not from the spec**): how long one cross-process
/// call may take.
pub const TRANSACTION_TIMEOUT: Duration = Duration::from_secs(1);

/// The answer to one measurement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OccupancyResult {
    /// The buttons' extents.
    Measured(AppButtonExtents),
    /// UI Automation failed or timed out.
    Unknown,
}

/// The request slot shared with the thread.
#[derive(Default)]
struct Queue {
    /// The taskbar to measure next; a newer request replaces an older one (coalescing).
    pending: Option<isize>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Queue> {
        // The queue holds plain values; a panic elsewhere cannot leave it inconsistent.
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Handle to the occupancy thread (owned by the readout thread).
pub struct OccupancyHandle {
    thread: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl OccupancyHandle {
    /// Starts the thread; COM (MTA) and `CUIAutomation8` with its timeouts follow on the first
    /// request. `sink` runs on the occupancy thread and must only post to the readout thread.
    pub fn spawn(
        sink: Box<dyn Fn(OccupancyResult) + Send + 'static>,
    ) -> std::io::Result<OccupancyHandle> {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("lastdrop-occupancy".to_owned())
            .spawn(move || run(&worker, &*sink))?;
        Ok(OccupancyHandle {
            thread: Some(thread),
            shared,
        })
    }

    /// Requests a measurement of the taskbar `taskbar_hwnd` (`HWND` as `isize`); requests that
    /// arrive while one runs are coalesced into one more.
    pub fn request(&self, taskbar_hwnd: isize) {
        let mut queue = self.shared.lock();
        queue.pending = Some(taskbar_hwnd);
        drop(queue);
        self.shared.wake.notify_one();
    }

    /// Stops and joins the thread.
    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::warn!("occupancy thread panicked");
        }
    }
}

impl Drop for OccupancyHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The thread body: measure on request until stopped; COM MTA and one [`Automation`] from the
/// first request on.
fn run(shared: &Shared, sink: &dyn Fn(OccupancyResult)) {
    // `None` until the first request; then whether COM is initialized on this thread.
    let mut com_ok: Option<bool> = None;
    let mut automation: Option<Automation> = None;
    loop {
        let hwnd = {
            let mut queue = shared.lock();
            loop {
                if queue.stop {
                    break None;
                }
                if let Some(h) = queue.pending.take() {
                    break Some(h);
                }
                queue = shared
                    .wake
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let Some(hwnd) = hwnd else {
            break;
        };
        if com_ok.is_none() {
            // SAFETY: initializes COM for this thread; balanced by CoUninitialize below on
            // success.
            let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if com.is_ok() {
                automation = match Automation::new() {
                    Ok(a) => Some(a),
                    Err(e) => {
                        log::warn!("UI Automation unavailable: {e}");
                        None
                    }
                };
            } else {
                log::warn!("COM initialization failed on the occupancy thread: {com:?}");
            }
            com_ok = Some(com.is_ok());
        }
        let result = match &automation {
            Some(a) => match measure(a, hwnd) {
                Ok(extents) => OccupancyResult::Measured(extents),
                Err(e) => {
                    log::debug!("taskbar measurement failed: {e}");
                    OccupancyResult::Unknown
                }
            },
            None => OccupancyResult::Unknown,
        };
        sink(result);
    }
    drop(automation);
    if com_ok == Some(true) {
        // SAFETY: balances the successful CoInitializeEx above; every COM object of this
        // thread has been released.
        unsafe { CoUninitialize() };
    }
}

/// The UI Automation objects a measurement uses, made once.
struct Automation {
    automation: IUIAutomation,
    /// `BoundingRectangle`, `ControlType` and `ClassName`, `AutomationElementMode_None`.
    cache: IUIAutomationCacheRequest,
    /// `ControlType == Button`.
    buttons: IUIAutomationCondition,
}

impl Automation {
    fn new() -> windows::core::Result<Automation> {
        let automation = create_automation()?;
        // SAFETY: plain calls on a live object; the VARIANT is a local valid for the call.
        unsafe {
            let cache = automation.CreateCacheRequest()?;
            cache.AddProperty(UIA_BoundingRectanglePropertyId)?;
            cache.AddProperty(UIA_ControlTypePropertyId)?;
            cache.AddProperty(UIA_ClassNamePropertyId)?;
            cache.SetAutomationElementMode(AutomationElementMode_None)?;
            let buttons = automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_ButtonControlTypeId.0),
            )?;
            Ok(Automation {
                automation,
                cache,
                buttons,
            })
        }
    }
}

/// `CUIAutomation8` with the connection and transaction timeouts; plain `CUIAutomation` (without
/// timeouts) only where the newer class is missing.
fn create_automation() -> windows::core::Result<IUIAutomation> {
    // SAFETY: a valid class id; COM is initialized on this thread.
    let created: windows::core::Result<IUIAutomation2> =
        unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) };
    match created {
        Ok(a2) => {
            let ms = |d: Duration| u32::try_from(d.as_millis()).unwrap_or(u32::MAX);
            // SAFETY: plain setters on a live object.
            unsafe {
                a2.SetConnectionTimeout(ms(CONNECTION_TIMEOUT))?;
                a2.SetTransactionTimeout(ms(TRANSACTION_TIMEOUT))?;
            }
            a2.cast()
        }
        Err(e) => {
            log::warn!("CUIAutomation8 unavailable ({e}); measuring without timeouts");
            // SAFETY: as above.
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        }
    }
}

/// One measurement of the taskbar `hwnd`.
fn measure(automation: &Automation, hwnd: isize) -> windows::core::Result<AppButtonExtents> {
    let taskbar = HWND(hwnd as *mut core::ffi::c_void);
    // SAFETY: IsWindow accepts any value.
    if !unsafe { IsWindow(Some(taskbar)) }.as_bool() {
        return Err(windows::Win32::Foundation::E_HANDLE.into());
    }
    let tray_left = tray_left(taskbar);
    // SAFETY: plain calls on live COM objects.
    let found = unsafe {
        let element = automation.automation.ElementFromHandle(taskbar)?;
        element.FindAllBuildCache(
            TreeScope_Descendants,
            &automation.buttons,
            &automation.cache,
        )?
    };
    // SAFETY: plain getter on a live array.
    let count = unsafe { found.Length()? };
    let mut buttons = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
    for i in 0..count {
        // SAFETY: `i` is within the array; the cached properties were requested above.
        let (rect, class) = unsafe {
            let e = found.GetElement(i)?;
            (e.CachedBoundingRectangle()?, e.CachedClassName()?)
        };
        buttons.push((class.to_string(), rect));
    }
    if tray_left.is_some() {
        return Ok(AppButtonExtents {
            rightmost_right: rightmost_app_button(&buttons, tray_left),
            tray_left: None,
        });
    }
    let mut rect = RECT::default();
    // SAFETY: `rect` is a valid out pointer; a stale handle only makes the call fail.
    unsafe { GetWindowRect(taskbar, &mut rect)? };
    Ok(secondary_extents(&buttons, rect))
}

/// A task list (app) button's class: `Taskbar.TaskListButtonAutomationPeer` and its kin.
fn is_task_button(class: &str) -> bool {
    class.starts_with("Taskbar.TaskListButton")
}

/// The extents on a taskbar without `TrayNotifyWnd` (a secondary taskbar): the buttons at its
/// right end are those right of the rightmost task button (without one: those in the right half
/// of `taskbar` that are not task buttons), plus every `SystemTray.*` button; the rest are app
/// buttons. Empty rectangles do not count.
fn secondary_extents(buttons: &[(String, RECT)], taskbar: RECT) -> AppButtonExtents {
    let real = || {
        buttons
            .iter()
            .filter(|(_, r)| r.right > r.left && r.bottom > r.top)
    };
    let tasks_right = real()
        .filter(|(class, _)| is_task_button(class))
        .map(|(_, r)| r.right)
        .max();
    let middle = taskbar.left + (taskbar.right - taskbar.left) / 2;
    let at_end = |class: &str, r: &RECT| {
        class.starts_with("SystemTray.")
            || (!is_task_button(class) && r.left >= tasks_right.unwrap_or(middle))
    };
    AppButtonExtents {
        rightmost_right: real()
            .filter(|(class, r)| !at_end(class, r))
            .map(|(_, r)| r.right)
            .max(),
        tray_left: real()
            .filter(|(class, r)| at_end(class, r))
            .map(|(_, r)| r.left)
            .min(),
    }
}

/// The left edge of `TrayNotifyWnd`, the notification area (`GetWindowRect`).
fn tray_left(taskbar: HWND) -> Option<i32> {
    // SAFETY: plain lookups; `rect` is a valid out pointer.
    unsafe {
        let tray = FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), None).ok()?;
        let mut rect = RECT::default();
        GetWindowRect(tray, &mut rect).ok()?;
        Some(rect.left)
    }
}

/// The right edge of the rightmost app button among UIA `Button`s `(class name, rect)`: buttons of
/// the notification area (class `SystemTray.*`, or starting at or right of `tray_left`) and empty
/// rectangles do not count. `None` when no button counts.
fn rightmost_app_button(buttons: &[(String, RECT)], tray_left: Option<i32>) -> Option<i32> {
    buttons
        .iter()
        .filter(|(class, r)| {
            !class.starts_with("SystemTray.")
                && r.right > r.left
                && r.bottom > r.top
                && tray_left.is_none_or(|t| r.left < t)
        })
        .map(|(_, r)| r.right)
        .max()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn rect(left: i32, right: i32) -> RECT {
        RECT {
            left,
            top: 1392,
            right,
            bottom: 1440,
        }
    }

    #[test]
    fn app_buttons_exclude_the_notification_area() {
        // The shape of a recorded Windows 11 taskbar (classes and rects only).
        let buttons = vec![
            ("ToggleButton".to_owned(), rect(2081, 2126)),
            ("ToggleButton".to_owned(), rect(2126, 2170)),
            (
                "Taskbar.TaskListButtonAutomationPeer".to_owned(),
                rect(2170, 2214),
            ),
            (
                "Taskbar.TaskListButtonAutomationPeer".to_owned(),
                rect(2860, 3040),
            ),
            ("SystemTray.NormalButton".to_owned(), rect(4798, 4830)),
            ("SystemTray.OmniButton".to_owned(), rect(5034, 5104)),
            ("SystemTray.ShowDesktopButton".to_owned(), rect(5108, 5120)),
        ];
        assert_eq!(rightmost_app_button(&buttons, Some(4798)), Some(3040));
        assert_eq!(rightmost_app_button(&buttons, None), Some(3040));
        // An unknown class inside the tray still does not count.
        let mut in_tray = buttons.clone();
        in_tray.push(("Button".to_owned(), rect(4900, 4950)));
        assert_eq!(rightmost_app_button(&in_tray, Some(4798)), Some(3040));
        // Empty rectangles (off-screen or collapsed buttons) do not count.
        let empty = vec![("ToggleButton".to_owned(), rect(0, 0))];
        assert_eq!(rightmost_app_button(&empty, Some(4798)), None);
    }

    #[test]
    fn a_secondary_taskbar_reports_its_right_end() {
        let taskbar = RECT {
            left: -1920,
            top: 1032,
            right: 0,
            bottom: 1080,
        };
        let r = |left, right| RECT {
            left,
            top: 1032,
            right,
            bottom: 1080,
        };
        let mut buttons = vec![
            ("ToggleButton".to_owned(), r(-1100, -1056)),
            (
                "Taskbar.TaskListButtonAutomationPeer".to_owned(),
                r(-1056, -1012),
            ),
            (
                "Taskbar.TaskListButtonAutomationPeer".to_owned(),
                r(-1012, -968),
            ),
            // The clock and Show desktop, whatever their classes.
            ("Clock".to_owned(), r(-120, -12)),
            ("SystemTray.ShowDesktopButton".to_owned(), r(-12, 0)),
        ];
        assert_eq!(
            secondary_extents(&buttons, taskbar),
            AppButtonExtents {
                rightmost_right: Some(-968),
                tray_left: Some(-120),
            }
        );
        // No app open on that monitor: the right half is the end.
        buttons.retain(|(class, _)| !is_task_button(class));
        buttons.push(("ToggleButton".to_owned(), r(-1500, -1456)));
        assert_eq!(
            secondary_extents(&buttons, taskbar),
            AppButtonExtents {
                rightmost_right: Some(-1056),
                tray_left: Some(-120),
            }
        );
        // No clock: nothing at the end.
        buttons.retain(|(_, rect)| rect.right < -200);
        assert_eq!(
            secondary_extents(&buttons, taskbar),
            AppButtonExtents {
                rightmost_right: Some(-1056),
                tray_left: None,
            }
        );
    }

    #[test]
    fn a_missing_taskbar_is_unknown_and_requests_coalesce() {
        let (tx, rx) = mpsc::channel();
        let handle = OccupancyHandle::spawn(Box::new(move |r| {
            let _ = tx.send(r);
        }))
        .expect("spawn");
        // Not a window: the answer is Unknown.
        handle.request(0);
        let first = rx.recv_timeout(Duration::from_secs(10)).expect("answer");
        assert_eq!(first, OccupancyResult::Unknown);
        handle.shutdown();
        // The sink was dropped with the thread.
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
