//! The settings schema (ARCHITECTURE "Settings"): versioned, `serde`, defaults for every field,
//! unknown fields kept.
//!
//! Stored by the app in `%APPDATA%\Lastdrop\settings.json` (atomic write). **No secrets**: the
//! Meshy API key lives in Credential Manager (`Lastdrop:meshy:<account>:api_key`), Claude and
//! Codex tokens stay in those tools' own files.
//!
//! Forward compatibility: every struct keeps unknown fields in an `extra` map
//! (`#[serde(flatten)]`), and a provider entry whose id this build does not know is kept as raw
//! JSON ([`ProviderEntry::Unknown`]), so a settings file written by a newer Lastdrop survives a
//! round trip through an older one.

use std::collections::BTreeMap;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::consts;
use crate::grammar::TimeStyle;
use crate::model::{AccountKey, MeterId, MeterKey, ProviderId};

/// Current schema version. Bump it when a field changes meaning; `persist::load` in the app
/// migrates older files.
pub const SETTINGS_VERSION: u32 = 1;

/// Unknown JSON fields, kept verbatim.
pub type Extra = BTreeMap<String, serde_json::Value>;

/// All user settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version, [`SETTINGS_VERSION`] when written by this build.
    pub version: u32,
    /// Services in the user's order (F§2.2 "Cards in the user's service order"; R§3.10 tie
    /// break "service order").
    pub providers: Vec<ProviderEntry>,
    /// Source-specific options per provider.
    pub provider_options: ProviderOptions,
    /// Refresh interval and adaptivity (ROADMAP "Settings", ARCHITECTURE "Refresh scheduler").
    pub refresh: RefreshSettings,
    /// The taskbar readout (R§4.3, R§5.2, R§10 context menu).
    pub readout: ReadoutSettings,
    /// Updates paused by the user (R§10 "Pause updates ▸ for 1 hour / until I resume", R§3.8
    /// PAUSED). `None` = not paused.
    pub pause: Option<Pause>,
    /// Start with Windows (`HKCU\…\Run\Lastdrop`, ARCHITECTURE "Autostart"). Default off
    /// (judgement call: nothing is written to the Run key before the user asks).
    pub start_with_windows: bool,
    /// Follow the Windows light/dark mode and accent (the only M1 theme; Theme Studio is M4).
    pub theme_follows_system: bool,
    /// The language of every string (ROADMAP M4 "Language"): a BCP 47 tag (`en`, `sv`), or
    /// `None` to follow Windows' display language (the first preferred display language with a
    /// table, else English: [`crate::grammar::Language::resolve`]). A tag without a table (a
    /// newer build's language) is kept as it is and follows Windows too. Numbers and times
    /// always follow the regional format.
    pub language: Option<String>,
    /// The global hotkey that toggles the flyout (R§10 "Global hotkey: optional, off by
    /// default"). `None` = off.
    pub hotkey: Option<Hotkey>,
    /// Windows notifications at levels, when used up and on resets (N§2).
    pub notifications: NotificationSettings,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

/// What Lastdrop notifies about (N§2). Every notice is on by default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationSettings {
    /// Notifications at all.
    pub enabled: bool,
    /// The levels a window meter's `pct` is announced at (N§1 LEVEL), 1–99; read through
    /// [`NotificationSettings::levels`], which drops anything else.
    pub levels: Vec<u8>,
    /// A window meter is used up (N§1 USED_UP), and a balance is empty (BALANCE_EMPTY).
    pub used_up: bool,
    /// A window resets after a level or used up was announced in it (N§1 RESET).
    pub resets: bool,
    /// A balance runs low: WARNING or CRITICAL (N§1 BALANCE_LOW).
    pub low_balance: bool,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for NotificationSettings {
    fn default() -> NotificationSettings {
        NotificationSettings {
            enabled: true,
            levels: consts::NOTIFY_DEFAULT_LEVELS.to_vec(),
            used_up: true,
            resets: true,
            low_balance: true,
            extra: Extra::new(),
        }
    }
}

