//! The data model: what a provider returns and how meters are identified (ARCHITECTURE
//! "Data model", R§2 vocabulary, R§3.1 inputs).
//!
//! Shares are `f64` where `1.0` is the whole limit (R§0). A value the service did not report is
//! `None`, never `0` (ARCHITECTURE "Rules for every provider").
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::glyph::ProviderGlyph;

/// A service Lastdrop reads (R§2 "Service"). M2 adds Copilot, Cursor, Antigravity, … .
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// Claude plans (Claude Code / Claude Desktop): 5-hour, weekly and per-model weekly windows.
    Claude,
    /// OpenAI Codex: 5-hour and weekly windows.
    Codex,
    /// Meshy: a credit balance.
    Meshy,
}

impl ProviderId {
    /// Every provider, in the default service order (settings default, F§2.2).
    pub const ALL: [ProviderId; 3] = [ProviderId::Claude, ProviderId::Codex, ProviderId::Meshy];

    /// Stable lower-case id used in settings, history file names and credential names
    /// (`claude`, `codex`, `meshy`). Never changes once shipped.
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
            ProviderId::Meshy => "meshy",
        }
    }

    /// Parses [`ProviderId::as_str`] back; `None` for an id this build does not know.
    pub fn from_id(id: &str) -> Option<ProviderId> {
        ProviderId::ALL.into_iter().find(|p| p.as_str() == id)
    }

    /// The service name shown to people: Appendix B's `{service}` before an account alias is
    /// added (R§6.4: `Claude · Work` in M2).
    pub const fn display_name(self) -> &'static str {
        match self {
            ProviderId::Claude => "Claude",
            ProviderId::Codex => "Codex",
            ProviderId::Meshy => "Meshy",
        }
    }

    /// The service glyph (R§6.7, R Appendix A). Every M1 provider has a drawn glyph.
    pub const fn glyph(self) -> ProviderGlyph {
        ProviderGlyph::Drawn(self)
    }

    /// The vendor's own client that renews a sign-in, for Appendix B `card.sentence.signin`
    /// (`open {client} in this account`). `None` for services signed in with an API key that the
    /// user enters in Lastdrop's settings (Meshy): their cards show the API-key message instead
    /// (`flyout::MessageKind::ApiKey`).
    pub const fn client_name(self) -> Option<&'static str> {
        match self {
            ProviderId::Claude => Some("Claude Code"),
            ProviderId::Codex => Some("Codex"),
            ProviderId::Meshy => None,
        }
    }
}

/// Identifies one account of one service. In M1 every service has exactly one account,
/// [`AccountKey::primary`]; M2 keys accounts by who is signed in (ROADMAP "Multiple accounts").
///
/// Privacy: the key appears in history file names, settings and logs, so it is never an
/// e-mail address, account id or token; M2 derives it from an alias or a hash.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountKey(pub String);

impl AccountKey {
    /// The key of the single account per service in M1: `"default"`.
    pub fn primary() -> AccountKey {
        AccountKey("default".to_owned())
    }
}

/// The account a snapshot belongs to, as shown to people (ARCHITECTURE: "account (label,
/// plan)"; R§6.4; F§4.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRef {
    /// Stable key, see [`AccountKey`].
    pub key: AccountKey,
    /// User alias from settings (`Work`); `None` in M1. Shown as `Claude · Work` (R§6.4).
    pub alias: Option<String>,
    /// 1–2 character tag for readout labels when a service has several accounts (`W 5h`,
    /// R§6.4). `None` in M1.
    pub tag: Option<String>,
    /// Plan as the service names it (`Max`, `Pro`), for the plan badge (F§4.2) and the hover
    /// card's service row (R§8.2). `None` when unknown: the badge is omitted.
    pub plan: Option<String>,
}

impl AccountRef {
    /// The M1 account: [`AccountKey::primary`], no alias, no tag, no plan.
    pub fn primary() -> AccountRef {
        AccountRef {
            key: AccountKey::primary(),
            alias: None,
            tag: None,
            plan: None,
        }
    }
}

/// Stable id of a meter within a service: `five_hour`, `weekly`, `weekly:fable`, `credits`
/// (ARCHITECTURE "Meter"). Used in settings (pins, hidden meters) and history file names.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeterId(pub String);

impl MeterId {
    /// The 5-hour session window (Claude, Codex).
    pub const FIVE_HOUR: &'static str = "five_hour";
    /// The weekly all-models window (Claude, Codex).
    pub const WEEKLY: &'static str = "weekly";
    /// Meshy's credit balance.
    pub const CREDITS: &'static str = "credits";

    /// Builds an id from a static name such as [`MeterId::FIVE_HOUR`].
    pub fn new(id: &str) -> MeterId {
        MeterId(id.to_owned())
    }

