//! The optional global hotkey that toggles the flyout (R§10 "Global hotkey: optional, off by
//! default (settings)"): `RegisterHotKey` on a message-only window of the main thread, `WM_HOTKEY`
//! → `on_press`.
//!
//! Threading: main thread; `on_press` runs in the window procedure and must not re-enter Slint
//! synchronously (defer with `slint::Timer::single_shot(0)`).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use lastdrop_core::settings::Hotkey;

/// Why a hotkey could not be registered.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    /// `Hotkey::key` is not a letter, a digit or `F1`…`F24`, or no modifier is set.
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
/// when the setting changes.
pub struct HotkeyRegistration {
    #[expect(dead_code, reason = "used once the registration is implemented (M1)")]
    hwnd: isize,
}

/// Registers `hotkey` (`MOD_NOREPEAT` added) and calls `on_press` on each press.
pub fn register(
    hotkey: &Hotkey,
    on_press: Box<dyn Fn() + 'static>,
) -> Result<HotkeyRegistration, HotkeyError> {
    todo!("M1: RegisterHotKey on a message-only window")
}

impl HotkeyRegistration {
    /// `UnregisterHotKey` and destroys the window.
    pub fn unregister(self) {
        todo!("M1: UnregisterHotKey")
    }
}
