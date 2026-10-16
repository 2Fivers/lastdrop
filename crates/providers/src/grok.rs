//! Grok (SuperGrok, Grok Build): the shared credit pool of the week or month, on-demand spend and
//! prepaid credit.
//!
//! Source (ARCHITECTURE provider table): `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`,
//! the request xAI's own Grok CLI makes for its `/usage` panel (xai-org/grok-build,
//! `xai-grok-shell/src/extensions/billing.rs`; undocumented as a public API), with the Grok CLI's
//! sign-in from its `auth.json`. The token is read, used in memory for the one request and
//! dropped: Lastdrop never refreshes or rewrites it (the CLI renews it when it next runs).
//!
//! `auth.json` (`xai-grok-login`'s `AuthStore`) maps a scope to a sign-in
//! `{key, auth_mode, user_id, expires_at (RFC 3339), refresh_token, …}`. The `xai::api_key` scope
//! and `api_key` sign-ins are developer API keys, which cannot read this pool; of the others the
//! one that expires last is used.
//!
//! The answer is protobuf-JSON (`BillingConfig`, possibly under `config`), which leaves out zero
//! values: `creditUsagePercent` (0–100) of `currentPeriod {type, start, end}`, `onDemandCap` and
//! `onDemandUsed` and `prepaidBalance` as `{val}` in US cents (`{}` is 0). A current period
//! without a percent is 0 % used, as the Grok CLI reads it.

use std::path::Path;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{
    AccountKey, AccountRef, Balance, Health, Meter, MeterId, MeterNames, ProviderId, Snapshot,
    SourceKind, Unit, WindowLen,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::secret::SecretString;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::account::AccountSource;
use crate::claude::json::instant_of;
use crate::error::FetchError;
use crate::fingerprint::file_fingerprint;
use crate::io::{HeaderValue, HttpRequest, HttpResponse, Method, TerminalCommand};
use crate::lenient::{Text, non_empty, wipe};
use crate::{FetchContext, Provider};

/// The billing endpoint (GET), as the Grok CLI calls it.
pub const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";

/// The Grok CLI's sign-in file.
pub const AUTH_FILE: &str = "auth.json";

/// The variable that moves the Grok CLI's folder (`~\.grok`).
pub const HOME_VAR: &str = "GROK_HOME";

/// The variable that names the sign-in file itself.
pub const AUTH_PATH_VAR: &str = "GROK_AUTH_PATH";

/// The value of `X-XAI-Token-Auth` the Grok CLI sends.
pub const TOKEN_AUTH: &str = "xai-grok-cli";

/// Largest `auth.json` read.
const MAX_AUTH_BYTES: u64 = 256 << 10;

/// The scope of a plain API key in `auth.json` (never the subscription's pool).
const API_KEY_SCOPE: &str = "xai::api_key";

/// Timeout of the billing request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Lower bound between fetches: 5 minutes, the rule for undocumented endpoints (ARCHITECTURE
/// "Rules for every provider").
pub const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// A token this close to its expiry is treated as expired (the CLI renews it first).
pub const EXPIRY_MARGIN: SignedDuration = SignedDuration::from_secs(60);

/// The source every Grok reading and error carries.
const SOURCE: SourceKind = SourceKind::GrokApi;

/// The order of the meters within the service.
const POOL_ORDER: u16 = 0;
/// See [`POOL_ORDER`].
const ON_DEMAND_ORDER: u16 = 1;
/// See [`POOL_ORDER`].
const PREPAID_ORDER: u16 = 2;

/// Which period the credit pool counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    /// `USAGE_PERIOD_TYPE_WEEKLY`.
    Weekly,
    /// `USAGE_PERIOD_TYPE_MONTHLY`, or a period of another or unknown type.
    Monthly,
}

/// What the billing endpoint returned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GrokUsage {
    /// The credit pool: its period and the share used (0–1); `None` when the answer has no
    /// current period and no percent.
    pub pool: Option<(Period, f64)>,
    /// When the period ends (`currentPeriod.end`, else `billingPeriodEnd`).
    pub resets_at: Option<Timestamp>,
    /// On-demand spend and its cap in US dollars, when a cap is set.
    pub on_demand: Option<(f64, f64)>,
    /// Prepaid credit in US dollars, when there is some.
    pub prepaid: Option<f64>,
}

