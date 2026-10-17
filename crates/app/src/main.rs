//! Lastdrop: how much AI is left in the tank, one click away (docs/ARCHITECTURE.md).
//!
//! Start-up order: single instance → folders and logger → settings → [`wiring::run`] (theme,
//! locale, history, readout thread, refresh workers, Slint windows created hidden and kept alive,
//! tray icon, minute tick, Slint event loop).
//!
//! `--settings`, `--settings-pins` and `--about` run the dialog process instead
//! (`lastdrop_app::dialog`, ARCHITECTURE "Processes"): it has its own single-instance mutex and
//! log file and never takes the app's mutex.
//!
//! `--preview-flyout <png>` / `--preview-hovercard <png>` are development tools that render the
//! UI without starting the app (`lastdrop_app::preview`); they neither take the single-instance
//! mutex nor write the log.
#![windows_subsystem = "windows"]

use std::process::ExitCode;

use lastdrop_app::{Args, custom_services, dialog, install, logger, persist, preview, wiring};
use lastdrop_win::single_instance::{self, Instance};

fn main() -> ExitCode {
    let args = Args::parse(std::env::args_os());

    if let Some(out) = args
        .preview_flyout
        .clone()
        .or_else(|| args.preview_hover_card.clone())
        .or_else(|| args.preview_settings.clone())
        .or_else(|| args.preview_about.clone())
        .or_else(|| args.preview_history.clone())
        .or_else(|| args.preview_theme_studio.clone())
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

    // Installing and uninstalling (updates-spec §2): the release's Lastdrop-Setup.exe is this
    // program; under that name, or with --install / --uninstall, it installs or uninstalls.
    if args.uninstall {
        return install::run(install::Mode::Uninstall { quiet: args.quiet });
    }
    if args.install {
        return install::run(install::Mode::Install {
            update: args.update,
            quiet: args.quiet,
        });
    }
    if std::env::current_exe().is_ok_and(|exe| install::is_setup(&exe)) {
        return install::run(install::Mode::Setup);
    }

    // Settings and About: the short-lived dialog process, before (and without) the app's mutex.
    if let Some(kind) = args.dialog {
        return dialog::run(kind);
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

    // 3. The services described in JSON, before the settings name them (custom-services-spec §3).
    custom_services::load(&paths.providers);

    // 4. Settings: defaults on first run; a corrupt file is kept aside, never overwritten.
    let (settings, outcome) = persist::load_settings(&paths.settings);
    log::info!("settings: {outcome:?}");

    // 5. Everything else runs on the main thread's event loop.
    let first_run = matches!(outcome, persist::LoadOutcome::Missing);
    let code = match wiring::run(wiring::Startup {
        args,
        guard,
        paths,
        settings,
        first_run,
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
