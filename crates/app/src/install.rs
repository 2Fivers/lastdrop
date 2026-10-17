//! Installing and uninstalling Lastdrop for one Windows user (docs/design/updates-spec.md §2):
//! the release's `Lastdrop-Setup.exe` is the app itself. Run under that name (or with
//! `--install`) it copies itself to `%LOCALAPPDATA%\Programs\Lastdrop\lastdrop.exe`, writes a
//! Start menu shortcut and the "Installed apps" entry, keeps "Start with Windows" pointing at the
//! installed copy (and back on after a reinstall when the settings have it on), and starts it.
//! `--uninstall` undoes that and leaves settings and history.
//! An update runs the new installer with `--install --update`: no questions, and the installed
//! app starts again quietly. A package manager (winget) runs `--install --quiet` and
//! `--uninstall --quiet`: no questions, no message boxes, only the exit code and the log.
//!
//! The log is the installer's own, `%LOCALAPPDATA%\Lastdrop\logs\install.log` ([`run`]); an
//! uninstall leaves it with the other logs.
//!
//! Every step that touches the running app asks it to quit first (the settings-changed message
//! with [`Changes::quit`]) and waits for its mutex to go.
//!
//! Threading: the installer's own process, main thread (and the logger's writer thread); no
//! Slint, only message boxes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Language, Msg, Strings};
use lastdrop_core::settings::Settings;
use lastdrop_win::install::{self as win_install, UninstallEntry};
use lastdrop_win::single_instance;

use crate::dialog::Changes;
use crate::logger;

/// The program's file name in the install folder.
pub const EXE_NAME: &str = "lastdrop.exe";

/// The file stem a release's installer has (`Lastdrop-Setup.exe`), compared case-insensitively.
pub const SETUP_STEM: &str = "lastdrop-setup";

/// How long the installer waits for a running Lastdrop to quit.
const QUIT_WAIT: Duration = Duration::from_secs(15);

/// How often it asks again while it waits ([`quit_running_app`]).
const QUIT_RESEND: Duration = Duration::from_secs(1);

/// How long it retries replacing a file another process still holds (an anti-virus scan).
const COPY_WAIT: Duration = Duration::from_secs(10);

/// `CREATE_NO_WINDOW`.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What the installer is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Run as `Lastdrop-Setup.exe` without arguments: ask, then install.
    Setup,
    /// `--install`: install without asking (`--update`: and start the app quietly).
    Install {
        /// Started by an update of a running Lastdrop.
        update: bool,
        /// `--quiet`: no message boxes, and the app starts quietly.
        quiet: bool,
    },
    /// `--uninstall`: ask, then uninstall (`--quiet`: neither ask nor tell).
    Uninstall {
        /// `--quiet`.
        quiet: bool,
    },
}

/// The folder Lastdrop is installed in: `%LOCALAPPDATA%\Programs\Lastdrop`.
pub fn install_dir() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(local).join("Programs").join("Lastdrop"))
}

/// Whether `exe` is a release's installer by its name.
pub fn is_setup(exe: &Path) -> bool {
    exe.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.to_ascii_lowercase().starts_with(SETUP_STEM))
}

/// The saved settings (defaults when there are none or they cannot be read). Read without side
/// effects: a broken file stays where it is for the app to set aside and report.
fn saved_settings() -> Settings {
    crate::persist::Paths::from_env()
        .ok()
        .and_then(|p| crate::persist::read_settings(&p.settings).ok())
        .map(|(settings, _)| settings)
        .unwrap_or_default()
}

/// The language of the installer's messages: the settings', else Windows'.
fn strings() -> &'static dyn Strings {
    Language::resolve(
        saved_settings().language.as_deref(),
        &lastdrop_win::locale::ui_languages(),
    )
    .strings()
}

/// What the Run value (`HKCU\…\Run\Lastdrop`) names before an install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunValue {
    /// The installed copy.
    Installed,
    /// Another program: an older copy elsewhere, or a development build.
    Other,
    /// Nothing: never turned on, turned off, or removed by an uninstall.
    Missing,
}

/// Whether the install writes the Run value for the installed copy. It follows the user's
/// choice wherever it was made: a Run value for another copy moves to the installed one, and a
/// missing one comes back when `settings.json` has "Start with Windows" on. An uninstall removes
/// the Run value but leaves the settings, so without that a reinstall would show the switch on
/// while Windows no longer starts Lastdrop.
fn writes_run_value(before: RunValue, setting_on: bool) -> bool {
    match before {
        RunValue::Installed => false,
        RunValue::Other => true,
        RunValue::Missing => setting_on,
    }
}

/// What the Run value names now; `None` when the registry cannot be read (left alone then).
fn run_value(installed: &Path) -> Option<RunValue> {
    if lastdrop_win::autostart::is_enabled(installed).ok()? {
        return Some(RunValue::Installed);
    }
    Some(if lastdrop_win::autostart::is_set().ok()? {
        RunValue::Other
    } else {
        RunValue::Missing
    })
}

