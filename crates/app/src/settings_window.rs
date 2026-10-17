//! The settings window (settings-spec): a navigation pane and four pages of settings cards —
//! Services (the services on or off, each expanding to what it has to manage: Claude Code's and
//! Codex's accounts, [`crate::settings_accounts`], accounts-spec §7, or an API key; privacy; the
//! folder of described services), Display (theme, times, the taskbar readout and its pins,
//! the meters), Notifications (N§2) and General (start with Windows, language, refresh interval,
//! keyboard shortcut, updates). Its texts are the `settings.*` strings in the language the
//! settings pick (else Windows' display language, [`Wording`]); when a commit changes the
//! language, the window switches to it.
//!
//! **Instant apply** (settings-spec §5): every change goes to the window's [`Edits`] at its commit
//! point and is committed — onto `settings.json` as it is then, under the settings lock
//! (settings-spec §6) — 250 ms after the last one, at once for account actions, and when the
//! window closes. After a commit every control shows the file again unless it holds an edit of
//! its own (reconcile, §5.4). The API key is the exception: it is stored at once when the user
//! saves it ([`Outside::store_key`]) and never kept anywhere else — not in the edits, not in a
//! Slint property, not in `settings.json`, not in the log — and the tray process is told so it
//! reads the service again.
//!
//! It runs in the dialog process ([`crate::dialog`]), not in the tray process. Opening reads
//! `settings.json` and the meters earlier readings reported (`state.json`) ([`Inputs::read`]).
//!
//! Threading: the dialog process's main thread.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Duration;

use jiff::tz::TimeZone;
use lastdrop_core::grammar::meters::localized;
use lastdrop_core::grammar::numbers::percent;
use lastdrop_core::grammar::strings::{capitalise, count, text};
use lastdrop_core::grammar::{Language, LocaleFormat, Msg, PluralMsg, Strings, TimeStyle};
use lastdrop_core::model::{AccountKey, MeterKey, MeterNames, ProviderId, SourceKey, catalog};
use lastdrop_core::secret::SecretString;
use lastdrop_core::settings::{ProviderEntry, Settings, SourceSettings};
use lastdrop_providers::io::CredentialName;
use lastdrop_win::hotkey::HotkeyError;
use lastdrop_win::theme::ThemeInfo;
use slint::{ComponentHandle, Model, ModelRc, SharedString, TimerMode, VecModel};

use crate::dialog::{self, Changes};
use crate::persist::{self, Paths, SavedState};
use crate::settings_accounts::{self, AccountsInfo, MeterAccounts, NotAdded};
use crate::settings_edits::{
    Commit, Edit, Edits, Field, INTERVALS, LEVELS, Outside, RealOutside, Shortcut, ShortcutProblem,
    StartupProblem, interval_index, language_index, level_indices, pins,
};
use crate::settings_sync::SettingsFile;
use crate::{AppError, ui, wiring};

/// Where the window opens (settings-spec §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The page it showed last (Services when it opens).
    None,
    /// The readout's pins, highlighted (tray menu "Taskbar readout ▸ Pinned meters…", R§10).
    Pins,
    /// The accounts (the first run found more folders, accounts-spec §6).
    Accounts,
}

/// The pages (settings-spec §2), in the navigation pane's order.
pub const PAGE_SERVICES: i32 = 0;
/// See [`PAGE_SERVICES`].
pub const PAGE_DISPLAY: i32 = 1;
/// See [`PAGE_SERVICES`].
pub const PAGE_NOTIFICATIONS: i32 = 2;
/// See [`PAGE_SERVICES`].
pub const PAGE_GENERAL: i32 = 3;

/// A burst of switches, check boxes and combo boxes commits once, this long after its last change
/// (settings-spec §5.2).
pub const COMMIT_DELAY: Duration = Duration::from_millis(250);
/// An alias box commits after this long without a keystroke (settings-spec §5.2).
pub const ALIAS_PAUSE: Duration = Duration::from_millis(800);

/// The window's preferred size and its smallest one, logical px (settings-spec §2).
pub const PREFERRED_SIZE: (f32, f32) = (880.0, 680.0);
/// See [`PREFERRED_SIZE`].
pub const MIN_SIZE: (f32, f32) = (680.0, 520.0);

