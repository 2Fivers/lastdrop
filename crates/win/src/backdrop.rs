//! Window styling and show/hide for the Slint windows (flyout, hover card): DWM backdrop,
//! corners, dark mode, tool-window and no-activate styles, and the spike's keep-alive show/hide
//! (F§1.1, F§1.4, R§8.1).
//!
//! Threading: main thread (the Slint windows' thread). Win32 is called on the raw `HWND` behind
//! Slint's back, as in the spike, because winit always activates a window it shows.

use std::rc::Rc;

use windows::Win32::Foundation::{
    E_FAIL, HWND, LPARAM, LRESULT, SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWM_SYSTEMBACKDROP_TYPE, DWM_WINDOW_CORNER_PREFERENCE, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW,
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE,
    DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWINDOWATTRIBUTE, DwmExtendFrameIntoClientArea,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{RDW_INTERNALPAINT, RDW_INVALIDATE, RedrawWindow};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetWindowLongPtrW, HWND_TOPMOST, SET_WINDOW_POS_FLAGS, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
    SWP_SHOWWINDOW, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, WA_INACTIVE,
    WINDOW_LONG_PTR_INDEX, WM_ACTIVATE, WM_DPICHANGED, WM_NCACTIVATE, WM_NCDESTROY,
    WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
    WS_SYSMENU,
};
use windows::core::{BOOL, Error, PCSTR, w};

use crate::geom::Rect;
use crate::theme::{ThemeChange, classify};

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
///
/// Every step is attempted; the first failure is returned (the window is still usable, only
/// without that part of the styling).
pub fn apply(hwnd: HWND, style: &PopupStyle) -> windows::core::Result<()> {
    let mut result = Ok(());
    let mut keep = |r: windows::core::Result<()>| {
        if let Err(e) = r {
            log::warn!("backdrop: window setup step failed: {e}");
            if result.is_ok() {
                result = Err(e);
            }
        }
    };

    // SAFETY: style reads on a window of this thread.
    let (ex, st) = unsafe {
        (
            GetWindowLongPtrW(hwnd, GWL_EXSTYLE),
            GetWindowLongPtrW(hwnd, GWL_STYLE),
        )
    };
    let mut ex = (ex | WS_EX_TOOLWINDOW.0 as isize) & !(WS_EX_APPWINDOW.0 as isize);
    if style.no_activate {
        ex |= WS_EX_NOACTIVATE.0 as isize;
    } else {
        ex &= !(WS_EX_NOACTIVATE.0 as isize);
    }
    keep(set_long(hwnd, GWL_EXSTYLE, ex));
    let st = st & !((WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0) as isize);
    keep(set_long(hwnd, GWL_STYLE, st));

    let corner = if style.round_corners {
        DWMWCP_ROUND
    } else {
        DWMWCP_DONOTROUND
    };
    keep(set_attribute::<DWM_WINDOW_CORNER_PREFERENCE>(
        hwnd,
        DWMWA_WINDOW_CORNER_PREFERENCE,
        corner,
    ));
    keep(set_theme(hwnd, style.dark, style.backdrop));
    let margins = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // SAFETY: `margins` is a live MARGINS for the duration of the call.
    keep(unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins) });
    // Topmost (WS_EX_TOPMOST is only honoured through SetWindowPos) and the frame change that
    // makes the style edits take effect.
    keep(set_pos(
        hwnd,
        None,
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
    ));
    hide(hwnd);
    result
}

/// Updates dark mode and backdrop after a theme change.
pub fn set_theme(hwnd: HWND, dark: bool, backdrop: Backdrop) -> windows::core::Result<()> {
    let dark_result = set_attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, BOOL::from(dark));
    let kind = match backdrop {
        Backdrop::Acrylic => DWMSBT_TRANSIENTWINDOW,
        Backdrop::None => DWMSBT_NONE,
    };
    let backdrop_result =
        set_attribute::<DWM_SYSTEMBACKDROP_TYPE>(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, kind);
    dark_result.and(backdrop_result)
}

/// Moves and sizes the window (device px) without activating it.
pub fn set_rect(hwnd: HWND, rect: Rect) {
    if let Err(e) = set_pos(hwnd, Some(rect), SWP_NOACTIVATE | SWP_NOOWNERZORDER) {
        log::warn!("backdrop: moving the window failed: {e}");
    }
}

