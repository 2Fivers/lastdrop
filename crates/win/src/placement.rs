//! Readout placement: a **pure** function from recorded taskbar geometry to a rectangle
//! (ARCHITECTURE "Readout placement", R§4.2, R§6.1; ROADMAP "Bug-free by construction"). No Win32
//! calls here: the readout thread gathers [`TaskbarGeometry`] with `GetWindowRect` (never
//! `SHAppBarMessage`) and the occupancy thread's [`AppButtonExtents`]; tests replay recorded
//! geometries from `tests/layouts/*.json`.
//!
//! Threading: pure; called on the readout thread.

use lastdrop_core::consts::layout::{ROOM_APP_GAP, ROOM_CHEVRON_GAP, SLOT_RIGHT_GAP};
use lastdrop_core::readout::{ContentForm, ReadoutForm, ReadoutLayout, choose_form};
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
    /// On a taskbar without a notification area window (a secondary taskbar,
    /// every-taskbar-spec §3): the left edge of the buttons at its right end (the clock, Show
    /// desktop), device px; `None` when there are none or the taskbar has `TrayNotifyWnd`.
    #[serde(default)]
    pub tray_left: Option<i32>,
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
    /// A secondary taskbar (`Shell_SecondaryTrayWnd`, every-taskbar-spec §3): no notification
    /// area window; the readout keeps clear of [`AppButtonExtents::tray_left`], else of the
    /// taskbar's right edge.
    #[serde(default)]
    pub secondary: bool,
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

/// A logical constant in device px at `scale`: `round(c × s)`, half up, on its own (R§0).
fn px(c: f32, scale: f64) -> i32 {
    // Layout constants times a sane scale are small; `as` saturates otherwise.
    (f64::from(c) * scale + 0.5).floor() as i32
}

/// `s = dpi / 96` (1 when no DPI was recorded).
fn scale_of(dpi: u32) -> f64 {
    if dpi == 0 { 1.0 } else { f64::from(dpi) / 96.0 }
}

/// The x the readout's right edge keeps `round(4 s)` away from: the left edge of the overflow
/// chevron, else of the notification area (R§4.2, R§6.1). On Windows 11 the chevron is a XAML
/// element inside `TrayNotifyWnd`, so the notification area's left edge *is* the chevron's. A
/// secondary taskbar has neither: the left edge of the buttons at its right end, else its right
/// edge (every-taskbar-spec §3). `None` otherwise.
fn anchor_x(geometry: &TaskbarGeometry) -> Option<i32> {
    geometry
        .chevron
        .or(geometry.notification_area)
        .map(|r| r.left)
        .or_else(|| geometry.app_buttons.and_then(|b| b.tray_left))
        .or_else(|| geometry.secondary.then_some(geometry.taskbar.right))
}

/// `room` (R§4.2), device px: left edge of the overflow chevron (or of the notification area if
/// there is no chevron) − `round(4 s)` − (right edge of the rightmost app button + `round(8 s)`).
/// `None` when the app-button extents are unknown, or when neither the chevron nor the
/// notification area was found. With no app button at all the taskbar's left edge stands in for
/// the rightmost button's right edge.
pub fn room(geometry: &TaskbarGeometry) -> Option<i32> {
    let s = scale_of(geometry.dpi);
    let anchor = anchor_x(geometry)?;
    let buttons = geometry.app_buttons?;
    let rightmost = buttons.rightmost_right.unwrap_or(geometry.taskbar.left);
    Some(anchor - px(ROOM_CHEVRON_GAP, s) - (rightmost + px(ROOM_APP_GAP, s)))
}

