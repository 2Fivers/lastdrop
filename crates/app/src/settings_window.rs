//! The settings window (ROADMAP M1 "settings basics"; out of both specs' scope, F§12): services
//! on/off, the Meshy API key (Credential Manager, never echoed back), the refresh interval, the
//! readout (on/off, auto or pinned meters, time style), the global hotkey and start with Windows.
//!
//! It runs in the dialog process ([`crate::dialog`]), not in the tray process. Opening reads
//! `settings.json` and the meters earlier readings reported (`state.json`); the window edits a
//! [`Draft`] of the settings. **Apply** re-reads `settings.json`, takes every field from the
//! draft except the pause ([`apply_onto`]), sets autostart, tests a changed hotkey, writes the
//! file (atomic) and tells the tray process, which re-reads and applies it. The API key is the
//! exception: it is stored at once when the user saves it and never kept anywhere else — not in
//! the draft, not in a Slint property, not in `settings.json`, not in the log — and the tray
//! process is told so it reads Meshy again.
//!
//! Threading: the dialog process's main thread.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use lastdrop_core::grammar::TimeStyle;
use lastdrop_core::model::{AccountKey, MeterKey, MeterNames, ProviderId, catalog};
use lastdrop_core::secret::SecretString;
use lastdrop_core::settings::{
    Hotkey, ProviderEntry, ReadoutContent, RefreshInterval, Settings, SourceSettings,
};
use lastdrop_providers::io::CredentialName;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::dialog::{self, Changes};
use crate::persist::{self, LoadOutcome, Paths, SavedState};
use crate::wiring::{self, toggle_in};
use crate::{AppError, ui};

/// The refresh interval choices: label and minutes (`None` = manual).
pub const INTERVALS: [(&str, Option<u16>); 7] = [
    ("Manual", None),
    ("1 min", Some(1)),
    ("2 min", Some(2)),
    ("5 min", Some(5)),
    ("10 min", Some(10)),
    ("15 min", Some(15)),
    ("30 min", Some(30)),
];

/// The settings being edited.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The edited settings.
    pub settings: Settings,
    /// The providers listed, in order (row index → provider).
    pub providers: Vec<ProviderId>,
    /// The meters listed, in order (row index → meter).
    pub meters: Vec<MeterKey>,
}

/// The index in [`INTERVALS`] of an interval (the nearest one for other minute values).
pub fn interval_index(interval: RefreshInterval) -> usize {
    match interval {
        RefreshInterval::Manual => 0,
        RefreshInterval::Minutes(m) => INTERVALS
            .iter()
            .enumerate()
            .filter_map(|(i, (_, v))| v.map(|v| (i, v.abs_diff(m))))
            .min_by_key(|(_, d)| *d)
            .map_or(3, |(i, _)| i),
    }
}

/// The interval of an [`INTERVALS`] index.
pub fn interval_at(index: i32) -> RefreshInterval {
    let i = usize::try_from(index).unwrap_or(3).min(INTERVALS.len() - 1);
    match INTERVALS[i].1 {
        None => RefreshInterval::Manual,
        Some(m) => RefreshInterval::Minutes(m),
    }
}

/// The pins of `settings` (empty in auto mode).
fn pins(settings: &Settings) -> Vec<MeterKey> {
    match &settings.readout.content {
        ReadoutContent::Pinned(p) => p.clone(),
        ReadoutContent::Auto => Vec::new(),
    }
}

/// The meters to list for `settings`: every enabled service's catalogue meters plus the meters
/// its readings reported (Claude's per-model weekly windows), with their labels.
pub fn meter_choices(
    settings: &Settings,
    reported: &[(MeterKey, String)],
) -> Vec<(MeterKey, String)> {
    let mut out: Vec<(MeterKey, String)> = Vec::new();
    for (_, p) in settings.enabled_providers() {
        for account in &p.accounts {
            for def in catalog(p.id) {
                out.push((
                    MeterKey {
                        provider: p.id,
                        account: account.key.clone(),
                        meter: def.meter_id(),
                    },
                    format!("{} {}", p.id.display_name(), def.short_name),
                ));
            }
            for (key, label) in reported {
                if key.provider == p.id
                    && key.account == account.key
                    && !out.iter().any(|(k, _)| k == key)
                {
                    out.push((key.clone(), label.clone()));
                }
            }
        }
    }
    out
}

