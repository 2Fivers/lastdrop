//! Lastdrop: how much AI is left in the tank, one click away (docs/ARCHITECTURE.md).
//!
//! Start-up order: single instance → folders and logger → settings → [`wiring::run`] (theme,
//! locale, history, readout thread, refresh workers, Slint windows created hidden and kept alive,
//! tray icon, minute tick, Slint event loop).
#![windows_subsystem = "windows"]

use std::process::ExitCode;

use lastdrop_app::{Args, logger, persist, wiring};
use lastdrop_win::single_instance::{self, Instance};

fn main() -> ExitCode {
    let args = Args::parse(std::env::args_os());

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

    // 3. Settings: defaults on first run; a corrupt file is kept aside, never overwritten.
    let (settings, outcome) = persist::load_settings(&paths.settings);
    log::info!("settings: {outcome:?}");

    // 4. Everything else runs on the main thread's event loop.
    match wiring::run(wiring::Startup {
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
    }
}
