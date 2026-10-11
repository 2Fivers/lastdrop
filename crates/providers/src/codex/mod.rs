//! Codex: 5-hour and weekly windows, whichever the plan has (R§2 table).
//!
//! Sources, in order (ARCHITECTURE provider table):
//! 1. [`rollout`] while fresh: the newest rollout file's last `token_count.rate_limits` event,
//!    used as it is when it is younger than [`ROLLOUT_FRESH`] (Codex is in use and wrote it a
//!    moment ago, from the same rate-limit headers the app-server would ask for). No process is
//!    started; health `Ok`.
//! 2. [`app_server`]: `codex app-server`, JSON-RPC over stdio, `account/rateLimits/read` (the
//!    official way; Codex renews its own sign-in). Newest bundled or `PATH` `codex.exe`, no
//!    window, plugins off, 8 s to initialize, 3 s per call; started at most every
//!    [`APP_SERVER_PACE`] per `CODEX_HOME` (below).
//! 3. [`rollout`] as the fallback: when the app-server fails, or rests and the rollout event is
//!    newer than its last answer (health `Stale`, or `SignInRequired` after an app-server sign-in
//!    error).
//!
//! Both rollout uses follow the `rollout_fallback` option: when it is off the rollout is never
//! read.
//!
//! **The app-server's pace.** Each start is a process (two with its console host) and its own
//! network calls, so the provider starts it at most every [`APP_SERVER_PACE`] per `CODEX_HOME`,
//! counted from the start of the fetch that started it, with [`APP_SERVER_PACE_SLACK`] so a fetch
//! the scheduler starts on the 15-minute grid is not pushed to the next one. In between, the last
//! start's result stands in with its original `measured_at` (a reading is never re-dated), unless
//! the rollout has a newer event. The memory (interior `Mutex`, shared by clones, locked only to
//! read or update it, never across file or process I/O) is keyed by `CODEX_HOME`, so accounts
//! sharing a folder share the pace. A start counts when a process was started, whatever it
//! answered; a missing `codex.exe` or Codex folder starts nothing and does not count. A changed
//! credential fingerprint ([`CREDENTIAL_FILES`]: a new sign-in, another account) lifts the pace and
//! forgets the last answer. The app-server itself rewrites `auth.json` when it renews Codex's
//! sign-in (codex-rs `login/src/auth/manager.rs`: shortly before the access token expires), so the
//! fingerprint is taken again after every start that did not end in a sign-in error: its own
//! renewal never lifts the pace, a sign-in after a sign-in error always does. A manual refresh
//! does not lift it: the provider cannot tell one from a scheduled fetch. The memory lives as
//! long as the provider, which the app rebuilds when the Codex options change.
//!
//! **Why not `min_interval`.** [`Provider::min_interval`] stays [`MIN_INTERVAL`] (1 minute), the
//! lower bound between whole fetches in `lastdrop_core::refresh`: a fetch is now a few file reads
//! unless the app-server is due. Raising it to 15 minutes would also hold "Refresh now" and the
//! back-off floor at 15 minutes and read a fresh rollout only every 15 minutes while Codex is in
//! use; and the scheduler counts it per account, not per `CODEX_HOME`. The Claude provider paces
//! its OAuth source inside the provider the same way.
//!
//! Windows are told apart by their length (`window_minutes`), never by their position: a plan
//! without a 5-hour limit reports its weekly window as `primary` ([`meter`]).

pub mod app_server;
pub mod rollout;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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
    /// When the values were computed (the rollout event's timestamp; `None` from
    /// [`app_server::fetch`], which answers live: the provider dates the answer when it keeps it
    /// to stand in between starts).
    pub measured_at: Option<Timestamp>,
}

/// What the provider remembers about the app-server of one `CODEX_HOME` (module docs, "The
/// app-server's pace").
#[derive(Clone, Debug, Default)]
struct LiveMemory {
    /// The credential fingerprint this memory belongs to.
    fingerprint: Option<CredentialFingerprint>,
    /// Start of the fetch that last started the app-server.
    started: Option<Timestamp>,
    /// What that start gave (an answer dated when it came), standing in until the next start;
    /// `None` while it runs.
    last: Option<Result<CodexUsage, FetchError>>,
}

