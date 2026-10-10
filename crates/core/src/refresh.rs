//! The refresh scheduling policy, pure (ARCHITECTURE "Refresh scheduler", R§3.9, F§3).
//!
//! Per credential source: never more often than the provider's `min_interval` (undocumented
//! endpoints: 5 minutes); the user's interval, or manual; adaptive (faster while the flyout is
//! open, slower when idle); poll again shortly after a window's reset time (R§3.9); back off
//! after errors and HTTP 429/403, honouring `Retry-After`; stop after an auth error (HTTP 401)
//! until the credential changes. One job per source in flight. The app's scheduler arms one
//! timer for the earliest [`Due::At`] and runs jobs on its worker threads.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::settings::RefreshInterval;
use crate::stats::FailureKind;

/// An opaque fingerprint of a credential (file size + modification time of a token file, or a
/// Credential Manager `LastWritten`), never the secret itself. A changed fingerprint lifts an
/// auth stop (ARCHITECTURE: "stop on 401 until the credential file changes").
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CredentialFingerprint(pub String);

/// What happened to the last attempt of a source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AttemptResult {
    /// A snapshot was read.
    Success,
    /// The attempt failed.
    Failure {
        /// Failure class (R§3.8).
        kind: FailureKind,
        /// The service asked to wait (HTTP `Retry-After`), if it said.
        retry_after: Option<Duration>,
        /// The service rate-limited or refused (HTTP 429/403): back off even when `kind` is
        /// `NoData`.
        throttled: bool,
        /// For `kind == Auth`: the credential fingerprint at the time, so a change can be
        /// detected.
        credential: Option<CredentialFingerprint>,
    },
}

/// The scheduler's memory of one credential source.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SourceState {
    /// Start of the last attempt.
    pub last_attempt: Option<Timestamp>,
    /// `fetched_at` of the last success.
    pub last_success: Option<Timestamp>,
    /// Consecutive failures since the last success.
    pub consecutive_failures: u32,
    /// No attempt before this instant (back-off, `Retry-After`).
    pub backoff_until: Option<Timestamp>,
    /// Stopped after an auth error until the credential fingerprint differs from this one.
    pub auth_stopped: Option<CredentialFingerprint>,
    /// A job for this source is running (one job per source in flight).
    pub in_flight: bool,
}

/// Adaptive refresh inputs (ARCHITECTURE "adaptive (faster while the flyout is open, slower when
/// idle)").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Activity {
    /// The flyout is open.
    pub flyout_open: bool,
    /// The user is idle (no input for a while, session locked, display off).
    pub user_idle: bool,
}

/// Everything [`next_due`] reads for one source.
#[derive(Clone, Debug, PartialEq)]
pub struct PolicyInput<'a> {
    /// The clock.
    pub now: Timestamp,
    /// The provider's lower bound between fetches of one credential source.
    pub min_interval: Duration,
    /// The interval in effect for this service (its own, else the global one).
    pub interval: RefreshInterval,
    /// Adaptive refresh on.
    pub adaptive: bool,
    /// Current activity.
    pub activity: Activity,
    /// Updates paused (settings `pause`, evaluated at `now`).
    pub paused: bool,
    /// Upcoming resets of the source's meters (R§3.9).
    pub resets: &'a [Timestamp],
    /// Uniform random value in `[0, 1)` drawn by the caller for the reset jitter
    /// (`RESET_REFRESH_JITTER` 30–90 s), keeping this function pure.
    pub jitter_unit: f64,
    /// The credential's fingerprint as a worker last reported it (after a fetch or a probe), if
    /// the provider can tell. Never read on the main thread.
    pub credential: Option<&'a CredentialFingerprint>,
    /// The source's state.
    pub state: &'a SourceState,
}

/// When a source should be fetched next.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Due {
    /// At this instant (in the past = now).
    At(Timestamp),
    /// A job is running; decide again when it finishes.
    InFlight,
    /// Stopped after an auth error: at this instant (every effective interval) a worker only
    /// re-reads the credential's fingerprint — a file's size and mtime, no request to the
    /// service — and fetches only when it differs from [`SourceState::auth_stopped`]. Fingerprints
    /// are read on workers because the main thread does no I/O (ARCHITECTURE "Threads"); without
    /// probes an auth stop could never lift by itself ("stop on 401 until the credential file
    /// changes"). Sources whose provider has no fingerprint (Meshy) get [`Due::Never`] instead and
    /// are lifted by the settings window storing a new key.
    ProbeCredential(Timestamp),
    /// Not automatically: manual mode, paused, or auth-stopped without a fingerprint.
    Never,
}

/// The poll interval in effect (ARCHITECTURE "Refresh scheduler"): the user's interval, or
/// adaptive: `max(min_interval, ADAPTIVE_OPEN_INTERVAL)` while the flyout is open; interval ×
/// `ADAPTIVE_IDLE_FACTOR` (≤ `ADAPTIVE_IDLE_MAX`) while idle; never below `min_interval`. `None`
/// in manual mode. Also the `poll_interval` of R§3.8's `stale_after`.
pub fn effective_interval(
    interval: RefreshInterval,
    adaptive: bool,
    activity: Activity,
    min_interval: Duration,
) -> Option<SignedDuration> {
    todo!("M1: effective interval")
}

/// The next due time of one source: the earliest of (last attempt + effective interval) and the
/// next reset refresh (R§3.9, `reset_refresh_at`), then pushed back to `backoff_until` and to
/// `last_attempt + min_interval`. Auth-stopped: [`Due::At`] now when `input.credential` (the
/// latest fingerprint a worker reported) differs from the stopped one, else
/// [`Due::ProbeCredential`] (or [`Due::Never`] without a fingerprint). [`Due::Never`] in manual
/// mode (reset refreshes are skipped there too) and while paused.
pub fn next_due(input: &PolicyInput<'_>) -> Due {
    todo!("M1: next due")
}

/// Whether a manual refresh ("Refresh now", F5, `Retry`) may run now; otherwise the source keeps
/// its timer (F§3 "a service that cannot be refreshed yet keeps its timer"). Allowed when not in
/// flight, `now ≥ last_attempt + min_interval` and `now ≥ backoff_until`; an auth stop is lifted
/// by a manual refresh.
pub fn manual_allowed(state: &SourceState, now: Timestamp, min_interval: Duration) -> bool {
    todo!("M1: manual refresh")
}

/// Back-off after `consecutive_failures` failures: `BACKOFF_INITIAL × 2^(n−1)` capped at
/// `BACKOFF_MAX`, never shorter than `retry_after` or `min_interval` (**numbers not from the
/// spec**, see `consts`).
pub fn backoff(
    consecutive_failures: u32,
    retry_after: Option<Duration>,
    min_interval: Duration,
) -> Duration {
    todo!("M1: back-off")
}

/// Updates `state` with the result of an attempt that started at `started` and ended at `now`:
/// success resets the failure count and the back-off; a failure counts up and sets
/// `backoff_until`; an auth failure records `auth_stopped`.
pub fn record(
    state: &mut SourceState,
    started: Timestamp,
    now: Timestamp,
    result: &AttemptResult,
    min_interval: Duration,
) {
    todo!("M1: record attempt")
}
