//! The dialog process (ARCHITECTURE "Processes"): `lastdrop.exe --settings`, `--settings-pins`,
//! `--settings-accounts`, `--history` and `--about` show the settings window, the usage history
//! window or the about window in a short-lived process of their own, which ends when its last
//! window closes.
//!
//! Why a process: the tray process measured 10.1–10.3 MB private working set fresh and
//! 11.9–12.25 MB once Settings had been opened. Slint's own caches kept only about 0.2 MB of
//! that; the rest (about 1.4 MB) is what Windows sets up once per thread for a real decorated
//! window with text fields (text services and IME, DWM, theme, UI Automation) and keeps for the
//! life of the process. In a process of its own it goes away with the dialog.
//!
//! - The tray process starts this one with [`spawn`] and forgets it (no wait, no handles kept).
//! - Single instance: the `Local\Lastdrop.Dialog` mutex. A second launch posts the dialog it
//!   wants ([`DialogKind::to_wparam`]) to the running dialog process, which opens it or brings
//!   it to the front, and exits.
//! - The installer asks it to close every window and end ([`CLOSE_REQUEST`]) before it replaces
//!   or removes `lastdrop.exe` (updates-spec §2).
//! - What the dialog changed reaches the tray process as one posted message ([`notify`],
//!   [`Changes`]): it re-reads `settings.json` and applies it, or lifts the sign-in stop of a
//!   service whose API key was saved or removed.
//! - Settings ▸ Check now is answered the other way: the tray process posts what it found to the
//!   dialog listener ([`answer_check`], [`CheckResult`]), and the version card shows it.
//!
//! Threading: the main thread runs the Slint event loop and every window; the log has its own
//! writer thread ([`crate::logger`], file `settings.log`).

use std::any::Any;
use std::cell::RefCell;
use std::process::{ExitCode, Stdio};
use std::time::{Duration, Instant};

use lastdrop_core::custom;
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Language, Msg, Strings};
use lastdrop_core::model::{ProviderId, fnv1a64};
use lastdrop_core::update::Version;
use lastdrop_win::single_instance::{self, Instance, InstanceGuard};
use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
};

use crate::persist::Paths;
use crate::settings_window::Focus;
use crate::{AppError, history_window, logger, settings_window, ui, wiring};

/// How long a second launch keeps trying to reach a dialog process that is just starting (no
/// listener yet) or just ending (listener gone, mutex not yet released).
pub const FORWARD_WAIT: Duration = Duration::from_secs(2);

/// The `WPARAM` that asks the running dialog process to close every window and end, which the
/// installer posts before it replaces or removes `lastdrop.exe` (updates-spec §2). It is no
/// dialog's: its low byte is none of theirs, so [`DialogKind::from_wparam`] finds nothing for it,
/// and a dialog process from before it (0.1.x) logs it as an unknown request and goes on.
pub const CLOSE_REQUEST: usize = 0xFF;

/// The low byte of the `WPARAM` that answers Settings ▸ Check now ([`CheckResult::to_wparam`]).
/// Like [`CLOSE_REQUEST`] it is no dialog's, so a dialog process from before it (0.1.x) logs it
/// as an unknown request and goes on.
pub const CHECK_ANSWER: usize = 0xFE;

/// What Settings ▸ Check now found (updates-spec §1), as the tray process answers the dialog
/// process ([`answer_check`]); the notification says it too, with the details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckResult {
    /// This is the newest version.
    UpToDate,
    /// A newer version is out: which one, `None` when its number does not fit the message
    /// ([`CheckResult::to_wparam`]).
    Available(Option<Version>),
    /// The check failed.
    Failed,
}

impl CheckResult {
    /// Where the outcome is (bits 8–9: 1 up to date, 2 available, 3 failed).
    const OUTCOME_SHIFT: u32 = 8;
    /// Where the version of an available release is: major in bits 10–15, minor in 16–23, patch
    /// in 24–31 (all zero: not said). 32 bits, so the message is the same in a 32-bit build.
    const MAJOR_SHIFT: u32 = 10;
    /// See [`CheckResult::MAJOR_SHIFT`].
    const MINOR_SHIFT: u32 = 16;
    /// See [`CheckResult::MAJOR_SHIFT`].
    const PATCH_SHIFT: u32 = 24;

