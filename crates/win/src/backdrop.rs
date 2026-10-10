//! Window styling and show/hide for the Slint windows (flyout, hover card): DWM backdrop,
//! corners, dark mode, tool-window and no-activate styles, and the spike's keep-alive show/hide
//! (F§1.1, F§1.4, R§8.1).
//!
//! Threading: main thread (the Slint windows' thread). Win32 is called on the raw `HWND` behind
//! Slint's back, as in the spike, because winit always activates a window it shows.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use windows::Win32::Foundation::HWND;

use crate::geom::Rect;
use crate::theme::ThemeChange;

/// The DWM system backdrop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Backdrop {
    /// `DWMSBT_TRANSIENTWINDOW`: acrylic; DWM draws its solid fallback with transparency off.
    Acrylic,
    /// `DWMSBT_NONE`: high contrast, the window paints COLOR_WINDOW.
    None,
}

/// How a popup window is styled after creation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PopupStyle {
    /// Backdrop.
    pub backdrop: Backdrop,
    /// `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` (8 px, DWM border and shadow).
    pub round_corners: bool,
    /// `DWMWA_USE_IMMERSIVE_DARK_MODE` (the taskbar theme).
    pub dark: bool,
    /// `WS_EX_NOACTIVATE` (the hover card; never takes focus, R§8.1).
    pub no_activate: bool,
}

/// Applies the spike's native setup: `WS_EX_TOOLWINDOW` (no taskbar button, not in Alt+Tab), no
/// system menu or caption boxes, `WS_EX_TOPMOST`, the backdrop, corners and dark mode,
/// `DwmExtendFrameIntoClientArea(−1)`, `SWP_FRAMECHANGED`; then hides the window (keep-alive).
pub fn apply(hwnd: HWND, style: &PopupStyle) -> windows::core::Result<()> {
    todo!("M1: spike setup_native_window")
}

/// Updates dark mode and backdrop after a theme change.
pub fn set_theme(hwnd: HWND, dark: bool, backdrop: Backdrop) -> windows::core::Result<()> {
    todo!("M1: DWMWA_USE_IMMERSIVE_DARK_MODE + backdrop")
}

/// Moves and sizes the window (device px) without activating it.
pub fn set_rect(hwnd: HWND, rect: Rect) {
    todo!("M1: SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)")
}

/// Shows without activation: `ShowWindow(SW_SHOWNOACTIVATE)` + `SetWindowPos(HWND_TOPMOST, …,
/// SWP_NOACTIVATE)` (the hover card; `GetForegroundWindow()` must be unchanged afterwards).
pub fn show_no_activate(hwnd: HWND) {
    todo!("M1: show without activation")
}

/// Shows and activates (the flyout, F§1.4): `SetWindowPos(HWND_TOPMOST, … SWP_SHOWWINDOW)`,
/// `SetForegroundWindow` (allowed: the process just received the click), then
/// `RedrawWindow(RDW_INVALIDATE | RDW_INTERNALPAINT)` so Slint renders at once.
pub fn show_and_activate(hwnd: HWND) {
    todo!("M1: show and activate")
}

/// Hides (`SW_HIDE`).
pub fn hide(hwnd: HWND) {
    todo!("M1: hide")
}

/// Lets the tray's Win32 popup menus follow dark mode (uxtheme ordinals 135 `SetPreferredAppMode`
/// and 136 `FlushMenuThemes`, as in the spike). Call once at start-up.
pub fn allow_dark_menus() {
    todo!("M1: spike allow_dark_menus")
}

/// What a watched window reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowNotice {
    /// `WM_ACTIVATE` → inactive: the flyout hides (F§1.4) and records the time for the 300 ms
    /// toggle debounce.
    Deactivated,
    /// A theme message ([`crate::theme::classify`]).
    Theme(ThemeChange),
    /// `WM_DPICHANGED` with the new DPI.
    DpiChanged(u32),
}

/// Subclasses `hwnd` (`SetWindowSubclass`) to report [`WindowNotice`]s and to keep DWM drawing
/// the active backdrop while the window is shown without activation (`WM_NCACTIVATE` with
/// `TRUE`, as in the spike). `on_notice` runs on the main thread inside the window procedure: it
/// must not re-enter Slint synchronously (defer with `slint::Timer::single_shot(0)`).
pub fn watch(
    hwnd: HWND,
    on_notice: Box<dyn Fn(WindowNotice) + 'static>,
) -> windows::core::Result<()> {
    todo!("M1: subclass")
}
