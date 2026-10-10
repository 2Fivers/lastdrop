//! The `lastdrop` app: Slint UI (flyout, hover card, settings, about), the tray icon, wiring
//! between the threads, the refresh scheduler, and settings and history persistence
//! (ARCHITECTURE "App"). The binary (`src/main.rs`) only sequences start-up.
//!
//! - [`ui`]: the compiled Slint windows and structs (`ui/*.slint`).
//! - [`state`]: [`state::AppState`], the main thread's state.
//! - [`scheduler`]: refresh worker threads driven by `lastdrop_core::refresh`.
//! - [`history`]: the append-only JSONL history store.
//! - [`persist`]: folders, settings load/save (atomic).
//! - [`logger`]: the tiny file logger with redaction.
//! - [`wiring`]: start-up, event handlers, model → Slint conversions.
//!
//! Threads (ARCHITECTURE "Threads"): **main** owns the Slint event loop, every window, the tray
//! icon and [`state::AppState`], and never blocks on I/O; the **readout** and **occupancy**
//! threads live in `lastdrop-win`; **refresh workers** ([`scheduler`]) run provider fetches and
//! never touch UI objects. Workers report with `slint::invoke_from_event_loop`; the main thread
//! hands the readout an `Arc<ReadoutModel>`; no lock is held across a Win32 call.
//!
//! Slint licence rule: Slint's Royalty-free License 2.0 requires the About window's `AboutSlint`
//! and forbids exposing Slint's APIs, so themes, layouts and provider definitions are data, never
//! `.slint` code loaded at runtime, and nothing user-supplied reaches a Slint interpreter.

pub mod history;
pub mod logger;
pub mod persist;
pub mod scheduler;
pub mod state;
pub mod ui;
pub mod wiring;

/// Command-line flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// Started by the Run key (`--autostart`): do not open the flyout on start.
    pub autostart: bool,
}

impl Args {
    /// Parses `std::env::args_os()` (the first item is the program); unknown flags are ignored.
    pub fn parse(args: impl IntoIterator<Item = std::ffi::OsString>) -> Args {
        let autostart_flag = std::ffi::OsStr::new(lastdrop_win::autostart::AUTOSTART_ARG);
        Args {
            autostart: args.into_iter().skip(1).any(|a| a == autostart_flag),
        }
    }
}

/// Why start-up or the event loop failed.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A file or folder could not be used.
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    /// Slint failed (backend, window creation, event loop).
    #[error("UI: {0}")]
    Ui(String),
    /// A Win32 call failed.
    #[error("Win32: {0}")]
    Win32(#[from] windows::core::Error),
}

#[cfg(test)]
mod tests {
    use super::Args;

    #[test]
    fn autostart_flag() {
        let args = |list: &[&str]| Args::parse(list.iter().map(std::ffi::OsString::from));
        assert!(args(&["lastdrop.exe", "--autostart"]).autostart);
        assert!(!args(&["lastdrop.exe"]).autostart);
        assert!(!args(&["--autostart"]).autostart, "argv[0] is the program");
    }
}
