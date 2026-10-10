//! Single instance: the `Local\Lastdrop` mutex; a second launch asks the first to open the flyout
//! and exits (ARCHITECTURE "Single instance").
//!
//! The first instance creates a message-only listener window (class [`LISTENER_CLASS`]) on the
//! main thread; the second finds it with `FindWindowExW(HWND_MESSAGE, …)` and posts the
//! registered message [`ACTIVATE_MESSAGE`].
//!
//! Threading: [`acquire`] and [`signal_first`] run first thing in `main`; the listener lives on
//! the main thread and its callback runs there.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

/// The mutex name (per session).
pub const MUTEX_NAME: &str = r"Local\Lastdrop";

/// The registered window message (`RegisterWindowMessageW`) that asks for the flyout.
pub const ACTIVATE_MESSAGE: &str = "Lastdrop.OpenFlyout";

/// The listener window class.
pub const LISTENER_CLASS: &str = "Lastdrop.Instance";

/// Held by the first instance for its lifetime; releases the mutex on drop.
pub struct InstanceGuard {
    #[expect(dead_code, reason = "used once the handle is implemented (M1)")]
    mutex: isize,
}

/// The outcome of [`acquire`].
pub enum Instance {
    /// This is the first instance.
    First(InstanceGuard),
    /// Another instance is running.
    Second,
}

/// Creates or opens the mutex (`CreateMutexW`, `ERROR_ALREADY_EXISTS` → [`Instance::Second`]).
pub fn acquire() -> std::io::Result<Instance> {
    todo!("M1: CreateMutexW")
}

/// From a second instance: asks the first to open its flyout. `Ok(false)` when no listener was
/// found (the first is still starting). Calls `AllowSetForegroundWindow` with the first
/// instance's process id (`GetWindowThreadProcessId` of the listener) before posting: the second
/// instance holds the foreground right (the user just launched it), and without passing it on the
/// first instance's `SetForegroundWindow` fails, so the flyout would open inactive and never hide
/// on focus loss (F§1.4).
pub fn signal_first() -> std::io::Result<bool> {
    todo!("M1: FindWindowExW(HWND_MESSAGE) + PostMessageW")
}

/// The listener window; destroyed on drop.
pub struct Listener {
    hwnd: isize,
}

impl InstanceGuard {
    /// Creates the listener window on the calling thread (the main thread, whose Slint/winit loop
    /// dispatches its messages). `on_activate` runs on the main thread when a second instance
    /// signals.
    pub fn listen(&self, on_activate: Box<dyn Fn() + 'static>) -> windows::core::Result<Listener> {
        todo!("M1: register class + CreateWindowExW(HWND_MESSAGE)")
    }
}

impl Listener {
    /// The listener's window handle as `isize` (diagnostics).
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }
}