impl NotificationSettings {
    /// The levels in use: 1–99 only, ascending, each once.
    pub fn levels(&self) -> Vec<u8> {
        let mut levels: Vec<u8> = self
            .levels
            .iter()
            .copied()
            .filter(|l| (1..=99).contains(l))
            .collect();
        levels.sort_unstable();
        levels.dedup();
        levels
    }
}

/// A global hotkey (R§10), registered with `RegisterHotKey` by `lastdrop_win::hotkey`. Opening
/// the flyout with it counts as a keyboard open (focus on the first card, F§9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    /// Ctrl held.
    #[serde(default)]
    pub ctrl: bool,
    /// Alt held.
    #[serde(default)]
    pub alt: bool,
    /// Shift held.
    #[serde(default)]
    pub shift: bool,
    /// The Windows key held.
    #[serde(default)]
    pub win: bool,
    /// The key: one letter or digit (`L`, `7`) or `F1`…`F24`.
    pub key: String,
}

impl Default for Settings {
    /// First-run settings: Claude and Codex enabled (they read local files and need no secret),
    /// Copilot present but disabled until the user turns it on (it needs the GitHub CLI signed
    /// in), Meshy present but disabled until the user stores a key; refresh every 5 minutes;
    /// readout on, auto rows, clock times (R§4.3, R§5.2); every notification on (N§2); the
    /// language of Windows.
    fn default() -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            providers: ProviderId::ALL
                .into_iter()
                .map(|id| {
                    let on = matches!(id, ProviderId::Claude | ProviderId::Codex);
                    ProviderEntry::Known(ProviderSettings::new(id, on))
                })
                .collect(),
            provider_options: ProviderOptions::default(),
            refresh: RefreshSettings::default(),
            readout: ReadoutSettings::default(),
            pause: None,
            start_with_windows: false,
            theme_follows_system: true,
            language: None,
            hotkey: None,
            notifications: NotificationSettings::default(),
            extra: Extra::new(),
        }
    }
}

impl Settings {
    /// The settings of a known provider, if it has an entry.
    pub fn provider(&self, id: ProviderId) -> Option<&ProviderSettings> {
        self.providers
            .iter()
            .find_map(|e| e.known().filter(|p| p.id == id))
    }

    /// Mutable access to a known provider's settings.
    pub fn provider_mut(&mut self, id: ProviderId) -> Option<&mut ProviderSettings> {
        self.providers.iter_mut().find_map(|e| match e {
            ProviderEntry::Known(p) if p.id == id => Some(p),
            _ => None,
        })
    }

    /// Adds an entry, turned off, for every provider this build knows that the settings lack (a
    /// service added since the file was written), after the existing ones, so it shows in the
    /// settings window without starting to read by itself. Returns whether it added any.
    ///
    /// An entry this build cannot read but whose `id` it knows ([`ProviderEntry::id`]: a newer
    /// build's option, a malformed field) counts as present: a second entry for the same service
    /// would be turned off, and a newer build reading the file back would find two.
    pub fn add_new_providers(&mut self) -> bool {
        let missing: Vec<ProviderId> = ProviderId::ALL
            .into_iter()
            .filter(|id| !self.providers.iter().any(|e| e.id() == Some(*id)))
            .collect();
        for id in &missing {
            self.providers
                .push(ProviderEntry::Known(ProviderSettings::new(*id, false)));
        }
        !missing.is_empty()
    }

    /// Enabled known providers in service order; the index is the service order used for
    /// ranking ties (R§3.10) and card order (F§2.2).
    pub fn enabled_providers(&self) -> impl Iterator<Item = (usize, &ProviderSettings)> {
        self.providers
            .iter()
            .filter_map(ProviderEntry::known)
            .filter(|p| p.enabled)
            .enumerate()
    }
}

/// One entry of [`Settings::providers`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProviderEntry {
    /// A provider this build knows.
    Known(ProviderSettings),
    /// An entry this build cannot read (a newer build's provider, or a malformed entry): kept
    /// verbatim and written back unchanged, never shown.
    Unknown(serde_json::Value),
}

