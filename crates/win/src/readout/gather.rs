//! Gathering the taskbar's geometry for [`crate::placement::place`] (ARCHITECTURE "Readout
//! placement"): `GetWindowRect` on `Shell_TrayWnd` and `TrayNotifyWnd`, the monitor's rectangles
//! and effective DPI. Never `SHAppBarMessage`, which can deadlock while Explorer hangs; auto-hide
//! is read from the geometry instead (an auto-hide taskbar does not shrink the work area, and a
//! hidden one is moved off the monitor edge).
//!
//! None of these calls sends a message to another process, so a hung Explorer cannot block the
//! readout thread here.

use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW,
    GetWindowRect, IsWindowVisible, IsZoomed, WS_CAPTION, WS_THICKFRAME,
};
use windows::core::w;

use super::TaskbarTarget;
use crate::geom::Rect;
use crate::placement::{AppButtonExtents, TaskbarAlignment, TaskbarEdge, TaskbarGeometry};

/// The most secondary taskbars looked at (a guard against an endless window walk).
const MAX_SECONDARY: usize = 16;

/// The taskbar of `target`: `Shell_TrayWnd`, or the `Shell_SecondaryTrayWnd` on the target's
/// monitor (every-taskbar-spec §2).
pub(super) fn find_taskbar(target: TaskbarTarget) -> Option<HWND> {
    match target {
        TaskbarTarget::Primary => {
            // SAFETY: a plain lookup by class name.
            unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.ok()
        }
        TaskbarTarget::Secondary { monitor } => secondary_windows()
            .into_iter()
            .find(|h| monitor_of(*h).is_some_and(|(_, m, _, _)| m == monitor)),
    }
}

/// Every `Shell_SecondaryTrayWnd` (top-level windows of that class, in z-order).
fn secondary_windows() -> Vec<HWND> {
    let mut out = Vec::new();
    let mut after: Option<HWND> = None;
    while out.len() < MAX_SECONDARY {
        // SAFETY: a plain lookup by class name among top-level windows.
        let next = unsafe { FindWindowExW(None, after, w!("Shell_SecondaryTrayWnd"), None) }.ok();
        let Some(next) = next else {
            break;
        };
        out.push(next);
        after = Some(next);
    }
    out
}

/// The monitors that have a secondary taskbar now, left to right (every-taskbar-spec §2).
pub fn secondary_monitors() -> Vec<Rect> {
    let mut monitors: Vec<Rect> = secondary_windows()
        .into_iter()
        .filter_map(|h| monitor_of(h).map(|(_, m, _, _)| m))
        .collect();
    monitors.sort_by_key(|m| (m.left, m.top));
    monitors.dedup();
    monitors
}

/// `GetWindowRect` as a [`Rect`].
pub(super) fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut r = RECT::default();
    // SAFETY: `r` is a valid out pointer; a stale handle only makes the call fail.
    unsafe { GetWindowRect(hwnd, &mut r) }.ok()?;
    Some(rect(r))
}

fn rect(r: RECT) -> Rect {
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// The monitor of `hwnd`: its rectangle, work area and effective DPI.
pub(super) fn monitor_of(hwnd: HWND) -> Option<(HMONITOR, Rect, Rect, u32)> {
    // SAFETY: plain queries; `info` and the DPI out pointers are valid locals.
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: u32::try_from(size_of::<MONITORINFO>()).unwrap_or(40),
            ..MONITORINFO::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_err() {
            dpi_x = 96;
        }
        Some((monitor, rect(info.rcMonitor), rect(info.rcWork), dpi_x))
    }
}

/// The class name of a window (empty when it cannot be read).
pub(super) fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    // SAFETY: the buffer is valid for the call.
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..usize::try_from(n).unwrap_or(0).min(buf.len())])
}