    /// The per-model weekly window id `weekly:<model>` (`weekly:fable`); `model` is lower-cased
    /// by the provider before calling this.
    pub fn weekly_model(model: &str) -> MeterId {
        MeterId(format!("weekly:{model}"))
    }
}

/// One credential source: one account of one service. The refresh scheduler runs at most one
/// job per source (ARCHITECTURE "Threads"), and freshness is per source (R§3.8).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceKey {
    /// The service.
    pub provider: ProviderId,
    /// The account within the service.
    pub account: AccountKey,
}

/// Globally unique meter key: service + account + meter. Keys severity memory, history and
/// readout pins.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MeterKey {
    /// The service.
    pub provider: ProviderId,
    /// The account within the service.
    pub account: AccountKey,
    /// The meter within the account.
    pub meter: MeterId,
}

impl MeterKey {
    /// The credential source this meter is read from.
    pub fn source(&self) -> SourceKey {
        SourceKey {
            provider: self.provider,
            account: self.account.clone(),
        }
    }
}

/// The three names of a meter (R§2 table).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterNames {
    /// ≤ 5 characters, readout label column (`5h`, `Wk`, `Fable`, `Cr`).
    pub short_label: String,
    /// In running text after the service name or "limit" (`5-hour`, `weekly`, `Fable weekly`,
    /// `credits`): card titles, "Also tracked", tray tooltip, flyout verdict.
    pub short_name: String,
    /// Meter blocks in the hover card and flyout (`5-hour session`, `Weekly · all models`).
    pub name: String,
}

impl MeterNames {
    /// Names from static strings (a [`MeterDef`]).
    pub fn new(short_label: &str, short_name: &str, name: &str) -> MeterNames {
        MeterNames {
            short_label: short_label.to_owned(),
            short_name: short_name.to_owned(),
            name: name.to_owned(),
        }
    }

    /// The names of a Claude per-model weekly window (R§2 table): `Fable` / `Fable weekly` /
    /// `Weekly · Fable`, from the model's display name as the service gives it.
    pub fn weekly_model(model: &str) -> MeterNames {
        MeterNames {
            short_label: model.to_owned(),
            short_name: format!("{model} weekly"),
            name: format!("Weekly · {model}"),
        }
    }
}

/// The kind of a meter definition (R§2 "Two kinds").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterKind {
    /// A window meter with its `L` and `starts_on_use`.
    Window {
        /// `L`.
        window: WindowLen,
        /// The next window starts only with the next use.
        starts_on_use: bool,
    },
    /// A balance meter.
    Balance,
}

/// A meter definition from R§2's table, known **before** the first reading. It gives LOADING
/// rows their labels (R§4.1, R§5.7 `Reading…`), and the readout's width inputs come from the
/// definitions of every meter that *can* appear (R§6.3: "computed, never from the current data").
/// Providers build their [`Meter`]s and [`Balance`]s with these names so the two cannot drift.
///
/// Meters that exist only in some readings (Claude's per-model weekly windows) are not in the
/// catalogue: their names come from [`MeterNames::weekly_model`] and they join the width inputs
/// when a reading first reports them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeterDef {
    /// [`MeterId`] as a static string.
    pub id: &'static str,
    /// R§2 `short_label`.
    pub short_label: &'static str,
    /// R§2 `short_name`.
    pub short_name: &'static str,
    /// R§2 `name`.
    pub name: &'static str,
    /// Order within the service.
    pub order: u16,
    /// Window or balance.
    pub kind: MeterKind,
}

impl MeterDef {
    /// The meter id.
    pub fn meter_id(&self) -> MeterId {
        MeterId::new(self.id)
    }

    /// The names.
    pub fn names(&self) -> MeterNames {
        MeterNames::new(self.short_label, self.short_name, self.name)
    }
}

const FIVE_HOURS: WindowLen = WindowLen::Fixed(SignedDuration::from_hours(5));
const SEVEN_DAYS: WindowLen = WindowLen::Fixed(SignedDuration::from_hours(7 * 24));

const CLAUDE_METERS: [MeterDef; 2] = [
    MeterDef {
        id: MeterId::FIVE_HOUR,
        short_label: "5h",
        short_name: "5-hour",
        name: "5-hour session",
        order: 0,
        kind: MeterKind::Window {
            window: FIVE_HOURS,
            starts_on_use: true,
        },
    },
    MeterDef {
        id: MeterId::WEEKLY,
        short_label: "Wk",
        short_name: "weekly",
        name: "Weekly · all models",
        order: 1,
        kind: MeterKind::Window {
            window: SEVEN_DAYS,
            starts_on_use: false,
        },
    },
];