/// The size the window opens at on a work area of `work` (logical px): the preferred size, made
/// smaller to fit with a 24 px margin, never below the smallest size unless the work area itself
/// is smaller.
pub fn opening_size(work: (f32, f32)) -> (f32, f32) {
    let fit = |preferred: f32, min: f32, work: f32| {
        let room = work - 48.0;
        preferred.min(room).max(min).min(work).max(1.0)
    };
    (
        fit(PREFERRED_SIZE.0, MIN_SIZE.0, work.0),
        fit(PREFERRED_SIZE.1, MIN_SIZE.1, work.1),
    )
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
/// that the settings have, in service order: the services whose expander holds an API key.
pub fn key_services(settings: &Settings) -> Vec<ProviderId> {
    ProviderId::every()
        .into_iter()
        .filter(|p| p.uses_api_key() && settings.provider(*p).is_some())
        .collect()
}

/// What `provider`'s row in the service list expands to (settings-spec §4), from its
/// capabilities: its accounts when its accounts are folders ([`settings_accounts::has_folders`]:
/// Claude, Codex), its API key when it is read with a key stored here ([`key_services`]), else
/// nothing.
pub fn service_kind(provider: ProviderId, settings: &Settings) -> ui::ServiceKind {
    if settings_accounts::has_folders(provider) {
        ui::ServiceKind::Accounts
    } else if provider.uses_api_key() && settings.provider(provider).is_some() {
        ui::ServiceKind::Key
    } else {
        ui::ServiceKind::None
    }
}

/// The line under `provider` in the service list (settings-spec §4): how it is read, with the
/// number of its accounts (`Claude Code's sign-in on this PC · 2 accounts`), or the state of its
/// API key (`key_stored`: one is stored here).
pub fn service_description(
    provider: ProviderId,
    settings: &Settings,
    key_stored: bool,
    strings: &dyn Strings,
) -> String {
    let t = |msg: Msg| strings.template(msg).to_owned();
    let key = || {
        t(if key_stored {
            Msg::SettingsServiceKeyStored
        } else {
            Msg::SettingsServiceKeyNeeded
        })
    };
    let accounts = || {
        let n = settings.provider(provider).map_or(0, |p| p.accounts.len());
        count(
            strings,
            PluralMsg::SettingsAccountsCount,
            i64::try_from(n).unwrap_or(0),
        )
    };
    match provider {
        ProviderId::Claude => format!("{} · {}", t(Msg::SettingsServiceClaude), accounts()),
        ProviderId::Codex => format!("{} · {}", t(Msg::SettingsServiceCodex), accounts()),
        ProviderId::Copilot => t(Msg::SettingsServiceCopilot),
        ProviderId::Cursor => t(Msg::SettingsServiceCursor),
        ProviderId::Grok => t(Msg::SettingsServiceGrok),
        ProviderId::Antigravity => t(Msg::SettingsServiceAntigravity),
        ProviderId::Meshy => key(),
        // Without a key here it uses the one OpenCode keeps: no key is not "needs a key".
        ProviderId::OpenCodeGo => t(if key_stored {
            Msg::SettingsServiceOpenCodeOwn
        } else {
            Msg::SettingsServiceOpenCodeShared
        }),
        ProviderId::Custom(_) => format!("{} · {}", t(Msg::SettingsServiceCustom), key()),
    }
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

/// Each API key service ([`key_services`]) and whether its key is stored (its credential exists;
/// the secret is not read into the app).
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

/// What the window shows besides its own edits, read when it opens.
pub struct Inputs {
    /// `settings.json` as read.
    pub settings: Settings,
    /// The meters the readings reported ([`reported_meters`]).
    pub reported: Vec<(MeterKey, MeterNames)>,
    /// Who is signed in to each listed folder and the folders found on this PC.
    pub accounts: AccountsInfo,
    /// Whether the key of each API key service ([`key_services`]) is stored; a service not
    /// listed has none.
    pub keys: Vec<(ProviderId, bool)>,
    /// What the texts are made of.
    pub wording: Wording,
    /// Windows' app mode, accent and contrast colours.
    pub theme: ThemeInfo,
    /// Lastdrop's folders (the themes folder, Theme Studio).
    pub paths: Paths,
}

impl Inputs {
    /// Reads them on this PC: `settings.json`, `state.json`, who is signed in to the accounts'
    /// folders, which keys are stored, the wording and Windows' theme.
    pub fn read(paths: &Paths) -> Inputs {
        let (settings, outcome) = persist::load_settings(&paths.settings);
        log::info!("settings: {outcome:?}");
        let reported = reported_meters(&persist::load_state(&paths.state), &settings);
        Inputs {
            reported,
            accounts: AccountsInfo::read(&settings),
            keys: keys_stored(&settings),
            wording: Wording::read(&settings),
            theme: lastdrop_win::theme::read(),
            paths: paths.clone(),
            settings,
        }
    }
}

/// One accounts block's rows, updated in place (settings-spec §5.4).
struct Block {
    provider: ProviderId,
    /// The account rows' sources, in model order.
    keys: RefCell<Vec<SourceKey>>,
    accounts: Rc<VecModel<ui::AccountRow>>,
    found: Rc<VecModel<ui::FoundFolder>>,
}

/// The open settings window and what it edits.
struct Session {
    /// The window.
    window: ui::SettingsWindow,
    /// The edits: base, pending edits, raw editors.
    edits: RefCell<Edits>,
    /// What a commit does outside the window.
    outside: RefCell<Box<dyn Outside>>,
    /// The meters the readings reported, read when the window opened.
    reported: Vec<(MeterKey, MeterNames)>,
    /// What the texts are made of: read when the window opened, the language again after each
    /// commit.
    wording: RefCell<Wording>,
    /// Who is signed in to each listed folder and the folders found on this PC: read when the
    /// window opened, a folder added later when it is added.
    accounts: RefCell<AccountsInfo>,
    /// The API key rows: each service, whether a key is stored, and what the last action said.
    keys: RefCell<Vec<(ProviderId, bool, String)>>,
    /// Lastdrop's folders (the themes folder, Theme Studio, the described services' folder).
    paths: Paths,
    /// The ids of the themes the theme list shows, in its order.
    theme_ids: RefCell<Vec<String>>,
    /// The services listed (row index → service).
    providers: Vec<ProviderId>,
    provider_rows: Rc<VecModel<ui::ProviderRow>>,
    /// The services whose expander is open: kept while the window is open, whatever the page.
    expanded: RefCell<BTreeSet<ProviderId>>,
    /// The meters listed (row index → meter).
    meters: RefCell<Vec<MeterKey>>,
    meter_rows: Rc<VecModel<ui::MeterChoice>>,
    key_rows: Rc<VecModel<ui::ApiKeyRow>>,
    /// Claude Code's and Codex's accounts.
    blocks: Vec<Block>,
    block_status: Rc<VecModel<SharedString>>,
    /// Alias boxes whose text is set again from the settings at the next sync (their edit
    /// reached its commit point, so they show what is saved).
    alias_reset: RefCell<BTreeSet<SourceKey>>,
    /// The 250 ms commit timer.
    commit_timer: slint::Timer,
    /// Each alias box's 800 ms pause.
    alias_timers: RefCell<BTreeMap<SourceKey, slint::Timer>>,
    /// A real window: the folder picker, the sign-in console, Theme Studio, File Explorer and the
    /// tray process are used (not in tests and previews; Credential Manager is `outside`'s).
    native: bool,
    /// The window is closing but the last commit failed (the InfoBar offers Close without
    /// saving).
    closing: Cell<bool>,
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

/// Opens the settings window, created on each open and released when it closes: reads what it
/// shows ([`Inputs::read`]), fills the window, sizes it to the monitor's work area and brings it
/// to the front, then follows `focus` (settings-spec §7). When the window is open already it keeps
/// its page, scroll positions and edits, comes to the front and only follows `focus`.
pub fn open(paths: &Paths, focus: Focus) -> Result<(), AppError> {
    let open = SESSION.with(|s| s.borrow().clone());
    if let Some(session) = open {
        session.window.show()?;
        reveal(focus);
        dialog::bring_to_front(session.window.as_weak());
        return Ok(());
    }
    let inputs = Inputs::read(paths);
    let outside = RealOutside::new(SettingsFile::new(paths.settings.clone()));
    let window = start(inputs, Box::new(outside), true)?;
    let (w, h) = opening_size(work_area());
    window.window().set_size(slint::LogicalSize::new(w, h));
    if let Err(e) = window.show() {
        release();
        return Err(e.into());
    }
    reveal(focus);
    dialog::bring_to_front(window.as_weak());
    Ok(())
}

/// The work area, logical px, of the monitor under the cursor (where the tray menu or the first
/// run opened the window).
fn work_area() -> (f32, f32) {
    use lastdrop_win::flyout_pos::{Anchor, monitor_for};
    use lastdrop_win::geom::Point;
    let mut pt = windows::Win32::Foundation::POINT::default();
    // SAFETY: `pt` is a valid out pointer for the duration of the call.
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) };
    let monitor = monitor_for(Anchor::Point(Point { x: pt.x, y: pt.y }));
    let scale = monitor.dpi.max(96) as f32 / 96.0;
    (
        monitor.work.width() as f32 / scale,
        monitor.work.height() as f32 / scale,
    )
}

/// Creates the settings window from `inputs` with `outside` for its commits, fills and wires it
/// and keeps it as the open window (not shown yet). `native`: a real window (the folder picker,
/// the sign-in console, Credential Manager, Theme Studio and the tray process are used); tests
/// and previews pass `false` and an `outside` of their own.
pub fn start(
    inputs: Inputs,
    outside: Box<dyn Outside>,
    native: bool,
) -> Result<ui::SettingsWindow, AppError> {
    release();
    let Inputs {
        settings,
        reported,
        accounts,
        keys,
        wording,
        theme,
        paths,
    } = inputs;
    let window = wiring::create_window(false, ui::SettingsWindow::new)?;
    set_colors(&window, &theme);
    let providers: Vec<ProviderId> = settings
        .providers
        .iter()
        .filter_map(ProviderEntry::known)
        .map(|p| p.id)
        .collect();
    let blocks: Vec<Block> = settings_accounts::shown_services(&settings)
        .into_iter()
        .map(|provider| Block {
            provider,
            keys: RefCell::new(Vec::new()),
            accounts: Rc::new(VecModel::default()),
            found: Rc::new(VecModel::default()),
        })
        .collect();
    let key_rows: Vec<(ProviderId, bool, String)> = key_services(&settings)
        .into_iter()
        .map(|p| {
            let stored = keys.iter().any(|(k, s)| *k == p && *s);
            (p, stored, String::new())
        })
        .collect();
    let session = Rc::new(Session {
        window: window.clone_strong(),
        edits: RefCell::new(Edits::new(settings)),
        outside: RefCell::new(outside),
        reported,
        wording: RefCell::new(wording),
        accounts: RefCell::new(accounts),
        keys: RefCell::new(key_rows),
        paths,
        theme_ids: RefCell::new(Vec::new()),
        providers,
        provider_rows: Rc::new(VecModel::default()),
        expanded: RefCell::new(BTreeSet::new()),
        meters: RefCell::new(Vec::new()),
        meter_rows: Rc::new(VecModel::default()),
        key_rows: Rc::new(VecModel::default()),
        block_status: Rc::new(VecModel::from(vec![SharedString::new(); blocks.len()])),
        blocks,
        alias_reset: RefCell::new(BTreeSet::new()),
        commit_timer: slint::Timer::default(),
        alias_timers: RefCell::new(BTreeMap::new()),
        native,
        closing: Cell::new(false),
    });
    window.set_providers(ModelRc::from(session.provider_rows.clone()));
    window.set_meters(ModelRc::from(session.meter_rows.clone()));
    window.set_api_keys(ModelRc::from(session.key_rows.clone()));
    window.set_block_status(ModelRc::from(session.block_status.clone()));
    connect(&window);
    SESSION.with(|s| *s.borrow_mut() = Some(session.clone()));
    set_texts(&session);
    show_draft(&session);
    Ok(window)
}

/// The accounts blocks for the window in the language of `strings`: each block's rows (updated
/// in place) and the line under a new account's name box. Set again when the language changes.
fn block_rows(session: &Session, strings: &dyn Strings) -> Vec<ui::AccountBlock> {
    session
        .blocks
        .iter()
        .map(|b| {
            let client = b
                .provider
                .client_name()
                .unwrap_or(b.provider.display_name());
            ui::AccountBlock {
                accounts: ModelRc::from(b.accounts.clone()),
                found: ModelRc::from(b.found.clone()),
                new_note: text(strings, Msg::SettingsAccountsNewNote, &[("client", client)]).into(),
            }
        })
        .collect()
}

/// Sets the window's colours from Windows' theme (settings-spec §3.3): in a contrast theme, the
/// contrast colours (light, dark and the accent come from Slint's Fluent palette, which follows
/// Windows' app mode and accent colour).
pub fn set_colors(window: &ui::SettingsWindow, theme: &ThemeInfo) {
    let global = window.global::<ui::SettingsTheme>();
    let color =
        |c: lastdrop_win::palette::Argb| slint::Color::from_argb_u8(c.a(), c.r(), c.g(), c.b());
    global.set_high_contrast(theme.high_contrast.is_some());
    if let Some(hc) = theme.high_contrast {
        let w = hc.window;
        let luma =
            0.2126 * f32::from(w.r()) + 0.7152 * f32::from(w.g()) + 0.0722 * f32::from(w.b());
        window.invoke_use_scheme_for_contrast(luma < 128.0);
        global.set_hc_window(color(hc.window));
        global.set_hc_window_text(color(hc.window_text));
        global.set_hc_highlight(color(hc.highlight));
        global.set_hc_highlight_text(color(hc.highlight_text));
        global.set_hc_gray_text(color(hc.gray_text));
        global.set_hc_hotlight(color(hc.hotlight));
    }
}

/// Follows a deep link in the open window (settings-spec §7): the page, then what it opens, then
/// the scroll, the keyboard focus and the highlight. A repeated request keeps every edit.
pub fn reveal(focus: Focus) {
    with_session(|session| match focus {
        Focus::None => {}
        Focus::Pins => reveal_pins(session),
        Focus::Accounts => reveal_accounts(session),
    });
}

/// Tray menu "Taskbar readout ▸ Pinned meters…": the readout on and "Meters I pin" (the choosing
/// state when no meter is pinned yet) — committed at once, the menu choice is the user's intent —
/// then the Display page at the readout, the first Pin box focused, the readout's content and the
/// meters outlined.
fn reveal_pins(session: &Session) {
    {
        let mut edits = session.edits.borrow_mut();
        if !edits.draft().readout.enabled {
            edits.set(Edit::ReadoutEnabled(true));
        }
        edits.set_readout_mode(true);
    }
    if session.edits.borrow().has_pending() {
        commit_now(session);
    } else {
        show_draft(session);
    }
    session.window.set_highlight_pins(true);
    session.window.invoke_reveal_pins();
}

/// The first run found more folders (accounts-spec §6): the Services page, the services with found
/// folders expanded, the first of them scrolled to the top and its first found folder's Add
/// focused.
fn reveal_accounts(session: &Session) {
    let mut first = None;
    for (b, block) in session.blocks.iter().enumerate() {
        if block.found.row_count() > 0 {
            session.expanded.borrow_mut().insert(block.provider);
            first.get_or_insert(b);
        }
    }
    show_draft(session);
    if let Some(b) = first {
        session
            .window
            .set_reveal_block(i32::try_from(b).unwrap_or(0));
    }
    session.window.invoke_reveal_accounts(first.is_some());
}

/// Theme Studio saved a theme: the list is read again. With `used`, Studio made it the theme of
/// `settings.json` itself: the window shows it and takes it as written, so no commit changes it
/// back.
pub fn themes_changed(used: Option<&str>) {
    with_session(|session| {
        if let Some(id) = used {
            let theme = (id != lastdrop_core::theme::DEFAULT_ID).then(|| id.to_owned());
            session
                .edits
                .borrow_mut()
                .written_elsewhere(&Edit::Theme(theme));
        }
        fill_themes(session);
        show_draft(session);
    });
}

/// Fills the theme list from the themes folder.
fn fill_themes(session: &Session) {
    let strings = session.wording.borrow().strings();
    let entries = crate::themes::list(&session.paths.themes, strings);
    session.window.set_themes(string_model(
        entries.iter().map(|e| e.name.clone()).collect(),
    ));
    *session.theme_ids.borrow_mut() = entries.into_iter().map(|e| e.id).collect();
}

/// A Slint model of `items`.
fn string_model(items: Vec<String>) -> ModelRc<SharedString> {
    let items: Vec<SharedString> = items.into_iter().map(SharedString::from).collect();
    ModelRc::new(VecModel::from(items))
}

/// Sets every fixed text of the window in the language of the session's wording, the choices of
/// the combo boxes (their selections stay) and the theme list; [`show_draft`] labels the rows.
fn set_texts(session: &Session) {
    let window = &session.window;
    let wording = session.wording.borrow().clone();
    let strings = wording.strings();
    window.set_texts(texts(strings));
    window.set_interval_choices(string_model(interval_choices(strings)));
    window.set_level_choices(string_model(level_choices(strings, &wording.locale)));
    window.set_language_choices(string_model(language_choices(
        &wording.ui_languages,
        strings,
    )));
    // The same account and found-folder models: their repeaters keep their rows.
    window.set_account_blocks(ModelRc::new(VecModel::from(block_rows(session, strings))));
    fill_themes(session);
}

/// The window's fixed texts in the language of `strings`.
pub fn texts(strings: &dyn Strings) -> ui::SettingsTexts {
    let t = |msg: Msg| SharedString::from(strings.template(msg));
    ui::SettingsTexts {
        title: t(Msg::SettingsTitle),
        page_services: t(Msg::SettingsPageServices),
        page_services_description: t(Msg::SettingsPageServicesDescription),
        page_display: t(Msg::SettingsPageDisplay),
        page_display_description: t(Msg::SettingsPageDisplayDescription),
        page_notifications: t(Msg::SettingsPageNotifications),
        page_general: t(Msg::SettingsPageGeneral),
        accounts_alias: t(Msg::SettingsAccountsAlias),
        accounts_alias_none: t(Msg::SettingsAccountsAliasNone),
        accounts_remove: t(Msg::SettingsAccountsRemove),
        accounts_found: t(Msg::SettingsAccountsFound),
        accounts_add: t(Msg::SettingsAccountsAdd),
        accounts_add_folder: t(Msg::SettingsAccountsAddFolder),
        accounts_new: t(Msg::SettingsAccountsNew),
        accounts_new_open: t(Msg::SettingsAccountsNewOpen),
        accounts_new_name: t(Msg::SettingsAccountsNewName),
        accounts_new_create: t(Msg::SettingsAccountsNewCreate),
        accounts_new_cancel: t(Msg::SettingsAccountsNewCancel),
        privacy: t(Msg::SettingsPrivacy),
        privacy_hide: t(Msg::SettingsPrivacyHide),
        privacy_hide_description: t(Msg::SettingsPrivacyHideDescription),
        custom: t(Msg::SettingsCustom),
        custom_add: t(Msg::SettingsCustomAdd),
        custom_add_description: t(Msg::SettingsCustomAddDescription),
        custom_open: t(Msg::SettingsCustomOpen),
        custom_open_label: t(Msg::SettingsCustomOpenA11y),
        key_label: t(Msg::SettingsKeyLabel),
        key_save: t(Msg::SettingsKeySave),
        key_remove: t(Msg::SettingsKeyRemove),
        refresh: t(Msg::SettingsRefresh),
        refresh_check: t(Msg::SettingsRefreshCheck),
        appearance: t(Msg::SettingsAppearance),
        theme: t(Msg::SettingsTheme),
        theme_studio: t(Msg::SettingsThemeStudio),
        theme_hint: t(Msg::SettingsThemeHint),
        updates: t(Msg::SettingsUpdates),
        updates_check: t(Msg::SettingsUpdatesCheck),
        updates_now: t(Msg::SettingsUpdatesNow),
        updates_hint: t(Msg::SettingsUpdatesHint),
        version: text(
            strings,
            Msg::SettingsVersion,
            &[("version", env!("CARGO_PKG_VERSION"))],
        )
        .into(),
        readout: t(Msg::SettingsReadout),
        readout_show: t(Msg::SettingsReadoutShow),
        readout_every: t(Msg::SettingsReadoutEvery),
        readout_rows: t(Msg::SettingsReadoutRows),
        readout_auto: t(Msg::SettingsReadoutAuto),
        readout_pinned: t(Msg::SettingsReadoutPinned),
        readout_choosing: t(Msg::SettingsReadoutChoosing),
        readout_times: t(Msg::SettingsReadoutTimes),
        readout_clock: t(Msg::SettingsReadoutClock),
        readout_relative: t(Msg::SettingsReadoutRelative),
        meters: t(Msg::SettingsMeters),
        meters_hint: t(Msg::SettingsMetersHint),
        meters_empty: t(Msg::SettingsMetersEmpty),
        meter_show: t(Msg::SettingsMeterShow),
        meter_pin: t(Msg::SettingsMeterPin),
        notify_on: t(Msg::SettingsNotifyOn),
        notify_levels: t(Msg::SettingsNotifyLevels),
        notify_and: t(Msg::SettingsNotifyAnd),
        notify_also: t(Msg::SettingsNotifyAlso),
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
        startup_on: t(Msg::SettingsStartupOn),
        retry: t(Msg::SettingsRetry),
        discard: t(Msg::SettingsDiscard),
        not_saved_title: t(Msg::SettingsNotSavedTitle),
    }
}

/// Shows the draft in every control (settings-spec §5.4): after each edit and after each commit
/// (then the draft is the file as written, with what the tray changed meanwhile). An editor
/// holding an edit of its own — an alias box being typed in, the shortcut being composed — is
/// left alone; keyboard focus is not the criterion.
fn show_draft(session: &Session) {
    let window = &session.window;
    let (draft, shortcut, dirty, problem, mode, choosing) = {
        let edits = session.edits.borrow();
        (
            edits.draft(),
            edits.shortcut(),
            edits.shortcut_dirty(),
            edits.shortcut_problem().cloned(),
            edits.readout_mode_index(),
            edits.choosing_pins(),
        )
    };
    let strings = session.wording.borrow().strings();
    let index = |i: usize| i32::try_from(i).unwrap_or(0);
    window.set_interval_index(index(interval_index(draft.refresh.interval)));
    window.set_readout_enabled(draft.readout.enabled);
    window.set_readout_every(draft.readout.every_taskbar);
    window.set_readout_content_index(mode);
    window.set_choosing_pins(choosing);
    window.set_time_style_index(i32::from(draft.readout.time_style == TimeStyle::Relative));
    let n = &draft.notifications;
    window.set_notify_enabled(n.enabled);
    let (first, second) = level_indices(&n.levels());
    window.set_notify_first_index(first);
    window.set_notify_second_index(second);
    window.set_notify_used_up(n.used_up);
    window.set_notify_resets(n.resets);
    window.set_notify_low_balance(n.low_balance);
    window.set_start_with_windows(draft.start_with_windows);
    window.set_language_index(language_index(draft.language.as_deref()));
    window.set_updates_check(draft.updates.check);
    window.set_hide_personal_info(draft.hide_personal_info);
    let theme = draft
        .theme
        .clone()
        .unwrap_or_else(|| lastdrop_core::theme::DEFAULT_ID.to_owned());
    let theme_index = session
        .theme_ids
        .borrow()
        .iter()
        .position(|id| *id == theme)
        .unwrap_or(0);
    window.set_theme_index(index(theme_index));
    if !dirty {
        window.set_hotkey_enabled(shortcut.enabled);
        window.set_hotkey_ctrl(shortcut.ctrl);
        window.set_hotkey_alt(shortcut.alt);
        window.set_hotkey_shift(shortcut.shift);
        window.set_hotkey_win(shortcut.win);
        window.set_hotkey_key(shortcut.key.as_str().into());
    }
    window.set_hotkey_error(
        problem
            .as_ref()
            .map(|p| shortcut_message(p, strings))
            .unwrap_or_default()
            .into(),
    );
    window.set_hotkey_retry(matches!(problem, Some(ShortcutProblem::Refused(_))));

    sync_meters(session, &draft, strings);
    sync_accounts(session, &draft, strings);
    sync_keys(session, strings);
    sync_services(session, &draft, strings);
}

/// The service rows from the draft (settings-spec §4), in place: each service's glyph, name,
/// line, switch, what its expander holds ([`service_kind`]) and whether it is open.
fn sync_services(session: &Session, draft: &Settings, strings: &dyn Strings) {
    let keys = session.keys.borrow();
    let expanded = session.expanded.borrow();
    let index = |i: Option<usize>| i.and_then(|i| i32::try_from(i).ok()).unwrap_or(-1);
    for (row, id) in session.providers.iter().enumerate() {
        let block = session.blocks.iter().position(|b| b.provider == *id);
        let key_row = keys.iter().position(|(p, _, _)| p == id);
        let stored = key_row.is_some_and(|k| keys[k].1);
        let kind = match service_kind(*id, draft) {
            ui::ServiceKind::Accounts if block.is_some() => ui::ServiceKind::Accounts,
            ui::ServiceKind::Key if key_row.is_some() => ui::ServiceKind::Key,
            _ => ui::ServiceKind::None,
        };
        let name = id.display_name();
        let toggle_label = match kind {
            ui::ServiceKind::Accounts => text(
                strings,
                Msg::SettingsServiceAccountsA11y,
                &[("service", name)],
            ),
            ui::ServiceKind::Key => text(strings, Msg::SettingsKeyA11y, &[("service", name)]),
            ui::ServiceKind::None => String::new(),
        };
        let data = ui::ProviderRow {
            id: id.as_str().into(),
            name: name.into(),
            enabled: draft.provider(*id).is_some_and(|p| p.enabled),
            glyph: wiring::glyph_data(&id.glyph()),
            description: service_description(*id, draft, stored, strings).into(),
            expanded: kind != ui::ServiceKind::None && expanded.contains(id),
            kind,
            block: index(block),
            key_row: index(key_row),
            toggle_label: toggle_label.into(),
        };
        update_row(&session.provider_rows, row, data);
    }
}

/// Sets row `row` of `model` to `data` (appended when the model is that short), unless it is
/// that already.
fn update_row<T: Clone + PartialEq + 'static>(model: &VecModel<T>, row: usize, data: T) {
    match model.row_data(row) {
        Some(current) if current == data => {}
        Some(_) => model.set_row_data(row, data),
        None => model.push(data),
    }
}

