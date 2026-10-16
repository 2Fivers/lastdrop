//! The settings window (ROADMAP M1 "settings basics"; out of both specs' scope, F§12): services
//! on/off, Claude Code's and Codex's accounts ([`crate::settings_accounts`], accounts-spec §7),
//! the API keys of Meshy and OpenCode Go (Credential Manager, never echoed back), the refresh
//! interval, the
//! readout (on/off, auto or pinned meters, time style), notifications (N§2), the global hotkey,
//! the language (ROADMAP M4) and start with Windows. Its texts are the `settings.*` strings in the
//! language the settings pick (else Windows' display language, [`Wording`]); when Apply changes
//! the language, the window switches to it.
//!
//! It runs in the dialog process ([`crate::dialog`]), not in the tray process. Opening reads
//! `settings.json` and the meters earlier readings reported (`state.json`); the window edits a
//! [`Draft`] of the settings. **Apply** re-reads `settings.json`, applies only what the window
//! changed onto it ([`apply_onto`]: the tray may have paused, pinned a meter or turned the readout
//! off meanwhile), sets autostart, tests a changed hotkey, writes the file (atomic) and tells the
//! tray process, which re-reads and applies it. The API key is the exception: it is stored at once
//! when the user saves it and never kept anywhere else — not in the draft, not in a Slint
//! property, not in `settings.json`, not in the log — and the tray process is told so it reads
//! Meshy again.
//!
//! Threading: the dialog process's main thread.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::rc::Rc;

use jiff::tz::TimeZone;
use lastdrop_core::grammar::meters::localized;
use lastdrop_core::grammar::numbers::percent;
use lastdrop_core::grammar::strings::{capitalise, text};
use lastdrop_core::grammar::{Language, LocaleFormat, Msg, Strings, TimeStyle};
use lastdrop_core::model::{AccountKey, MeterKey, MeterNames, ProviderId, SourceKey, catalog};
use lastdrop_core::secret::SecretString;
use lastdrop_core::settings::{
    Hotkey, ProviderEntry, ReadoutContent, RefreshInterval, Settings, SourceSettings,
};
use lastdrop_providers::io::CredentialName;
use lastdrop_win::hotkey::HotkeyError;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::dialog::{self, Changes};
use crate::persist::{self, LoadOutcome, Paths, SavedState};
use crate::settings_accounts::{self, AccountsInfo, MeterAccounts, NotAdded, SERVICES};
use crate::wiring::{self, toggle_in};
use crate::{AppError, ui};

/// The refresh interval choices in minutes (`None` = manual).
pub const INTERVALS: [Option<u16>; 7] = [
    None,
    Some(1),
    Some(2),
    Some(5),
    Some(10),
    Some(15),
    Some(30),
];

/// The notification level choices (N§2; `None` = off).
pub const LEVELS: [Option<u8>; 9] = [
    None,
    Some(50),
    Some(60),
    Some(70),
    Some(75),
    Some(80),
    Some(85),
    Some(90),
    Some(95),
];

/// Where the window opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// At the top.
    None,
    /// At the readout's pins, highlighted (tray menu "Taskbar readout ▸ Pinned meters…").
    Pins,
    /// At the accounts (the first run found more folders, accounts-spec §6).
    Accounts,
}

/// What the window's texts depend on besides the settings: the language, Windows' display
/// languages (what "Same as Windows" follows) and the regional format (the levels' percents).
#[derive(Clone, Debug)]
pub struct Wording {
    /// The language of the texts.
    pub language: Language,
    /// Windows' preferred display languages, most preferred first.
    pub ui_languages: Vec<String>,
    /// The regional format.
    pub locale: LocaleFormat,
}

impl Wording {
    /// The wording of `settings` on this PC: the language they pick, else the first of Windows'
    /// display languages that has a table (as the tray process resolves it), and the user's
    /// regional format.
    pub fn read(settings: &Settings) -> Wording {
        let ui_languages = lastdrop_win::locale::ui_languages();
        Wording {
            language: Language::resolve(settings.language.as_deref(), &ui_languages),
            ui_languages,
            locale: lastdrop_win::locale::read(TimeZone::system()),
        }
    }

    /// The language's strings.
    pub fn strings(&self) -> &'static dyn Strings {
        self.language.strings()
    }
}

/// The labels of [`INTERVALS`]: manual, then the minutes as a duration (`5 min`).
pub fn interval_choices(strings: &dyn Strings) -> Vec<String> {
    INTERVALS
        .iter()
        .map(|minutes| match minutes {
            None => strings.template(Msg::SettingsRefreshManual).to_owned(),
            Some(m) => text(strings, Msg::DurationMin, &[("n", &m.to_string())]),
        })
        .collect()
}

/// The labels of [`LEVELS`]: off, then the percents in the regional format (`80%`, sv-SE
/// `80 %`).
pub fn level_choices(strings: &dyn Strings, locale: &LocaleFormat) -> Vec<String> {
    LEVELS
        .iter()
        .map(|level| match level {
            None => strings.template(Msg::SettingsNotifyOff).to_owned(),
            Some(l) => percent(*l, locale),
        })
        .collect()
}

/// The language choices: Windows' display language (with the language it resolves to, from
/// `ui_languages`), then every language with a table by its own name ([`Language::ALL`]).
pub fn language_choices(ui_languages: &[String], strings: &dyn Strings) -> Vec<String> {
    let windows = Language::resolve(None, ui_languages);
    let same = text(
        strings,
        Msg::SettingsLanguageWindows,
        &[("language", windows.native_name())],
    );
    std::iter::once(same)
        .chain(Language::ALL.iter().map(|l| l.native_name().to_owned()))
        .collect()
}

/// The [`language_choices`] index of a language setting: 0 follows Windows (so does a tag
/// without a table), else the language's place in [`Language::ALL`] after it.
pub fn language_index(setting: Option<&str>) -> i32 {
    setting
        .and_then(Language::from_tag)
        .and_then(|l| Language::ALL.iter().position(|a| *a == l))
        .and_then(|i| i32::try_from(i + 1).ok())
        .unwrap_or(0)
}

/// The language setting of a [`language_choices`] index: `None` follows Windows, else the
/// language's tag.
pub fn language_at(index: i32) -> Option<String> {
    let i = usize::try_from(index).ok()?.checked_sub(1)?;
    Language::ALL.get(i).map(|l| l.tag().to_owned())
}

/// The [`LEVELS`] indices of the first two of `levels` (as `NotificationSettings::levels` gives
/// them; a missing one is off), each the nearest choice.
pub fn level_indices(levels: &[u8]) -> (i32, i32) {
    let index = |level: Option<&u8>| {
        let Some(&level) = level else {
            return 0;
        };
        LEVELS
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.map(|v| (i, v.abs_diff(level))))
            .min_by_key(|(_, d)| *d)
            .map_or(0, |(i, _)| i32::try_from(i).unwrap_or(0))
    };
    (index(levels.first()), index(levels.get(1)))
}

/// The levels of two [`LEVELS`] indices: the ones that are not off, ascending, each once.
pub fn levels_at(first: i32, second: i32) -> Vec<u8> {
    let level = |i: i32| {
        usize::try_from(i)
            .ok()
            .and_then(|i| LEVELS.get(i))
            .copied()
            .flatten()
    };
    let mut levels: Vec<u8> = [level(first), level(second)]
        .into_iter()
        .flatten()
        .collect();
    levels.sort_unstable();
    levels.dedup();
    levels
}

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
            .filter_map(|(i, v)| v.map(|v| (i, v.abs_diff(m))))
            .min_by_key(|(_, d)| *d)
            .map_or(3, |(i, _)| i),
    }
}

