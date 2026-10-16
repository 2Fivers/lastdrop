//! Services described in JSON (docs/design/custom-services-spec.md §5): one provider per loaded
//! file (`lastdrop_core::custom`), reading the file's endpoint with the API key stored in
//! Lastdrop's settings (Credential Manager, `Lastdrop:<id>:<account>:api_key`; a secret header,
//! never logged) and taking the numbers at the file's JSON pointers.
//!
//! A pointer that finds nothing, or not a number, gives no value for its meter or balance: it is
//! left out, never 0. Numbers may be JSON numbers or numbers in strings (`"110.00"`).

use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::custom::{self, Currency, CustomId, CustomService, Used};
use lastdrop_core::model::{
    AccountKey, AccountRef, Balance, Health, Meter, MeterId, ProviderId, Snapshot, SourceKind,
    Unit, catalog,
};
use lastdrop_core::secret::SecretString;
use serde_json::Value;

use crate::account::AccountSource;
use crate::claude::json::instant_of;
use crate::error::FetchError;
use crate::io::{HeaderValue, HttpRequest, HttpResponse, Method};
use crate::{FetchContext, Provider};

/// Timeout of the request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Lower bound between fetches: 5 minutes (spec §1; the rule for endpoints Lastdrop does not know).
pub const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The source every described service's reading and error carries.
const SOURCE: SourceKind = SourceKind::CustomApi;

/// The provider of one described service.
#[derive(Clone, Copy, Debug)]
pub struct CustomProvider {
    id: CustomId,
}

impl CustomProvider {
    /// The provider of the loaded service `id`.
    pub fn new(id: CustomId) -> CustomProvider {
        CustomProvider { id }
    }

    /// Its definition, `None` when the registry has no such service (never, once loaded).
    fn service(&self) -> Option<&'static CustomService> {
        custom::get(self.id).map(|r| &r.service)
    }
}

impl Provider for CustomProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Custom(self.id)
    }

    /// Key from the secret store (none → Auth, the API-key message) → request → interpret.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let (
            Some(service),
            AccountSource::Custom {
                account,
                credential,
                ..
            },
        ) = (self.service(), account)
        else {
            return Err(FetchError::Unavailable {
                detail: "not a described service".to_owned(),
            });
        };
        let key = match ctx.secrets.get(credential) {
            Ok(Some(key)) if !key.is_empty() => key,
            Ok(_) => {
                log::info!("{}: no API key stored", service.id);
                return Err(FetchError::Auth { from: SOURCE });
            }
            Err(e) => {
                log::warn!("{}: cannot read the stored key: {e}", service.id);
                return Err(FetchError::Unavailable {
                    detail: "the API key could not be read".to_owned(),
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
        let response = ctx.http.request(&request(service, &key, timeout));
        drop(key);
        let result = response
            .map_err(|e| FetchError::from_http(SOURCE, &e))
            .and_then(|r| {
                log::debug!("{}: HTTP {}", service.id, r.status);
                interpret(&r, ctx.clock.now())
            })
            .and_then(|body| {
                to_snapshot(
                    service,
                    ProviderId::Custom(self.id),
                    &body,
                    account.clone(),
                    ctx.clock.now(),
                )
            });
        if let Err(e) = &result {
            log::warn!("{}: {e}", service.id);
        }
        result
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    fn usage_page(&self) -> Option<&'static str> {
        self.service().and_then(|s| s.usage_page.as_deref())
    }
}

/// `GET url` with the key in the file's header (a secret header) and `Accept: application/json`.
pub fn request(service: &CustomService, key: &SecretString, timeout: Duration) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        url: service.url.clone(),
        headers: vec![
            (
                service.key_header.clone(),
                HeaderValue::Secret(SecretString::new(format!(
                    "{}{}",
                    service.key_prefix,
                    key.expose()
                ))),
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

/// 200 → the body as JSON; 401 → Auth (the key is wrong: the API-key message); 403/429 →
/// RateLimited; 5xx → Transient; anything else → Parse.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<Value, FetchError> {
    match response.status {
        200..=299 => serde_json::from_slice(&response.body).map_err(|_| FetchError::Parse {
            from: SOURCE,
            detail: "body is not JSON".to_owned(),
        }),
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

/// The number at `pointer`: a JSON number, or a number in a string.
fn number_at(body: &Value, pointer: &str) -> Option<f64> {
    match body.pointer(pointer)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// The meters and balances of an answer, by the file. Errors when none could be read.
pub fn to_snapshot(
    service: &CustomService,
    provider: ProviderId,
    body: &Value,
    account: AccountKey,
    fetched_at: Timestamp,
) -> Result<Snapshot, FetchError> {
    let defs = catalog(provider);
    let order = |id: &str| defs.iter().find(|d| d.id == id).map_or(0, |d| d.order);
    let meters: Vec<Meter> = service
        .meters
        .iter()
        .filter_map(|m| {
            let used = match &m.used {
                Used::OfLimit { used, limit } => {
                    let limit = number_at(body, limit).filter(|l| *l > 0.0)?;
                    number_at(body, used)? / limit
                }
                Used::Percent(p) => number_at(body, p)? / 100.0,
            }
            .max(0.0);
            Some(Meter {
                id: MeterId::new(&m.id),
                names: lastdrop_core::model::MeterNames::new(&m.label, &m.short_name, &m.name),
                order: order(&m.id),
                used,
                limited: used >= 1.0,
                resets_at: m
                    .resets_at
                    .as_deref()
                    .and_then(|p| body.pointer(p))
                    .and_then(instant_of),
                window: m.window,
                starts_on_use: false,
            })
        })
        .collect();
    let balances: Vec<Balance> = service
        .balances
        .iter()
        .filter_map(|b| {
            let minus = match &b.minus {
                Some(p) => number_at(body, p)?,
                None => 0.0,
            };
            let amount = (number_at(body, &b.amount)? - minus) * b.scale;
            let unit = match &b.currency {
                Currency::Credits => Unit::Credits,
                Currency::Code(code) => Unit::Currency { code: code.clone() },
                Currency::At(p) => {
                    let code = body.pointer(p)?.as_str()?.trim().to_ascii_uppercase();
                    if code.len() != 3 || !code.bytes().all(|c| c.is_ascii_uppercase()) {
                        return None;
                    }
                    Unit::Currency { code }
                }
            };
            Some(Balance {
                id: MeterId::new(&b.id),
                names: lastdrop_core::model::MeterNames::new(&b.label, &b.short_name, &b.name),
                order: order(&b.id),
                amount,
                unit,
                last_top_up: None,
            })
        })
        .collect();
    if meters.is_empty() && balances.is_empty() {
        return Err(FetchError::Parse {
            from: SOURCE,
            detail: "none of the file's numbers in the answer".to_owned(),
        });
    }
    Ok(Snapshot {
        provider,
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
    })
}