impl ProviderEntry {
    /// The service of the entry: a known entry's, or an unknown entry's `"id"` when this build
    /// knows it (the entry itself could not be read); `None` for a service this build does not
    /// know.
    pub fn id(&self) -> Option<ProviderId> {
        match self {
            ProviderEntry::Known(p) => Some(p.id),
            ProviderEntry::Unknown(raw) => raw
                .get("id")
                .and_then(serde_json::Value::as_str)
                .and_then(ProviderId::from_id),
        }
    }

    /// The settings when the entry is known.
    pub fn known(&self) -> Option<&ProviderSettings> {
        match self {
            ProviderEntry::Known(p) => Some(p),
            ProviderEntry::Unknown(_) => None,
        }
    }
}

/// Settings of one service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderSettings {
    /// Which service.
    pub id: ProviderId,
    /// Polled and shown.
    #[serde(default)]
    pub enabled: bool,
    /// Override of the global refresh interval for this service (ROADMAP "Refresh interval per
    /// provider"); `None` = the global one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<RefreshInterval>,
    /// Meters not shown anywhere (F§2.2 "Hidden meters (settings) are not shown").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_meters: Vec<MeterId>,
    /// User thresholds for balances that raise WARNING/CRITICAL earlier, never later (R§3.7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance_alert: Option<BalanceAlert>,
    /// Accounts of this service. M1 has one, keyed [`AccountKey::primary`].
    #[serde(default)]
    pub accounts: Vec<AccountSettings>,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

impl ProviderSettings {
    /// A provider entry with one default account.
    pub fn new(id: ProviderId, enabled: bool) -> ProviderSettings {
        ProviderSettings {
            id,
            enabled,
            interval: None,
            hidden_meters: Vec::new(),
            balance_alert: None,
            accounts: vec![AccountSettings::primary()],
            extra: Extra::new(),
        }
    }
}

/// One account of a service (ROADMAP "Multiple accounts"). Holds folder paths and aliases,
/// never tokens.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountSettings {
    /// Stable key ([`AccountKey`]).
    pub key: AccountKey,
    /// Alias shown as `Claude · Work` (M2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// 1–2 character readout tag (R§6.4, M2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Where the account's data and credentials are.
    #[serde(default)]
    pub source: SourceSettings,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

impl AccountSettings {
    /// The M1 account: default source location, no alias.
    pub fn primary() -> AccountSettings {
        AccountSettings {
            key: AccountKey::primary(),
            alias: None,
            tag: None,
            source: SourceSettings::Default,
            extra: Extra::new(),
        }
    }
}

/// Where an account's data comes from. `lastdrop-providers` resolves it into an
/// `AccountSource` (config dir / codex home / api-key name).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SourceSettings {
    /// The service's default location: `CLAUDE_CONFIG_DIR` or `~\.claude`; `CODEX_HOME` or
    /// `~\.codex`; the Meshy key named `api_key`.
    #[default]
    Default,
    /// A Claude Code config folder (`CLAUDE_CONFIG_DIR`).
    ClaudeConfigDir {
        /// The folder.
        path: PathBuf,
    },
    /// A Codex home folder (`CODEX_HOME`).
    CodexHome {
        /// The folder.
        path: PathBuf,
    },
    /// The name of an API key in Credential Manager: the `<name>` part of
    /// `Lastdrop:<provider>:<account>:<name>` (ARCHITECTURE "Secrets").
    ApiKey {
        /// The credential name.
        name: String,
    },
}

/// Options that only some providers have.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderOptions {
    /// Claude.
    pub claude: ClaudeOptions,
    /// Codex.
    pub codex: CodexOptions,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Claude's sources can be turned off one by one (ARCHITECTURE provider table).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeOptions {
    /// Read Claude Desktop's `plan-usage-history.json` (no token).
    pub desktop_history: bool,
    /// Read Claude Desktop's last usage response from its HTTP cache (no token).
    pub desktop_cache: bool,
    /// Call the OAuth usage endpoint with Claude Code's token (read-only, ≥ 5 min apart).
    pub oauth_usage: bool,
    /// Fall back to Claude Code's cached usage in `.claude.json`.
    pub cached_usage: bool,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for ClaudeOptions {
    fn default() -> ClaudeOptions {
        ClaudeOptions {
            desktop_history: true,
            desktop_cache: true,
            oauth_usage: true,
            cached_usage: true,
            extra: Extra::new(),
        }
    }
}