/// The interval of an [`INTERVALS`] index.
pub fn interval_at(index: i32) -> RefreshInterval {
    let i = usize::try_from(index).unwrap_or(3).min(INTERVALS.len() - 1);
    match INTERVALS[i] {
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
/// its readings reported (Claude's per-model weekly windows), with the names their provider
/// gives them; none of the `duplicates` (accounts-spec §3: never evaluated, so never shown).
pub fn listed_meters(
    settings: &Settings,
    reported: &[(MeterKey, MeterNames)],
    duplicates: &BTreeSet<SourceKey>,
) -> Vec<(MeterKey, MeterNames)> {
    let mut out: Vec<(MeterKey, MeterNames)> = Vec::new();
    for (_, p) in settings.enabled_providers() {
        for account in &p.accounts {
            let source = SourceKey {
                provider: p.id,
                account: account.key.clone(),
            };
            if duplicates.contains(&source) {
                continue;
            }
            for def in catalog(p.id) {
                out.push((
                    MeterKey {
                        provider: p.id,
                        account: account.key.clone(),
                        meter: def.meter_id(),
                    },
                    def.names(),
                ));
            }
            for (key, names) in reported {
                if key.provider == p.id
                    && key.account == account.key
                    && !out.iter().any(|(k, _)| k == key)
                {
                    out.push((key.clone(), names.clone()));
                }
            }
        }
    }
    out
}

/// The keys of [`listed_meters`] (row index → meter).
fn listed_keys(
    settings: &Settings,
    reported: &[(MeterKey, MeterNames)],
    duplicates: &BTreeSet<SourceKey>,
) -> Vec<MeterKey> {
    listed_meters(settings, reported, duplicates)
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

/// The [`listed_meters`] with their labels in the language of `strings` ([`meter_label`]);
/// `accounts` names the accounts of services with two or more shown and leaves out the
/// duplicates ([`settings_accounts::meter_accounts`]).
pub fn meter_choices(
    settings: &Settings,
    reported: &[(MeterKey, MeterNames)],
    strings: &dyn Strings,
    accounts: &MeterAccounts,
) -> Vec<(MeterKey, String)> {
    listed_meters(settings, reported, &accounts.duplicates)
        .into_iter()
        .map(|(key, names)| {
            let account = accounts.names.get(&key.source()).map(String::as_str);
            let label = meter_label(&key, &names, strings, account);
            (key, label)
        })
        .collect()
}

/// A meter's row label: its localized `short_name` with its service (`settings.meter`,
/// capitalised): `Claude weekly`, `Veckogränsen för Claude`; with the account's name when its
/// service has several (`Claude · Work weekly`, accounts-spec §7).
pub fn meter_label(
    key: &MeterKey,
    names: &MeterNames,
    strings: &dyn Strings,
    account: Option<&str>,
) -> String {
    let names = localized(key.provider, &key.meter, names, strings);
    let service = match account {
        Some(account) => format!("{} · {account}", key.provider.display_name()),
        None => key.provider.display_name().to_owned(),
    };
    capitalise(&text(
        strings,
        Msg::SettingsMeter,
        &[("service", &service), ("short_name", &names.short_name)],
    ))
}

/// The meters the tray process's readings reported, as it remembers them in `state.json`
/// (`SavedState::seen`), plus any pinned meter it does not remember, each with the names its
/// provider gave (English; [`meter_label`] localizes them).
pub fn reported_meters(saved: &SavedState, settings: &Settings) -> Vec<(MeterKey, MeterNames)> {
    let mut out: Vec<(MeterKey, MeterNames)> = saved
        .seen
        .iter()
        .map(|m| (m.key.clone(), m.names.clone()))
        .collect();
    for key in pins(settings) {
        if !out.iter().any(|(k, _)| *k == key) {
            let names = pinned_names(&key);
            out.push((key, names));
        }
    }
    out
}

/// The names of a pinned meter no reading is remembered for: the catalogue's, a per-model weekly
/// window's (`weekly:fable` → `Fable weekly`), else its id.
fn pinned_names(key: &MeterKey) -> MeterNames {
    if let Some(def) = catalog(key.provider).iter().find(|d| d.id == key.meter.0) {
        return def.names();
    }
    match key.meter.0.strip_prefix("weekly:") {
        Some(model) => {
            let mut chars = model.chars();
            let model: String = chars
                .next()
                .map(|c| c.to_uppercase().chain(chars).collect())
                .unwrap_or_default();
            MeterNames::weekly_model(&model)
        }
        None => {
            let id = &key.meter.0;
            MeterNames::new(id, id, id)
        }
    }
}

/// The services read with an API key stored here (Meshy, OpenCode Go, the described services)
/// that the settings have, in service order: the rows of the API keys section.
pub fn key_services(settings: &Settings) -> Vec<ProviderId> {
    ProviderId::every()
        .into_iter()
        .filter(|p| p.uses_api_key() && settings.provider(*p).is_some())
        .collect()
}

/// The credential name of `provider`'s API key for its first account.
fn key_credential(settings: &Settings, provider: ProviderId) -> CredentialName {
    let account = settings.provider(provider).and_then(|p| p.accounts.first());
    let name = account
        .and_then(|a| match &a.source {
            SourceSettings::ApiKey { name } if !name.is_empty() => Some(name.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "api_key".to_owned());
    CredentialName {
        provider,
        account: account.map_or_else(AccountKey::primary, |a| a.key.clone()),
        name,
    }
}

/// The open settings window and what it edits.
struct Session {
    /// The window.
    window: ui::SettingsWindow,
    /// The draft.
    draft: RefCell<Draft>,
    /// The settings the draft started from: as read when the window opened, then as the last
    /// Apply wrote them. What the draft changed from these is what Apply writes ([`apply_onto`]).
    base: RefCell<Settings>,
    /// The meters the readings reported ([`reported_meters`]), read when the window opened.
    reported: Vec<(MeterKey, MeterNames)>,
    /// What the texts are made of: read when the window opened, the language again after each
    /// Apply.
    wording: RefCell<Wording>,
    /// Who is signed in to each listed folder and the folders found on this PC: read when the
    /// window opened, a folder added later when it is added.
    accounts: RefCell<AccountsInfo>,
    /// The API key rows: each service and whether a key is stored ([`keys_stored`]).
    keys: RefCell<Vec<(ProviderId, bool)>>,
    /// `settings.json`.
    settings_path: PathBuf,
    /// Lastdrop's folders (the themes folder, Theme Studio).
    paths: Paths,
    /// The ids of the themes the theme list shows, in its order.
    theme_ids: RefCell<Vec<String>>,
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
/// `settings.json`, `state.json` and who is signed in to the accounts' folders, fills the window
/// and brings it to the front. `focus` scrolls to the readout's pins (tray menu "Taskbar readout
/// ▸ Pinned meters…") or the accounts (first run). When the window is open already it keeps its
/// edits and only comes to the front (and to the focus).
pub fn open(paths: &Paths, focus: Focus) -> Result<(), AppError> {
    let focus_pins = focus == Focus::Pins;
    let open = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = open {
        let window = &session.window;
        if focus_pins {
            show_pins(window);
        }
        window.show()?;
        scroll_to(window, focus);
        dialog::bring_to_front(window.as_weak());
        return Ok(());
    }

    let (mut settings, outcome) = persist::load_settings(&paths.settings);
    log::info!("settings: {outcome:?}");
    let reported = reported_meters(&persist::load_state(&paths.state), &settings);
    // Taken before the pins' focus turns the readout on: that is the window's change.
    let base = settings.clone();
    if focus_pins && !matches!(settings.readout.content, ReadoutContent::Pinned(_)) {
        settings.readout.enabled = true;
    }
    let wording = Wording::read(&base);
    let accounts = AccountsInfo::read(&settings);
    let duplicates =
        settings_accounts::meter_accounts(&settings, &accounts, wording.strings()).duplicates;
    let draft = draft_with(settings, &reported, &duplicates);
    let window = wiring::create_window(false, ui::SettingsWindow::new)?;
    connect(&window);
    let keys = keys_stored(&draft.settings);
    fill(&window, &draft, &reported, &keys, &wording, &accounts);
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
            base: RefCell::new(base),
            reported,
            wording: RefCell::new(wording),
            accounts: RefCell::new(accounts),
            keys: RefCell::new(keys),
            settings_path: paths.settings.clone(),
            paths: paths.clone(),
            theme_ids: RefCell::new(Vec::new()),
        }));
    });
    with_session(|session| fill_themes(session, None));
    if let Err(e) = window.show() {
        release();
        return Err(e.into());
    }
    scroll_to(&window, focus);
    dialog::bring_to_front(window.as_weak());
    Ok(())
}

/// Fills the theme list from the themes folder and selects `select`, else the draft's theme.
fn fill_themes(session: &Session, select: Option<&str>) {
    let strings = session.wording.borrow().strings();
    let entries = crate::themes::list(&session.paths.themes, strings);
    let current = select
        .map(str::to_owned)
        .or_else(|| session.draft.borrow().settings.theme.clone());
    let current = current.unwrap_or_else(|| lastdrop_core::theme::DEFAULT_ID.to_owned());
    let index = entries.iter().position(|e| e.id == current).unwrap_or(0);
    session.window.set_themes(ModelRc::new(VecModel::from(
        entries
            .iter()
            .map(|e| SharedString::from(e.name.as_str()))
            .collect::<Vec<_>>(),
    )));
    session
        .window
        .set_theme_index(i32::try_from(index).unwrap_or(0));
    *session.theme_ids.borrow_mut() = entries.into_iter().map(|e| e.id).collect();
}

/// Theme Studio saved a theme: the list is read again. With `used`, Studio made it the theme of
/// `settings.json` itself: the window shows it and treats it as already applied, so a later
/// Apply does not change it back.
pub fn themes_changed(used: Option<&str>) {
    with_session(|session| {
        if let Some(id) = used {
            let theme = (id != lastdrop_core::theme::DEFAULT_ID).then(|| id.to_owned());
            session.base.borrow_mut().theme.clone_from(&theme);
            session.draft.borrow_mut().settings.theme = theme;
        }
        let keep = usize::try_from(session.window.get_theme_index())
            .ok()
            .and_then(|i| session.theme_ids.borrow().get(i).cloned());
        fill_themes(session, used.or(keep.as_deref()));
    });
}

/// Scrolls the shown window to `focus`.
fn scroll_to(window: &ui::SettingsWindow, focus: Focus) {
    match focus {
        Focus::Pins => window.invoke_show_pins(),
        Focus::Accounts => window.invoke_show_accounts(),
        Focus::None => {}
    }
}

/// Highlights the readout's pins: the readout on and in pinned mode (Apply saves it so).
fn show_pins(window: &ui::SettingsWindow) {
    window.set_focus_pins(true);
    window.set_readout_enabled(true);
    window.set_readout_content_index(1);
}

/// A draft of `settings` with the meters the readings reported, but not the `duplicates`'.
pub fn draft_with(
    settings: Settings,
    reported: &[(MeterKey, MeterNames)],
    duplicates: &BTreeSet<SourceKey>,
) -> Draft {
    let providers = settings
        .providers
        .iter()
        .filter_map(ProviderEntry::known)
        .map(|p| p.id)
        .collect();
    let meters = listed_keys(&settings, reported, duplicates);
    Draft {
        settings,
        providers,
        meters,
    }
}

/// Each API key service and whether its key is stored (its credential exists; the secret is not
/// read into the app).
fn keys_stored(settings: &Settings) -> Vec<(ProviderId, bool)> {
    key_services(settings)
        .into_iter()
        .map(|p| {
            let target = key_credential(settings, p).target();
            let stored = lastdrop_win::credentials::last_written(&target)
                .ok()
                .flatten()
                .is_some();
            (p, stored)
        })
        .collect()
}

/// Fills every property of the window from the draft, the meters the readings reported, which
/// API keys are stored, the wording and what the accounts section knows (no I/O;
/// `--preview-settings` uses it directly).
pub fn fill(
    window: &ui::SettingsWindow,
    draft: &Draft,
    reported: &[(MeterKey, MeterNames)],
    keys: &[(ProviderId, bool)],
    wording: &Wording,
    accounts: &AccountsInfo,
) {
    set_texts(window, draft, reported, keys, wording, accounts);
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
    window.set_interval_index(i32::try_from(interval_index(s.refresh.interval)).unwrap_or(3));
    window.set_readout_enabled(s.readout.enabled);
    window.set_updates_check(s.updates.check);
    window.set_readout_every(s.readout.every_taskbar);
    window.set_readout_content_index(i32::from(matches!(
        s.readout.content,
        ReadoutContent::Pinned(_)
    )));
    window.set_time_style_index(i32::from(s.readout.time_style == TimeStyle::Relative));
    let n = &s.notifications;
    window.set_notify_enabled(n.enabled);
    let (first, second) = level_indices(&n.levels());
    window.set_notify_first_index(first);
    window.set_notify_second_index(second);
    window.set_notify_used_up(n.used_up);
    window.set_notify_resets(n.resets);
    window.set_notify_low_balance(n.low_balance);
    window.set_start_with_windows(s.start_with_windows);
    window.set_language_index(language_index(s.language.as_deref()));
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

/// Sets every text of the window in the language of `wording`: the fixed texts, the choices of
/// the combo boxes (their selections stay) and the meter rows.
fn set_texts(
    window: &ui::SettingsWindow,
    draft: &Draft,
    reported: &[(MeterKey, MeterNames)],
    keys: &[(ProviderId, bool)],
    wording: &Wording,
    accounts: &AccountsInfo,
) {
    let strings = wording.strings();
    fill_keys(window, keys, strings);
    window.set_texts(texts(strings));
    window.set_interval_choices(string_model(interval_choices(strings)));
    window.set_level_choices(string_model(level_choices(strings, &wording.locale)));
    window.set_language_choices(string_model(language_choices(
        &wording.ui_languages,
        strings,
    )));
    fill_accounts(window, draft, reported, accounts, strings);
}

/// Fills the accounts section and the meter rows (whose labels name the accounts) from the draft.
fn fill_accounts(
    window: &ui::SettingsWindow,
    draft: &Draft,
    reported: &[(MeterKey, MeterNames)],
    accounts: &AccountsInfo,
    strings: &dyn Strings,
) {
    let blocks = settings_accounts::blocks(&draft.settings, accounts, strings);
    window.set_account_blocks(ModelRc::new(VecModel::from(blocks)));
    window.set_hide_personal_info(draft.settings.hide_personal_info);
    let names = settings_accounts::meter_accounts(&draft.settings, accounts, strings);
    fill_meters(window, draft, reported, strings, &names);
}

/// Fills the API key rows: each service's name, the box's placeholder (`Paste your Meshy API
/// key`, or that a key is stored) and whether a key is stored.
fn fill_keys(window: &ui::SettingsWindow, keys: &[(ProviderId, bool)], strings: &dyn Strings) {
    let rows: Vec<ui::ApiKeyRow> = keys
        .iter()
        .map(|(provider, stored)| {
            let placeholder = if *stored {
                strings.template(Msg::SettingsKeyReplace).to_owned()
            } else {
                text(
                    strings,
                    Msg::SettingsKeyPaste,
                    &[("service", provider.display_name())],
                )
            };
            ui::ApiKeyRow {
                title: provider.display_name().into(),
                placeholder: placeholder.into(),
                stored: *stored,
            }
        })
        .collect();
    window.set_api_keys(ModelRc::new(VecModel::from(rows)));
}

/// A Slint model of `items`.
fn string_model(items: Vec<String>) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = items.into_iter().map(SharedString::from).collect();
    ModelRc::new(VecModel::from(items))
}

/// The window's fixed texts in the language of `strings`.
pub fn texts(strings: &dyn Strings) -> ui::SettingsTexts {
    let t = |msg: Msg| SharedString::from(strings.template(msg));
    ui::SettingsTexts {
        title: t(Msg::SettingsTitle),
        heading: t(Msg::SettingsHeading),
        services: t(Msg::SettingsServices),
        services_hint: t(Msg::SettingsServicesHint),
        accounts: t(Msg::SettingsAccounts),
        accounts_hint: t(Msg::SettingsAccountsHint),
        accounts_alias: t(Msg::SettingsAccountsAlias),
        accounts_alias_none: t(Msg::SettingsAccountsAliasNone),
        accounts_remove: t(Msg::SettingsAccountsRemove),
        accounts_found: t(Msg::SettingsAccountsFound),
        accounts_add: t(Msg::SettingsAccountsAdd),
        accounts_add_folder: t(Msg::SettingsAccountsAddFolder),
        accounts_new: t(Msg::SettingsAccountsNew),
        accounts_new_name: t(Msg::SettingsAccountsNewName),
        accounts_new_create: t(Msg::SettingsAccountsNewCreate),
        privacy_hide: t(Msg::SettingsPrivacyHide),
        key: t(Msg::SettingsKey),
        key_save: t(Msg::SettingsKeySave),
        key_remove: t(Msg::SettingsKeyRemove),
        key_hint: t(Msg::SettingsKeyHint),
        refresh: t(Msg::SettingsRefresh),
        refresh_check: t(Msg::SettingsRefreshCheck),
        theme: t(Msg::SettingsTheme),
        theme_studio: t(Msg::SettingsThemeStudio),
        theme_hint: t(Msg::SettingsThemeHint),
        updates: t(Msg::SettingsUpdates),
        updates_check: t(Msg::SettingsUpdatesCheck),
        updates_now: t(Msg::SettingsUpdatesNow),
        updates_hint: t(Msg::SettingsUpdatesHint),
        readout: t(Msg::SettingsReadout),
        readout_show: t(Msg::SettingsReadoutShow),
        readout_every: t(Msg::SettingsReadoutEvery),
        readout_rows: t(Msg::SettingsReadoutRows),
        readout_auto: t(Msg::SettingsReadoutAuto),
        readout_pinned: t(Msg::SettingsReadoutPinned),
        readout_times: t(Msg::SettingsReadoutTimes),
        readout_clock: t(Msg::SettingsReadoutClock),
        readout_relative: t(Msg::SettingsReadoutRelative),
        meters_hint: t(Msg::SettingsMetersHint),
        meter_show: t(Msg::SettingsMeterShow),
        meter_pin: t(Msg::SettingsMeterPin),
        notify: t(Msg::SettingsNotify),
        notify_on: t(Msg::SettingsNotifyOn),
        notify_levels: t(Msg::SettingsNotifyLevels),
        notify_and: t(Msg::SettingsNotifyAnd),
        notify_used_up: t(Msg::SettingsNotifyUsedUp),
        notify_resets: t(Msg::SettingsNotifyResets),
        notify_low: t(Msg::SettingsNotifyLow),
        notify_hint: t(Msg::SettingsNotifyHint),
        shortcut: t(Msg::SettingsShortcut),
        shortcut_on: t(Msg::SettingsShortcutOn),
        shortcut_ctrl: t(Msg::SettingsShortcutCtrl),
        shortcut_alt: t(Msg::SettingsShortcutAlt),
        shortcut_shift: t(Msg::SettingsShortcutShift),
        shortcut_win: t(Msg::SettingsShortcutWin),
        shortcut_key: t(Msg::SettingsShortcutKey),
        language: t(Msg::SettingsLanguage),
        language_hint: t(Msg::SettingsLanguageHint),
        startup: t(Msg::SettingsStartup),
        startup_on: t(Msg::SettingsStartupOn),
        apply: t(Msg::SettingsApply),
        close: t(Msg::SettingsClose),
    }
}

/// Fills the meter rows of the draft from the reported meters, labelled in the language of
/// `strings` with the account names of `accounts`.
fn fill_meters(
    window: &ui::SettingsWindow,
    draft: &Draft,
    reported: &[(MeterKey, MeterNames)],
    strings: &dyn Strings,
    accounts: &MeterAccounts,
) {
    let pinned = pins(&draft.settings);
    let choices = meter_choices(&draft.settings, reported, strings, accounts);
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

/// Runs `f` on the draft, then refreshes the accounts section and the meter rows.
fn edit(f: impl FnOnce(&mut Draft)) {
    with_session(|session| {
        let strings = session.wording.borrow().strings();
        let accounts = session.accounts.borrow();
        let draft = {
            let mut draft = session.draft.borrow_mut();
            f(&mut draft);
            let duplicates =
                settings_accounts::meter_accounts(&draft.settings, &accounts, strings).duplicates;
            draft.meters = listed_keys(&draft.settings, &session.reported, &duplicates);
            draft.clone()
        };
        fill_accounts(
            &session.window,
            &draft,
            &session.reported,
            &accounts,
            strings,
        );
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
    window.on_account_alias_edited(|block, row, alias| {
        // Kept in the draft only: refilling the rows while typing would reset the box.
        with_session(|session| {
            let mut draft = session.draft.borrow_mut();
            let account = service_of(block)
                .and_then(|p| draft.settings.provider_mut(p))
                .and_then(|p| p.accounts.get_mut(usize::try_from(row).ok()?));
            if let Some(account) = account {
                account.alias = lastdrop_core::settings::clean_alias(&alias);
            }
        });
    });
    window.on_account_removed(|block, row| {
        let (Some(provider), Ok(row)) = (service_of(block), usize::try_from(row)) else {
            return;
        };
        edit(|draft| {
            settings_accounts::remove(&mut draft.settings, provider, row);
        });
    });
    window.on_found_added(|block, index| {
        with_session(|session| add_found(session, block, index));
    });
    window.on_add_folder(|block| with_session(|session| pick_and_add(session, block)));
    window.on_new_account(|block, name| {
        with_session(|session| new_account(session, block, &name));
    });
    window.on_privacy_toggled(|hide| {
        edit(|draft| draft.settings.hide_personal_info = hide);
    });
    window.on_set_api_key(|row, key| with_session(|session| store_key(session, row, &key)));
    window.on_clear_api_key(|row| with_session(|session| clear_key(session, row)));
    window.on_apply(|| with_session(apply));
    window.on_check_updates(|| {
        dialog::notify(Changes {
            check_updates: true,
            ..Changes::default()
        });
        with_session(|session| {
            let strings = session.wording.borrow().strings();
            session
                .window
                .set_updates_status(strings.template(Msg::SettingsUpdatesAsked).into());
        });
    });
    window.on_open_studio(|index| {
        with_session(|session| {
            let id = usize::try_from(index)
                .ok()
                .and_then(|i| session.theme_ids.borrow().get(i).cloned());
            if let Err(e) = crate::theme_studio::open(&session.paths, id.as_deref()) {
                log::error!("theme studio: {e}");
            }
        });
    });
    window.on_dismiss(close);
    window.window().on_close_requested(|| {
        close();
        slint::CloseRequestResponse::HideWindow
    });
}

/// The service of an accounts block.
fn service_of(block: i32) -> Option<ProviderId> {
    usize::try_from(block)
        .ok()
        .and_then(|b| SERVICES.get(b))
        .copied()
}

/// Shows `msg` (with `{client}` the service's client) under the buttons.
fn account_status(session: &Session, msg: Msg, provider: ProviderId, detail: Option<&str>) {
    let strings = session.wording.borrow().strings();
    let client = provider.client_name().unwrap_or(provider.display_name());
    let mut values = vec![("client", client)];
    if let Some(detail) = detail {
        values.push(("detail", detail));
    }
    session
        .window
        .set_status_text(text(strings, msg, &values).into());
}

/// Adds `folder` as an account of `provider` (`alias` for a new account), reads who is signed in
/// there, refreshes the section and says what happened. Returns whether it was added.
fn add_account(
    session: &Session,
    provider: ProviderId,
    folder: &std::path::Path,
    alias: Option<&str>,
) -> bool {
    let added = {
        let info = session.accounts.borrow();
        let mut draft = session.draft.borrow_mut();
        settings_accounts::add(&mut draft.settings, &info, provider, folder, alias)
    };
    match added {
        Ok(key) => {
            log::info!("settings window: added {:?} account {}", provider, key.0);
            {
                let draft = session.draft.borrow();
                let mut info = session.accounts.borrow_mut();
                if let Some(account) = draft
                    .settings
                    .provider(provider)
                    .and_then(|p| p.accounts.iter().find(|a| a.key == key))
                {
                    info.learn(
                        provider,
                        account,
                        &lastdrop_providers::real::RealEnv,
                        &lastdrop_providers::real::RealFs,
                    );
                }
            }
            edit(|_| {});
            account_status(session, Msg::SettingsAccountsAdded, provider, None);
            true
        }
        Err(NotAdded::AlreadyUsed) => {
            account_status(session, Msg::SettingsAccountsAlready, provider, None);
            false
        }
        Err(NotAdded::NoService) => false,
    }
}

/// **Add** on a found folder (accounts-spec §6).
fn add_found(session: &Session, block: i32, index: i32) {
    let Some(provider) = service_of(block) else {
        return;
    };
    let found = {
        let info = session.accounts.borrow();
        let draft = session.draft.borrow();
        settings_accounts::unused(&draft.settings, &info, provider)
    };
    if let Some(candidate) = usize::try_from(index).ok().and_then(|i| found.get(i)) {
        add_account(session, provider, &candidate.folder, None);
    }
}

/// **Add folder…**: the folder picker, owned by the window, opening in the profile folder.
fn pick_and_add(session: &Session, block: i32) {
    let Some(provider) = service_of(block) else {
        return;
    };
    let strings = session.wording.borrow().strings();
    let client = provider.client_name().unwrap_or(provider.display_name());
    let title = text(strings, Msg::SettingsAccountsPicker, &[("client", client)]);
    let owner = wiring::hwnd_of_window(session.window.window());
    let home = session.accounts.borrow().home.clone();
    match lastdrop_win::folder_picker::pick_folder(owner, &title, home.as_deref()) {
        Ok(Some(folder)) => {
            add_account(session, provider, &folder, None);
        }
        Ok(None) => {}
        Err(e) => log::warn!("settings window: folder picker: {e}"),
    }
}

/// **Create and sign in** (accounts-spec §7): a folder of its own in the profile folder, added
/// with the name as its alias, and the vendor's sign-in in a terminal with the folder set.
/// Lastdrop never signs in itself.
fn new_account(session: &Session, block: i32, name: &str) {
    let Some(provider) = service_of(block) else {
        return;
    };
    let Some(alias) = lastdrop_core::settings::clean_alias(name) else {
        return;
    };
    let Some(home) = session.accounts.borrow().home.clone() else {
        return;
    };
    let Some(folder) = settings_accounts::new_folder(&home, provider, &alias, &|p| p.exists())
    else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&folder) {
        log::warn!("settings window: cannot create a {provider:?} folder: {e}");
        account_status(
            session,
            Msg::SettingsAccountsNotCreated,
            provider,
            Some(&e.to_string()),
        );
        return;
    }
    if !add_account(session, provider, &folder, Some(&alias)) {
        // Created just now and still empty: not left behind.
        let _ = std::fs::remove_dir(&folder);
        return;
    }
    let command = {
        let draft = session.draft.borrow();
        let source = draft
            .settings
            .provider(provider)
            .and_then(|p| p.accounts.last())
            .and_then(|a| {
                lastdrop_providers::AccountSource::resolve(
                    provider,
                    a,
                    &lastdrop_providers::real::RealEnv,
                )
                .ok()
            });
        let providers = lastdrop_providers::providers(&draft.settings);
        source.and_then(|s| {
            providers
                .iter()
                .find(|p| p.id() == provider)
                .and_then(|p| p.sign_in_command(&s))
        })
    };
    let Some(command) = command else {
        return;
    };
    match lastdrop_win::shell::open_console(
        &command.program,
        &command.args,
        &command.env,
        command.cwd.as_deref(),
    ) {
        Ok(()) => account_status(session, Msg::SettingsAccountsCreated, provider, None),
        Err(e) => {
            log::warn!("settings window: cannot open the sign-in console: {e}");
            account_status(
                session,
                Msg::SettingsAccountsNoSignIn,
                provider,
                Some(&e.to_string()),
            );
        }
    }
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

/// The service of API key row `row`.
fn key_service(session: &Session, row: i32) -> Option<ProviderId> {
    let keys = session.keys.borrow();
    usize::try_from(row)
        .ok()
        .and_then(|r| keys.get(r))
        .map(|(p, _)| *p)
}

/// Marks `provider`'s key as stored or not and refills the key rows.
fn set_key_stored(session: &Session, provider: ProviderId, stored: bool) {
    {
        let mut keys = session.keys.borrow_mut();
        for (p, s) in keys.iter_mut() {
            if *p == provider {
                *s = stored;
            }
        }
    }
    let strings = session.wording.borrow().strings();
    fill_keys(&session.window, &session.keys.borrow(), strings);
}

/// Stores the API key of row `row` in Credential Manager at once and drops it, then tells the
/// tray process, which lifts the API-key services' sign-in stop (they have no credential
/// fingerprint Lastdrop owns, so only this can lift it). Turns the service on in the draft (Apply
/// saves that).
fn store_key(session: &Session, row: i32, key: &SharedString) {
    let Some(provider) = key_service(session, row) else {
        return;
    };
    let secret = SecretString::new(key.trim().to_owned());
    if secret.is_empty() {
        return;
    }
    let credential = key_credential(&session.draft.borrow().settings, provider);
    let stored = lastdrop_win::credentials::write(&credential.target(), &secret);
    drop(secret);
    let window = &session.window;
    let strings = session.wording.borrow().strings();
    match stored {
        Ok(()) => {
            log::info!("{provider:?} API key stored");
            dialog::notify(Changes::key_of(provider));
            edit(|draft| {
                if let Some(p) = draft.settings.provider_mut(provider) {
                    p.enabled = true;
                }
            });
            set_key_stored(session, provider, true);
            let rows = window.get_providers();
            let draft = session.draft.borrow().clone();
            for (i, id) in draft.providers.iter().enumerate() {
                if let Some(mut row) = slint::Model::row_data(&rows, i) {
                    row.enabled = draft.settings.provider(*id).is_some_and(|p| p.enabled);
                    slint::Model::set_row_data(&rows, i, row);
                }
            }
            let status = text(
                strings,
                Msg::SettingsKeySaved,
                &[("service", provider.display_name())],
            );
            window.set_status_text(status.into());
        }
        Err(e) => {
            log::warn!("cannot store the {provider:?} key: {e}");
            let status = text(
                strings,
                Msg::SettingsKeyNotSaved,
                &[("detail", &e.to_string())],
            );
            window.set_status_text(status.into());
        }
    }
}

/// Removes the stored API key of row `row` and tells the tray process, which reads the service
/// again (and so shows that the key is missing).
fn clear_key(session: &Session, row: i32) {
    let Some(provider) = key_service(session, row) else {
        return;
    };
    let credential = key_credential(&session.draft.borrow().settings, provider);
    let window = &session.window;
    let strings = session.wording.borrow().strings();
    let status = match lastdrop_win::credentials::delete(&credential.target()) {
        Ok(()) => {
            log::info!("{provider:?} API key removed");
            set_key_stored(session, provider, false);
            dialog::notify(Changes::key_of(provider));
            strings.template(Msg::SettingsKeyRemoved).to_owned()
        }
        Err(e) => {
            log::warn!("cannot remove the {provider:?} key: {e}");
            text(
                strings,
                Msg::SettingsKeyNotRemoved,
                &[("detail", &e.to_string())],
            )
        }
    };
    window.set_status_text(status.into());
}

/// Why Apply kept the shortcut saved before.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ShortcutProblem {
    /// No modifier, or not a letter, a digit or F1–F24.
    Invalid,
    /// Registering it failed.
    Refused(HotkeyError),
}

impl ShortcutProblem {
    /// The line under the shortcut, in the language of `strings`.
    fn message(&self, strings: &dyn Strings) -> String {
        match self {
            // Unsupported is what the window does not check: Shift alone with a letter or digit.
            ShortcutProblem::Invalid | ShortcutProblem::Refused(HotkeyError::Unsupported(_)) => {
                strings.template(Msg::SettingsShortcutInvalid).to_owned()
            }
            ShortcutProblem::Refused(HotkeyError::InUse) => {
                strings.template(Msg::SettingsShortcutInUse).to_owned()
            }
            ShortcutProblem::Refused(HotkeyError::Win32(detail)) => {
                text(strings, Msg::SettingsShortcutFailed, &[("detail", detail)])
            }
        }
    }
}

/// The status line after Apply saved the settings: what it could not apply, if anything.
fn saved_status(strings: &dyn Strings, shortcut_kept: bool, startup_kept: bool) -> &str {
    let msg = match (shortcut_kept, startup_kept) {
        (false, false) => Msg::SettingsSaved,
        (true, false) => Msg::SettingsSavedNoShortcut,
        (false, true) => Msg::SettingsSavedNoStartup,
        (true, true) => Msg::SettingsSavedNeither,
    };
    strings.template(msg)
}

/// The hotkey the window describes, or why it is not valid.
fn hotkey_of(window: &ui::SettingsWindow) -> Result<Option<Hotkey>, ShortcutProblem> {
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
        return Err(ShortcutProblem::Invalid);
    }
    Ok(Some(hotkey))
}

/// The settings Apply writes, a three-way merge: `settings.json` as it is now (`disk`) with what
/// the window changed (`draft` against `base`, the settings the draft started from) applied onto
/// it. The tray process may have changed the file while the window was open (pause and resume, a
/// pinned meter, readout auto or off); those changes survive unless the window changed the same
/// setting, in which case the window wins. The window never edits the pause, so the pause is
/// always the file's.
///
/// The merge works on the JSON form ([`merge_changes`]); should the merged JSON not read back
/// as settings (it always should), the draft is written with the file's pause.
pub fn apply_onto(base: &Settings, draft: Settings, disk: &Settings) -> Settings {
    let merged = (|| {
        let base = serde_json::to_value(base).ok()?;
        let changed = serde_json::to_value(&draft).ok()?;
        let mut merged = serde_json::to_value(disk).ok()?;
        merge_changes(&base, &changed, &mut merged);
        serde_json::from_value::<Settings>(merged).ok()
    })();
    merged.unwrap_or_else(|| {
        log::warn!("settings: cannot merge the window's changes; writing the window's settings");
        Settings {
            pause: disk.pause,
            ..draft
        }
    })
}

/// Applies onto `target` what `changed` changed from `base`, recursively: an unchanged value keeps
/// `target`'s, a changed one replaces it. Objects are merged key by key only when `base`,
/// `changed` and `target` have the same keys, so a struct's fields merge one by one while an
/// enum that changed its variant (or a section with a newer build's extra field) is replaced
/// whole and the result always reads back. Arrays and scalars are single values.
fn merge_changes(base: &Value, changed: &Value, target: &mut Value) {
    if base == changed {
        return;
    }
    if let (Value::Object(b), Value::Object(c), Value::Object(t)) = (base, changed, &mut *target)
        && b.len() == c.len()
        && c.len() == t.len()
        && c.keys().all(|k| b.contains_key(k) && t.contains_key(k))
    {
        for (key, value) in c {
            if let (Some(base_value), Some(target_value)) = (b.get(key), t.get_mut(key)) {
                merge_changes(base_value, value, target_value);
            }
        }
        return;
    }
    target.clone_from(changed);
}

/// Apply: what the window changed, onto `settings.json` as it is now ([`apply_onto`]);
/// autostart when "Start Lastdrop when I sign in" changed; a changed hotkey is tested by
/// registering it here and unregistering it at once (the tray process registers it for real);
/// the file is written atomically and the tray process is told. A changed language switches the
/// window's texts, and the lines under the shortcut and the buttons come in it.
fn apply(session: &Session) {
    let window = &session.window;
    let mut draft = session.draft.borrow().settings.clone();
    draft.refresh.interval = interval_at(window.get_interval_index());
    draft.readout.enabled = window.get_readout_enabled();
    draft.updates.check = window.get_updates_check();
    if let Some(id) = usize::try_from(window.get_theme_index())
        .ok()
        .and_then(|i| session.theme_ids.borrow().get(i).cloned())
    {
        draft.theme = (id != lastdrop_core::theme::DEFAULT_ID).then_some(id);
    }
    draft.readout.every_taskbar = window.get_readout_every();
    if window.get_readout_content_index() == 0 {
        draft.readout.content = ReadoutContent::Auto;
    }
    draft.readout.time_style = if window.get_time_style_index() == 1 {
        TimeStyle::Relative
    } else {
        TimeStyle::Clock
    };
    draft.start_with_windows = window.get_start_with_windows();
    // A language this build has no table for (a newer build's) shows as "Same as Windows" and
    // stays until the user picks another choice.
    let language = window.get_language_index();
    if language != language_index(draft.language.as_deref()) {
        draft.language = language_at(language);
    }
    let n = &mut draft.notifications;
    n.enabled = window.get_notify_enabled();
    n.used_up = window.get_notify_used_up();
    n.resets = window.get_notify_resets();
    n.low_balance = window.get_notify_low_balance();
    // Levels the combo boxes cannot show (a hand-edited 83) stay until the user picks others.
    let picked = (
        window.get_notify_first_index(),
        window.get_notify_second_index(),
    );
    if picked != level_indices(&n.levels()) {
        n.levels = levels_at(picked.0, picked.1);
    }

    let (disk, outcome) = persist::load_settings(&session.settings_path);
    if !matches!(outcome, LoadOutcome::Loaded) {
        log::info!("settings before Apply: {outcome:?}");
    }
    let mut settings = apply_onto(&session.base.borrow(), draft, &disk);
    let mut shortcut = None;
    match hotkey_of(window) {
        Ok(hotkey) => settings.hotkey = hotkey,
        Err(problem) => {
            settings.hotkey.clone_from(&disk.hotkey);
            shortcut = Some(problem);
        }
    }
    let mut startup_kept = false;

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
            startup_kept = true;
        }
    }
    if settings.hotkey != disk.hotkey
        && let Some(hotkey) = &settings.hotkey
    {
        match lastdrop_win::hotkey::register(hotkey, Box::new(|| {})) {
            Ok(registration) => registration.unregister(),
            Err(e) => {
                log::warn!("hotkey: {e}");
                settings.hotkey.clone_from(&disk.hotkey);
                shortcut = Some(ShortcutProblem::Refused(e));
            }
        }
    }

    let saved = persist::save_settings(&session.settings_path, &settings);
    match &saved {
        Ok(()) => {
            dialog::notify(Changes {
                settings: true,
                ..Changes::default()
            });
            session.base.borrow_mut().clone_from(&settings);
            session.draft.borrow_mut().settings = settings;
            switch_language(session);
            // The accounts' names follow the aliases typed since.
            edit(|_| {});
        }
        Err(e) => log::error!("cannot save settings: {e}"),
    }
    // In the language the window shows now: the new one when it was saved.
    let strings = session.wording.borrow().strings();
    let status = match saved {
        Ok(()) => saved_status(strings, shortcut.is_some(), startup_kept).to_owned(),
        Err(e) => text(
            strings,
            Msg::SettingsNotSaved,
            &[("detail", &e.to_string())],
        ),
    };
    let hotkey_error = shortcut.map(|p| p.message(strings)).unwrap_or_default();
    window.set_hotkey_error(hotkey_error.into());
    window.set_status_text(status.into());
}

