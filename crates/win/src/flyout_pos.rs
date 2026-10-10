//! Flyout and hover-card positions (F§1.3, R§8.1). The rectangles are pure functions of the
//! anchor, the window size and the monitor; [`monitor_for`] and [`popup_position`] are the Win32
//! parts (from the spike: `Shell_NotifyIconGetRect` via [`crate::tray::Tray::icon_rect`] → anchor →
//! `CalculatePopupWindowPosition` with the work area).
//!
//! Threading: main thread.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use crate::geom::{Point, Rect, Size};
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

/// The monitor of the anchor (`MonitorFromPoint` nearest, `GetMonitorInfoW`, `GetDpiForMonitor`).
pub fn monitor_for(anchor: Anchor) -> MonitorInfo {
    todo!("M1: monitor of the anchor")
}

/// The flyout rectangle (F§1.3), device px: centred on the anchor horizontally, clamped to
/// `[work.left + 12 s, work.right − 12 s − width]`; bottom edge `12 s` above the work area's
/// bottom (top taskbar: top edge `12 s` below the work area's top). `size` is the window size in
/// device px (width 360 × s; height from content, max `min(560, work height − 24) × s`).
pub fn flyout_rect(anchor: Anchor, size: Size, monitor: &MonitorInfo, edge: TaskbarEdge) -> Rect {
    todo!("M1: F§1.3")
}

/// The hover card rectangle (R§8.1): horizontally centred on `anchor`, clamped `12 s` inside the
/// work area; bottom edge `8 s` above the taskbar's top edge (top taskbar: top edge `8 s` below
/// its bottom).
pub fn hover_card_rect(
    anchor: Rect,
    size: Size,
    monitor: &MonitorInfo,
    taskbar: Rect,
    edge: TaskbarEdge,
) -> Rect {
    todo!("M1: R§8.1")
}

/// The spike's `CalculatePopupWindowPosition` placement (`TPM_CENTERALIGN | TPM_BOTTOMALIGN |
/// TPM_VERTICAL | TPM_WORKAREA` with an exclusion rect), used to cross-check [`flyout_rect`] on
/// unusual layouts.
pub fn popup_position(anchor: Anchor, size: Size, monitor: &MonitorInfo) -> Point {
    todo!("M1: CalculatePopupWindowPosition")
}