/// `HKCU\…\Explorer\Advanced\TaskbarAl`: 0 = left; anything else, or missing, = centred (the
/// Windows 11 default). Read once per start and per `WM_SETTINGCHANGE`, not per placement.
pub(super) fn read_alignment() -> TaskbarAlignment {
    let mut value = 1u32;
    let mut size = u32::try_from(size_of::<u32>()).unwrap_or(4);
    // SAFETY: `value` and `size` are valid out pointers of the stated size.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"),
            w!("TaskbarAl"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_SUCCESS && value == 0 {
        TaskbarAlignment::Left
    } else {
        TaskbarAlignment::Center
    }
}

/// The edge the taskbar sits on: wider than tall → top or bottom by its centre; else left or
/// right.
pub(super) fn edge_of(taskbar: Rect, monitor: Rect) -> TaskbarEdge {
    let horizontal = taskbar.width() >= taskbar.height();
    if horizontal {
        if taskbar.top + taskbar.bottom < monitor.top + monitor.bottom {
            TaskbarEdge::Top
        } else {
            TaskbarEdge::Bottom
        }
    } else if taskbar.left + taskbar.right < monitor.left + monitor.right {
        TaskbarEdge::Left
    } else {
        TaskbarEdge::Right
    }
}

/// The taskbar is set to auto-hide (the work area does not exclude it) **and** is hidden right
/// now (moved partly off its monitor edge). A shown auto-hide taskbar docks like any other, so
/// the readout follows it in and out.
pub(super) fn auto_hidden(taskbar: Rect, monitor: Rect, work: Rect, edge: TaskbarEdge) -> bool {
    match edge {
        TaskbarEdge::Bottom => work.bottom >= monitor.bottom && taskbar.bottom > monitor.bottom,
        TaskbarEdge::Top => work.top <= monitor.top && taskbar.top < monitor.top,
        TaskbarEdge::Left => work.left <= monitor.left && taskbar.left < monitor.left,
        TaskbarEdge::Right => work.right >= monitor.right && taskbar.right > monitor.right,
    }
}

/// Shell windows that cover a monitor without being a full-screen app.
fn is_shell_window(class: &str) -> bool {
    matches!(
        class,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

/// Whether a window with this rectangle, maximized state and `GWL_STYLE` is a full-screen app on
/// `monitor`: it covers the whole monitor rectangle and is neither maximized nor framed (a caption
/// or a sizing border).
///
/// With a normal taskbar a maximized window covers only the work area. With an auto-hide taskbar
/// the work area is the whole monitor, and a maximized framed window covers it plus its invisible
/// resize borders; the shell does not treat it as full screen either (the taskbar still slides in
/// over it), so neither does the readout. Full-screen browsers, players and games drop the frame.
pub(super) fn covers_as_full_screen(window: Rect, monitor: Rect, zoomed: bool, style: u32) -> bool {
    let framed = style & WS_CAPTION.0 == WS_CAPTION.0 || style & WS_THICKFRAME.0 != 0;
    !zoomed
        && !framed
        && window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

/// A full-screen window is in the foreground on `monitor` ([`covers_as_full_screen`]) and is not
/// the desktop or the taskbar.
pub(super) fn fullscreen_app_on(monitor_handle: HMONITOR, monitor: Rect) -> bool {
    // SAFETY: plain queries.
    let fg = unsafe { GetForegroundWindow() };
    if fg.is_invalid() {
        return false;
    }
    if is_shell_window(&class_name(fg)) {
        return false;
    }
    // SAFETY: as above.
    if unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) } != monitor_handle {
        return false;
    }
    // SAFETY: plain queries on a window handle (a stale one only gives false / 0).
    let (zoomed, style) = unsafe { (IsZoomed(fg).as_bool(), GetWindowLongPtrW(fg, GWL_STYLE)) };
    // The style bits are the low 32 bits of the value.
    let style = style as u32;
    window_rect(fg).is_some_and(|r| covers_as_full_screen(r, monitor, zoomed, style))
}

/// The notification area (`TrayNotifyWnd`) and, on Windows 10, the overflow chevron (a visible
/// `Button` child of it). On Windows 11 the chevron is a XAML element at the notification area's
/// left edge, so `chevron` is `None` and the area's left edge is the chevron's.
pub(super) fn notification_area(taskbar: HWND) -> (Option<HWND>, Option<Rect>, Option<Rect>) {
    // SAFETY: plain lookups by class name under a window of another process.
    let tray = unsafe { FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), None) }.ok();
    let Some(tray) = tray else {
        return (None, None, None);
    };
    let area = window_rect(tray);
    // SAFETY: as above.
    let chevron = unsafe { FindWindowExW(Some(tray), None, w!("Button"), None) }
        .ok()
        // SAFETY: plain query.
        .filter(|b| unsafe { IsWindowVisible(*b) }.as_bool())
        .and_then(window_rect)
        .filter(|r| r.width() > 0 && r.height() > 0);
    (Some(tray), area, chevron)
}

