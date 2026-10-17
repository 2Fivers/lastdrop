//! Installing and uninstalling Lastdrop for one Windows user (docs/design/updates-spec.md §2):
//! the release's `Lastdrop-Setup.exe` is the app itself. Run under that name (or with
//! `--install`) it copies itself to `%LOCALAPPDATA%\Programs\Lastdrop\lastdrop.exe`, writes a
//! Start menu shortcut and the "Installed apps" entry, keeps "Start with Windows" pointing at the
//! installed copy, and starts it. `--uninstall` undoes that and leaves settings and history.
//! An update runs the new installer with `--install --update`: no questions, and the installed
//! app starts again quietly. A package manager (winget) runs `--install --quiet` and
//! `--uninstall --quiet`: no questions, no message boxes, only the exit code and the log.
//!
//! Every step that touches the running app asks it to quit first (the settings-changed message
//! with [`Changes::quit`]) and waits for its mutex to go and for every process that runs the
//! installed `lastdrop.exe` to end ([`quit_running_app`]).
//!
//! Threading: the installer's own process, main thread; no Slint, only message boxes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Language, Msg, Strings};
use lastdrop_win::install::{self as win_install, UninstallEntry};
use lastdrop_win::single_instance;

use crate::dialog::Changes;

/// The program's file name in the install folder.
pub const EXE_NAME: &str = "lastdrop.exe";

/// The file stem a release's installer has (`Lastdrop-Setup.exe`), compared case-insensitively.
pub const SETUP_STEM: &str = "lastdrop-setup";

/// How long the installer waits for a running Lastdrop to quit, its process included.
const QUIT_WAIT: Duration = Duration::from_secs(15);

/// How often it asks again, and looks again for the processes, while it waits
/// ([`quit_running_app`]).
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

/// The language of the installer's messages: the settings', else Windows'.
fn strings() -> &'static dyn Strings {
    let settings = crate::persist::Paths::from_env()
        .ok()
        .map(|p| crate::persist::load_settings(&p.settings).0)
        .unwrap_or_default();
    Language::resolve(
        settings.language.as_deref(),
        &lastdrop_win::locale::ui_languages(),
    )
    .strings()
}

/// Asks a running Lastdrop to quit and waits, at most [`QUIT_WAIT`] in all, for its mutex to go
/// and then for every process that runs `exe` (the program about to be replaced or removed) to
/// end; `false` when one is still running.
///
/// While a process still runs it checks the mutex again every [`QUIT_RESEND`]: a Lastdrop that
/// was just starting when the mutex was first checked takes it later, and is asked to quit then.
fn quit_running_app(exe: &Path) -> bool {
    let deadline = Instant::now() + QUIT_WAIT;
    while mutex_gone(deadline) {
        let until = deadline.min(Instant::now() + QUIT_RESEND);
        let Some(pid) = still_running(exe, until) else {
            return true;
        };
        if Instant::now() >= deadline {
            log::warn!("install: process {pid} still runs {}", exe.display());
            return false;
        }
        // A failed wait returns at once: look again a moment later, not in a busy loop.
        std::thread::sleep(until.saturating_duration_since(Instant::now()));
    }
    false
}

