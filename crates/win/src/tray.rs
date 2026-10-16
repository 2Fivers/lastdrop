//! The notification-area icon, our own (R§9, R§10, F§1.4): `Shell_NotifyIconW` with
//! `NOTIFYICON_VERSION_4` on a hidden top-level window that this module owns.
//!
//! Why not the `tray-icon` crate: it registers the legacy callback version, so it never delivers
//! `NIN_POPUPOPEN`/`NIN_POPUPCLOSE` (the hover card), `NIN_KEYSELECT` (keyboard open) or
//! `WM_CONTEXTMENU` (Shift+F10 / Menu key), and it always shows `szTip` as a standard tooltip on
//! top of our hover card. Owning the window also gives us `TaskbarCreated` directly.
//!
//! Behaviour:
//! - `NIM_ADD` with a `uID` (not a GUID: Windows binds an icon GUID to the executable's path, so an
//!   unsigned build started from another folder would fail until M5 signing), then
//!   `NIM_SETVERSION` with `NOTIFYICON_VERSION_4`. `NIF_SHOWTIP` is never set, so `szTip` is only
//!   the accessible name and the fallback text (R§9.3).
//! - The hidden window is a normal top-level window (never `HWND_MESSAGE`, which does not receive
//!   broadcasts): on the registered `TaskbarCreated` message it re-adds the icon, sets the version
//!   again and re-applies the last image and tooltip.
//! - When `NIM_ADD` fails (the shell is not up yet, early in a sign-in) the icon is retried every
//!   2 s for a minute, and again on `TaskbarCreated`; [`Tray::create`] still succeeds.
//! - The context menu is a Win32 popup menu (`TrackPopupMenuEx` with `TPM_RETURNCMD`), shown after
//!   `SetForegroundWindow(hidden window)` and followed by `PostMessage(WM_NULL)` (the documented
//!   pattern so the menu closes when the user clicks elsewhere). Dark mode comes from
//!   [`crate::backdrop::allow_dark_menus`].
//! - [`Tray::icon_rect`] is `Shell_NotifyIconGetRect` (the flyout anchor, F§1.3). When the icon is
//!   in the overflow area Windows returns the overflow chevron's rect or fails; callers fall back
//!   to the cursor position.
//! - [`Tray::notify`] shows a notification from the icon (`NIF_INFO`, N§4): Windows shows it as a
//!   toast like any app notification (Do not disturb holds it back; `NIIF_RESPECT_QUIET_TIME`
//!   also during a new account's quiet time) and reports a click on it as `NIN_BALLOONUSERCLICK`
//!   ([`TrayInput::NotificationClicked`]); one that times out sends nothing we act on.
//! - Dropping [`Tray`] sends `NIM_DELETE` and destroys the window.
//!
//! Threading: main thread only (the hidden window belongs to it and its messages are pumped by
//! Slint's event loop). `on_input` runs inside the window procedure and must not re-enter Slint
//! synchronously: defer with `slint::Timer::single_shot(Duration::ZERO, …)`.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, TRUE,
    WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_RESPECT_QUIET_TIME, NIIF_WARNING,
    NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONUSERCLICK, NIN_POPUPCLOSE,
    NIN_POPUPOPEN, NIN_SELECT, NOTIFY_ICON_DATA_FLAGS, NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4,
    NOTIFYICONDATAW, NOTIFYICONIDENTIFIER, Shell_NotifyIconGetRect, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, ChangeWindowMessageFilterEx, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, GWLP_USERDATA, GetSystemMetrics,
    GetWindowLongPtrW, HICON, HMENU, ICONINFO, KillTimer, MF_CHECKED, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, MSGFLT_ALLOW, PostMessageW, RegisterClassExW, RegisterWindowMessageW,
    SM_MENUDROPALIGNMENT, SetForegroundWindow, SetMenuDefaultItem, SetTimer, SetWindowLongPtrW,
    TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON,
    TrackPopupMenuEx, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_NCDESTROY, WM_NULL, WM_TIMER,
    WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{Error, GUID, PCWSTR, w};

use crate::geom::{Point, Rect, rect_from_win};

/// What the user did to the tray icon (version-4 callbacks, R§10 "On the tray icon").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrayInput {
    /// `NIN_SELECT` (left click, button up; a double click counts as one): toggle the flyout,
    /// with the 300 ms debounce after a deactivation hide (F§1.4).
    Select,
    /// `NIN_KEYSELECT` (Enter/Space after Win+B and arrows): open the flyout with keyboard focus
    /// on the first card (F§1.4, F§9).
    KeySelect,
    /// `WM_CONTEXTMENU` (right click, Shift+F10, Menu key): show the context menu at `pt`.
    ContextMenu {
        /// Anchor point from the callback's `wParam`, device px.
        pt: Point,
    },
    /// `NIN_POPUPOPEN`: the pointer rested on the icon for the system hover time: show the hover
    /// card (R§8.3).
    PopupOpen {
        /// Anchor point from the callback's `wParam`, device px.
        pt: Point,
    },
    /// `NIN_POPUPCLOSE`: hide the hover card.
    PopupClose,
    /// The taskbar was re-created (Explorer restart) and the icon has been re-added; the app
    /// should re-render the icon in case the DPI changed.
    Recreated,
    /// `NIN_BALLOONUSERCLICK`: the user selected a notification of ours ([`Tray::notify`]), as a
    /// toast or in the notification centre: open the flyout (N§4).
    NotificationClicked,
}

