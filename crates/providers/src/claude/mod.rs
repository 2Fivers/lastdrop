//! Claude plans: 5-hour session, weekly all models, weekly per model (R§2 table).
//!
//! Sources, in order (ARCHITECTURE provider table):
//! 1. [`oauth`]: the OAuth usage endpoint with Claude Code's token, when the file holds a usable
//!    one — read-only, never refreshed, at most every 5 minutes per credential, `Retry-After`
//!    honoured, stop on 401 until the token file changes.
//! 2. [`desktop_cache`]: Claude Desktop's own last usage response, from its Chromium HTTP cache
//!    (no token; read with [`blockfile`] and [`response_info`]).
//! 3. [`desktop`]: Claude Desktop's `plan-usage-history.json` (no token).
//! 4. [`cache`]: Claude Code's cached usage in `.claude.json` (no token).
//!
//! The provider returns the most recently **measured** success (by `measured_at`). Readings less
//! than [`SAME_READING`] apart count as one moment, and the earlier source in the order wins:
//! Claude Desktop writes a history sample when it receives the response its cache keeps, and that
//! response is the richer reading (reset times, per-model windows). Each source parses into a
//! [`ClaudeUsage`]; [`to_snapshot`] turns it into meters with the R§2 definitions.
//!
//! **Logging.** Every source's outcome is logged at INFO, without secrets or ids: why it was
//! skipped (file missing, no or expired or rejected token, resting, no usage response, a body that
//! cannot be decoded, …) or how many windows it read and how old they are; then which reading is
//! used.
//!
//! **Reset times.** Claude Desktop's history has no reset times. When the freshest reading lacks
//! a window's `resets_at`, the provider takes it from an older reading of this fetch (the cache,
//! or the OAuth source's last answer — its last *successful* answer when the latest request
//! failed, so a 429 or 5xx does not lose the reset times) of the **same window instance**: that reading's reset is
//! still ahead at the freshest reading's time and its usage is not higher (usage only grows
//! within a window; one point of slack for the history's whole percentages). Otherwise the
//! reset stays unknown (R§3.2 "no reset time").
//!
//! **401 from the OAuth source** stops only that source, not the account: Claude Desktop's cache
//! and history and Claude Code's cached usage need no token and keep being read. The provider
//! remembers, per account, the `.credentials.json` fingerprint that got the 401 (interior
//! `Mutex`, the trait takes `&self`; locked only to read or update that map, never across file or
//! HTTP I/O) and skips the OAuth source until the fingerprint changes. When another source
//! answers, the snapshot carries `Health::SignInRequired` (the service shows STALE(SIGN_IN),
//! R§3.8); when none does, the fetch fails with `FetchError::Auth` and the scheduler stops the
//! whole source until [`Provider::credential_fingerprint`] changes.
//!
//! The same memory keeps the OAuth source's own pace: no request less than 5 minutes after the
//! previous one (the scheduler's minimum covers whole fetches; this covers the source when
//! another source answered), and after 429/403 none before the back-off (`Retry-After`, growing
//! with consecutive refusals) has passed. While the source rests, its last answer stands in for
//! it, with its original `measured_at`.
//!
//! An expired token (Claude Code not run for a while) or no token in the file (Claude Code
//! keeping it in Credential Manager) skips the OAuth source without a request and without
//! `SignInRequired`; when no other source answers, an expired token is the reported error
//! (`Auth`: open Claude Code to renew it).

mod json;

pub mod blockfile;
pub mod cache;
pub mod desktop;
pub mod desktop_cache;
pub mod oauth;
pub mod response_info;

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::consts::{STALE_MIN, UNDOCUMENTED_MIN_INTERVAL};
use lastdrop_core::model::{
    AccountKey, AccountRef, Health, Meter, MeterId, MeterKind, MeterNames, ProviderId, Snapshot,
    SourceKind, WindowLen, catalog,
};
use lastdrop_core::refresh::{CredentialFingerprint, backoff};
use lastdrop_core::settings::ClaudeOptions;

use crate::account::AccountSource;
use crate::error::FetchError;
use crate::io::TerminalCommand;
use crate::{FetchContext, Provider};

