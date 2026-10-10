//! Codex: 5-hour and weekly windows, whichever the plan has (R§2 table).
//!
//! Sources, in order (ARCHITECTURE provider table):
//! 1. [`app_server`]: `codex app-server`, JSON-RPC over stdio, `account/rateLimits/read` (the
//!    official way; Codex renews its own sign-in). Newest bundled or `PATH` `codex.exe`, no
//!    window, 8 s to initialize, 3 s per call.
//! 2. [`rollout`]: the newest rollout file's last `token_count.rate_limits` event (offline).
//!
//! Windows are told apart by their length (`window_minutes`), never by their position: a plan
//! without a 5-hour limit reports its weekly window as `primary` ([`meter`]).

pub mod app_server;
pub mod rollout;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{
    AccountRef, Health, Meter, MeterDef, MeterId, MeterKind, MeterNames, ProviderId, Snapshot,
    SourceKind, WindowLen, catalog,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::settings::CodexOptions;
use serde_json::Value;

use crate::account::AccountSource;
use crate::error::FetchError;
use crate::io::TerminalCommand;
use crate::{FetchContext, Provider};

/// One Codex rate-limit window as Codex reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct CodexWindow {
    /// `used_percent` (0–100, may exceed 100).
    pub used_percent: f64,
    /// `window_minutes` (300 = 5-hour, 10080 = weekly); `None` when not reported.
    pub window_minutes: Option<i64>,
    /// When it resets; `None` when not reported.
    pub resets_at: Option<Timestamp>,
}

/// What a Codex source returned: the primary and secondary windows, whichever exist.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CodexUsage {
    /// The primary window (usually the 5-hour session).
    pub primary: Option<CodexWindow>,
    /// The secondary window (usually weekly).
    pub secondary: Option<CodexWindow>,
    /// Plan name when the source says (`Plus`, `Pro`).
    pub plan: Option<String>,
    /// When the values were computed (the rollout event's timestamp; `None` from the app-server,
    /// which answers live).
    pub measured_at: Option<Timestamp>,
}

/// The Codex provider.
#[derive(Clone, Debug)]
pub struct CodexProvider {
    options: CodexOptions,
    /// The `codex.exe` the last fetch found, for [`Provider::sign_in_command`] (the bundled one
    /// is usually not on `PATH`). Shared by clones; locked only to read or replace the path.
    last_exe: Arc<Mutex<Option<PathBuf>>>,
}

impl CodexProvider {
    /// A provider with the user's options (`codex.exe` override, rollout fallback).
    pub fn new(options: CodexOptions) -> CodexProvider {
        CodexProvider {
            options,
            last_exe: Arc::new(Mutex::new(None)),
        }
    }

    /// The options in use.
    pub fn options(&self) -> &CodexOptions {
        &self.options
    }

    fn remember_exe(&self, exe: &Path) {
        let mut last = self.last_exe.lock().unwrap_or_else(PoisonError::into_inner);
        if last.as_deref() != Some(exe) {
            *last = Some(exe.to_path_buf());
        }
    }