/// The Grok provider.
#[derive(Clone, Copy, Debug, Default)]
pub struct GrokProvider;

impl GrokProvider {
    /// The provider.
    pub fn new() -> GrokProvider {
        GrokProvider
    }
}

impl Provider for GrokProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Grok
    }

    /// Sign-in from `auth.json` → request → interpret.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::Grok { account, auth_file } = account else {
            return Err(FetchError::Unavailable {
                detail: "not a Grok account".to_owned(),
            });
        };
        let result = read_sign_in(ctx, auth_file).and_then(|sign_in| {
            let timeout = TIMEOUT.min(ctx.remaining());
            if timeout.is_zero() {
                return Err(FetchError::Transient {
                    from: SOURCE,
                    detail: "timed out".to_owned(),
                    retry_after: None,
                });
            }
            let response = ctx.http.request(&request(&sign_in, timeout));
            drop(sign_in);
            response
                .map_err(|e| FetchError::from_http(SOURCE, &e))
                .and_then(|r| {
                    log::debug!("grok: billing HTTP {}", r.status);
                    interpret(&r, ctx.clock.now())
                })
        });
        match result {
            Ok(usage) => Ok(to_snapshot(&usage, account.clone(), ctx.clock.now())),
            Err(e) => {
                log::warn!("grok: {e}");
                Err(e)
            }
        }
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    /// `auth.json` (size + modification time): it changes when the CLI renews or replaces the
    /// sign-in, which lifts the auth stop.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        let AccountSource::Grok { auth_file, .. } = account else {
            return None;
        };
        file_fingerprint(ctx, auth_file)
    }

    /// `grok login` in a terminal, with the account's folder when it is not the default one.
    fn sign_in_command(&self, account: &AccountSource) -> Option<TerminalCommand> {
        let AccountSource::Grok { auth_file, .. } = account else {
            return None;
        };
        Some(TerminalCommand {
            program: "grok".into(),
            args: vec!["login".into()],
            env: vec![(
                AUTH_PATH_VAR.into(),
                Some(auth_file.clone().into_os_string()),
            )],
            cwd: None,
        })
    }
}

// ---- The sign-in ------------------------------------------------------------------------------

/// One sign-in of `auth.json` (`GrokAuth`): only what the request needs; the refresh token and
/// the rest are skipped while parsing.
#[derive(Deserialize, Default)]
struct Entry {
    #[serde(default)]
    key: Text,
    #[serde(default)]
    auth_mode: Text,
    #[serde(default)]
    user_id: Text,
    #[serde(default)]
    expires_at: Text,
}

/// The token and user id a request needs.
pub struct SignIn {
    /// The bearer token.
    pub token: SecretString,
    /// The user id (`x-userid`), when the file gives one.
    pub user_id: Option<SecretString>,
}

/// Reads the sign-in from `auth_file` (read-only; the bytes are overwritten once parsed):
/// - no file, or no subscription sign-in in it (only an API key) → [`FetchError::Auth`];
/// - not JSON of the expected shape → [`FetchError::Parse`];
/// - the latest sign-in expires within [`EXPIRY_MARGIN`] → [`FetchError::Auth`].
pub fn read_sign_in(ctx: &FetchContext<'_>, auth_file: &Path) -> Result<SignIn, FetchError> {
    let mut bytes = match ctx.fs.read(auth_file, MAX_AUTH_BYTES) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::info!("grok: not signed in (no {AUTH_FILE})");
            return Err(FetchError::Auth { from: SOURCE });
        }
        Err(_) => {
            return Err(FetchError::Unavailable {
                detail: format!("cannot read the Grok CLI's {AUTH_FILE}"),
            });
        }
    };
    let parsed = serde_json::from_slice::<std::collections::BTreeMap<String, Entry>>(&bytes);
    wipe(&mut bytes);
    let store = parsed.map_err(|_| FetchError::Parse {
        from: SOURCE,
        detail: format!("the Grok CLI's {AUTH_FILE} is not understood"),
    })?;
    choose(store, ctx.clock.now())
}

