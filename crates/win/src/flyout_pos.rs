//! Flyout and hover-card positions (F§1.3, R§8.1). The rectangles are pure functions of the
//! anchor, the window size and the monitor; [`monitor_for`] and [`popup_position`] are the Win32
//! parts (from the spike: `Shell_NotifyIconGetRect` via [`crate::tray::Tray::icon_rect`] → anchor →
//! `CalculatePopupWindowPosition` with the work area).
//!
//! Logical constants become device px one by one, `round(x × s)` half up with `s = dpi / 96`
//! (R§0). "Centred" means the window's centre is the anchor's centre, rounded down for odd
//! differences (negative coordinates of monitors left of the primary included).
//!
//! Threading: main thread.

use windows::Win32::Foundation::{POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CalculatePopupWindowPosition, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, TPM_BOTTOMALIGN,
    TPM_CENTERALIGN, TPM_VERTICAL, TPM_WORKAREA,
};

use crate::geom::{Point, Rect, Size, point_to_win, rect_from_win, rect_to_win};
use crate::placement::TaskbarEdge;

/// What opened the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Anchor {
    /// The tray icon's rect (`Shell_NotifyIconGetRect`) or the readout's slot.
    Rect(Rect),
    /// A point (the icon rect is unavailable, e.g. the icon is in the overflow).
    Point(Point),
}

/// A monitor's rectangles and DPI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MonitorInfo {
    /// The monitor.
    pub monitor: Rect,
    /// Its work area.
    pub work: Rect,
    /// Its effective DPI.
    pub dpi: u32,
}

/// Flyout and hover-card margin from the work-area edges, logical px (F§1.3, R§8.1).
const EDGE_MARGIN: i32 = 12;
/// Hover card gap above (or below) the taskbar, logical px (R§8.1).
const CARD_GAP: i32 = 8;

/// `round(x × dpi / 96)`, half up, for `x ≥ 0` (R§0).
fn scale(x: i32, dpi: u32) -> i32 {
    let v = (2 * i64::from(x) * i64::from(dpi) + 96).div_euclid(192);
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// Twice the anchor's horizontal centre (exact for odd widths).
fn center2_x(anchor: Anchor) -> i64 {
    match anchor {
        Anchor::Rect(r) => i64::from(r.left) + i64::from(r.right),
        Anchor::Point(p) => 2 * i64::from(p.x),
    }
}

/// The left (or top) edge of a `length`-long window centred on a doubled centre.
fn centred(center2: i64, length: i32) -> i32 {
    i32::try_from((center2 - i64::from(length)).div_euclid(2)).unwrap_or(0)
}

/// Clamps a window start to `[lo, hi − length]`; when it does not fit, it starts at `lo`.
fn clamp_span(start: i32, length: i32, lo: i32, hi: i32) -> i32 {
    let max = hi - length;
    if max < lo { lo } else { start.clamp(lo, max) }
}

/// The monitor of the anchor (`MonitorFromPoint` nearest, `GetMonitorInfoW`, `GetDpiForMonitor`).
///
/// Should Windows not answer, the primary screen's size with 96 DPI is returned.
pub fn monitor_for(anchor: Anchor) -> MonitorInfo {
    let probe = match anchor {
        Anchor::Rect(r) => r.center(),
        Anchor::Point(p) => p,
    };
    // SAFETY: plain value calls; `info` is a live MONITORINFO with cbSize set, and the DPI
    // out-parameters are live locals.
    unsafe {
        let monitor = MonitorFromPoint(point_to_win(probe), MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let (rect, work) = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            (rect_from_win(info.rcMonitor), rect_from_win(info.rcWork))
        } else {
            log::warn!("flyout_pos: GetMonitorInfoW failed; using the primary screen size");
            let screen = Rect {
                left: 0,
                top: 0,
                right: GetSystemMetrics(SM_CXSCREEN),
                bottom: GetSystemMetrics(SM_CYSCREEN),
            };
            (screen, screen)
        };
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        let dpi = match GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) {
            Ok(()) if dpi_x > 0 => dpi_x,
            _ => 96,
        };
        MonitorInfo {
            monitor: rect,
            work,
            dpi,
        }
    }
}