/// A Claude usage window as the service names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ClaudeWindow {
    /// The 5-hour session (`five_hour`): `5h` / `5-hour` / `5-hour session`, L 5 h, starts on use.
    FiveHour,
    /// Weekly, all models (`weekly`): `Wk` / `weekly` / `Weekly · all models`, L 7 d.
    Weekly,
    /// Weekly for one model (`weekly:<model>`): `<Model>` / `<Model> weekly` /
    /// `Weekly · <Model>`, L 7 d. The model's display name as the service gives it (`Fable`).
    WeeklyModel(String),
}

/// One window reading from any Claude source.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowReading {
    /// Which window.
    pub window: ClaudeWindow,
    /// Used share (`utilization / 100`), ≥ 0. A window whose utilization is missing is omitted
    /// by the parser, never reported as 0.
    pub used: f64,
    /// `resets_at`; `None` when the service sends null (a 5-hour window between sessions, an
    /// unused per-model weekly).
    pub resets_at: Option<Timestamp>,
}

/// What a Claude source returned.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClaudeUsage {
    /// The windows read.
    pub windows: Vec<WindowReading>,
    /// Plan name when the source says (`Max`, `Pro`).
    pub plan: Option<String>,
    /// When the service computed the values, when the source says.
    pub measured_at: Option<Timestamp>,
}

/// Claude's usage page on claude.ai (Settings → Usage), opened by Enter on the flyout card (F§9).
pub const USAGE_PAGE: &str = "https://claude.ai/settings/usage";

/// Slack on the OAuth source's own 5-minute minimum, so a fetch the scheduler starts exactly
/// `min_interval` after the previous one is not skipped because the previous request went out a
/// moment after its fetch started (**not from the spec**).
pub const OAUTH_MIN_GAP_SLACK: Duration = Duration::from_secs(30);

/// Usage slack when taking a reset time from an older reading of the same window: one percentage
/// point, because the desktop history stores whole percentages (**not from the spec**).
const RESET_FILL_TOLERANCE: f64 = 0.01;

/// Readings of different sources less than this apart count as the same moment, and the earlier
/// source in the order wins (**not from the spec**): Claude Desktop writes a history sample when
/// it receives the usage response its HTTP cache keeps, moments apart, and the cached response
/// has the reset times and per-model windows the sample lacks.
pub const SAME_READING: SignedDuration = SignedDuration::from_mins(2);

/// The OAuth source's memory for one account (see the module docs).
#[derive(Clone, Debug, Default)]
struct OAuthMemory {
    /// The credentials fingerprint this memory belongs to.
    fingerprint: Option<CredentialFingerprint>,
    /// The endpoint answered 401 with this credential: skip until it changes.
    rejected: bool,
    /// No request before this instant: the 5-minute minimum, or the back-off after 429/403.
    rest_until: Option<Timestamp>,
    /// Consecutive 429/403 answers.
    throttled: u32,
    /// The last request's result, standing in for the source while it rests.
    last: Option<Result<ClaudeUsage, FetchError>>,
    /// The last successful answer (with its `measured_at`), kept apart from `last`: after a
    /// 429/5xx `last` holds the error, and the desktop history (which has no reset times) still
    /// needs the reset times of the current windows from somewhere ([`fill_resets`]).
    last_good: Option<ClaudeUsage>,
}

impl OAuthMemory {
    /// The memory for `fingerprint`: a changed credential lifts a 401 stop and forgets the last
    /// answer and the 5-minute rest (it is a new credential), but keeps a 429/403 back-off (the
    /// service throttles the account, which a renewed token does not change).
    fn for_credential(self, fingerprint: Option<&CredentialFingerprint>) -> OAuthMemory {
        if self.fingerprint.as_ref() == fingerprint {
            return self;
        }
        OAuthMemory {
            fingerprint: fingerprint.cloned(),
            rejected: false,
            rest_until: if self.throttled > 0 {
                self.rest_until
            } else {
                None
            },
            throttled: self.throttled,
            last: None,
            last_good: None,
        }
    }
}

/// What the OAuth source gave in one fetch.
struct OAuthOutcome {
    result: Result<ClaudeUsage, FetchError>,
    /// The endpoint rejected the token (now or remembered): `Health::SignInRequired`.
    rejected: bool,
    /// The plan from the credentials file, when it was read.
    plan: Option<String>,
    /// The source's last successful answer, a donor of reset times when `result` is an error.
    last_good: Option<ClaudeUsage>,
    /// What the log adds to the result: why no request was sent (expired token, a remembered
    /// 401, resting) or that the endpoint rejected the token.
    note: Option<String>,
}