/// Shows without activation: `ShowWindow(SW_SHOWNOACTIVATE)` + `SetWindowPos(HWND_TOPMOST, …,
/// SWP_NOACTIVATE)` + `RedrawWindow(RDW_INVALIDATE | RDW_INTERNALPAINT)` (the hover card;
/// `GetForegroundWindow()` must be unchanged afterwards).
pub fn show_no_activate(hwnd: HWND) {
    // SAFETY: a window of this thread. ShowWindow's return value is the previous visibility,
    // not an error.
    let _ = unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
    if let Err(e) = set_pos(hwnd, None, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) {
        log::warn!("backdrop: raising the window failed: {e}");
    }
    // As in `show_and_activate`: a re-shown window does not always get a WM_PAINT (DWM keeps the
    // old surface); ask for one so Slint renders right away.
    // SAFETY: a window of this thread.
    if !unsafe { RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_INTERNALPAINT) }
        .as_bool()
    {
        log::debug!("backdrop: RedrawWindow failed");
    }
}

/// Shows and activates (the flyout, F§1.4): `SetWindowPos(HWND_TOPMOST, … SWP_SHOWWINDOW)`,
/// `SetForegroundWindow` (allowed: the process just received the click), then
/// `RedrawWindow(RDW_INVALIDATE | RDW_INTERNALPAINT)` so Slint renders at once.
pub fn show_and_activate(hwnd: HWND) {
    if let Err(e) = set_pos(hwnd, None, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW) {
        log::warn!("backdrop: showing the window failed: {e}");
    }
    // SAFETY: a window of this thread.
    unsafe {
        if !SetForegroundWindow(hwnd).as_bool() {
            // The flyout then opens inactive and will not hide on focus loss (F§1.4); usually
            // a missing AllowSetForegroundWindow from a second instance.
            log::warn!("backdrop: SetForegroundWindow was refused");
        }
        // Re-showing a hidden window does not always produce a WM_PAINT (DWM keeps the old
        // surface); ask for one so Slint renders right away.
        if !RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_INTERNALPAINT).as_bool() {
            log::debug!("backdrop: RedrawWindow failed");
        }
    }
}

/// Hides (`SW_HIDE`).
pub fn hide(hwnd: HWND) {
    // SAFETY: a window of this thread; the return value is the previous visibility.
    let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
}

/// Lets the tray's Win32 popup menus follow dark mode (uxtheme ordinals 135 `SetPreferredAppMode`
/// and 136 `FlushMenuThemes`, as in the spike). Call once at start-up.
///
/// Undocumented but stable since Windows 10 1903; Lastdrop runs on Windows 11 only. When the
/// exports are missing nothing happens (menus stay light).
pub fn allow_dark_menus() {
    /// `PreferredAppMode::AllowDark`.
    const ALLOW_DARK: i32 = 1;
    // SAFETY: loads the system uxtheme.dll from System32 only; it stays loaded (user32 already
    // depends on it), so the function pointers stay valid.
    let module =
        match unsafe { LoadLibraryExW(w!("uxtheme.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) } {
            Ok(module) => module,
            Err(e) => {
                log::warn!("backdrop: uxtheme.dll not loaded ({e}); menus stay light");
                return;
            }
        };
    // SAFETY: lookups by ordinal (MAKEINTRESOURCE: the ordinal in the low word of the pointer).
    let (set_mode, flush) = unsafe {
        (
            GetProcAddress(module, PCSTR(135usize as *const u8)),
            GetProcAddress(module, PCSTR(136usize as *const u8)),
        )
    };
    match set_mode {
        Some(f) => {
            // SAFETY: ordinal 135 of uxtheme.dll is `PreferredAppMode WINAPI
            // SetPreferredAppMode(PreferredAppMode)` (an int enum) on Windows 10 1903 and later.
            let set_mode: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(f) };
            // SAFETY: called with a valid enum value.
            unsafe { set_mode(ALLOW_DARK) };
        }
        None => log::warn!("backdrop: SetPreferredAppMode not found; menus stay light"),
    }
    if let Some(f) = flush {
        // SAFETY: ordinal 136 of uxtheme.dll is `void WINAPI FlushMenuThemes(void)`.
        let flush: unsafe extern "system" fn() = unsafe { std::mem::transmute(f) };
        // SAFETY: no arguments.
        unsafe { flush() };
    }
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

/// The subclass id ("LDBD").
const SUBCLASS_ID: usize = 0x4C44_4244;

type NoticeFn = Box<dyn Fn(WindowNotice) + 'static>;

/// Subclasses `hwnd` (`SetWindowSubclass`) to report [`WindowNotice`]s and to keep DWM drawing
/// the active backdrop while the window is shown without activation (`WM_NCACTIVATE` with
/// `TRUE`, as in the spike). `on_notice` runs on the main thread inside the window procedure: it
/// must not re-enter Slint synchronously (defer with `slint::Timer::single_shot(0)`).
///
/// Watching the same window again replaces the callback. The callback is released when the
/// window is destroyed.
pub fn watch(
    hwnd: HWND,
    on_notice: Box<dyn Fn(WindowNotice) + 'static>,
) -> windows::core::Result<()> {
    let raw = Rc::into_raw(Rc::new(on_notice)) as usize;
    // SAFETY: subclassing a window of this thread with our own procedure and id. The reference
    // data is a leaked `Rc<NoticeFn>` that `subclass_proc` releases on WM_NCDESTROY.
    if !unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, raw) }.as_bool() {
        // SAFETY: reclaims the reference leaked above, which Windows did not take.
        drop(unsafe { Rc::from_raw(raw as *const NoticeFn) });
        return Err(Error::new(E_FAIL, "SetWindowSubclass failed"));
    }
    // A replaced callback is released here (`GetWindowSubclass` would tell us, but the
    // system32 comctl32 that runs without a v6 manifest, as tests do, exports it by ordinal only).
    if let Some(previous) = WATCHED.with_borrow_mut(|w| w.insert(hwnd.0 as isize, raw))
        && previous != raw
    {
        // SAFETY: `previous` was the reference data of this subclass until the call above
        // replaced it; the procedure holds its own clone while it runs.
        drop(unsafe { Rc::from_raw(previous as *const NoticeFn) });
    }
    Ok(())
}