/// The sign-in to use of `store` at `now` (see [`read_sign_in`]).
fn choose(
    store: std::collections::BTreeMap<String, Entry>,
    now: Timestamp,
) -> Result<SignIn, FetchError> {
    let expiry = |e: &Entry| {
        non_empty(&e.expires_at.0).and_then(|t| instant_of(&Value::String(t.to_owned())))
    };
    let mut best: Option<(Option<Timestamp>, Entry)> = None;
    for (scope, entry) in store {
        let api_key = scope == API_KEY_SCOPE || non_empty(&entry.auth_mode.0) == Some("api_key");
        if api_key || non_empty(&entry.key.0).is_none() {
            continue;
        }
        let expires = expiry(&entry);
        let better = best.as_ref().is_none_or(|(b, _)| expires > *b);
        if better {
            best = Some((expires, entry));
        }
    }
    let Some((expires, mut entry)) = best else {
        log::info!("grok: no subscription sign-in in {AUTH_FILE}");
        return Err(FetchError::Auth { from: SOURCE });
    };
    if expires.is_some_and(|t| t.duration_since(now) <= EXPIRY_MARGIN) {
        log::info!("grok: the sign-in has expired; the Grok CLI renews it when it next runs");
        return Err(FetchError::Auth { from: SOURCE });
    }
    Ok(SignIn {
        token: SecretString::new(entry.key.0.take().unwrap_or_default().trim().to_owned()),
        user_id: entry
            .user_id
            .0
            .take()
            .filter(|u| !u.trim().is_empty())
            .map(SecretString::new),
    })
}

// ---- The request ------------------------------------------------------------------------------

/// `GET BILLING_URL` with the Grok CLI's headers: `Authorization: Bearer <token>` and `x-userid`
/// (secret headers), `X-XAI-Token-Auth: xai-grok-cli`.
pub fn request(sign_in: &SignIn, timeout: Duration) -> HttpRequest {
    let mut headers = vec![
        (
            "Authorization".to_owned(),
            HeaderValue::Secret(SecretString::new(format!(
                "Bearer {}",
                sign_in.token.expose()
            ))),
        ),
        (
            "X-XAI-Token-Auth".to_owned(),
            HeaderValue::Plain(TOKEN_AUTH.to_owned()),
        ),
        (
            "Accept".to_owned(),
            HeaderValue::Plain("application/json".to_owned()),
        ),
    ];
    if let Some(user) = &sign_in.user_id {
        headers.push(("x-userid".to_owned(), HeaderValue::Secret(user.clone())));
    }
    HttpRequest {
        method: Method::Get,
        url: BILLING_URL.to_owned(),
        headers,
        body: None,
        timeout,
    }
}

