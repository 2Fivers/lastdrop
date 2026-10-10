//! Why a fetch failed, classified for freshness (R§3.8 reasons) and for the refresh policy
//! (back-off, `Retry-After`, auth stop). Texts never contain secrets.

use std::time::Duration;

use lastdrop_core::model::SourceKind;
use lastdrop_core::refresh::{AttemptResult, CredentialFingerprint};
use lastdrop_core::stats::FailureKind;

use crate::io::HttpError;

/// A failed fetch. `from` is the source whose failure is reported (a provider that tries several
/// sources reports the most telling one: an auth error over a parse error over "not found").
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