/// The meter rows from the draft: replaced when the meters listed change (a service turned on or
/// off), else updated in place.
fn sync_meters(session: &Session, draft: &Settings, strings: &dyn Strings) {
    let names = {
        let accounts = session.accounts.borrow();
        settings_accounts::meter_accounts(draft, &accounts, strings)
    };
    let pinned = pins(draft);
    let rows: Vec<(MeterKey, ui::MeterChoice)> =
        meter_choices(draft, &session.reported, strings, &names)
            .into_iter()
            .map(|(key, label)| {
                let shown = !draft
                    .provider(key.provider)
                    .is_some_and(|p| p.hidden_meters.contains(&key.meter));
                let row = ui::MeterChoice {
                    show_label: text(strings, Msg::SettingsMeterShowA11y, &[("meter", &label)])
                        .into(),
                    pin_label: text(strings, Msg::SettingsMeterPinA11y, &[("meter", &label)])
                        .into(),
                    label: label.into(),
                    shown,
                    pinned: pinned.contains(&key),
                };
                (key, row)
            })
            .collect();
    let keys: Vec<MeterKey> = rows.iter().map(|(k, _)| k.clone()).collect();
    let focus_row = rows.iter().position(|(_, r)| r.shown).unwrap_or(0);
    if *session.meters.borrow() == keys {
        for (i, (_, row)) in rows.into_iter().enumerate() {
            update_row(&session.meter_rows, i, row);
        }
    } else {
        session
            .meter_rows
            .set_vec(rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>());
        *session.meters.borrow_mut() = keys;
    }
    session
        .window
        .set_pin_focus_row(i32::try_from(focus_row).unwrap_or(0));
}

