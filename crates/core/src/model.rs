//! The data model: what a provider returns and how meters are identified (ARCHITECTURE
//! "Data model", R§2 vocabulary, R§3.1 inputs).
//!
//! Shares are `f64` where `1.0` is the whole limit (R§0). A value the service did not report is
//! `None`, never `0` (ARCHITECTURE "Rules for every provider").

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Span, Timestamp};
use serde::{Deserialize, Serialize};

use crate::glyph::ProviderGlyph;
use crate::secret::SecretString;

/// A service Lastdrop reads (R§2 "Service"). M2 adds Cursor, Antigravity, … .
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    /// Claude plans (Claude Code / Claude Desktop): 5-hour, weekly and per-model weekly windows.
    Claude,
    /// OpenAI Codex: 5-hour and weekly windows.
    Codex,
    /// GitHub Copilot: monthly quotas (premium requests; chat and completions on Copilot Free).
    Copilot,
    /// Meshy: a credit balance.
    Meshy,
}

impl ProviderId {
    /// Every provider, in the default service order (settings default, F§2.2).
    pub const ALL: [ProviderId; 4] = [
        ProviderId::Claude,
        ProviderId::Codex,
        ProviderId::Copilot,
        ProviderId::Meshy,
    ];

    /// Stable lower-case id used in settings, history file names and credential names
    /// (`claude`, `codex`, `copilot`, `meshy`). Never changes once shipped.
    pub const fn as_str(self) -> &'static str {
        match self {
            ProviderId::Claude => "claude",
            ProviderId::Codex => "codex",
            ProviderId::Copilot => "copilot",
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
            ProviderId::Copilot => "Copilot",
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
            ProviderId::Copilot => Some("GitHub CLI"),
            ProviderId::Meshy => None,
        }
    }
}

/// Identifies one account entry (one source, accounts-spec §1) of one service: the default
/// folder is [`AccountKey::primary`], every folder added later `f-` and 8 hex digits of its path's
/// hash ([`AccountKey::for_folder`]). Keys never change, so history, pins and notifications follow
/// the folder entry (accounts-spec §7).
///
/// Privacy: the key appears in history file names, settings and logs, so it is never an
/// e-mail address, account id or token.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountKey(pub String);

impl AccountKey {
    /// The key of a service's default folder (and of the single account of Copilot and Meshy):
    /// `"default"`.
    pub fn primary() -> AccountKey {
        AccountKey("default".to_owned())
    }

    /// The key of a folder added as an account (accounts-spec §7): `f-` and the first 8 hex
    /// digits of [`fnv1a64`] over the folder's path in lower case, then `-2`, `-3` … until it is
    /// not one of `taken`.
    pub fn for_folder(path: &str, taken: &[AccountKey]) -> AccountKey {
        let lower = path.to_lowercase();
        let hash = format!("{:016x}", fnv1a64(&[lower.as_str()]));
        let base = format!("f-{}", &hash[..8]);
        let mut key = AccountKey(base.clone());
        let mut n = 2;
        while taken.contains(&key) {
            key = AccountKey(format!("{base}-{n}"));
            n += 1;
        }
        key
    }
}

/// FNV-1a, 64 bits, over `parts` joined by `\n` (accounts-spec §2 "Hash"): stable across builds
/// and machines, unlike `std`'s hashers.
pub fn fnv1a64(parts: &[&str]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hash ^= u64::from(b'\n');
            hash = hash.wrapping_mul(PRIME);
        }
        for byte in part.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    hash
}

/// Who is signed in to a source, as 16 lowercase hex digits of [`fnv1a64`] over what names the
/// account (accounts-spec §1–§2). The only form of the account Lastdrop keeps (`state.json`); it is
/// never shown, and its `Debug` form hides it so it never reaches a log.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AccountIdentity(String);

impl AccountIdentity {
    /// The identity named by `parts` (the service's id first: `["claude", org, account]`).
    pub fn of(parts: &[&str]) -> AccountIdentity {
        AccountIdentity(format!("{:016x}", fnv1a64(parts)))
    }

    /// The hex digits.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AccountIdentity {
    type Error = String;

    /// Exactly 16 lowercase hex digits, as [`AccountIdentity::of`] writes them.
    fn try_from(value: String) -> Result<AccountIdentity, String> {
        let valid = value.len() == 16
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if valid {
            Ok(AccountIdentity(value))
        } else {
            Err("not an account identity".to_owned())
        }
    }
}

impl From<AccountIdentity> for String {
    fn from(identity: AccountIdentity) -> String {
        identity.0
    }
}

impl std::fmt::Debug for AccountIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccountIdentity([hidden])")
    }
}