thread_local! {
    /// Watched windows of this thread → their current reference data.
    static WATCHED: std::cell::RefCell<std::collections::HashMap<isize, usize>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    let ptr = data as *const NoticeFn;
    if msg == WM_NCDESTROY {
        WATCHED.with_borrow_mut(|w| w.remove(&(hwnd.0 as isize)));
        // SAFETY: removing our own subclass; `data` came from Rc::into_raw in `watch` and is
        // released exactly once, here.
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), id);
            if !ptr.is_null() {
                drop(Rc::from_raw(ptr));
            }
            return DefSubclassProc(hwnd, msg, wparam, lparam);
        }
    }
    if msg == WM_NCACTIVATE {
        // Keep DWM drawing the backdrop in its active state even while shown without activation.
        // SAFETY: forwarding to the next procedure in the chain with a valid BOOL.
        return unsafe { DefSubclassProc(hwnd, msg, WPARAM(1), lparam) };
    }
    if let Some(notice) = notice_for(msg, wparam, lparam)
        && !ptr.is_null()
    {
        // SAFETY: the subclass still owns its reference (released only on WM_NCDESTROY or when
        // replaced by `watch`); our own clone keeps the callback alive while it runs.
        let callback = unsafe {
            Rc::increment_strong_count(ptr);
            Rc::from_raw(ptr)
        };
        callback(notice);
    }
    // SAFETY: forwarding the unchanged message to the next procedure in the chain.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// The notice a message stands for, if any.
fn notice_for(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<WindowNotice> {
    match msg {
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => Some(WindowNotice::Deactivated),
        // LOWORD(wParam) is the new X-axis DPI (equal to the Y-axis DPI on Windows).
        WM_DPICHANGED => Some(WindowNotice::DpiChanged((wparam.0 & 0xFFFF) as u32)),
        _ => classify(msg, wparam, lparam).map(WindowNotice::Theme),
    }
}

/// `SetWindowLongPtrW`, whose 0 return is only an error when the last error says so.
fn set_long(hwnd: HWND, index: WINDOW_LONG_PTR_INDEX, value: isize) -> windows::core::Result<()> {
    // SAFETY: setting a style long on a window of this thread.
    unsafe {
        SetLastError(WIN32_ERROR(0));
        if SetWindowLongPtrW(hwnd, index, value) == 0 {
            let e = Error::from_thread();
            if e.code().is_err() {
                return Err(e);
            }
        }
    }
    Ok(())
}

fn set_attribute<T: Copy>(
    hwnd: HWND,
    attribute: DWMWINDOWATTRIBUTE,
    value: T,
) -> windows::core::Result<()> {
    // SAFETY: `value` is a live 4-byte value of the type the attribute expects (BOOL, the
    // corner-preference enum or the backdrop enum) and its size is passed.
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute,
            std::ptr::from_ref(&value).cast(),
            size_of::<T>() as u32,
        )
    }
}