    /// A newer version, `version` (without a pre-release part, which the message does not carry).
    pub fn available(version: &Version) -> CheckResult {
        CheckResult::Available(Some(Version {
            pre: None,
            ..version.clone()
        }))
    }

    /// The answer as the dialog request's `WPARAM`: [`CHECK_ANSWER`] in the low byte, then the
    /// outcome and the version. A version past 63.255.255 is not said.
    pub fn to_wparam(&self) -> usize {
        let (outcome, version) = match self {
            CheckResult::UpToDate => (1, 0),
            CheckResult::Available(version) => {
                let packed = version
                    .as_ref()
                    .filter(|v| v.major < 64 && v.minor < 256 && v.patch < 256)
                    .and_then(|v| {
                        let [major, minor, patch] =
                            [v.major, v.minor, v.patch].map(|n| usize::try_from(n).ok());
                        Some(
                            major? << CheckResult::MAJOR_SHIFT
                                | minor? << CheckResult::MINOR_SHIFT
                                | patch? << CheckResult::PATCH_SHIFT,
                        )
                    })
                    .unwrap_or(0);
                (2, packed)
            }
            CheckResult::Failed => (3, 0),
        };
        CHECK_ANSWER | outcome << CheckResult::OUTCOME_SHIFT | version
    }

    /// The answer of a posted `WPARAM`; `None` for anything else (a dialog request).
    pub fn from_wparam(wparam: usize) -> Option<CheckResult> {
        if wparam & 0xFF != CHECK_ANSWER {
            return None;
        }
        let field = |shift: u32, bits: u32| {
            u32::try_from((wparam >> shift) & ((1 << bits) - 1)).unwrap_or(0)
        };
        match field(CheckResult::OUTCOME_SHIFT, 2) {
            1 => Some(CheckResult::UpToDate),
            2 => {
                let version = Version {
                    major: field(CheckResult::MAJOR_SHIFT, 6),
                    minor: field(CheckResult::MINOR_SHIFT, 8),
                    patch: field(CheckResult::PATCH_SHIFT, 8),
                    pre: None,
                };
                let said = version.major + version.minor + version.patch > 0;
                Some(CheckResult::Available(said.then_some(version)))
            }
            3 => Some(CheckResult::Failed),
            _ => None,
        }
    }
}

/// A dialog of the dialog process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// `--settings`: the settings window.
    Settings,
    /// `--settings-pins`: the settings window scrolled to the readout's pins (tray menu
    /// "Taskbar readout ▸ Pinned meters…", R§10).
    SettingsPins,
    /// `--settings-accounts`: the settings window scrolled to the accounts (the first run found
    /// more folders than the default ones, accounts-spec §6).
    SettingsAccounts,
    /// `--about`: the about window (R§10 "About Lastdrop", Slint licence).
    About,
    /// `--history`: the usage history window (charts-spec §1); `--history=<tag>` opens it at the
    /// service of that tag ([`crate::history_window::source_tag`]).
    History(Option<u32>),
}

impl DialogKind {
    /// Every dialog (the usage history without a service).
    pub const ALL: [DialogKind; 5] = [
        DialogKind::Settings,
        DialogKind::SettingsPins,
        DialogKind::SettingsAccounts,
        DialogKind::About,
        DialogKind::History(None),
    ];

    /// The `WPARAM` of the usage history; its tag is in bits 8–31.
    const HISTORY: usize = 5;

