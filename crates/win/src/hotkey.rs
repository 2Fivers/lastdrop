//! The optional global hotkey that toggles the flyout (R§10 "Global hotkey: optional, off by
//! default (settings)"): `RegisterHotKey` on a message-only window of the main thread, `WM_HOTKEY`
//! → `on_press`.
//!
//! Threading: main thread; `on_press` runs in the window procedure and must not re-enter Slint
//! synchronously (defer with `slint::Timer::single_shot(0)`).

use std::rc::Rc;

use lastdrop_core::settings::Hotkey;
use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, ERROR_HOTKEY_ALREADY_REGISTERED, HINSTANCE, HWND, LPARAM, LRESULT,
    WIN32_ERROR, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey, VK_F1,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HWND_MESSAGE,
    RegisterClassExW, SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_HOTKEY, WM_NCDESTROY,
    WNDCLASSEXW,
};
use windows::core::{Error, PCWSTR, w};

/// Why a hotkey could not be registered.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    /// `Hotkey::key` is not a letter, a digit or `F1`…`F24`, or no modifier is set (or Shift is
    /// the only modifier of a letter or digit, which would swallow typing).
    #[error("unsupported key: {0}")]
    Unsupported(String),
    /// Another program owns the combination (`ERROR_HOTKEY_ALREADY_REGISTERED`); the settings
    /// window says so.
    #[error("the shortcut is already in use")]
    InUse,
    /// Any other Win32 failure.
    #[error("{0}")]
    Win32(String),
}

/// A registered hotkey; unregister it with [`HotkeyRegistration::unregister`] before quitting or
/// when the setting changes. Dropping it unregisters too.
pub struct HotkeyRegistration {
    hwnd: isize,
}

/// The hotkey id on our window (one hotkey per window).
const HOTKEY_ID: i32 = 1;
const CLASS_NAME: PCWSTR = w!("Lastdrop.Hotkey");

/// Registers `hotkey` (`MOD_NOREPEAT` added) and calls `on_press` on each press.
pub fn register(
    hotkey: &Hotkey,
    on_press: Box<dyn Fn() + 'static>,
) -> Result<HotkeyRegistration, HotkeyError> {
    let (modifiers, vk) = parse(hotkey)?;
    let hwnd = create_window(on_press).map_err(|e| HotkeyError::Win32(e.message()))?;
    // SAFETY: registers the hotkey on our own window of this thread.
    if let Err(e) = unsafe { RegisterHotKey(Some(hwnd), HOTKEY_ID, modifiers | MOD_NOREPEAT, vk) } {
        // SAFETY: our window, destroyed once (WM_NCDESTROY releases the callback).
        let _ = unsafe { DestroyWindow(hwnd) };
        return Err(
            if WIN32_ERROR::from_error(&e) == Some(ERROR_HOTKEY_ALREADY_REGISTERED) {
                HotkeyError::InUse
            } else {
                HotkeyError::Win32(e.message())
            },
        );
    }
    Ok(HotkeyRegistration {
        hwnd: hwnd.0 as isize,
    })
}

impl HotkeyRegistration {
    /// `UnregisterHotKey` and destroys the window.
    pub fn unregister(self) {
        drop(self);
    }
}

impl Drop for HotkeyRegistration {
    fn drop(&mut self) {
        let hwnd = HWND(self.hwnd as *mut _);
        // SAFETY: our window and hotkey id; each is released once.
        unsafe {
            if let Err(e) = UnregisterHotKey(Some(hwnd), HOTKEY_ID) {
                log::warn!("hotkey: UnregisterHotKey failed: {e}");
            }
            if let Err(e) = DestroyWindow(hwnd) {
                log::warn!("hotkey: DestroyWindow failed: {e}");
            }
        }
    }
}

/// Modifiers and virtual-key code of `hotkey`.
fn parse(hotkey: &Hotkey) -> Result<(HOT_KEY_MODIFIERS, u32), HotkeyError> {
    let key = hotkey.key.trim();
    let unsupported = || HotkeyError::Unsupported(hotkey.key.clone());
    let mut chars = key.chars();
    let (vk, typing_key) = match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphabetic() => (u32::from(c.to_ascii_uppercase()), true),
        (Some(c), None) if c.is_ascii_digit() => (u32::from(c), true),
        (Some('F' | 'f'), Some(_)) => {
            let n: u32 = key[1..].parse().map_err(|_| unsupported())?;
            if !(1..=24).contains(&n) || key[1..].starts_with('0') {
                return Err(unsupported());
            }
            (u32::from(VK_F1.0) + n - 1, false)
        }
        _ => return Err(unsupported()),
    };
    let mut modifiers = HOT_KEY_MODIFIERS(0);
    for (on, flag) in [
        (hotkey.ctrl, MOD_CONTROL),
        (hotkey.alt, MOD_ALT),
        (hotkey.shift, MOD_SHIFT),
        (hotkey.win, MOD_WIN),
    ] {
        if on {
            modifiers |= flag;
        }
    }
    if modifiers.0 == 0 || (typing_key && modifiers == MOD_SHIFT) {
        return Err(unsupported());
    }
    Ok((modifiers, vk))
}