/// The meters the tray process's readings reported, as it remembers them in `state.json`
/// (`SavedState::seen`), plus any pinned meter it does not remember, each with a
/// `{service} {short_name}` label.
pub fn reported_meters(saved: &SavedState, settings: &Settings) -> Vec<(MeterKey, String)> {
    let label =
        |key: &MeterKey, short_name: &str| format!("{} {short_name}", key.provider.display_name());
    let mut out: Vec<(MeterKey, String)> = saved
        .seen
        .iter()
        .map(|m| (m.key.clone(), label(&m.key, &m.names.short_name)))
        .collect();
    for key in pins(settings) {
        if !out.iter().any(|(k, _)| *k == key) {
            let short_name = pinned_short_name(&key);
            out.push((key.clone(), label(&key, &short_name)));
        }
    }
    out
}

/// The `short_name` of a pinned meter no reading is remembered for: the catalogue's, a per-model
/// weekly window's (`weekly:fable` → `Fable weekly`), else its id.
fn pinned_short_name(key: &MeterKey) -> String {
    if let Some(def) = catalog(key.provider).iter().find(|d| d.id == key.meter.0) {
        return def.short_name.to_owned();
    }
    match key.meter.0.strip_prefix("weekly:") {
        Some(model) => {
            let mut chars = model.chars();
            let model: String = chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default();
            MeterNames::weekly_model(&model).short_name
        }
        None => key.meter.0.clone(),
    }
}

