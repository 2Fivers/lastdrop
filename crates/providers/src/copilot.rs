//! GitHub Copilot: the plan's monthly quotas (R§2 table).
//!
//! Source (ARCHITECTURE provider table): `GET https://api.github.com/copilot_internal/user`, the
//! endpoint Copilot's own editor clients read their quotas from (undocumented), with the GitHub
//! CLI's sign-in. The token comes from `gh auth token`, run without a window for each fetch; it
//! lives in memory for the one request and is never written, logged or refreshed (gh keeps its
//! own sign-in in Credential Manager or its `hosts.yml`).
//!
//! The answer's `quota_snapshots` has one entry per quota: `premium_interactions` (paid plans),
//! `chat` and `completions` (Copilot Free's monthly chat messages and code completions). A quota that is `unlimited`, or whose `entitlement` is 0 (premium requests on Copilot
//! Free), is not a limit and becomes no meter. Every quota resets at `quota_reset_date_utc`: a
//! calendar month ([`WindowLen::CalendarMonth`]; Copilot Free's month may start on the day the
//! plan began, which [`WindowLen::length_ending_at`] handles).
//!
//! Which quotas exist depends on the plan, so the catalogue has none
//! (`lastdrop_core::model::catalog`): the names come from [`meter_names`].

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{
    AccountRef, Health, Meter, MeterId, MeterNames, ProviderId, Snapshot, SourceKind, WindowLen,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::secret::SecretString;
use serde_json::{Map, Value};

use crate::account::AccountSource;
use crate::error::FetchError;
use crate::io::{HeaderValue, HttpRequest, HttpResponse, Method, SpawnSpec, TerminalCommand};
use crate::{FetchContext, Provider};

/// The quota endpoint (GET).
pub const USER_URL: &str = "https://api.github.com/copilot_internal/user";

/// The account's Copilot settings, opened from a flyout card.
pub const USAGE_PAGE: &str = "https://github.com/settings/copilot";

/// Timeout of the quota request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// How long `gh auth token` may take to print the token (capped by the fetch deadline).
pub const TOKEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Lower bound between fetches: 5 minutes, the rule for undocumented endpoints (ARCHITECTURE
/// "Rules for every provider").
pub const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The GitHub host whose sign-in is used.
pub const HOST: &str = "github.com";

/// The GitHub CLI's config folder variable (`hosts.yml`).
pub const GH_CONFIG_DIR_VAR: &str = "GH_CONFIG_DIR";

/// The file in the config folder that changes when the user signs in or out with gh.
pub const HOSTS_FILE: &str = "hosts.yml";

/// Where the GitHub CLI installer puts `gh.exe`, under `%ProgramFiles%` (machine-wide) and
/// `%LOCALAPPDATA%\Programs` (per user).
pub const INSTALL_DIR: &str = "GitHub CLI";

/// The premium-requests quota (paid plans).
pub const PREMIUM: &str = "premium";
/// The chat-messages quota (Copilot Free).
pub const CHAT: &str = "chat";
/// The code-completions quota (Copilot Free).
pub const COMPLETIONS: &str = "completions";

/// The source every Copilot reading and error carries.
const SOURCE: SourceKind = SourceKind::CopilotApi;

/// `quota_snapshots` keys and the meter ids they become, in meter order.
const QUOTAS: [(&str, &str); 3] = [
    ("premium_interactions", PREMIUM),
    ("chat", CHAT),
    ("completions", COMPLETIONS),
];

/// The names and order of a Copilot meter (R§2 table), `None` for an id this build does not
/// know.
pub fn meter_names(id: &str) -> Option<(MeterNames, u16)> {
    match id {
        PREMIUM => Some((MeterNames::new("Prem", "premium", "Premium requests"), 0)),
        CHAT => Some((MeterNames::new("Chat", "chat", "Chat messages"), 1)),
        COMPLETIONS => Some((
            MeterNames::new("Code", "completions", "Code completions"),
            2,
        )),
        _ => None,
    }
}

/// One quota as Copilot reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Quota {
    /// The meter id ([`PREMIUM`], [`CHAT`], [`COMPLETIONS`]).
    pub id: &'static str,
    /// `u`: the share used, from `percent_remaining` (else `remaining` / `entitlement`) plus any
    /// `overage_count`; may exceed 1, never negative.
    pub used: f64,
    /// Used up and no overage allowed.
    pub limited: bool,
}

/// What the endpoint returned.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CopilotUsage {
    /// The plan as GitHub names it (`Free`, `Business`, `Enterprise`); `None` when the answer
    /// does not say which paid plan it is.
    pub plan: Option<String>,
    /// When every quota resets (`quota_reset_date_utc`, else `quota_reset_date` at 00:00 UTC).
    pub resets_at: Option<Timestamp>,
    /// The limited quotas, in meter order.
    pub quotas: Vec<Quota>,
}

