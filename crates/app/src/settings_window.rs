//! The settings window (ROADMAP M1 "settings basics"; out of both specs' scope, F§12): services
//! on/off, the Meshy API key (Credential Manager, never echoed back), the refresh interval, the
//! readout (on/off, auto or pinned meters, time style), the global hotkey and start with Windows.
//!
//! The window edits a [`Draft`] of the settings; **Apply** commits it (state, settings file,
//! autostart, hotkey, readout). The API key is the exception: it is stored at once when the user
//! saves it and never kept anywhere else — not in the draft, not in a Slint property, not in
//! `settings.json`, not in the log.
//!
//! Threading: main thread.

use jiff::Timestamp;
use lastdrop_core::grammar::TimeStyle;
use lastdrop_core::model::{AccountKey, MeterId, MeterKey, ProviderId, catalog};
use lastdrop_core::secret::SecretString;
use lastdrop_core::settings::{
    Hotkey, ProviderEntry, ReadoutContent, RefreshInterval, Settings, SourceSettings,
};
use lastdrop_providers::io::CredentialName;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::ui;
use crate::wiring::{self, App, toggle_in, with_app};

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
/// its latest reading reported (Claude's per-model weekly windows), with their labels.
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

/// Opens the settings window (created on first use); `focus_pins` scrolls to the readout's pins
/// (tray menu "Taskbar readout ▸ Pinned meters…").
pub fn open(app: &App, focus_pins: bool) {
    let created = {
        let mut slot = app.windows.settings.borrow_mut();
        if slot.is_none() {
            match wiring::create_window(false, ui::SettingsWindow::new) {
                Ok(window) => {
                    connect(&window);
                    *slot = Some(window);
                }
                Err(e) => {
                    log::error!("settings window: {e}");
                    return;
                }
            }
        }
        slot.as_ref().map(ComponentHandle::clone_strong)
    };
    let Some(window) = created else {
        return;
    };
    let mut settings = app.state.borrow().settings.clone();
    if focus_pins && !matches!(settings.readout.content, ReadoutContent::Pinned(_)) {
        settings.readout.enabled = true;
    }
    let draft = draft_of(app, settings);
    populate(app, &window, &draft);
    window.set_focus_pins(focus_pins);
    if focus_pins {
        window.set_readout_enabled(true);
        window.set_readout_content_index(1);
    }
    window.set_status_text(SharedString::new());
    window.set_hotkey_error(SharedString::new());
    *app.rt.settings_draft.borrow_mut() = Some(draft);
    if let Err(e) = window.show() {
        log::error!("settings window: {e}");
        return;
    }
    if focus_pins {
        window.invoke_show_pins();
    }
}