/// Switches the window's texts to the language the draft's settings pick (else Windows'
/// display language) when it is not the one shown.
fn switch_language(session: &Session) {
    let draft = session.draft.borrow();
    let mut wording = session.wording.borrow_mut();
    let language = Language::resolve(draft.settings.language.as_deref(), &wording.ui_languages);
    if language == wording.language {
        return;
    }
    log::info!("settings window: language {}", language.tag());
    wording.language = language;
    let accounts = session.accounts.borrow();
    let keys = session.keys.borrow();
    set_texts(
        &session.window,
        &draft,
        &session.reported,
        &keys,
        &wording,
        &accounts,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lastdrop_core::grammar::{English, LocaleFormat, Swedish};
    use lastdrop_core::model::MeterId;
    use lastdrop_core::settings::{Pause, ProviderSettings};

    use crate::persist::SeenMeter;

    #[test]
    fn notification_levels_round_trip_and_snap_to_the_nearest_choice() {
        assert_eq!(level_indices(&[80, 95]), (5, 8));
        assert_eq!(levels_at(5, 8), [80, 95]);
        assert_eq!(level_indices(&[]), (0, 0));
        assert_eq!(levels_at(0, 0), Vec::<u8>::new());
        assert_eq!(level_indices(&[90]), (7, 0));
        // Picked in either order or twice: ascending, once.
        assert_eq!(levels_at(8, 5), [80, 95]);
        assert_eq!(levels_at(7, 7), [90]);
        // Off for one of them, or an index out of range.
        assert_eq!(levels_at(0, 6), [85]);
        assert_eq!(levels_at(-1, 99), Vec::<u8>::new());
        // A hand-edited 83 shows as the nearest choice.
        assert_eq!(level_indices(&[83, 99]), (6, 8));
    }

    #[test]
    fn choices_are_labelled_in_the_language_and_the_regional_format() {
        assert_eq!(
            interval_choices(&English),
            [
                "Manual", "1 min", "2 min", "5 min", "10 min", "15 min", "30 min"
            ]
        );
        assert_eq!(interval_choices(&Swedish)[..2], ["Manuellt", "1 min"]);
        assert_eq!(interval_choices(&English).len(), INTERVALS.len());

        let en = LocaleFormat::en_gb(TimeZone::UTC);
        let sv = LocaleFormat::sv_se(TimeZone::UTC);
        let levels = level_choices(&English, &en);
        assert_eq!(levels.len(), LEVELS.len());
        assert_eq!(levels[..3], ["Off", "50%", "60%"]);
        assert_eq!(level_choices(&Swedish, &sv)[..2], ["Av", "50\u{202F}%"]);
    }

    #[test]
    fn languages_round_trip_through_the_choices() {
        // "Same as Windows", then every language by its own name (one per locale file).
        let choices = language_choices(&["sv-SE".to_owned(), "en-US".to_owned()], &English);
        assert_eq!(choices[..2], ["Same as Windows (Svenska)", "English"]);
        assert_eq!(choices.len(), Language::ALL.len() + 1);
        assert!(choices.iter().any(|c| c == "Svenska"));
        assert_eq!(
            language_choices(&[], &English)[0],
            "Same as Windows (English)"
        );
        assert_eq!(
            language_choices(&["sv-SE".to_owned()], &Swedish)[0],
            "Samma som Windows (Svenska)"
        );
        let tags = Language::ALL.map(|l| Some(l.tag()));
        for setting in [None].into_iter().chain(tags) {
            assert_eq!(
                language_at(language_index(setting)).as_deref(),
                setting,
                "{setting:?}"
            );
        }
        assert_eq!(language_index(Some("sv-SE")), language_index(Some("sv")));
        // A newer build's language shows as "Same as Windows" (`qaa` is for private use, so no
        // locale file will ever be named after it).
        assert_eq!(language_index(Some("qaa")), 0);
        assert_eq!(language_at(-1), None);
        let past_the_end = i32::try_from(Language::ALL.len() + 1).expect("a few languages");
        assert_eq!(language_at(past_the_end), None);
    }

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
        let reported = [(fable.clone(), MeterNames::weekly_model("Fable"))];
        let choices = meter_choices(&settings, &reported, &English, &MeterAccounts::default());
        let labels: Vec<&str> = choices.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            labels,
            ["Claude 5-hour", "Claude weekly", "Claude Fable weekly"]
        );
        assert_eq!(choices[2].0, fable);
        assert_eq!(
            listed_keys(&settings, &reported, &BTreeSet::new()),
            choices.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>()
        );

        // In Swedish the meter comes first, with "limit" in it, and is capitalised.
        let choices = meter_choices(&settings, &reported, &Swedish, &MeterAccounts::default());
        let labels: Vec<&str> = choices.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "5-timmarsgränsen för Claude",
                "Veckogränsen för Claude",
                "Fable-veckogränsen för Claude"
            ]
        );
    }

    #[test]
    fn shortcut_problems_and_the_status_line_come_in_the_language() {
        assert_eq!(
            ShortcutProblem::Refused(HotkeyError::InUse).message(&English),
            "The shortcut is already in use."
        );
        assert_eq!(
            ShortcutProblem::Refused(HotkeyError::InUse).message(&Swedish),
            "Kortkommandot används redan."
        );
        assert_eq!(
            ShortcutProblem::Refused(HotkeyError::Win32("Access is denied.".to_owned()))
                .message(&English),
            "Couldn't register the shortcut: Access is denied."
        );
        // Shift alone with a letter is refused by the registration, not by the window.
        assert_eq!(
            ShortcutProblem::Refused(HotkeyError::Unsupported("L".to_owned())).message(&Swedish),
            ShortcutProblem::Invalid.message(&Swedish)
        );
        assert_eq!(saved_status(&English, false, false), "Saved.");
        assert_eq!(
            saved_status(&English, true, true),
            "Saved, except the shortcut and start with Windows."
        );
        assert_eq!(
            saved_status(&Swedish, false, true),
            "Sparat, utom start med Windows."
        );
    }

    #[test]
    fn api_key_names_follow_the_account_source() {
        let mut settings = Settings::default();
        assert_eq!(
            key_services(&settings),
            [ProviderId::Meshy, ProviderId::OpenCodeGo]
        );
        assert_eq!(
            key_credential(&settings, ProviderId::Meshy).target(),
            "Lastdrop:meshy:default:api_key"
        );
        assert_eq!(
            key_credential(&settings, ProviderId::OpenCodeGo).target(),
            "Lastdrop:opencode_go:default:api_key"
        );
        if let Some(p) = settings.provider_mut(ProviderId::OpenCodeGo) {
            p.accounts[0].source = SourceSettings::ApiKey {
                name: "studio".to_owned(),
            };
        }
        assert_eq!(
            key_credential(&settings, ProviderId::OpenCodeGo).target(),
            "Lastdrop:opencode_go:default:studio"
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
                unit: None,
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
        let labels: Vec<String> = reported
            .iter()
            .map(|(key, names)| meter_label(key, names, &English, None))
            .collect();
        assert_eq!(
            labels,
            ["Claude Fable weekly", "Claude Opus weekly", "Claude 5-hour"]
        );
        assert_eq!(
            reported.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>(),
            [fable, opus.clone(), five]
        );
        assert_eq!(
            meter_label(&opus, &reported[1].1, &Swedish, None),
            "Opus-veckogränsen för Claude"
        );
        assert!(reported_meters(&SavedState::default(), &Settings::default()).is_empty());
    }

    /// A window that changed the interval, turned the readout off, set a hotkey and autostart
    /// and pinned a meter.
    fn window_edits(base: &Settings) -> Settings {
        let mut draft = base.clone();
        draft.start_with_windows = true;
        draft.refresh.interval = RefreshInterval::Minutes(15);
        draft.readout.enabled = false;
        draft.hotkey = Some(Hotkey {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: "L".to_owned(),
        });
        set_pins(&mut draft, vec![claude(MeterId::new(MeterId::WEEKLY))]);
        draft
    }

    #[test]
    fn apply_writes_the_windows_changes_onto_the_file_as_it_is_now() {
        let base = Settings::default();
        let draft = window_edits(&base);
        assert_eq!(
            apply_onto(&base, draft.clone(), &base),
            draft,
            "an unchanged file gets the draft"
        );

        // Meanwhile the tray paused, and a field the window never shows changed.
        let mut disk = base.clone();
        disk.pause = Some(Pause { until: None });
        disk.theme_follows_system = false;
        assert_eq!(
            apply_onto(&base, draft.clone(), &disk),
            Settings {
                pause: disk.pause,
                theme_follows_system: false,
                ..draft
            }
        );
    }

    #[test]
    fn apply_keeps_the_pause_on_disk() {
        // Paused when the window opened, resumed from the tray meanwhile: not paused again.
        let paused = Settings {
            pause: Some(Pause { until: None }),
            ..Settings::default()
        };
        let resumed = Settings::default();
        assert_eq!(
            apply_onto(&paused, window_edits(&paused), &resumed).pause,
            None
        );
        // Paused from the tray meanwhile: stays paused.
        assert_eq!(
            apply_onto(&resumed, window_edits(&resumed), &paused).pause,
            paused.pause
        );
    }

    #[test]
    fn a_meter_pinned_from_the_tray_survives_an_interval_changed_in_the_window() {
        let base = Settings::default();
        let mut draft = base.clone();
        draft.refresh.interval = RefreshInterval::Minutes(15);
        let five = claude(MeterId::new(MeterId::FIVE_HOUR));
        let mut disk = base.clone();
        set_pins(&mut disk, vec![five.clone()]);

        let applied = apply_onto(&base, draft, &disk);
        assert_eq!(applied.refresh.interval, RefreshInterval::Minutes(15));
        assert_eq!(applied.readout.content, ReadoutContent::Pinned(vec![five]));

        // The readout turned off from the tray survives too.
        let mut disk = base.clone();
        disk.readout.enabled = false;
        let mut draft = base.clone();
        draft.readout.time_style = TimeStyle::Relative;
        let applied = apply_onto(&base, draft, &disk);
        assert!(!applied.readout.enabled);
        assert_eq!(applied.readout.time_style, TimeStyle::Relative);
    }

    #[test]
    fn the_window_wins_when_both_changed_the_same_setting() {
        let weekly = claude(MeterId::new(MeterId::WEEKLY));
        let five = claude(MeterId::new(MeterId::FIVE_HOUR));
        let mut base = Settings::default();
        set_pins(&mut base, vec![weekly.clone()]);

        // The window pinned a second meter; the tray switched the readout to auto.
        let mut draft = base.clone();
        set_pins(&mut draft, vec![weekly.clone(), five.clone()]);
        let mut disk = base.clone();
        disk.readout.content = ReadoutContent::Auto;
        assert_eq!(
            apply_onto(&base, draft, &disk).readout.content,
            ReadoutContent::Pinned(vec![weekly.clone(), five.clone()])
        );

        // The window switched to auto; the tray pinned another meter.
        let mut draft = base.clone();
        draft.readout.content = ReadoutContent::Auto;
        let mut disk = base.clone();
        set_pins(&mut disk, vec![weekly, five]);
        assert_eq!(
            apply_onto(&base, draft, &disk).readout.content,
            ReadoutContent::Auto
        );

        // Both changed the interval.
        let mut draft = base.clone();
        draft.refresh.interval = RefreshInterval::Manual;
        let mut disk = base.clone();
        disk.refresh.interval = RefreshInterval::Minutes(30);
        assert_eq!(
            apply_onto(&base, draft, &disk).refresh.interval,
            RefreshInterval::Manual
        );
    }

    #[test]
    fn objects_merge_key_by_key_only_when_their_keys_match() {
        let base = serde_json::json!({
            "a": {"x": 1, "y": 1},
            "e": {"mode": "auto"},
            "n": [1],
            "s": 1
        });
        let changed = serde_json::json!({
            "a": {"x": 2, "y": 1},
            "e": {"mode": "pinned", "meters": [1]},
            "n": [1],
            "s": 1
        });
        let mut target = serde_json::json!({
            "a": {"x": 1, "y": 3},
            "e": {"mode": "other", "k": 0},
            "n": [2],
            "s": 3
        });
        merge_changes(&base, &changed, &mut target);
        assert_eq!(
            target,
            serde_json::json!({
                "a": {"x": 2, "y": 3},
                "e": {"mode": "pinned", "meters": [1]},
                "n": [2],
                "s": 3
            })
        );
    }
}
