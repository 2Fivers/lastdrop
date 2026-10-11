//! Single instance: the `Local\Lastdrop` mutex; a second launch asks the first to open the flyout
//! and exits (ARCHITECTURE "Single instance").
//!
//! The first instance creates a message-only listener window (class [`LISTENER_CLASS`]) on the
//! main thread; the second finds it with `FindWindowExW(HWND_MESSAGE, …)` and posts the
//! registered message [`ACTIVATE_MESSAGE`].
//!
//! Threading: [`acquire`] and [`signal_first`] run first thing in `main`; the listener lives on
//! the main thread and its callback runs there.

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

/// The mutex name (per session).
pub const MUTEX_NAME: &str = r"Local\Lastdrop";

/// The registered window message (`RegisterWindowMessageW`) that asks for the flyout.
pub const ACTIVATE_MESSAGE: &str = "Lastdrop.OpenFlyout";

/// The listener window class.
pub const LISTENER_CLASS: &str = "Lastdrop.Instance";

/// Held by the first instance for its lifetime; releases the mutex on drop.
pub struct InstanceGuard {
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
    acquire_named(MUTEX_NAME)
}

/// From a second instance: asks the first to open its flyout. `Ok(false)` when no listener was
/// found (the first is still starting). Calls `AllowSetForegroundWindow` with the first
/// instance's process id (`GetWindowThreadProcessId` of the listener) before posting: the second
/// instance holds the foreground right (the user just launched it), and without passing it on the
/// first instance's `SetForegroundWindow` fails, so the flyout would open inactive and never hide
/// on focus loss (F§1.4).
pub fn signal_first() -> std::io::Result<bool> {
    signal_named(LISTENER_CLASS, ACTIVATE_MESSAGE)
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
        listen_named(LISTENER_CLASS, ACTIVATE_MESSAGE, on_activate)
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

fn acquire_named(name: &str) -> std::io::Result<Instance> {
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
    }))
}

fn signal_named(class: &str, message: &str) -> std::io::Result<bool> {
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
    let mut pid = 0u32;
    // SAFETY: `hwnd` is a window handle (possibly already gone, then the call returns 0) and
    // `pid` a live out-parameter.
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if thread != 0 && pid != 0 {
        // SAFETY: plain call. Failure only means the flyout may open inactive; still signal.
        if let Err(e) = unsafe { AllowSetForegroundWindow(pid) } {
            log::warn!("single instance: AllowSetForegroundWindow failed: {e}");
        }
    }
    // SAFETY: posting a registered message without pointers in its parameters.
    match unsafe { PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0)) } {
        Ok(()) => Ok(true),
        Err(e) => {
            log::warn!("single instance: posting to the first instance failed: {e}");
            Ok(false)
        }
    }
}

/// The listener's state, owned by its window (released on `WM_NCDESTROY`).
struct ListenerState {
    message: u32,
    on_activate: Box<dyn Fn() + 'static>,
}

fn listen_named(
    class: &str,
    message: &str,
    on_activate: Box<dyn Fn() + 'static>,
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
        on_activate,
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
                (state.on_activate)();
                return LRESULT(0);
            }
        }
    }
    // SAFETY: default handling for everything else.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
    };

    use super::*;

    /// Names unique to this test run, so the tests never touch a running Lastdrop.
    fn scratch(tag: &str) -> String {
        format!("Lastdrop.Test.{tag}.{}", std::process::id())
    }

    #[test]
    fn the_second_acquire_sees_the_first() {
        let name = format!(r"Local\{}", scratch("Mutex"));
        let Instance::First(guard) = acquire_named(&name).expect("first") else {
            panic!("the scratch mutex already existed");
        };
        assert!(matches!(acquire_named(&name), Ok(Instance::Second)));
        drop(guard);
        assert!(matches!(acquire_named(&name), Ok(Instance::First(_))));
    }

    #[test]
    fn a_signal_reaches_the_listener() {
        let class = scratch("Listener");
        let message = scratch("Open");
        assert!(!signal_named(&class, &message).expect("no listener yet"));

        let hits = Rc::new(Cell::new(0));
        let counter = Rc::clone(&hits);
        let listener = listen_named(
            &class,
            &message,
            Box::new(move || counter.set(counter.get() + 1)),
        )
        .expect("listener");
        assert!(signal_named(&class, &message).expect("signal"));

        // Pump this thread's queue until the posted message has been handled.
        let deadline = Instant::now() + Duration::from_secs(5);
        while hits.get() == 0 && Instant::now() < deadline {
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
        assert_eq!(hits.get(), 1);

        drop(listener);
        assert_eq!(
            Rc::strong_count(&hits),
            1,
            "callback released with the window"
        );
        assert!(!signal_named(&class, &message).expect("listener gone"));
    }
}