/// A draft of `settings` with its provider and meter rows.
fn draft_of(app: &App, settings: Settings) -> Draft {
    draft_with(settings, &reported_meters(app))
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

/// Meters the latest readings reported, with `{service} {short_name}` labels.
fn reported_meters(app: &App) -> Vec<(MeterKey, String)> {
    let st = app.state.borrow();
    st.sources
        .iter()
        .flat_map(|(key, source)| {
            source.latest.iter().flat_map(move |snap| {
                let meters = snap
                    .meters
                    .iter()
                    .map(|m| (m.id.clone(), m.names.short_name.clone()));
                let balances = snap
                    .balances
                    .iter()
                    .map(|b| (b.id.clone(), b.names.short_name.clone()));
                meters
                    .chain(balances)
                    .map(move |(id, short): (MeterId, String)| {
                        (
                            MeterKey {
                                provider: key.provider,
                                account: key.account.clone(),
                                meter: id,
                            },
                            format!("{} {short}", key.provider.display_name()),
                        )
                    })
            })
        })
        .collect()
}

/// Fills every property of the window from the draft.
fn populate(app: &App, window: &ui::SettingsWindow, draft: &Draft) {
    let stored =
        lastdrop_win::credentials::last_written(&meshy_credential(&draft.settings).target())
            .ok()
            .flatten()
            .is_some();
    fill(window, draft, &reported_meters(app), stored);
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

/// Fills the meter rows of the draft.
fn set_meters(app: &App, window: &ui::SettingsWindow, draft: &Draft) {
    fill_meters(window, draft, &reported_meters(app));
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

/// Runs `f` on the draft and the window, then refreshes the meter rows.
fn edit(f: impl FnOnce(&mut Draft)) {
    with_app(|app| {
        let window = app
            .windows
            .settings
            .borrow()
            .as_ref()
            .map(ComponentHandle::clone_strong);
        let Some(window) = window else {
            return;
        };
        let draft = {
            let mut slot = app.rt.settings_draft.borrow_mut();
            let Some(draft) = slot.as_mut() else {
                return;
            };
            f(draft);
            draft.meters = meter_choices(&draft.settings, &reported_meters(app))
                .into_iter()
                .map(|(k, _)| k)
                .collect();
            draft.clone()
        };
        set_meters(app, &window, &draft);
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
    window.on_set_meshy_key(|key| with_app(|app| store_key(app, &key)));
    window.on_clear_meshy_key(|| with_app(clear_key));
    window.on_apply(|| with_app(apply));
    window.on_dismiss(|| {
        with_app(|app| {
            *app.rt.settings_draft.borrow_mut() = None;
            if let Some(w) = app.windows.settings.borrow().as_ref() {
                let _ = w.hide();
            }
        });
    });
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

/// Stores the Meshy API key in Credential Manager at once and drops it. A stored key lifts
/// Meshy's sign-in stop (Meshy has no credential fingerprint, so only this can lift it) and
/// enables Meshy in the draft.
fn store_key(app: &App, key: &SharedString) {
    let secret = SecretString::new(key.trim().to_owned());
    if secret.is_empty() {
        return;
    }
    let window = app
        .windows
        .settings
        .borrow()
        .as_ref()
        .map(ComponentHandle::clone_strong);
    let credential = meshy_credential(&app.state.borrow().settings);
    let stored = lastdrop_win::credentials::write(&credential.target(), &secret);
    drop(secret);
    match stored {
        Ok(()) => {
            log::info!("Meshy API key stored");
            {
                let mut st = app.state.borrow_mut();
                for (k, source) in st.sources.iter_mut() {
                    if k.provider == ProviderId::Meshy {
                        source.refresh.auth_stopped = None;
                        source.refresh.backoff_until = None;
                        source.status.last_failure = None;
                        source.status.failure_detail = None;
                        source.status.sign_in_required = false;
                    }
                }
            }
            edit(|draft| {
                if let Some(p) = draft.settings.provider_mut(ProviderId::Meshy) {
                    p.enabled = true;
                }
            });
            if let Some(w) = &window {
                w.set_meshy_key_stored(true);
                let rows = w.get_providers();
                let draft = app.rt.settings_draft.borrow().clone();
                if let Some(draft) = draft {
                    for (i, id) in draft.providers.iter().enumerate() {
                        if let Some(mut row) = slint::Model::row_data(&rows, i) {
                            row.enabled = draft.settings.provider(*id).is_some_and(|p| p.enabled);
                            slint::Model::set_row_data(&rows, i, row);
                        }
                    }
                }
                w.set_status_text("Key saved. Apply to start reading Meshy.".into());
            }
            wiring::replan(app);
        }
        Err(e) => {
            log::warn!("cannot store the Meshy key: {e}");
            if let Some(w) = &window {
                w.set_status_text(format!("Couldn't store the key: {e}").into());
            }
        }
    }
}

/// Removes the stored Meshy API key.
fn clear_key(app: &App) {
    let credential = meshy_credential(&app.state.borrow().settings);
    let window = app
        .windows
        .settings
        .borrow()
        .as_ref()
        .map(ComponentHandle::clone_strong);
    let status = match lastdrop_win::credentials::delete(&credential.target()) {
        Ok(()) => {
            if let Some(w) = &window {
                w.set_meshy_key_stored(false);
            }
            "Key removed.".to_owned()
        }
        Err(e) => format!("Couldn't remove the key: {e}"),
    };
    if let Some(w) = &window {
        w.set_status_text(status.into());
    }
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

/// Apply: commits the draft with the window's fields (state, file, autostart, hotkey, readout).
fn apply(app: &App) {
    let window = app
        .windows
        .settings
        .borrow()
        .as_ref()
        .map(ComponentHandle::clone_strong);
    let Some(window) = window else {
        return;
    };
    let Some(mut settings) = app
        .rt
        .settings_draft
        .borrow()
        .as_ref()
        .map(|d| d.settings.clone())
    else {
        return;
    };
    settings.refresh.interval = interval_at(window.get_interval_index());
    settings.readout.enabled = window.get_readout_enabled();
    if window.get_readout_content_index() == 0 {
        settings.readout.content = ReadoutContent::Auto;
    }
    settings.readout.time_style = if window.get_time_style_index() == 1 {
        TimeStyle::Relative
    } else {
        TimeStyle::Clock
    };
    settings.start_with_windows = window.get_start_with_windows();
    let mut problems = Vec::new();
    match hotkey_of(&window) {
        Ok(hotkey) => {
            settings.hotkey = hotkey;
            window.set_hotkey_error(SharedString::new());
        }
        Err(why) => {
            window.set_hotkey_error(why.as_str().into());
            problems.push("the shortcut");
        }
    }

    let previous = app.state.borrow().settings.clone();
    // The pause may have changed from the tray menu while the window was open.
    settings.pause = previous.pause;

    if settings.start_with_windows != previous.start_with_windows {
        let changed = if settings.start_with_windows {
            std::env::current_exe().and_then(|exe| lastdrop_win::autostart::enable(&exe))
        } else {
            lastdrop_win::autostart::disable()
        };
        if let Err(e) = changed {
            log::warn!("autostart: {e}");
            settings.start_with_windows = previous.start_with_windows;
            window.set_start_with_windows(previous.start_with_windows);
            problems.push("start with Windows");
        }
    }
    if settings.hotkey != previous.hotkey {
        match &settings.hotkey {
            Some(hotkey) => {
                if let Some(why) = wiring::register_hotkey(app, hotkey) {
                    window.set_hotkey_error(why.as_str().into());
                    problems.push("the shortcut");
                }
            }
            None => {
                if let Some(old) = app.hotkey.borrow_mut().take() {
                    old.unregister();
                }
            }
        }
    }

    let allowed = app.readout_allowed(&settings);
    app.state.borrow_mut().apply_settings(settings.clone());
    app.readout.set_enabled(allowed);
    wiring::save_settings(app);
    if let Some(draft) = app.rt.settings_draft.borrow_mut().as_mut() {
        draft.settings = settings;
    }
    wiring::rebuild(app, Timestamp::now());
    wiring::replan(app);
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
    use lastdrop_core::settings::ProviderSettings;

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
}
