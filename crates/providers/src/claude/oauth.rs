//! Source 2: the OAuth usage endpoint with Claude Code's token.
//!
//! Read-only use of another tool's credential: the token is read from Claude Code's
//! `.credentials.json`, used for one request in memory, dropped. It is **never refreshed**
//! (refresh tokens rotate; refreshing one signs Claude Code out), never copied, never logged.
//! Polled at most every 5 minutes per credential; 429/403 back off with `Retry-After`; 401 stops
//! until the credentials file changes (ARCHITECTURE). That memory lives in
//! [`super::ClaudeProvider`]; the functions here are stateless.
//!
//! The credentials file is `{claudeAiOauth: {accessToken, refreshToken, expiresAt (epoch ms),
//! scopes[], subscriptionType, rateLimitTier}, …}`. Only `accessToken`, `expiresAt`, `scopes`
//! and `subscriptionType` are read; the refresh token and the other entries are skipped while
//! parsing, and the file's bytes are overwritten once parsed. Claude Code builds that keep the
//! token in Windows Credential Manager leave `accessToken` empty here: the source is then
//! unavailable (Lastdrop does not read another tool's Credential Manager entries).

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::SourceKind;
use lastdrop_core::secret::SecretString;
use serde::Deserialize;

use super::json::{Instant, Lenient, UsageBody, json_detail};
use super::{ClaudeUsage, file_error};
use crate::FetchContext;
use crate::error::FetchError;
use crate::io::{HeaderValue, HttpRequest, HttpResponse, Method};

/// The usage endpoint (GET).
pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// The beta header Claude Code sends with OAuth requests (confirmed in CodexBar's fetcher and in
/// the Claude Code binary, which call the same path with it).
pub const BETA_HEADER: (&str, &str) = ("anthropic-beta", "oauth-2025-04-20");

/// Claude Code's credentials file in its config folder.
pub const CREDENTIALS_FILE: &str = ".credentials.json";

/// Largest credentials file read.
pub const MAX_CREDENTIALS_BYTES: u64 = 64 << 10;

/// Timeout of the usage request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// A token that expires within this margin is treated as expired, so a request never races the
/// expiry into a 401 that would stop the source (**not from the spec**).
pub const EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The scope the usage endpoint needs. Tokens from `claude setup-token` are inference-only.
const USAGE_SCOPE: &str = "user:profile";

const FROM: SourceKind = SourceKind::ClaudeOAuthUsage;

/// The credentials file of a config folder.
pub fn credentials_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CREDENTIALS_FILE)
}

/// What Lastdrop reads from Claude Code's credentials file.
pub(crate) struct Credentials {
    /// The access token, possibly empty.
    token: SecretString,
    /// The token's own expiry, when the file gives one.
    expires_at: Option<Timestamp>,
    /// The token's scopes, when the file lists them.
    scopes: Option<Vec<String>>,
    /// The plan (`Max`, `Pro`, …), from `subscriptionType`.
    pub(crate) plan: Option<String>,
}

impl Credentials {
    /// The token, when it can be used at `now`: `FetchError::Unavailable` when it is empty (kept
    /// elsewhere, or signed out) or lacks the usage scope; `FetchError::Auth` when it has
    /// expired (within [`EXPIRY_MARGIN`]): Claude Code renews it on its next start, Lastdrop
    /// never does.
    pub(crate) fn usable_token(self, now: Timestamp) -> Result<SecretString, FetchError> {
        if self.token.expose().trim().is_empty() {
            return Err(FetchError::Unavailable {
                detail: format!("no Claude Code token in {CREDENTIALS_FILE}"),
            });
        }
        if let Some(scopes) = &self.scopes
            && !scopes.iter().any(|s| s == USAGE_SCOPE)
        {
            return Err(FetchError::Unavailable {
                detail: "the Claude Code token cannot read usage".to_owned(),
            });
        }
        if let Some(expires_at) = self.expires_at {
            let margin = SignedDuration::try_from(EXPIRY_MARGIN).unwrap_or(SignedDuration::ZERO);
            if expires_at.duration_since(now) <= margin {
                return Err(FetchError::Auth { from: FROM });
            }
        }
        Ok(self.token)
    }
}

#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth", default)]
    claude_ai_oauth: Option<OAuthEntry>,
}