/// The Copilot provider.
#[derive(Clone, Copy, Debug, Default)]
pub struct CopilotProvider;

impl CopilotProvider {
    /// The provider.
    pub fn new() -> CopilotProvider {
        CopilotProvider
    }
}

impl Provider for CopilotProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Copilot
    }

    /// `gh auth token` → request → interpret.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::Copilot {
            account,
            config_dir,
        } = account
        else {
            return Err(FetchError::Unavailable {
                detail: "not a Copilot account".to_owned(),
            });
        };
        let result = gh_token(ctx, config_dir).and_then(|token| {
            let timeout = TIMEOUT.min(ctx.remaining());
            if timeout.is_zero() {
                return Err(timed_out());
            }
            let response = ctx.http.request(&request(&token, timeout));
            drop(token);
            response
                .map_err(|e| FetchError::from_http(SOURCE, &e))
                .and_then(|r| {
                    log::debug!("copilot: user HTTP {}", r.status);
                    interpret(&r, ctx.clock.now())
                })
        });
        match result {
            Ok(usage) => {
                if usage.quotas.is_empty() {
                    log::info!("copilot: no limited quota on this plan");
                }
                Ok(to_snapshot(usage, account.clone(), ctx.clock.now()))
            }
            Err(e) => {
                log::warn!("copilot: {e}");
                Err(e)
            }
        }
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    /// gh's `hosts.yml` (size + modification time): it changes when the user signs in again
    /// (`gh auth login`), which lifts the auth stop.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        let AccountSource::Copilot { config_dir, .. } = account else {
            return None;
        };
        let meta = ctx.fs.metadata(&config_dir.join(HOSTS_FILE)).ok()?;
        if meta.is_dir {
            return None;
        }
        let modified = meta
            .modified
            .map_or_else(|| "-".to_owned(), |t| t.as_nanosecond().to_string());
        Some(CredentialFingerprint(format!(
            "{HOSTS_FILE}:{}:{modified}",
            meta.len
        )))
    }

    /// `gh auth login` for github.com in a terminal, with the account's gh folder.
    fn sign_in_command(&self, account: &AccountSource) -> Option<TerminalCommand> {
        let AccountSource::Copilot { config_dir, .. } = account else {
            return None;
        };
        Some(TerminalCommand {
            program: "gh".into(),
            args: ["auth", "login", "--hostname", HOST, "--web"]
                .into_iter()
                .map(OsString::from)
                .collect(),
            env: vec![(
                GH_CONFIG_DIR_VAR.into(),
                Some(config_dir.clone().into_os_string()),
            )],
            cwd: None,
        })
    }

    fn usage_page(&self) -> Option<&'static str> {
        Some(USAGE_PAGE)
    }
}

// ---- The token ------------------------------------------------------------------------------------