/// 200 → [`parse_billing`]; 401 → Auth; 403/429 → RateLimited; 5xx → Transient; anything else →
/// Parse. `Retry-After` is read relative to `now`.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<GrokUsage, FetchError> {
    match response.status {
        200..=299 => parse_billing(&response.body),
        401 => Err(FetchError::Auth { from: SOURCE }),
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

/// A number, or a number in a string (protobuf-JSON writes 64-bit integers as strings).
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// A `{val}` amount in US cents as dollars: `{}` (protobuf's zero) is 0; not an object is `None`.
fn dollars(value: Option<&Value>) -> Option<f64> {
    let object = value?.as_object()?;
    Some(number(object.get("val")).unwrap_or(0.0) / 100.0)
}

/// The usage of an answer. Tolerant of unknown and missing fields; errors only when the body is
/// not a JSON object or says nothing Lastdrop can show.
pub fn parse_billing(body: &[u8]) -> Result<GrokUsage, FetchError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| parse_error("body is not JSON"))?;
    let top = value
        .as_object()
        .ok_or_else(|| parse_error("body is not a JSON object"))?;
    let config: &Map<String, Value> = top.get("config").and_then(Value::as_object).unwrap_or(top);
    let period = config.get("currentPeriod").and_then(Value::as_object);
    let instant = |object: Option<&Map<String, Value>>, key: &str| {
        object.and_then(|o| o.get(key)).and_then(instant_of)
    };
    let resets_at = instant(period, "end").or_else(|| instant(Some(config), "billingPeriodEnd"));
    let kind = period
        .and_then(|p| p.get("type"))
        .and_then(Value::as_str)
        .map(|t| {
            if t.ends_with("WEEKLY") {
                Period::Weekly
            } else {
                Period::Monthly
            }
        });
    let percent = number(config.get("creditUsagePercent"));
    let pool = match (percent, kind) {
        (Some(p), kind) => Some((kind.unwrap_or(Period::Monthly), (p / 100.0).max(0.0))),
        // protobuf-JSON leaves a zero out: a current period without a percent is unused.
        (None, Some(kind)) => Some((kind, 0.0)),
        (None, None) => None,
    };
    let on_demand = match (
        dollars(config.get("onDemandUsed")),
        dollars(config.get("onDemandCap")),
    ) {
        (used, Some(cap)) if cap > 0.0 => Some((used.unwrap_or(0.0).max(0.0), cap)),
        _ => None,
    };
    let prepaid = dollars(config.get("prepaidBalance")).filter(|p| *p > 0.0);
    if pool.is_none() && on_demand.is_none() && prepaid.is_none() {
        return Err(parse_error("no credit usage in the answer"));
    }
    Ok(GrokUsage {
        pool,
        resets_at,
        on_demand,
        prepaid,
    })
}

// ---- The snapshot -----------------------------------------------------------------------------

/// The window of a period.
fn window_of(period: Period) -> WindowLen {
    match period {
        Period::Weekly => WindowLen::Fixed(SignedDuration::from_hours(7 * 24)),
        Period::Monthly => WindowLen::CalendarMonth,
    }
}

/// The meters and the balance of a reading.
pub fn to_snapshot(usage: &GrokUsage, account: AccountKey, fetched_at: Timestamp) -> Snapshot {
    let mut meters = Vec::new();
    let period = usage.pool.map_or(Period::Monthly, |(p, _)| p);
    if let Some((period, used)) = usage.pool {
        let (id, names) = match period {
            Period::Weekly => (MeterId::WEEKLY, MeterNames::new("Wk", "weekly", "Weekly")),
            Period::Monthly => (
                MeterId::MONTHLY,
                MeterNames::new("Mo", "monthly", "Monthly"),
            ),
        };
        meters.push(Meter {
            id: MeterId::new(id),
            names,
            order: POOL_ORDER,
            used,
            limited: used >= 1.0,
            resets_at: usage.resets_at,
            window: window_of(period),
            starts_on_use: false,
        });
    }
    if let Some((used, cap)) = usage.on_demand {
        let share = used / cap;
        meters.push(Meter {
            id: MeterId::new(MeterId::ON_DEMAND),
            names: MeterNames::new("Extra", "on-demand", "On-demand spend"),
            order: ON_DEMAND_ORDER,
            used: share,
            limited: share >= 1.0,
            resets_at: usage.resets_at,
            window: window_of(period),
            starts_on_use: false,
        });
    }
    let balances = usage
        .prepaid
        .map(|amount| Balance {
            id: MeterId::new(MeterId::PREPAID),
            names: MeterNames::new("Pre", "prepaid credit", "Prepaid credit"),
            order: PREPAID_ORDER,
            amount,
            unit: Unit::Currency {
                code: "USD".to_owned(),
            },
            last_top_up: None,
        })
        .into_iter()
        .collect();
    Snapshot {
        provider: ProviderId::Grok,
        account: AccountRef {
            key: account,
            ..AccountRef::primary()
        },
        source: SOURCE,
        // A live reading: the values are true when read.
        measured_at: fetched_at,
        fetched_at,
        meters,
        balances,
        health: Health::Ok,
    }
}
