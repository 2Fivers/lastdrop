//! Single instance and the small messages between Lastdrop's two processes (ARCHITECTURE
//! "Processes", "Single instance").
//!
//! - **The app** (the tray process): the `Local\Lastdrop` mutex; a second launch asks the first
//!   to open the flyout and exits. The first instance creates a message-only listener window
//!   (class [`LISTENER_CLASS`]) on the main thread; the second finds it with
//!   `FindWindowExW(HWND_MESSAGE, …)` and posts the registered message [`ACTIVATE_MESSAGE`].
//! - **Dialogs** (the short-lived dialog process: settings, about): the same pattern with its own
//!   mutex ([`DIALOG_MUTEX_NAME`]), class ([`DIALOG_LISTENER_CLASS`]) and message
//!   ([`DIALOG_MESSAGE`]). The message's `WPARAM` says which dialog a second launch wants; the
//!   app crate defines the values.
//! - **Settings changed**: the tray process listens (class [`SETTINGS_CHANGED_CLASS`]) for
//!   [`SETTINGS_CHANGED_MESSAGE`], which the dialog process posts after it wrote
//!   `settings.json` or the Meshy key. The `WPARAM` carries flags the app crate defines.
//!
//! Threading: [`acquire`], [`acquire_dialog`] and the `signal_*` calls run first thing in `main`
//! (or on the dialog process's main thread); listeners live on the main thread of their process
//! and their callbacks run there.

use std::rc::Rc;

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_CLASS_ALREADY_EXISTS, GetLastError, HANDLE, HINSTANCE,
    HWND, LPARAM, LRESULT, SetLastError, WIN32_ERROR, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW,
    DestroyWindow, FindWindowExW, GWLP_USERDATA, GetWindowLongPtrW, GetWindowThreadProcessId,
    HWND_MESSAGE, MSGFLT_ALLOW, PostMessageW, RegisterClassExW, RegisterWindowMessageW,
    SetWindowLongPtrW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_NCDESTROY, WNDCLASSEXW,
};
use windows::core::{Error, PCWSTR};

/// The app's mutex name (per session).
pub const MUTEX_NAME: &str = r"Local\Lastdrop";

/// The registered window message (`RegisterWindowMessageW`) that asks for the flyout.
pub const ACTIVATE_MESSAGE: &str = "Lastdrop.OpenFlyout";

/// The app's listener window class.
pub const LISTENER_CLASS: &str = "Lastdrop.Instance";

/// The dialog process's mutex name (per session).
pub const DIALOG_MUTEX_NAME: &str = r"Local\Lastdrop.Dialog";

/// The registered message that asks the running dialog process for a dialog (`WPARAM`: which).
pub const DIALOG_MESSAGE: &str = "Lastdrop.OpenDialog";

/// The dialog process's listener window class.
pub const DIALOG_LISTENER_CLASS: &str = "Lastdrop.Dialog";

/// The registered message that tells the tray process what the dialog process changed
/// (`WPARAM`: flags).
pub const SETTINGS_CHANGED_MESSAGE: &str = "Lastdrop.SettingsChanged";

/// The tray process's settings-changed listener window class.
pub const SETTINGS_CHANGED_CLASS: &str = "Lastdrop.SettingsListener";

/// Held by the first instance for its lifetime; releases the mutex on drop.
pub struct InstanceGuard {
    mutex: isize,
    /// The listener class and message of this mutex's process kind (app or dialog).
    class: &'static str,
    message: &'static str,
}

/// The outcome of [`acquire`] and [`acquire_dialog`].
pub enum Instance {
    /// This is the first instance.
    First(InstanceGuard),
    /// Another instance is running.
    Second,
}

/// Creates or opens the app's mutex (`CreateMutexW`, `ERROR_ALREADY_EXISTS` →
/// [`Instance::Second`]).
pub fn acquire() -> std::io::Result<Instance> {
    acquire_named(MUTEX_NAME, LISTENER_CLASS, ACTIVATE_MESSAGE)
}

/// Creates or opens the dialog process's mutex, like [`acquire`]. The guard's listener is the
/// dialog listener ([`InstanceGuard::listen_for`]).
pub fn acquire_dialog() -> std::io::Result<Instance> {
    acquire_named(DIALOG_MUTEX_NAME, DIALOG_LISTENER_CLASS, DIALOG_MESSAGE)
}