/// The standard icon of a notification ([`Tray::notify`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NoticeIcon {
    /// `NIIF_INFO`.
    Info,
    /// `NIIF_WARNING`.
    Warning,
}

/// A tray icon image: straight (non-premultiplied) BGRA, top-down rows, `size × size` px
/// ([`crate::trayicon::render`]); the byte order of the icon's 32-bpp DIB, so it is copied as is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayImage {
    /// Edge length in device px (16/20/24/32 at 100/125/150/200 %).
    pub size: u32,
    /// `size * size * 4` bytes.
    pub bgra: Vec<u8>,
}

/// One entry of the context menu (R§10). Ids are the app's; `0` is never used (it means
/// "dismissed" from [`Tray::show_menu`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// A command.
    Command {
        /// Returned by [`Tray::show_menu`] when chosen; never 0.
        id: u32,
        /// Localized label (with `&` mnemonics if any).
        label: String,
        /// Shows a check mark (radio-style groups are drawn as checks).
        checked: bool,
        /// Greyed out when `false`.
        enabled: bool,
        /// The default (bold) item.
        default: bool,
    },
    /// A submenu.
    Submenu {
        /// Localized label.
        label: String,
        /// Its items.
        items: Vec<MenuItem>,
    },
    /// A separator line.
    Separator,
}

/// The icon's `uID` (one icon per window).
const ICON_ID: u32 = 1;
/// `uCallbackMessage`.
const CALLBACK_MESSAGE: u32 = WM_APP + 0x4C44; // "LD"
/// `NIN_KEYSELECT` = `NIN_SELECT | NINF_KEY` (not exported by the `windows` crate).
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;
/// The window class of the hidden window.
const CLASS_NAME: PCWSTR = w!("Lastdrop.Tray");
/// Timer id for retrying `NIM_ADD`.
const RETRY_TIMER: usize = 1;
/// Retry interval and count when `NIM_ADD` fails (the shell is still starting).
const RETRY_INTERVAL_MS: u32 = 2_000;
const MAX_ADD_ATTEMPTS: u32 = 30;
/// `szTip` holds 128 UTF-16 units including the terminating NUL.
const TIP_UNITS: usize = 128;
/// `szInfoTitle` holds 64 UTF-16 units including the terminating NUL.
const INFO_TITLE_UNITS: usize = 64;
/// `szInfo` holds 256 UTF-16 units including the terminating NUL.
const INFO_UNITS: usize = 256;

/// State shared by [`Tray`] and the window procedure (main thread only). The window holds one
/// strong reference (released on `WM_NCDESTROY`); the procedure takes another while it runs, so
/// dropping the `Tray` from inside `on_input` cannot free the state under it.
struct Shared {
    on_input: Box<dyn Fn(TrayInput) + 'static>,
    /// `RegisterWindowMessageW("TaskbarCreated")`, 0 if that failed.
    taskbar_created: u32,
    /// The current icon, owned (destroyed when replaced and on drop).
    hicon: Cell<HICON>,
    /// The current `szTip`.
    tip: Cell<[u16; TIP_UNITS]>,
    /// The icon is in the notification area.
    added: Cell<bool>,
    /// `NIM_ADD` attempts in the current retry run.
    add_attempts: Cell<u32>,
    /// The last delivered `NIN_SELECT` (double-click suppression).
    last_select: Cell<Option<Instant>>,
    /// The last delivered `NIN_KEYSELECT` (Enter is reported twice on some builds).
    last_key_select: Cell<Option<Instant>>,
}