/// Asks a running Lastdrop to quit and waits for it; `false` when it is still running.
///
/// It asks again every [`QUIT_RESEND`] until the app is gone: a Lastdrop that has just started
/// holds its mutex before its settings-changed listener exists, and a quit posted in between is
/// lost (no listener to post to). That happens when an install is followed at once by an
/// uninstall, as winget's validation and the release workflow's install test do.
fn quit_running_app() -> bool {
    if !single_instance::app_running() {
        return true;
    }
    let quit = Changes {
        quit: true,
        ..Changes::default()
    }
    .to_wparam();
    let start = Instant::now();
    let mut asked: Option<Instant> = None;
    while start.elapsed() < QUIT_WAIT {
        if !single_instance::app_running() {
            return true;
        }
        if asked.is_none_or(|at| at.elapsed() >= QUIT_RESEND) {
            match single_instance::signal_settings_changed(quit) {
                Ok(true) => {}
                Ok(false) => log::info!("install: Lastdrop is not listening yet; asking again"),
                Err(e) => log::warn!("install: cannot ask Lastdrop to quit: {e}"),
            }
            asked = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Copies `from` over `to`, retrying while another process holds `to`.
fn copy_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let start = Instant::now();
    loop {
        match std::fs::copy(from, to) {
            Ok(_) => return Ok(()),
            Err(e) if start.elapsed() < COPY_WAIT => {
                log::info!("install: copy retried: {e}");
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => return Err(e),
        }
    }
}

/// Starts the installed app (`quietly`, after an update or a quiet install: with `--autostart`,
/// so it opens nothing).
fn start(exe: &Path, quietly: bool) {
    let mut command = Command::new(exe);
    if quietly {
        command.arg(lastdrop_win::autostart::AUTOSTART_ARG);
    }
    if let Some(dir) = exe.parent() {
        command.current_dir(dir);
    }
    if let Err(e) = command.spawn() {
        log::warn!("install: the installed app did not start: {e}");
    }
}

/// Installs this executable (updates-spec §2) and starts it, `quietly` or not. Returns a message
/// for the user on failure.
fn install(quietly: bool) -> Result<PathBuf, String> {
    let strings = strings();
    let dir = install_dir().ok_or_else(|| "%LOCALAPPDATA% is not set".to_owned())?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let target = dir.join(EXE_NAME);
    if !quit_running_app() {
        return Err(strings.template(Msg::InstallStillRunning).to_owned());
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let same = std::fs::canonicalize(&me).ok() == std::fs::canonicalize(&target).ok();
    if !same {
        copy_with_retry(&me, &target).map_err(|e| e.to_string())?;
    }
    if let Some(programs) = win_install::start_menu_programs() {
        let _ = std::fs::create_dir_all(&programs);
        if let Err(e) = win_install::write_shortcut(
            &programs.join("Lastdrop.lnk"),
            &target,
            strings.template(Msg::InstallShortcut),
        ) {
            log::warn!("install: no Start menu shortcut: {e}");
        }
    }
    let size_kb = std::fs::metadata(&target)
        .map(|m| u32::try_from(m.len() / 1024).unwrap_or(u32::MAX))
        .unwrap_or(0);
    win_install::write_uninstall_entry(&UninstallEntry {
        name: "Lastdrop",
        version: env!("CARGO_PKG_VERSION"),
        exe: &target,
        folder: &dir,
        size_kb,
    })
    .map_err(|e| e.to_string())?;
    // "Start with Windows" names the installed copy when it is on ([`writes_run_value`]).
    if let Some(before) = run_value(&target)
        && writes_run_value(before, saved_settings().start_with_windows)
    {
        match lastdrop_win::autostart::enable(&target) {
            Ok(()) => log::info!("install: Start with Windows names the installed copy"),
            Err(e) => log::warn!("install: Start with Windows not set: {e}"),
        }
    }
    log::info!("install: Lastdrop {} installed", env!("CARGO_PKG_VERSION"));
    start(&target, quietly);
    Ok(target)
}

/// Uninstalls (updates-spec §2): quits the app, removes the Run value, the shortcut and the
/// "Installed apps" entry, and the install folder once this process has ended. Settings, history
/// and logs stay.
fn uninstall() -> Result<(), String> {
    let strings = strings();
    if !quit_running_app() {
        return Err(strings.template(Msg::InstallStillRunning).to_owned());
    }
    let dir = install_dir().ok_or_else(|| "%LOCALAPPDATA% is not set".to_owned())?;
    if lastdrop_win::autostart::is_enabled(&dir.join(EXE_NAME)).unwrap_or(false) {
        let _ = lastdrop_win::autostart::disable();
    }
    if let Some(programs) = win_install::start_menu_programs() {
        let _ = std::fs::remove_file(programs.join("Lastdrop.lnk"));
    }
    win_install::remove_uninstall_entry().map_err(|e| e.to_string())?;
    remove_folder_later(&dir);
    log::info!(
        "uninstall: Lastdrop {} uninstalled",
        env!("CARGO_PKG_VERSION")
    );
    Ok(())
}

/// Removes `dir` after this process ends (it may be running from it): a hidden `cmd.exe` waits
/// two seconds and deletes it. Only Lastdrop's own install folder, and only a path `cmd` cannot
/// misread.
fn remove_folder_later(dir: &Path) {
    let Some(text) = dir.to_str() else {
        return;
    };
    if Some(dir) != install_dir().as_deref()
        || text.contains(['"', '%', '^', '&', '|', '<', '>'])
        || !dir.is_dir()
    {
        log::warn!("uninstall: the install folder is left in place");
        return;
    }
    use std::os::windows::process::CommandExt;
    let system = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join("cmd.exe");
    let script = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{text}\"");
    let spawned = Command::new(system)
        .args(["/d", "/c"])
        .raw_arg(&script)
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(std::env::temp_dir())
        .spawn();
    if let Err(e) = spawned {
        log::warn!("uninstall: the install folder is left in place: {e}");
    }
}

/// Runs the installer in `mode` and logs it to its own file (`install.log`,
/// [`logger::INSTALL_STEM`]; the app it quits may still be writing `lastdrop.log`); the
/// process's exit code. The installer still runs without a log file.
pub fn run(mode: Mode) -> std::process::ExitCode {
    if let Ok(paths) = crate::persist::Paths::from_env() {
        let _ = logger::init_named(&paths.logs, logger::INSTALL_STEM, log::LevelFilter::Info);
    }
    log::info!(
        "Lastdrop {} installer starting ({mode:?})",
        env!("CARGO_PKG_VERSION")
    );
    let ok = run_mode(mode);
    log::info!("installer stopped (exit code {})", u8::from(!ok));
    logger::shutdown();
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// [`run`] without the log's start and end; whether it went well (a question answered No did).
fn run_mode(mode: Mode) -> bool {
    let strings = strings();
    let title = "Lastdrop";
    let version = env!("CARGO_PKG_VERSION");
    match mode {
        Mode::Setup => {
            let dir = install_dir()
                .map(|d| d.display().to_string())
                .unwrap_or_default();
            let question = text(
                strings,
                Msg::InstallAsk,
                &[("version", version), ("folder", &dir)],
            );
            if !win_install::ask(title, &question) {
                log::info!("install: not wanted");
                return true;
            }
            finish(
                "install",
                install(false).map(|_| ()),
                strings,
                Msg::InstallDone,
            )
        }
        Mode::Install { update, quiet } => {
            let result = install(update || quiet).map(|_| ());
            if update || quiet {
                // Quietly: nothing is shown when it worked (the app starts again with
                // `--autostart` and opens nothing, updates-spec §3), and the log says why when it
                // did not. An update still shows its failure; a quiet install leaves it to the
                // exit code and the log.
                if let Err(e) = &result {
                    log::error!("{}: {e}", if update { "update" } else { "install" });
                    if !quiet {
                        win_install::tell(title, e, true);
                    }
                }
                return result.is_ok();
            }
            finish("install", result, strings, Msg::InstallDone)
        }
        Mode::Uninstall { quiet: true } => {
            let result = uninstall();
            if let Err(e) = &result {
                log::error!("uninstall: {e}");
            }
            result.is_ok()
        }
        Mode::Uninstall { quiet: false } => {
            if !win_install::ask(title, strings.template(Msg::UninstallAsk)) {
                log::info!("uninstall: not wanted");
                return true;
            }
            finish("uninstall", uninstall(), strings, Msg::UninstallDone)
        }
    }
}

/// Tells the user how `what` (`install`, `uninstall`) went; whether it went well.
fn finish(what: &str, result: Result<(), String>, strings: &dyn Strings, done: Msg) -> bool {
    match result {
        Ok(()) => {
            win_install::tell("Lastdrop", strings.template(done), false);
            true
        }
        Err(e) => {
            log::error!("{what}: {e}");
            let message = text(strings, Msg::InstallFailed, &[("detail", &e)]);
            win_install::tell("Lastdrop", &message, true);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_names() {
        assert!(is_setup(Path::new(
            r"C:\Users\x\Downloads\Lastdrop-Setup.exe"
        )));
        assert!(is_setup(Path::new(
            r"C:\Users\x\Downloads\Lastdrop-Setup (1).exe"
        )));
        assert!(is_setup(Path::new("lastdrop-setup.exe")));
        assert!(!is_setup(Path::new(
            r"C:\Users\x\AppData\Local\Programs\Lastdrop\lastdrop.exe"
        )));
        assert!(!is_setup(Path::new("Setup.exe")));
    }

    #[test]
    fn start_with_windows_follows_the_user_after_an_install() {
        // Already the installed copy: nothing to write.
        assert!(!writes_run_value(RunValue::Installed, true));
        assert!(!writes_run_value(RunValue::Installed, false));
        // Another copy's Run value moves to the installed copy.
        assert!(writes_run_value(RunValue::Other, true));
        assert!(writes_run_value(RunValue::Other, false));
        // A reinstall after an uninstall (which removed the value) restores it when the settings
        // still say on, and writes nothing when they say off.
        assert!(writes_run_value(RunValue::Missing, true));
        assert!(!writes_run_value(RunValue::Missing, false));
    }
}