/// What a source's own files say about who is signed in (accounts-spec §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    /// The files name an account.
    SignedIn {
        /// The account.
        id: AccountIdentity,
        /// The address it is signed in with, when the files give one; shown only as
        /// accounts-spec §4 says.
        email: Option<SecretString>,
    },
    /// The files say nobody is signed in.
    SignedOut,
    /// The files cannot tell: missing, unreadable, or a layout Lastdrop does not know.
    Unknown,
}

impl Identity {
    /// The account when one is signed in.
    pub fn id(&self) -> Option<&AccountIdentity> {
        match self {
            Identity::SignedIn { id, .. } => Some(id),
            Identity::SignedOut | Identity::Unknown => None,
        }
    }

    /// The e-mail address when one is signed in and the files give it.
    pub fn email(&self) -> Option<&SecretString> {
        match self {
            Identity::SignedIn { email, .. } => email.as_ref(),
            Identity::SignedOut | Identity::Unknown => None,
        }
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
    /// What names the account when it has no alias and its service shows two or more accounts
    /// (accounts-spec §4): the e-mail address, or `Account 2`. Set by the app on each evaluation;
    /// a secret because it can be an e-mail address, and never written to a file.
    #[serde(skip)]
    pub label: Option<SecretString>,
}

impl AccountRef {
    /// The default account: [`AccountKey::primary`], no alias, no tag, no plan, no label.
    pub fn primary() -> AccountRef {
        AccountRef {
            key: AccountKey::primary(),
            alias: None,
            tag: None,
            plan: None,
            label: None,
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
    /// Copilot's premium-requests quota (paid plans).
    pub const PREMIUM: &'static str = "premium";
    /// Copilot's chat-messages quota (Copilot Free).
    pub const CHAT: &'static str = "chat";
    /// Copilot's code-completions quota (Copilot Free).
    pub const COMPLETIONS: &'static str = "completions";

    /// The prefix of a per-model weekly window id ([`MeterId::weekly_model`]).
    const WEEKLY_MODEL_PREFIX: &'static str = "weekly:";
    /// The prefix of a window named by its length ([`MeterId::window`]).
    const WINDOW_PREFIX: &'static str = "window:";

    /// Builds an id from a static name such as [`MeterId::FIVE_HOUR`].
    pub fn new(id: &str) -> MeterId {
        MeterId(id.to_owned())
    }

    /// The per-model weekly window id `weekly:<model>` (`weekly:fable`); `model` is lower-cased
    /// by the provider before calling this.
    pub fn weekly_model(model: &str) -> MeterId {
        MeterId(format!("{}{model}", MeterId::WEEKLY_MODEL_PREFIX))
    }

    /// A per-model weekly window ([`MeterId::weekly_model`]).
    pub fn is_weekly_model(&self) -> bool {
        self.0.starts_with(MeterId::WEEKLY_MODEL_PREFIX)
    }

    /// The id `window:<minutes>` of a window outside the catalogue, named by its length (a Codex
    /// plan with other window lengths).
    pub fn window(minutes: i64) -> MeterId {
        MeterId(format!("{}{minutes}", MeterId::WINDOW_PREFIX))
    }

    /// The length in minutes of a [`MeterId::window`] id; `None` for any other id.
    pub fn window_minutes(&self) -> Option<i64> {
        self.0.strip_prefix(MeterId::WINDOW_PREFIX)?.parse().ok()
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
    /// `Weekly · Fable`, from the model's display name as the service gives it (English; the
    /// surfaces show [`crate::grammar::meters::localized`]).
    pub fn weekly_model(model: &str) -> MeterNames {
        MeterNames {
            short_label: model.to_owned(),
            short_name: format!("{model} weekly"),
            name: format!("{WEEKLY_MODEL_NAME_PREFIX}{model}"),
        }
    }

    /// The model's display name of names built by [`MeterNames::weekly_model`]
    /// (`Weekly · Fable` → `Fable`); `None` for any other names.
    pub fn weekly_model_name(&self) -> Option<&str> {
        self.name.strip_prefix(WEEKLY_MODEL_NAME_PREFIX)
    }
}

/// The start of a per-model weekly window's `name` ([`MeterNames::weekly_model`]).
const WEEKLY_MODEL_NAME_PREFIX: &str = "Weekly · ";

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
/// the surfaces show them in the user's language, [`crate::grammar::meters::localized`]). Claude's
/// per-model weekly windows come from readings (order 2, 3, … in the service's order). A Codex
/// plan with other window lengths reports meters outside the catalogue (`codex::meter`). Copilot
/// has no always-present meter.
pub const fn catalog(provider: ProviderId) -> &'static [MeterDef] {
    match provider {
        ProviderId::Claude => &CLAUDE_METERS,
        ProviderId::Codex => &CODEX_METERS,
        // Which monthly quotas exist depends on the plan (premium requests on paid plans, chat
        // and completions on Copilot Free), so none is always present: they come from readings
        // (`lastdrop_providers::copilot::meter_names`).
        ProviderId::Copilot => &[],
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
    ///
    /// A calendar month starts one calendar month (UTC) before `resets_at`: for a reset on a
    /// month start (`Nov 1 00:00`) that is the start of the month that ends there (`Oct 1`, so
    /// `L` = 31 days); a reset on another day (`Nov 15 12:00`) gives the month-long billing
    /// cycle before it (`Oct 15 12:00`). `None` when the month arithmetic leaves jiff's range.
    pub fn length_ending_at(self, resets_at: Timestamp) -> Option<SignedDuration> {
        match self {
            WindowLen::Fixed(len) => Some(len),
            WindowLen::CalendarMonth => {
                let start = resets_at
                    .to_zoned(TimeZone::UTC)
                    .checked_sub(Span::new().months(1))
                    .ok()?;
                Some(resets_at.duration_since(start.timestamp()))
            }
            WindowLen::Unknown => None,
        }
    }

    /// The reset after `resets_at` for a fixed-schedule window: `R + L`, or the next month start
    /// (R§3.9). `None` when `L` is unknown (or the result is out of range).
    pub fn next_reset(self, resets_at: Timestamp) -> Option<Timestamp> {
        match self {
            WindowLen::Fixed(len) => resets_at.checked_add(len).ok(),
            WindowLen::CalendarMonth => resets_at
                .to_zoned(TimeZone::UTC)
                .checked_add(Span::new().months(1))
                .ok()
                .map(|z| z.timestamp()),
            WindowLen::Unknown => None,
        }
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
    /// Claude Desktop's own usage response in its Chromium HTTP cache (no token needed).
    ClaudeDesktopCache,
    /// The OAuth usage endpoint with Claude Code's token (read-only, never refreshed).
    ClaudeOAuthUsage,
    /// Claude Code's cached usage in `.claude.json`.
    ClaudeCodeCache,
    /// `codex app-server`, `account/rateLimits/read`.
    CodexAppServer,
    /// The newest Codex rollout file's `token_count.rate_limits`.
    CodexRollout,
    /// `GET https://api.github.com/copilot_internal/user` with the GitHub CLI's sign-in.
    CopilotApi,
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

    #[test]
    fn fnv1a64_matches_the_reference_vectors() {
        use super::fnv1a64;
        // The FNV reference values (draft-eastlake-fnv): the offset basis, "a" and "foobar".
        assert_eq!(fnv1a64(&[""]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(&["a"]), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(&["foobar"]), 0x8594_4171_f739_67e8);
        // Parts are joined by a newline.
        assert_eq!(fnv1a64(&["foo", "bar"]), fnv1a64(&["foo\nbar"]));
        assert_ne!(fnv1a64(&["foo", "bar"]), fnv1a64(&["foobar"]));
    }

    #[test]
    fn identities_are_sixteen_hex_digits_and_never_print() {
        use super::AccountIdentity;
        let id = AccountIdentity::of(&["claude", "org", "account"]);
        assert_eq!(id.as_str().len(), 16);
        assert_eq!(id, AccountIdentity::of(&["claude", "org", "account"]));
        assert_ne!(id, AccountIdentity::of(&["codex", "org", "account"]));
        assert_eq!(format!("{id:?}"), "AccountIdentity([hidden])");
        let json = serde_json::to_string(&id).expect("serialize");
        let back: AccountIdentity = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
        for bad in [
            "\"\"",
            "\"ABCDEF0123456789\"",
            "\"0123\"",
            "\"0123456789abcdefg\"",
            "7",
        ] {
            assert!(
                serde_json::from_str::<AccountIdentity>(bad).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn folder_keys_are_stable_and_never_clash() {
        use super::AccountKey;
        let key = AccountKey::for_folder(r"C:\Users\tester\.claude-work", &[]);
        assert!(key.0.starts_with("f-") && key.0.len() == 10, "{}", key.0);
        assert_eq!(
            AccountKey::for_folder(r"c:\users\TESTER\.claude-work", &[]),
            key,
            "case does not matter, as on Windows"
        );
        let second =
            AccountKey::for_folder(r"C:\Users\tester\.claude-work", std::slice::from_ref(&key));
        assert_eq!(second.0, format!("{}-2", key.0));
        let third = AccountKey::for_folder(r"C:\Users\tester\.claude-work", &[key.clone(), second]);
        assert_eq!(third.0, format!("{}-3", key.0));
    }
}