const CODEX_METERS: [MeterDef; 2] = [
    MeterDef {
        id: MeterId::FIVE_HOUR,
        short_label: "5h",
        short_name: "5-hour",
        name: "5-hour session",
        order: 0,
        kind: MeterKind::Window {
            window: FIVE_HOURS,
            starts_on_use: true,
        },
    },
    MeterDef {
        id: MeterId::WEEKLY,
        short_label: "Wk",
        short_name: "weekly",
        name: "Weekly",
        order: 1,
        kind: MeterKind::Window {
            window: SEVEN_DAYS,
            starts_on_use: false,
        },
    },
];

const MESHY_METERS: [MeterDef; 1] = [MeterDef {
    id: MeterId::CREDITS,
    short_label: "Cr",
    short_name: "credits",
    name: "Credits",
    order: 0,
    kind: MeterKind::Balance,
}];

/// The R§2 definitions of a service's always-present meters, in meter order (English names;
/// R§5.9 localizes them with the other strings when a second language arrives). Claude's
/// per-model weekly windows come from readings (order 2, 3, … in the service's order). A Codex
/// plan with other window lengths reports meters outside the catalogue (`codex::meter`).
pub const fn catalog(provider: ProviderId) -> &'static [MeterDef] {
    match provider {
        ProviderId::Claude => &CLAUDE_METERS,
        ProviderId::Codex => &CODEX_METERS,
        ProviderId::Meshy => &MESHY_METERS,
    }
}

/// The window length `L` of a window meter (R§2, R§3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowLen {
    /// A fixed length: 5 h, 7 d.
    Fixed(SignedDuration),
    /// A calendar month ending at `resets_at` (Copilot, M2): `L` is that month's actual length
    /// (R§2). Month boundaries are taken in UTC (judgement call: the services reset on UTC
    /// month starts).
    CalendarMonth,
    /// The provider reports only `resets_at` (declarative providers): no tick, no pace,
    /// `S = 1 h` (R§3.2, R§3.5).
    Unknown,
}

impl WindowLen {
    /// `L` for the window instance that ends at `resets_at`; `None` when unknown (R§3.1).
    pub fn length_ending_at(self, resets_at: Timestamp) -> Option<SignedDuration> {
        todo!("M1: R§2 — Fixed → the length; CalendarMonth → resets_at − start of that month (UTC)")
    }

    /// The reset after `resets_at` for a fixed-schedule window: `R + L`, or the next month start
    /// (R§3.9). `None` when `L` is unknown.
    pub fn next_reset(self, resets_at: Timestamp) -> Option<Timestamp> {
        todo!("M1: R§3.9 — R' = R + L (calendar month: next month start)")
    }
}

/// A window meter reading (R§2 "Window meter", R§3.1 inputs; ARCHITECTURE "Meter").
///
/// Providers omit a meter whose used share is missing (a missing value is never 0).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meter {
    /// Stable id.
    pub id: MeterId,
    /// Labels for the surfaces (R§2 table).
    pub names: MeterNames,
    /// Order within the service (R§2 "its order within the service"; F§2.2 provider order:
    /// Claude 5-hour, weekly all models, per-model weekly).
    pub order: u16,
    /// `u`: share used as reported (`utilization / 100`). May exceed 1; clamped at 0 by the
    /// provider (R§3.1).
    pub used: f64,
    /// `limited`: the provider says the limit is reached (R§3.1, R§3.3). `false` when the
    /// service has no such flag.
    pub limited: bool,
    /// `R`: when the window resets, UTC. `None` when the provider reports none (R§3.1, R§3.2).
    pub resets_at: Option<Timestamp>,
    /// `L` (R§3.1).
    pub window: WindowLen,
    /// The next window starts only with the next use (Claude's 5-hour session), so between
    /// windows there is no reset time (R§2, R§3.2, R§3.9).
    pub starts_on_use: bool,
}

/// A balance meter reading: an amount that only goes down until topped up (R§2 "Balance
/// meter", R§3.6; ARCHITECTURE "Balance").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Balance {
    /// Stable id (`credits`).
    pub id: MeterId,
    /// Labels for the surfaces (`Cr` / `credits` / `Credits`).
    pub names: MeterNames,
    /// Order within the service.
    pub order: u16,
    /// Current balance in [`Balance::unit`].
    pub amount: f64,
    /// What the amount counts.
    pub unit: Unit,
    /// The most recent top-up when the service reports it. `None` for Meshy, whose top-ups are
    /// detected from history instead (R§3.6 `ref`, [`crate::stats::balance_reference`]).
    pub last_top_up: Option<TopUp>,
}

/// A top-up as reported by a service (R§3.6 "balance right after the most recent top-up").
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopUp {
    /// When the top-up happened.
    pub at: Timestamp,
    /// The balance right after it: R§3.6's `ref`.
    pub balance_after: f64,
}