/// `SetWindowPos(HWND_TOPMOST, …)` to `rect` (or keeping position and size with `None`).
fn set_pos(
    hwnd: HWND,
    rect: Option<Rect>,
    flags: SET_WINDOW_POS_FLAGS,
) -> windows::core::Result<()> {
    let r = rect.unwrap_or_default();
    // SAFETY: positioning a window of this thread.
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            r.left,
            r.top,
            r.width(),
            r.height(),
            flags,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use windows::Win32::Foundation::RECT;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow, IsWindowVisible,
        RegisterClassExW, SendMessageW, WINDOW_EX_STYLE, WM_SETTINGCHANGE, WM_SYSCOLORCHANGE,
        WNDCLASSEXW, WS_EX_TOPMOST, WS_POPUP,
    };

    use super::*;

    unsafe extern "system" fn test_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        // SAFETY: default handling for the test window.
        unsafe { DefWindowProcW(h, m, w, l) }
    }

    /// A hidden 10 × 10 popup far off-screen, so a test that shows it shows nothing to the user.
    fn test_window(ex: WINDOW_EX_STYLE) -> HWND {
        // SAFETY: registering a test class (re-registration fails harmlessly) and creating a
        // window of this thread.
        unsafe {
            let instance = GetModuleHandleW(None).expect("module").into();
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(test_proc),
                hInstance: instance,
                lpszClassName: w!("Lastdrop.Test.Backdrop"),
                ..Default::default()
            };
            RegisterClassExW(&class);
            CreateWindowExW(
                ex,
                w!("Lastdrop.Test.Backdrop"),
                w!("backdrop test"),
                WS_POPUP | WS_SYSMENU,
                -32000,
                -32000,
                10,
                10,
                None,
                None,
                Some(instance),
                None,
            )
            .expect("test window")
        }
    }

    #[test]
    fn apply_sets_tool_window_no_activate_topmost_and_stays_hidden() {
        let hwnd = test_window(WS_EX_APPWINDOW);
        let style = PopupStyle {
            backdrop: Backdrop::Acrylic,
            round_corners: true,
            dark: true,
            no_activate: true,
        };
        // DWM attributes can fail on a session without composition (CI); the styles must not.
        let _ = apply(hwnd, &style);
        // SAFETY: test-owned window.
        unsafe {
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            let st = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
            assert_ne!(ex & WS_EX_TOOLWINDOW.0, 0);
            assert_ne!(ex & WS_EX_NOACTIVATE.0, 0);
            assert_ne!(ex & WS_EX_TOPMOST.0, 0);
            assert_eq!(ex & WS_EX_APPWINDOW.0, 0);
            assert_eq!(st & WS_SYSMENU.0, 0);
            assert!(!IsWindowVisible(hwnd).as_bool());
            DestroyWindow(hwnd).expect("destroy");
        }
    }

    #[test]
    fn show_no_activate_leaves_the_foreground_alone() {
        let hwnd = test_window(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE);
        // SAFETY: test-owned window, shown off-screen for an instant.
        unsafe {
            let before = GetForegroundWindow();
            show_no_activate(hwnd);
            assert!(IsWindowVisible(hwnd).as_bool());
            assert_eq!(GetForegroundWindow(), before);
            assert_ne!(
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            set_rect(
                hwnd,
                Rect {
                    left: -31000,
                    top: -31000,
                    right: -30980,
                    bottom: -30990,
                },
            );
            assert_eq!(GetForegroundWindow(), before);
            hide(hwnd);
            assert!(!IsWindowVisible(hwnd).as_bool());
            DestroyWindow(hwnd).expect("destroy");
        }
    }

    #[test]
    fn watch_reports_notices_and_releases_the_callback_on_destroy() {
        let hwnd = test_window(WS_EX_TOOLWINDOW);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        watch(hwnd, Box::new(move |n| sink.borrow_mut().push(n))).expect("watch");
        assert_eq!(Rc::strong_count(&seen), 2);
        let wide: Vec<u16> = "ImmersiveColorSet\0".encode_utf16().collect();
        let mut suggested = RECT {
            left: -32000,
            top: -32000,
            right: -31985,
            bottom: -31985,
        };
        // SAFETY: test-owned window; every lParam is what the message's contract says.
        unsafe {
            SendMessageW(
                hwnd,
                WM_ACTIVATE,
                Some(WPARAM(WA_INACTIVE as usize)),
                Some(LPARAM(0)),
            );
            SendMessageW(
                hwnd,
                WM_DPICHANGED,
                Some(WPARAM((144 << 16) | 144)),
                Some(LPARAM(std::ptr::from_mut(&mut suggested) as isize)),
            );
            SendMessageW(
                hwnd,
                WM_SETTINGCHANGE,
                Some(WPARAM(0)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
            SendMessageW(hwnd, WM_SYSCOLORCHANGE, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        assert_eq!(
            *seen.borrow(),
            vec![
                WindowNotice::Deactivated,
                WindowNotice::DpiChanged(144),
                WindowNotice::Theme(ThemeChange::ColorSet),
                WindowNotice::Theme(ThemeChange::SysColors),
            ]
        );
        // Re-watching replaces (and releases) the first callback.
        let other = Rc::clone(&seen);
        watch(hwnd, Box::new(move |_| drop(Rc::clone(&other)))).expect("watch again");
        assert_eq!(Rc::strong_count(&seen), 2);
        // SAFETY: test-owned window.
        unsafe { DestroyWindow(hwnd).expect("destroy") };
        assert_eq!(
            Rc::strong_count(&seen),
            1,
            "callback released on WM_NCDESTROY"
        );
    }

    #[test]
    fn dark_menus_can_be_allowed_twice() {
        allow_dark_menus();
        allow_dark_menus();
    }
}
