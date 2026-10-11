//! Lastdrop: how much AI is left in the tank, one click away (docs/ARCHITECTURE.md).
//!
//! Start-up order: single instance → folders and logger → settings → [`wiring::run`] (theme,
//! locale, history, readout thread, refresh workers, Slint windows created hidden and kept alive,
//! tray icon, minute tick, Slint event loop).
//!
//! `--preview-flyout <png>` / `--preview-hovercard <png>` are development tools that render the
//! UI without starting the app (`lastdrop_app::preview`); they neither take the single-instance
//! mutex nor write the log.
#![windows_subsystem = "windows"]

use std::process::ExitCode;

use lastdrop_app::{Args, logger, persist, preview, wiring};
use lastdrop_win::single_instance::{self, Instance};

fn main() -> ExitCode {
    let args = Args::parse(std::env::args_os());

    if let Some(out) = args
        .preview_flyout
        .clone()
        .or_else(|| args.preview_hover_card.clone())
        .or_else(|| args.preview_settings.clone())
        .or_else(|| args.preview_about.clone())
    {
        return match preview::run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                // A GUI program has no console: leave the reason next to the requested file.
                let mut note = out.into_os_string();
                note.push(".error.txt");
                let _ = std::fs::write(note, format!("{e}\n"));
                ExitCode::FAILURE
            }
        };
    }

    // 1. Single instance: a second launch asks the first to open its flyout and exits.
    let guard = match single_instance::acquire() {
        Ok(Instance::First(guard)) => guard,
        Ok(Instance::Second) => {
            let _ = single_instance::signal_first();
            return ExitCode::SUCCESS;
        }
        Err(_) => return ExitCode::FAILURE,
    };

    // 2. Folders and the logger (local only, rotated, secrets redacted). Lastdrop still runs
    //    without a log file.
    let Ok(paths) = persist::Paths::from_env() else {
        return ExitCode::FAILURE;
    };
    let _ = logger::init(&paths.logs, log::LevelFilter::Info);
    log::info!("Lastdrop {} starting", env!("CARGO_PKG_VERSION"));

    // 3. Settings: defaults on first run; a corrupt file is kept aside, never overwritten.
    let (settings, outcome) = persist::load_settings(&paths.settings);
    log::info!("settings: {outcome:?}");

    // 4. Everything else runs on the main thread's event loop.
    let code = match wiring::run(wiring::Startup {
        args,
        guard,
        paths,
        settings,
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    };
    logger::shutdown();
    code
}
