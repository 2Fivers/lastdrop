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
//! - The context menu is a Win32 popup menu (`TrackPopupMenuEx` with `TPM_RETURNCMD`), shown after
//!   `SetForegroundWindow(hidden window)` and followed by `PostMessage(WM_NULL)` (the documented
//!   pattern so the menu closes when the user clicks elsewhere). Dark mode comes from
//!   [`crate::backdrop::allow_dark_menus`].
//! - [`Tray::icon_rect`] is `Shell_NotifyIconGetRect` (the flyout anchor, F§1.3). When the icon is
//!   in the overflow area Windows returns the overflow chevron's rect or fails; callers fall back
//!   to the cursor position.
//! - Dropping [`Tray`] sends `NIM_DELETE` and destroys the window.
//!
//! Threading: main thread only (the hidden window belongs to it and its messages are pumped by
//! Slint's event loop). `on_input` runs inside the window procedure and must not re-enter Slint
//! synchronously: defer with `slint::Timer::single_shot(Duration::ZERO, …)`.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use crate::geom::{Point, Rect};

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
}

/// A tray icon image: straight (non-premultiplied) RGBA, top-down rows, `size × size` px
/// ([`crate::trayicon::render`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayImage {
    /// Edge length in device px (16/20/24/32 at 100/125/150/200 %).
    pub size: u32,
    /// `size * size * 4` bytes.
    pub rgba: Vec<u8>,
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

/// The tray icon and its hidden window. Main thread only (`!Send`).
pub struct Tray {
    hwnd: isize,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Tray {
    /// Creates the hidden window, adds the icon with `image` and `tooltip`, switches it to
    /// version 4 and starts reporting [`TrayInput`]s to `on_input`.
    pub fn create(
        image: &TrayImage,
        tooltip: &str,
        on_input: Box<dyn Fn(TrayInput) + 'static>,
    ) -> windows::core::Result<Tray> {
        todo!("M1: hidden window, NIM_ADD, NIM_SETVERSION, TaskbarCreated")
    }

    /// Replaces the image (`NIM_MODIFY` with a new `HICON`; the previous icon is destroyed after
    /// the call succeeds). Call only when the image changed.
    pub fn set_image(&self, image: &TrayImage) -> windows::core::Result<()> {
        todo!("M1: CreateIconIndirect + NIM_MODIFY")
    }

    /// Replaces `szTip` (truncated to 127 UTF-16 units at a grapheme boundary).
    pub fn set_tooltip(&self, tooltip: &str) -> windows::core::Result<()> {
        todo!("M1: NIM_MODIFY NIF_TIP")
    }

    /// The icon's rectangle on screen in device px (`Shell_NotifyIconGetRect`), or `None` when
    /// Windows cannot tell (overflow closed): the caller falls back to the cursor position.
    pub fn icon_rect(&self) -> Option<Rect> {
        todo!("M1: Shell_NotifyIconGetRect")
    }

    /// Shows the context menu at `pt` and returns the chosen command id, or `None` when the menu
    /// was dismissed. Blocks in a modal menu loop on the main thread (Slint keeps painting
    /// through the same message pump).
    pub fn show_menu(&self, pt: Point, items: &[MenuItem]) -> Option<u32> {
        todo!("M1: CreatePopupMenu, TrackPopupMenuEx(TPM_RETURNCMD), WM_NULL")
    }

    /// The hidden window's handle, for `AllowSetForegroundWindow`/diagnostics.
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }
}

impl Drop for Tray {
    /// `NIM_DELETE`, destroys the current `HICON` and the hidden window.
    fn drop(&mut self) {}
}
