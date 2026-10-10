//! Source 2: the OAuth usage endpoint with Claude Code's token.
//!
//! Read-only use of another tool's credential: the token is read from Claude Code's
//! `.credentials.json`, used for one request in memory, dropped. It is **never refreshed**
//! (refresh tokens rotate; refreshing one signs Claude Code out), never copied, never logged.
//! Polled at most every 5 minutes per credential; 429/403 back off with `Retry-After`; 401 stops
//! until the credentials file changes (ARCHITECTURE).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::secret::SecretString;

use super::ClaudeUsage;
use crate::FetchContext;
use crate::error::FetchError;
use crate::io::{HttpRequest, HttpResponse};

/// The usage endpoint (GET).
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The beta header Claude Code sends with OAuth requests; confirm against Claude Code before
/// relying on it.
pub const BETA_HEADER: (&str, &str) = ("anthropic-beta", "oauth-2025-04-20");

/// Claude Code's credentials file in its config folder.
pub const CREDENTIALS_FILE: &str = ".credentials.json";

/// Largest credentials file read.
pub const MAX_CREDENTIALS_BYTES: u64 = 64 << 10;

/// Timeout of the usage request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// The credentials file of a config folder.
pub fn credentials_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CREDENTIALS_FILE)
}

/// Reads the access token (`claudeAiOauth.accessToken`) read-only. `FetchError::Auth` when the
/// file or token is missing, or the token's own expiry (`expiresAt`) has passed — Lastdrop does
/// not refresh it; the user opens Claude Code, which does.
pub fn read_token(ctx: &FetchContext<'_>, config_dir: &Path) -> Result<SecretString, FetchError> {
    todo!("M1: read the OAuth access token, read-only")
}

/// The request: `GET USAGE_URL`, `Authorization: Bearer <token>` (a secret header),
/// [`BETA_HEADER`], `Accept: application/json`.
pub fn request(token: &SecretString, timeout: Duration) -> HttpRequest {
    todo!("M1: build the usage request")
}

/// Interprets the response: 200 → [`parse`]; 401 → Auth; 429/403 → RateLimited with
/// `Retry-After`; 5xx → Transient.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<ClaudeUsage, FetchError> {
    todo!("M1: map the HTTP status")
}

/// Parses the JSON body: `five_hour`, `seven_day`, and per-model weekly objects with
/// `utilization` (percent) and `resets_at` (RFC 3339 or null). Tolerant of unknown and missing
/// fields.
pub fn parse(body: &[u8]) -> Result<ClaudeUsage, FetchError> {
    todo!("M1: parse the usage body")
}

/// The whole source: token → request → interpret.
pub fn fetch(ctx: &FetchContext<'_>, config_dir: &Path) -> Result<ClaudeUsage, FetchError> {
    todo!("M1: OAuth usage source")
}