    /// Its command-line flag, without a tag.
    pub fn flag(self) -> &'static str {
        match self {
            DialogKind::Settings => "--settings",
            DialogKind::SettingsPins => "--settings-pins",
            DialogKind::SettingsAccounts => "--settings-accounts",
            DialogKind::About => "--about",
            DialogKind::History(_) => "--history",
        }
    }

    /// Its command-line argument: the flag, with the usage history's tag (`--history=0a1b2c`).
    pub fn arg(self) -> String {
        match self {
            DialogKind::History(Some(tag)) => format!("--history={tag:06x}"),
            _ => self.flag().to_owned(),
        }
    }

    /// The dialog of a command-line argument ([`DialogKind::arg`]).
    pub fn from_flag(flag: &str) -> Option<DialogKind> {
        if let Some(tag) = flag.strip_prefix("--history=") {
            let tag = u32::from_str_radix(tag, 16)
                .ok()
                .filter(|t| *t <= 0xFF_FFFF);
            return Some(DialogKind::History(tag));
        }
        DialogKind::ALL.into_iter().find(|k| k.flag() == flag)
    }

    /// The `WPARAM` a second launch posts for it (never 0, so a stray message asks for nothing).
    pub fn to_wparam(self) -> usize {
        match self {
            DialogKind::Settings => 1,
            DialogKind::SettingsPins => 2,
            DialogKind::About => 3,
            DialogKind::SettingsAccounts => 4,
            DialogKind::History(tag) => {
                let tag = tag.map_or(0, |t| usize::try_from(t & 0xFF_FFFF).unwrap_or(0));
                DialogKind::HISTORY | tag << 8
            }
        }
    }

    /// The dialog of a posted `WPARAM`; `None` for anything else.
    pub fn from_wparam(wparam: usize) -> Option<DialogKind> {
        if wparam & 0xFF == DialogKind::HISTORY {
            let tag = u32::try_from(wparam >> 8).ok().filter(|t| *t != 0);
            return Some(DialogKind::History(tag));
        }
        DialogKind::ALL
            .into_iter()
            .find(|k| k.to_wparam() == wparam)
    }
}

/// What the dialog process changed, as the flags of the settings-changed message's `WPARAM`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// `settings.json` was written (a settings window commit, Theme Studio's "Save and use"):
    /// re-read and apply it.
    pub settings: bool,
    /// Meshy's API key was saved or removed: lift Meshy's sign-in stop and read it again.
    pub meshy_key: bool,
    /// OpenCode Go's API key was saved or removed: the same for OpenCode Go.
    pub opencode_go_key: bool,
    /// A described service's API key was saved or removed: the same for that service. Sent as a
    /// hash of its file id, since the two processes may have loaded different files (one added
    /// since the tray process started); `None` in the tray process when it has no such service.
    pub custom_key: Option<ProviderId>,
    /// "Check now" in Settings ▸ Updates: check for an update now and say what was found
    /// (updates-spec §1).
    pub check_updates: bool,
    /// The installer is about to replace `lastdrop.exe`: quit (updates-spec §2).
    pub quit: bool,
}

impl Changes {
    /// The flag of [`Changes::settings`].
    const SETTINGS: usize = 1;
    /// The flag of [`Changes::meshy_key`].
    const MESHY_KEY: usize = 2;
    /// The flag of [`Changes::opencode_go_key`].
    const OPENCODE_GO_KEY: usize = 4;
    /// The flag of [`Changes::custom_key`]; the service's [`Changes::custom_hash`] is in bits
    /// 8–31.
    const CUSTOM_KEY: usize = 8;
    /// The flag of [`Changes::check_updates`].
    const CHECK_UPDATES: usize = 16;
    /// The flag of [`Changes::quit`].
    const QUIT: usize = 32;
    /// Where the hash of [`Changes::custom_key`] starts.
    const CUSTOM_SHIFT: u32 = 8;

    /// The change of `provider`'s API key; nothing for a service without one.
    pub fn key_of(provider: ProviderId) -> Changes {
        Changes {
            meshy_key: provider == ProviderId::Meshy,
            opencode_go_key: provider == ProviderId::OpenCodeGo,
            custom_key: matches!(provider, ProviderId::Custom(_)).then_some(provider),
            ..Changes::default()
        }
    }

    /// The services whose API key changed.
    pub fn keys(self) -> Vec<ProviderId> {
        [
            (self.meshy_key, ProviderId::Meshy),
            (self.opencode_go_key, ProviderId::OpenCodeGo),
        ]
        .into_iter()
        .filter_map(|(changed, p)| changed.then_some(p))
        .chain(self.custom_key)
        .collect()
    }

    /// 24 bits of the FNV-1a hash of a described service's file id. Two services sharing them
    /// only lift one stop too many.
    fn custom_hash(id: &str) -> usize {
        usize::try_from(fnv1a64(&[id]) & 0xFF_FFFF).unwrap_or(0)
    }