    fn remembered_exe(&self) -> Option<PathBuf> {
        self.last_exe
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Source 1: find `codex.exe` (remembered for the sign-in command), then ask it.
    fn fetch_live(
        &self,
        ctx: &FetchContext<'_>,
        codex_home: &Path,
    ) -> Result<CodexUsage, FetchError> {
        let exe = app_server::find_codex_exe(ctx, self.options.codex_exe.as_deref()).ok_or_else(
            || FetchError::Unavailable {
                detail: "codex.exe not found".to_owned(),
            },
        )?;
        self.remember_exe(&exe);
        app_server::fetch_with(ctx, &exe, codex_home)
    }
}

/// Lower bound between fetches: 1 minute (**not from the spec**: the app-server is official and
/// local, but each fetch starts a process).
pub const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// Codex's own usage page (F§9 "Enter on a card"), the page Codex itself links to when a limit
/// is reached.
pub const USAGE_PAGE: &str = "https://chatgpt.com/codex/settings/usage";

/// Credential files whose size + modification time make the fingerprint, relative to
/// `CODEX_HOME`, first existing wins: `auth.json` (file storage, the default) or the encrypted
/// store Codex uses with keyring storage on Windows. Only their metadata is read, never their
/// contents.
pub const CREDENTIAL_FILES: [&str; 2] = ["auth.json", r"secrets\codex_auth.age"];

impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    /// App-server first; on failure (and when the fallback is on) the newest rollout event. An
    /// app-server sign-in error with rollout values gives `Health::SignInRequired`.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::Codex {
            account: key,
            codex_home,
        } = account
        else {
            return Err(FetchError::Unavailable {
                detail: "not a Codex account".to_owned(),
            });
        };
        let account_ref = AccountRef {
            key: key.clone(),
            ..AccountRef::primary()
        };

        let live_error = match self.fetch_live(ctx, codex_home) {
            Ok(usage) => {
                log::info!(
                    "codex[{}]: app-server answered ({} window(s))",
                    key.0,
                    window_count(&usage)
                );
                return Ok(to_snapshot(
                    &usage,
                    SourceKind::CodexAppServer,
                    account_ref,
                    ctx.clock.now(),
                ));
            }
            Err(error) => error,
        };
        log::warn!("codex[{}]: app-server failed: {live_error}", key.0);
        if !self.options.rollout_fallback {
            return Err(live_error);
        }

        match rollout::fetch(ctx, codex_home) {
            Ok(usage) => {
                log::info!(
                    "codex[{}]: rollout answered ({} window(s))",
                    key.0,
                    window_count(&usage)
                );
                let mut snapshot = to_snapshot(
                    &usage,
                    SourceKind::CodexRollout,
                    account_ref,
                    ctx.clock.now(),
                );
                if matches!(live_error, FetchError::Auth { .. }) {
                    snapshot.health = Health::SignInRequired;
                }
                Ok(snapshot)
            }
            Err(rollout_error) => {
                log::warn!("codex[{}]: rollout failed: {rollout_error}", key.0);
                Err(most_telling(live_error, rollout_error))
            }
        }
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    /// Size + mtime of `<codex_home>\auth.json` (or of the keyring-mode store, see
    /// [`CREDENTIAL_FILES`]); `None` when neither exists.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        let AccountSource::Codex { codex_home, .. } = account else {
            return None;
        };
        CREDENTIAL_FILES.iter().find_map(|name| {
            let meta = ctx.fs.metadata(&codex_home.join(name)).ok()?;
            if meta.is_dir {
                return None;
            }
            let modified = meta
                .modified
                .map_or_else(|| "-".to_owned(), |t| t.as_nanosecond().to_string());
            Some(CredentialFingerprint(format!(
                "{name}:{}:{modified}",
                meta.len
            )))
        })
    }

    /// `codex login` in a terminal with `CODEX_HOME` set to the account's folder: the configured
    /// `codex.exe`, else the one the last fetch found, else `codex` from `PATH`.
    fn sign_in_command(&self, account: &AccountSource) -> Option<TerminalCommand> {
        let AccountSource::Codex { codex_home, .. } = account else {
            return None;
        };
        let program = self
            .options
            .codex_exe
            .clone()
            .or_else(|| self.remembered_exe())
            .map_or_else(|| "codex".into(), PathBuf::into_os_string);
        Some(TerminalCommand {
            program,
            args: vec!["login".into()],
            env: vec![(
                app_server::CODEX_HOME_VAR.into(),
                Some(codex_home.clone().into_os_string()),
            )],
            cwd: None,
        })
    }

    fn usage_page(&self) -> Option<&'static str> {
        Some(USAGE_PAGE)
    }
}

fn window_count(usage: &CodexUsage) -> usize {
    usize::from(usage.primary.is_some()) + usize::from(usage.secondary.is_some())
}

/// The more telling of two failures (`FetchError` docs: an auth error over a parse error over
/// "not found"); `first` on a tie.
fn most_telling(first: FetchError, second: FetchError) -> FetchError {
    fn rank(e: &FetchError) -> u8 {
        match e {
            FetchError::Auth { .. } => 5,
            FetchError::RateLimited { .. } | FetchError::Offline { .. } => 4,
            FetchError::Transient { .. } => 3,
            FetchError::Parse { .. } => 2,
            FetchError::Unavailable { .. } => 1,
        }
    }
    if rank(&second) > rank(&first) {
        second
    } else {
        first
    }
}

// ---- Meters -------------------------------------------------------------------------------------

/// The 5-hour window's length in minutes.
pub const FIVE_HOUR_MINUTES: i64 = 300;

/// The weekly window's length in minutes.
pub const WEEKLY_MINUTES: i64 = 7 * 24 * 60;

/// How far a reported length may be from 300 or 10080 and still count as that window (Codex's
/// own TUI rounds window lengths with the same 3-minute bias).
pub const LENGTH_TOLERANCE_MINUTES: i64 = 3;

/// Longest window length taken as given (10 years); longer or non-positive lengths count as not
/// reported.
const MAX_WINDOW_MINUTES: i64 = 10 * 366 * 24 * 60;

/// Order of meters outside the catalogue: after the 5-hour (0) and weekly (1) meters.
const OTHER_ORDER: u16 = 2;