/// The tray icon and its hidden window. Main thread only (`!Send`).
pub struct Tray {
    hwnd: isize,
    shared: Rc<Shared>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Tray {
    /// Creates the hidden window, adds the icon with `image` and `tooltip`, switches it to
    /// version 4 and starts reporting [`TrayInput`]s to `on_input`.
    ///
    /// Fails when the image is malformed or the window cannot be created. A failing `NIM_ADD` is
    /// not an error: the icon is retried (module docs).
    pub fn create(
        image: &TrayImage,
        tooltip: &str,
        on_input: Box<dyn Fn(TrayInput) + 'static>,
    ) -> windows::core::Result<Tray> {
        let hinstance = register_class()?;
        let hicon = create_hicon(image)?;
        // SAFETY: a static, NUL-terminated name.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        if taskbar_created == 0 {
            log::warn!(
                "tray: RegisterWindowMessage(TaskbarCreated) failed; no re-add after an Explorer restart"
            );
        }
        let shared = Rc::new(Shared {
            on_input,
            taskbar_created,
            hicon: Cell::new(hicon),
            tip: Cell::new(tip_units(tooltip)),
            added: Cell::new(false),
            add_attempts: Cell::new(0),
            last_select: Cell::new(None),
            last_key_select: Cell::new(None),
        });

        // SAFETY: the class was registered above with `wndproc`; all string pointers are static.
        // A zero-size WS_POPUP window is never shown; WS_EX_TOOLWINDOW keeps it out of the
        // taskbar and Alt+Tab even if something showed it.
        let created = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                CLASS_NAME,
                w!("Lastdrop"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance),
                None,
            )
        };
        let hwnd = match created {
            Ok(hwnd) => hwnd,
            Err(e) => {
                destroy_icon(hicon);
                return Err(e);
            }
        };
        let raw = Rc::into_raw(Rc::clone(&shared));
        // SAFETY: `hwnd` is our window; the pointer is a leaked strong reference that the window
        // procedure reclaims on WM_NCDESTROY.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize) };
        if taskbar_created != 0 {
            // SAFETY: plain call on our own window. Only matters when Lastdrop runs elevated
            // (UIPI would otherwise drop Explorer's broadcast); failure is harmless.
            let allowed =
                unsafe { ChangeWindowMessageFilterEx(hwnd, taskbar_created, MSGFLT_ALLOW, None) };
            if let Err(e) = allowed {
                log::debug!("tray: ChangeWindowMessageFilterEx failed: {e}");
            }
        }

        let tray = Tray {
            hwnd: hwnd.0 as isize,
            shared,
            _not_send: std::marker::PhantomData,
        };
        if let Err(e) = add_icon(hwnd, &tray.shared) {
            log::warn!("tray: NIM_ADD failed ({e}); retrying");
            start_retry(hwnd, &tray.shared);
        }
        Ok(tray)
    }

    /// Replaces the image (`NIM_MODIFY` with a new `HICON`; the previous icon is destroyed after
    /// the call succeeds). Call only when the image changed.
    pub fn set_image(&self, image: &TrayImage) -> windows::core::Result<()> {
        let new = create_hicon(image)?;
        if self.shared.added.get() {
            let mut nid = self.nid(NIF_ICON);
            nid.hIcon = new;
            if let Err(e) = notify(NIM_MODIFY, &nid) {
                destroy_icon(new);
                return Err(e);
            }
        }
        // Not added yet: the retry or TaskbarCreated uses the new icon.
        destroy_icon(self.shared.hicon.replace(new));
        Ok(())
    }

    /// Replaces `szTip` (truncated to 127 UTF-16 units at a grapheme boundary).
    pub fn set_tooltip(&self, tooltip: &str) -> windows::core::Result<()> {
        let tip = tip_units(tooltip);
        self.shared.tip.set(tip);
        if self.shared.added.get() {
            let mut nid = self.nid(NIF_TIP);
            nid.szTip = tip;
            notify(NIM_MODIFY, &nid)?;
        }
        Ok(())
    }

    /// Shows a notification from the icon (`NIM_MODIFY` with `NIF_INFO`, N§4): `title` (cut to
    /// 63 UTF-16 units) in bold over `body` (cut to 255; lines separated by `\n`), with the
    /// standard `icon`. Windows shows it like any app notification (Do not disturb holds it
    /// back). Fails when the icon is not in the notification area (the shell is not up
    /// yet): the notice is not kept for later. An empty `body` would remove a showing
    /// notification instead, so the title stands in for it.
    pub fn notify(&self, title: &str, body: &str, icon: NoticeIcon) -> windows::core::Result<()> {
        if !self.shared.added.get() {
            return Err(Error::new(
                E_FAIL,
                "the tray icon is not in the notification area",
            ));
        }
        let mut nid = self.nid(NIF_INFO);
        nid.szInfoTitle = units::<INFO_TITLE_UNITS>(title);
        nid.szInfo = units::<INFO_UNITS>(if body.is_empty() { title } else { body });
        nid.dwInfoFlags = NIIF_RESPECT_QUIET_TIME
            | match icon {
                NoticeIcon::Info => NIIF_INFO,
                NoticeIcon::Warning => NIIF_WARNING,
            };
        notify(NIM_MODIFY, &nid)
    }

    /// The icon's rectangle on screen in device px (`Shell_NotifyIconGetRect`), or `None` when
    /// Windows cannot tell (overflow closed): the caller falls back to the cursor position.
    pub fn icon_rect(&self) -> Option<Rect> {
        if !self.shared.added.get() {
            return None;
        }
        let id = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: self.win_hwnd(),
            uID: ICON_ID,
            guidItem: GUID::zeroed(),
        };
        // SAFETY: `id` is a fully initialized NOTIFYICONIDENTIFIER with its cbSize set.
        match unsafe { Shell_NotifyIconGetRect(&id) } {
            Ok(r) => {
                let rect = rect_from_win(r);
                (rect.width() > 0 && rect.height() > 0).then_some(rect)
            }
            Err(e) => {
                log::debug!("tray: Shell_NotifyIconGetRect failed: {e}");
                None
            }
        }
    }

    /// Shows the context menu at `pt` and returns the chosen command id, or `None` when the menu
    /// was dismissed. Blocks in a modal menu loop on the main thread (Slint keeps painting
    /// through the same message pump).
    pub fn show_menu(&self, pt: Point, items: &[MenuItem]) -> Option<u32> {
        let menu = match build_menu(items) {
            Ok(menu) => menu,
            Err(e) => {
                log::warn!("tray: building the context menu failed: {e}");
                return None;
            }
        };
        let hwnd = self.win_hwnd();
        // SAFETY: `hwnd` is our live window and `menu` a popup menu we own until DestroyMenu
        // below (which also destroys its submenus). SetForegroundWindow + WM_NULL is the
        // documented pattern for notification-icon menus.
        let chosen = unsafe {
            if !SetForegroundWindow(hwnd).as_bool() {
                log::debug!("tray: SetForegroundWindow before the menu was refused");
            }
            let horizontal = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
                TPM_RIGHTALIGN
            } else {
                TPM_LEFTALIGN
            };
            let flags =
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | horizontal;
            let chosen = TrackPopupMenuEx(menu, flags.0, pt.x, pt.y, hwnd, None);
            if let Err(e) = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)) {
                log::debug!("tray: posting WM_NULL after the menu failed: {e}");
            }
            if let Err(e) = DestroyMenu(menu) {
                log::warn!("tray: DestroyMenu failed: {e}");
            }
            chosen.0
        };
        u32::try_from(chosen).ok().filter(|&id| id != 0)
    }

    /// The hidden window's handle, for `AllowSetForegroundWindow`/diagnostics.
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    fn win_hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    /// A `NOTIFYICONDATAW` that names our icon, with `flags`.
    fn nid(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        base_nid(self.win_hwnd(), flags)
    }
}