/// The Meshy API key's credential name for the primary account.
fn meshy_credential(settings: &Settings) -> CredentialName {
    let name = settings
        .provider(ProviderId::Meshy)
        .and_then(|p| p.accounts.first())
        .and_then(|a| match &a.source {
            SourceSettings::ApiKey { name } => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "api_key".to_owned());
    CredentialName {
        provider: ProviderId::Meshy,
        account: AccountKey::primary(),
        name,
    }
}

/// The open settings window and what it edits.
struct Session {
    /// The window.
    window: ui::SettingsWindow,
    /// The draft.
    draft: RefCell<Draft>,
    /// The meters the readings reported ([`reported_meters`]), read when the window opened.
    reported: Vec<(MeterKey, String)>,
    /// `settings.json`.
    settings_path: PathBuf,
}

thread_local! {
    /// The settings window while it is open (main thread). Callbacks look it up instead of
    /// capturing it, so closing can take it back.
    static SESSION: RefCell<Option<Rc<Session>>> = const { RefCell::new(None) };
}

/// Runs `f` with the open window's session. The `Rc` is cloned out first, so `f` may itself call
/// `with_session` (no `RefCell` borrow is held while it runs).
fn with_session(f: impl FnOnce(&Session)) {
    let session = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = session {
        f(&session);
    }
}

/// Opens the settings window, created on each open and released by [`close`]: reads
/// `settings.json` and `state.json`, fills the window and brings it to the front. `focus_pins`
/// scrolls to the readout's pins (tray menu "Taskbar readout ▸ Pinned meters…"). When the window
/// is open already it keeps its edits and only comes to the front (and to the pins).
pub fn open(paths: &Paths, focus_pins: bool) -> Result<(), AppError> {
    let open = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = open {
        let window = &session.window;
        if focus_pins {
            show_pins(window);
        }
        window.show()?;
        if focus_pins {
            window.invoke_show_pins();
        }
        dialog::bring_to_front(window.as_weak());
        return Ok(());
    }

    let (mut settings, outcome) = persist::load_settings(&paths.settings);
    log::info!("settings: {outcome:?}");
    let reported = reported_meters(&persist::load_state(&paths.state), &settings);
    if focus_pins && !matches!(settings.readout.content, ReadoutContent::Pinned(_)) {
        settings.readout.enabled = true;
    }
    let draft = draft_with(settings, &reported);
    let window = wiring::create_window(false, ui::SettingsWindow::new)?;
    connect(&window);
    let stored = meshy_key_stored(&draft.settings);
    fill(&window, &draft, &reported, stored);
    window.set_focus_pins(false);
    if focus_pins {
        show_pins(&window);
    }
    window.set_status_text(SharedString::new());
    window.set_hotkey_error(SharedString::new());
    SESSION.with(|s| {
        *s.borrow_mut() = Some(Rc::new(Session {
            window: window.clone_strong(),
            draft: RefCell::new(draft),
            reported,
            settings_path: paths.settings.clone(),
        }));
    });
    if let Err(e) = window.show() {
        release();
        return Err(e.into());
    }
    if focus_pins {
        window.invoke_show_pins();
    }
    dialog::bring_to_front(window.as_weak());
    Ok(())
}

/// Highlights the readout's pins: the readout on and in pinned mode (Apply saves it so).
fn show_pins(window: &ui::SettingsWindow) {
    window.set_focus_pins(true);
    window.set_readout_enabled(true);
    window.set_readout_content_index(1);
}

/// A draft of `settings` with the meters the readings reported.
pub fn draft_with(settings: Settings, reported: &[(MeterKey, String)]) -> Draft {
    let providers = settings
        .providers
        .iter()
        .filter_map(ProviderEntry::known)
        .map(|p| p.id)
        .collect();
    let meters = meter_choices(&settings, reported)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    Draft {
        settings,
        providers,
        meters,
    }
}

/// Whether a Meshy key is stored (its credential exists; the secret is not read into the app).
fn meshy_key_stored(settings: &Settings) -> bool {
    lastdrop_win::credentials::last_written(&meshy_credential(settings).target())
        .ok()
        .flatten()
        .is_some()
}

/// Fills every property of the window from the draft, the meters the readings reported and
/// whether a Meshy key is stored (no I/O; `--preview-settings` uses it directly).
pub fn fill(
    window: &ui::SettingsWindow,
    draft: &Draft,
    reported: &[(MeterKey, String)],
    meshy_key_stored: bool,
) {
    let s = &draft.settings;
    let rows: Vec<ui::ProviderRow> = s
        .providers
        .iter()
        .filter_map(ProviderEntry::known)
        .map(|p| ui::ProviderRow {
            id: p.id.as_str().into(),
            name: p.id.display_name().into(),
            enabled: p.enabled,
        })
        .collect();
    window.set_providers(ModelRc::new(VecModel::from(rows)));
    let choices: Vec<SharedString> = INTERVALS.iter().map(|(l, _)| (*l).into()).collect();
    window.set_interval_choices(ModelRc::new(VecModel::from(choices)));
    window.set_interval_index(i32::try_from(interval_index(s.refresh.interval)).unwrap_or(3));
    window.set_readout_enabled(s.readout.enabled);
    window.set_readout_content_index(i32::from(matches!(
        s.readout.content,
        ReadoutContent::Pinned(_)
    )));
    window.set_time_style_index(i32::from(s.readout.time_style == TimeStyle::Relative));
    fill_meters(window, draft, reported);
    window.set_start_with_windows(s.start_with_windows);
    window.set_meshy_key_stored(meshy_key_stored);
    match &s.hotkey {
        Some(h) => {
            window.set_hotkey_enabled(true);
            window.set_hotkey_ctrl(h.ctrl);
            window.set_hotkey_alt(h.alt);
            window.set_hotkey_shift(h.shift);
            window.set_hotkey_win(h.win);
            window.set_hotkey_key(h.key.as_str().into());
        }
        None => {
            window.set_hotkey_enabled(false);
            window.set_hotkey_ctrl(true);
            window.set_hotkey_alt(true);
            window.set_hotkey_shift(false);
            window.set_hotkey_win(false);
            window.set_hotkey_key("L".into());
        }
    }
}

/// Fills the meter rows of the draft from the reported meters.
fn fill_meters(window: &ui::SettingsWindow, draft: &Draft, reported: &[(MeterKey, String)]) {
    let pinned = pins(&draft.settings);
    let choices = meter_choices(&draft.settings, reported);
    let rows: Vec<ui::MeterChoice> = choices
        .iter()
        .map(|(key, label)| {
            let hidden = draft
                .settings
                .provider(key.provider)
                .is_some_and(|p| p.hidden_meters.contains(&key.meter));
            ui::MeterChoice {
                label: label.as_str().into(),
                shown: !hidden,
                pinned: pinned.contains(key),
            }
        })
        .collect();
    window.set_meters(ModelRc::new(VecModel::from(rows)));
}

/// Runs `f` on the draft, then refreshes the meter rows.
fn edit(f: impl FnOnce(&mut Draft)) {
    with_session(|session| {
        let draft = {
            let mut draft = session.draft.borrow_mut();
            f(&mut draft);
            draft.meters = meter_choices(&draft.settings, &session.reported)
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            draft.clone()
        };
        fill_meters(&session.window, &draft, &session.reported);
    });
}

/// Wires the window's callbacks.
fn connect(window: &ui::SettingsWindow) {
    window.on_provider_toggled(|index, on| {
        edit(|draft| {
            let id = usize::try_from(index)
                .ok()
                .and_then(|i| draft.providers.get(i).copied());
            if let Some(p) = id.and_then(|id| draft.settings.provider_mut(id)) {
                p.enabled = on;
            }
        });
    });
    window.on_meter_shown_toggled(|index, shown| {
        edit(|draft| {
            let Some(key) = usize::try_from(index)
                .ok()
                .and_then(|i| draft.meters.get(i).cloned())
            else {
                return;
            };
            if let Some(p) = draft.settings.provider_mut(key.provider) {
                p.hidden_meters.retain(|m| *m != key.meter);
                if !shown {
                    p.hidden_meters.push(key.meter.clone());
                }
            }
            if !shown {
                let mut pinned = pins(&draft.settings);
                pinned.retain(|k| *k != key);
                set_pins(&mut draft.settings, pinned);
            }
        });
    });
    window.on_meter_pinned_toggled(|index, pinned| {
        edit(|draft| {
            let Some(key) = usize::try_from(index)
                .ok()
                .and_then(|i| draft.meters.get(i).cloned())
            else {
                return;
            };
            let mut current = pins(&draft.settings);
            if pinned != current.contains(&key) {
                toggle_in(&mut current, key);
            }
            set_pins(&mut draft.settings, current);
        });
    });
    window.on_set_meshy_key(|key| with_session(|session| store_key(session, &key)));
    window.on_clear_meshy_key(|| with_session(clear_key));
    window.on_apply(|| with_session(apply));
    window.on_dismiss(close);
    window.window().on_close_requested(|| {
        close();
        slint::CloseRequestResponse::HideWindow
    });
}

/// Closes the settings window (Close button or the title bar's ✕) and releases it, not inside
/// the window's own callback ([`dialog::release_later`]). Unapplied edits are dropped. The last
/// window closing ends the dialog process's event loop.
fn close() {
    let session = SESSION.with(|s| s.borrow_mut().take());
    if let Some(session) = session {
        let _ = session.window.hide();
        dialog::release_later(session);
    }
}

/// Drops the settings window (after the event loop, or when it could not be shown).
pub fn release() {
    drop(SESSION.with(|s| s.borrow_mut().take()));
}

/// Pinned meters, or auto when none is left. Kept while the content mode is auto too, so
/// switching the combo box to "Meters I pin" finds them.
fn set_pins(settings: &mut Settings, pins: Vec<MeterKey>) {
    settings.readout.content = if pins.is_empty() {
        ReadoutContent::Auto
    } else {
        ReadoutContent::Pinned(pins)
    };
}

/// Stores the Meshy API key in Credential Manager at once and drops it, then tells the tray
/// process, which lifts Meshy's sign-in stop (Meshy has no credential fingerprint, so only this
/// can lift it). Enables Meshy in the draft (Apply saves that).
fn store_key(session: &Session, key: &SharedString) {
    let secret = SecretString::new(key.trim().to_owned());
    if secret.is_empty() {
        return;
    }
    let credential = meshy_credential(&session.draft.borrow().settings);
    let stored = lastdrop_win::credentials::write(&credential.target(), &secret);
    drop(secret);
    let window = &session.window;
    match stored {
        Ok(()) => {
            log::info!("Meshy API key stored");
            dialog::notify(Changes {
                meshy_key: true,
                ..Changes::default()
            });
            edit(|draft| {
                if let Some(p) = draft.settings.provider_mut(ProviderId::Meshy) {
                    p.enabled = true;
                }
            });
            window.set_meshy_key_stored(true);
            let rows = window.get_providers();
            let draft = session.draft.borrow().clone();
            for (i, id) in draft.providers.iter().enumerate() {
                if let Some(mut row) = slint::Model::row_data(&rows, i) {
                    row.enabled = draft.settings.provider(*id).is_some_and(|p| p.enabled);
                    slint::Model::set_row_data(&rows, i, row);
                }
            }
            window.set_status_text("Key saved. Apply to start reading Meshy.".into());
        }
        Err(e) => {
            log::warn!("cannot store the Meshy key: {e}");
            window.set_status_text(format!("Couldn't store the key: {e}").into());
        }
    }
}

/// Removes the stored Meshy API key and tells the tray process, which reads Meshy again (and so
/// shows that the key is missing).
fn clear_key(session: &Session) {
    let credential = meshy_credential(&session.draft.borrow().settings);
    let window = &session.window;
    let status = match lastdrop_win::credentials::delete(&credential.target()) {
        Ok(()) => {
            log::info!("Meshy API key removed");
            window.set_meshy_key_stored(false);
            dialog::notify(Changes {
                meshy_key: true,
                ..Changes::default()
            });
            "Key removed.".to_owned()
        }
        Err(e) => {
            log::warn!("cannot remove the Meshy key: {e}");
            format!("Couldn't remove the key: {e}")
        }
    };
    window.set_status_text(status.into());
}

/// The hotkey the window describes, or why it is not valid.
fn hotkey_of(window: &ui::SettingsWindow) -> Result<Option<Hotkey>, String> {
    if !window.get_hotkey_enabled() {
        return Ok(None);
    }
    let key = window.get_hotkey_key().trim().to_uppercase();
    let hotkey = Hotkey {
        ctrl: window.get_hotkey_ctrl(),
        alt: window.get_hotkey_alt(),
        shift: window.get_hotkey_shift(),
        win: window.get_hotkey_win(),
        key,
    };
    let valid_key = (hotkey.key.chars().count() == 1
        && hotkey.key.chars().all(|c| c.is_ascii_alphanumeric()))
        || hotkey
            .key
            .strip_prefix('F')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=24).contains(&n));
    if !valid_key || !(hotkey.ctrl || hotkey.alt || hotkey.shift || hotkey.win) {
        return Err(
            "Choose at least one of Ctrl, Alt, Shift or Win, and a letter, a digit or F1–F24."
                .to_owned(),
        );
    }
    Ok(Some(hotkey))
}

