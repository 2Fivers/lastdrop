//! Freshness per service (R§3.8).

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::consts::{STALE_INTERVAL_FACTOR, STALE_MANUAL, STALE_MIN};

/// Why a service has no fresh reading (R§3.8 `reason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StaleReason {
    /// The last attempt failed with an auth error (HTTP 401, expired token): stale at once, no
    /// reading can follow. Shown as `Sign in` in AccentTextFillColorPrimary.
    SignIn,
    /// Updates are paused by the user: stale at once.
    Paused,
    /// No internet per Network List Manager, or DNS/connect failures.
    Offline,
    /// Anything else: HTTP 5xx, 429 back-off, parse error.
    NoData,
}

/// Fresh or stale with a reason (R§3.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Freshness {
    /// The last good reading is recent enough and nothing forces staleness.
    Fresh,
    /// Meters show the last reading frozen: `u`, `e(t_r)`, no forecast, no severity colour.
    Stale(StaleReason),
}

/// The class of a failed fetch attempt, as the provider layer reports it (`FetchError` maps onto
/// this).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailureKind {
    /// Authentication failed (401, expired or missing sign-in) → [`StaleReason::SignIn`].
    Auth,
    /// No network (NLM says offline, DNS or connect failure) → [`StaleReason::Offline`].
    Offline,
    /// Anything else (5xx, 429/403 back-off, parse error, timeout) → [`StaleReason::NoData`].
    NoData,
}

/// What the scheduler knows about one service's attempts, the input of [`freshness`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// `last_success`: `t_r` of the last good reading, i.e. its `measured_at`
    /// ([`crate::model::Snapshot::reading_at`]); a fetch that returns an old cached reading does
    /// not make the service fresh.
    pub last_success: Option<Timestamp>,
    /// The last attempt, if it failed and no success followed: when and how.
    pub last_failure: Option<(Timestamp, FailureKind)>,
    /// The newest attempt reported an expired sign-in although a fallback source answered
    /// (`Health::SignInRequired`): STALE(SIGN_IN) at once, like an auth failure (R§3.8).
    pub sign_in_required: bool,
    /// A short, secret-free description of the last failure for the hover card
    /// (`card.sentence.nodata` `{detail}`, e.g. `HTTP 503`) and the flyout's "Couldn't read".
    pub failure_detail: Option<String>,
    /// Updates are paused by the user (settings `pause`, evaluated at `now`).
    pub paused: bool,
    /// The poll interval in effect; `None` in manual refresh mode.
    pub poll_interval: Option<SignedDuration>,
    /// When the next attempt is planned (`next try 13:30`, F§4.3, R Appendix B), if planned.
    pub next_try: Option<Timestamp>,
}

/// `stale_after = max(15 min, 2 × poll_interval)`; manual refresh mode: 60 min (R§3.8).
/// Interval 5 min → 15 min; 10 min → 20 min.
pub fn stale_after(poll_interval: Option<SignedDuration>) -> SignedDuration {
    match poll_interval {
        Some(interval) => interval
            .checked_mul(STALE_INTERVAL_FACTOR)
            .unwrap_or(SignedDuration::MAX)
            .max(STALE_MIN),
        None => STALE_MANUAL,
    }
}

/// Freshness of a service at `now` (R§3.8):
///
/// ```text
/// STALE if   now − last_success > stale_after
///       or   the last attempt failed with an auth error   (SIGN_IN, immediately)
///            (or `sign_in_required`: a fallback answered after a 401)
///       or   updates are paused by the user               (PAUSED, immediately)
/// ```
///
/// The reason is the first that applies in the order SIGN_IN, PAUSED, then the class of the last
/// failure (OFFLINE / NO_DATA), NO_DATA when there is no failure on record (e.g. a successful
/// fetch that returned an old cached reading). A service without any success is reported by its
/// caller as LOADING, not through this function.
pub fn freshness(now: Timestamp, status: &ServiceStatus) -> Freshness {
    let failure = status.last_failure.map(|(_, kind)| kind);
    if status.sign_in_required || failure == Some(FailureKind::Auth) {
        return Freshness::Stale(StaleReason::SignIn);
    }
    if status.paused {
        return Freshness::Stale(StaleReason::Paused);
    }
    let reason = match failure {
        Some(FailureKind::Offline) => StaleReason::Offline,
        _ => StaleReason::NoData,
    };
    match status.last_success {
        Some(t) if now.duration_since(t) > stale_after(status.poll_interval) => {
            Freshness::Stale(reason)
        }
        Some(_) => Freshness::Fresh,
        // No success at all: the caller shows LOADING (or "Couldn't read"); stale only when an
        // attempt failed.
        None if failure.is_some() => Freshness::Stale(reason),
        None => Freshness::Fresh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(mins: i64) -> Timestamp {
        Timestamp::from_second(mins * 60).expect("valid")
    }

    fn status(last_success_min: i64) -> ServiceStatus {
        ServiceStatus {
            last_success: Some(ts(last_success_min)),
            poll_interval: Some(SignedDuration::from_mins(5)),
            ..ServiceStatus::default()
        }
    }

    #[test]
    fn immediate_reasons_come_first() {
        let paused = ServiceStatus {
            paused: true,
            ..status(100)
        };
        assert_eq!(
            freshness(ts(100), &paused),
            Freshness::Stale(StaleReason::Paused)
        );
        let sign_in = ServiceStatus {
            sign_in_required: true,
            ..paused
        };
        assert_eq!(
            freshness(ts(100), &sign_in),
            Freshness::Stale(StaleReason::SignIn)
        );
    }

    #[test]
    fn offline_only_after_stale_after() {
        let offline = ServiceStatus {
            last_failure: Some((ts(110), FailureKind::Offline)),
            ..status(100)
        };
        assert_eq!(freshness(ts(110), &offline), Freshness::Fresh);
        assert_eq!(
            freshness(ts(116), &offline),
            Freshness::Stale(StaleReason::Offline)
        );
        let manual = ServiceStatus {
            poll_interval: None,
            ..status(100)
        };
        assert_eq!(freshness(ts(160), &manual), Freshness::Fresh);
        assert_eq!(
            freshness(ts(161), &manual),
            Freshness::Stale(StaleReason::NoData)
        );
    }

    #[test]
    fn without_any_success() {
        assert_eq!(
            freshness(ts(0), &ServiceStatus::default()),
            Freshness::Fresh
        );
        let failed = ServiceStatus {
            last_failure: Some((ts(0), FailureKind::NoData)),
            ..ServiceStatus::default()
        };
        assert_eq!(
            freshness(ts(0), &failed),
            Freshness::Stale(StaleReason::NoData)
        );
    }
}
