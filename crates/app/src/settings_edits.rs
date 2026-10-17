//! The settings window's change model (settings-spec §5): instant apply, one field at a time.
//!
//! Three states:
//! 1. **Raw editor values**: what an editor shows while the user is still typing (an alias, a
//!    shortcut being composed). An editor holding such an edit is *dirty* until its commit point.
//! 2. **Pending edits**: [`Edit`]s whose commit point has arrived, valid and normalised, keyed by
//!    a stable [`Field`] (a later edit of the same field replaces an earlier one).
//! 3. **Base**: the settings as last written.
//!
//! The window shows the **draft** ([`Edits::draft`]): the base with the pending edits applied.
//! [`Edits::commit`] applies the pending edits — and nothing else from the window — onto
//! `settings.json` as it is now, under the settings lock (settings-spec §6), with the side effects
//! a field needs (start with Windows, the hotkey test). Nothing here touches a window, a file or
//! the registry itself: that is [`Outside`]'s, so the model is tested with fakes.
//!
//! Every per-account state is keyed by [`SourceKey`] (service + account key), never by the
//! account key alone: Claude's and Codex's default accounts are both `default`.

use std::collections::{BTreeMap, BTreeSet};

use lastdrop_core::grammar::{Language, TimeStyle};
use lastdrop_core::model::{MeterId, MeterKey, ProviderId, SourceKey};
use lastdrop_core::settings::{
    AccountSettings, Hotkey, ReadoutContent, RefreshInterval, Settings, clean_alias,
};
use lastdrop_win::hotkey::HotkeyError;
use lastdrop_win::named_lock::NamedLock;

use crate::dialog::{self, Changes};
use crate::persist;
use crate::settings_sync::SettingsFile;

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

/// The language choices' index of a language setting: 0 follows Windows (so does a tag without a
/// table), else the language's place in [`Language::ALL`] after it.
pub fn language_index(setting: Option<&str>) -> i32 {
    setting
        .and_then(Language::from_tag)
        .and_then(|l| Language::ALL.iter().position(|a| *a == l))
        .and_then(|i| i32::try_from(i + 1).ok())
        .unwrap_or(0)
}

/// The language setting of a language choices' index: `None` follows Windows, else the
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

/// The pins of `settings` (empty in auto mode).
pub fn pins(settings: &Settings) -> Vec<MeterKey> {
    match &settings.readout.content {
        ReadoutContent::Pinned(p) => p.clone(),
        ReadoutContent::Auto => Vec::new(),
    }
}

/// Pinned meters, or auto when none is left.
pub fn content_of(pins: Vec<MeterKey>) -> ReadoutContent {
    if pins.is_empty() {
        ReadoutContent::Auto
    } else {
        ReadoutContent::Pinned(pins)
    }
}

/// A stable id of what one edit changes: a later edit of the same field replaces an earlier one.
/// The order is the order edits are applied in (structural account changes first).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    /// An account removed.
    AccountRemoved(SourceKey),
    /// An account added for a folder ([`crate::settings_accounts::folder_id`]).
    AccountAdded(ProviderId, String),
    /// A service turned on or off.
    ProviderEnabled(ProviderId),
    /// A meter of a service shown or hidden (`hidden_meters` is per service).
    HiddenMeter(ProviderId, MeterId),
    /// The readout's content: auto or the pinned meters.
    Pins,
    /// The readout on or off.
    ReadoutEnabled,
    /// The readout on every taskbar.
    ReadoutEvery,
    /// Clock times or time remaining.
    TimeStyle,
    /// The refresh interval.
    Interval,
    /// The theme.
    Theme,
    /// The daily update check.
    UpdatesCheck,
    /// Notifications on or off.
    NotifyEnabled,
    /// The notification levels (both combo boxes are one field).
    Levels,
    /// Notify when used up.
    NotifyUsedUp,
    /// Notify on resets.
    NotifyResets,
    /// Notify on low balances.
    NotifyLowBalance,
    /// Start with Windows.
    StartWithWindows,
    /// The language.
    Language,
    /// The global shortcut, whole (on/off, modifiers and key are one field).
    Hotkey,
    /// Hide personal info.
    HidePersonalInfo,
    /// An account's alias.
    Alias(SourceKey),
}

/// One pending change, with its value (already valid and normalised).
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// Remove the account (unless it is its service's last one) and its readout pins.
    AccountRemoved(SourceKey),
    /// Add the account for the folder (its [`crate::settings_accounts::folder_id`]), unless the
    /// service already has one with its key or its source.
    AccountAdded(ProviderId, Box<AccountSettings>, String),
    /// Turn a service on or off.
    ProviderEnabled(ProviderId, bool),
    /// Hide (`true`) or show a meter of a service.
    HiddenMeter(ProviderId, MeterId, bool),
    /// The readout's content.
    Pins(ReadoutContent),
    /// The readout on or off.
    ReadoutEnabled(bool),
    /// The readout on every taskbar.
    ReadoutEvery(bool),
    /// How times show.
    TimeStyle(TimeStyle),
    /// The refresh interval.
    Interval(RefreshInterval),
    /// The theme (`None` = the Windows theme).
    Theme(Option<String>),
    /// The daily update check.
    UpdatesCheck(bool),
    /// Notifications on or off.
    NotifyEnabled(bool),
    /// The notification levels, as stored.
    Levels(Vec<u8>),
    /// Notify when used up.
    NotifyUsedUp(bool),
    /// Notify on resets.
    NotifyResets(bool),
    /// Notify on low balances.
    NotifyLowBalance(bool),
    /// Start with Windows.
    StartWithWindows(bool),
    /// The language (`None` follows Windows).
    Language(Option<String>),
    /// The shortcut (`None` = off).
    Hotkey(Option<Hotkey>),
    /// Hide personal info.
    HidePersonalInfo(bool),
    /// An account's alias (cleaned).
    Alias(SourceKey, Option<String>),
}

impl Edit {
    /// The field the edit changes.
    pub fn field(&self) -> Field {
        match self {
            Edit::AccountRemoved(key) => Field::AccountRemoved(key.clone()),
            Edit::AccountAdded(p, _, folder) => Field::AccountAdded(*p, folder.clone()),
            Edit::ProviderEnabled(p, _) => Field::ProviderEnabled(*p),
            Edit::HiddenMeter(p, m, _) => Field::HiddenMeter(*p, m.clone()),
            Edit::Pins(_) => Field::Pins,
            Edit::ReadoutEnabled(_) => Field::ReadoutEnabled,
            Edit::ReadoutEvery(_) => Field::ReadoutEvery,
            Edit::TimeStyle(_) => Field::TimeStyle,
            Edit::Interval(_) => Field::Interval,
            Edit::Theme(_) => Field::Theme,
            Edit::UpdatesCheck(_) => Field::UpdatesCheck,
            Edit::NotifyEnabled(_) => Field::NotifyEnabled,
            Edit::Levels(_) => Field::Levels,
            Edit::NotifyUsedUp(_) => Field::NotifyUsedUp,
            Edit::NotifyResets(_) => Field::NotifyResets,
            Edit::NotifyLowBalance(_) => Field::NotifyLowBalance,
            Edit::StartWithWindows(_) => Field::StartWithWindows,
            Edit::Language(_) => Field::Language,
            Edit::Hotkey(_) => Field::Hotkey,
            Edit::HidePersonalInfo(_) => Field::HidePersonalInfo,
            Edit::Alias(key, _) => Field::Alias(key.clone()),
        }
    }