/// From a second instance: asks the first to open its flyout. `Ok(false)` when no listener was
/// found (the first is still starting). Calls `AllowSetForegroundWindow` with the first
/// instance's process id (`GetWindowThreadProcessId` of the listener) before posting: the second
/// instance holds the foreground right (the user just launched it), and without passing it on the
/// first instance's `SetForegroundWindow` fails, so the flyout would open inactive and never hide
/// on focus loss (F§1.4).
pub fn signal_first() -> std::io::Result<bool> {
    signal_named(LISTENER_CLASS, ACTIVATE_MESSAGE, 0, true)
}

/// From a second dialog launch: asks the running dialog process for the dialog `request` names
/// (its `WPARAM`). Passes the foreground right on like [`signal_first`], so the dialog can come
/// to the front. `Ok(false)` when no dialog listener was found.
pub fn signal_dialog(request: usize) -> std::io::Result<bool> {
    signal_named(DIALOG_LISTENER_CLASS, DIALOG_MESSAGE, request, true)
}

/// From the dialog process: tells the tray process what changed (`flags`, its `WPARAM`). The
/// foreground right stays with the dialog. `Ok(false)` when the tray process is not running.
pub fn signal_settings_changed(flags: usize) -> std::io::Result<bool> {
    signal_named(
        SETTINGS_CHANGED_CLASS,
        SETTINGS_CHANGED_MESSAGE,
        flags,
        false,
    )
}

/// The tray process's settings-changed listener on the calling thread (the main thread):
/// `on_change` runs there with the flags of each [`signal_settings_changed`].
pub fn listen_settings_changed(
    on_change: Box<dyn Fn(usize) + 'static>,
) -> windows::core::Result<Listener> {
    listen_named(SETTINGS_CHANGED_CLASS, SETTINGS_CHANGED_MESSAGE, on_change)
}

/// A listener window; destroyed on drop.
pub struct Listener {
    hwnd: isize,
}

impl InstanceGuard {
    /// Creates the listener window on the calling thread (the main thread, whose Slint/winit loop
    /// dispatches its messages). `on_activate` runs on the main thread when a second instance
    /// signals.
    pub fn listen(&self, on_activate: Box<dyn Fn() + 'static>) -> windows::core::Result<Listener> {
        listen_named(self.class, self.message, Box::new(move |_| on_activate()))
    }

    /// Like [`InstanceGuard::listen`], with the message's `WPARAM` (the dialog a second dialog
    /// launch asks for).
    pub fn listen_for(
        &self,
        on_message: Box<dyn Fn(usize) + 'static>,
    ) -> windows::core::Result<Listener> {
        listen_named(self.class, self.message, on_message)
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateMutexW and is closed exactly once. The mutex was
        // created unowned, so closing the last handle is all "release" means.
        if let Err(e) = unsafe { CloseHandle(HANDLE(self.mutex as *mut _)) } {
            log::warn!("single instance: closing the mutex failed: {e}");
        }
    }
}

impl Listener {
    /// The listener's window handle as `isize` (diagnostics).
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // SAFETY: our own window, destroyed once; WM_NCDESTROY releases the callback.
        if let Err(e) = unsafe { DestroyWindow(HWND(self.hwnd as *mut _)) } {
            log::warn!("single instance: destroying the listener failed: {e}");
        }
    }
}