fn near(minutes: i64, target: i64) -> bool {
    (minutes - target).abs() <= LENGTH_TOLERANCE_MINUTES
}

/// A reported window length, `None` when missing or not plausible.
fn length_of(window: &CodexWindow) -> Option<i64> {
    window
        .window_minutes
        .filter(|m| (1..=MAX_WINDOW_MINUTES).contains(m))
}

fn catalog_def(id: &str) -> Option<&'static MeterDef> {
    catalog(ProviderId::Codex).iter().find(|d| d.id == id)
}

/// `u` (R§3.1): `used_percent / 100`, never negative; may exceed 1.
fn used_share(window: &CodexWindow) -> f64 {
    (window.used_percent / 100.0).max(0.0)
}

fn from_def(def: &MeterDef, window: &CodexWindow) -> Meter {
    let (len, starts_on_use) = match def.kind {
        MeterKind::Window {
            window,
            starts_on_use,
        } => (window, starts_on_use),
        MeterKind::Balance => (WindowLen::Unknown, false),
    };
    Meter {
        id: def.meter_id(),
        names: def.names(),
        order: def.order,
        used: used_share(window),
        limited: false,
        resets_at: window.resets_at,
        window: len,
        starts_on_use,
    }
}

/// Names of a window outside the catalogue, by its length: whole hours below two days (`24h` /
/// `24-hour` / `24-hour window`), else whole days (`30d` / `30-day` / `30-day window`).
fn length_names(minutes: i64) -> MeterNames {
    const DAY: i64 = 24 * 60;
    let (n, unit, word) = if minutes < 2 * DAY {
        ((minutes + 30) / 60, "h", "hour")
    } else {
        ((minutes + DAY / 2) / DAY, "d", "day")
    };
    let n = n.max(1);
    MeterNames {
        short_label: format!("{n}{unit}"),
        short_name: format!("{n}-{word}"),
        name: format!("{n}-{word} window"),
    }
}

fn by_length(minutes: i64, window: &CodexWindow, secondary: bool) -> Meter {
    Meter {
        id: MeterId(format!("window:{minutes}")),
        names: length_names(minutes),
        order: OTHER_ORDER + u16::from(secondary),
        used: used_share(window),
        limited: false,
        resets_at: window.resets_at,
        window: WindowLen::Fixed(SignedDuration::from_mins(minutes)),
        starts_on_use: false,
    }
}

/// The meter of a Codex window with the R§2 definition: `window_minutes` 300 → `five_hour`
/// (`5h`, starts on use), 10080 → `weekly` (`Wk`), both with names and windows from
/// `lastdrop_core::model::catalog`; other lengths → a fixed window named by its length; unknown
/// length → primary = 5-hour, secondary = weekly.
///
/// "300" and "10080" allow [`LENGTH_TOLERANCE_MINUTES`]. Other lengths get the id
/// `window:<minutes>`, names from the length (`24h` / `24-hour` / `24-hour window`), order 2
/// (primary) or 3 (secondary) and `starts_on_use = false`.
pub fn meter(window: &CodexWindow, secondary: bool) -> Meter {
    let id = match length_of(window) {
        Some(m) if near(m, FIVE_HOUR_MINUTES) => MeterId::FIVE_HOUR,
        Some(m) if near(m, WEEKLY_MINUTES) => MeterId::WEEKLY,
        Some(m) => return by_length(m, window, secondary),
        None if secondary => MeterId::WEEKLY,
        None => MeterId::FIVE_HOUR,
    };
    match catalog_def(id) {
        Some(def) => from_def(def, window),
        // The catalogue always has both Codex meters; a fixed 5-hour/weekly window otherwise.
        None => by_length(
            if id == MeterId::WEEKLY {
                WEEKLY_MINUTES
            } else {
                FIVE_HOUR_MINUTES
            },
            window,
            secondary,
        ),
    }
}