/// The placement (R§4.2, R§6.1): hidden for off content, vertical taskbars, auto-hide and
/// full-screen apps; otherwise the form from `lastdrop_core::readout::choose_form` with
/// [`room`] and the taskbar height, and the slot rectangle; floating when `room` is unknown.
/// `size` is the readout layout at this taskbar's DPI.
///
/// - Docked slot: `width(form)` × `slotH`, `top = taskbar.top + floor((taskbar.height − slotH) /
///   2)`, right edge `round(4 s)` left of the chevron or notification area (R§6.1).
/// - Floating (Lastdrop's choice, not in the spec): the same slot, right-aligned as when docked
///   (on the taskbar's right edge when no notification area was found), its bottom `round(4 s)`
///   above a bottom taskbar (top `round(4 s)` below a top taskbar), kept inside the monitor. The
///   form is chosen as if the whole taskbar left of the anchor were free.
pub fn place(
    geometry: &TaskbarGeometry,
    size: &ReadoutLayout,
    prefs: &PlacementPrefs,
) -> Placement {
    if prefs.content == ContentForm::Hidden {
        return Placement::Hidden(HiddenReason::Off);
    }
    if matches!(geometry.edge, TaskbarEdge::Left | TaskbarEdge::Right) {
        return Placement::Hidden(HiddenReason::VerticalTaskbar);
    }
    if geometry.fullscreen_app {
        return Placement::Hidden(HiddenReason::FullScreen);
    }
    if geometry.auto_hide {
        return Placement::Hidden(HiddenReason::AutoHide);
    }
    let s = scale_of(geometry.dpi);
    let taskbar = geometry.taskbar;
    let gap = px(SLOT_RIGHT_GAP, s);
    let slot_h = size.slot_h;
    let width = |form: ReadoutForm| match form {
        ReadoutForm::Compact => size.width_compact,
        ReadoutForm::TwoRows | ReadoutForm::SingleRow | ReadoutForm::Hidden => size.width_rows,
    };
    let right = anchor_x(geometry).unwrap_or(taskbar.right) - gap;
    let docked_room = room(geometry);
    let room = docked_room.unwrap_or(right - taskbar.left);
    let form = choose_form(prefs.content, room, taskbar.height(), size, prefs.previous);
    if form == ReadoutForm::Hidden {
        return Placement::Hidden(HiddenReason::NoRoom);
    }
    let w = width(form);
    if docked_room.is_some() {
        let top = taskbar.top + (taskbar.height() - slot_h).div_euclid(2);
        return Placement::Docked {
            rect: Rect {
                left: right - w,
                top,
                right,
                bottom: top + slot_h,
            },
            form,
        };
    }
    let monitor = geometry.monitor;
    let top = match geometry.edge {
        TaskbarEdge::Top => taskbar.bottom + gap,
        TaskbarEdge::Bottom | TaskbarEdge::Left | TaskbarEdge::Right => taskbar.top - gap - slot_h,
    };
    let top = top.clamp(monitor.top, (monitor.bottom - slot_h).max(monitor.top));
    let right = right.min(monitor.right).max(monitor.left + w);
    Placement::Floating {
        rect: Rect {
            left: right - w,
            top,
            right,
            bottom: top + slot_h,
        },
        form,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(scale: f64, width_rows: i32, width_compact: i32) -> ReadoutLayout {
        let slot_h = px(40.0, scale);
        let row_h = px(16.0, scale);
        let y_row1 = (slot_h - 2 * row_h).div_euclid(2);
        ReadoutLayout {
            scale,
            slot_h,
            row_h,
            y_row1,
            y_row2: y_row1 + row_h,
            y_single: (slot_h - row_h).div_euclid(2),
            x_glyph: None,
            x_label: px(6.0, scale),
            label_w: 17,
            x_bar: 0,
            bar_w: px(40.0, scale),
            x_right: 0,
            right_w: 79,
            compact_w: 53,
            value_box_w: px(24.0, scale),
            time_x: px(30.0, scale),
            width_rows,
            width_compact,
        }
    }

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn geometry() -> TaskbarGeometry {
        TaskbarGeometry {
            monitor: rect(0, 0, 1920, 1080),
            work_area: rect(0, 0, 1920, 1032),
            taskbar: rect(0, 1032, 1920, 1080),
            notification_area: Some(rect(1660, 1032, 1920, 1080)),
            chevron: None,
            dpi: 96,
            edge: TaskbarEdge::Bottom,
            alignment: TaskbarAlignment::Center,
            auto_hide: false,
            fullscreen_app: false,
            app_buttons: Some(AppButtonExtents {
                rightmost_right: Some(1200),
                tray_left: None,
            }),
            secondary: false,
        }
    }

    fn prefs(previous: ReadoutForm) -> PlacementPrefs {
        PlacementPrefs {
            content: ContentForm::TwoRows,
            previous,
        }
    }

    #[test]
    fn room_follows_r4_2() {
        let g = geometry();
        assert_eq!(room(&g), Some(1660 - 4 - 1208));
        let mut chevron = g.clone();
        chevron.chevron = Some(rect(1636, 1032, 1660, 1080));
        assert_eq!(room(&chevron), Some(1636 - 4 - 1208));
        let mut empty = g.clone();
        empty.app_buttons = Some(AppButtonExtents {
            rightmost_right: None,
            tray_left: None,
        });
        assert_eq!(room(&empty), Some(1660 - 4 - 8));
        let mut unknown = g.clone();
        unknown.app_buttons = None;
        assert_eq!(room(&unknown), None);
        let mut no_tray = g.clone();
        no_tray.notification_area = None;
        assert_eq!(room(&no_tray), None);
        let mut big = g;
        big.dpi = 192;
        assert_eq!(room(&big), Some(1660 - 8 - (1200 + 16)));
        assert_eq!((px(4.0, 1.25), px(8.0, 1.25), px(4.0, 1.75)), (5, 10, 7));
    }

    #[test]
    fn hidden_reasons_come_before_room() {
        let l = layout(1.0, 160, 134);
        let mut g = geometry();
        let off = PlacementPrefs {
            content: ContentForm::Hidden,
            previous: ReadoutForm::TwoRows,
        };
        assert_eq!(place(&g, &l, &off), Placement::Hidden(HiddenReason::Off));
        g.fullscreen_app = true;
        assert_eq!(
            place(&g, &l, &prefs(ReadoutForm::TwoRows)),
            Placement::Hidden(HiddenReason::FullScreen)
        );
        g.fullscreen_app = false;
        g.auto_hide = true;
        g.app_buttons = None;
        assert_eq!(
            place(&g, &l, &prefs(ReadoutForm::TwoRows)),
            Placement::Hidden(HiddenReason::AutoHide)
        );
        g.edge = TaskbarEdge::Right;
        assert_eq!(
            place(&g, &l, &prefs(ReadoutForm::TwoRows)),
            Placement::Hidden(HiddenReason::VerticalTaskbar)
        );
    }

    #[test]
    fn unknown_buttons_float_above_or_below_the_taskbar() {
        let l = layout(1.0, 160, 134);
        let mut g = geometry();
        g.app_buttons = None;
        assert_eq!(
            place(&g, &l, &prefs(ReadoutForm::Hidden)),
            Placement::Floating {
                rect: rect(1496, 1032 - 4 - 40, 1656, 1032 - 4),
                form: ReadoutForm::TwoRows,
            }
        );
        g.edge = TaskbarEdge::Top;
        g.taskbar = rect(0, 0, 1920, 48);
        g.notification_area = Some(rect(1660, 0, 1920, 48));
        assert_eq!(
            place(&g, &l, &prefs(ReadoutForm::TwoRows)),
            Placement::Floating {
                rect: rect(1496, 52, 1656, 92),
                form: ReadoutForm::TwoRows,
            }
        );
    }

    #[test]
    fn secondary_taskbars_keep_clear_of_the_clock_or_the_right_edge() {
        let l = layout(1.0, 160, 134);
        let mut g = geometry();
        g.notification_area = None;
        g.secondary = true;
        // The clock and Show desktop at the right end.
        g.app_buttons = Some(AppButtonExtents {
            rightmost_right: Some(1200),
            tray_left: Some(1800),
        });
        assert_eq!(room(&g), Some(1800 - 4 - 1208));
        assert_eq!(
            place(&g, &l, &prefs(ReadoutForm::TwoRows)),
            Placement::Docked {
                rect: rect(1636, 1036, 1796, 1076),
                form: ReadoutForm::TwoRows,
            }
        );
        // No clock: the taskbar's right edge.
        g.app_buttons = Some(AppButtonExtents {
            rightmost_right: Some(1200),
            tray_left: None,
        });
        assert_eq!(room(&g), Some(1920 - 4 - 1208));
        // Unknown buttons float, right-aligned to the taskbar's end.
        g.app_buttons = None;
        assert_eq!(
            place(&g, &l, &prefs(ReadoutForm::TwoRows)),
            Placement::Floating {
                rect: rect(1756, 1032 - 4 - 40, 1916, 1032 - 4),
                form: ReadoutForm::TwoRows,
            }
        );
        // A primary taskbar without a notification area still has no anchor.
        g.secondary = false;
        g.app_buttons = Some(AppButtonExtents {
            rightmost_right: Some(1200),
            tray_left: None,
        });
        assert_eq!(room(&g), None);
    }
}