impl LiveMemory {
    /// Forgets everything when the credential changed (a new sign-in or another account).
    fn follow_credential(&mut self, fingerprint: Option<CredentialFingerprint>) {
        if self.fingerprint != fingerprint {
            *self = LiveMemory {
                fingerprint,
                ..LiveMemory::default()
            };
        }
    }

    /// The end of the rest after the last start, when `now` is inside it. A start "after" `now`
    /// (the clock went back) does not hold the app-server back.
    fn rest_until(&self, now: Timestamp) -> Option<Timestamp> {
        let started = self.started?;
        let until = add(
            started,
            APP_SERVER_PACE.saturating_sub(APP_SERVER_PACE_SLACK),
        );
        (started <= now && now < until).then_some(until)
    }
}

/// What the app-server source gave in one fetch.
enum Live {
    /// Started now; its result.
    Started(Result<CodexUsage, FetchError>),
    /// Not started: the last start (at `started`) rests until `until`; its result stands in
    /// (`None`: that start has not finished yet).
    Resting {
        started: Timestamp,
        until: Timestamp,
        last: Option<Result<CodexUsage, FetchError>>,
    },
    /// Not started and nothing to stand in: no `codex.exe`, no Codex folder or no time left.
    Unavailable(FetchError),
}

/// The Codex provider.
#[derive(Clone, Debug)]
pub struct CodexProvider {
    options: CodexOptions,
    /// The `codex.exe` the last fetch found, for [`Provider::sign_in_command`] (the bundled one
    /// is usually not on `PATH`). Shared by clones; locked only to read or replace the path.
    last_exe: Arc<Mutex<Option<PathBuf>>>,
    /// Per `CODEX_HOME`: the app-server's pace and last result, shared by clones (it describes
    /// the folder, not this value).
    live: Arc<Mutex<BTreeMap<PathBuf, LiveMemory>>>,
}