impl Drop for Tray {
    /// `NIM_DELETE`, destroys the current `HICON` and the hidden window.
    fn drop(&mut self) {
        let hwnd = self.win_hwnd();
        if self.shared.added.replace(false)
            && let Err(e) = notify(NIM_DELETE, &self.nid(NOTIFY_ICON_DATA_FLAGS(0)))
        {
            log::warn!("tray: NIM_DELETE failed: {e}");
        }
        // SAFETY: our own window and timer id; KillTimer on a timer that is not running fails
        // harmlessly. DestroyWindow sends WM_NCDESTROY, which releases the window's reference to
        // `shared`.
        unsafe {
            let _ = KillTimer(Some(hwnd), RETRY_TIMER);
            if let Err(e) = DestroyWindow(hwnd) {
                log::warn!("tray: DestroyWindow failed: {e}");
            }
        }
        destroy_icon(self.shared.hicon.replace(HICON::default()));
    }
}

fn base_nid(hwnd: HWND, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: ICON_ID,
        uFlags: flags,
        ..Default::default()
    }
}

/// `Shell_NotifyIconW`, which reports failure as `FALSE` without a reliable last error.
fn notify(message: NOTIFY_ICON_MESSAGE, nid: &NOTIFYICONDATAW) -> windows::core::Result<()> {
    // SAFETY: `nid` is a fully initialized NOTIFYICONDATAW with cbSize set.
    if unsafe { Shell_NotifyIconW(message, nid) }.as_bool() {
        Ok(())
    } else {
        Err(Error::new(
            E_FAIL,
            format!("Shell_NotifyIconW({}) failed", message.0),
        ))
    }
}

/// `NIM_ADD` + `NIM_SETVERSION` with the current icon and tooltip.
fn add_icon(hwnd: HWND, shared: &Shared) -> windows::core::Result<()> {
    let mut nid = base_nid(hwnd, NIF_MESSAGE | NIF_ICON | NIF_TIP);
    nid.uCallbackMessage = CALLBACK_MESSAGE;
    nid.hIcon = shared.hicon.get();
    nid.szTip = shared.tip.get();
    if notify(NIM_ADD, &nid).is_err() {
        // A stale icon with our id (a TaskbarCreated that raced a pending add): replace it.
        let _ = notify(NIM_DELETE, &base_nid(hwnd, NOTIFY_ICON_DATA_FLAGS(0)));
        notify(NIM_ADD, &nid)?;
    }
    shared.added.set(true);
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    if let Err(e) = notify(NIM_SETVERSION, &nid) {
        // Without version 4 the icon still works, but sends legacy callbacks we do not decode.
        log::warn!("tray: NIM_SETVERSION 4 failed: {e}");
    }
    Ok(())
}