/// A `windows` error as an `io::Error` with the Win32 code when there is one.
fn io_error(e: Error) -> std::io::Error {
    match WIN32_ERROR::from_error(&e) {
        Some(code) => std::io::Error::from_raw_os_error(code.0 as i32),
        None => std::io::Error::other(e),
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Creates or opens the mutex `name`; the first instance's guard listens on `class`/`message`.
fn acquire_named(
    name: &str,
    class: &'static str,
    message: &'static str,
) -> std::io::Result<Instance> {
    let name = wide(name);
    // SAFETY: clear the last error so ERROR_ALREADY_EXISTS below can only come from this call;
    // the name is NUL-terminated; default security, not initially owned.
    let (mutex, last) = unsafe {
        SetLastError(WIN32_ERROR(0));
        let mutex = CreateMutexW(None, false, PCWSTR(name.as_ptr())).map_err(io_error)?;
        (mutex, GetLastError())
    };
    if last == ERROR_ALREADY_EXISTS {
        // SAFETY: our handle to the other instance's mutex; we do not need it.
        let _ = unsafe { CloseHandle(mutex) };
        return Ok(Instance::Second);
    }
    Ok(Instance::First(InstanceGuard {
        mutex: mutex.0 as isize,
        class,
        message,
    }))
}

/// Posts `message` with `wparam` to the message-only window of `class`; `pass_foreground` first
/// passes this process's foreground right to the window's process (`AllowSetForegroundWindow`).
/// `Ok(false)` when no such window exists or the post failed.
fn signal_named(
    class: &str,
    message: &str,
    wparam: usize,
    pass_foreground: bool,
) -> std::io::Result<bool> {
    let (class, message) = (wide(class), wide(message));
    // SAFETY: searches message-only windows by a NUL-terminated class name.
    let found = unsafe {
        FindWindowExW(
            Some(HWND_MESSAGE),
            None,
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
        )
    };
    let hwnd = match found {
        Ok(hwnd) if !hwnd.is_invalid() => hwnd,
        _ => return Ok(false),
    };
    // SAFETY: a NUL-terminated message name.
    let msg = unsafe { RegisterWindowMessageW(PCWSTR(message.as_ptr())) };
    if msg == 0 {
        return Err(std::io::Error::last_os_error());
    }
    if pass_foreground {
        let mut pid = 0u32;
        // SAFETY: `hwnd` is a window handle (possibly already gone, then the call returns 0) and
        // `pid` a live out-parameter.
        let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if thread != 0 && pid != 0 {
            // SAFETY: plain call. Failure only means the window may open inactive; still signal.
            if let Err(e) = unsafe { AllowSetForegroundWindow(pid) } {
                log::warn!("single instance: AllowSetForegroundWindow failed: {e}");
            }
        }
    }
    // SAFETY: posting a registered message without pointers in its parameters.
    match unsafe { PostMessageW(Some(hwnd), msg, WPARAM(wparam), LPARAM(0)) } {
        Ok(()) => Ok(true),
        Err(e) => {
            log::warn!("single instance: posting to the other process failed: {e}");
            Ok(false)
        }
    }
}

/// The listener's state, owned by its window (released on `WM_NCDESTROY`).
struct ListenerState {
    message: u32,
    on_message: Box<dyn Fn(usize) + 'static>,
}

/// Creates a message-only window of `class` on the calling thread; `on_message` runs with the
/// `WPARAM` of each registered `message` it receives.
fn listen_named(
    class: &str,
    message: &str,
    on_message: Box<dyn Fn(usize) + 'static>,
) -> windows::core::Result<Listener> {
    let (class, message) = (wide(class), wide(message));
    // SAFETY: a NUL-terminated message name.
    let msg = unsafe { RegisterWindowMessageW(PCWSTR(message.as_ptr())) };
    if msg == 0 {
        return Err(Error::from_thread());
    }
    // SAFETY: the executable's module handle, valid for the process.
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }?.into();
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(listener_proc),
        hInstance: instance,
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    // SAFETY: `wc` is fully initialized; RegisterClassExW copies the class name.
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
            PCWSTR(class.as_ptr()),
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
    let state = Rc::new(ListenerState {
        message: msg,
        on_message,
    });
    // SAFETY: `hwnd` is ours; the leaked reference is reclaimed on WM_NCDESTROY.
    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::into_raw(state) as isize) };
    // SAFETY: plain call on our own window. Lets a non-elevated second launch reach an elevated
    // first instance; failure is harmless otherwise.
    if let Err(e) = unsafe { ChangeWindowMessageFilterEx(hwnd, msg, MSGFLT_ALLOW, None) } {
        log::debug!("single instance: ChangeWindowMessageFilterEx failed: {e}");
    }
    Ok(Listener {
        hwnd: hwnd.0 as isize,
    })
}