    /// The flags as a `WPARAM`.
    pub fn to_wparam(self) -> usize {
        let mut flags = 0;
        if self.settings {
            flags |= Changes::SETTINGS;
        }
        if self.meshy_key {
            flags |= Changes::MESHY_KEY;
        }
        if self.opencode_go_key {
            flags |= Changes::OPENCODE_GO_KEY;
        }
        if self.check_updates {
            flags |= Changes::CHECK_UPDATES;
        }
        if self.quit {
            flags |= Changes::QUIT;
        }
        if let Some(provider) = self.custom_key {
            flags |= Changes::CUSTOM_KEY
                | Changes::custom_hash(provider.as_str()) << Changes::CUSTOM_SHIFT;
        }
        flags
    }

    /// The flags of a posted `WPARAM`; unknown bits (a newer build's) are ignored. A described
    /// service's hash is matched against this process's services.
    pub fn from_wparam(wparam: usize) -> Changes {
        let hash = (wparam >> Changes::CUSTOM_SHIFT) & 0xFF_FFFF;
        let custom_key = (wparam & Changes::CUSTOM_KEY != 0)
            .then(|| {
                custom::all()
                    .into_iter()
                    .map(ProviderId::Custom)
                    .find(|p| Changes::custom_hash(p.as_str()) == hash)
            })
            .flatten();
        Changes {
            settings: wparam & Changes::SETTINGS != 0,
            meshy_key: wparam & Changes::MESHY_KEY != 0,
            opencode_go_key: wparam & Changes::OPENCODE_GO_KEY != 0,
            custom_key,
            check_updates: wparam & Changes::CHECK_UPDATES != 0,
            quit: wparam & Changes::QUIT != 0,
        }
    }
}

/// From the tray process: starts `lastdrop.exe <flag>` (this executable) for `kind`, without a
/// console and without waiting; its handles are closed at once. Passes the foreground right to
/// it so the dialog can come to the front. A dialog process that runs already gets the request
/// from the new one ([`run`]).
pub fn spawn(kind: DialogKind) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg(kind.arg())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id();
    // Dropping the `Child` closes its process and thread handles; nothing waits for it.
    drop(child);
    // SAFETY: plain call. Failure only means the dialog may open behind the foreground window.
    if let Err(e) = unsafe { AllowSetForegroundWindow(pid) } {
        log::debug!("dialog: AllowSetForegroundWindow failed: {e}");
    }
    log::info!("dialog process started ({})", kind.flag());
    Ok(())
}

/// From the dialog process: tells the tray process what changed; whether it was told. Not
/// running is fine: it reads `settings.json` and Credential Manager when it starts.
pub fn notify(changes: Changes) -> bool {
    match single_instance::signal_settings_changed(changes.to_wparam()) {
        Ok(true) => true,
        Ok(false) => {
            log::info!("Lastdrop is not running; it reads the change when it starts");
            false
        }
        Err(e) => {
            log::warn!("cannot tell Lastdrop about the change: {e}");
            false
        }
    }
}

/// From the tray process: tells the dialog process what Settings ▸ Check now found. No dialog
/// process (the settings window closed meanwhile) is fine: the notification says it too.
pub fn answer_check(result: &CheckResult) {
    match single_instance::post_dialog(result.to_wparam()) {
        Ok(true) => {}
        Ok(false) => log::info!("update: no settings window to answer"),
        Err(e) => log::warn!("update: cannot answer the settings window: {e}"),
    }
}

/// The dialog process's `main` (`lastdrop.exe --settings | --settings-pins | --settings-accounts
/// | --about`): the dialog mutex (or the request forwarded to the dialog process that runs
/// already), folders, its own log file, the Slint backend, the requested window, the event loop
/// until the last window closes.
pub fn run(kind: DialogKind) -> ExitCode {
    let guard = match first_or_forward(kind) {
        Ok(Some(guard)) => guard,
        Ok(None) => return ExitCode::SUCCESS,
        Err(_) => return ExitCode::FAILURE,
    };
    let Ok(paths) = Paths::from_env() else {
        return ExitCode::FAILURE;
    };
    let _ = logger::init_named(&paths.logs, logger::DIALOG_STEM, log::LevelFilter::Info);
    log::info!(
        "Lastdrop {} dialog process starting ({})",
        env!("CARGO_PKG_VERSION"),
        kind.flag()
    );
    crate::custom_services::load(&paths.providers);
    let code = match run_dialogs(&guard, &paths, kind) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("{e}");
            ExitCode::FAILURE
        }
    };
    // A later launch finds no dialog process from here on and starts its own.
    drop(guard);
    log::info!("dialog process stopped");
    logger::shutdown();
    code
}

