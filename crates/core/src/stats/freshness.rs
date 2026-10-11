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
    /// The source answers, but its newest reading is old: no failure is on record (Claude
    /// Desktop's history while Claude Desktop is closed, a cached usage). Nothing is broken, so
    /// the readout shows AGE on every row and the flyout says "No new reading", not "Not
    /// answering". **Not from the spec** (R§3.8 put this case under NO_DATA).
    OldReading,
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
    /// The longest a source lets its last live answer stand in between polls, when it paces
    /// itself below the poll interval (Codex's app-server: at most every 15 minutes, its answer
    /// standing in between): [`stale_after_paced`].
    #[serde(default)]
    pub reading_pace: Option<SignedDuration>,
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

/// [`stale_after`] of a source that paces its live reading (`ServiceStatus::reading_pace`): at
/// least `pace + poll_interval` (**not from the spec**). Such a source answers every poll, but with
/// its last live reading until `pace` has passed; the next live reading comes with the first poll
/// after that, up to one poll interval later. Without the extension, polls every 2, 4, 6 or 7
/// minutes against a 15-minute pace would show the service STALE(OLD_READING) for part of every
/// cycle although nothing is wrong. Manual mode and sources without a pace: [`stale_after`].
pub fn stale_after_paced(
    poll_interval: Option<SignedDuration>,
    pace: Option<SignedDuration>,
) -> SignedDuration {
    let spec = stale_after(poll_interval);
    match (poll_interval, pace) {
        (Some(interval), Some(pace)) => spec.max(pace.saturating_add(interval)),
        _ => spec,
    }
}

/// Freshness of a service at `now` (R§3.8; a paced source per [`stale_after_paced`]):
///
/// ```text
/// STALE if   now − last_success > stale_after
///       or   the last attempt failed with an auth error   (SIGN_IN, immediately)
///            (or `sign_in_required`: a fallback answered after a 401)
///       or   updates are paused by the user               (PAUSED, immediately)
/// ```
///
/// The reason is the first that applies in the order SIGN_IN, PAUSED, then the class of the last
/// failure (OFFLINE / NO_DATA), [`StaleReason::OldReading`] when there is no failure on record
/// (a successful fetch that returned an old reading). A service without any success is reported
/// by its caller as LOADING, not through this function.
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
        Some(FailureKind::NoData | FailureKind::Auth) => StaleReason::NoData,
        None => StaleReason::OldReading,
    };
    match status.last_success {
        Some(t)
            if now.duration_since(t)
                > stale_after_paced(status.poll_interval, status.reading_pace) =>
        {
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
            Freshness::Stale(StaleReason::OldReading),
            "no failure on record: the source answered with an old reading"
        );
        let failed = ServiceStatus {
            last_failure: Some((ts(150), FailureKind::NoData)),
            ..manual
        };
        assert_eq!(
            freshness(ts(161), &failed),
            Freshness::Stale(StaleReason::NoData)
        );
    }

    #[test]
    fn a_paced_source_stays_fresh_for_its_pace_plus_one_poll() {
        let pace = Some(SignedDuration::from_mins(15));
        let m = |n: i64| Some(SignedDuration::from_mins(n));
        assert_eq!(stale_after_paced(m(7), pace), SignedDuration::from_mins(22));
        assert_eq!(
            stale_after_paced(m(20), pace),
            SignedDuration::from_mins(40),
            "the spec's 2 × interval when it is longer"
        );
        assert_eq!(stale_after_paced(m(7), None), SignedDuration::from_mins(15));
        assert_eq!(stale_after_paced(None, pace), SignedDuration::from_mins(60));

        // A 15-minute pace polled every 7 minutes: the stand-in is 21 minutes old when the
        // next live reading comes, and the service stays fresh.
        let paced = ServiceStatus {
            poll_interval: m(7),
            reading_pace: pace,
            ..status(100)
        };
        assert_eq!(freshness(ts(121), &paced), Freshness::Fresh);
        assert_eq!(
            freshness(ts(123), &paced),
            Freshness::Stale(StaleReason::OldReading)
        );
        let unpaced = ServiceStatus {
            reading_pace: None,
            ..paced
        };
        assert_eq!(
            freshness(ts(121), &unpaced),
            Freshness::Stale(StaleReason::OldReading)
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
