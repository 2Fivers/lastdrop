//! The refresh scheduling policy, pure (ARCHITECTURE "Refresh scheduler", R§3.9, F§3).
//!
//! Per credential source: never more often than the provider's `min_interval` (undocumented
//! endpoints: 5 minutes); the user's interval, or manual; adaptive (faster while the flyout is
//! open, slower when idle); poll again shortly after a window's reset time (R§3.9); back off
//! after errors and HTTP 429/403, honouring `Retry-After`; stop after an auth error (HTTP 401)
//! until the credential changes. One job per source in flight. The app's scheduler arms one
//! timer for the earliest [`Due::At`] and runs jobs on its worker threads.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::consts::{
    ADAPTIVE_IDLE_FACTOR, ADAPTIVE_IDLE_MAX, ADAPTIVE_OPEN_INTERVAL, BACKOFF_INITIAL, BACKOFF_MAX,
    REFRESH_MINUTES_RANGE, RESET_REFRESH_JITTER_MAX, RESET_REFRESH_JITTER_MIN,
};
use crate::settings::RefreshInterval;
use crate::stats::{FailureKind, reset_refresh_at};
use crate::util::add;

/// A std duration as a signed duration, saturating (std durations can exceed jiff's range).
fn signed(d: Duration) -> SignedDuration {
    SignedDuration::try_from(d).unwrap_or(SignedDuration::MAX)
}

/// `RESET_REFRESH_JITTER` drawn from the caller's uniform `unit` in `[0, 1)` (R§3.9: 30–90 s).
fn reset_jitter(unit: f64) -> SignedDuration {
    let unit = if unit.is_finite() {
        unit.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let min = RESET_REFRESH_JITTER_MIN.as_secs_f64();
    let span = RESET_REFRESH_JITTER_MAX.as_secs_f64() - min;
    SignedDuration::try_from_secs_f64(min + span * unit).unwrap_or(RESET_REFRESH_JITTER_MIN)
}

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
    let RefreshInterval::Minutes(minutes) = interval else {
        return None;
    };
    let (lo, hi) = REFRESH_MINUTES_RANGE;
    let base = Duration::from_secs(u64::from(minutes.clamp(lo, hi)) * 60);
    let chosen = if !adaptive {
        base
    } else if activity.flyout_open {
        ADAPTIVE_OPEN_INTERVAL
    } else if activity.user_idle {
        base.saturating_mul(ADAPTIVE_IDLE_FACTOR)
            .min(ADAPTIVE_IDLE_MAX)
            .max(base)
    } else {
        base
    };
    Some(signed(chosen.max(min_interval)))
}

/// The next due time of one source: the earliest of (last attempt + effective interval) and the
/// next reset refresh (R§3.9, `reset_refresh_at`), then pushed back to `backoff_until` and to
/// `last_attempt + min_interval`. Auth-stopped: [`Due::At`] now when `input.credential` (the
/// latest fingerprint a worker reported) differs from the stopped one, else
/// [`Due::ProbeCredential`] (or [`Due::Never`] without a fingerprint). [`Due::Never`] in manual
/// mode (reset refreshes are skipped there too) and while paused.
pub fn next_due(input: &PolicyInput<'_>) -> Due {
    let state = input.state;
    if state.in_flight {
        return Due::InFlight;
    }
    if input.paused {
        return Due::Never;
    }
    let Some(interval) = effective_interval(
        input.interval,
        input.adaptive,
        input.activity,
        input.min_interval,
    ) else {
        return Due::Never;
    };
    let min_interval = signed(input.min_interval);
    let not_before_min = |t: Timestamp| match state.last_attempt {
        Some(last) => t.max(add(last, min_interval)),
        None => t,
    };

    if let Some(stopped) = &state.auth_stopped {
        return match input.credential {
            // A new credential is fetched at once: the minimum interval is per credential.
            Some(current) if current != stopped => Due::At(input.now),
            Some(_) => {
                // Probe on the interval's grid from the last attempt, the next point after `now`
                // (at least one interval after the failure), so probes neither stop nor spin.
                let at = match state.last_attempt {
                    Some(last) => {
                        let step = interval.as_secs_f64().max(1.0);
                        let behind = input.now.duration_since(last).as_secs_f64() / step;
                        // `behind` is finite; the cast saturates for absurd clocks.
                        let k = (behind.ceil() as i32).max(1);
                        add(last, interval.checked_mul(k).unwrap_or(SignedDuration::MAX))
                    }
                    None => input.now,
                };
                Due::ProbeCredential(at)
            }
            None => Due::Never,
        };
    }

    let mut due = match state.last_attempt {
        Some(last) => add(last, interval),
        None => input.now,
    };
    let jitter = reset_jitter(input.jitter_unit);
    for &reset in input.resets {
        // A reset that the last attempt already followed needs no extra refresh.
        if state.last_attempt.is_some_and(|last| last >= reset) {
            continue;
        }
        let at = reset_refresh_at(reset, jitter, state.last_attempt, min_interval);
        due = due.min(at);
    }
    if let Some(until) = state.backoff_until {
        due = due.max(until);
    }
    Due::At(not_before_min(due))
}