    /// Applies the edit onto `s` (the base for the draft, the file for a commit); only its field
    /// changes. An edit of an account or a service `s` does not have changes nothing.
    pub fn apply(&self, s: &mut Settings) {
        match self {
            Edit::AccountRemoved(key) => remove_account(s, key),
            Edit::AccountAdded(provider, account, _) => {
                if let Some(p) = s.provider_mut(*provider)
                    && !p
                        .accounts
                        .iter()
                        .any(|a| a.key == account.key || a.source == account.source)
                {
                    p.accounts.push(account.as_ref().clone());
                }
            }
            Edit::ProviderEnabled(provider, on) => {
                if let Some(p) = s.provider_mut(*provider) {
                    p.enabled = *on;
                }
            }
            Edit::HiddenMeter(provider, meter, hidden) => {
                if let Some(p) = s.provider_mut(*provider) {
                    p.hidden_meters.retain(|m| m != meter);
                    if *hidden {
                        p.hidden_meters.push(meter.clone());
                    }
                }
            }
            Edit::Pins(content) => s.readout.content.clone_from(content),
            Edit::ReadoutEnabled(on) => s.readout.enabled = *on,
            Edit::ReadoutEvery(on) => s.readout.every_taskbar = *on,
            Edit::TimeStyle(style) => s.readout.time_style = *style,
            Edit::Interval(interval) => s.refresh.interval = *interval,
            Edit::Theme(theme) => s.theme.clone_from(theme),
            Edit::UpdatesCheck(on) => s.updates.check = *on,
            Edit::NotifyEnabled(on) => s.notifications.enabled = *on,
            Edit::Levels(levels) => s.notifications.levels.clone_from(levels),
            Edit::NotifyUsedUp(on) => s.notifications.used_up = *on,
            Edit::NotifyResets(on) => s.notifications.resets = *on,
            Edit::NotifyLowBalance(on) => s.notifications.low_balance = *on,
            Edit::StartWithWindows(on) => s.start_with_windows = *on,
            Edit::Language(language) => s.language.clone_from(language),
            Edit::Hotkey(hotkey) => s.hotkey.clone_from(hotkey),
            Edit::HidePersonalInfo(on) => s.hide_personal_info = *on,
            Edit::Alias(key, alias) => {
                let account = s
                    .provider_mut(key.provider)
                    .and_then(|p| p.accounts.iter_mut().find(|a| a.key == key.account));
                if let Some(account) = account {
                    account.alias.clone_from(alias);
                }
            }
        }
    }
}

/// Removes the account `key` (not its service's last one) and the readout pins of its meters
/// (accounts-spec §7); no pins left → auto.
fn remove_account(s: &mut Settings, key: &SourceKey) {
    let Some(p) = s.provider_mut(key.provider) else {
        return;
    };
    if p.accounts.len() < 2 {
        return;
    }
    let before = p.accounts.len();
    p.accounts.retain(|a| a.key != key.account);
    if p.accounts.len() == before {
        return;
    }
    let mut pinned = pins(s);
    pinned.retain(|k| k.source() != *key);
    if matches!(s.readout.content, ReadoutContent::Pinned(_)) {
        s.readout.content = content_of(pinned);
    }
}

/// Why the shortcut is not registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutProblem {
    /// No modifier, or not a letter, a digit or F1–F24.
    Invalid,
    /// Registering it failed.
    Refused(HotkeyError),
}

/// What the shortcut editor shows: on or off, the modifiers and the key as typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    /// The shortcut is on.
    pub enabled: bool,
    /// Ctrl.
    pub ctrl: bool,
    /// Alt.
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// The Windows key.
    pub win: bool,
    /// The key as typed.
    pub key: String,
}

impl Shortcut {
    /// The editor for a saved hotkey; Ctrl + Alt + L, off, when there is none.
    pub fn of(hotkey: Option<&Hotkey>) -> Shortcut {
        match hotkey {
            Some(h) => Shortcut {
                enabled: true,
                ctrl: h.ctrl,
                alt: h.alt,
                shift: h.shift,
                win: h.win,
                key: h.key.clone(),
            },
            None => Shortcut {
                enabled: false,
                ctrl: true,
                alt: true,
                shift: false,
                win: false,
                key: "L".to_owned(),
            },
        }
    }

    /// The hotkey it describes (`None` when off), or why it is not valid: a letter, a digit or
    /// F1–F24 (trimmed, upper case) and at least one modifier. Shift alone with a letter or a
    /// digit is refused by the registration test, not here.
    pub fn parse(&self) -> Result<Option<Hotkey>, ShortcutProblem> {
        if !self.enabled {
            return Ok(None);
        }
        let key = self.key.trim().to_uppercase();
        let valid_key = (key.chars().count() == 1
            && key.chars().all(|c| c.is_ascii_alphanumeric()))
            || key
                .strip_prefix('F')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (1..=24).contains(&n));
        if !valid_key || !(self.ctrl || self.alt || self.shift || self.win) {
            return Err(ShortcutProblem::Invalid);
        }
        Ok(Some(Hotkey {
            ctrl: self.ctrl,
            alt: self.alt,
            shift: self.shift,
            win: self.win,
            key,
        }))
    }
}

/// The shortcut editor's state.
#[derive(Clone, Debug)]
struct ShortcutEditor {
    /// What it shows.
    shown: Shortcut,
    /// Bumped by every shortcut edit.
    revision: u64,
    /// The revision of the pending [`Edit::Hotkey`].
    pending: Option<u64>,
    /// The last revision a commit tried to register (successfully or not).
    attempted: Option<u64>,
    /// What its card says.
    problem: Option<ShortcutProblem>,
    /// It holds an edit that is not pending (incomplete, invalid or refused).
    dirty: bool,
}

/// Why start with Windows did not change as asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartupProblem {
    /// The registry refused the change: start with Windows stays as it was.
    NotChanged(String),
    /// The registry changed, the file could not be written and undoing the registry failed:
    /// Windows starts Lastdrop at sign-in (`on`) or not, whatever the file says.
    Stuck {
        /// What the registry says now.
        on: bool,
    },
}