impl CodexProvider {
    /// A provider with the user's options (`codex.exe` override, rollout fallback).
    pub fn new(options: CodexOptions) -> CodexProvider {
        CodexProvider {
            options,
            last_exe: Arc::new(Mutex::new(None)),
            live: Arc::default(),
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

    fn memories(&self) -> MutexGuard<'_, BTreeMap<PathBuf, LiveMemory>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The app-server source with its pace: find `codex.exe` (remembered for the sign-in
    /// command), check the folder and the time left, then start it unless the last start of this
    /// `CODEX_HOME` is less than [`APP_SERVER_PACE`] ago. `started` is when the fetch began.
    fn live(&self, ctx: &FetchContext<'_>, codex_home: &Path, started: Timestamp) -> Live {
        let Some(exe) = app_server::find_codex_exe(ctx, self.options.codex_exe.as_deref()) else {
            return Live::Unavailable(FetchError::Unavailable {
                detail: "codex.exe not found".to_owned(),
            });
        };
        self.remember_exe(&exe);
        if let Err(error) = app_server::preflight(ctx, codex_home) {
            return Live::Unavailable(error);
        }
        let fingerprint = fingerprint_of(ctx, codex_home);
        {
            let mut memories = self.memories();
            let memory = memories.entry(codex_home.to_path_buf()).or_default();
            memory.follow_credential(fingerprint);
            if let (Some(until), Some(last_start)) = (memory.rest_until(started), memory.started) {
                return Live::Resting {
                    started: last_start,
                    until,
                    last: memory.last.clone(),
                };
            }
            // Claims the start before the lock is released: a concurrent fetch of an account
            // with the same folder rests instead of starting a second app-server.
            memory.started = Some(started);
            memory.last = None;
        }
        let result = app_server::fetch_with(ctx, &exe, codex_home).map(|mut usage| {
            usage.measured_at.get_or_insert(ctx.clock.now());
            usage
        });
        // The app-server may have renewed the sign-in and rewritten auth.json: that is not a new
        // sign-in. After a sign-in error a changed file is one, so the old fingerprint stays.
        let fingerprint_after = (!matches!(result, Err(FetchError::Auth { .. })))
            .then(|| fingerprint_of(ctx, codex_home));
        if let Some(memory) = self.memories().get_mut(codex_home)
            && memory.started == Some(started)
        {
            if let Some(fingerprint) = fingerprint_after {
                memory.fingerprint = fingerprint;
            }
            memory.last = Some(result.clone());
        }
        Live::Started(result)
    }
}

/// Lower bound between fetches: 1 minute (**not from the spec**). A fetch is a few file reads;
/// the app-server, which starts a process, has its own pace ([`APP_SERVER_PACE`], module docs
/// "Why not `min_interval`").
pub const MIN_INTERVAL: Duration = Duration::from_secs(60);

/// A rollout event younger than this is used as it is, without starting the app-server
/// (**not from the spec**).
pub const ROLLOUT_FRESH: SignedDuration = SignedDuration::from_mins(10);

/// How far after the clock a rollout event may be dated and still count as fresh (Codex and
/// Lastdrop read the same clock; an event dated later than this is not trusted to be fresh).
pub const ROLLOUT_FUTURE_TOLERANCE: SignedDuration = SignedDuration::from_mins(1);

/// The app-server is started at most this often per `CODEX_HOME` (**not from the spec**), counted
/// from the start of the fetch that started it.
pub const APP_SERVER_PACE: Duration = Duration::from_secs(15 * 60);

/// Slack on [`APP_SERVER_PACE`], so a fetch the scheduler starts exactly 15 minutes after the
/// last start is not pushed to the next interval by the moments between the scheduler's start
/// time and the provider's (the same 30 s as `claude::OAUTH_MIN_GAP_SLACK`).
pub const APP_SERVER_PACE_SLACK: Duration = Duration::from_secs(30);

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

    /// A fresh rollout event (younger than [`ROLLOUT_FRESH`]) first, without starting anything;
    /// else the app-server, started at most every [`APP_SERVER_PACE`] per `CODEX_HOME` (its last
    /// result stands in between starts); then the rollout event when the app-server failed or
    /// its stand-in is older. The rollout is read only when `rollout_fallback` is on. An
    /// app-server sign-in error with rollout values gives `Health::SignInRequired`. Every choice
    /// is logged at INFO with its reason (module docs).
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
        let started = ctx.clock.now();

        // 1. A fresh rollout event: Codex is in use; nothing to start.
        let rollout = self
            .options
            .rollout_fallback
            .then(|| rollout::fetch(ctx, codex_home));
        if let Some(Ok(usage)) = &rollout
            && let Some(age) = fresh_age(usage, started)
        {
            log::info!(
                "codex[{}]: using the rollout event from {} min ago ({} window(s)): younger than \
                 {} min, so the app-server is not started",
                key.0,
                age.as_mins(),
                window_count(usage),
                ROLLOUT_FRESH.as_mins()
            );
            let mut snapshot = to_snapshot(
                usage,
                SourceKind::CodexRollout,
                account_ref,
                ctx.clock.now(),
            );
            snapshot.health = Health::Ok;
            return Ok(snapshot);
        }

        // 2. The app-server, at its pace.
        let live_result = match self.live(ctx, codex_home, started) {
            Live::Started(result) => {
                let reason = start_reason(rollout.as_ref(), started);
                match &result {
                    Ok(usage) => log::info!(
                        "codex[{}]: using the app-server's answer ({} window(s)); started because \
                         {reason}",
                        key.0,
                        window_count(usage),
                    ),
                    Err(error) => log::warn!(
                        "codex[{}]: app-server failed: {error} (started because {reason})",
                        key.0
                    ),
                }
                result
            }
            Live::Resting {
                started: last_start,
                until,
                last,
            } => {
                log::info!(
                    "codex[{}]: app-server not started: last started {} min ago, at most every \
                     {} min per Codex folder (next from {until})",
                    key.0,
                    started.duration_since(last_start).as_mins(),
                    APP_SERVER_PACE.as_secs() / 60
                );
                let last = last.unwrap_or_else(|| {
                    Err(FetchError::Transient {
                        from: SourceKind::CodexAppServer,
                        detail: "codex app-server is starting for another account".to_owned(),
                        retry_after: None,
                    })
                });
                match last {
                    Ok(usage) => {
                        let newer_rollout = match &rollout {
                            Some(Ok(event)) if event.measured_at > usage.measured_at => Some(event),
                            _ => None,
                        };
                        let Some(event) = newer_rollout else {
                            log::info!(
                                "codex[{}]: using the app-server's answer from {} min ago",
                                key.0,
                                age_mins(usage.measured_at, started)
                            );
                            return Ok(to_snapshot(
                                &usage,
                                SourceKind::CodexAppServer,
                                account_ref,
                                ctx.clock.now(),
                            ));
                        };
                        log::info!(
                            "codex[{}]: using the rollout event from {} min ago: newer than the \
                             app-server's answer from {} min ago",
                            key.0,
                            age_mins(event.measured_at, started),
                            age_mins(usage.measured_at, started)
                        );
                        return Ok(to_snapshot(
                            event,
                            SourceKind::CodexRollout,
                            account_ref,
                            ctx.clock.now(),
                        ));
                    }
                    Err(error) => {
                        log::info!(
                            "codex[{}]: the app-server's last start gave no reading: {error}",
                            key.0
                        );
                        Err(error)
                    }
                }
            }
            Live::Unavailable(error) => {
                log::warn!("codex[{}]: app-server not started: {error}", key.0);
                Err(error)
            }
        };
        let live_error = match live_result {
            Ok(usage) => {
                return Ok(to_snapshot(
                    &usage,
                    SourceKind::CodexAppServer,
                    account_ref,
                    ctx.clock.now(),
                ));
            }
            Err(error) => error,
        };

        // 3. The rollout as the fallback.
        match rollout {
            None => Err(live_error),
            Some(Ok(usage)) => {
                log::info!(
                    "codex[{}]: using the rollout event from {} min ago ({} window(s)) because \
                     the app-server gave no reading",
                    key.0,
                    age_mins(usage.measured_at, started),
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
            Some(Err(rollout_error)) => {
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
        fingerprint_of(ctx, codex_home)
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

/// `t + d`, saturating at jiff's range.
fn add(t: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| t.checked_add(d).ok())
        .unwrap_or(Timestamp::MAX)
}

/// The age of a rollout reading at `now` when it is fresh: dated less than [`ROLLOUT_FRESH`]
/// before `now` and at most [`ROLLOUT_FUTURE_TOLERANCE`] after it (then age 0). `None` when
/// older, too far ahead or undated.
pub fn fresh_age(usage: &CodexUsage, now: Timestamp) -> Option<SignedDuration> {
    let age = now.duration_since(usage.measured_at?);
    (age < ROLLOUT_FRESH && age >= -ROLLOUT_FUTURE_TOLERANCE).then(|| age.max(SignedDuration::ZERO))
}

/// Whole minutes between `at` and `now` (0 for an undated reading or one after `now`), for logs.
fn age_mins(at: Option<Timestamp>, now: Timestamp) -> i64 {
    at.map_or(0, |t| now.duration_since(t).as_mins().max(0))
}

/// Why the app-server is started, for the log: what the rollout had to offer.
fn start_reason(rollout: Option<&Result<CodexUsage, FetchError>>, now: Timestamp) -> String {
    match rollout {
        None => "the rollout is turned off".to_owned(),
        Some(Err(error)) => format!("the rollout has no reading ({error})"),
        Some(Ok(usage)) => match usage.measured_at {
            Some(t) if t > now => "the newest rollout event is dated ahead of the clock".to_owned(),
            Some(t) => format!(
                "the newest rollout event is {} min old",
                now.duration_since(t).as_mins()
            ),
            None => "the newest rollout event is undated".to_owned(),
        },
    }
}

/// Size + mtime of the first of [`CREDENTIAL_FILES`] in `codex_home`; `None` when neither exists.
/// Only metadata is read, never the contents.
fn fingerprint_of(ctx: &FetchContext<'_>, codex_home: &Path) -> Option<CredentialFingerprint> {
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
