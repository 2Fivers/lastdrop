//! The taskbar readout thread (R§6, R§10–R§12; ARCHITECTURE "Threads", "Taskbar events").
//!
//! The thread owns: a hidden top-level message window that receives `TaskbarCreated`,
//! `WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE`, `WM_DPICHANGED`, `WM_TIMECHANGE` and
//! `WM_POWERBROADCAST`; the readout window (`WS_POPUP`, `WS_EX_LAYERED | WS_EX_TOOLWINDOW |
//! WS_EX_TOPMOST | WS_EX_NOACTIVATE`, never parented to the taskbar, per-pixel alpha with
//! `UpdateLayeredWindow`); out-of-context WinEvent hooks (`EVENT_OBJECT_LOCATIONCHANGE` filtered
//! to the taskbar and tray, `EVENT_SYSTEM_FOREGROUND` / `EVENT_OBJECT_REORDER` to re-assert
//! `HWND_TOPMOST` on `Shell_TrayWnd` activity — never polling); its own message loop and
//! drawing; the UIA provider of the readout (R§11); the occupancy thread's handle.
//!
//! It draws the latest [`ReadoutModel`] with `lastdrop-core`'s layout and bar functions at the
//! taskbar monitor's DPI, places it with [`crate::placement::place`], and reports user actions
//! and placement through the event sink. It never does I/O, never waits on locks held by other
//! threads, and makes no synchronous cross-process call while holding a lock. After an Explorer
//! restart (`TaskbarCreated`) it rebuilds the readout in-process.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use lastdrop_core::readout::{ReadoutForm, ReadoutModel};

use crate::flyout_pos::MonitorInfo;
use crate::geom::{Point, Rect};
use crate::placement::{Placement, TaskbarEdge};
use crate::theme::{ThemeChange, ThemeInfo};

/// Start-up options of the readout thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReadoutConfig {
    /// Show the readout (settings "Taskbar readout"); the thread runs either way so the message
    /// window keeps receiving system broadcasts.
    pub enabled: bool,
    /// Hover delay; `None` = `SPI_GETMOUSEHOVERTIME` (default 400 ms, R§3.11 `HOVER_DELAY`).
    pub hover_delay: Option<Duration>,
    /// The theme to draw with until [`ReadoutHandle::set_theme`] says otherwise. The readout
    /// thread never reads the theme itself (registry, `UISettings`): the main thread reads it once
    /// per change and hands the same value to the readout, the tray icon and the Slint windows,
    /// so they cannot disagree.
    pub theme: ThemeInfo,
}

/// A system broadcast seen by the readout thread's message window, forwarded to the main thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SystemEvent {
    /// Theme, accent or contrast changed: re-read the theme, rebuild models and the tray icon.
    Theme(ThemeChange),
    /// `WM_SETTINGCHANGE` "intl": re-read the locale, rebuild models (widths change, R§6.3).
    Locale,
    /// `WM_TIMECHANGE` or a time-zone change: re-arm the minute tick (R§12).
    TimeChanged,
    /// Resume from sleep (`WM_POWERBROADCAST` / `PBT_APMRESUMEAUTOMATIC`): re-arm the minute tick
    /// and refresh (R§12).
    Resumed,
}

/// What the readout thread reports (R§10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReadoutEvent {
    /// The pointer rested on the readout for the hover delay: show the hover card centred on
    /// `rect` (device px, R§8.1). The hover plate is drawn by the readout itself.
    Hover {
        /// The readout's slot on screen.
        rect: Rect,
    },
    /// The pointer left the readout (after the 100 ms grace): hide the hover card.
    Leave,
    /// Left click (button up; a double click counts as one): toggle the flyout.
    Click,
    /// UI Automation `Invoke` on the readout root (R§11 "Invoke = open flyout"): open the flyout
    /// (not a toggle) as a keyboard open, focus on the first card (F§9).
    Invoked,
    /// Right click (button up): the context menu at `pt`. The main thread shows it (a popup menu
    /// must be owned by a window of the calling thread).
    RightClick {
        /// Screen point, device px.
        pt: Point,
    },
    /// The placement, form or DPI changed (rows shown by the hover card follow the form). Sent
    /// once after start-up and after every change, also while the readout is hidden, so the main
    /// thread always knows the taskbar monitor (tray icon size R§9.1, hover card and flyout
    /// placement R§8.1, F§1.3).
    PlacementChanged {
        /// Where the readout is now.
        placement: Placement,
        /// The form drawn (Hidden when not shown).
        form: ReadoutForm,
        /// The taskbar's monitor, work area and DPI.
        monitor: MonitorInfo,
        /// The taskbar's rectangle (hover card placement, R§8.1).
        taskbar: Rect,
        /// The taskbar's edge (the hover card opens below a top taskbar, R§8.1).
        edge: TaskbarEdge,
    },
    /// A system broadcast.
    System(SystemEvent),
}

/// Receives readout events **on the readout thread**. It must not block: the app's sink forwards
/// with `slint::invoke_from_event_loop` and returns.
pub type EventSink = Box<dyn Fn(ReadoutEvent) + Send + 'static>;

/// The main thread's handle to the readout thread.
pub struct ReadoutHandle {
    #[expect(dead_code, reason = "used once the handle is implemented (M1)")]
    thread: Option<JoinHandle<()>>,
}

impl ReadoutHandle {
    /// Starts the readout thread. It creates its windows, hooks and the occupancy thread, then
    /// waits for a model; nothing is shown before the first [`ReadoutHandle::update`].
    pub fn spawn(config: ReadoutConfig, sink: EventSink) -> std::io::Result<ReadoutHandle> {
        todo!("M1: spawn the readout thread")
    }

    /// Hands the readout a new model: swaps it into a one-slot mailbox (a lock held only for the
    /// swap, never across a Win32 call) and `PostMessage`s the thread to redraw. Identical models
    /// do not redraw (R§12).
    pub fn update(&self, model: Arc<ReadoutModel>) {
        todo!("M1: post the model")
    }

    /// Shows or hides the readout (settings, context menu "Taskbar readout ▸ Off").
    pub fn set_enabled(&self, enabled: bool) {
        todo!("M1: post enabled")
    }

    /// Hands the readout a new theme (after a [`SystemEvent::Theme`] round trip through the main
    /// thread): same mailbox rule as [`ReadoutHandle::update`]; redraws (R§12
    /// "theme/accent/HC change").
    pub fn set_theme(&self, theme: ThemeInfo) {
        todo!("M1: post the theme")
    }

    /// Asks the thread to destroy its windows, unhook and exit, and joins it.
    pub fn shutdown(self) {
        todo!("M1: post quit and join")
    }
}