/// The flyout rectangle (F§1.3), device px: centred on the anchor horizontally, clamped to
/// `[work.left + 12 s, work.right − 12 s − width]`; bottom edge `12 s` above the work area's
/// bottom (top taskbar: top edge `12 s` below the work area's top). `size` is the window size in
/// device px (width 360 × s; height from content, max `min(560, work height − 24) × s`).
///
/// Left and right (third-party vertical) taskbars use the bottom rule. A window taller than the
/// work area minus the margins starts `12 s` below the work area's top.
///
/// A rect anchor inside the work area — the readout floating above the taskbar (no room on it)
/// — moves the flyout's edge `12 s` beyond the anchor instead, so the flyout never covers it
/// (**not from the spec**: F§1.3 assumes the anchor is on the taskbar).
pub fn flyout_rect(anchor: Anchor, size: Size, monitor: &MonitorInfo, edge: TaskbarEdge) -> Rect {
    let margin = scale(EDGE_MARGIN, monitor.dpi);
    let work = monitor.work;
    let left = clamp_span(
        centred(center2_x(anchor), size.width),
        size.width,
        work.left + margin,
        work.right - margin,
    );
    // A point anchor (the cursor, when the icon rect is unknown) may lie in the overflow popup
    // above the taskbar; only a rect anchor (tray icon, readout slot) moves the flyout.
    let (near_top, near_bottom) = match anchor {
        Anchor::Rect(r) => (r.top, r.bottom),
        Anchor::Point(_) => (work.bottom, work.top),
    };
    let top = match edge {
        TaskbarEdge::Top => work.top.max(near_bottom) + margin,
        TaskbarEdge::Bottom | TaskbarEdge::Left | TaskbarEdge::Right => {
            (work.bottom.min(near_top) - margin - size.height).max(work.top + margin)
        }
    };
    Rect {
        left,
        top,
        right: left + size.width,
        bottom: top + size.height,
    }
}

/// The hover card rectangle (R§8.1): horizontally centred on `anchor`, clamped `12 s` inside the
/// work area; bottom edge `8 s` above the taskbar's top edge (top taskbar: top edge `8 s` below
/// its bottom), or above the anchor when the anchor sits above the taskbar (a floating readout),
/// so the card never overlaps it.
///
/// The `12 s` clamp runs along the taskbar; across it the `8 s` gap rules. Vertical taskbars
/// (tray icon only, the readout is hidden there): the card sits `8 s` beside the taskbar,
/// vertically centred on the anchor and clamped `12 s` inside the work area. A card taller than
/// the space above a bottom taskbar starts `12 s` below the work area's top (the app drops lines
/// first, R§8.1).
pub fn hover_card_rect(
    anchor: Rect,
    size: Size,
    monitor: &MonitorInfo,
    taskbar: Rect,
    edge: TaskbarEdge,
) -> Rect {
    let margin = scale(EDGE_MARGIN, monitor.dpi);
    let gap = scale(CARD_GAP, monitor.dpi);
    let work = monitor.work;
    let along_x = |center2: i64| {
        clamp_span(
            centred(center2, size.width),
            size.width,
            work.left + margin,
            work.right - margin,
        )
    };
    let along_y = |center2: i64| {
        clamp_span(
            centred(center2, size.height),
            size.height,
            work.top + margin,
            work.bottom - margin,
        )
    };
    let center_x = i64::from(anchor.left) + i64::from(anchor.right);
    let center_y = i64::from(anchor.top) + i64::from(anchor.bottom);
    // A readout floating above (below) the taskbar is the nearer edge: the card never overlaps
    // its anchor (R§8.1).
    let (left, top) = match edge {
        TaskbarEdge::Bottom => (
            along_x(center_x),
            (taskbar.top.min(anchor.top) - gap - size.height).max(work.top + margin),
        ),
        TaskbarEdge::Top => (along_x(center_x), taskbar.bottom.max(anchor.bottom) + gap),
        TaskbarEdge::Left => (taskbar.right + gap, along_y(center_y)),
        TaskbarEdge::Right => (taskbar.left - gap - size.width, along_y(center_y)),
    };
    Rect {
        left,
        top,
        right: left + size.width,
        bottom: top + size.height,
    }
}