/// Whether a manual refresh ("Refresh now", F5, `Retry`) may run now; otherwise the source keeps
/// its timer (F§3 "a service that cannot be refreshed yet keeps its timer"). Allowed when not in
/// flight, `now ≥ last_attempt + min_interval` and `now ≥ backoff_until`; an auth stop is lifted
/// by a manual refresh.
pub fn manual_allowed(state: &SourceState, now: Timestamp, min_interval: Duration) -> bool {
    !state.in_flight
        && state
            .last_attempt
            .is_none_or(|last| now >= add(last, signed(min_interval)))
        && state.backoff_until.is_none_or(|until| now >= until)
}

/// Back-off after `consecutive_failures` failures: `BACKOFF_INITIAL × 2^(n−1)` capped at
/// `BACKOFF_MAX`, never shorter than `retry_after` or `min_interval` (**numbers not from the
/// spec**, see `consts`).
pub fn backoff(
    consecutive_failures: u32,
    retry_after: Option<Duration>,
    min_interval: Duration,
) -> Duration {
    let exponential = match consecutive_failures {
        0 => Duration::ZERO,
        n => {
            let doublings = (n - 1).min(31);
            BACKOFF_INITIAL
                .saturating_mul(1u32 << doublings)
                .min(BACKOFF_MAX)
        }
    };
    exponential
        .max(retry_after.unwrap_or(Duration::ZERO))
        .max(min_interval)
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
    state.last_attempt = Some(started);
    state.in_flight = false;
    match result {
        AttemptResult::Success => {
            state.last_success = Some(now);
            state.consecutive_failures = 0;
            state.backoff_until = None;
            state.auth_stopped = None;
        }
        AttemptResult::Failure {
            kind,
            retry_after,
            throttled: _,
            credential,
        } => {
            state.consecutive_failures = state.consecutive_failures.saturating_add(1);
            let wait = backoff(state.consecutive_failures, *retry_after, min_interval);
            state.backoff_until = Some(add(now, signed(wait)));
            if *kind == FailureKind::Auth {
                // Without a fingerprint the stop is lifted only by a manual refresh or a new key
                // (`Due::Never`); an empty fingerprint never equals a reported one.
                state.auth_stopped = Some(
                    credential
                        .clone()
                        .unwrap_or_else(|| CredentialFingerprint(String::new())),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN5: Duration = Duration::from_secs(300);

    fn ts(mins: i64) -> Timestamp {
        Timestamp::from_second(1_800_000_000 + mins * 60).expect("valid")
    }

    fn input<'a>(
        now: Timestamp,
        state: &'a SourceState,
        resets: &'a [Timestamp],
    ) -> PolicyInput<'a> {
        PolicyInput {
            now,
            min_interval: MIN5,
            interval: RefreshInterval::Minutes(5),
            adaptive: false,
            activity: Activity::default(),
            paused: false,
            resets,
            jitter_unit: 0.5,
            credential: None,
            state,
        }
    }

    #[test]
    fn effective_intervals() {
        let m = |n: u64| Some(SignedDuration::from_mins(n.try_into().expect("small")));
        let quiet = Activity::default();
        let open = Activity {
            flyout_open: true,
            user_idle: false,
        };
        let idle = Activity {
            flyout_open: false,
            user_idle: true,
        };
        let five = RefreshInterval::Minutes(5);
        assert_eq!(
            effective_interval(RefreshInterval::Manual, true, open, MIN5),
            None
        );
        assert_eq!(effective_interval(five, false, open, Duration::ZERO), m(5));
        assert_eq!(effective_interval(five, true, open, Duration::ZERO), m(1));
        assert_eq!(effective_interval(five, true, open, MIN5), m(5));
        assert_eq!(effective_interval(five, true, idle, Duration::ZERO), m(15));
        let twenty = RefreshInterval::Minutes(20);
        assert_eq!(
            effective_interval(twenty, true, idle, Duration::ZERO),
            m(30)
        );
        assert_eq!(
            effective_interval(twenty, true, quiet, Duration::ZERO),
            m(20)
        );
        // Out-of-range settings are clamped to 1–30 minutes.
        assert_eq!(
            effective_interval(RefreshInterval::Minutes(0), false, quiet, Duration::ZERO),
            m(1)
        );
        assert_eq!(
            effective_interval(RefreshInterval::Minutes(999), false, quiet, Duration::ZERO),
            m(30)
        );
    }

    #[test]
    fn due_after_the_interval_or_shortly_after_a_reset() {
        let fresh = SourceState::default();
        assert_eq!(next_due(&input(ts(0), &fresh, &[])), Due::At(ts(0)));
        let polled = SourceState {
            last_attempt: Some(ts(0)),
            ..SourceState::default()
        };
        assert_eq!(next_due(&input(ts(1), &polled, &[])), Due::At(ts(5)));
        // A reset at 3 min would refresh at 3 min + 60 s, but not before the 5-minute minimum.
        let resets = [ts(3)];
        assert_eq!(next_due(&input(ts(1), &polled, &resets)), Due::At(ts(5)));
        let long = SourceState {
            last_attempt: Some(ts(0)),
            ..SourceState::default()
        };
        let mut i = input(ts(1), &long, &[]);
        i.interval = RefreshInterval::Minutes(30);
        let resets = [ts(10)];
        i.resets = &resets;
        assert_eq!(
            next_due(&i),
            Due::At(add(ts(10), SignedDuration::from_secs(60)))
        );
        // A reset the last attempt already followed is skipped.
        let after = SourceState {
            last_attempt: Some(ts(11)),
            ..SourceState::default()
        };
        let mut i = input(ts(12), &after, &resets);
        i.interval = RefreshInterval::Minutes(30);
        assert_eq!(next_due(&i), Due::At(ts(41)));
    }

    #[test]
    fn paused_manual_in_flight_and_back_off() {
        let s = SourceState::default();
        let mut i = input(ts(0), &s, &[]);
        i.paused = true;
        assert_eq!(next_due(&i), Due::Never);
        let mut i = input(ts(0), &s, &[]);
        i.interval = RefreshInterval::Manual;
        assert_eq!(next_due(&i), Due::Never);
        let busy = SourceState {
            in_flight: true,
            ..SourceState::default()
        };
        assert_eq!(next_due(&input(ts(0), &busy, &[])), Due::InFlight);
        let backing_off = SourceState {
            last_attempt: Some(ts(0)),
            backoff_until: Some(ts(20)),
            ..SourceState::default()
        };
        assert_eq!(next_due(&input(ts(1), &backing_off, &[])), Due::At(ts(20)));
    }

    #[test]
    fn auth_stop_probes_until_the_credential_changes() {
        let old = CredentialFingerprint("size=10;mtime=1".to_owned());
        let new = CredentialFingerprint("size=12;mtime=2".to_owned());
        let stopped = SourceState {
            last_attempt: Some(ts(0)),
            auth_stopped: Some(old.clone()),
            ..SourceState::default()
        };
        let mut i = input(ts(1), &stopped, &[]);
        i.credential = Some(&old);
        assert_eq!(next_due(&i), Due::ProbeCredential(ts(5)));
        i.now = ts(5);
        assert_eq!(next_due(&i), Due::ProbeCredential(ts(5)));
        i.now = add(ts(5), SignedDuration::from_secs(1));
        assert_eq!(next_due(&i), Due::ProbeCredential(ts(10)));
        i.credential = Some(&new);
        i.now = ts(7);
        assert_eq!(next_due(&i), Due::At(ts(7)));
        i.credential = None;
        assert_eq!(next_due(&i), Due::Never);
    }

    #[test]
    fn manual_refresh_respects_the_minimum_and_back_off() {
        let s = SourceState {
            last_attempt: Some(ts(0)),
            ..SourceState::default()
        };
        assert!(!manual_allowed(&s, ts(4), MIN5));
        assert!(manual_allowed(&s, ts(5), MIN5));
        let backing_off = SourceState {
            backoff_until: Some(ts(30)),
            ..s.clone()
        };
        assert!(!manual_allowed(&backing_off, ts(10), MIN5));
        let stopped = SourceState {
            auth_stopped: Some(CredentialFingerprint(String::new())),
            ..s
        };
        assert!(manual_allowed(&stopped, ts(10), MIN5), "lifts an auth stop");
        assert!(manual_allowed(&SourceState::default(), ts(0), MIN5));
    }

    #[test]
    fn back_off_doubles_and_honours_retry_after() {
        let m = |n: u64| Duration::from_secs(n * 60);
        assert_eq!(backoff(0, None, Duration::ZERO), Duration::ZERO);
        assert_eq!(backoff(1, None, Duration::ZERO), m(5));
        assert_eq!(backoff(2, None, Duration::ZERO), m(10));
        assert_eq!(backoff(4, None, Duration::ZERO), m(40));
        assert_eq!(backoff(5, None, Duration::ZERO), m(60));
        assert_eq!(backoff(500, None, Duration::ZERO), m(60));
        assert_eq!(backoff(1, Some(m(90)), Duration::ZERO), m(90));
        assert_eq!(backoff(1, None, m(7)), m(7));
    }

    #[test]
    fn recording_attempts() {
        let mut s = SourceState {
            in_flight: true,
            ..SourceState::default()
        };
        let fail = AttemptResult::Failure {
            kind: FailureKind::NoData,
            retry_after: None,
            throttled: true,
            credential: None,
        };
        record(&mut s, ts(0), ts(1), &fail, MIN5);
        assert_eq!(s.consecutive_failures, 1);
        assert_eq!(s.backoff_until, Some(ts(6)));
        assert!(!s.in_flight);
        record(&mut s, ts(10), ts(10), &fail, MIN5);
        assert_eq!(s.backoff_until, Some(ts(20)));
        let auth = AttemptResult::Failure {
            kind: FailureKind::Auth,
            retry_after: None,
            throttled: false,
            credential: Some(CredentialFingerprint("fp".to_owned())),
        };
        record(&mut s, ts(30), ts(30), &auth, MIN5);
        assert_eq!(s.auth_stopped, Some(CredentialFingerprint("fp".to_owned())));
        record(&mut s, ts(40), ts(41), &AttemptResult::Success, MIN5);
        assert_eq!(s.consecutive_failures, 0);
        assert_eq!(s.backoff_until, None);
        assert_eq!(s.auth_stopped, None);
        assert_eq!(
            (s.last_attempt, s.last_success),
            (Some(ts(40)), Some(ts(41)))
        );
    }
}
