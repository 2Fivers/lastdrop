//! Meshy: a credit balance (R§2 `Cr` / `credits` / `Credits`, R§3.6).
//!
//! Source (ARCHITECTURE provider table): `GET https://api.meshy.ai/openapi/v1/balance` with the
//! API key from Credential Manager (`Lastdrop:meshy:<account>:api_key`), stored there by the
//! settings window. The key is a secret header and never logged.
//!
//! The answer is `{"balance": <credits>}`. Meshy reports no reset date (paid plans reset on the
//! subscription anniversary, which the API does not expose), so the snapshot has a balance and no
//! window meter; R§3.6 detects top-ups from history. Without a stored key the fetch fails with
//! [`FetchError::Auth`], which the flyout shows as the API-key message (`MessageKind::ApiKey`,
//! because Meshy has no vendor sign-in client).

use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{
    AccountRef, Balance, Health, MeterId, MeterNames, ProviderId, Snapshot, SourceKind, Unit,
    catalog,
};
use lastdrop_core::secret::SecretString;

use crate::account::AccountSource;
use crate::error::FetchError;
use crate::io::{HeaderValue, HttpRequest, HttpResponse, Method};
use crate::{FetchContext, Provider};

/// The documented balance endpoint (GET).
pub const BALANCE_URL: &str = "https://api.meshy.ai/openapi/v1/balance";

/// The credential name of the API key within the account.
pub const API_KEY_NAME: &str = "api_key";

/// Timeout of the balance request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Lower bound between fetches: 5 minutes (**not from the spec**: a documented endpoint, but a
/// balance changes slowly and R§3.6 needs no finer history).
pub const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The source every Meshy reading and error carries.
const SOURCE: SourceKind = SourceKind::MeshyApi;

/// The Meshy provider.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeshyProvider;

impl MeshyProvider {
    /// The provider.
    pub fn new() -> MeshyProvider {
        MeshyProvider
    }
}

impl Provider for MeshyProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Meshy
    }

    /// Key from the secret store (missing → `FetchError::Auth`) → request → interpret.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::Meshy {
            account,
            credential,
        } = account
        else {
            return Err(FetchError::Unavailable {
                detail: "not a Meshy account".to_owned(),
            });
        };
        let key = match ctx.secrets.get(credential) {
            Ok(Some(key)) if !key.is_empty() => key,
            Ok(_) => {
                log::info!("meshy: no API key stored");
                return Err(FetchError::Auth { from: SOURCE });
            }
            Err(e) => {
                log::warn!("meshy: {e}");
                return Err(FetchError::Transient {
                    from: SOURCE,
                    detail: "credential store failed".to_owned(),
                    retry_after: None,
                });
            }
        };
        let timeout = TIMEOUT.min(ctx.remaining());
        if timeout.is_zero() {
            return Err(FetchError::Transient {
                from: SOURCE,
                detail: "timed out".to_owned(),
                retry_after: None,
            });
        }
        let response = ctx.http.request(&request(&key, timeout));
        drop(key);
        let result = response
            .map_err(|e| FetchError::from_http(SOURCE, &e))
            .and_then(|r| {
                log::debug!("meshy: balance HTTP {}", r.status);
                interpret(&r, ctx.clock.now())
            });
        match result {
            Ok(amount) => {
                let account = AccountRef {
                    key: account.clone(),
                    ..AccountRef::primary()
                };
                Ok(to_snapshot(amount, account, ctx.clock.now()))
            }
            Err(e) => {
                log::warn!("meshy: {e}");
                Err(e)
            }
        }
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }
}

/// `GET BALANCE_URL` with `Authorization: Bearer <key>` (a secret header).
pub fn request(key: &SecretString, timeout: Duration) -> HttpRequest {
    let bearer = SecretString::new(format!("Bearer {}", key.expose()));
    HttpRequest {
        method: Method::Get,
        url: BALANCE_URL.to_owned(),
        headers: vec![
            ("Authorization".to_owned(), HeaderValue::Secret(bearer)),
            (
                "Accept".to_owned(),
                HeaderValue::Plain("application/json".to_owned()),
            ),
        ],
        body: None,
        timeout,
    }
}