/// The settings Apply writes: every field of the draft, except `pause`, which stays as
/// `settings.json` has it now. The tray menu owns the pause and may have paused or resumed while
/// the window was open.
pub fn apply_onto(draft: Settings, disk: &Settings) -> Settings {
    Settings {
        pause: disk.pause,
        ..draft
    }
}

/// Apply: the draft with the window's fields onto `settings.json` as it is now ([`apply_onto`]);
/// autostart when "Start Lastdrop when I sign in" changed; a changed hotkey is tested by
/// registering it here and unregistering it at once (the tray process registers it for real);
/// the file is written atomically and the tray process is told.
fn apply(session: &Session) {
    let window = &session.window;
    let mut draft = session.draft.borrow().settings.clone();
    draft.refresh.interval = interval_at(window.get_interval_index());
    draft.readout.enabled = window.get_readout_enabled();
    if window.get_readout_content_index() == 0 {
        draft.readout.content = ReadoutContent::Auto;
    }
    draft.readout.time_style = if window.get_time_style_index() == 1 {
        TimeStyle::Relative
    } else {
        TimeStyle::Clock
    };
    draft.start_with_windows = window.get_start_with_windows();

    let (disk, outcome) = persist::load_settings(&session.settings_path);
    if !matches!(outcome, LoadOutcome::Loaded) {
        log::info!("settings before Apply: {outcome:?}");
    }
    let mut settings = apply_onto(draft, &disk);
    let mut problems = Vec::new();
    match hotkey_of(window) {
        Ok(hotkey) => {
            settings.hotkey = hotkey;
            window.set_hotkey_error(SharedString::new());
        }
        Err(why) => {
            settings.hotkey.clone_from(&disk.hotkey);
            window.set_hotkey_error(why.as_str().into());
            problems.push("the shortcut");
        }
    }

    if settings.start_with_windows != disk.start_with_windows {
        let changed = if settings.start_with_windows {
            std::env::current_exe().and_then(|exe| lastdrop_win::autostart::enable(&exe))
        } else {
            lastdrop_win::autostart::disable()
        };
        if let Err(e) = changed {
            log::warn!("autostart: {e}");
            settings.start_with_windows = disk.start_with_windows;
            window.set_start_with_windows(disk.start_with_windows);
            problems.push("start with Windows");
        }
    }
    if settings.hotkey != disk.hotkey
        && let Some(hotkey) = &settings.hotkey
    {
        match lastdrop_win::hotkey::register(hotkey, Box::new(|| {})) {
            Ok(registration) => registration.unregister(),
            Err(e) => {
                log::warn!("hotkey: {e}");
                window.set_hotkey_error(e.to_string().into());
                settings.hotkey.clone_from(&disk.hotkey);
                problems.push("the shortcut");
            }
        }
    }

    if let Err(e) = persist::save_settings(&session.settings_path, &settings) {
        log::error!("cannot save settings: {e}");
        window.set_status_text(format!("Couldn't save the settings: {e}").into());
        return;
    }
    dialog::notify(Changes {
        settings: true,
        ..Changes::default()
    });
    session.draft.borrow_mut().settings = settings;
    let status = if problems.is_empty() {
        "Saved.".to_owned()
    } else {
        format!("Saved, except {}.", problems.join(" and "))
    };
    window.set_status_text(status.into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use lastdrop_core::model::MeterId;
    use lastdrop_core::settings::{Pause, ProviderSettings};

    use crate::persist::SeenMeter;

    #[test]
    fn intervals_round_trip_and_snap_to_the_nearest_choice() {
        for (i, _) in INTERVALS.iter().enumerate() {
            let index = i32::try_from(i).expect("small");
            assert_eq!(interval_index(interval_at(index)), i);
        }
        assert_eq!(interval_index(RefreshInterval::Minutes(4)), 3);
        assert_eq!(interval_index(RefreshInterval::Minutes(25)), 6);
        assert_eq!(interval_at(99), RefreshInterval::Minutes(30));
        assert_eq!(interval_at(-1), RefreshInterval::Minutes(5));
    }

    #[test]
    fn meters_list_the_catalogue_and_reported_ones_of_enabled_services() {
        let settings = Settings {
            providers: vec![
                ProviderEntry::Known(ProviderSettings::new(ProviderId::Claude, true)),
                ProviderEntry::Known(ProviderSettings::new(ProviderId::Meshy, false)),
            ],
            ..Settings::default()
        };
        let fable = MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter: MeterId::weekly_model("fable"),
        };
        let choices = meter_choices(
            &settings,
            &[(fable.clone(), "Claude Fable weekly".to_owned())],
        );
        let labels: Vec<&str> = choices.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            labels,
            ["Claude 5-hour", "Claude weekly", "Claude Fable weekly"]
        );
        assert_eq!(choices[2].0, fable);
    }

    #[test]
    fn the_meshy_key_name_follows_the_account_source() {
        let settings = Settings::default();
        assert_eq!(
            meshy_credential(&settings).target(),
            "Lastdrop:meshy:default:api_key"
        );
    }

    fn claude(meter: MeterId) -> MeterKey {
        MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter,
        }
    }

    #[test]
    fn reported_meters_come_from_the_state_file_and_the_pins() {
        let fable = claude(MeterId::weekly_model("fable"));
        let saved = SavedState {
            seen: vec![SeenMeter {
                key: fable.clone(),
                names: MeterNames::weekly_model("Fable"),
                balance: false,
            }],
            ..SavedState::default()
        };
        let opus = claude(MeterId::weekly_model("opus"));
        let five = claude(MeterId::new(MeterId::FIVE_HOUR));
        let mut settings = Settings::default();
        set_pins(
            &mut settings,
            vec![fable.clone(), opus.clone(), five.clone()],
        );
        let reported = reported_meters(&saved, &settings);
        assert_eq!(
            reported,
            [
                (fable, "Claude Fable weekly".to_owned()),
                (opus, "Claude Opus weekly".to_owned()),
                (five, "Claude 5-hour".to_owned()),
            ]
        );
        assert!(reported_meters(&SavedState::default(), &Settings::default()).is_empty());
    }

    #[test]
    fn apply_keeps_the_pause_on_disk_and_takes_everything_else_from_the_draft() {
        let mut draft = Settings {
            start_with_windows: true,
            ..Settings::default()
        };
        draft.refresh.interval = RefreshInterval::Minutes(15);
        draft.readout.enabled = false;
        draft.hotkey = Some(Hotkey {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: "L".to_owned(),
        });
        draft.pause = None;
        set_pins(&mut draft, vec![claude(MeterId::new(MeterId::WEEKLY))]);

        // The tray menu paused while the window was open; the file also differs elsewhere.
        let mut disk = Settings {
            pause: Some(Pause { until: None }),
            ..Settings::default()
        };
        disk.refresh.interval = RefreshInterval::Manual;

        let applied = apply_onto(draft.clone(), &disk);
        assert_eq!(applied.pause, disk.pause, "the pause stays as on disk");
        assert_eq!(
            applied,
            Settings {
                pause: disk.pause,
                ..draft.clone()
            },
            "everything else is the draft's"
        );

        // Resumed from the tray while the window was open: the draft's old pause is not
        // brought back.
        let mut paused_draft = draft;
        paused_draft.pause = Some(Pause { until: None });
        assert_eq!(apply_onto(paused_draft, &Settings::default()).pause, None);
    }
}