/// The snapshot of a usage reading from `source`.
///
/// Meters in meter order; a second window that maps to a meter already present is dropped,
/// except that a window of unknown length takes the other catalogue meter when its own is taken.
/// `measured_at` is the usage's time (never after `fetched_at`), else `fetched_at`. The plan
/// replaces `account.plan` when the source names one. Health: `Ok` from the app-server, `Stale`
/// from a rollout file (cached values).
pub fn to_snapshot(
    usage: &CodexUsage,
    source: SourceKind,
    account: AccountRef,
    fetched_at: Timestamp,
) -> Snapshot {
    let mut meters: Vec<Meter> = Vec::with_capacity(2);
    let windows = [(&usage.primary, false), (&usage.secondary, true)];
    let windows = windows
        .iter()
        .filter_map(|(w, secondary)| Some((w.as_ref()?, *secondary)));
    // Windows of known length first: they own their meter; unknown lengths take what is left.
    let (known, unknown): (Vec<_>, Vec<_>) = windows.partition(|(w, _)| length_of(w).is_some());
    for (window, secondary) in known.into_iter().chain(unknown) {
        let mut m = meter(window, secondary);
        if meters.iter().any(|x| x.id == m.id) && length_of(window).is_none() {
            let other = if m.id.0 == MeterId::WEEKLY {
                MeterId::FIVE_HOUR
            } else {
                MeterId::WEEKLY
            };
            if let Some(def) = catalog_def(other) {
                m = from_def(def, window);
            }
        }
        if meters.iter().any(|x| x.id == m.id) {
            log::warn!(
                "codex: two windows map to meter {}; keeping the first",
                m.id.0
            );
            continue;
        }
        meters.push(m);
    }
    meters.sort_by_key(|m| m.order);

    let measured_at = usage.measured_at.map_or(fetched_at, |t| t.min(fetched_at));
    let mut account = account;
    if let Some(plan) = &usage.plan {
        account.plan = Some(plan.clone());
    }
    let health = match source {
        SourceKind::CodexRollout => Health::Stale,
        _ => Health::Ok,
    };
    Snapshot {
        provider: ProviderId::Codex,
        account,
        source,
        measured_at,
        fetched_at,
        meters,
        balances: Vec::new(),
        health,
    }
}

// ---- Shared parsing helpers (app-server and rollout) ----------------------------------------------

/// The plan's display name for the plan badge (`plus` → `Plus`, `prolite` → `Pro Lite`);
/// `None` for an empty or `unknown` plan. Unknown ids are title-cased word by word.
pub fn plan_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let known = match raw.to_ascii_lowercase().as_str() {
        "" | "unknown" => return None,
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "pro" => "Pro",
        "prolite" => "Pro Lite",
        "promax" => "Pro Max",
        "team" => "Team",
        "business" | "self_serve_business_prolite" | "self_serve_business_usage_based" => {
            "Business"
        }
        "enterprise" | "ent26" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" => {
            "Enterprise"
        }
        "edu" => "Edu",
        "edu_plus" => "Edu Plus",
        "edu_pro" => "Edu Pro",
        _ => "",
    };
    if !known.is_empty() {
        return Some(known.to_owned());
    }
    let words: Vec<String> = raw
        .split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            chars.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// A number that may arrive as an integer, a float or a numeric string.
pub(crate) fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|f| f.is_finite())
}

/// Whole minutes from a number (rounded).
pub(crate) fn minutes(v: &Value) -> Option<i64> {
    if let Some(i) = v.as_i64() {
        return Some(i);
    }
    let f = number(v)?.round();
    // `as` saturates; the range check in `length_of` rejects absurd values.
    (f.abs() < 1e15).then_some(f as i64)
}

/// An instant given as Unix seconds (milliseconds when it is too large for seconds) or an
/// RFC 3339 string.
pub(crate) fn instant(v: &Value) -> Option<Timestamp> {
    if let Value::String(s) = v
        && let Ok(t) = s.trim().parse::<Timestamp>()
    {
        return Some(t);
    }
    let secs = number(v)?;
    // Unix seconds stay below 1e11 until the year 5138; larger values are milliseconds.
    let secs = if secs.abs() >= 1e11 {
        secs / 1000.0
    } else {
        secs
    };
    if secs.abs() >= 1e12 {
        return None;
    }
    Timestamp::from_second(secs.round() as i64).ok()
}

/// One window object from either source. `keys` = (used percent, window minutes, resets at);
/// `relative` = the legacy `resets_in_seconds` key and the instant it counts from. `None` when
/// the object or its used percent is missing (a missing value is never 0).
pub(crate) fn window_from(
    v: &Value,
    keys: (&str, &str, &str),
    relative: Option<(&str, Option<Timestamp>)>,
) -> Option<CodexWindow> {
    let obj = v.as_object()?;
    let used_percent = obj.get(keys.0).and_then(number)?;
    let window_minutes = obj.get(keys.1).and_then(minutes);
    let mut resets_at = obj.get(keys.2).and_then(instant);
    if resets_at.is_none()
        && let Some((key, Some(base))) = relative
        && let Some(secs) = obj.get(key).and_then(number)
        && secs.abs() < 1e12
    {
        resets_at = base
            .checked_add(SignedDuration::from_secs(secs.round() as i64))
            .ok();
    }
    Some(CodexWindow {
        used_percent,
        window_minutes,
        resets_at,
    })
}