/// A successful reading of one source.
struct Candidate {
    source: SourceKind,
    usage: ClaudeUsage,
    measured_at: Timestamp,
}

/// The Claude provider.
#[derive(Clone, Debug)]
pub struct ClaudeProvider {
    options: ClaudeOptions,
    /// Per-account memory of the OAuth source, shared by clones (it describes the credential,
    /// not this value).
    oauth: Arc<Mutex<BTreeMap<AccountKey, OAuthMemory>>>,
}

impl ClaudeProvider {
    /// A provider with the user's source options.
    pub fn new(options: ClaudeOptions) -> ClaudeProvider {
        ClaudeProvider {
            options,
            oauth: Arc::default(),
        }
    }

    /// The source options in use.
    pub fn options(&self) -> &ClaudeOptions {
        &self.options
    }

    fn memories(&self) -> MutexGuard<'_, BTreeMap<AccountKey, OAuthMemory>> {
        self.oauth.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Source 2 with this provider's memory (module docs). `started` is when the fetch began.
    fn oauth_source(
        &self,
        ctx: &FetchContext<'_>,
        key: &AccountKey,
        config_dir: &Path,
        started: Timestamp,
    ) -> OAuthOutcome {
        let fingerprint = fingerprint_of(ctx, config_dir);
        let memory = self
            .memories()
            .get(key)
            .cloned()
            .unwrap_or_default()
            .for_credential(fingerprint.as_ref());
        let (mut outcome, memory) = self.oauth_attempt(ctx, config_dir, started, memory);
        outcome.last_good.clone_from(&memory.last_good);
        self.memories().insert(key.clone(), memory);
        outcome
    }

    fn oauth_attempt(
        &self,
        ctx: &FetchContext<'_>,
        config_dir: &Path,
        started: Timestamp,
        mut memory: OAuthMemory,
    ) -> (OAuthOutcome, OAuthMemory) {
        let from = SourceKind::ClaudeOAuthUsage;
        let outcome = |result, rejected, plan| OAuthOutcome {
            result,
            rejected,
            plan,
            last_good: None,
            note: None,
        };
        let noted = |mut outcome: OAuthOutcome, note: String| {
            outcome.note = Some(note);
            outcome
        };
        let credentials = match oauth::read_credentials(ctx, config_dir) {
            Ok(credentials) => credentials,
            Err(e) => return (outcome(Err(e), false, None), memory),
        };
        let plan = credentials.plan.clone();
        if memory.rejected {
            let note = "the token was rejected (HTTP 401); waiting for Claude Code to renew it";
            let rejected = outcome(Err(FetchError::Auth { from }), true, plan);
            return (noted(rejected, note.to_owned()), memory);
        }
        let now = ctx.clock.now();
        let token = match credentials.usable_token(now) {
            Ok(token) => token,
            Err(e @ FetchError::Auth { .. }) => {
                let note = "Claude Code's token has expired; Claude Code renews it when it next \
                            runs";
                return (noted(outcome(Err(e), false, plan), note.to_owned()), memory);
            }
            Err(e) => return (outcome(Err(e), false, plan), memory),
        };
        if let Some(until) = memory.rest_until.filter(|until| now < *until) {
            let note = if memory.throttled > 0 {
                format!("backing off after HTTP 429/403 until {until}")
            } else {
                format!("resting until {until} (at most one request per 5 minutes)")
            };
            let retry_after = Duration::try_from(until.duration_since(now)).ok();
            let result = memory.last.clone().unwrap_or(Err(if memory.throttled > 0 {
                FetchError::RateLimited { from, retry_after }
            } else {
                FetchError::Transient {
                    from,
                    detail: "waiting for the 5-minute minimum".to_owned(),
                    retry_after,
                }
            }));
            return (noted(outcome(result, false, plan), note), memory);
        }

        if ctx.remaining().is_zero() {
            // No request, so no rest either.
            let timed_out = FetchError::Transient {
                from,
                detail: "timed out".to_owned(),
                retry_after: None,
            };
            return (outcome(Err(timed_out), false, plan), memory);
        }
        let mut result = oauth::send(ctx, &token);
        drop(token);
        let answered = ctx.clock.now();
        if let Ok(usage) = &mut result {
            usage.plan.clone_from(&plan);
            let mut good = usage.clone();
            good.measured_at.get_or_insert(answered);
            memory.last_good = Some(good);
        }
        memory.rejected = matches!(result, Err(FetchError::Auth { .. }));
        if let Err(FetchError::RateLimited { retry_after, .. }) = &result {
            memory.throttled = memory.throttled.saturating_add(1);
            let wait = backoff(memory.throttled, *retry_after, UNDOCUMENTED_MIN_INTERVAL);
            memory.rest_until = Some(add(answered, wait));
        } else {
            memory.throttled = 0;
            let gap = UNDOCUMENTED_MIN_INTERVAL.saturating_sub(OAUTH_MIN_GAP_SLACK);
            memory.rest_until = Some(add(started, gap));
        }
        memory.last = Some(result.clone());
        let rejected = memory.rejected;
        let sent = outcome(result, rejected, plan);
        if rejected {
            let note = "the endpoint rejected the token (HTTP 401)".to_owned();
            return (noted(sent, note), memory);
        }
        (sent, memory)
    }
}

impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::Claude {
            account: key,
            config_dir,
            state_file,
            desktop_dir,
        } = account
        else {
            return Err(FetchError::Unavailable {
                detail: "not a Claude account".to_owned(),
            });
        };
        let options = &self.options;
        if !(options.oauth_usage
            || options.desktop_cache
            || options.desktop_history
            || options.cached_usage)
        {
            return Err(FetchError::Unavailable {
                detail: "every Claude source is turned off".to_owned(),
            });
        }
        let started = ctx.clock.now();
        let mut found = Readings::default();
        let mut rejected = false;
        let mut plan = None;
        let no_desktop = || FetchError::Unavailable {
            detail: "Claude Desktop's data folder not found".to_owned(),
        };

        // Older answers that only lend reset times (module docs, "Reset times").
        let mut donors = Vec::new();
        if options.oauth_usage {
            let outcome = self.oauth_source(ctx, key, config_dir, started);
            rejected = outcome.rejected;
            plan = outcome.plan;
            if outcome.result.is_err()
                && let Some(good) = outcome.last_good
                && let Some(measured_at) = good.measured_at
            {
                donors.push(Candidate {
                    source: SourceKind::ClaudeOAuthUsage,
                    usage: good,
                    measured_at,
                });
            }
            found.collect(
                ctx,
                SourceKind::ClaudeOAuthUsage,
                outcome.result,
                outcome.note,
            );
        }
        if options.desktop_cache {
            let (result, note) = match desktop_dir {
                Some(dir) => match desktop_cache::read_detailed(ctx, dir) {
                    Ok(cached) => {
                        let note = cached.note();
                        (Ok(cached.usage), Some(note))
                    }
                    Err(e) => (Err(e), None),
                },
                None => (Err(no_desktop()), None),
            };
            found.collect(ctx, SourceKind::ClaudeDesktopCache, result, note);
        }
        if options.desktop_history {
            let result = match desktop_dir {
                Some(dir) => desktop::read(ctx, dir),
                None => Err(no_desktop()),
            };
            found.collect(ctx, SourceKind::ClaudeDesktopHistory, result, None);
        }
        if options.cached_usage {
            let result = cache::read(ctx, state_file);
            found.collect(ctx, SourceKind::ClaudeCodeCache, result, None);
        }
        let Readings {
            mut readings,
            failures,
        } = found;

        let fetched_at = ctx.clock.now();
        let Some(best) = freshest(&readings) else {
            return Err(most_telling(failures));
        };
        let mut chosen = readings.swap_remove(best);
        readings.append(&mut donors);
        fill_resets(&mut chosen, &readings);
        let account_ref = AccountRef {
            key: key.clone(),
            alias: None,
            tag: None,
            plan,
        };
        let mut snapshot = to_snapshot(&chosen.usage, chosen.source, account_ref, fetched_at);
        let age = fetched_at.duration_since(snapshot.measured_at);
        snapshot.health = if rejected {
            Health::SignInRequired
        } else if chosen.source != SourceKind::ClaudeOAuthUsage && age > STALE_MIN {
            Health::Stale
        } else {
            Health::Ok
        };
        log::info!(
            "claude: using {:?}, measured {} ({}), {} meters, health {:?}",
            snapshot.source,
            snapshot.measured_at,
            age_text(age),
            snapshot.meters.len(),
            snapshot.health
        );
        Ok(snapshot)
    }

    /// 5 minutes: the OAuth usage endpoint is undocumented (ARCHITECTURE).
    fn min_interval(&self) -> Duration {
        UNDOCUMENTED_MIN_INTERVAL
    }

    /// Size + mtime of `<config_dir>\.credentials.json`; `absent` while the file does not exist
    /// (so its appearance lifts an auth stop); `None` when the metadata cannot be read or the
    /// account is not a Claude account.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        match account {
            AccountSource::Claude { config_dir, .. } => fingerprint_of(ctx, config_dir),
            _ => None,
        }
    }

    /// `claude` in a terminal with `CLAUDE_CONFIG_DIR` set to the account's folder (the user
    /// signs in with `/login`). For the default folder (`~\.claude` with `~\.claude.json`) the
    /// variable is removed instead: set explicitly, Claude Code would read
    /// `~\.claude\.claude.json` rather than `~\.claude.json`, a different account state.
    fn sign_in_command(&self, account: &AccountSource) -> Option<TerminalCommand> {
        let AccountSource::Claude {
            config_dir,
            state_file,
            ..
        } = account
        else {
            return None;
        };
        let custom = *state_file == config_dir.join(".claude.json");
        let value = custom.then(|| config_dir.clone().into_os_string());
        Some(TerminalCommand {
            program: "claude".into(),
            args: Vec::new(),
            env: vec![("CLAUDE_CONFIG_DIR".into(), value)],
            cwd: None,
        })
    }

    /// claude.ai Settings → Usage ([`USAGE_PAGE`]).
    fn usage_page(&self) -> Option<&'static str> {
        Some(USAGE_PAGE)
    }
}