/// `claudeAiOauth`. `accessToken` is typed (never buffered as a `Value`); `refreshToken` is not a
/// field, so serde skips it without allocating.
#[derive(Deserialize)]
struct OAuthEntry {
    #[serde(rename = "accessToken", default)]
    access_token: Option<String>,
    #[serde(rename = "expiresAt", default)]
    expires_at: Lenient<Instant>,
    #[serde(default)]
    scopes: Lenient<Vec<String>>,
    #[serde(rename = "subscriptionType", default)]
    subscription_type: Lenient<String>,
}

/// Reads the credentials file read-only. File errors only: `FetchError::Unavailable` when the
/// file or the `claudeAiOauth` entry is missing (Claude Code not signed in with this folder),
/// `FetchError::Parse` when it is not JSON (the detail names a position, never a value).
pub(crate) fn read_credentials(
    ctx: &FetchContext<'_>,
    config_dir: &Path,
) -> Result<Credentials, FetchError> {
    let mut bytes = ctx
        .fs
        .read(&credentials_path(config_dir), MAX_CREDENTIALS_BYTES)
        .map_err(|e| file_error(FROM, &e, CREDENTIALS_FILE))?;
    let parsed = serde_json::from_slice::<CredentialsFile>(&bytes);
    // Best effort, like `SecretString`'s drop: the buffer held the refresh token too.
    bytes.fill(0);
    std::hint::black_box(&bytes);
    let file = parsed.map_err(|e| FetchError::Parse {
        from: FROM,
        detail: json_detail(CREDENTIALS_FILE, &e),
    })?;
    let Some(entry) = file.claude_ai_oauth else {
        return Err(FetchError::Unavailable {
            detail: format!("no Claude sign-in in {CREDENTIALS_FILE}"),
        });
    };
    Ok(Credentials {
        token: SecretString::new(entry.access_token.unwrap_or_default()),
        expires_at: entry.expires_at.0.map(|i| i.0),
        scopes: entry.scopes.0,
        plan: entry.subscription_type.0.as_deref().and_then(plan_name),
    })
}

/// The credentials file's top-level `organizationUuid` alone; everything else is skipped while
/// parsing.
#[derive(Deserialize)]
struct OrganisationOnly {
    #[serde(rename = "organizationUuid", default)]
    organisation: Lenient<String>,
}

/// The organisation Claude Code is signed in to, from the credentials file's `organizationUuid`
/// (read-only; the file's bytes are overwritten once parsed). Only compared in place with the
/// organisation of Claude Desktop's cached usage (`super::desktop_cache`), never logged or kept
/// past the fetch. `None` when the file, the field or a usable value is missing.
pub(crate) fn read_organisation(ctx: &FetchContext<'_>, config_dir: &Path) -> Option<SecretString> {
    let mut bytes = ctx
        .fs
        .read(&credentials_path(config_dir), MAX_CREDENTIALS_BYTES)
        .ok()?;
    let parsed = serde_json::from_slice::<OrganisationOnly>(&bytes);
    bytes.fill(0);
    std::hint::black_box(&bytes);
    let organisation = parsed.ok()?.organisation.0?;
    let organisation = SecretString::new(organisation);
    (!organisation.expose().trim().is_empty()).then_some(organisation)
}

/// The plan badge text of a `subscriptionType` (`max` → `Max`); `None` for values Lastdrop does
/// not know, so no internal name reaches the UI.
pub(crate) fn plan_name(subscription_type: &str) -> Option<String> {
    let name = match subscription_type.trim().to_ascii_lowercase().as_str() {
        "max" => "Max",
        "pro" => "Pro",
        "team" => "Team",
        "enterprise" => "Enterprise",
        "free" => "Free",
        _ => return None,
    };
    Some(name.to_owned())
}

/// Reads the access token (`claudeAiOauth.accessToken`) read-only.
///
/// - `FetchError::Unavailable` when the file, the entry or the token is missing or empty (Claude
///   Code not signed in with this folder, or keeping its token in Credential Manager), or the token
///   lacks the usage scope: nothing to sign in to, the source is skipped.
/// - `FetchError::Auth` when the token's own expiry (`expiresAt`) has passed — Lastdrop does not
///   refresh it; the user opens Claude Code, which does.
/// - `FetchError::Parse` when the file is not JSON.
pub fn read_token(ctx: &FetchContext<'_>, config_dir: &Path) -> Result<SecretString, FetchError> {
    read_credentials(ctx, config_dir)?.usable_token(ctx.clock.now())
}