/// The dialog mutex, or `None` once the request reached the dialog process that runs already. A
/// dialog process that is starting or ending is retried for up to [`FORWARD_WAIT`]; after that
/// the request is dropped (the user can ask again).
fn first_or_forward(kind: DialogKind) -> std::io::Result<Option<InstanceGuard>> {
    let deadline = Instant::now() + FORWARD_WAIT;
    loop {
        match single_instance::acquire_dialog()? {
            Instance::First(guard) => return Ok(Some(guard)),
            Instance::Second => {
                if single_instance::signal_dialog(kind.to_wparam())? {
                    return Ok(None);
                }
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The Slint backend, the dialog listener and the first window; then the event loop, which ends
/// when the last window closes (Close, the title bar's ✕, About's Close).
fn run_dialogs(guard: &InstanceGuard, paths: &Paths, kind: DialogKind) -> Result<(), AppError> {
    wiring::select_backend()?;
    let listen_paths = paths.clone();
    let listener = guard.listen_for(Box::new(move |wparam| {
        // A window procedure: open on the next turn of the event loop, never re-enter Slint.
        let paths = listen_paths.clone();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if wparam == CLOSE_REQUEST {
                log::info!("asked to close (an install or uninstall)");
                close_all();
                return;
            }
            if let Some(result) = CheckResult::from_wparam(wparam) {
                log::info!("update check answered: {result:?}");
                settings_window::check_answered(result);
                return;
            }
            match DialogKind::from_wparam(wparam) {
                Some(kind) => {
                    log::info!("asked for {}", kind.flag());
                    if let Err(e) = open(&paths, kind) {
                        log::error!("{}: {e}", kind.flag());
                    }
                }
                None => log::warn!("unknown dialog request {wparam}"),
            }
        });
    }))?;
    // Without a window the event loop would never end.
    let ran = open(paths, kind).and_then(|()| Ok(slint::run_event_loop()?));
    drop(listener);
    // Every window is dropped here, before `main` returns, not by thread-local teardown at exit.
    settings_window::release();
    history_window::release();
    crate::theme_studio::release();
    release_about();
    drop_closed();
    ran
}

/// Opens `kind`, or brings it to the front when it is open.
fn open(paths: &Paths, kind: DialogKind) -> Result<(), AppError> {
    match kind {
        DialogKind::Settings => settings_window::open(paths, Focus::None),
        DialogKind::SettingsPins => settings_window::open(paths, Focus::Pins),
        DialogKind::SettingsAccounts => settings_window::open(paths, Focus::Accounts),
        DialogKind::About => open_about(paths),
        DialogKind::History(tag) => history_window::open(paths, tag),
    }
}

/// Closes every window as its ✕ does, for the installer ([`CLOSE_REQUEST`]): the settings window
/// saves what its fields hold first, and stays open with the InfoBar of a failed close only when
/// that cannot be written; Theme Studio drops a draft not saved; the usage history and About have
/// nothing to keep. The last window closing ends the event loop, and with it the process.
fn close_all() {
    settings_window::close();
    crate::theme_studio::close();
    history_window::close();
    close_about();
}

/// Brings a dialog window to the front once winit has created it: gives it the app icon (title
/// bar, Alt+Tab, taskbar button), restores it when minimized, then `SetForegroundWindow`, which
/// Windows allows because the tray process (or the second launch) passed its foreground right on.
pub(crate) fn bring_to_front<T: ComponentHandle + 'static>(weak: slint::Weak<T>) {
    let spawned = slint::spawn_local(async move {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if let Err(e) = window.window().winit_window().await {
            log::warn!("window not created: {e}");
            return;
        }
        let Some(hwnd) = wiring::hwnd_of_window(window.window()) else {
            return;
        };
        if let Err(e) = lastdrop_win::app_icon::set(hwnd.0 as isize) {
            log::debug!("dialog: no app icon: {e}");
        }
        // SAFETY: plain calls on our own top-level window.
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            if !SetForegroundWindow(hwnd).as_bool() {
                log::debug!("dialog: SetForegroundWindow refused");
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("cannot bring the window to the front: {e}");
    }
}

thread_local! {
    /// The about window while it is open (main thread).
    static ABOUT: RefCell<Option<ui::AboutWindow>> = const { RefCell::new(None) };
    /// Windows closed from their own callbacks, waiting to be dropped ([`release_later`]).
    static CLOSED: RefCell<Vec<Box<dyn Any>>> = const { RefCell::new(Vec::new()) };
}

/// Drops a closed window (or what holds it) on the next turn of the event loop, never inside its
/// own callback. When that closing ends the event loop, the turn never comes and [`run`] drops it
/// after the loop instead; the timer itself holds nothing.
pub(crate) fn release_later(closed: impl Any) {
    CLOSED.with(|c| c.borrow_mut().push(Box::new(closed)));
    slint::Timer::single_shot(Duration::ZERO, drop_closed);
}

/// Drops every window [`release_later`] holds.
fn drop_closed() {
    let closed = CLOSED.with(|c| std::mem::take(&mut *c.borrow_mut()));
    drop(closed);
}

/// The about window's texts in the language of `strings`, with this build's version.
pub(crate) fn about_texts(strings: &dyn Strings) -> ui::AboutTexts {
    let t = |msg: Msg| slint::SharedString::from(strings.template(msg));
    ui::AboutTexts {
        title: t(Msg::AboutTitle),
        heading: text(
            strings,
            Msg::AboutHeading,
            &[("version", env!("CARGO_PKG_VERSION"))],
        )
        .into(),
        tagline: t(Msg::AboutTagline),
        copyright: t(Msg::AboutCopyright),
        credits: t(Msg::AboutCredits),
        support: t(Msg::AboutSupport),
        close: t(Msg::AboutClose),
    }
}

/// Opens the about window (R§10 "About Lastdrop", Slint licence) in the language of the
/// settings (else Windows' display language), or brings it to the front.
fn open_about(paths: &Paths) -> Result<(), AppError> {
    let existing = ABOUT.with(|a| a.borrow().as_ref().map(ComponentHandle::clone_strong));
    let window = match existing {
        Some(window) => window,
        None => {
            let (settings, _) = crate::persist::load_settings(&paths.settings);
            let language = Language::resolve(
                settings.language.as_deref(),
                &lastdrop_win::locale::ui_languages(),
            );
            let window = wiring::create_window(false, ui::AboutWindow::new)?;
            window.set_texts(about_texts(language.strings()));
            window.on_dismiss(close_about);
            window.on_support(open_support_page);
            window.window().on_close_requested(|| {
                close_about();
                slint::CloseRequestResponse::HideWindow
            });
            ABOUT.with(|a| *a.borrow_mut() = Some(window.clone_strong()));
            window
        }
    };
    if let Err(e) = window.show() {
        release_about();
        return Err(e.into());
    }
    bring_to_front(window.as_weak());
    Ok(())
}

/// The README's "Support Lastdrop" section (GitHub Sponsors, Buy Me a Coffee): one stable address
/// for every way to support Lastdrop, whichever of them exist.
const SUPPORT_URL: &str = "https://github.com/2Fivers/lastdrop#support-lastdrop";

/// About's "Support Lastdrop" link (R§10): the support page in the browser, on the user's click
/// only. The window stays open.
fn open_support_page() {
    if let Err(e) = lastdrop_win::shell::open_url(SUPPORT_URL) {
        log::warn!("cannot open the support page: {e}");
    }
}

/// Closes the about window and releases it ([`release_later`]). The last window closing ends the
/// event loop.
fn close_about() {
    let window = ABOUT.with(|a| a.borrow_mut().take());
    if let Some(window) = window {
        let _ = window.hide();
        release_later(window);
    }
}

/// Drops the about window (after the event loop).
fn release_about() {
    drop(ABOUT.with(|a| a.borrow_mut().take()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dialog_requests_round_trip_through_the_wparam() {
        for kind in DialogKind::ALL {
            assert_eq!(DialogKind::from_wparam(kind.to_wparam()), Some(kind));
            assert_eq!(DialogKind::from_flag(kind.flag()), Some(kind));
            assert_ne!(kind.to_wparam(), 0, "0 asks for nothing");
        }
        let wparams: std::collections::BTreeSet<usize> =
            DialogKind::ALL.iter().map(|k| k.to_wparam()).collect();
        assert_eq!(wparams.len(), DialogKind::ALL.len(), "distinct");
        assert_eq!(DialogKind::from_wparam(0), None);
        assert_eq!(DialogKind::from_wparam(99), None);
        assert_eq!(DialogKind::from_flag("--autostart"), None);

        // The usage history at a service: its tag rides along on the command line and the
        // message.
        let at = DialogKind::History(Some(0x0a_1b2c));
        assert_eq!(at.arg(), "--history=0a1b2c");
        assert_eq!(DialogKind::from_flag(&at.arg()), Some(at));
        assert_eq!(DialogKind::from_wparam(at.to_wparam()), Some(at));
        assert_eq!(
            DialogKind::from_flag("--history=zz"),
            Some(DialogKind::History(None)),
            "an unreadable tag opens it without one"
        );
        assert_eq!(DialogKind::from_wparam(5), Some(DialogKind::History(None)));
    }

    #[test]
    fn the_close_request_is_no_dialog() {
        // A dialog process ignores a request it does not know. 0.1.x reads requests with the same
        // `from_wparam`, so an older one leaves its windows open but never opens one for it.
        assert_eq!(DialogKind::from_wparam(CLOSE_REQUEST), None);
        assert_ne!(CLOSE_REQUEST, 0, "0 asks for nothing");
        for kind in [DialogKind::History(Some(0xFF_FFFF))]
            .into_iter()
            .chain(DialogKind::ALL)
        {
            assert_ne!(kind.to_wparam() & 0xFF, CLOSE_REQUEST & 0xFF, "{kind:?}");
            assert_ne!(kind.to_wparam() & 0xFF, CHECK_ANSWER, "{kind:?}");
        }
        assert_eq!(CheckResult::from_wparam(CLOSE_REQUEST), None);
    }

    #[test]
    fn check_answers_round_trip_and_are_no_dialog() {
        let v = |s: &str| Version::parse(s).expect("a version");
        for result in [
            CheckResult::UpToDate,
            CheckResult::Failed,
            CheckResult::available(&v("0.2.0")),
            CheckResult::available(&v("63.255.255")),
            CheckResult::available(&v("1.0.7")),
        ] {
            let wparam = result.to_wparam();
            assert_eq!(CheckResult::from_wparam(wparam), Some(result.clone()));
            assert_eq!(DialogKind::from_wparam(wparam), None, "{result:?}");
            assert!(u32::try_from(wparam).is_ok(), "fits a 32-bit WPARAM");
        }
        assert_eq!(
            CheckResult::from_wparam(CheckResult::available(&v("0.3.0-rc.1")).to_wparam()),
            Some(CheckResult::available(&v("0.3.0"))),
            "the pre-release part is not sent"
        );
        assert_eq!(
            CheckResult::from_wparam(CheckResult::available(&v("64.0.0")).to_wparam()),
            Some(CheckResult::Available(None)),
            "a number that does not fit is not said"
        );
        assert_eq!(CheckResult::from_wparam(CHECK_ANSWER), None, "no outcome");
        for kind in DialogKind::ALL {
            assert_eq!(CheckResult::from_wparam(kind.to_wparam()), None);
        }
    }

    #[test]
    fn change_flags_round_trip_and_ignore_unknown_bits() {
        for settings in [false, true] {
            for meshy_key in [false, true] {
                let changes = Changes {
                    settings,
                    meshy_key,
                    opencode_go_key: !meshy_key,
                    custom_key: None,
                    check_updates: settings,
                    quit: !settings,
                };
                assert_eq!(Changes::from_wparam(changes.to_wparam()), changes);
            }
        }
        assert_eq!(Changes::default().to_wparam(), 0);
        assert_eq!(
            Changes::from_wparam(1 | 64),
            Changes {
                settings: true,
                ..Changes::default()
            }
        );
        assert_eq!(
            Changes::from_wparam(2),
            Changes {
                meshy_key: true,
                ..Changes::default()
            }
        );
        assert_eq!(
            Changes::key_of(ProviderId::OpenCodeGo).keys(),
            [ProviderId::OpenCodeGo]
        );
        assert_eq!(Changes::key_of(ProviderId::Claude), Changes::default());
    }
}