unsafe extern "system" fn listener_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: reading our own window's user data.
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const ListenerState;
    if !ptr.is_null() {
        if msg == WM_NCDESTROY {
            // SAFETY: the pointer came from Rc::into_raw in `listen_named` and is released once.
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Rc::from_raw(ptr));
            }
        } else {
            // SAFETY: the window still owns its reference; our clone keeps the state alive while
            // the callback runs (it may drop the Listener).
            let state = unsafe {
                Rc::increment_strong_count(ptr);
                Rc::from_raw(ptr)
            };
            if msg == state.message {
                (state.on_message)(wparam.0);
                return LRESULT(0);
            }
        }
    }
    // SAFETY: default handling for everything else.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::time::{Duration, Instant};

    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    use super::*;

    /// Names unique to this test run, so the tests never touch a running Lastdrop.
    fn scratch(tag: &str) -> String {
        format!("Lastdrop.Test.{tag}.{}", std::process::id())
    }

    /// Pumps this thread's queue until `done` or 5 s.
    fn pump_until(done: &dyn Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() && Instant::now() < deadline {
            let mut msg = MSG::default();
            // SAFETY: a standard message pump on this thread.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_second_acquire_sees_the_first() {
        let name = format!(r"Local\{}", scratch("Mutex"));
        let acquire = || acquire_named(&name, LISTENER_CLASS, ACTIVATE_MESSAGE);
        let Instance::First(guard) = acquire().expect("first") else {
            panic!("the scratch mutex already existed");
        };
        assert!(matches!(acquire(), Ok(Instance::Second)));
        drop(guard);
        assert!(matches!(acquire(), Ok(Instance::First(_))));
    }

    #[test]
    fn a_signal_reaches_the_listener() {
        let class = scratch("Listener");
        let message = scratch("Open");
        assert!(!signal_named(&class, &message, 0, true).expect("no listener yet"));

        let hits = Rc::new(Cell::new(0));
        let counter = Rc::clone(&hits);
        let listener = listen_named(
            &class,
            &message,
            Box::new(move |_| counter.set(counter.get() + 1)),
        )
        .expect("listener");
        assert!(signal_named(&class, &message, 0, true).expect("signal"));

        // Pump this thread's queue until the posted message has been handled.
        pump_until(&|| hits.get() > 0);
        assert_eq!(hits.get(), 1);

        drop(listener);
        assert_eq!(
            Rc::strong_count(&hits),
            1,
            "callback released with the window"
        );
        assert!(!signal_named(&class, &message, 0, true).expect("listener gone"));
    }

    #[test]
    fn the_wparam_reaches_the_listener() {
        let class = scratch("DialogListener");
        let message = scratch("OpenDialog");
        let seen = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&seen);
        let listener = listen_named(
            &class,
            &message,
            Box::new(move |wparam| sink.borrow_mut().push(wparam)),
        )
        .expect("listener");
        // A dialog request (passes the foreground right) and a settings-changed flag set (keeps
        // it), as the dialog process and the tray process exchange them.
        assert!(signal_named(&class, &message, 2, true).expect("signal"));
        assert!(signal_named(&class, &message, 3, false).expect("signal"));
        pump_until(&|| seen.borrow().len() >= 2);
        assert_eq!(*seen.borrow(), [2, 3]);
        drop(listener);
    }

    #[test]
    fn a_dialog_guard_listens_on_the_dialog_channel() {
        let name = format!(r"Local\{}", scratch("DialogMutex"));
        let class: &'static str = Box::leak(scratch("DialogGuardListener").into_boxed_str());
        let message: &'static str = Box::leak(scratch("DialogGuardOpen").into_boxed_str());
        let Instance::First(guard) = acquire_named(&name, class, message).expect("first") else {
            panic!("the scratch mutex already existed");
        };
        assert!(matches!(
            acquire_named(&name, class, message),
            Ok(Instance::Second)
        ));
        let seen = Rc::new(Cell::new(0));
        let sink = Rc::clone(&seen);
        let listener = guard
            .listen_for(Box::new(move |wparam| sink.set(wparam)))
            .expect("listener");
        assert!(signal_named(class, message, 3, true).expect("signal"));
        pump_until(&|| seen.get() != 0);
        assert_eq!(seen.get(), 3);
        drop(listener);
        drop(guard);
    }

    #[test]
    fn the_three_channels_never_share_a_name() {
        let names = [
            MUTEX_NAME,
            ACTIVATE_MESSAGE,
            LISTENER_CLASS,
            DIALOG_MUTEX_NAME,
            DIALOG_MESSAGE,
            DIALOG_LISTENER_CLASS,
            SETTINGS_CHANGED_MESSAGE,
            SETTINGS_CHANGED_CLASS,
        ];
        for (i, a) in names.iter().enumerate() {
            for b in &names[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert!(DIALOG_MUTEX_NAME.starts_with(r"Local\"));
    }
}
