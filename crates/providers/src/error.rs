//! Why a fetch failed, classified for freshness (R§3.8 reasons) and for the refresh policy
//! (back-off, `Retry-After`, auth stop). Texts never contain secrets.

use std::time::Duration;

use lastdrop_core::model::SourceKind;
use lastdrop_core::refresh::{AttemptResult, CredentialFingerprint};
use lastdrop_core::stats::FailureKind;

use crate::io::HttpError;

/// A failed fetch. `from` is the source whose failure is reported (a provider that tries several
/// sources reports the most telling one, [`FetchError::most_telling`]: an auth error over a
/// throttle over offline over a temporary failure over a parse error over "not found").
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum FetchError {
    /// The sign-in expired or is missing (HTTP 401, no token, Meshy without an API key): stale
    /// with `Sign in` at once, and no automatic retry until the credential changes.
    #[error("{from:?}: sign-in expired or missing")]
    Auth {
        /// The source.
        from: SourceKind,
    },
    /// The service throttled or refused (HTTP 429/403): back off, honouring `Retry-After`.
    #[error("{from:?}: rate limited")]
    RateLimited {
        /// The source.
        from: SourceKind,
        /// `Retry-After`, if given.
        retry_after: Option<Duration>,
    },
    /// No network (Network List Manager offline, DNS or connect failure).
    #[error("{from:?}: offline")]
    Offline {
        /// The source.
        from: SourceKind,
    },
    /// A temporary failure: HTTP 5xx, timeouts, a child process that did not answer in time.
    #[error("{from:?}: {detail}")]
    Transient {
        /// The source.
        from: SourceKind,
        /// Short, secret-free detail for the hover card (`HTTP 503`, `timed out`).
        detail: String,
        /// `Retry-After`, if given.
        retry_after: Option<Duration>,
    },
    /// The answer could not be understood (format changed): no retry storm, normal interval.
    #[error("{from:?}: unexpected response: {detail}")]
    Parse {
        /// The source.
        from: SourceKind,
        /// Short, secret-free detail (`missing five_hour.utilization`).
        detail: String,
    },
    /// No source exists for this account (no Claude folder, no `codex.exe`, no data yet).
    #[error("no data source: {detail}")]
    Unavailable {
        /// Short, secret-free detail (`codex.exe not found`).
        detail: String,
    },
}

impl FetchError {
    /// Maps an HTTP transport error of `from`.
    pub fn from_http(from: SourceKind, error: &HttpError) -> FetchError {
        match error {
            HttpError::Unreachable => FetchError::Offline { from },
            HttpError::Timeout => FetchError::Transient {
                from,
                detail: "timed out".to_owned(),
                retry_after: None,
            },
            HttpError::Tls(_) | HttpError::TooLarge | HttpError::Other(_) => {
                FetchError::Transient {
                    from,
                    detail: error.to_string(),
                    retry_after: None,
                }
            }
        }
    }

    /// The failure class for freshness (R§3.8): Auth → SIGN_IN, Offline → OFFLINE, everything
    /// else → NO_DATA.
    pub fn kind(&self) -> FailureKind {
        match self {
            FetchError::Auth { .. } => FailureKind::Auth,
            FetchError::Offline { .. } => FailureKind::Offline,
            FetchError::RateLimited { .. }
            | FetchError::Transient { .. }
            | FetchError::Parse { .. }
            | FetchError::Unavailable { .. } => FailureKind::NoData,
        }
    }

    /// `Retry-After` given by the service, if any.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            FetchError::RateLimited { retry_after, .. }
            | FetchError::Transient { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// The input of [`lastdrop_core::refresh::record`] for this failure.
    pub fn attempt_result(&self, credential: Option<CredentialFingerprint>) -> AttemptResult {
        AttemptResult::Failure {
            kind: self.kind(),
            retry_after: self.retry_after(),
            throttled: matches!(self, FetchError::RateLimited { .. }),
            credential,
        }
    }

    /// How telling the failure is when several sources failed, higher is more: Auth (the user
    /// can act) over RateLimited over Offline over Transient over Parse over Unavailable.
    pub fn rank(&self) -> u8 {
        match self {
            FetchError::Auth { .. } => 5,
            FetchError::RateLimited { .. } => 4,
            FetchError::Offline { .. } => 3,
            FetchError::Transient { .. } => 2,
            FetchError::Parse { .. } => 1,
            FetchError::Unavailable { .. } => 0,
        }
    }

    /// The failure a provider reports when none of its sources answered: the highest
    /// [`rank`](Self::rank), the earlier source on a tie. `None` when there are no failures.
    pub fn most_telling(failures: impl IntoIterator<Item = FetchError>) -> Option<FetchError> {
        let mut best: Option<FetchError> = None;
        for failure in failures {
            if best.as_ref().is_none_or(|b| failure.rank() > b.rank()) {
                best = Some(failure);
            }
        }
        best
    }

    /// The more telling of `self` and a later source's `other`
    /// ([`most_telling`](Self::most_telling) of the two): `self` on a tie.
    pub fn or_more_telling(self, other: FetchError) -> FetchError {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }

    /// The short detail shown in the hover card's NO_DATA sentence and the flyout's
    /// "Couldn't read" line.
    pub fn detail(&self) -> String {
        match self {
            FetchError::Auth { .. } => "sign-in expired".to_owned(),
            FetchError::RateLimited { .. } => "rate limited".to_owned(),
            FetchError::Offline { .. } => "offline".to_owned(),
            FetchError::Transient { detail, .. }
            | FetchError::Parse { detail, .. }
            | FetchError::Unavailable { detail } => detail.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lastdrop_core::model::SourceKind;

    use super::FetchError;

    const FROM: SourceKind = SourceKind::CodexRollout;

    fn auth() -> FetchError {
        FetchError::Auth { from: FROM }
    }
    fn throttled() -> FetchError {
        FetchError::RateLimited {
            from: FROM,
            retry_after: Some(Duration::from_secs(60)),
        }
    }
    fn offline() -> FetchError {
        FetchError::Offline { from: FROM }
    }
    fn transient(detail: &str) -> FetchError {
        FetchError::Transient {
            from: FROM,
            detail: detail.to_owned(),
            retry_after: None,
        }
    }
    fn parse() -> FetchError {
        FetchError::Parse {
            from: FROM,
            detail: "bad".to_owned(),
        }
    }
    fn unavailable() -> FetchError {
        FetchError::Unavailable {
            detail: "none".to_owned(),
        }
    }

    #[test]
    fn ranking_puts_auth_over_throttle_over_offline() {
        let ordered = [
            auth(),
            throttled(),
            offline(),
            transient("x"),
            parse(),
            unavailable(),
        ];
        for pair in ordered.windows(2) {
            assert!(
                pair[0].rank() > pair[1].rank(),
                "{:?} > {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn most_telling_picks_the_highest_rank_and_the_earlier_on_a_tie() {
        assert_eq!(FetchError::most_telling([]), None);
        assert_eq!(
            FetchError::most_telling([unavailable(), offline(), throttled(), parse()]),
            Some(throttled())
        );
        assert_eq!(
            FetchError::most_telling([transient("first"), transient("second")]),
            Some(transient("first"))
        );
        assert_eq!(offline().or_more_telling(throttled()), throttled());
        assert_eq!(throttled().or_more_telling(offline()), throttled());
        assert_eq!(
            transient("first").or_more_telling(transient("second")),
            transient("first")
        );
        assert_eq!(parse().or_more_telling(auth()), auth());
    }
}
