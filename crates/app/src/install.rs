//! Installing and uninstalling Lastdrop for one Windows user (docs/design/updates-spec.md §2):
//! the release's `Lastdrop-Setup.exe` is the app itself. Run under that name (or with
//! `--install`) it copies itself to `%LOCALAPPDATA%\Programs\Lastdrop\lastdrop.exe`, writes a
//! Start menu shortcut and the "Installed apps" entry, keeps "Start with Windows" pointing at the
//! installed copy, and starts it. `--uninstall` undoes that and leaves settings and history.
//! An update runs the new installer with `--install --update`: no questions, and the installed
//! app starts again quietly.
//!
//! Every step that touches the running app asks it to quit first (the settings-changed message
//! with [`Changes::quit`]) and waits for its mutex to go.
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

/// How long the installer waits for a running Lastdrop to quit.
const QUIT_WAIT: Duration = Duration::from_secs(15);

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
    },
    /// `--uninstall`: ask, then uninstall.
    Uninstall,
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

/// Asks a running Lastdrop to quit and waits for it; `false` when it is still running.
fn quit_running_app() -> bool {
    if !single_instance::app_running() {
        return true;
    }
    let quit = Changes {
        quit: true,
        ..Changes::default()
    };
    let _ = single_instance::signal_settings_changed(quit.to_wparam());
    let start = Instant::now();
    while start.elapsed() < QUIT_WAIT {
        if !single_instance::app_running() {
            return true;
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

/// Starts the installed app (with `--autostart` after an update: it opens nothing).
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

/// Installs this executable (updates-spec §2). Returns a message for the user on failure.
fn install(update: bool) -> Result<PathBuf, String> {
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
    // "Start with Windows" follows the installed copy (an older copy elsewhere, or the previous
    // version, may be what the Run key names).
    if lastdrop_win::autostart::is_enabled(&target).is_ok_and(|on| !on)
        && run_key_set()
        && let Err(e) = lastdrop_win::autostart::enable(&target)
    {
        log::warn!("install: autostart not moved: {e}");
    }
    log::info!("install: Lastdrop {} installed", env!("CARGO_PKG_VERSION"));
    start(&target, update);
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
        Mode::Install { update } => {
            let result = install(update).map(|_| ());
            if update {
                // Quietly: the app says itself that it was updated, or the log says why not.
                if let Err(e) = &result {
                    log::error!("update: {e}");
                    win_install::tell(title, e, true);
                }
                return exit(result.is_ok());
            }
            finish(result, strings, Msg::InstallDone)
        }
        Mode::Uninstall => {
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
}