/// What a commit did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Commit {
    /// The fields written (the base is the written file now); empty when nothing was pending or
    /// the write failed.
    pub written: BTreeSet<Field>,
    /// Why the file could not be written (the pending edits stay pending).
    pub error: Option<String>,
    /// The shortcut was refused by the registration test (it dropped out of this commit).
    pub shortcut: Option<ShortcutProblem>,
    /// Start with Windows did not change as asked.
    pub startup: Option<StartupProblem>,
}

impl Commit {
    /// Something was written.
    pub fn saved(&self) -> bool {
        !self.written.is_empty()
    }
}

/// What a commit does outside the model (the real one is [`RealOutside`]; tests use fakes).
pub trait Outside {
    /// Takes the settings lock (settings-spec §6) and reads `settings.json`: its settings, or
    /// `base` without a usable file.
    fn lock_and_read(&mut self, base: &Settings) -> Settings;
    /// Writes the settings atomically, still under the lock; the error's text is secret-free.
    fn write(&mut self, settings: &Settings) -> Result<(), String>;
    /// Releases the lock.
    fn unlock(&mut self);
    /// Turns start with Windows on or off (the Run value).
    fn set_autostart(&mut self, on: bool) -> Result<(), String>;
    /// Registers the hotkey and releases it at once: whether the tray process will get it.
    fn test_hotkey(&mut self, hotkey: &Hotkey) -> Result<(), HotkeyError>;
    /// The file was written: tells the tray process.
    fn written(&mut self);
}

/// The real [`Outside`]: `settings.json` under the settings lock, the Run value, `RegisterHotKey`
/// and the settings-changed message to the tray process.
pub struct RealOutside {
    /// `settings.json` and its lock.
    pub file: SettingsFile,
    lock: Option<NamedLock>,
}

impl RealOutside {
    /// For `file`.
    pub fn new(file: SettingsFile) -> RealOutside {
        RealOutside { file, lock: None }
    }
}

impl Outside for RealOutside {
    fn lock_and_read(&mut self, base: &Settings) -> Settings {
        self.lock = self.file.lock();
        self.file.read(base)
    }

    fn write(&mut self, settings: &Settings) -> Result<(), String> {
        persist::save_settings(&self.file.path, settings).map_err(|e| e.to_string())
    }

    fn unlock(&mut self) {
        self.lock = None;
    }

    fn set_autostart(&mut self, on: bool) -> Result<(), String> {
        let changed = if on {
            std::env::current_exe().and_then(|exe| lastdrop_win::autostart::enable(&exe))
        } else {
            lastdrop_win::autostart::disable()
        };
        changed.map_err(|e| {
            log::warn!("autostart: {e}");
            e.to_string()
        })
    }

    fn test_hotkey(&mut self, hotkey: &Hotkey) -> Result<(), HotkeyError> {
        lastdrop_win::hotkey::register(hotkey, Box::new(|| {})).map(|r| r.unregister())
    }

    fn written(&mut self) {
        dialog::notify(Changes {
            settings: true,
            ..Changes::default()
        });
    }
}

/// An [`Outside`] in memory (previews and the window's interaction tests): the "file" is a
/// value, the registry and the hotkey test do what the fields say, and nothing reaches the tray.
#[derive(Clone, Debug, Default)]
pub struct MemoryOutside {
    /// The file's settings.
    pub disk: Settings,
    /// Writes fail with this detail.
    pub fail_write: Option<String>,
    /// The hotkey test refuses with this.
    pub refuse_hotkey: Option<HotkeyError>,
    /// Hotkeys tested.
    pub tested: Vec<Hotkey>,
    /// Each start with Windows change asked for.
    pub autostart: Vec<bool>,
    /// Successful writes.
    pub writes: usize,
}

impl Outside for MemoryOutside {
    fn lock_and_read(&mut self, _base: &Settings) -> Settings {
        self.disk.clone()
    }

    fn write(&mut self, settings: &Settings) -> Result<(), String> {
        if let Some(detail) = &self.fail_write {
            return Err(detail.clone());
        }
        self.disk = settings.clone();
        self.writes += 1;
        Ok(())
    }

    fn unlock(&mut self) {}

    fn set_autostart(&mut self, on: bool) -> Result<(), String> {
        self.autostart.push(on);
        Ok(())
    }

    fn test_hotkey(&mut self, hotkey: &Hotkey) -> Result<(), HotkeyError> {
        self.tested.push(hotkey.clone());
        self.refuse_hotkey.clone().map_or(Ok(()), Err)
    }

    fn written(&mut self) {}
}

/// An [`Outside`] shared with whoever made it (the window's interaction tests look at it while the
/// window owns it).
impl Outside for std::rc::Rc<std::cell::RefCell<MemoryOutside>> {
    fn lock_and_read(&mut self, base: &Settings) -> Settings {
        self.borrow_mut().lock_and_read(base)
    }

    fn write(&mut self, settings: &Settings) -> Result<(), String> {
        self.borrow_mut().write(settings)
    }

    fn unlock(&mut self) {
        self.borrow_mut().unlock();
    }

    fn set_autostart(&mut self, on: bool) -> Result<(), String> {
        self.borrow_mut().set_autostart(on)
    }

    fn test_hotkey(&mut self, hotkey: &Hotkey) -> Result<(), HotkeyError> {
        self.borrow_mut().test_hotkey(hotkey)
    }

    fn written(&mut self) {
        self.borrow_mut().written();
    }
}

/// The window's edits: the base, the pending edits and the raw editors (see the module docs).
#[derive(Clone, Debug)]
pub struct Edits {
    base: Settings,
    pending: BTreeMap<Field, Edit>,
    /// Alias editors holding an edit, by account: the text as typed.
    aliases: BTreeMap<SourceKey, String>,
    shortcut: ShortcutEditor,
    /// "Meters I pin" is chosen but no meter yet (settings-spec §4): the saved mode stays auto.
    choosing_pins: bool,
}

impl Edits {
    /// No edits yet, on settings as read.
    pub fn new(base: Settings) -> Edits {
        let shown = Shortcut::of(base.hotkey.as_ref());
        Edits {
            base,
            pending: BTreeMap::new(),
            aliases: BTreeMap::new(),
            shortcut: ShortcutEditor {
                shown,
                revision: 0,
                pending: None,
                attempted: None,
                problem: None,
                dirty: false,
            },
            choosing_pins: false,
        }
    }

    /// The settings as last written (or read).
    pub fn base(&self) -> &Settings {
        &self.base
    }

    /// What the window shows: the base with the pending edits applied.
    pub fn draft(&self) -> Settings {
        let mut draft = self.base.clone();
        for edit in self.pending.values() {
            edit.apply(&mut draft);
        }
        draft
    }

    /// The pending edits, in the order they apply.
    pub fn pending(&self) -> impl Iterator<Item = &Edit> {
        self.pending.values()
    }

