//! The `lastdrop` app: Slint UI (flyout, hover card, settings, about), the tray icon, wiring
//! between the threads, the refresh scheduler, and settings and history persistence
//! (ARCHITECTURE "App"). The binary (`src/main.rs`) only sequences start-up.
//!
//! Two processes run the same executable (ARCHITECTURE "Processes"): the **tray process**
//! (always running: tray icon, readout, flyout, hover card, refreshes) and the short-lived
//! **dialog process** (`--settings`, `--settings-pins`, `--settings-accounts`, `--about`:
//! [`dialog`]), so the window setup a decorated window with text fields costs never stays in the
//! tray process.
//!
//! - [`ui`]: the compiled Slint windows and structs (`ui/*.slint`).
//! - [`state`]: [`state::AppState`], the main thread's state.
//! - [`scheduler`]: refresh worker threads driven by `lastdrop_core::refresh`.
//! - [`history`]: the append-only JSONL history store.
//! - [`persist`]: folders, settings and state file load/save (atomic).
//! - [`logger`]: the tiny file logger with redaction.
//! - [`wiring`]: start-up, event handlers, model → Slint conversions.
//! - [`bench`]: the `--bench` timeline and its JSON (compatible with `tools/measure`).
//! - [`demo`]: the built-in demo data of `--preview-flyout --demo` (invented, no I/O).
//! - [`preview`]: `--preview-flyout`: renders the flyout headless and saves a PNG.
//! - [`png`]: the minimal PNG writer the preview uses.
//! - [`dialog`]: the dialog process, the requests between it and the tray process.
//! - [`settings_window`]: the settings window's draft and callbacks (dialog process);
//!   [`settings_accounts`]: its Accounts section (accounts-spec §6–§7).
//! - [`writer`]: the file writer thread (history appends and compaction, settings and state
//!   saves, in queue order).
//!
//! Threads of the tray process (ARCHITECTURE "Threads"): **main** owns the Slint event loop, the
//! flyout and the hover card, the tray icon and [`state::AppState`], and never blocks on I/O; the **readout** and **occupancy**
//! threads live in `lastdrop-win`; **refresh workers** ([`scheduler`]) run provider fetches and
//! never touch UI objects; the **writer** ([`writer`]) does every file write in queue order and
//! the **log** writer ([`logger`]) the log's. Workers report with
//! `slint::invoke_from_event_loop`; the main thread hands the readout an `Arc<ReadoutModel>`; no
//! lock is held across a Win32 call.
//!
//! Slint licence rule: Slint's Royalty-free License 2.0 requires the About window's `AboutSlint`
//! and forbids exposing Slint's APIs, so themes, layouts and provider definitions are data, never
//! `.slint` code loaded at runtime, and nothing user-supplied reaches a Slint interpreter.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::time::Duration;

pub mod bench;
pub mod demo;
pub mod dialog;
pub mod history;
pub mod logger;
pub mod persist;
pub mod png;
pub mod preview;
pub mod scheduler;
pub mod settings_accounts;
pub mod settings_window;
pub mod state;
pub mod ui;
pub mod wiring;
pub mod writer;

/// Command-line flags. Unknown flags are ignored; a flag whose value is missing or unreadable is
/// ignored too (a GUI program has nowhere to print a usage message).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args {
    /// Started by the Run key (`--autostart`): do not open the flyout on start.
    pub autostart: bool,
    /// `--smoke <seconds>`: run normally (without opening the flyout), then quit cleanly after
    /// this long. For smoke tests.
    pub smoke: Option<Duration>,
    /// `--no-readout`: never show the taskbar readout in this run (the readout thread still runs
    /// for the system broadcasts).
    pub no_readout: bool,
    /// `--bench <out.json>`: the spike-compatible timeline (+10 s show without activation, +20 s
    /// hide, +30 s show warm, +40 s hide), then +50 s Settings (the dialog process), +70 s exit,
    /// with its JSON ([`bench`]).
    pub bench: Option<PathBuf>,
    /// `--preview-flyout <png>`: render the flyout headless, save it as a PNG and exit
    /// ([`preview`]).
    pub preview_flyout: Option<PathBuf>,
    /// `--preview-hovercard <png>`: the same for the hover card (over the readout's rows).
    pub preview_hover_card: Option<PathBuf>,
    /// `--preview-settings <png>`: the settings window with the demo's settings (`--demo`).
    pub preview_settings: Option<PathBuf>,
    /// `--preview-about <png>`: the about window (`--demo`).
    pub preview_about: Option<PathBuf>,
    /// `--demo`: with `--preview-flyout` / `--preview-hovercard`, use the built-in demo data ([`demo`]): no network, no
    /// files, no Windows settings.
    pub demo: bool,
    /// `--demo-scenario <normal|warning|stale|signin>`: which state of the mock the demo shows.
    pub demo_scenario: Option<String>,
    /// `--scale <factor>`: the preview's scale (1 = 100 %, 1.5 = 150 %). Default 1.
    pub scale: Option<f64>,
    /// `--dark`: the preview in the dark Windows mode.
    pub dark: bool,
    /// `--language <tag>`: the preview's language (`en`, `sv`); default: the settings' and
    /// Windows' (English with `--demo`).
    pub language: Option<String>,
    /// `--settings`, `--settings-pins` or `--about`: run as the dialog process with that dialog
    /// ([`dialog`]) instead of the app. The last one given counts.
    pub dialog: Option<dialog::DialogKind>,
}

