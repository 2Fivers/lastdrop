//! A named mutex held for one read-merge-write of a file that both of Lastdrop's processes write
//! (settings-spec §6): the tray process's writer thread, the settings window and Theme Studio take
//! [`SETTINGS_LOCK`] around every read-merge-write of `settings.json`, so none of them writes over
//! a change it did not read.
//!
//! The lock is held only for that one read-merge-write: nobody waits for another thread or
//! process while holding it. A process that ends while holding it releases it (an abandoned mutex
//! is owned by the next waiter, which then reads the file as it is).
//!
//! Threading: a Win32 mutex belongs to the thread that took it, so a [`NamedLock`] is released on
//! that thread (it is neither `Send` nor `Sync`).

use std::marker::PhantomData;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
use windows::core::PCWSTR;

/// The mutex around every read-merge-write of `settings.json` (per session).
pub const SETTINGS_LOCK: &str = r"Local\Lastdrop.Settings";

/// A named mutex this thread owns; released and closed on drop.
#[derive(Debug)]
pub struct NamedLock {
    handle: isize,
    /// Owned by the thread that took it.
    _thread: PhantomData<*const ()>,
}

impl NamedLock {
    /// Takes the mutex `name` (created when no process has it open), waiting at most `wait`.
    /// An abandoned mutex (its owner ended without releasing it) is taken. A timeout is
    /// `ErrorKind::TimedOut`.
    pub fn acquire(name: &str, wait: Duration) -> std::io::Result<NamedLock> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: a NUL-terminated name, default security, not initially owned.
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) }
            .map_err(std::io::Error::other)?;
        let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: a mutex handle we just opened.
        let waited = unsafe { WaitForSingleObject(handle, millis) };
        if waited == WAIT_OBJECT_0 || waited == WAIT_ABANDONED {
            return Ok(NamedLock {
                handle: handle.0 as isize,
                _thread: PhantomData,
            });
        }
        let error = if waited == WAIT_TIMEOUT {
            std::io::Error::from(std::io::ErrorKind::TimedOut)
        } else if waited == WAIT_FAILED {
            std::io::Error::last_os_error()
        } else {
            std::io::Error::other(format!("unexpected wait result {}", waited.0))
        };
        // SAFETY: our handle, closed once; we never owned the mutex.
        let _ = unsafe { CloseHandle(handle) };
        Err(error)
    }
}

impl Drop for NamedLock {
    fn drop(&mut self) {
        let handle = HANDLE(self.handle as *mut _);
        // SAFETY: this thread owns the mutex (see `acquire`; the type cannot leave the thread);
        // the handle is closed exactly once.
        unsafe {
            if let Err(e) = ReleaseMutex(handle) {
                log::warn!("named lock: releasing failed: {e}");
            }
            let _ = CloseHandle(handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use super::*;

    /// A name unique to this test run, so the tests never touch a running Lastdrop.
    fn scratch(tag: &str) -> String {
        format!(r"Local\Lastdrop.Test.Lock.{tag}.{}", std::process::id())
    }

    #[test]
    fn a_second_thread_waits_until_the_first_releases() {
        let name = scratch("Wait");
        let held = NamedLock::acquire(&name, Duration::from_secs(1)).expect("first");
        let other = name.clone();
        let refused = std::thread::spawn(move || {
            NamedLock::acquire(&other, Duration::from_millis(50)).map(drop)
        })
        .join()
        .expect("thread");
        assert_eq!(
            refused.expect_err("held elsewhere").kind(),
            std::io::ErrorKind::TimedOut
        );

        let (taken_tx, taken_rx) = mpsc::channel();
        let other = name.clone();
        let waiter = std::thread::spawn(move || {
            let lock = NamedLock::acquire(&other, Duration::from_secs(5)).expect("after release");
            taken_tx.send(Instant::now()).expect("send");
            drop(lock);
        });
        std::thread::sleep(Duration::from_millis(100));
        let released = Instant::now();
        drop(held);
        let taken = taken_rx.recv().expect("taken");
        assert!(taken >= released, "taken only after the release");
        waiter.join().expect("thread");
    }

    #[test]
    fn an_abandoned_lock_is_taken_by_the_next_waiter() {
        let name = scratch("Abandoned");
        let other = name.clone();
        std::thread::spawn(move || {
            let lock = NamedLock::acquire(&other, Duration::from_secs(1)).expect("first");
            // The thread ends without releasing it.
            std::mem::forget(lock);
        })
        .join()
        .expect("thread");
        assert!(NamedLock::acquire(&name, Duration::from_secs(1)).is_ok());
    }

    #[test]
    fn the_same_thread_can_take_it_again() {
        let name = scratch("Again");
        let first = NamedLock::acquire(&name, Duration::from_secs(1)).expect("first");
        let second = NamedLock::acquire(&name, Duration::from_millis(10)).expect("again");
        drop(second);
        drop(first);
    }
}