/// The accounts blocks from the draft, in place by source (settings-spec §5.4): rows of removed
/// accounts go, rows of added ones come, the others change only where they differ. A row whose
/// alias box holds an edit is left alone until that edit reaches its commit point.
fn sync_accounts(session: &Session, draft: &Settings, strings: &dyn Strings) {
    let info = session.accounts.borrow();
    let edits = session.edits.borrow();
    let mut reset = session.alias_reset.borrow_mut();
    for block in &session.blocks {
        let rows = settings_accounts::account_rows(draft, &info, block.provider, strings);
        let mut keys = block.keys.borrow_mut();
        // Removed accounts, from the end so the indices stay right.
        for i in (0..keys.len()).rev() {
            if !rows.iter().any(|(k, _)| *k == keys[i]) {
                keys.remove(i);
                block.accounts.remove(i);
            }
        }
        // Added accounts, at their places.
        for (i, (key, row)) in rows.iter().enumerate() {
            if !keys.contains(key) {
                let at = i.min(keys.len());
                keys.insert(at, key.clone());
                block.accounts.insert(at, row.clone());
            }
        }
        for (i, (key, mut row)) in rows.into_iter().enumerate() {
            if edits.alias_dirty(&key) {
                continue;
            }
            let Some(current) = block.accounts.row_data(i) else {
                continue;
            };
            row.revision = current.revision;
            if reset.remove(&key) || row.alias != current.alias {
                row.revision += 1;
            }
            if row != current {
                block.accounts.set_row_data(i, row);
            }
        }
        let found = settings_accounts::found_rows(draft, &info, block.provider, strings);
        let current: Vec<ui::FoundFolder> = block.found.iter().collect();
        if current != found {
            block.found.set_vec(found);
        }
    }
}