    /// Whether `field` has a pending edit.
    pub fn is_pending(&self, field: &Field) -> bool {
        self.pending.contains_key(field)
    }

    /// Whether anything is pending.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Makes `edit` pending (it replaces a pending edit of its field).
    pub fn set(&mut self, edit: Edit) {
        if let Edit::AccountRemoved(key) = &edit
            && let Some(Edit::Pins(content)) = self.pending.get_mut(&Field::Pins)
        {
            // Applied after the removal, a pin of the removed account would come back.
            let mut kept = match content {
                ReadoutContent::Pinned(p) => p.clone(),
                ReadoutContent::Auto => Vec::new(),
            };
            kept.retain(|k| k.source() != *key);
            *content = content_of(kept);
        }
        self.pending.insert(edit.field(), edit);
    }

    /// Another writer wrote `edit` to the file (Theme Studio's "Save and use"): it is part of the
    /// base now, and a pending edit of its field is dropped.
    pub fn written_elsewhere(&mut self, edit: &Edit) {
        self.pending.remove(&edit.field());
        edit.apply(&mut self.base);
    }

    /// The interval combo box picked `index`. An interval it cannot show (a hand-edited 4 min
    /// shows as 5 min) stays until the user picks another choice.
    pub fn set_interval_index(&mut self, index: i32) {
        let base = self.base.refresh.interval;
        let interval = if usize::try_from(index).ok() == Some(interval_index(base)) {
            base
        } else {
            interval_at(index)
        };
        self.set(Edit::Interval(interval));
    }

    /// The language combo box picked `index`. A language this build has no table for (a newer
    /// build's) shows as "Same as Windows" and stays until the user picks another choice.
    pub fn set_language_index(&mut self, index: i32) {
        let base = &self.base.language;
        let language = if index == language_index(base.as_deref()) {
            base.clone()
        } else {
            language_at(index)
        };
        self.set(Edit::Language(language));
    }

    /// The level combo boxes show `first` and `second`. Levels they cannot show (a hand-edited
    /// 83) stay until the user picks others.
    pub fn set_levels(&mut self, first: i32, second: i32) {
        let base = &self.base.notifications;
        let levels = if (first, second) == level_indices(&base.levels()) {
            base.levels.clone()
        } else {
            levels_at(first, second)
        };
        self.set(Edit::Levels(levels));
    }

    /// "The most constrained meters" (`pinned` false) or "Meters I pin". Pinned without a pin
    /// yet is the choosing state: nothing changes until a meter is picked.
    pub fn set_readout_mode(&mut self, pinned: bool) {
        if pinned {
            if pins(&self.draft()).is_empty() {
                self.choosing_pins = true;
            }
        } else {
            self.choosing_pins = false;
            self.set(Edit::Pins(ReadoutContent::Auto));
        }
    }

    /// The readout mode combo box's index: 1 when meters are pinned or being chosen.
    pub fn readout_mode_index(&self) -> i32 {
        let pinned = matches!(self.draft().readout.content, ReadoutContent::Pinned(_));
        i32::from(pinned || self.choosing_pins)
    }

    /// "Meters I pin" is chosen but no meter is pinned yet.
    pub fn choosing_pins(&self) -> bool {
        self.choosing_pins
    }

    /// Pins (`on`) or unpins a meter (R§4.3: at most two, in the order picked).
    pub fn set_pinned(&mut self, key: MeterKey, on: bool) {
        let mut current = pins(&self.draft());
        if on != current.contains(&key) {
            crate::wiring::toggle_in(&mut current, key);
        }
        self.choosing_pins = current.is_empty();
        self.set(Edit::Pins(content_of(current)));
    }

    /// Shows or hides a meter (for its whole service). A hidden meter is unpinned, for every
    /// account of the service.
    pub fn set_meter_shown(&mut self, key: &MeterKey, shown: bool) {
        self.set(Edit::HiddenMeter(key.provider, key.meter.clone(), !shown));
        if shown {
            return;
        }
        let current = pins(&self.draft());
        let kept: Vec<MeterKey> = current
            .iter()
            .filter(|k| !(k.provider == key.provider && k.meter == key.meter))
            .cloned()
            .collect();
        if kept.len() != current.len() {
            self.choosing_pins = kept.is_empty();
            self.set(Edit::Pins(content_of(kept)));
        }
    }

    /// Removes an account (not its service's last one). Returns whether it was removed.
    pub fn remove_account(&mut self, key: &SourceKey) -> bool {
        let draft = self.draft();
        let Some(p) = draft.provider(key.provider) else {
            return false;
        };
        if p.accounts.len() < 2 || !p.accounts.iter().any(|a| a.key == key.account) {
            return false;
        }
        let was_pinned = matches!(draft.readout.content, ReadoutContent::Pinned(_));
        self.aliases.remove(key);
        self.pending.remove(&Field::Alias(key.clone()));
        self.set(Edit::AccountRemoved(key.clone()));
        if was_pinned && pins(&self.draft()).is_empty() {
            self.choosing_pins = true;
        }
        true
    }

    /// An alias editor's text changed: it holds an edit until its commit point.
    pub fn alias_typed(&mut self, key: SourceKey, text: &str) {
        self.aliases.insert(key, text.to_owned());
    }

    /// An alias editor's commit point (Enter, focus loss, a pause in typing, its section
    /// closing): its edit becomes pending when it changes the alias. Returns whether it did.
    pub fn alias_done(&mut self, key: &SourceKey) -> bool {
        let Some(text) = self.aliases.remove(key) else {
            return false;
        };
        let draft = self.draft();
        let Some(account) = draft
            .provider(key.provider)
            .and_then(|p| p.accounts.iter().find(|a| a.key == key.account))
        else {
            return false;
        };
        let alias = clean_alias(&text);
        if alias == account.alias {
            return false;
        }
        self.set(Edit::Alias(key.clone(), alias));
        true
    }

    /// Whether the alias editor of `key` holds an edit not yet pending.
    pub fn alias_dirty(&self, key: &SourceKey) -> bool {
        self.aliases.contains_key(key)
    }

    /// The accounts whose alias editors hold an edit.
    pub fn dirty_aliases(&self) -> Vec<SourceKey> {
        self.aliases.keys().cloned().collect()
    }

    /// What the shortcut editor shows: the composition while it holds an edit or a pending one,
    /// else the saved hotkey (off keeps the composition, so turning it on again finds it).
    pub fn shortcut(&self) -> Shortcut {
        if self.shortcut_dirty() {
            return self.shortcut.shown.clone();
        }
        match &self.base.hotkey {
            Some(h) => Shortcut::of(Some(h)),
            None => Shortcut {
                enabled: false,
                ..self.shortcut.shown.clone()
            },
        }
    }

    /// What the shortcut's card says: why the composition is not registered.
    pub fn shortcut_problem(&self) -> Option<&ShortcutProblem> {
        self.shortcut.problem.as_ref()
    }