fn non_empty_var(ctx: &FetchContext<'_>, name: &str) -> Option<PathBuf> {
    ctx.env
        .var(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn is_file(ctx: &FetchContext<'_>, path: &Path) -> bool {
    ctx.fs.metadata(path).is_ok_and(|m| !m.is_dir)
}

/// Finds `gh.exe`: the first on `PATH`, else the installer's `%ProgramFiles%\GitHub CLI` or
/// `%LOCALAPPDATA%\Programs\GitHub CLI`. `None` when there is none.
pub fn find_gh(ctx: &FetchContext<'_>) -> Option<PathBuf> {
    let on_path = ctx.env.var("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join("gh.exe"))
            .find(|candidate| is_file(ctx, candidate))
    });
    on_path.or_else(|| {
        [
            non_empty_var(ctx, "ProgramFiles"),
            non_empty_var(ctx, "LOCALAPPDATA").map(|l| l.join("Programs")),
        ]
        .into_iter()
        .flatten()
        .map(|dir| dir.join(INSTALL_DIR).join("gh.exe"))
        .find(|candidate| is_file(ctx, candidate))
    })
}

/// `gh auth token --hostname github.com` with the account's gh folder. gh's update check is off
/// (it would reach the network).
pub fn token_spec(gh: &Path, config_dir: &Path) -> SpawnSpec {
    SpawnSpec {
        program: gh.to_path_buf(),
        args: ["auth", "token", "--hostname", HOST]
            .into_iter()
            .map(OsString::from)
            .collect(),
        env: vec![
            (
                GH_CONFIG_DIR_VAR.into(),
                Some(config_dir.as_os_str().to_owned()),
            ),
            ("GH_NO_UPDATE_NOTIFIER".into(), Some("1".into())),
        ],
        cwd: None,
    }
}

fn timed_out() -> FetchError {
    FetchError::Transient {
        from: SOURCE,
        detail: "timed out".to_owned(),
        retry_after: None,
    }
}

/// The GitHub CLI's token for github.com. No gh → Unavailable; gh signed out (it prints nothing
/// to stdout) → Auth; no answer in time → Transient.
fn gh_token(ctx: &FetchContext<'_>, config_dir: &Path) -> Result<SecretString, FetchError> {
    let Some(gh) = find_gh(ctx) else {
        return Err(FetchError::Unavailable {
            detail: "GitHub CLI (gh) not found".to_owned(),
        });
    };
    let timeout = TOKEN_TIMEOUT.min(ctx.remaining());
    if timeout.is_zero() {
        return Err(timed_out());
    }
    let mut child = ctx
        .spawner
        .spawn(&token_spec(&gh, config_dir))
        .map_err(|e| {
            if e.kind() == ErrorKind::NotFound {
                FetchError::Unavailable {
                    detail: "GitHub CLI (gh) not found".to_owned(),
                }
            } else {
                FetchError::Transient {
                    from: SOURCE,
                    detail: "gh could not start".to_owned(),
                    retry_after: None,
                }
            }
        })?;
    let line = child.read_line(timeout);
    let _ = child.kill();
    match line {
        Ok(Some(line)) => {
            let token = SecretString::new(line.trim().to_owned());
            drop(line);
            if token.is_empty() || token.expose().contains(char::is_whitespace) {
                log::info!("copilot: gh printed no token");
                Err(FetchError::Auth { from: SOURCE })
            } else {
                Ok(token)
            }
        }
        Ok(None) => {
            log::info!("copilot: gh is not signed in to {HOST}");
            Err(FetchError::Auth { from: SOURCE })
        }
        Err(e) if e.kind() == ErrorKind::TimedOut => Err(timed_out()),
        Err(_) => Err(FetchError::Transient {
            from: SOURCE,
            detail: "gh failed".to_owned(),
            retry_after: None,
        }),
    }
}

// ---- The request ----------------------------------------------------------------------------------

/// `GET USER_URL` with `Authorization: token <token>` (a secret header).
pub fn request(token: &SecretString, timeout: Duration) -> HttpRequest {
    let auth = SecretString::new(format!("token {}", token.expose()));
    HttpRequest {
        method: Method::Get,
        url: USER_URL.to_owned(),
        headers: vec![
            ("Authorization".to_owned(), HeaderValue::Secret(auth)),
            (
                "Accept".to_owned(),
                HeaderValue::Plain("application/json".to_owned()),
            ),
        ],
        body: None,
        timeout,
    }
}

/// 200 → [`parse_user`]; 401 → Auth; 404 → Unavailable (the account has no Copilot plan);
/// 429/403 → RateLimited; 5xx → Transient; anything else → Parse (the endpoint no longer answers
/// as it did). `Retry-After` is read relative to `now`.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<CopilotUsage, FetchError> {
    match response.status {
        200..=299 => parse_user(&response.body),
        401 => Err(FetchError::Auth { from: SOURCE }),
        404 => Err(FetchError::Unavailable {
            detail: "no Copilot plan on this GitHub account".to_owned(),
        }),
        403 | 429 => Err(FetchError::RateLimited {
            from: SOURCE,
            retry_after: response.retry_after(now),
        }),
        500..=599 => Err(FetchError::Transient {
            from: SOURCE,
            detail: format!("HTTP {}", response.status),
            retry_after: response.retry_after(now),
        }),
        status => Err(FetchError::Parse {
            from: SOURCE,
            detail: format!("HTTP {status}"),
        }),
    }
}

fn parse_error(detail: &str) -> FetchError {
    FetchError::Parse {
        from: SOURCE,
        detail: detail.to_owned(),
    }
}

