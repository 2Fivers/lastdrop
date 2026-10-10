//! Readout placement: a **pure** function from recorded taskbar geometry to a rectangle
//! (ARCHITECTURE "Readout placement", R§4.2, R§6.1; ROADMAP "Bug-free by construction"). No Win32
//! calls here: the readout thread gathers [`TaskbarGeometry`] with `GetWindowRect` (never
//! `SHAppBarMessage`) and the occupancy thread's [`AppButtonExtents`]; tests replay recorded
//! geometries from `tests/layouts/*.json`.
//!
//! Threading: pure; called on the readout thread.

use lastdrop_core::readout::{ContentForm, ReadoutForm, ReadoutLayout};
use serde::{Deserialize, Serialize};

use crate::geom::Rect;

/// Which screen edge the taskbar is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskbarEdge {
    /// Bottom (default).
    Bottom,
    /// Top (supported; the hover card opens below, R§14).
    Top,
    /// Left (third-party vertical taskbars only): the readout is HIDDEN, the tray icon remains.
    Left,
    /// Right: as Left.
    Right,
}

/// Taskbar button alignment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskbarAlignment {
    /// Centred (Windows 11 default).
    Center,
    /// Left.
    Left,
}

/// Where the taskbar's app buttons are, from the occupancy thread (UI Automation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppButtonExtents {
    /// Right edge of the rightmost app button (device px); `None` when there are no buttons.
    pub rightmost_right: Option<i32>,
}

/// The geometry of one taskbar, device px (ARCHITECTURE: "taskbar and tray rects per monitor,
/// DPI, auto-hide, alignment, app-button extents from the occupancy thread").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskbarGeometry {
    /// The monitor's rectangle.
    pub monitor: Rect,
    /// The monitor's work area.
    pub work_area: Rect,
    /// `Shell_TrayWnd` (or `Shell_SecondaryTrayWnd`).
    pub taskbar: Rect,
    /// The notification area (`TrayNotifyWnd`); `None` if not found.
    pub notification_area: Option<Rect>,
    /// The overflow chevron, when shown.
    pub chevron: Option<Rect>,
    /// The monitor's effective DPI (`GetDpiForMonitor`); `s = dpi / 96`.
    pub dpi: u32,
    /// Edge.
    pub edge: TaskbarEdge,
    /// Button alignment.
    pub alignment: TaskbarAlignment,
    /// The taskbar auto-hides (HIDDEN, R§4.2).
    pub auto_hide: bool,
    /// A full-screen app covers this monitor (HIDDEN, R§4.2).
    pub fullscreen_app: bool,
    /// App buttons; `None` when unknown (UI Automation failed or timed out).
    pub app_buttons: Option<AppButtonExtents>,
}

/// What placement needs besides geometry and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlacementPrefs {
    /// What the content allows (off → `Hidden`).
    pub content: ContentForm,
    /// The form currently drawn, for the "+ 8 px to go up" hysteresis (R§4.2).
    pub previous: ReadoutForm,
}

/// Why the readout is not shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HiddenReason {
    /// Turned off, or nothing to show.
    Off,
    /// `room < width(compact)`.
    NoRoom,
    /// A full-screen app.
    FullScreen,
    /// Auto-hidden taskbar.
    AutoHide,
    /// A vertical taskbar (R§14).
    VerticalTaskbar,
}

/// Where the readout goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Placement {
    /// In the taskbar: slot rect (`width(form)` × `round(40 s)`, vertically centred,
    /// `top = taskbar.top + floor((taskbar.height − slotH) / 2)`, right edge `round(4 s)` left of
    /// the chevron or notification area, R§6.1).
    Docked {
        /// The slot.
        rect: Rect,
        /// The form to draw.
        form: ReadoutForm,
    },
    /// App-button extents are unknown, so docking could cover buttons: a slot of the same size
    /// just above the taskbar (below a top taskbar), right-aligned as when docked (ROADMAP
    /// "falling back to a floating card instead of covering buttons"; the exact rect is a
    /// Lastdrop choice, not in the spec).
    Floating {
        /// The slot.
        rect: Rect,
        /// The form to draw.
        form: ReadoutForm,
    },
    /// Not shown.
    Hidden(HiddenReason),
}

/// `room` (R§4.2), device px: left edge of the overflow chevron (or of the notification area if
/// there is no chevron) − `round(4 s)` − (right edge of the rightmost app button + `round(8 s)`).
/// `None` when the app-button extents are unknown.
#[expect(unused_variables, reason = "M1 contract stub")]
pub fn room(geometry: &TaskbarGeometry) -> Option<i32> {
    todo!("M1: R§4.2 room")
}

/// The placement (R§4.2, R§6.1): hidden for off content, vertical taskbars, auto-hide and
/// full-screen apps; otherwise the form from `lastdrop_core::readout::choose_form` with
/// [`room`] and the taskbar height, and the slot rectangle; floating when `room` is unknown.
/// `size` is the readout layout at this taskbar's DPI.
#[expect(unused_variables, reason = "M1 contract stub")]
pub fn place(
    geometry: &TaskbarGeometry,
    size: &ReadoutLayout,
    prefs: &PlacementPrefs,
) -> Placement {
    todo!("M1: R§4.2 / R§6.1 placement")
}