/// The unit of a balance (R§2, F§4.5 "provider unit").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Unit {
    /// Service credits (Meshy). Shown with the strings' `credits` word.
    Credits,
    /// Money, ISO 4217 code (`USD`); declarative balance providers (M2).
    Currency {
        /// ISO 4217 code.
        code: String,
    },
}

/// Which source produced a snapshot (ARCHITECTURE "Data model", provider table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Claude Desktop's `plan-usage-history.json` (no token needed).
    ClaudeDesktopHistory,
    /// The OAuth usage endpoint with Claude Code's token (read-only, never refreshed).
    ClaudeOAuthUsage,
    /// Claude Code's cached usage in `.claude.json`.
    ClaudeCodeCache,
    /// `codex app-server`, `account/rateLimits/read`.
    CodexAppServer,
    /// The newest Codex rollout file's `token_count.rate_limits`.
    CodexRollout,
    /// `GET https://api.meshy.ai/openapi/v1/balance`.
    MeshyApi,
}

/// What the provider says about the reading it returns (ARCHITECTURE "Snapshot"). The
/// service-level freshness shown to people is computed separately ([`crate::stats::freshness`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    /// A current reading from a live source.
    Ok,
    /// The reading came from a cached or offline source whose `measured_at` is old (e.g. a
    /// rollout file); freshness is judged from `measured_at`, as for every reading.
    Stale,
    /// A fallback source answered, but a token source reported an expired sign-in (HTTP 401):
    /// the app records the attempt as a success for scheduling and sets
    /// `ServiceStatus::sign_in_required`, so the service shows STALE(SIGN_IN) (R§3.8) even though
    /// values were read.
    SignInRequired,
    /// Values were read, but some meters could not be parsed (details in the log).
    Error,
}

/// One reading of one account of one service (ARCHITECTURE "Snapshot"; R§2 "Reading").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// The service.
    pub provider: ProviderId,
    /// The account.
    pub account: AccountRef,
    /// The source that answered.
    pub source: SourceKind,
    /// When the values were true: the service's own time when it reports one (cached usage, a
    /// rollout event, a desktop history entry), else equal to `fetched_at`. **This is `t_r` of
    /// R§3.1** ([`Snapshot::reading_at`]): history samples, `e(t_r)` of stale meters, ages,
    /// `Last reading 12:42` and `last_success` (R§3.8) all use it, so a 3-hour-old cached
    /// reading is shown and judged as 3 hours old (ARCHITECTURE goal 3, "Honest data"). Never
    /// later than `fetched_at`.
    pub measured_at: Timestamp,
    /// When Lastdrop read the values (logs). Equals `measured_at` for live sources; statistics
    /// never use it.
    pub fetched_at: Timestamp,
    /// Window meters, in the provider's order.
    pub meters: Vec<Meter>,
    /// Balance meters, in the provider's order.
    pub balances: Vec<Balance>,
    /// The provider's verdict on the reading.
    pub health: Health,
}

impl Snapshot {
    /// `t_r` (R§3.1): [`Snapshot::measured_at`].
    pub fn reading_at(&self) -> Timestamp {
        self.measured_at
    }

    /// The credential source this snapshot was read from.
    pub fn source_key(&self) -> SourceKey {
        SourceKey {
            provider: self.provider,
            account: self.account.key.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MeterId, MeterKind, MeterNames, ProviderId, WindowLen, catalog};
    use jiff::SignedDuration;

    #[test]
    fn catalogue_matches_the_r2_table() {
        let claude = catalog(ProviderId::Claude);
        assert_eq!(claude[0].id, MeterId::FIVE_HOUR);
        assert_eq!(
            claude[0].names(),
            MeterNames::new("5h", "5-hour", "5-hour session")
        );
        assert_eq!(
            claude[0].kind,
            MeterKind::Window {
                window: WindowLen::Fixed(SignedDuration::from_hours(5)),
                starts_on_use: true,
            }
        );
        assert_eq!(
            claude[1].names(),
            MeterNames::new("Wk", "weekly", "Weekly · all models")
        );
        assert_eq!(catalog(ProviderId::Codex)[1].name, "Weekly");
        let meshy = catalog(ProviderId::Meshy);
        assert_eq!(
            (meshy[0].id, meshy[0].kind),
            (MeterId::CREDITS, MeterKind::Balance)
        );
        for p in ProviderId::ALL {
            let defs = catalog(p);
            assert!(defs.iter().all(|d| d.short_label.chars().count() <= 5));
            assert!(defs.windows(2).all(|w| w[0].order < w[1].order));
        }
    }

    #[test]
    fn per_model_weekly_names() {
        assert_eq!(
            MeterNames::weekly_model("Fable"),
            MeterNames::new("Fable", "Fable weekly", "Weekly · Fable")
        );
    }
}
