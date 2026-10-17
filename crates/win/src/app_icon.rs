//! The app icon on a window of ours: the dialogs' title bar, Alt+Tab and taskbar button. winit
//! registers its window class without an icon, so without this Windows shows its generic one.
//! The icon is the exe's icon resource 1 (`crates/app/lastdrop.rc`), loaded in the small and the
//! big size for the window's DPI, so Windows takes the hand-tuned image of that size
//! (`tools/icon/make_icon.py`) instead of scaling another one.
//!
//! Threading: the thread that owns the window. Windows does not copy an icon set with
//! `WM_SETICON`, so the loaded icons are kept for the life of the process: one pair per DPI.

use std::cell::RefCell;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    HICON, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXICON, SM_CXSMICON,
    SendMessageW, WM_SETICON,
};
use windows::core::PCWSTR;

/// The icon's resource id in `lastdrop.rc` (the first icon group, which Explorer shows too).
const ICON_ID: u16 = 1;

thread_local! {
    /// The loaded icons by DPI: (small, big).
    static ICONS: RefCell<Vec<(u32, HICON, HICON)>> = const { RefCell::new(Vec::new()) };
}

/// Gives `hwnd` (a window of this thread) the app icon with `WM_SETICON`, small and big. Fails
/// when the exe has no icon resource (a test binary); the window then keeps Windows' default.
pub fn set(hwnd: isize) -> windows::core::Result<()> {
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);
    // SAFETY: a plain query; it returns 0 for a handle that is not a window.
    let dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        dpi => dpi,
    };
    let (small, big) = icons(dpi)?;
    // SAFETY: our own window; the icons stay valid for the life of the process (ICONS).
    unsafe {
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_SMALL as usize)),
            Some(LPARAM(small.0 as isize)),
        );
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(big.0 as isize)),
        );
    }
    Ok(())
}

/// The small and the big icon for `dpi`, loaded once.
fn icons(dpi: u32) -> windows::core::Result<(HICON, HICON)> {
    if let Some(found) = ICONS.with(|i| {
        i.borrow()
            .iter()
            .find(|(d, ..)| *d == dpi)
            .map(|&(_, small, big)| (small, big))
    }) {
        return Ok(found);
    }
    // SAFETY: plain metrics queries.
    let (small_px, big_px) = unsafe {
        (
            GetSystemMetricsForDpi(SM_CXSMICON, dpi),
            GetSystemMetricsForDpi(SM_CXICON, dpi),
        )
    };
    let small = load(small_px)?;
    let big = load(big_px)?;
    ICONS.with(|i| i.borrow_mut().push((dpi, small, big)));
    Ok((small, big))
}

/// Loads the icon resource at `px` × `px`: the group's image of that size, else the nearest one
/// scaled.
fn load(px: i32) -> windows::core::Result<HICON> {
    // SAFETY: the module handle of this exe; the resource is named by its integer id
    // (MAKEINTRESOURCE); not LR_SHARED, because the sizes depend on the DPI.
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let handle = LoadImageW(
            Some(instance),
            PCWSTR(usize::from(ICON_ID) as *const u16),
            IMAGE_ICON,
            px,
            px,
            LR_DEFAULTCOLOR,
        )?;
        Ok(HICON(handle.0))
    }
}