fn start_retry(hwnd: HWND, shared: &Shared) {
    shared.add_attempts.set(0);
    // SAFETY: a timer on our own window; WM_TIMER arrives in `wndproc`.
    if unsafe { SetTimer(Some(hwnd), RETRY_TIMER, RETRY_INTERVAL_MS, None) } == 0 {
        log::warn!("tray: SetTimer for the NIM_ADD retry failed; waiting for TaskbarCreated");
    }
}

fn register_class() -> windows::core::Result<HINSTANCE> {
    // SAFETY: GetModuleHandleW(None) returns the executable's module, valid for the process.
    let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None) }?.into();
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinstance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `class` is fully initialized; the class name is static.
    if unsafe { RegisterClassExW(&class) } == 0 {
        let e = Error::from_thread();
        if e.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
            return Err(e);
        }
    }
    Ok(hinstance)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: reading our own window's user data.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Shared;
    if ptr.is_null() {
        // SAFETY: default handling for messages before the state is attached.
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    if msg == WM_NCDESTROY {
        // SAFETY: the pointer came from Rc::into_raw in `create` and is released exactly once,
        // here, after the user data is cleared.
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Rc::from_raw(ptr));
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
    }
    // SAFETY: the window still holds its strong reference (released only on WM_NCDESTROY), so the
    // pointer is live; taking our own reference keeps it alive while callbacks run.
    let shared = unsafe {
        Rc::increment_strong_count(ptr);
        Rc::from_raw(ptr)
    };
    match msg {
        CALLBACK_MESSAGE => {
            if let Some(input) = decode_callback(wparam, lparam) {
                deliver(&shared, input);
            }
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == RETRY_TIMER => {
            retry_add(hwnd, &shared);
            return LRESULT(0);
        }
        // Only `Drop` destroys this window.
        WM_CLOSE => return LRESULT(0),
        m if m != 0 && m == shared.taskbar_created => {
            shared.added.set(false);
            match add_icon(hwnd, &shared) {
                Ok(()) => {
                    // SAFETY: our own window and timer id.
                    let _ = unsafe { KillTimer(Some(hwnd), RETRY_TIMER) };
                    (shared.on_input)(TrayInput::Recreated);
                }
                Err(e) => {
                    log::warn!("tray: re-adding the icon after TaskbarCreated failed ({e})");
                    start_retry(hwnd, &shared);
                }
            }
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: default handling of every other message of our own window.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn retry_add(hwnd: HWND, shared: &Shared) {
    let attempts = shared.add_attempts.get() + 1;
    shared.add_attempts.set(attempts);
    let stop = match add_icon(hwnd, shared) {
        Ok(()) => {
            log::info!("tray: icon added after {attempts} retries");
            (shared.on_input)(TrayInput::Recreated);
            true
        }
        Err(e) if attempts >= MAX_ADD_ATTEMPTS => {
            log::warn!("tray: giving up on NIM_ADD ({e}); waiting for TaskbarCreated");
            true
        }
        Err(_) => false,
    };
    if stop {
        // SAFETY: our own window and timer id.
        let _ = unsafe { KillTimer(Some(hwnd), RETRY_TIMER) };
    }
}

/// Applies the double-click and repeated-Enter suppression, then calls `on_input`.
fn deliver(shared: &Shared, input: TrayInput) {
    let now = Instant::now();
    // SAFETY: a plain system setting read, no pointers.
    let dbl_time = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
    match input {
        TrayInput::Select => {
            // Both clicks land on the same icon, so time alone tells a double click.
            if is_repeat(shared.last_select.get(), now, dbl_time) {
                shared.last_select.set(None);
                return;
            }
            shared.last_select.set(Some(now));
        }
        TrayInput::KeySelect => {
            if is_repeat(shared.last_key_select.get(), now, dbl_time) {
                return;
            }
            shared.last_key_select.set(Some(now));
        }
        _ => {}
    }
    (shared.on_input)(input);
}

/// A second event within the double-click time of the first is the same gesture.
fn is_repeat(previous: Option<Instant>, now: Instant, window: Duration) -> bool {
    previous.is_some_and(|t| now.saturating_duration_since(t) <= window)
}

/// Decodes a version-4 callback: `LOWORD(lParam)` is the event, `HIWORD(lParam)` the icon id,
/// `wParam` the anchor point (signed 16-bit x and y).
fn decode_callback(wparam: WPARAM, lparam: LPARAM) -> Option<TrayInput> {
    let event = (lparam.0 as u32) & 0xFFFF;
    let id = ((lparam.0 as u32) >> 16) & 0xFFFF;
    if id != ICON_ID {
        return None;
    }
    let pt = Point {
        x: i32::from((wparam.0 & 0xFFFF) as u16 as i16),
        y: i32::from(((wparam.0 >> 16) & 0xFFFF) as u16 as i16),
    };
    match event {
        NIN_SELECT => Some(TrayInput::Select),
        NIN_KEYSELECT => Some(TrayInput::KeySelect),
        WM_CONTEXTMENU => Some(TrayInput::ContextMenu { pt }),
        NIN_POPUPOPEN => Some(TrayInput::PopupOpen { pt }),
        NIN_POPUPCLOSE => Some(TrayInput::PopupClose),
        NIN_BALLOONUSERCLICK => Some(TrayInput::NotificationClicked),
        _ => None,
    }
}

/// `tooltip` as `szTip`: at most 127 UTF-16 units ([`units`]).
fn tip_units(tooltip: &str) -> [u16; TIP_UNITS] {
    units::<TIP_UNITS>(tooltip)
}

/// `text` as a fixed field of `N` UTF-16 units: at most `N − 1` units, cut at a grapheme boundary,
/// NUL-terminated. Embedded NULs are dropped (they would end the string early).
fn units<const N: usize>(text: &str) -> [u16; N] {
    let mut out = [0u16; N];
    let max = N.saturating_sub(1);
    let cleaned: Vec<char> = text.chars().filter(|&c| c != '\0').collect();
    // How many chars fit.
    let mut used = 0;
    let mut fit = 0;
    for c in &cleaned {
        if used + c.len_utf16() > max {
            break;
        }
        used += c.len_utf16();
        fit += 1;
    }
    if fit < cleaned.len() {
        fit = grapheme_cut(&cleaned, fit);
    }
    let mut i = 0;
    for c in &cleaned[..fit] {
        i += c.encode_utf16(&mut out[i..]).len();
    }
    out
}

/// Moves a cut at char index `cut` left until it does not split a grapheme cluster: never before
/// a combining mark, variation selector, emoji modifier, tag or ZWJ, never right after a ZWJ, and
/// never between the two halves of a regional-indicator flag. An approximation of UAX #29 that
/// covers the text Lastdrop puts in a tooltip.
fn grapheme_cut(chars: &[char], mut cut: usize) -> usize {
    fn extends(c: char) -> bool {
        matches!(c as u32,
            0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F | 0xFE00..=0xFE0F | 0x200C..=0x200D | 0x1F3FB..=0x1F3FF
            | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
    }
    fn regional(c: char) -> bool {
        (0x1F1E6..=0x1F1FF).contains(&(c as u32))
    }
    while cut > 0 && cut < chars.len() {
        if extends(chars[cut]) || chars[cut - 1] == '\u{200D}' {
            cut -= 1;
            continue;
        }
        if regional(chars[cut]) && regional(chars[cut - 1]) {
            let run = chars[..cut]
                .iter()
                .rev()
                .take_while(|&&c| regional(c))
                .count();
            if run % 2 == 1 {
                cut -= 1;
                continue;
            }
        }
        break;
    }
    cut
}

/// An `HICON` from straight BGRA (32-bpp DIB colour bitmap + 1-bpp AND mask from the alpha).
fn create_hicon(image: &TrayImage) -> windows::core::Result<HICON> {
    let size = image.size;
    let expected = (size as usize) * (size as usize) * 4;
    if size == 0 || size > 256 || image.bgra.len() != expected {
        return Err(Error::new(
            E_INVALIDARG,
            "tray image: size × size × 4 bytes expected",
        ));
    }
    let side = size as i32;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: side,
            biHeight: -side, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `info` describes a 32-bpp top-down DIB; `bits` receives the pointer to its pixels.
    let color = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }?;
    if bits.is_null() {
        delete_bitmap(color);
        return Err(Error::new(E_FAIL, "CreateDIBSection returned no pixels"));
    }
    // SAFETY: the DIB section owns `expected` bytes of pixel memory at `bits` (32 bpp, no row
    // padding at 4 bytes per pixel) until DeleteObject below.
    let pixels = unsafe { std::slice::from_raw_parts_mut(bits.cast::<u8>(), expected) };
    pixels.copy_from_slice(&image.bgra);

    let mask_bits = and_mask(&image.bgra, size as usize);
    // SAFETY: `mask_bits` holds a word-aligned 1-bpp bitmap of `size × size` (see `and_mask`).
    let mask = unsafe { CreateBitmap(side, side, 1, 1, Some(mask_bits.as_ptr().cast())) };
    if mask.is_invalid() {
        delete_bitmap(color);
        return Err(Error::from_thread());
    }
    let icon_info = ICONINFO {
        fIcon: TRUE,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    // SAFETY: both bitmaps are valid; CreateIconIndirect copies them.
    let icon = unsafe { CreateIconIndirect(&icon_info) };
    delete_bitmap(color);
    delete_bitmap(mask);
    icon
}

/// The 1-bpp AND mask (rows padded to 16 bits, as `CreateBitmap` wants): 1 where the pixel is
/// fully transparent. Drawing uses the alpha channel; the mask matters only where alpha is not
/// supported. `pixels` are 4 bytes each with the alpha last (BGRA).
fn and_mask(pixels: &[u8], size: usize) -> Vec<u8> {
    let stride = size.div_ceil(16) * 2;
    let mut mask = vec![0u8; stride * size];
    for y in 0..size {
        for x in 0..size {
            if pixels[(y * size + x) * 4 + 3] == 0 {
                mask[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    mask
}

fn delete_bitmap(bitmap: HBITMAP) {
    // SAFETY: a bitmap we created and no longer select anywhere.
    let _ = unsafe { DeleteObject(HGDIOBJ(bitmap.0)) };
}

fn destroy_icon(icon: HICON) {
    if icon.is_invalid() {
        return;
    }
    // SAFETY: an icon we created with CreateIconIndirect and no longer hand to the shell.
    if let Err(e) = unsafe { DestroyIcon(icon) } {
        log::warn!("tray: DestroyIcon failed: {e}");
    }
}

fn build_menu(items: &[MenuItem]) -> windows::core::Result<HMENU> {
    // SAFETY: creates an empty popup menu that we own.
    let menu = unsafe { CreatePopupMenu() }?;
    if let Err(e) = fill_menu(menu, items) {
        // SAFETY: our own menu (and the submenus already attached to it).
        let _ = unsafe { DestroyMenu(menu) };
        return Err(e);
    }
    Ok(menu)
}

fn fill_menu(menu: HMENU, items: &[MenuItem]) -> windows::core::Result<()> {
    for item in items {
        match item {
            MenuItem::Command {
                id,
                label,
                checked,
                enabled,
                default,
            } => {
                let mut flags = MF_STRING;
                if *checked {
                    flags |= MF_CHECKED;
                }
                if !*enabled {
                    flags |= MF_GRAYED;
                }
                let text = wide(label);
                // SAFETY: `menu` is ours; AppendMenuW copies the NUL-terminated label.
                unsafe { AppendMenuW(menu, flags, *id as usize, PCWSTR(text.as_ptr())) }?;
                if *default {
                    // SAFETY: `menu` is ours and contains the command `id`.
                    unsafe { SetMenuDefaultItem(menu, *id, 0) }?;
                }
            }
            MenuItem::Submenu { label, items } => {
                let sub = build_menu(items)?;
                let text = wide(label);
                // SAFETY: on success `menu` takes ownership of `sub` (destroyed with it); on
                // failure we destroy `sub` ourselves.
                unsafe {
                    if let Err(e) = AppendMenuW(
                        menu,
                        MF_POPUP | MF_STRING,
                        sub.0 as usize,
                        PCWSTR(text.as_ptr()),
                    ) {
                        let _ = DestroyMenu(sub);
                        return Err(e);
                    }
                }
            }
            MenuItem::Separator => {
                // SAFETY: `menu` is ours; a separator has no text.
                unsafe { AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null()) }?;
            }
        }
    }
    Ok(())
}

/// A NUL-terminated UTF-16 copy of `s` with embedded NULs dropped.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .filter(|&u| u != 0)
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tip_string(tip: &[u16; TIP_UNITS]) -> String {
        let len = tip.iter().position(|&u| u == 0).unwrap_or(TIP_UNITS);
        String::from_utf16(&tip[..len]).expect("valid UTF-16")
    }

    #[test]
    fn short_tooltips_are_kept_whole() {
        let text = "Lastdrop: Claude weekly 61% used, resets Mon 09:00";
        assert_eq!(tip_string(&tip_units(text)), text);
        assert_eq!(tip_string(&tip_units("")), "");
        assert_eq!(tip_string(&tip_units("a\0b")), "ab");
    }

    #[test]
    fn long_tooltips_stop_at_127_units() {
        let text = "x".repeat(300);
        let tip = tip_units(&text);
        assert_eq!(tip_string(&tip).len(), 127);
        assert_eq!(tip[127], 0);
    }

    #[test]
    fn truncation_never_splits_a_surrogate_pair_or_a_cluster() {
        // 126 ASCII + an astral char (2 units) does not fit in 127: the char is dropped whole.
        let text = format!("{}{}", "a".repeat(126), '\u{1F4A7}');
        assert_eq!(tip_string(&tip_units(&text)), "a".repeat(126));
        // A combining acute after the cut point takes its base letter with it.
        let text = format!("{}e\u{301}tc", "a".repeat(126));
        assert_eq!(tip_string(&tip_units(&text)), "a".repeat(126));
        // A ZWJ sequence cut in the middle is dropped back to before the ZWJ's left side.
        let family = "\u{1F468}\u{200D}\u{1F469}";
        let text = format!("{}{family}", "a".repeat(124));
        assert_eq!(tip_string(&tip_units(&text)), "a".repeat(124));
        // A flag (two regional indicators, 4 units) is never halved.
        let text = format!("{}\u{1F1F8}\u{1F1EA}", "a".repeat(125));
        assert_eq!(tip_string(&tip_units(&text)), "a".repeat(125));
    }

    #[test]
    fn notification_fields_are_cut_to_their_size() {
        let title: [u16; INFO_TITLE_UNITS] = units(&"t".repeat(100));
        assert_eq!(title.iter().position(|&u| u == 0), Some(63));
        let text = "Back to 0% used.\nClaude weekly limit is 82% used";
        let body: [u16; INFO_UNITS] = units(text);
        let len = body.iter().position(|&u| u == 0).unwrap_or(INFO_UNITS);
        assert_eq!(
            String::from_utf16(&body[..len]).expect("valid UTF-16"),
            text
        );
    }

    #[test]
    fn decodes_version_4_callbacks() {
        let l = |event: u32, id: u32| LPARAM(((id << 16) | event) as isize);
        let at = |x: i16, y: i16| WPARAM(((y as u16 as usize) << 16) | x as u16 as usize);
        assert_eq!(
            decode_callback(at(10, 20), l(NIN_SELECT, ICON_ID)),
            Some(TrayInput::Select)
        );
        assert_eq!(
            decode_callback(at(10, 20), l(NIN_KEYSELECT, ICON_ID)),
            Some(TrayInput::KeySelect)
        );
        assert_eq!(
            decode_callback(at(-1910, 1050), l(WM_CONTEXTMENU, ICON_ID)),
            Some(TrayInput::ContextMenu {
                pt: Point { x: -1910, y: 1050 }
            })
        );
        assert_eq!(
            decode_callback(at(1800, 1060), l(NIN_POPUPOPEN, ICON_ID)),
            Some(TrayInput::PopupOpen {
                pt: Point { x: 1800, y: 1060 }
            })
        );
        assert_eq!(
            decode_callback(at(0, 0), l(NIN_POPUPCLOSE, ICON_ID)),
            Some(TrayInput::PopupClose)
        );
        assert_eq!(
            decode_callback(at(0, 0), l(NIN_BALLOONUSERCLICK, ICON_ID)),
            Some(TrayInput::NotificationClicked)
        );
        // Mouse moves, other icons' ids and unknown events are ignored.
        assert_eq!(decode_callback(at(0, 0), l(0x0200, ICON_ID)), None);
        assert_eq!(decode_callback(at(0, 0), l(NIN_SELECT, 7)), None);
    }

    #[test]
    fn repeats_within_the_double_click_time_are_one_gesture() {
        let t0 = Instant::now();
        let window = Duration::from_millis(500);
        assert!(!is_repeat(None, t0, window));
        assert!(is_repeat(Some(t0), t0 + Duration::from_millis(300), window));
        assert!(!is_repeat(
            Some(t0),
            t0 + Duration::from_millis(501),
            window
        ));
    }

    #[test]
    fn and_mask_marks_transparent_pixels() {
        // 2 × 2: opaque, transparent / transparent, opaque.
        let bgra = [
            1, 2, 3, 255, 0, 0, 0, 0, //
            0, 0, 0, 0, 9, 9, 9, 128,
        ];
        let mask = and_mask(&bgra, 2);
        assert_eq!(mask, vec![0b0100_0000, 0, 0b1000_0000, 0]);
    }

    #[test]
    fn creates_icons_from_bgra_and_rejects_bad_sizes() {
        for size in [16u32, 20, 24, 32] {
            let mut bgra = vec![0u8; (size * size * 4) as usize];
            for px in bgra.as_chunks_mut::<4>().0.iter_mut().step_by(3) {
                *px = [0xA7, 0x3A, 0x8E, 0xFF];
            }
            let icon = create_hicon(&TrayImage { size, bgra }).expect("icon");
            assert!(!icon.is_invalid());
            destroy_icon(icon);
        }
        let bad = TrayImage {
            size: 16,
            bgra: vec![0; 10],
        };
        assert_eq!(create_hicon(&bad).map_err(|e| e.code()), Err(E_INVALIDARG));
    }

    #[test]
    fn builds_and_destroys_nested_menus() {
        let items = vec![
            MenuItem::Command {
                id: 1,
                label: "Refresh now".into(),
                checked: false,
                enabled: true,
                default: true,
            },
            MenuItem::Submenu {
                label: "Pause updates".into(),
                items: vec![
                    MenuItem::Command {
                        id: 2,
                        label: "For 1 hour".into(),
                        checked: true,
                        enabled: true,
                        default: false,
                    },
                    MenuItem::Separator,
                    MenuItem::Command {
                        id: 3,
                        label: "Until I resume".into(),
                        checked: false,
                        enabled: false,
                        default: false,
                    },
                ],
            },
            MenuItem::Separator,
        ];
        let menu = build_menu(&items).expect("menu");
        // SAFETY: test-owned menu.
        unsafe {
            assert_eq!(
                windows::Win32::UI::WindowsAndMessaging::GetMenuItemCount(Some(menu)),
                3
            );
            DestroyMenu(menu).expect("destroy");
        }
    }
}