fn create_window(on_press: Box<dyn Fn() + 'static>) -> windows::core::Result<HWND> {
    // SAFETY: the executable's module handle, valid for the process.
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }?.into();
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(hotkey_proc),
        hInstance: instance,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialized with a static class name.
    if unsafe { RegisterClassExW(&wc) } == 0 {
        let e = Error::from_thread();
        if e.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
            return Err(e);
        }
    }
    // SAFETY: a message-only window of the registered class on this thread.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )
    }?;
    let callback: Rc<Box<dyn Fn() + 'static>> = Rc::new(on_press);
    // SAFETY: our window; the leaked reference is reclaimed on WM_NCDESTROY.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(callback) as isize) };
    Ok(hwnd)
}

unsafe extern "system" fn hotkey_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    type Callback = Box<dyn Fn() + 'static>;
    // SAFETY: reading our own window's user data.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Callback;
    if !ptr.is_null() {
        if msg == WM_NCDESTROY {
            // SAFETY: the pointer came from Rc::into_raw in `create_window`; released once.
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Rc::from_raw(ptr));
            }
        } else if msg == WM_HOTKEY && wparam.0 == HOTKEY_ID as usize {
            // SAFETY: the window still owns its reference; our clone keeps the callback alive
            // while it runs (it may drop the registration).
            let callback = unsafe {
                Rc::increment_strong_count(ptr);
                Rc::from_raw(ptr)
            };
            callback();
            return LRESULT(0);
        }
    }
    // SAFETY: default handling for everything else.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use windows::Win32::UI::WindowsAndMessaging::SendMessageW;

    use super::*;

    fn key(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> Hotkey {
        Hotkey {
            ctrl,
            alt,
            shift,
            win,
            key: key.to_owned(),
        }
    }

    #[test]
    fn parses_letters_digits_and_function_keys() {
        assert_eq!(
            parse(&key(true, true, false, false, "l")),
            Ok((MOD_CONTROL | MOD_ALT, u32::from(b'L')))
        );
        assert_eq!(
            parse(&key(false, false, false, true, "7")),
            Ok((MOD_WIN, u32::from(b'7')))
        );
        assert_eq!(
            parse(&key(true, false, false, false, "F1")),
            Ok((MOD_CONTROL, 0x70))
        );
        assert_eq!(
            parse(&key(false, false, true, false, "F24")),
            Ok((MOD_SHIFT, 0x87))
        );
    }

    #[test]
    fn rejects_what_cannot_be_a_shortcut() {
        for bad in [
            key(false, false, false, false, "L"), // no modifier
            key(false, false, true, false, "L"),  // Shift alone would swallow capitals
            key(false, false, true, false, "7"),  // ... and symbols
            key(true, false, false, false, ""),
            key(true, false, false, false, "LL"),
            key(true, false, false, false, "F0"),
            key(true, false, false, false, "F25"),
            key(true, false, false, false, "F01"),
            key(true, false, false, false, "Ä"),
            key(true, false, false, false, "Space"),
        ] {
            assert!(
                matches!(parse(&bad), Err(HotkeyError::Unsupported(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn registers_reports_in_use_and_unregisters() {
        // Ctrl+Alt+Shift+F24: no keyboard sends it, so the test cannot collide with real use.
        let combo = key(true, true, true, false, "F24");
        let presses = Rc::new(Cell::new(0));
        let counter = Rc::clone(&presses);
        let registration = match register(&combo, Box::new(move || counter.set(counter.get() + 1)))
        {
            Ok(r) => r,
            // Another program on this machine owns it; nothing to test here.
            Err(HotkeyError::InUse) => return,
            Err(e) => panic!("register: {e}"),
        };
        assert_eq!(
            register(&combo, Box::new(|| {})).err(),
            Some(HotkeyError::InUse)
        );
        // Simulate the press the system would deliver.
        // SAFETY: our own window, a WM_HOTKEY without pointers.
        unsafe {
            SendMessageW(
                HWND(registration.hwnd as *mut _),
                WM_HOTKEY,
                Some(WPARAM(HOTKEY_ID as usize)),
                Some(LPARAM(0)),
            );
        }
        assert_eq!(presses.get(), 1);
        registration.unregister();
        assert_eq!(Rc::strong_count(&presses), 1, "callback released");
        // Free again after unregistering.
        let again = register(&combo, Box::new(|| {})).expect("re-register");
        again.unregister();
    }
}