    /// Whether the shortcut editor holds an edit (not committed yet, incomplete, invalid or
    /// refused): reconciling leaves it alone.
    pub fn shortcut_dirty(&self) -> bool {
        self.shortcut.dirty || self.pending.contains_key(&Field::Hotkey)
    }

    /// The shortcut editor changed (a toggle, a modifier, the key). A complete and valid
    /// shortcut becomes pending at once; an incomplete or invalid one is only hinted at, and
    /// the registered shortcut stays.
    pub fn shortcut_edited(&mut self, shown: Shortcut) {
        self.shortcut.revision += 1;
        self.shortcut.shown = shown;
        self.queue_shortcut();
    }

    /// The shortcut card's **Try again**: the composition is tried once more.
    pub fn shortcut_retry(&mut self) {
        self.queue_shortcut();
    }

    /// Makes the editor's composition pending when it is valid, else hints why not.
    fn queue_shortcut(&mut self) {
        match self.shortcut.shown.parse() {
            Ok(hotkey) => {
                self.shortcut.problem = None;
                self.shortcut.dirty = false;
                self.shortcut.pending = Some(self.shortcut.revision);
                self.set(Edit::Hotkey(hotkey));
            }
            Err(problem) => {
                self.shortcut.problem = Some(problem);
                self.shortcut.dirty = true;
                self.shortcut.pending = None;
                self.pending.remove(&Field::Hotkey);
            }
        }
    }

    /// Closing (settings-spec §5.5): every alias editor's edit becomes pending, and a complete
    /// valid shortcut whose revision was never tried. A refused shortcut is not tried again (the
    /// one that works stays); an incomplete one is dropped.
    pub fn flush_editors(&mut self) {
        for key in self.dirty_aliases() {
            self.alias_done(&key);
        }
        let s = &self.shortcut;
        if s.dirty
            && s.attempted != Some(s.revision)
            && !self.pending.contains_key(&Field::Hotkey)
            && s.shown.parse().is_ok()
        {
            self.queue_shortcut();
        }
    }

    /// Writes the pending edits onto `settings.json` as it is now (settings-spec §5.3): under
    /// the lock, read the file; start with Windows in the registry when that field changes it;
    /// the hotkey tested when that field changes it; the edits applied onto the file; write;
    /// unlock; tell the tray. A refused shortcut or a registry that refuses start with Windows
    /// drops out of the commit (the rest is written). When the write fails, everything else
    /// stays pending and a registry change is undone.
    pub fn commit(&mut self, out: &mut dyn Outside) -> Commit {
        let mut report = Commit::default();
        if self.pending.is_empty() {
            return report;
        }
        let disk = out.lock_and_read(&self.base);

        let mut autostart_set = None;
        if let Some(Edit::StartWithWindows(on)) = self.pending.get(&Field::StartWithWindows)
            && *on != disk.start_with_windows
        {
            let on = *on;
            match out.set_autostart(on) {
                Ok(()) => autostart_set = Some(on),
                Err(detail) => {
                    self.pending.remove(&Field::StartWithWindows);
                    report.startup = Some(StartupProblem::NotChanged(detail));
                }
            }
        }
        let mut hotkey_tried = false;
        if let Some(Edit::Hotkey(Some(hotkey))) = self.pending.get(&Field::Hotkey)
            && disk.hotkey.as_ref() != Some(hotkey)
        {
            hotkey_tried = true;
            if let Err(e) = out.test_hotkey(hotkey) {
                log::warn!("hotkey: {e}");
                self.pending.remove(&Field::Hotkey);
                let problem = ShortcutProblem::Refused(e);
                self.shortcut.attempted = self.shortcut.pending.take();
                self.shortcut.dirty = true;
                self.shortcut.problem = Some(problem.clone());
                report.shortcut = Some(problem);
            }
        }
        if self.pending.is_empty() {
            out.unlock();
            return report;
        }

        let mut merged = disk;
        for edit in self.pending.values() {
            edit.apply(&mut merged);
        }
        match out.write(&merged) {
            Ok(()) => {
                out.unlock();
                out.written();
                if self.pending.contains_key(&Field::Hotkey) {
                    self.shortcut.attempted = self.shortcut.pending.take();
                    self.shortcut.dirty = false;
                    self.shortcut.problem = None;
                }
                report.written = std::mem::take(&mut self.pending).into_keys().collect();
                self.base = merged;
            }
            Err(detail) => {
                log::error!("cannot save settings: {detail}");
                if let Some(on) = autostart_set
                    && out.set_autostart(!on).is_err()
                {
                    report.startup = Some(StartupProblem::Stuck { on });
                }
                if hotkey_tried {
                    // Tested, but not saved: a later commit tests it again.
                    self.shortcut.attempted = None;
                }
                out.unlock();
                report.error = Some(detail);
            }
        }
        report
    }

