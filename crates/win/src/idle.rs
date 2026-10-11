//! How long the user has been away: the time since the session's last keyboard or mouse input
//! (`GetLastInputInfo`), for the adaptive refresh's "slower when idle" (ARCHITECTURE "Refresh
//! scheduler").
//!
//! The input time is a `GetTickCount` value, which wraps every 49.7 days; the difference is taken
//! modulo 2³², so it stays right across a wrap. A locked session or a display that turned off for
//! inactivity gets no input, so it counts as idle after the same time.

use std::time::Duration;

use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

/// The time since the last input in this session; `None` when Windows does not say.
pub fn since_last_input() -> Option<Duration> {
    let mut info = LASTINPUTINFO {
        cbSize: u32::try_from(size_of::<LASTINPUTINFO>()).unwrap_or(8),
        dwTime: 0,
    };
    // SAFETY: `info` is a live LASTINPUTINFO with `cbSize` set, as the call requires.
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return None;
    }
    // SAFETY: a plain query without pointers.
    let now = unsafe { GetTickCount() };
    Some(elapsed(now, info.dwTime))
}

/// `now − then` for 32-bit millisecond tick counts, across one wrap.
fn elapsed(now: u32, then: u32) -> Duration {
    Duration::from_millis(u64::from(now.wrapping_sub(then)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_differences_survive_the_wrap() {
        assert_eq!(elapsed(5_000, 2_000), Duration::from_secs(3));
        assert_eq!(elapsed(1_000, u32::MAX - 999), Duration::from_secs(2));
        assert_eq!(elapsed(7, 7), Duration::ZERO);
    }

    #[test]
    fn reads_the_last_input() {
        // Any session answers; the value itself depends on the machine.
        assert!(since_last_input().is_some());
    }
}