/// The API key rows: the box's placeholder (`Paste your Meshy API key`, or that a key is stored),
/// whether a key is stored, what the last Save or Remove said, the note under it and the
/// accessible names naming the service (`Save the Meshy API key`).
fn sync_keys(session: &Session, strings: &dyn Strings) {
    let keys = session.keys.borrow();
    for (i, (provider, stored, status)) in keys.iter().enumerate() {
        let service = [("service", provider.display_name())];
        let placeholder = if *stored {
            strings.template(Msg::SettingsKeyReplace).to_owned()
        } else {
            text(strings, Msg::SettingsKeyPaste, &service)
        };
        let mut note = strings.template(Msg::SettingsKeyHint).to_owned();
        if *provider == ProviderId::OpenCodeGo {
            note.push(' ');
            note.push_str(strings.template(Msg::SettingsKeyOpenCode));
        }
        let row = ui::ApiKeyRow {
            placeholder: placeholder.into(),
            stored: *stored,
            status: status.as_str().into(),
            note: note.into(),
            box_label: text(strings, Msg::SettingsKeyA11y, &service).into(),
            save_label: text(strings, Msg::SettingsKeySaveA11y, &service).into(),
            remove_label: text(strings, Msg::SettingsKeyRemoveA11y, &service).into(),
        };
        update_row(&session.key_rows, i, row);
    }
}

/// An edit was made: the window shows the draft and a commit follows [`COMMIT_DELAY`] after the
/// last edit of a burst.
fn edited(session: &Session) {
    show_draft(session);
    session
        .commit_timer
        .start(TimerMode::SingleShot, COMMIT_DELAY, || {
            with_session(|s| {
                commit_now(s);
            });
        });
}

/// Commits the pending edits now ([`Edits::commit`]) and shows what came of it: the InfoBar when
/// the file could not be written, the shortcut's and start with Windows' problems in their cards,
/// the language when it changed, and the draft in every control.
fn commit_now(session: &Session) -> Commit {
    session.commit_timer.stop();
    let commit = {
        let mut edits = session.edits.borrow_mut();
        let mut outside = session.outside.borrow_mut();
        edits.commit(outside.as_mut())
    };
    let window = &session.window;
    if commit.saved() {
        switch_language(session);
    }
    let strings = session.wording.borrow().strings();
    match &commit.error {
        Some(detail) => {
            let message = text(strings, Msg::SettingsNotSaved, &[("detail", detail)]);
            window.set_info_text(message.into());
        }
        None => {
            window.set_info_text(SharedString::new());
            window.set_info_closing(false);
            session.closing.set(false);
        }
    }
    match &commit.startup {
        Some(StartupProblem::NotChanged(detail)) => window.set_startup_error(
            text(
                strings,
                Msg::SettingsStartupNotChanged,
                &[("detail", detail)],
            )
            .into(),
        ),
        Some(StartupProblem::Stuck { on }) => {
            let msg = if *on {
                Msg::SettingsStartupStuckOn
            } else {
                Msg::SettingsStartupStuckOff
            };
            window.set_startup_error(strings.template(msg).into());
        }
        None => {
            if commit.written.contains(&Field::StartWithWindows) {
                window.set_startup_error(SharedString::new());
            }
        }
    }
    show_draft(session);
    commit
}

/// Switches the window's texts to the language the draft picks (else Windows' display
/// language) when it is not the one shown.
fn switch_language(session: &Session) {
    let setting = session.edits.borrow().draft().language;
    {
        let mut wording = session.wording.borrow_mut();
        let language = Language::resolve(setting.as_deref(), &wording.ui_languages);
        if language == wording.language {
            return;
        }
        log::info!("settings window: language {}", language.tag());
        wording.language = language;
    }
    set_texts(session);
}