/// Asks a running Lastdrop to quit until its mutex is gone, at most until `deadline`; `false`
/// when it is still there.
///
/// It asks again every [`QUIT_RESEND`] until the app is gone: a Lastdrop that has just started
/// holds its mutex before its settings-changed listener exists, and a quit posted in between is
/// lost (no listener to post to). That happens when an install is followed at once by an
/// uninstall, as winget's validation and the release workflow's install test do.
fn mutex_gone(deadline: Instant) -> bool {
    if !single_instance::app_running() {
        return true;
    }
    let quit = Changes {
        quit: true,
        ..Changes::default()
    }
    .to_wparam();
    let mut asked: Option<Instant> = None;
    while Instant::now() < deadline {
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

/// Waits, at most until `until`, for every process but this one that runs `exe` to end; the id
/// of one that still runs then, `None` when none does.
///
/// The app closes its mutex at the end of its shutdown, but its process ends a moment later (the
/// log flushed, the threads and DLLs torn down), and until then `exe` cannot be replaced nor its
/// folder removed. A Settings, Theme Studio, Usage history or About window started from `exe`
/// (the dialog process) has a mutex of its own and is not asked to quit, so it keeps the
/// installer waiting until the user closes it. When the processes cannot be listed, the mutex is
/// all the installer waits for.
fn still_running(exe: &Path, until: Instant) -> Option<u32> {
    let running = match win_install::processes_running(exe) {
        Ok(running) => running,
        Err(e) => {
            log::warn!(
                "install: cannot list the processes running {}: {e}",
                exe.display()
            );
            return None;
        }
    };
    running.iter().find_map(|process| {
        let pid = process.pid();
        match process.wait(until.saturating_duration_since(Instant::now())) {
            Ok(true) => None,
            Ok(false) => Some(pid),
            Err(e) => {
                log::warn!("install: cannot wait for process {pid}: {e}");
                Some(pid)
            }
        }
    })
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
    if !quit_running_app(&target) {
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
    // "Start with Windows" follows the installed copy (an older copy elsewhere, or the previous
    // version, may be what the Run key names).
    if lastdrop_win::autostart::is_enabled(&target).is_ok_and(|on| !on)
        && run_key_set()
        && let Err(e) = lastdrop_win::autostart::enable(&target)
    {
        log::warn!("install: autostart not moved: {e}");
    }
    log::info!("install: Lastdrop {} installed", env!("CARGO_PKG_VERSION"));
    start(&target, quietly);
    Ok(target)
}

/// Whether the Run value exists at all (whatever program it names).
fn run_key_set() -> bool {
    lastdrop_win::autostart::is_set().unwrap_or(false)
}

/// Uninstalls (updates-spec §2): quits the app, removes the Run value, the shortcut and the
/// "Installed apps" entry, and the install folder once this process has ended. Settings, history
/// and logs stay.
fn uninstall() -> Result<(), String> {
    let strings = strings();
    let dir = install_dir().ok_or_else(|| "%LOCALAPPDATA% is not set".to_owned())?;
    if !quit_running_app(&dir.join(EXE_NAME)) {
        return Err(strings.template(Msg::InstallStillRunning).to_owned());
    }
    if lastdrop_win::autostart::is_enabled(&dir.join(EXE_NAME)).unwrap_or(false) {
        let _ = lastdrop_win::autostart::disable();
    }
    if let Some(programs) = win_install::start_menu_programs() {
        let _ = std::fs::remove_file(programs.join("Lastdrop.lnk"));
    }
    win_install::remove_uninstall_entry().map_err(|e| e.to_string())?;
    remove_folder_later(&dir);
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

/// Runs the installer in `mode`; the process's exit code.
pub fn run(mode: Mode) -> std::process::ExitCode {
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
                return std::process::ExitCode::SUCCESS;
            }
            finish(install(false).map(|_| ()), strings, Msg::InstallDone)
        }
        Mode::Install { update, quiet } => {
            let result = install(update || quiet).map(|_| ());
            if update || quiet {
                // Quietly: the app says itself that it was updated, or the log says why not. An
                // update still shows its failure; a quiet install leaves it to the exit code.
                if let Err(e) = &result {
                    log::error!("{}: {e}", if update { "update" } else { "install" });
                    if !quiet {
                        win_install::tell(title, e, true);
                    }
                }
                return exit(result.is_ok());
            }
            finish(result, strings, Msg::InstallDone)
        }
        Mode::Uninstall { quiet: true } => {
            let result = uninstall();
            if let Err(e) = &result {
                log::error!("uninstall: {e}");
            }
            exit(result.is_ok())
        }
        Mode::Uninstall { quiet: false } => {
            if !win_install::ask(title, strings.template(Msg::UninstallAsk)) {
                return std::process::ExitCode::SUCCESS;
            }
            finish(uninstall(), strings, Msg::UninstallDone)
        }
    }
}

fn exit(ok: bool) -> std::process::ExitCode {
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// Tells the user how it went.
fn finish(result: Result<(), String>, strings: &dyn Strings, done: Msg) -> std::process::ExitCode {
    match result {
        Ok(()) => {
            win_install::tell("Lastdrop", strings.template(done), false);
            exit(true)
        }
        Err(e) => {
            log::error!("install: {e}");
            let message = text(strings, Msg::InstallFailed, &[("detail", &e)]);
            win_install::tell("Lastdrop", &message, true);
            exit(false)
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
    fn the_installer_never_waits_for_itself() {
        // The uninstaller runs from the program it removes. Another run of this test program may
        // be found; this one never is.
        let me = std::env::current_exe().expect("the test program");
        assert_ne!(still_running(&me, Instant::now()), Some(std::process::id()));
        // Nothing runs a program that is not there.
        let missing = std::env::temp_dir().join(format!(
            "lastdrop-no-such-program-{}.exe",
            std::process::id()
        ));
        assert_eq!(still_running(&missing, Instant::now()), None);
    }
}
