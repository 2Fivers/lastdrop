//! Updates in the tray process (docs/design/updates-spec.md §1, §3): a check two minutes after
//! start and then once a day while "Check for updates" is on, or now from Settings; a notification
//! and the flyout's footer `Lastdrop 0.2.0 is available · Install` when a newer release is out;
//! and the install: download the installer and `SHA256SUMS` to `%LOCALAPPDATA%\Lastdrop\updates`,
//! check the size and the SHA-256, start the installer with `--install --update` and quit (the
//! installer waits for this process, replaces `lastdrop.exe` and starts it again).
//!
//! Threading: the requests and the download run on a thread of their own per check or install;
//! results come back through `slint::invoke_from_event_loop`. Everything else is main thread.

use std::path::{Path, PathBuf};
use std::time::Duration;

use lastdrop_core::flyout::{CardAction, FlyoutModel, FooterNote, LinkModel};
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Msg, Strings};
use lastdrop_core::update::{
    self, MAX_SETUP_BYTES, NoOffer, Offer, RELEASES_REPO, SETUP_ASSET, SUMS_ASSET, Version,
};
use lastdrop_providers::real::UreqHttp;
use lastdrop_providers::release::{self, Latest};
use lastdrop_win::tray::NoticeIcon;
use slint::TimerMode;

use crate::wiring::{self, App, with_app};

/// The first check after start-up.
pub const FIRST_CHECK: Duration = Duration::from_secs(120);

/// Checks after that.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);

/// The largest `SHA256SUMS` accepted.
const MAX_SUMS_BYTES: u64 = 64 << 10;

/// Where the updates are.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UpdateState {
    /// Nothing known (not checked, or up to date).
    #[default]
    Idle,
    /// A newer release is out.
    Available(Offer),
    /// Its installer is being downloaded and checked.
    Downloading(Offer),
}

/// What a check found.
enum Checked {
    Offer(Offer),
    UpToDate,
    Failed(String),
}

/// Starts or stops the daily checks to match the settings (start-up and every settings change).
pub fn plan(app: &App) {
    let on = app.state.borrow().settings.updates.check;
    let timer = &app.rt.update_timer;
    if !on {
        timer.stop();
        return;
    }
    if timer.running() {
        return;
    }
    timer.start(TimerMode::SingleShot, FIRST_CHECK, || {
        with_app(|app| {
            check(app, false);
            app.rt
                .update_timer
                .start(TimerMode::Repeated, CHECK_EVERY, || {
                    with_app(|app| check(app, false));
                });
        });
    });
}

/// Asks GitHub for the latest release on a thread of its own; `manual` (Settings ▸ Check now)
/// also says when there is nothing new or the check failed.
pub fn check(app: &App, manual: bool) {
    // An available offer stays shown while a later check runs.
    if app.rt.update_checking.replace(true)
        || matches!(*app.rt.update.borrow(), UpdateState::Downloading(_))
    {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("update-check".to_owned())
        .spawn(move || {
            let result = match release::latest(&UreqHttp::new(), RELEASES_REPO) {
                Ok(Latest::Release(json)) => match update::offer(&json, &Version::current()) {
                    Ok(offer) => Checked::Offer(offer),
                    Err(NoOffer::UpToDate) => Checked::UpToDate,
                    Err(NoOffer::Unusable(why)) => Checked::Failed(why),
                },
                Ok(Latest::None) => Checked::UpToDate,
                Err(e) => Checked::Failed(e.to_string()),
            };
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| on_checked(app, result, manual));
            });
        });
    if let Err(e) = spawned {
        log::warn!("update: no thread to check: {e}");
        app.rt.update_checking.set(false);
    }
}

fn notify(app: &App, title: &str, body: &str, warning: bool) {
    let icon = if warning {
        NoticeIcon::Warning
    } else {
        NoticeIcon::Info
    };
    if let Err(e) = app.tray.notify(title, body, icon) {
        log::warn!("update notification not shown: {e}");
    }
}

/// Whether an offer of `offer` is announced by a notification: once per version (`announced`,
/// the version announced last, is kept in the state file, so a restart repeats nothing), and
/// always after *Check now* (`manual`), which says what it found.
fn announces(offer: &Version, announced: Option<&Version>, manual: bool) -> bool {
    manual || announced != Some(offer)
}