/// The plan, quotas and reset of the answer. Tolerant of unknown and missing fields; a quota
/// without a usable share is left out (never 0). Errors only when there is no `quota_snapshots`
/// object.
pub fn parse_user(body: &[u8]) -> Result<CopilotUsage, FetchError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| parse_error("body is not JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| parse_error("body is not a JSON object"))?;
    let snapshots = object
        .get("quota_snapshots")
        .and_then(Value::as_object)
        .ok_or_else(|| parse_error("missing quota_snapshots"))?;
    let quotas = QUOTAS
        .iter()
        .filter_map(|(key, id)| {
            let quota = snapshots.get(*key)?.as_object()?;
            let parsed = quota_of(id, quota);
            if parsed.is_none() {
                log::debug!("copilot: {key} is not a limited quota");
            }
            parsed
        })
        .collect();
    Ok(CopilotUsage {
        plan: plan_of(object),
        resets_at: resets_at_of(object),
        quotas,
    })
}

fn number(object: &Map<String, Value>, key: &str) -> Option<f64> {
    object.get(key)?.as_f64().filter(|n| n.is_finite())
}

/// One quota, `None` when it is unlimited, has no entitlement, or reports no share.
fn quota_of(id: &'static str, quota: &Map<String, Value>) -> Option<Quota> {
    if quota.get("unlimited").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let entitlement = number(quota, "entitlement").filter(|e| *e > 0.0)?;
    let used = match number(quota, "percent_remaining") {
        Some(percent) => (100.0 - percent) / 100.0,
        None => {
            let remaining =
                number(quota, "quota_remaining").or_else(|| number(quota, "remaining"))?;
            (entitlement - remaining) / entitlement
        }
    };
    let overage = number(quota, "overage_count")
        .filter(|o| *o > 0.0)
        .unwrap_or(0.0);
    let used = (used + overage / entitlement).max(0.0);
    let overage_permitted = quota.get("overage_permitted").and_then(Value::as_bool) == Some(true);
    Some(Quota {
        id,
        used,
        limited: used >= 1.0 && !overage_permitted,
    })
}

/// `Free` for Copilot Free (`access_type_sku` `free_limited_copilot`), `Business` or
/// `Enterprise` from `copilot_plan`; `None` for an individual paid plan, whose answer does not
/// say Pro or Pro+.
fn plan_of(object: &Map<String, Value>) -> Option<String> {
    let sku = object.get("access_type_sku").and_then(Value::as_str);
    if sku.is_some_and(|s| s.starts_with("free_")) {
        return Some("Free".to_owned());
    }
    match object.get("copilot_plan").and_then(Value::as_str) {
        Some("business") => Some("Business".to_owned()),
        Some("enterprise") => Some("Enterprise".to_owned()),
        _ => None,
    }
}

/// `quota_reset_date_utc` (an instant), else `quota_reset_date` (`YYYY-MM-DD`) at 00:00 UTC.
fn resets_at_of(object: &Map<String, Value>) -> Option<Timestamp> {
    let text = |key: &str| object.get(key).and_then(Value::as_str);
    text("quota_reset_date_utc")
        .and_then(|t| t.parse::<Timestamp>().ok())
        .or_else(|| {
            text("quota_reset_date")
                .and_then(|d| format!("{d}T00:00:00Z").parse::<Timestamp>().ok())
        })
}

// ---- The snapshot ---------------------------------------------------------------------------------

/// The window meter of one quota: a calendar month ending at `resets_at`.
pub fn meter(quota: &Quota, resets_at: Option<Timestamp>) -> Meter {
    let (names, order) =
        meter_names(quota.id).unwrap_or_else(|| (MeterNames::new("Mo", "monthly", quota.id), 9));
    Meter {
        id: MeterId::new(quota.id),
        names,
        order,
        used: quota.used,
        limited: quota.limited,
        resets_at,
        window: WindowLen::CalendarMonth,
        starts_on_use: false,
    }
}

/// The snapshot of a reading.
pub fn to_snapshot(
    usage: CopilotUsage,
    account: lastdrop_core::model::AccountKey,
    fetched_at: Timestamp,
) -> Snapshot {
    Snapshot {
        provider: ProviderId::Copilot,
        account: AccountRef {
            key: account,
            plan: usage.plan,
            ..AccountRef::primary()
        },
        source: SOURCE,
        // A live reading: the values are true when read.
        measured_at: fetched_at,
        fetched_at,
        meters: usage
            .quotas
            .iter()
            .map(|q| meter(q, usage.resets_at))
            .collect(),
        balances: Vec::new(),
        health: Health::Ok,
    }
}