/// The request: `GET USAGE_URL`, `Authorization: Bearer <token>` (a secret header),
/// [`BETA_HEADER`], `Accept: application/json`.
pub fn request(token: &SecretString, timeout: Duration) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        url: USAGE_URL.to_owned(),
        headers: vec![
            (
                "Authorization".to_owned(),
                HeaderValue::Secret(SecretString::new(format!("Bearer {}", token.expose()))),
            ),
            (
                BETA_HEADER.0.to_owned(),
                HeaderValue::Plain(BETA_HEADER.1.to_owned()),
            ),
            (
                "Accept".to_owned(),
                HeaderValue::Plain("application/json".to_owned()),
            ),
        ],
        body: None,
        timeout,
    }
}

/// Interprets the response: 200 → [`parse`]; 401 → Auth; 429/403 → RateLimited with
/// `Retry-After` (capped, [`HttpResponse::retry_after`]); 5xx → Transient. Any other status is a
/// changed endpoint: Parse (`HTTP 404`), retried at the normal interval.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<ClaudeUsage, FetchError> {
    match response.status {
        200..=299 => parse(&response.body),
        401 => Err(FetchError::Auth { from: FROM }),
        403 | 429 => Err(FetchError::RateLimited {
            from: FROM,
            retry_after: response.retry_after(now),
        }),
        status @ 500..=599 => Err(FetchError::Transient {
            from: FROM,
            detail: format!("HTTP {status}"),
            retry_after: response.retry_after(now),
        }),
        status => Err(FetchError::Parse {
            from: FROM,
            detail: format!("HTTP {status}"),
        }),
    }
}

/// Parses the JSON body: `five_hour`, `seven_day`, and per-model weekly objects with
/// `utilization` (percent) and `resets_at` (RFC 3339 or null). Tolerant of unknown and missing
/// fields.
///
/// Per-model weekly windows come from `limits[]` entries of kind `weekly_scoped` with a
/// `scope.model.display_name` (current bodies), else from the legacy `seven_day_opus` /
/// `seven_day_sonnet` objects. `extra_usage`, `spend` and other windows have no R§2 meter and are
/// skipped. A body with no R§2 window at all is a Parse error (a changed format must not read as
/// "no limits"). `measured_at` is `None`: the body carries no time.
pub fn parse(body: &[u8]) -> Result<ClaudeUsage, FetchError> {
    parse_from(body, FROM)
}

/// [`parse`] for another source that holds the same body (Claude Desktop's cached copy): errors
/// name `from`.
pub(crate) fn parse_from(body: &[u8], from: SourceKind) -> Result<ClaudeUsage, FetchError> {
    let body: UsageBody = serde_json::from_slice(body).map_err(|e| FetchError::Parse {
        from,
        detail: json_detail("usage response", &e),
    })?;
    let windows = body.windows();
    if windows.is_empty() {
        return Err(FetchError::Parse {
            from,
            detail: "no usage windows in the response".to_owned(),
        });
    }
    Ok(ClaudeUsage {
        windows,
        plan: None,
        measured_at: None,
    })
}

/// One request with a usable `token`: request → interpret, `measured_at` = when the answer
/// arrived. No request when the fetch deadline has passed.
pub(crate) fn send(
    ctx: &FetchContext<'_>,
    token: &SecretString,
) -> Result<ClaudeUsage, FetchError> {
    let timeout = TIMEOUT.min(ctx.remaining());
    if timeout.is_zero() {
        return Err(FetchError::Transient {
            from: FROM,
            detail: "timed out".to_owned(),
            retry_after: None,
        });
    }
    let response = ctx
        .http
        .request(&request(token, timeout))
        .map_err(|e| FetchError::from_http(FROM, &e))?;
    let now = ctx.clock.now();
    let mut usage = interpret(&response, now)?;
    usage.measured_at = Some(now);
    Ok(usage)
}

/// The whole source: token → request → interpret. Stateless: the provider adds the 5-minute
/// minimum, the back-off and the stop after a 401.
pub fn fetch(ctx: &FetchContext<'_>, config_dir: &Path) -> Result<ClaudeUsage, FetchError> {
    let credentials = read_credentials(ctx, config_dir)?;
    let plan = credentials.plan.clone();
    let token = credentials.usable_token(ctx.clock.now())?;
    let mut usage = send(ctx, &token)?;
    usage.plan = plan;
    Ok(usage)
}