/// The line under the shortcut, in the language of `strings`.
fn shortcut_message(problem: &ShortcutProblem, strings: &dyn Strings) -> String {
    match problem {
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

/// The source of account row `row` of block `block`.
fn account_key(session: &Session, block: i32, row: i32) -> Option<SourceKey> {
    let block = session.blocks.get(usize::try_from(block).ok()?)?;
    block.keys.borrow().get(usize::try_from(row).ok()?).cloned()
}

/// An alias box's commit point (Enter, focus loss, its pause, its block closing, the page
/// changing): its edit becomes pending (settings-spec §5.2) and the box shows what is saved.
fn alias_point(session: &Session, key: &SourceKey) {
    if let Some(timer) = session.alias_timers.borrow_mut().remove(key) {
        timer.stop();
    }
    if !session.edits.borrow().alias_dirty(key) {
        return;
    }
    let changed = session.edits.borrow_mut().alias_done(key);
    session.alias_reset.borrow_mut().insert(key.clone());
    if changed {
        edited(session);
    } else {
        show_draft(session);
    }
}

/// The commit point of every alias box holding an edit (of `provider`'s, or of all).
fn alias_points(session: &Session, provider: Option<ProviderId>) {
    let dirty = session.edits.borrow().dirty_aliases();
    for key in dirty {
        if provider.is_none_or(|p| p == key.provider) {
            alias_point(session, &key);
        }
    }
}

/// Wires the window's callbacks.
fn connect(window: &ui::SettingsWindow) {
    window.on_page_changed(|page| {
        with_session(|session| {
            alias_points(session, None);
            if page != PAGE_DISPLAY {
                session.window.set_highlight_pins(false);
            }
        });
    });
    window.on_provider_toggled(|row, on| {
        with_session(|session| {
            let id = usize::try_from(row)
                .ok()
                .and_then(|i| session.providers.get(i).copied());
            if let Some(id) = id {
                session
                    .edits
                    .borrow_mut()
                    .set(Edit::ProviderEnabled(id, on));
                edited(session);
            }
        });
    });
    window.on_service_toggled(|row, open| {
        with_session(|session| {
            let Some(id) = usize::try_from(row)
                .ok()
                .and_then(|i| session.providers.get(i).copied())
            else {
                return;
            };
            if !open {
                // Closing the accounts is their alias boxes' commit point; a new account's name
                // box closes with them.
                alias_points(session, Some(id));
                let block = session.blocks.iter().position(|b| b.provider == id);
                let shown = usize::try_from(session.window.get_new_account_block()).ok();
                if block.is_some() && block == shown {
                    session.window.set_new_account_block(-1);
                }
                session.expanded.borrow_mut().remove(&id);
            } else {
                session.expanded.borrow_mut().insert(id);
            }
            show_draft(session);
        });
    });
    window.on_account_alias_edited(|block, row, alias| {
        with_session(|session| {
            let Some(key) = account_key(session, block, row) else {
                return;
            };
            session
                .edits
                .borrow_mut()
                .alias_typed(key.clone(), alias.as_str());
            let timer = slint::Timer::default();
            let at = key.clone();
            timer.start(TimerMode::SingleShot, ALIAS_PAUSE, move || {
                let at = at.clone();
                with_session(move |s| alias_point(s, &at));
            });
            session.alias_timers.borrow_mut().insert(key, timer);
        });
    });
    window.on_account_alias_done(|block, row| {
        with_session(|session| {
            if let Some(key) = account_key(session, block, row) {
                alias_point(session, &key);
            }
        });
    });
    window.on_account_removed(|block, row| {
        with_session(|session| {
            let Some(key) = account_key(session, block, row) else {
                return;
            };
            // The focus goes to the next account, else to Add folder… (settings-spec §8).
            let next = account_key(session, block, row.saturating_add(1));
            if session.edits.borrow_mut().remove_account(&key) {
                set_block_status(session, block, String::new());
                commit_now(session);
                let target = next.map_or_else(
                    || "add-folder".to_owned(),
                    |k| format!("account:{}", k.account.0),
                );
                focus_in_block(session, block, &target);
            }
        });
    });
    window.on_found_added(|block, index| {
        with_session(|session| add_found(session, block, index));
    });
    window.on_add_folder(|block| with_session(|session| pick_and_add(session, block)));
    window.on_new_account(|block, name| {
        with_session(|session| new_account(session, block, &name));
    });
    window.on_privacy_toggled(|hide| edit_now(Edit::HidePersonalInfo(hide)));
    window.on_set_api_key(|row, key| with_session(|session| store_key(session, row, &key)));
    window.on_clear_api_key(|row| with_session(|session| clear_key(session, row)));
    window.on_open_providers_folder(|| with_session(open_providers_folder));
    window.on_theme_selected(|index| {
        with_session(|session| {
            let id = usize::try_from(index)
                .ok()
                .and_then(|i| session.theme_ids.borrow().get(i).cloned());
            if let Some(id) = id {
                let theme = (id != lastdrop_core::theme::DEFAULT_ID).then_some(id);
                session.edits.borrow_mut().set(Edit::Theme(theme));
                edited(session);
            }
        });
    });
    window.on_open_studio(|index| {
        with_session(|session| {
            if !session.native {
                return;
            }
            let id = usize::try_from(index)
                .ok()
                .and_then(|i| session.theme_ids.borrow().get(i).cloned());
            if let Err(e) = crate::theme_studio::open(&session.paths, id.as_deref()) {
                log::error!("theme studio: {e}");
            }
        });
    });
    window.on_time_style_selected(|index| {
        let style = if index == 1 {
            TimeStyle::Relative
        } else {
            TimeStyle::Clock
        };
        edit_now(Edit::TimeStyle(style));
    });
    window.on_readout_enabled_toggled(|on| edit_now(Edit::ReadoutEnabled(on)));
    window.on_readout_every_toggled(|on| edit_now(Edit::ReadoutEvery(on)));
    window.on_readout_mode_selected(|index| {
        with_session(|session| {
            session.edits.borrow_mut().set_readout_mode(index == 1);
            edited(session);
        });
    });
    window.on_meter_shown_toggled(|row, shown| {
        with_session(|session| {
            if let Some(key) = meter_at(session, row) {
                session.edits.borrow_mut().set_meter_shown(&key, shown);
                edited(session);
            }
        });
    });
    window.on_meter_pinned_toggled(|row, on| {
        with_session(|session| {
            if let Some(key) = meter_at(session, row) {
                session.edits.borrow_mut().set_pinned(key, on);
                edited(session);
            }
        });
    });
    window.on_notify_enabled_toggled(|on| edit_now(Edit::NotifyEnabled(on)));
    window.on_levels_selected(|first, second| {
        with_session(|session| {
            session.edits.borrow_mut().set_levels(first, second);
            edited(session);
        });
    });
    window.on_notify_used_up_toggled(|on| edit_now(Edit::NotifyUsedUp(on)));
    window.on_notify_resets_toggled(|on| edit_now(Edit::NotifyResets(on)));
    window.on_notify_low_toggled(|on| edit_now(Edit::NotifyLowBalance(on)));
    window.on_startup_toggled(|on| edit_now(Edit::StartWithWindows(on)));
    window.on_language_selected(|index| {
        with_session(|session| {
            session.edits.borrow_mut().set_language_index(index);
            edited(session);
        });
    });
    window.on_interval_selected(|index| {
        with_session(|session| {
            session.edits.borrow_mut().set_interval_index(index);
            edited(session);
        });
    });
    window.on_hotkey_edited(|| {
        with_session(|session| {
            let w = &session.window;
            let shown = Shortcut {
                enabled: w.get_hotkey_enabled(),
                ctrl: w.get_hotkey_ctrl(),
                alt: w.get_hotkey_alt(),
                shift: w.get_hotkey_shift(),
                win: w.get_hotkey_win(),
                key: w.get_hotkey_key().to_string(),
            };
            session.edits.borrow_mut().shortcut_edited(shown);
            edited(session);
        });
    });
    window.on_hotkey_retry_clicked(|| {
        with_session(|session| {
            session.edits.borrow_mut().shortcut_retry();
            commit_now(session);
        });
    });
    window.on_updates_toggled(|on| edit_now(Edit::UpdatesCheck(on)));
    window.on_check_updates(|| {
        with_session(|session| {
            if session.native {
                dialog::notify(Changes {
                    check_updates: true,
                    ..Changes::default()
                });
            }
            let strings = session.wording.borrow().strings();
            session
                .window
                .set_updates_status(strings.template(Msg::SettingsUpdatesAsked).into());
        });
    });
    window.on_info_retry(|| {
        let mut closes = false;
        with_session(|session| {
            // The InfoBar of a failed close: closing once the commit works, with what the
            // editors hold since (an alias typed meanwhile).
            closes = if session.closing.get() {
                flush_for_close(session)
            } else {
                commit_now(session);
                false
            };
        });
        if closes {
            close_now();
        }
    });
    window.on_info_discard(|| {
        log::info!("settings window: closed without saving");
        close_now();
    });
    window.on_dismiss(|| {
        let mut closes = false;
        with_session(|session| closes = flush_for_close(session));
        if closes {
            close_now();
        }
    });
    window.window().on_close_requested(|| {
        let mut closes = true;
        with_session(|session| closes = flush_for_close(session));
        if closes {
            close_now();
            slint::CloseRequestResponse::HideWindow
        } else {
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
}

/// Makes `edit` pending in the open window and shows it ([`edited`]).
fn edit_now(edit: Edit) {
    with_session(|session| {
        session.edits.borrow_mut().set(edit);
        edited(session);
    });
}

/// The meter of row `row`.
fn meter_at(session: &Session, row: i32) -> Option<MeterKey> {
    usize::try_from(row)
        .ok()
        .and_then(|i| session.meters.borrow().get(i).cloned())
}

/// Shows `status` under the accounts of block `block`.
fn set_block_status(session: &Session, block: i32, status: String) {
    if let Some(b) = usize::try_from(block)
        .ok()
        .filter(|b| *b < session.blocks.len())
    {
        session.block_status.set_row_data(b, status.into());
    }
}

/// Shows `msg` (with `{client}` the service's client) under the accounts of block `block`.
fn account_status(session: &Session, block: i32, msg: Msg, detail: Option<&str>) {
    let Some(provider) = block_service(session, block) else {
        return;
    };
    let strings = session.wording.borrow().strings();
    let client = provider.client_name().unwrap_or(provider.display_name());
    let mut values = vec![("client", client)];
    if let Some(detail) = detail {
        values.push(("detail", detail));
    }
    set_block_status(session, block, text(strings, msg, &values));
}

/// The service of block `block`.
fn block_service(session: &Session, block: i32) -> Option<ProviderId> {
    usize::try_from(block)
        .ok()
        .and_then(|b| session.blocks.get(b))
        .map(|b| b.provider)
}

/// Adds `folder` as an account of block `block`'s service (`alias` for a new account) and
/// commits at once; says "Account added" only when the commit wrote it. Returns whether it was
/// added and saved.
fn add_account(
    session: &Session,
    block: i32,
    folder: &std::path::Path,
    alias: Option<&str>,
) -> bool {
    let Some(provider) = block_service(session, block) else {
        return false;
    };
    let added = {
        let info = session.accounts.borrow();
        let mut draft = session.edits.borrow().draft();
        settings_accounts::add(&mut draft, &info, provider, folder, alias).map(|key| {
            let account = draft
                .provider(provider)
                .and_then(|p| p.accounts.iter().find(|a| a.key == key))
                .cloned();
            (key, account)
        })
    };
    match added {
        Ok((key, Some(account))) => {
            log::info!("settings window: added {provider:?} account {}", key.0);
            if session.native {
                session.accounts.borrow_mut().learn(
                    provider,
                    &account,
                    &lastdrop_providers::real::RealEnv,
                    &lastdrop_providers::real::RealFs,
                );
            }
            let folder_id = settings_accounts::folder_id(folder);
            session.edits.borrow_mut().set(Edit::AccountAdded(
                provider,
                Box::new(account),
                folder_id,
            ));
            let commit = commit_now(session);
            if commit.saved() {
                account_status(session, block, Msg::SettingsAccountsAdded, None);
                true
            } else {
                set_block_status(session, block, String::new());
                false
            }
        }
        Ok((_, None)) | Err(NotAdded::NoService) => false,
        Err(NotAdded::AlreadyUsed) => {
            account_status(session, block, Msg::SettingsAccountsAlready, None);
            false
        }
    }
}

/// **Add** on a found folder (accounts-spec §6). Its row goes; the focus goes to the next found
/// folder's Add, else to Add folder… (settings-spec §8).
fn add_found(session: &Session, block: i32, index: i32) {
    let Some(provider) = block_service(session, block) else {
        return;
    };
    let found = {
        let info = session.accounts.borrow();
        settings_accounts::unused(&session.edits.borrow().draft(), &info, provider)
    };
    let Some(i) = usize::try_from(index).ok().filter(|i| *i < found.len()) else {
        return;
    };
    if add_account(session, block, &found[i].folder, None) {
        let target = found.get(i + 1).map_or_else(
            || "add-folder".to_owned(),
            |c| format!("found:{}", c.folder.display()),
        );
        focus_in_block(session, block, &target);
    }
}

/// Moves the keyboard focus within the accounts of block `block` to `target` (the window's
/// `focus-target`: `account:<key>`, `found:<folder>` or `add-folder`), after the row holding it
/// went.
fn focus_in_block(session: &Session, block: i32, target: &str) {
    let window = &session.window;
    window.set_focus_block(block);
    window.set_focus_target(target.into());
    window.set_focus_request(window.get_focus_request().wrapping_add(1));
}

/// **Add folder…**: the folder picker, owned by the window, opening in the profile folder.
fn pick_and_add(session: &Session, block: i32) {
    let Some(provider) = block_service(session, block) else {
        return;
    };
    if !session.native {
        return;
    }
    let strings = session.wording.borrow().strings();
    let client = provider.client_name().unwrap_or(provider.display_name());
    let title = text(strings, Msg::SettingsAccountsPicker, &[("client", client)]);
    let owner = wiring::hwnd_of_window(session.window.window());
    let home = session.accounts.borrow().home.clone();
    match lastdrop_win::folder_picker::pick_folder(owner, &title, home.as_deref()) {
        Ok(Some(folder)) => {
            add_account(session, block, &folder, None);
        }
        Ok(None) => {}
        Err(e) => log::warn!("settings window: folder picker: {e}"),
    }
}

/// **Create and sign in** (accounts-spec §7): a folder of its own in the profile folder, added
/// with the name as its alias, and the vendor's sign-in in a terminal with the folder set.
/// Lastdrop never signs in itself.
fn new_account(session: &Session, block: i32, name: &str) {
    let Some(provider) = block_service(session, block) else {
        return;
    };
    let Some(alias) = lastdrop_core::settings::clean_alias(name) else {
        return;
    };
    let Some(home) = session.accounts.borrow().home.clone() else {
        return;
    };
    if !session.native {
        return;
    }
    let Some(folder) = settings_accounts::new_folder(&home, provider, &alias, &|p| p.exists())
    else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&folder) {
        log::warn!("settings window: cannot create a {provider:?} folder: {e}");
        account_status(
            session,
            block,
            Msg::SettingsAccountsNotCreated,
            Some(&e.to_string()),
        );
        return;
    }
    if !add_account(session, block, &folder, Some(&alias)) {
        // Created just now and still empty: not left behind, and neither is its account when
        // its write failed (it would be saved later without its folder).
        let folder_id = settings_accounts::folder_id(&folder);
        if session
            .edits
            .borrow_mut()
            .discard(&Field::AccountAdded(provider, folder_id))
        {
            show_draft(session);
        }
        let _ = std::fs::remove_dir(&folder);
        return;
    }
    let command = {
        let draft = session.edits.borrow().draft();
        let folder_id = settings_accounts::folder_id(&folder);
        let source = draft
            .provider(provider)
            .and_then(|p| {
                p.accounts.iter().find(|a| {
                    a.source
                        .folder()
                        .is_some_and(|f| settings_accounts::folder_id(f) == folder_id)
                })
            })
            .and_then(|a| {
                lastdrop_providers::AccountSource::resolve(
                    provider,
                    a,
                    &lastdrop_providers::real::RealEnv,
                )
                .ok()
            });
        let providers = lastdrop_providers::providers(&draft);
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
        Ok(()) => account_status(session, block, Msg::SettingsAccountsCreated, None),
        Err(e) => {
            log::warn!("settings window: cannot open the sign-in console: {e}");
            account_status(
                session,
                block,
                Msg::SettingsAccountsNoSignIn,
                Some(&e.to_string()),
            );
        }
    }
}

/// The service of API key row `row`.
fn key_service(session: &Session, row: i32) -> Option<ProviderId> {
    let keys = session.keys.borrow();
    usize::try_from(row)
        .ok()
        .and_then(|r| keys.get(r))
        .map(|(p, _, _)| *p)
}

/// Marks `provider`'s key as stored or not, with `status` in its row.
fn set_key_row(session: &Session, provider: ProviderId, stored: bool, status: &str) {
    for (p, s, line) in session.keys.borrow_mut().iter_mut() {
        if *p == provider {
            *s = stored;
            status.clone_into(line);
        }
    }
}

/// **Save key**: stores the API key of row `row` in Credential Manager at once and drops it, and
/// the tray process is told, which lifts the service's sign-in stop (they have no credential
/// fingerprint Lastdrop owns, so only this can lift it) ([`Outside::store_key`]). Turns the
/// service on (committed at once).
fn store_key(session: &Session, row: i32, key: &SharedString) {
    let Some(provider) = key_service(session, row) else {
        return;
    };
    let secret = SecretString::new(key.trim().to_owned());
    if secret.is_empty() {
        return;
    }
    let credential = key_credential(&session.edits.borrow().draft(), provider);
    let stored = session
        .outside
        .borrow_mut()
        .store_key(provider, &credential.target(), &secret);
    drop(secret);
    let strings = session.wording.borrow().strings();
    match stored {
        Ok(()) => {
            log::info!("{provider:?} API key stored");
            let status = text(
                strings,
                Msg::SettingsKeySaved,
                &[("service", provider.display_name())],
            );
            set_key_row(session, provider, true, &status);
            session
                .edits
                .borrow_mut()
                .set(Edit::ProviderEnabled(provider, true));
            commit_now(session);
        }
        Err(e) => {
            log::warn!("cannot store the {provider:?} key: {e}");
            let status = text(strings, Msg::SettingsKeyNotSaved, &[("detail", &e)]);
            let stored = session
                .keys
                .borrow()
                .iter()
                .any(|(p, s, _)| *p == provider && *s);
            set_key_row(session, provider, stored, &status);
            show_draft(session);
        }
    }
}

/// **Remove**: removes the stored API key of row `row`, and the tray process is told, which reads
/// the service again (and so shows that the key is missing) ([`Outside::remove_key`]).
fn clear_key(session: &Session, row: i32) {
    let Some(provider) = key_service(session, row) else {
        return;
    };
    let credential = key_credential(&session.edits.borrow().draft(), provider);
    let strings = session.wording.borrow().strings();
    let removed = session
        .outside
        .borrow_mut()
        .remove_key(provider, &credential.target());
    match removed {
        Ok(()) => {
            log::info!("{provider:?} API key removed");
            set_key_row(
                session,
                provider,
                false,
                strings.template(Msg::SettingsKeyRemoved),
            );
        }
        Err(e) => {
            log::warn!("cannot remove the {provider:?} key: {e}");
            let status = text(strings, Msg::SettingsKeyNotRemoved, &[("detail", &e)]);
            set_key_row(session, provider, true, &status);
        }
    }
    show_draft(session);
}

/// **Open folder** ("Add your own service", custom-services-spec §3): the folder of described
/// services in File Explorer, made first when it is missing.
fn open_providers_folder(session: &Session) {
    if !session.native {
        return;
    }
    let dir = &session.paths.providers;
    if let Err(e) = std::fs::create_dir_all(dir) {
        log::warn!("settings window: no providers folder: {e}");
        return;
    }
    if let Err(e) = lastdrop_win::shell::open_folder(dir) {
        log::warn!("settings window: providers folder not opened: {e}");
    }
}

/// Closing (✕, Alt+F4, Escape; settings-spec §5.5): every alias box's edit and a shortcut never
/// tried become pending, and the commit runs now. Returns whether the window may close: not when
/// the file could not be written (the InfoBar then offers Try again and Close without saving).
fn flush_for_close(session: &Session) -> bool {
    for (_, timer) in std::mem::take(&mut *session.alias_timers.borrow_mut()) {
        timer.stop();
    }
    session.edits.borrow_mut().flush_editors();
    let commit = commit_now(session);
    if commit.error.is_some() {
        session.closing.set(true);
        session.window.set_info_closing(true);
        return false;
    }
    true
}

/// Closes the settings window and releases it, not inside the window's own callback
/// ([`dialog::release_later`]). The last window closing ends the dialog process's event loop.
fn close_now() {
    let session = SESSION.with(|s| s.borrow_mut().take());
    if let Some(session) = session {
        session.commit_timer.stop();
        let _ = session.window.hide();
        dialog::release_later(session);
    }
}

/// Drops the settings window (after the event loop, or when it could not be shown).
pub fn release() {
    drop(SESSION.with(|s| s.borrow_mut().take()));
}

/// Whether the settings window is open (tests).
pub fn is_open() -> bool {
    SESSION.with(|s| s.borrow().is_some())
}

/// The open window's draft: its settings as last written with the pending edits (tests and
/// previews).
pub fn draft() -> Option<Settings> {
    SESSION.with(|s| s.borrow().as_ref().map(|s| s.edits.borrow().draft()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lastdrop_core::grammar::{English, LocaleFormat, Swedish};
    use lastdrop_core::model::MeterId;
    use lastdrop_core::settings::ProviderSettings;

    use crate::persist::SeenMeter;
    use crate::settings_edits::{content_of, language_at};

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
        let past_the_end = i32::try_from(Language::ALL.len() + 1).expect("a few languages");
        assert_eq!(language_at(past_the_end), None);
    }

    #[test]
    fn the_window_opens_at_its_preferred_size_or_what_fits() {
        assert_eq!(opening_size((1920.0, 1040.0)), (880.0, 680.0));
        // 1366 × 768 at 150 %: 910 × 480 logical, less the taskbar.
        assert_eq!(opening_size((910.0, 480.0)), (862.0, 480.0));
        assert_eq!(opening_size((700.0, 600.0)), (680.0, 552.0));
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
    fn shortcut_problems_come_in_the_language() {
        assert_eq!(
            shortcut_message(&ShortcutProblem::Refused(HotkeyError::InUse), &English),
            "The shortcut is already in use."
        );
        assert_eq!(
            shortcut_message(&ShortcutProblem::Refused(HotkeyError::InUse), &Swedish),
            "Kortkommandot används redan."
        );
        assert_eq!(
            shortcut_message(
                &ShortcutProblem::Refused(HotkeyError::Win32("Access is denied.".to_owned())),
                &English
            ),
            "Couldn't register the shortcut: Access is denied."
        );
        // Shift alone with a letter is refused by the registration, not by the window.
        assert_eq!(
            shortcut_message(
                &ShortcutProblem::Refused(HotkeyError::Unsupported("L".to_owned())),
                &Swedish
            ),
            shortcut_message(&ShortcutProblem::Invalid, &Swedish)
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

    #[test]
    fn services_say_how_they_are_read_and_expand_to_what_they_manage() {
        let mut settings = Settings::default();
        let line = |s: &Settings, p, stored| service_description(p, s, stored, &English);
        assert_eq!(
            line(&settings, ProviderId::Claude, false),
            "Claude Code's sign-in on this PC · 1 account"
        );
        assert_eq!(
            line(&settings, ProviderId::Copilot, false),
            "The GitHub CLI's sign-in (gh auth login)"
        );
        assert_eq!(
            line(&settings, ProviderId::Antigravity, false),
            "Read through its CLI (agy)"
        );
        assert_eq!(
            line(&settings, ProviderId::Meshy, false),
            "Needs an API key"
        );
        assert_eq!(line(&settings, ProviderId::Meshy, true), "API key stored");
        // Without a key here OpenCode Go uses OpenCode's own: it does not need one.
        assert_eq!(
            line(&settings, ProviderId::OpenCodeGo, false),
            "Uses the key OpenCode keeps"
        );
        assert_eq!(
            line(&settings, ProviderId::OpenCodeGo, true),
            "Uses your API key"
        );
        if let Some(p) = settings.provider_mut(ProviderId::Codex) {
            let taken: Vec<AccountKey> = p.accounts.iter().map(|a| a.key.clone()).collect();
            p.accounts
                .push(lastdrop_core::settings::AccountSettings::for_folder(
                    SourceSettings::CodexHome {
                        path: r"C:\Users\tester\.codex-work".into(),
                    },
                    None,
                    &taken,
                ));
        }
        assert_eq!(
            service_description(ProviderId::Codex, &settings, false, &Swedish),
            "Inloggningen i Codex på den här datorn · 2 konton"
        );

        let kind = |p| service_kind(p, &settings);
        assert_eq!(kind(ProviderId::Claude), ui::ServiceKind::Accounts);
        assert_eq!(kind(ProviderId::Codex), ui::ServiceKind::Accounts);
        assert_eq!(kind(ProviderId::Meshy), ui::ServiceKind::Key);
        assert_eq!(kind(ProviderId::OpenCodeGo), ui::ServiceKind::Key);
        for p in [
            ProviderId::Copilot,
            ProviderId::Cursor,
            ProviderId::Antigravity,
            ProviderId::Grok,
        ] {
            assert_eq!(kind(p), ui::ServiceKind::None, "{p:?}");
        }
        // A key service the settings lack has no key row to expand to.
        settings
            .providers
            .retain(|e| e.id() != Some(ProviderId::Meshy));
        assert_eq!(
            service_kind(ProviderId::Meshy, &settings),
            ui::ServiceKind::None
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
        settings.readout.content = content_of(vec![fable.clone(), opus.clone(), five.clone()]);
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
}