/// Codex options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodexOptions {
    /// A specific `codex.exe`; `None` = newest bundled or the one on `PATH` (ARCHITECTURE).
    pub codex_exe: Option<PathBuf>,
    /// Fall back to the newest rollout file when the app-server fails.
    pub rollout_fallback: bool,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for CodexOptions {
    fn default() -> CodexOptions {
        CodexOptions {
            codex_exe: None,
            rollout_fallback: true,
            extra: Extra::new(),
        }
    }
}

/// How often a service is polled (ROADMAP "Settings": manual or 1–30 min).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshInterval {
    /// Only on "Refresh now" (R§3.8: stale after 60 min).
    Manual,
    /// Every n minutes, within [`consts::REFRESH_MINUTES_RANGE`]; never below the provider's
    /// `min_interval`.
    Minutes(u16),
}

/// Global refresh settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RefreshSettings {
    /// The interval for every service without its own.
    pub interval: RefreshInterval,
    /// Faster while the flyout is open, slower when the user is idle (ARCHITECTURE "Refresh
    /// scheduler"; ROADMAP lists it under M4, so it defaults off).
    pub adaptive: bool,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for RefreshSettings {
    fn default() -> RefreshSettings {
        RefreshSettings {
            interval: RefreshInterval::Minutes(consts::DEFAULT_REFRESH_MINUTES),
            adaptive: false,
            extra: Extra::new(),
        }
    }
}

/// The taskbar readout settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadoutSettings {
    /// Shown at all (R§4.2 HIDDEN "readout turned off"; context menu "Taskbar readout ▸ Off").
    pub enabled: bool,
    /// Which meters the rows show (R§4.3).
    pub content: ReadoutContent,
    /// Time points as clock times or as time remaining (R§5.2 "Show times as").
    pub time_style: TimeStyle,
    /// Unknown fields.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for ReadoutSettings {
    fn default() -> ReadoutSettings {
        ReadoutSettings {
            enabled: true,
            content: ReadoutContent::Auto,
            time_style: TimeStyle::Clock,
            extra: Extra::new(),
        }
    }
}

/// Readout content mode (R§4.3).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", content = "meters")]
pub enum ReadoutContent {
    /// The most constrained meters (R§3.10). Default on first run.
    #[default]
    Auto,
    /// 1–2 meters the user picked, in the user's order.
    Pinned(Vec<MeterKey>),
}

/// User thresholds for a balance (R§3.7 "A user threshold for balances may raise
/// WARNING/CRITICAL earlier; it never lowers them").
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalanceAlert {
    /// WARNING when `days_left` is below this (used only when above `BAL_WARN_DAYS`).
    pub warn_days: Option<f64>,
    /// CRITICAL when `days_left` is below this (used only when above `BAL_CRIT_DAYS`).
    pub critical_days: Option<f64>,
}