impl Args {
    /// Parses `std::env::args_os()` (the first item is the program); unknown flags are ignored.
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Args {
        let autostart_flag = OsStr::new(lastdrop_win::autostart::AUTOSTART_ARG);
        let mut out = Args::default();
        let mut iter = args.into_iter().skip(1);
        while let Some(arg) = iter.next() {
            if arg == autostart_flag {
                out.autostart = true;
                continue;
            }
            match arg.to_str() {
                Some("--smoke") => {
                    out.smoke = iter
                        .next()
                        .and_then(|v| v.to_str().and_then(|s| s.parse::<f64>().ok()))
                        .filter(|s| s.is_finite() && *s >= 0.0)
                        .and_then(|s| Duration::try_from_secs_f64(s).ok());
                }
                Some("--no-readout") => out.no_readout = true,
                Some("--bench") => out.bench = iter.next().map(PathBuf::from),
                Some("--preview-flyout") => out.preview_flyout = iter.next().map(PathBuf::from),
                Some("--preview-hovercard") => {
                    out.preview_hover_card = iter.next().map(PathBuf::from);
                }
                Some("--preview-settings") => out.preview_settings = iter.next().map(PathBuf::from),
                Some("--preview-about") => out.preview_about = iter.next().map(PathBuf::from),
                Some("--demo") => out.demo = true,
                Some("--demo-scenario") => {
                    out.demo_scenario = iter.next().and_then(|v| v.to_str().map(str::to_owned));
                }
                Some("--dark") => out.dark = true,
                Some("--language") => {
                    out.language = iter.next().and_then(|v| v.to_str().map(str::to_owned));
                }
                Some(
                    flag @ ("--settings" | "--settings-pins" | "--settings-accounts" | "--about"),
                ) => {
                    out.dialog = dialog::DialogKind::from_flag(flag);
                }
                Some("--scale") => {
                    out.scale = iter
                        .next()
                        .and_then(|v| v.to_str().and_then(|s| s.parse::<f64>().ok()))
                        .filter(|s| s.is_finite() && (0.5..=4.0).contains(s));
                }
                _ => {}
            }
        }
        out
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

impl From<slint::PlatformError> for AppError {
    fn from(e: slint::PlatformError) -> AppError {
        AppError::Ui(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::Args;
    use std::path::PathBuf;
    use std::time::Duration;

    fn args(list: &[&str]) -> Args {
        Args::parse(list.iter().map(std::ffi::OsString::from))
    }

    #[test]
    fn autostart_flag() {
        assert!(args(&["lastdrop.exe", "--autostart"]).autostart);
        assert!(!args(&["lastdrop.exe"]).autostart);
        assert!(!args(&["--autostart"]).autostart, "argv[0] is the program");
    }

    #[test]
    fn tool_flags() {
        let a = args(&[
            "lastdrop.exe",
            "--smoke",
            "12",
            "--no-readout",
            "--bench",
            "out.json",
        ]);
        assert_eq!(a.smoke, Some(Duration::from_secs(12)));
        assert!(a.no_readout);
        assert_eq!(a.bench, Some(PathBuf::from("out.json")));
        let p = args(&[
            "x",
            "--preview-flyout",
            "f.png",
            "--demo",
            "--scale",
            "1.5",
            "--dark",
            "--language",
            "sv",
        ]);
        assert_eq!(p.preview_flyout, Some(PathBuf::from("f.png")));
        assert!(p.demo && p.dark);
        assert_eq!(p.scale, Some(1.5));
        assert_eq!(p.language.as_deref(), Some("sv"));
    }

    #[test]
    fn dialog_flags() {
        use crate::dialog::DialogKind;
        assert_eq!(
            args(&["lastdrop.exe", "--settings"]).dialog,
            Some(DialogKind::Settings)
        );
        assert_eq!(
            args(&["lastdrop.exe", "--settings-pins"]).dialog,
            Some(DialogKind::SettingsPins)
        );
        assert_eq!(
            args(&["lastdrop.exe", "--about"]).dialog,
            Some(DialogKind::About)
        );
        assert_eq!(args(&["lastdrop.exe"]).dialog, None);
        assert_eq!(args(&["--settings"]).dialog, None, "argv[0] is the program");
        assert_eq!(
            args(&["x", "--about", "--settings-pins"]).dialog,
            Some(DialogKind::SettingsPins),
            "the last one counts"
        );
        // Each dialog's own flag parses back to it.
        for kind in DialogKind::ALL {
            assert_eq!(args(&["x", kind.flag()]).dialog, Some(kind));
        }
        let with_autostart = args(&["x", "--autostart", "--settings"]);
        assert!(with_autostart.autostart);
        assert_eq!(with_autostart.dialog, Some(DialogKind::Settings));
    }

    #[test]
    fn missing_or_bad_values_are_ignored() {
        assert_eq!(args(&["x", "--smoke"]).smoke, None);
        assert_eq!(args(&["x", "--smoke", "soon"]).smoke, None);
        assert_eq!(args(&["x", "--scale", "100"]).scale, None);
        assert_eq!(args(&["x", "--unknown", "--demo"]), {
            Args {
                demo: true,
                ..Args::default()
            }
        });
    }
}