fn on_checked(app: &App, result: Checked, manual: bool) {
    app.rt.update_checking.set(false);
    let strings = app.state.borrow().strings;
    match result {
        Checked::Offer(offer) => {
            log::info!("update: {} is available", offer.version);
            let version = offer.version.to_string();
            let announced = app.state.borrow().update_announced.clone();
            if announces(&offer.version, announced.as_ref(), manual) {
                let title = text(strings, Msg::UpdateAvailable, &[("version", &version)]);
                notify(
                    app,
                    &title,
                    strings.template(Msg::UpdateAvailableBody),
                    false,
                );
                // Kept in the state file by the rebuild below.
                app.state.borrow_mut().update_announced = Some(offer.version.clone());
            }
            *app.rt.update.borrow_mut() = UpdateState::Available(offer);
        }
        Checked::UpToDate => {
            log::info!("update: up to date");
            *app.rt.update.borrow_mut() = UpdateState::Idle;
            if manual {
                let body = text(
                    strings,
                    Msg::UpdateCurrent,
                    &[("version", env!("CARGO_PKG_VERSION"))],
                );
                notify(app, "Lastdrop", &body, false);
            }
        }
        Checked::Failed(why) => {
            log::warn!("update: check failed: {why}");
            if manual {
                let body = text(strings, Msg::UpdateCheckFailed, &[("detail", &why)]);
                notify(app, "Lastdrop", &body, true);
            }
        }
    }
    wiring::rebuild(app, jiff::Timestamp::now());
}

/// The flyout's footer note for an available update (F§5), unless the pause's note is there.
pub fn footer(app: &App, model: &mut FlyoutModel, strings: &dyn Strings) {
    if model.footer_note.is_some() {
        return;
    }
    let (version, label) = match &*app.rt.update.borrow() {
        UpdateState::Available(offer) => (
            offer.version.to_string(),
            strings.template(Msg::UpdateInstall).to_owned(),
        ),
        UpdateState::Downloading(offer) => (offer.version.to_string(), String::new()),
        _ => return,
    };
    let note = if label.is_empty() {
        Msg::UpdateDownloading
    } else {
        Msg::UpdateAvailable
    };
    model.footer_note = Some(FooterNote {
        text: text(strings, note, &[("version", &version)]),
        link: LinkModel {
            label,
            action: CardAction::InstallUpdate,
        },
    });
}

/// The footer link: the pause's `Resume`, else the update's `Install`.
pub fn install(app: &App) {
    let offer = match &*app.rt.update.borrow() {
        UpdateState::Available(offer) => offer.clone(),
        _ => return,
    };
    *app.rt.update.borrow_mut() = UpdateState::Downloading(offer.clone());
    wiring::rebuild(app, jiff::Timestamp::now());
    let dir = app.paths.data.join("updates");
    let spawned = std::thread::Builder::new()
        .name("update-download".to_owned())
        .spawn(move || {
            let result = fetch(&offer, &dir);
            let _ = slint::invoke_from_event_loop(move || {
                with_app(|app| on_fetched(app, offer, result));
            });
        });
    if let Err(e) = spawned {
        log::warn!("update: no thread to download: {e}");
    }
}

/// Downloads and checks the installer of `offer` into `dir`; its path.
fn fetch(offer: &Offer, dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let sums_path = dir.join(SUMS_ASSET);
    release::download(&offer.sums.url, &sums_path, MAX_SUMS_BYTES).map_err(|e| e.to_string())?;
    let sums = std::fs::read_to_string(&sums_path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&sums_path);
    let expected = update::checksum_of(&sums, SETUP_ASSET)
        .ok_or_else(|| format!("{SUMS_ASSET} does not list {SETUP_ASSET}"))?;
    let setup = dir.join(SETUP_ASSET);
    let written =
        release::download(&offer.setup.url, &setup, MAX_SETUP_BYTES).map_err(|e| e.to_string())?;
    let verified = (|| {
        if written != offer.setup.size {
            return Err(format!("{written} bytes instead of {}", offer.setup.size));
        }
        let digest = lastdrop_win::install::sha256_file(&setup).map_err(|e| e.to_string())?;
        if update::hex(&digest) != expected {
            return Err("the SHA-256 does not match".to_owned());
        }
        Ok(())
    })();
    if let Err(e) = verified {
        let _ = std::fs::remove_file(&setup);
        return Err(e);
    }
    Ok(setup)
}

fn on_fetched(app: &App, offer: Offer, result: Result<PathBuf, String>) {
    let strings = app.state.borrow().strings;
    let started = result.and_then(|setup| {
        log::info!("update: installing {}", offer.version);
        std::process::Command::new(&setup)
            .args(["--install", "--update"])
            .current_dir(setup.parent().unwrap_or(Path::new(".")))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
    match started {
        Ok(()) => wiring::quit(app),
        Err(why) => {
            log::warn!("update: not installed: {why}");
            *app.rt.update.borrow_mut() = UpdateState::Available(offer);
            let body = text(strings, Msg::UpdateFailed, &[("detail", &why)]);
            notify(app, "Lastdrop", &body, true);
            wiring::rebuild(app, jiff::Timestamp::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_version_is_announced_once_unless_asked() {
        let v = |s: &str| Version::parse(s).expect("a version");
        assert!(announces(&v("0.2.0"), None, false), "the first offer");
        assert!(
            !announces(&v("0.2.0"), Some(&v("0.2.0")), false),
            "announced already (also before a restart)"
        );
        assert!(
            announces(&v("0.2.0"), Some(&v("0.2.0")), true),
            "Check now says it again"
        );
        assert!(
            announces(&v("0.3.0"), Some(&v("0.2.0")), false),
            "a newer release"
        );
    }
}