/// The geometry of `taskbar` now (ARCHITECTURE "taskbar and tray rects per monitor, DPI,
/// auto-hide, alignment, app-button extents from the occupancy thread").
pub(super) fn gather(
    taskbar: HWND,
    alignment: TaskbarAlignment,
    app_buttons: Option<AppButtonExtents>,
    secondary: bool,
) -> Option<TaskbarGeometry> {
    let taskbar_rect = window_rect(taskbar)?;
    let (monitor_handle, monitor, work_area, dpi) = monitor_of(taskbar)?;
    let edge = edge_of(taskbar_rect, monitor);
    let (_, notification_area, chevron) = notification_area(taskbar);
    Some(TaskbarGeometry {
        monitor,
        work_area,
        taskbar: taskbar_rect,
        notification_area,
        chevron,
        dpi,
        edge,
        alignment,
        auto_hide: auto_hidden(taskbar_rect, monitor, work_area, edge),
        fullscreen_app: fullscreen_app_on(monitor_handle, monitor),
        app_buttons,
        secondary,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn edges_follow_the_taskbar_centre() {
        let m = r(0, 0, 1920, 1080);
        assert_eq!(edge_of(r(0, 1032, 1920, 1080), m), TaskbarEdge::Bottom);
        assert_eq!(edge_of(r(0, 0, 1920, 48), m), TaskbarEdge::Top);
        assert_eq!(edge_of(r(0, 0, 62, 1080), m), TaskbarEdge::Left);
        assert_eq!(edge_of(r(1858, 0, 1920, 1080), m), TaskbarEdge::Right);
        // A monitor left of the primary one.
        let left = r(-2560, 0, 0, 1440);
        assert_eq!(edge_of(r(-2560, 1392, 0, 1440), left), TaskbarEdge::Bottom);
    }

    #[test]
    fn auto_hide_counts_only_while_the_taskbar_is_out_of_view() {
        let m = r(0, 0, 1920, 1080);
        let full = m;
        let docked_work = r(0, 0, 1920, 1032);
        // Normal taskbar: the work area excludes it.
        assert!(!auto_hidden(
            r(0, 1032, 1920, 1080),
            m,
            docked_work,
            TaskbarEdge::Bottom
        ));
        // Auto-hide, shown: on screen, work area is the whole monitor.
        assert!(!auto_hidden(
            r(0, 1032, 1920, 1080),
            m,
            full,
            TaskbarEdge::Bottom
        ));
        // Auto-hide, hidden: 2 px left on screen.
        assert!(auto_hidden(
            r(0, 1078, 1920, 1126),
            m,
            full,
            TaskbarEdge::Bottom
        ));
        assert!(auto_hidden(r(0, -46, 1920, 2), m, full, TaskbarEdge::Top));
    }

    #[test]
    fn maximized_or_framed_windows_are_not_full_screen_apps() {
        use windows::Win32::UI::WindowsAndMessaging::{WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE};
        let m = r(0, 0, 1920, 1080);
        // Auto-hide taskbar: a maximized framed window spans the monitor plus its borders.
        let maximized = r(-8, -8, 1928, 1088);
        let framed = (WS_OVERLAPPEDWINDOW | WS_VISIBLE).0;
        assert!(!covers_as_full_screen(maximized, m, true, framed));
        assert!(!covers_as_full_screen(maximized, m, false, framed));
        // A full-screen browser, player or game: no frame, the whole monitor.
        let popup = (WS_POPUP | WS_VISIBLE).0;
        assert!(covers_as_full_screen(m, m, false, popup));
        // A frameless window that does not cover the monitor.
        assert!(!covers_as_full_screen(r(0, 0, 1920, 1032), m, false, popup));
        // Maximized without a frame (a kiosk-like app) is not full screen either.
        assert!(!covers_as_full_screen(m, m, true, popup));
    }

    #[test]
    fn shell_windows_are_not_full_screen_apps() {
        assert!(is_shell_window("Progman"));
        assert!(is_shell_window("WorkerW"));
        assert!(!is_shell_window("Chrome_WidgetWin_1"));
    }
}