    /// The draft's account `key`.
    pub fn account(&self, key: &SourceKey) -> Option<AccountSettings> {
        self.draft()
            .provider(key.provider)?
            .accounts
            .iter()
            .find(|a| a.key == key.account)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use lastdrop_core::model::AccountKey;
    use lastdrop_core::settings::{Pause, SourceSettings};

    use super::*;

    /// An [`Outside`] in memory: the file's settings, what the registry and the hotkey test did.
    #[derive(Default)]
    struct Fake {
        disk: Settings,
        fail_write: bool,
        /// Each call to `set_autostart`.
        autostart: Vec<bool>,
        /// Whether each next call to `set_autostart` fails (then they succeed).
        autostart_fails: Vec<bool>,
        /// Hotkeys tested.
        tested: Vec<Hotkey>,
        refuse: Option<HotkeyError>,
        locked: bool,
        notified: usize,
    }

    impl Outside for Fake {
        fn lock_and_read(&mut self, _base: &Settings) -> Settings {
            assert!(!self.locked, "locked twice");
            self.locked = true;
            self.disk.clone()
        }
        fn write(&mut self, settings: &Settings) -> Result<(), String> {
            assert!(self.locked, "written under the lock");
            if self.fail_write {
                return Err("The disk is full.".to_owned());
            }
            self.disk = settings.clone();
            Ok(())
        }
        fn unlock(&mut self) {
            self.locked = false;
        }
        fn set_autostart(&mut self, on: bool) -> Result<(), String> {
            assert!(self.locked, "registry before the file, under the lock");
            self.autostart.push(on);
            let fails = !self.autostart_fails.is_empty() && self.autostart_fails.remove(0);
            if fails {
                return Err("Access is denied.".to_owned());
            }
            Ok(())
        }
        fn test_hotkey(&mut self, hotkey: &Hotkey) -> Result<(), HotkeyError> {
            self.tested.push(hotkey.clone());
            self.refuse.clone().map_or(Ok(()), Err)
        }
        fn written(&mut self) {
            assert!(!self.locked, "the tray is told after the lock is released");
            self.notified += 1;
        }
    }

    fn fake(disk: &Settings) -> Fake {
        Fake {
            disk: disk.clone(),
            ..Fake::default()
        }
    }

    fn key(provider: ProviderId, account: &str) -> SourceKey {
        SourceKey {
            provider,
            account: AccountKey(account.to_owned()),
        }
    }

    fn meter(provider: ProviderId, account: &str, id: &str) -> MeterKey {
        MeterKey {
            provider,
            account: AccountKey(account.to_owned()),
            meter: MeterId::new(id),
        }
    }

    fn ctrl_alt(key: &str) -> Shortcut {
        Shortcut {
            enabled: true,
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: key.to_owned(),
        }
    }

    /// Claude with a second folder account `f-work`.
    fn with_work() -> Settings {
        let mut s = Settings::default();
        if let Some(p) = s.provider_mut(ProviderId::Claude) {
            p.accounts.push(AccountSettings {
                key: AccountKey("f-work".to_owned()),
                source: SourceSettings::ClaudeConfigDir {
                    path: r"C:\Users\tester\.claude-work".into(),
                },
                ..AccountSettings::primary()
            });
        }
        s
    }

    #[test]
    fn choices_round_trip_and_snap_to_the_nearest() {
        assert_eq!(level_indices(&[80, 95]), (5, 8));
        assert_eq!(levels_at(5, 8), [80, 95]);
        assert_eq!(level_indices(&[]), (0, 0));
        assert_eq!(levels_at(0, 0), Vec::<u8>::new());
        assert_eq!(level_indices(&[90]), (7, 0));
        assert_eq!(levels_at(8, 5), [80, 95]);
        assert_eq!(levels_at(7, 7), [90]);
        assert_eq!(levels_at(0, 6), [85]);
        assert_eq!(levels_at(-1, 99), Vec::<u8>::new());
        assert_eq!(level_indices(&[83, 99]), (6, 8));
        for (i, _) in INTERVALS.iter().enumerate() {
            let index = i32::try_from(i).expect("small");
            assert_eq!(interval_index(interval_at(index)), i);
        }
        assert_eq!(interval_index(RefreshInterval::Minutes(4)), 3);
        assert_eq!(interval_index(RefreshInterval::Minutes(25)), 6);
        assert_eq!(interval_at(99), RefreshInterval::Minutes(30));
        assert_eq!(interval_at(-1), RefreshInterval::Minutes(5));
        let tags = Language::ALL.map(|l| Some(l.tag()));
        for setting in [None].into_iter().chain(tags) {
            assert_eq!(language_at(language_index(setting)).as_deref(), setting);
        }
        assert_eq!(language_index(Some("sv-SE")), language_index(Some("sv")));
        assert_eq!(language_index(Some("qaa")), 0);
        assert_eq!(language_at(-1), None);
    }

    #[test]
    fn values_the_combo_boxes_cannot_show_stay_until_another_choice_is_picked() {
        let mut base = Settings {
            language: Some("qaa".to_owned()),
            ..Settings::default()
        };
        base.notifications.levels = vec![83];
        base.refresh.interval = RefreshInterval::Minutes(4);
        let mut edits = Edits::new(base.clone());
        // Picked as shown: kept as stored.
        edits.set_language_index(0);
        edits.set_levels(6, 0);
        edits.set_interval_index(3);
        let draft = edits.draft();
        assert_eq!(draft.language.as_deref(), Some("qaa"));
        assert_eq!(draft.notifications.levels, [83]);
        assert_eq!(draft.refresh.interval, RefreshInterval::Minutes(4));
        // Another choice replaces them.
        edits.set_language_index(language_index(Some("sv")));
        edits.set_levels(5, 8);
        edits.set_interval_index(4);
        let draft = edits.draft();
        assert_eq!(draft.language.as_deref(), Some("sv"));
        assert_eq!(draft.notifications.levels, [80, 95]);
        assert_eq!(draft.refresh.interval, RefreshInterval::Minutes(10));
        // Back to "Same as Windows": the newer build's language again.
        edits.set_language_index(0);
        assert_eq!(edits.draft().language.as_deref(), Some("qaa"));
    }

    #[test]
    fn a_shortcut_is_pending_only_when_complete_and_valid() {
        let mut edits = Edits::new(Settings::default());
        let mut composing = ctrl_alt("");
        edits.shortcut_edited(composing.clone());
        assert!(!edits.is_pending(&Field::Hotkey));
        assert_eq!(edits.shortcut_problem(), Some(&ShortcutProblem::Invalid));
        assert!(edits.shortcut_dirty());
        composing.key = " l ".to_owned();
        edits.shortcut_edited(composing.clone());
        assert_eq!(edits.shortcut_problem(), None);
        assert_eq!(
            edits.draft().hotkey.map(|h| h.key),
            Some("L".to_owned()),
            "trimmed, upper case"
        );
        // No modifier: invalid again, nothing pending, the saved shortcut stays.
        composing.ctrl = false;
        composing.alt = false;
        edits.shortcut_edited(composing.clone());
        assert!(!edits.is_pending(&Field::Hotkey));
        assert_eq!(edits.draft().hotkey, None);
        assert_eq!(
            edits.shortcut(),
            composing,
            "the editor keeps the composition"
        );
        composing.key = "F13".to_owned();
        composing.shift = true;
        edits.shortcut_edited(composing.clone());
        assert!(edits.is_pending(&Field::Hotkey), "Shift with F1–F24");
        composing.key = "F25".to_owned();
        edits.shortcut_edited(composing.clone());
        assert!(!edits.is_pending(&Field::Hotkey));
        // Turning it off is complete.
        edits.shortcut_edited(Shortcut {
            enabled: false,
            ..composing
        });
        assert_eq!(edits.pending().last(), Some(&Edit::Hotkey(None)));
    }

    #[test]
    fn a_commit_writes_only_the_pending_fields_onto_the_file_as_it_is() {
        let base = Settings::default();
        let mut edits = Edits::new(base.clone());
        edits.set(Edit::HidePersonalInfo(true));
        edits.set(Edit::ReadoutEvery(true));
        // The tray paused and pinned a meter meanwhile, and the interval changed on disk: none of
        // those are the window's edits.
        let mut disk = base.clone();
        disk.pause = Some(Pause { until: None });
        disk.refresh.interval = RefreshInterval::Manual;
        let pin = meter(ProviderId::Claude, "default", MeterId::WEEKLY);
        disk.readout.content = ReadoutContent::Pinned(vec![pin.clone()]);
        let mut out = fake(&disk);
        let commit = edits.commit(&mut out);
        assert_eq!(
            commit.written,
            BTreeSet::from([Field::ReadoutEvery, Field::HidePersonalInfo])
        );
        assert!(out.disk.hide_personal_info && out.disk.readout.every_taskbar);
        assert_eq!(out.disk.pause, disk.pause);
        assert_eq!(out.disk.refresh.interval, RefreshInterval::Manual);
        assert_eq!(out.disk.readout.content, ReadoutContent::Pinned(vec![pin]));
        assert_eq!(out.notified, 1);
        // The file is the base now, and the draft shows what the tray changed.
        assert_eq!(edits.base(), &out.disk);
        assert_eq!(edits.draft().refresh.interval, RefreshInterval::Manual);
        assert!(!edits.has_pending());
        // Nothing pending: nothing is read or written.
        let mut idle = fake(&out.disk);
        idle.fail_write = true;
        assert_eq!(edits.commit(&mut idle), Commit::default());
    }

    #[test]
    fn an_alias_saved_by_its_pause_leaves_a_shortcut_being_composed_alone() {
        let base = with_work();
        let mut edits = Edits::new(base.clone());
        let work = key(ProviderId::Claude, "f-work");
        edits.alias_typed(work.clone(), "Work ");
        edits.shortcut_edited(ctrl_alt(""));
        // The alias editor's 800 ms pause: its edit is pending; the shortcut's is not.
        assert!(edits.alias_done(&work));
        let mut out = fake(&base);
        let commit = edits.commit(&mut out);
        assert_eq!(commit.written, BTreeSet::from([Field::Alias(work.clone())]));
        assert_eq!(
            edits.account(&work).and_then(|a| a.alias).as_deref(),
            Some("Work")
        );
        assert_eq!(out.disk.hotkey, None);
        assert!(out.tested.is_empty());
        assert!(edits.shortcut_dirty(), "the composition is still there");
        assert_eq!(edits.shortcut().key, "");
    }

    #[test]
    fn an_account_action_does_not_commit_an_alias_being_typed() {
        let mut base = with_work();
        if let Some(p) = base.provider_mut(ProviderId::Claude) {
            p.accounts.push(AccountSettings {
                key: AccountKey("f-old".to_owned()),
                source: SourceSettings::ClaudeConfigDir {
                    path: r"C:\Users\tester\.claude-old".into(),
                },
                ..AccountSettings::primary()
            });
        }
        let mut edits = Edits::new(base.clone());
        let work = key(ProviderId::Claude, "f-work");
        edits.alias_typed(work.clone(), "Wo");
        assert!(edits.remove_account(&key(ProviderId::Claude, "f-old")));
        let mut out = fake(&base);
        edits.commit(&mut out);
        let accounts = &out
            .disk
            .provider(ProviderId::Claude)
            .expect("claude")
            .accounts;
        assert_eq!(accounts.len(), 2);
        assert_eq!(
            accounts[1].alias, None,
            "the alias was not at its commit point"
        );
        assert!(edits.alias_dirty(&work), "the editor keeps its text");
    }

    #[test]
    fn a_refused_shortcut_is_tried_again_only_by_a_newer_edit_or_try_again() {
        let mut edits = Edits::new(Settings::default());
        edits.shortcut_edited(ctrl_alt("K"));
        let mut out = fake(&Settings::default());
        out.refuse = Some(HotkeyError::InUse);
        let commit = edits.commit(&mut out);
        assert_eq!(
            commit.shortcut,
            Some(ShortcutProblem::Refused(HotkeyError::InUse))
        );
        assert!(!commit.saved(), "nothing else was pending");
        assert_eq!(out.tested.len(), 1);
        assert_eq!(out.disk.hotkey, None);
        assert!(edits.shortcut_dirty());
        assert_eq!(edits.shortcut().key, "K");

        // An unrelated commit does not try it again.
        edits.set(Edit::NotifyResets(false));
        assert!(edits.commit(&mut out).saved());
        assert_eq!(out.tested.len(), 1);
        // Nor does closing.
        edits.flush_editors();
        assert!(!edits.has_pending());
        edits.commit(&mut out);
        assert_eq!(out.tested.len(), 1);
        // Try again does, and a newer edit does.
        edits.shortcut_retry();
        edits.commit(&mut out);
        assert_eq!(out.tested.len(), 2);
        out.refuse = None;
        edits.shortcut_edited(ctrl_alt("J"));
        let commit = edits.commit(&mut out);
        assert_eq!(commit.written, BTreeSet::from([Field::Hotkey]));
        assert_eq!(out.disk.hotkey.map(|h| h.key).as_deref(), Some("J"));
        assert!(!edits.shortcut_dirty());
        assert_eq!(edits.shortcut_problem(), None);
    }

    #[test]
    fn a_valid_shortcut_never_tried_is_saved_on_closing() {
        let mut edits = Edits::new(Settings::default());
        edits.shortcut_edited(ctrl_alt("K"));
        let mut out = fake(&Settings::default());
        out.refuse = Some(HotkeyError::InUse);
        edits.commit(&mut out);
        // Composed again into an invalid shortcut, then back to a valid one never tried.
        edits.shortcut_edited(ctrl_alt(""));
        let mut shown = ctrl_alt("M");
        shown.win = true;
        edits.shortcut.shown = shown;
        edits.shortcut.revision += 1;
        edits.shortcut.dirty = true;
        out.refuse = None;
        edits.flush_editors();
        assert!(edits.commit(&mut out).saved());
        assert_eq!(out.disk.hotkey.map(|h| h.key).as_deref(), Some("M"));
    }

    #[test]
    fn the_default_accounts_of_claude_and_codex_keep_their_own_aliases() {
        let base = Settings::default();
        let mut edits = Edits::new(base.clone());
        let claude = key(ProviderId::Claude, "default");
        let codex = key(ProviderId::Codex, "default");
        edits.alias_typed(claude.clone(), "Mine");
        edits.alias_typed(codex.clone(), "Work");
        assert!(edits.alias_done(&claude));
        assert!(edits.alias_done(&codex));
        let mut out = fake(&base);
        edits.commit(&mut out);
        let alias = |p: ProviderId| {
            out.disk
                .provider(p)
                .and_then(|p| p.accounts[0].alias.clone())
        };
        assert_eq!(alias(ProviderId::Claude).as_deref(), Some("Mine"));
        assert_eq!(alias(ProviderId::Codex).as_deref(), Some("Work"));
    }

    #[test]
    fn reconciling_shows_the_file_but_leaves_an_alias_being_typed() {
        let base = with_work();
        let mut edits = Edits::new(base.clone());
        let work = key(ProviderId::Claude, "f-work");
        edits.alias_typed(work.clone(), "Half-ty");
        edits.set(Edit::TimeStyle(TimeStyle::Relative));
        // The tray changed the interval and turned the readout off meanwhile.
        let mut disk = base.clone();
        disk.refresh.interval = RefreshInterval::Minutes(30);
        disk.readout.enabled = false;
        let mut out = fake(&disk);
        edits.commit(&mut out);
        let draft = edits.draft();
        assert_eq!(draft.refresh.interval, RefreshInterval::Minutes(30));
        assert!(!draft.readout.enabled);
        assert_eq!(interval_index(draft.refresh.interval), 6);
        assert!(edits.alias_dirty(&work), "protected until its own commit");
        assert_eq!(
            edits.aliases.get(&work).map(String::as_str),
            Some("Half-ty")
        );
        // Its own commit point: the user's newer edit wins.
        assert!(edits.alias_done(&work));
        edits.commit(&mut out);
        assert_eq!(
            out.disk
                .provider(ProviderId::Claude)
                .expect("claude")
                .accounts[1]
                .alias
                .as_deref(),
            Some("Half-ty")
        );
    }

    #[test]
    fn a_failed_write_keeps_the_edits_pending() {
        let base = Settings::default();
        let mut edits = Edits::new(base.clone());
        edits.set(Edit::NotifyEnabled(false));
        let mut out = fake(&base);
        out.fail_write = true;
        let commit = edits.commit(&mut out);
        assert_eq!(commit.error.as_deref(), Some("The disk is full."));
        assert!(!commit.saved());
        assert!(edits.is_pending(&Field::NotifyEnabled));
        assert_eq!(edits.base(), &base);
        assert!(
            !edits.draft().notifications.enabled,
            "still what the user chose"
        );
        assert_eq!(out.notified, 0);
        assert!(!out.locked, "the lock is released");
        out.fail_write = false;
        assert!(edits.commit(&mut out).saved());
        assert!(!out.disk.notifications.enabled);
    }

    #[test]
    fn start_with_windows_is_undone_when_the_file_cannot_be_written() {
        let base = Settings::default();
        let mut edits = Edits::new(base.clone());
        edits.set(Edit::StartWithWindows(true));
        let mut out = fake(&base);
        out.fail_write = true;
        let commit = edits.commit(&mut out);
        assert_eq!(out.autostart, [true, false], "set, then undone");
        assert_eq!(commit.startup, None);
        assert!(commit.error.is_some());
        assert!(edits.is_pending(&Field::StartWithWindows));

        // The undo fails too: the registry says on, and so does the switch.
        out.autostart.clear();
        out.autostart_fails = vec![false, true];
        let commit = edits.commit(&mut out);
        assert_eq!(out.autostart, [true, false]);
        assert_eq!(commit.startup, Some(StartupProblem::Stuck { on: true }));
        assert!(edits.draft().start_with_windows);

        // The registry refuses: the field drops out, the rest is written.
        let mut edits = Edits::new(base.clone());
        edits.set(Edit::StartWithWindows(true));
        edits.set(Edit::UpdatesCheck(false));
        let mut out = fake(&base);
        out.autostart_fails = vec![true];
        let commit = edits.commit(&mut out);
        assert_eq!(
            commit.startup,
            Some(StartupProblem::NotChanged("Access is denied.".to_owned()))
        );
        assert_eq!(commit.written, BTreeSet::from([Field::UpdatesCheck]));
        assert!(!out.disk.start_with_windows);
        assert!(
            !edits.draft().start_with_windows,
            "the switch shows the registry"
        );
        // Unchanged on disk: the registry is not touched.
        let mut edits = Edits::new(base.clone());
        edits.set(Edit::StartWithWindows(false));
        let mut out = fake(&base);
        edits.commit(&mut out);
        assert!(out.autostart.is_empty());
    }

    #[test]
    fn pins_choose_promote_and_follow_hidden_meters_and_removed_accounts() {
        let base = with_work();
        let mut edits = Edits::new(base.clone());
        let weekly = meter(ProviderId::Claude, "default", MeterId::WEEKLY);
        let work_weekly = meter(ProviderId::Claude, "f-work", MeterId::WEEKLY);
        let five = meter(ProviderId::Claude, "default", MeterId::FIVE_HOUR);
        // Choosing: "Meters I pin" without a pin changes nothing yet.
        edits.set_readout_mode(true);
        assert!(edits.choosing_pins());
        assert_eq!(edits.readout_mode_index(), 1);
        assert!(!edits.has_pending());
        edits.set_pinned(five.clone(), true);
        edits.set_pinned(work_weekly.clone(), true);
        assert!(!edits.choosing_pins());
        assert_eq!(pins(&edits.draft()), [five.clone(), work_weekly.clone()]);
        // Hiding the weekly limit unpins it for every account of the service.
        edits.set_pinned(weekly.clone(), true);
        assert_eq!(pins(&edits.draft()), [work_weekly.clone(), weekly.clone()]);
        edits.set_meter_shown(&weekly, false);
        assert_eq!(pins(&edits.draft()), Vec::<MeterKey>::new());
        assert!(edits.choosing_pins(), "both gone: choosing again");
        assert_eq!(edits.readout_mode_index(), 1);
        assert_eq!(edits.draft().readout.content, ReadoutContent::Auto);
        // Removing an account drops its pins, even from a pending pin edit.
        edits.set_meter_shown(&weekly, true);
        edits.set_pinned(work_weekly.clone(), true);
        edits.set_pinned(five.clone(), true);
        assert!(edits.remove_account(&key(ProviderId::Claude, "f-work")));
        assert_eq!(pins(&edits.draft()), std::slice::from_ref(&five));
        let mut out = fake(&base);
        edits.commit(&mut out);
        assert_eq!(out.disk.readout.content, ReadoutContent::Pinned(vec![five]));
        assert!(
            !edits.remove_account(&key(ProviderId::Claude, "default")),
            "the last one"
        );
        // Back to auto.
        edits.set_readout_mode(false);
        assert_eq!(edits.readout_mode_index(), 0);
        assert!(!edits.choosing_pins());
    }

    #[test]
    fn an_added_account_is_added_once() {
        let base = Settings::default();
        let account = AccountSettings {
            key: AccountKey("f-new".to_owned()),
            alias: Some("New".to_owned()),
            source: SourceSettings::CodexHome {
                path: r"C:\Users\tester\.codex-new".into(),
            },
            ..AccountSettings::primary()
        };
        let mut edits = Edits::new(base.clone());
        edits.set(Edit::AccountAdded(
            ProviderId::Codex,
            Box::new(account.clone()),
            r"c:\users\tester\.codex-new".to_owned(),
        ));
        // Another writer added the same folder meanwhile.
        let mut disk = base.clone();
        if let Some(p) = disk.provider_mut(ProviderId::Codex) {
            p.accounts.push(account.clone());
        }
        let mut out = fake(&disk);
        edits.commit(&mut out);
        assert_eq!(
            out.disk
                .provider(ProviderId::Codex)
                .expect("codex")
                .accounts,
            [AccountSettings::primary(), account]
        );
    }
}