/// Size + mtime of the credentials file (module docs of [`Provider::credential_fingerprint`]).
fn fingerprint_of(ctx: &FetchContext<'_>, config_dir: &Path) -> Option<CredentialFingerprint> {
    match ctx.fs.metadata(&oauth::credentials_path(config_dir)) {
        Ok(meta) => Some(CredentialFingerprint(format!(
            "{}:{}",
            meta.len,
            meta.modified.map_or(0, Timestamp::as_millisecond)
        ))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Some(CredentialFingerprint("absent".to_owned()))
        }
        Err(_) => None,
    }
}

/// The sources' results of one fetch.
#[derive(Default)]
struct Readings {
    /// Successes, in source order.
    readings: Vec<Candidate>,
    /// Failures, in source order, for [`most_telling`].
    failures: Vec<FetchError>,
}

impl Readings {
    /// Records one source's result: a success becomes a candidate (`measured_at` defaults to
    /// now), a failure is kept for [`most_telling`]. Logged at INFO either way, without secrets
    /// or ids (module docs, "Logging"); `note` says more than the result does (why the OAuth
    /// source sent no request, the cache's content coding).
    fn collect(
        &mut self,
        ctx: &FetchContext<'_>,
        source: SourceKind,
        result: Result<ClaudeUsage, FetchError>,
        note: Option<String>,
    ) {
        let note = note.map(|n| format!("; {n}")).unwrap_or_default();
        match result {
            Ok(mut usage) => {
                let now = ctx.clock.now();
                let measured_at = *usage.measured_at.get_or_insert(now);
                log::info!(
                    "claude: {source:?}: read {} windows measured {measured_at} ({}){note}",
                    usage.windows.len(),
                    age_text(now.duration_since(measured_at))
                );
                self.readings.push(Candidate {
                    source,
                    usage,
                    measured_at,
                });
            }
            Err(e) => {
                log::info!("claude: {source:?}: skipped: {}{note}", reason(&e));
                self.failures.push(e);
            }
        }
    }
}

/// Why a source gave nothing, for the log: the error's own detail, without the source's name
/// (the log line has it).
fn reason(error: &FetchError) -> String {
    match error {
        FetchError::Auth { .. } => "sign-in expired or rejected".to_owned(),
        FetchError::RateLimited {
            retry_after: Some(wait),
            ..
        } => format!("rate limited, retry after {} s", wait.as_secs()),
        FetchError::RateLimited { .. } => "rate limited".to_owned(),
        FetchError::Offline { .. } => "offline".to_owned(),
        FetchError::Transient { detail, .. } => format!("failed for now: {detail}"),
        FetchError::Parse { detail, .. } => format!("unreadable: {detail}"),
        FetchError::Unavailable { detail } => detail.clone(),
    }
}