/// The spike's `CalculatePopupWindowPosition` placement (`TPM_CENTERALIGN | TPM_BOTTOMALIGN |
/// TPM_VERTICAL | TPM_WORKAREA` with an exclusion rect), used to cross-check [`flyout_rect`] on
/// unusual layouts.
///
/// The exclusion rect is the anchor extended up to `12 s` above the work area's bottom; the result
/// is kept `12 s` inside the work area, as in the spike. Windows uses the monitor under the anchor
/// point; `monitor` supplies the DPI and the work area for the margins.
pub fn popup_position(anchor: Anchor, size: Size, monitor: &MonitorInfo) -> Point {
    let margin = scale(EDGE_MARGIN, monitor.dpi);
    let work = monitor.work;
    let (anchor_x, mut exclude) = match anchor {
        Anchor::Rect(r) => (centred(center2_x(anchor), 0), r),
        Anchor::Point(p) => (
            p.x,
            Rect {
                left: p.x,
                top: p.y,
                right: p.x + 1,
                bottom: p.y + 1,
            },
        ),
    };
    exclude.top = exclude.top.min(work.bottom) - margin;
    let point = POINT {
        x: anchor_x,
        y: exclude.top,
    };
    let window = SIZE {
        cx: size.width,
        cy: size.height,
    };
    let exclude_win = rect_to_win(exclude);
    let flags = TPM_CENTERALIGN | TPM_BOTTOMALIGN | TPM_VERTICAL | TPM_WORKAREA;
    let mut out = RECT::default();
    // SAFETY: every pointer refers to a live local for the duration of the call.
    let placed = unsafe {
        CalculatePopupWindowPosition(&point, &window, flags.0, Some(&exclude_win), &mut out)
    };
    let (x, y) = match placed {
        Ok(()) => (out.left, out.top),
        Err(e) => {
            log::debug!("flyout_pos: CalculatePopupWindowPosition failed: {e}");
            (anchor_x - size.width / 2, exclude.top - size.height)
        }
    };
    Point {
        x: clamp_span(x, size.width, work.left + margin, work.right - margin),
        y: clamp_span(y, size.height, work.top + margin, work.bottom - margin),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rounds_half_up_per_constant() {
        assert_eq!(scale(12, 96), 12);
        assert_eq!(scale(12, 120), 15);
        assert_eq!(scale(12, 144), 18);
        assert_eq!(scale(12, 192), 24);
        assert_eq!(scale(6, 120), 8); // 7.5 → 8
        assert_eq!(scale(8, 168), 14); // 175 %
        assert_eq!(scale(0, 144), 0);
    }

    #[test]
    fn centring_floors_for_negative_coordinates_too() {
        assert_eq!(centred(2 * 100, 10), 95);
        assert_eq!(centred(201, 10), 95); // centre 100.5
        assert_eq!(centred(-201, 10), -106); // centre -100.5
    }

    #[test]
    fn the_monitor_of_the_primary_origin_is_sane() {
        let m = monitor_for(Anchor::Point(Point { x: 0, y: 0 }));
        assert!(m.dpi >= 96);
        assert!(m.monitor.width() > 0 && m.monitor.height() > 0);
        assert!(m.work.left >= m.monitor.left && m.work.right <= m.monitor.right);
        assert!(m.work.top >= m.monitor.top && m.work.bottom <= m.monitor.bottom);
    }

    #[test]
    fn popup_position_matches_flyout_rect_horizontally() {
        let m = monitor_for(Anchor::Point(Point { x: 0, y: 0 }));
        let size = Size {
            width: scale(360, m.dpi),
            height: scale(300, m.dpi).min(m.work.height() / 2),
        };
        for x in [m.work.left + 5, m.work.center().x, m.work.right - 30] {
            let icon = Rect {
                left: x,
                top: m.work.bottom - 30,
                right: x + 24,
                bottom: m.work.bottom - 6,
            };
            let anchor = Anchor::Rect(icon);
            let p = popup_position(anchor, size, &m);
            let r = flyout_rect(anchor, size, &m, TaskbarEdge::Bottom);
            assert_eq!(p.x, r.left, "anchor x {x}");
            let margin = scale(12, m.dpi);
            assert!(p.y >= m.work.top + margin);
            assert!(p.y + size.height <= m.work.bottom - margin);
        }
    }
}