/// 200 → [`parse_balance`]; 401 → Auth; 429/403 → RateLimited; 5xx → Transient.
///
/// Any other status (a redirect, which is never followed, or another 4xx) means the endpoint no
/// longer answers as documented: [`FetchError::Parse`], so the scheduler keeps its normal
/// interval instead of retrying hard. `Retry-After` is read relative to `now`.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<f64, FetchError> {
    match response.status {
        200..=299 => parse_balance(&response.body),
        401 => Err(FetchError::Auth { from: SOURCE }),
        403 | 429 => Err(FetchError::RateLimited {
            from: SOURCE,
            retry_after: retry_after(response, now),
        }),
        500..=599 => Err(FetchError::Transient {
            from: SOURCE,
            detail: format!("HTTP {}", response.status),
            retry_after: retry_after(response, now),
        }),
        status => Err(FetchError::Parse {
            from: SOURCE,
            detail: format!("HTTP {status}"),
        }),
    }
}

/// The `balance` field of the JSON body (credits); tolerant of unknown fields, an error when
/// missing (never 0).
pub fn parse_balance(body: &[u8]) -> Result<f64, FetchError> {
    let parse_error = |detail: &str| FetchError::Parse {
        from: SOURCE,
        detail: detail.to_owned(),
    };
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| parse_error("body is not JSON"))?;
    let object = value
        .as_object()
        .ok_or_else(|| parse_error("body is not a JSON object"))?;
    match object.get("balance") {
        None | Some(serde_json::Value::Null) => Err(parse_error("missing balance")),
        Some(balance) => balance
            .as_f64()
            .filter(|b| b.is_finite())
            .ok_or_else(|| parse_error("balance is not a number")),
    }
}

/// The credits balance meter (`credits`, unit credits, no reported top-up: R§3.6 detects top-ups
/// from history).
pub fn balance(amount: f64) -> Balance {
    let def = catalog(ProviderId::Meshy)
        .iter()
        .find(|d| d.id == MeterId::CREDITS);
    let (names, order) = match def {
        Some(def) => (def.names(), def.order),
        // The catalogue always has it (lastdrop_core::model); keep the R§2 names regardless.
        None => (MeterNames::new("Cr", "credits", "Credits"), 0),
    };
    Balance {
        id: MeterId::new(MeterId::CREDITS),
        names,
        order,
        amount,
        unit: Unit::Credits,
        last_top_up: None,
    }
}

/// The snapshot of a balance reading.
pub fn to_snapshot(amount: f64, account: AccountRef, fetched_at: Timestamp) -> Snapshot {
    Snapshot {
        provider: ProviderId::Meshy,
        account,
        source: SOURCE,
        // A live reading: the values are true when read.
        measured_at: fetched_at,
        fetched_at,
        meters: Vec::new(),
        balances: vec![balance(amount)],
        health: Health::Ok,
    }
}

/// `Retry-After` as a duration from `now`: delta-seconds or an HTTP-date (RFC 9110 §10.2.3).
///
/// The same rule as `HttpResponse::retry_after` in [`crate::io`], which is still a contract stub
/// on this branch; once it is implemented this can call it instead.
fn retry_after(response: &HttpResponse, now: Timestamp) -> Option<Duration> {
    let value = response.header("Retry-After")?.trim();
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return value.parse::<u64>().ok().map(Duration::from_secs);
    }
    let at = jiff::fmt::rfc2822::DateTimeParser::new()
        .parse_timestamp(value)
        .ok()?;
    Some(Duration::try_from(at.duration_since(now)).unwrap_or(Duration::ZERO))
}