/// A reading's age for the log: `40 s old`, `12 min old`, `3 h 5 min old`, `2 d 4 h old`.
fn age_text(age: SignedDuration) -> String {
    let secs = age.as_secs();
    if secs < 0 {
        return "dated in the future".to_owned();
    }
    let (mins, hours, days) = (secs / 60, secs / 3600, secs / 86_400);
    if secs < 60 {
        format!("{secs} s old")
    } else if mins < 120 {
        format!("{mins} min old")
    } else if hours < 48 {
        format!("{hours} h {} min old", mins % 60)
    } else {
        format!("{days} d {} h old", hours % 24)
    }
}

/// The index of the most recently measured reading. A reading counts as newer only when it is
/// [`SAME_READING`] or more after the best so far, so of readings of one moment the earlier
/// source in the order wins.
fn freshest(readings: &[Candidate]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, reading) in readings.iter().enumerate() {
        let newer = |b: usize| {
            let same_until = readings[b]
                .measured_at
                .checked_add(SAME_READING)
                .unwrap_or(Timestamp::MAX);
            reading.measured_at >= same_until
        };
        if best.is_none_or(newer) {
            best = Some(i);
        }
    }
    best
}

/// Fills the chosen reading's unknown reset times from older readings of the same window
/// instance (module docs, "Reset times"); the newest such reading wins.
fn fill_resets(chosen: &mut Candidate, others: &[Candidate]) {
    let at = chosen.measured_at;
    for window in chosen
        .usage
        .windows
        .iter_mut()
        .filter(|w| w.resets_at.is_none())
    {
        let found = others
            .iter()
            .filter(|other| other.measured_at <= at)
            .filter_map(|other| {
                let seen = other
                    .usage
                    .windows
                    .iter()
                    .find(|w| w.window == window.window)?;
                let resets_at = seen.resets_at?;
                let same_instance =
                    resets_at > at && seen.used <= window.used + RESET_FILL_TOLERANCE;
                same_instance.then_some((other.measured_at, resets_at))
            })
            .max_by_key(|(measured_at, _)| *measured_at);
        if let Some((_, resets_at)) = found {
            window.resets_at = Some(resets_at);
        }
    }
}

/// The failure to report when no source answered: an auth error over a throttle over offline
/// over a temporary failure over a parse error over "not found"; the earlier source on a tie.
fn most_telling(failures: Vec<FetchError>) -> FetchError {
    let rank = |e: &FetchError| match e {
        FetchError::Auth { .. } => 5,
        FetchError::RateLimited { .. } => 4,
        FetchError::Offline { .. } => 3,
        FetchError::Transient { .. } => 2,
        FetchError::Parse { .. } => 1,
        FetchError::Unavailable { .. } => 0,
    };
    let mut best: Option<FetchError> = None;
    for failure in failures {
        if best.as_ref().is_none_or(|b| rank(&failure) > rank(b)) {
            best = Some(failure);
        }
    }
    best.unwrap_or_else(|| FetchError::Unavailable {
        detail: "no Claude data source".to_owned(),
    })
}

/// A file error of a source: not found → Unavailable; over the size limit → Parse; anything else
/// (locked, denied) → Transient. Details name the file, never its path (which holds the user
/// name).
pub(crate) fn file_error(from: SourceKind, error: &io::Error, name: &str) -> FetchError {
    match error.kind() {
        io::ErrorKind::NotFound => FetchError::Unavailable {
            detail: format!("{name} not found"),
        },
        io::ErrorKind::InvalidData => FetchError::Parse {
            from,
            detail: format!("{name} is larger than expected"),
        },
        _ => FetchError::Transient {
            from,
            detail: format!("cannot read {name}"),
            retry_after: None,
        },
    }
}

/// `t + d`, saturating at jiff's range.
fn add(t: Timestamp, d: Duration) -> Timestamp {
    SignedDuration::try_from(d)
        .ok()
        .and_then(|d| t.checked_add(d).ok())
        .unwrap_or(Timestamp::MAX)
}