/// Paused updates (R§10, R§3.8 PAUSED).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    /// When the pause ends by itself ("for 1 hour"); `None` = until the user resumes.
    pub until: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let s = Settings::default();
        let json = serde_json::to_string(&s).expect("serialize");
        let back: Settings = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(s, back);
    }

    #[test]
    fn new_providers_are_added_turned_off() {
        let mut s = Settings {
            providers: vec![
                ProviderEntry::Known(ProviderSettings::new(ProviderId::Meshy, true)),
                ProviderEntry::Known(ProviderSettings::new(ProviderId::Claude, true)),
            ],
            ..Settings::default()
        };
        assert!(s.add_new_providers());
        let ids: Vec<(ProviderId, bool)> = s
            .providers
            .iter()
            .filter_map(ProviderEntry::known)
            .map(|p| (p.id, p.enabled))
            .collect();
        assert_eq!(
            ids,
            vec![
                (ProviderId::Meshy, true),
                (ProviderId::Claude, true),
                (ProviderId::Codex, false),
                (ProviderId::Copilot, false),
            ]
        );
        assert!(!s.add_new_providers());
        assert!(!Settings::default().clone().add_new_providers());
    }

    #[test]
    fn an_unreadable_entry_of_a_known_service_is_not_added_again() {
        let json = r#"{
            "version": 1,
            "providers": [
                { "id": "claude", "enabled": true, "interval": { "every_hour": 2 } },
                { "id": "codex", "enabled": "yes" },
                { "id": "someday", "enabled": true },
                { "enabled": true }
            ]
        }"#;
        let mut s: Settings = serde_json::from_str(json).expect("deserialize");
        assert!(
            s.providers
                .iter()
                .all(|e| matches!(e, ProviderEntry::Unknown(_)))
        );
        let ids: Vec<Option<ProviderId>> = s.providers.iter().map(ProviderEntry::id).collect();
        assert_eq!(
            ids,
            vec![
                Some(ProviderId::Claude),
                Some(ProviderId::Codex),
                None,
                None
            ]
        );
        assert!(s.add_new_providers());
        let added: Vec<ProviderId> = s.providers[4..]
            .iter()
            .filter_map(ProviderEntry::known)
            .map(|p| p.id)
            .collect();
        assert_eq!(
            added,
            vec![ProviderId::Copilot, ProviderId::Meshy],
            "no second Claude or Codex entry"
        );
        let out = serde_json::to_value(&s).expect("serialize");
        assert_eq!(
            out["providers"][0]["interval"]["every_hour"], 2,
            "kept verbatim"
        );
    }

    #[test]
    fn the_language_follows_windows_until_one_is_picked() {
        use crate::grammar::Language;
        let windows = ["sv-SE".to_owned()];
        let s = Settings::default();
        assert_eq!(s.language, None);
        assert_eq!(
            Language::resolve(s.language.as_deref(), &windows),
            Language::Swedish
        );
        // A newer build's language survives a round trip and follows Windows meanwhile.
        let s: Settings =
            serde_json::from_str(r#"{ "version": 1, "language": "de" }"#).expect("deserialize");
        assert_eq!(
            Language::resolve(s.language.as_deref(), &windows),
            Language::Swedish
        );
        let out = serde_json::to_value(&s).expect("serialize");
        assert_eq!(out["language"], "de");
        let s: Settings = serde_json::from_str(r#"{ "language": "en" }"#).expect("deserialize");
        assert_eq!(
            Language::resolve(s.language.as_deref(), &windows),
            Language::English
        );
    }

    #[test]
    fn copilot_starts_off() {
        let s = Settings::default();
        assert!(!s.provider(ProviderId::Copilot).expect("copilot").enabled);
        assert!(s.provider(ProviderId::Codex).expect("codex").enabled);
    }

    #[test]
    fn unknown_fields_and_unknown_providers_survive_a_round_trip() {
        let json = r#"{
            "version": 1,
            "future_field": { "a": 1 },
            "providers": [
                { "id": "claude", "enabled": true, "new_knob": 3 },
                { "id": "someday", "enabled": true }
            ],
            "readout": { "enabled": false, "density": "tight" }
        }"#;
        let s: Settings = serde_json::from_str(json).expect("deserialize");
        assert!(matches!(s.providers[1], ProviderEntry::Unknown(_)));
        let claude = s.provider(ProviderId::Claude).expect("claude");
        assert_eq!(claude.extra["new_knob"], 3);
        assert!(!s.readout.enabled);
        let out = serde_json::to_value(&s).expect("serialize");
        assert_eq!(out["future_field"]["a"], 1);
        assert_eq!(out["providers"][1]["id"], "someday");
        assert_eq!(out["readout"]["density"], "tight");
    }
}
