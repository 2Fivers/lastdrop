//! `lastdrop-win`: the Win32 layer. It draws what `lastdrop-core` decided and talks to the shell;
//! it contains no formatting or business logic (ARCHITECTURE "Windows layer").
//!
//! - [`theme`]: taskbar/app light-dark, accent shades, transparency, high contrast; change
//!   notifications.
//! - [`palette`]: `ColorRole` → ARGB per theme, including high contrast (R§7, F§7).
//! - [`render`]: Direct2D + DirectWrite on a WIC premultiplied-BGRA bitmap (software), grayscale
//!   text antialiasing; text measuring for `core::readout::layout`.
//! - [`trayicon`]: renders a `TrayModel` to straight BGRA at 16/20/24/32 px (R§9).
//! - [`tray`]: our own notification-area icon (`Shell_NotifyIconW`, `NOTIFYICON_VERSION_4`): hover
//!   card on `NIN_POPUPOPEN`, `NIN_KEYSELECT`, `WM_CONTEXTMENU`, the context menu, `TaskbarCreated`
//!   (R§8.3, R§10).
//! - [`readout`]: the readout thread and its handle (R§6, R§10–R§12).
//! - [`placement`]: pure `place(geometry, size, prefs) -> Placement` (R§4.2, R§6.1), tested
//!   against recorded layouts in `tests/layouts/`.
//! - [`occupancy`]: the UI Automation thread that measures the taskbar's app buttons.
//! - [`flyout_pos`]: flyout and hover-card rectangles from an anchor (F§1.3, R§8.1).
//! - [`backdrop`]: DWM backdrop, corners, dark mode, tool-window and no-activate show/hide.
//! - [`autostart`], [`credentials`], [`single_instance`], [`locale`], [`network`], [`hotkey`]
//!   (optional global hotkey, R§10), [`shell`] (usage page in the browser, vendor sign-in in a
//!   console, F§4.3, F§9), [`idle`] (time since the last input, for the adaptive refresh),
//!   [`folder_picker`] (the settings window's Add folder…, accounts-spec §7).
//!
//! # Threading rules (ARCHITECTURE "Threads")
//!
//! | Thread | Owns | Never does |
//! |---|---|---|
//! | **main** | Slint event loop: flyout and hover card; the tray icon ([`tray`]); the app state | blocking I/O |
//! | **readout** | the taskbar readout: a hidden top-level window that receives `TaskbarCreated`, the layered readout window, the WinEvent hooks on the taskbar, its own message loop and drawing | I/O, waiting on locks held by other threads, synchronous cross-process calls while holding a lock |
//! | **occupancy** | a UI Automation client (MTA) that measures where the taskbar's app buttons are, with connection and transaction timeouts; set up on the first measurement | anything else |
//! | **refresh workers** | provider fetches (files, HTTP, `codex app-server`), one job per credential source at a time | touching UI objects |
//!
//! The table is the tray process's. The dialog process (settings and about windows, its own
//! single-instance mutex, [`single_instance`]) runs only a main thread with the Slint event loop
//! and the log's writer thread.
//!
//! Why a separate readout thread: a window attached to Explorer's taskbar can tie its thread's
//! input queue to Explorer's; a slow frame or a blocked lock there could freeze the taskbar's
//! input. The readout thread does nothing but draw a small model and handle a handful of
//! messages.
//!
//! Messages between threads: workers send results to the main thread with
//! `slint::invoke_from_event_loop`; the main thread hands the readout a new immutable
//! `ReadoutModel` (an `Arc`) and `PostMessage`s it to wake up ([`readout::ReadoutHandle::update`]);
//! the readout posts user actions back through its event sink, which the app forwards with
//! `slint::invoke_from_event_loop`. **No lock is ever held across a Win32 call.** Geometry is
//! read with `GetWindowRect`, never `SHAppBarMessage` (which can deadlock when Explorer hangs).
//!
//! COM objects (Direct2D, DirectWrite, WIC, UI Automation) live on the thread that created them;
//! window handles cross threads only as `isize` and only to `PostMessage` to them.
//!
//! `log::*` calls are allowed on every thread, the readout thread included: the app's logger only
//! queues the line for its own writer thread (`lastdrop_app::logger`), so logging never blocks on
//! the file or on a lock another thread holds.

pub mod autostart;
pub mod backdrop;
pub mod credentials;
pub mod flyout_pos;
pub mod folder_picker;
pub mod geom;
pub mod hotkey;
pub mod idle;
pub mod install;
pub mod locale;
pub mod network;
pub mod occupancy;
pub mod palette;
pub mod placement;
pub mod readout;
pub mod render;
pub mod shell;
pub mod single_instance;
pub mod theme;
pub mod tray;
pub mod trayicon;