/// The R§2 catalogue entry of a Claude meter that every reading can have.
fn catalogue(id: &str) -> (MeterNames, WindowLen, bool) {
    let def = catalog(ProviderId::Claude)
        .iter()
        .find(|d| d.id == id)
        .expect("R§2: the Claude catalogue has five_hour and weekly");
    match def.kind {
        MeterKind::Window {
            window,
            starts_on_use,
        } => (def.names(), window, starts_on_use),
        MeterKind::Balance => unreachable!("R§2: Claude's catalogue meters are windows"),
    }
}

/// The meter id part of a model's display name: lower case, runs of other characters as `-`
/// (`Fable` → `fable`, `Opus 4.5` → `opus-4-5`).
fn model_slug(model: &str) -> String {
    let mut slug = String::new();
    for c in model.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        slug.push_str("model");
    }
    slug
}

/// A per-model weekly window's names (R§2); a display name longer than the readout's 5-character
/// label is shortened there to 4 characters and `…` (`Sonnet` → `Sonn…`).
fn model_names(model: &str) -> MeterNames {
    let mut names = MeterNames::weekly_model(model);
    if model.chars().count() > 5 {
        names.short_label = model.chars().take(4).chain(['…']).collect();
    }
    names
}

/// The meter of a window reading with the R§2 definition (id, names, `L`, `starts_on_use`,
/// order: 5-hour 0, weekly 1, per-model weekly 2… in the service's order). Names and windows of
/// `five_hour` and `weekly` come from `lastdrop_core::model::catalog`, per-model names from
/// `MeterNames::weekly_model`, so the readout's LOADING rows and widths match the readings.
pub fn meter(reading: &WindowReading, order: u16) -> Meter {
    let (id, names, window, starts_on_use) = match &reading.window {
        ClaudeWindow::FiveHour => {
            let (names, window, starts_on_use) = catalogue(MeterId::FIVE_HOUR);
            (
                MeterId::new(MeterId::FIVE_HOUR),
                names,
                window,
                starts_on_use,
            )
        }
        ClaudeWindow::Weekly => {
            let (names, window, starts_on_use) = catalogue(MeterId::WEEKLY);
            (MeterId::new(MeterId::WEEKLY), names, window, starts_on_use)
        }
        ClaudeWindow::WeeklyModel(model) => (
            MeterId::weekly_model(&model_slug(model)),
            model_names(model),
            WindowLen::Fixed(SignedDuration::from_hours(7 * 24)),
            false,
        ),
    };
    Meter {
        id,
        names,
        order,
        used: if reading.used.is_finite() {
            reading.used.max(0.0)
        } else {
            0.0
        },
        limited: false,
        resets_at: reading.resets_at,
        window,
        starts_on_use,
    }
}

/// The snapshot of a usage reading from `source`.
///
/// `measured_at` is the reading's own time (never later than `fetched_at`), else `fetched_at`.
/// Meters in R§2 order: 5-hour 0, weekly 1, per-model weekly 2, 3, … in the reading's order;
/// a window that appears twice is kept once. The plan is the reading's when it names one, else
/// `account`'s. Health is `Ok`; [`ClaudeProvider`] adjusts it.
pub fn to_snapshot(
    usage: &ClaudeUsage,
    source: SourceKind,
    account: AccountRef,
    fetched_at: Timestamp,
) -> Snapshot {
    let mut meters: Vec<Meter> = Vec::new();
    let mut next_model_order = 2;
    for reading in &usage.windows {
        let order = match reading.window {
            ClaudeWindow::FiveHour => 0,
            ClaudeWindow::Weekly => 1,
            ClaudeWindow::WeeklyModel(_) => next_model_order,
        };
        let meter = meter(reading, order);
        if meters.iter().any(|m| m.id == meter.id) {
            continue;
        }
        if matches!(reading.window, ClaudeWindow::WeeklyModel(_)) {
            next_model_order = next_model_order.saturating_add(1);
        }
        meters.push(meter);
    }
    meters.sort_by_key(|m| m.order);
    let mut account = account;
    if let Some(plan) = &usage.plan {
        account.plan = Some(plan.clone());
    }
    Snapshot {
        provider: ProviderId::Claude,
        account,
        source,
        measured_at: usage.measured_at.map_or(fetched_at, |m| m.min(fetched_at)),
        fetched_at,
        meters,
        balances: Vec::new(),
        health: Health::Ok,
    }
}
