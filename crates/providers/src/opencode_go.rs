//! OpenCode Go: the subscription's 5-hour (rolling), weekly and monthly windows.
//!
//! Source (ARCHITECTURE provider table): `GET https://opencode.ai/zen/go/v1/usage`, which
//! OpenCode's own console implements (anomalyco/opencode,
//! `packages/console/app/src/routes/zen/go/v1/usage.ts`; not in the public docs), with the Go API
//! key as a bearer token. The key is the one stored in Lastdrop's settings (Credential Manager,
//! `Lastdrop:opencode_go:<account>:api_key`), else the one OpenCode keeps in its `auth.json`
//! (`{"opencode-go": {"type": "api", "key": …}}`, read-only). It is a secret header and never
//! logged.
//!
//! The answer is `{usage: {rolling|weekly|monthly: {status: "ok"|"rate-limited", percent,
//! resetsAt}}}`: `percent` 0–100 (a float for some keys), `resetsAt` an instant. The weekly
//! window resets on Monday 00:00 UTC, the monthly one on the subscription's day; an unused rolling
//! window reports its reset a whole window from now, so it starts on use.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{
    AccountKey, AccountRef, Health, Meter, MeterId, ProviderId, Snapshot, SourceKind, catalog,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::secret::SecretString;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::account::AccountSource;
use crate::claude::json::instant_of;
use crate::error::FetchError;
use crate::fingerprint::file_fingerprint;
use crate::io::{HeaderValue, HttpRequest, HttpResponse, Method};
use crate::lenient::{Text, non_empty, wipe};
use crate::{FetchContext, Provider};

/// The usage endpoint (GET).
pub const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";

/// OpenCode's `auth.json`, relative to the profile folder (OpenCode's troubleshooting docs).
pub const AUTH_FILE: &str = r".local\share\opencode\auth.json";

/// The entry of `auth.json` that holds the Go key.
const AUTH_ENTRY: &str = "opencode-go";

/// Largest `auth.json` read.
const MAX_AUTH_BYTES: u64 = 256 << 10;

/// Timeout of the usage request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Lower bound between fetches: 5 minutes, the rule for undocumented endpoints (ARCHITECTURE
/// "Rules for every provider").
pub const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The source every OpenCode Go reading and error carries.
const SOURCE: SourceKind = SourceKind::OpenCodeGoApi;

/// The answer's windows and the meters they become.
const WINDOWS: [(&str, &str); 3] = [
    ("rolling", MeterId::FIVE_HOUR),
    ("weekly", MeterId::WEEKLY),
    ("monthly", MeterId::MONTHLY),
];

/// One window as OpenCode reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// The meter id ([`MeterId::FIVE_HOUR`], [`MeterId::WEEKLY`], [`MeterId::MONTHLY`]).
    pub id: &'static str,
    /// The share used, 0–1.
    pub used: f64,
    /// `status: "rate-limited"`, or used up.
    pub limited: bool,
    /// `resetsAt`.
    pub resets_at: Option<Timestamp>,
}

/// The OpenCode Go provider.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenCodeGoProvider;

impl OpenCodeGoProvider {
    /// The provider.
    pub fn new() -> OpenCodeGoProvider {
        OpenCodeGoProvider
    }
}

impl Provider for OpenCodeGoProvider {
    fn id(&self) -> ProviderId {
        ProviderId::OpenCodeGo
    }

    /// Key (Lastdrop's, else OpenCode's; none → Auth, the API-key message) → request →
    /// interpret.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::OpenCodeGo {
            account,
            credential,
            auth_file,
        } = account
        else {
            return Err(FetchError::Unavailable {
                detail: "not an OpenCode Go account".to_owned(),
            });
        };
        let key = match ctx.secrets.get(credential) {
            Ok(Some(key)) if !key.is_empty() => Some(key),
            Ok(_) => None,
            Err(e) => {
                log::warn!("opencode_go: cannot read the stored key: {e}");
                None
            }
        }
        .or_else(|| key_of_opencode(ctx, auth_file));
        let Some(key) = key else {
            log::info!("opencode_go: no Go API key (none stored, none in OpenCode's auth.json)");
            return Err(FetchError::Auth { from: SOURCE });
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
                log::debug!("opencode_go: usage HTTP {}", r.status);
                interpret(&r, ctx.clock.now())
            });
        match result {
            Ok(windows) => Ok(to_snapshot(&windows, account.clone(), ctx.clock.now())),
            Err(e) => {
                log::warn!("opencode_go: {e}");
                Err(e)
            }
        }
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    /// OpenCode's `auth.json`: a key added there (`/connect`) lifts the auth stop. A key stored in
    /// Lastdrop's settings lifts it through the settings window, as Meshy's does.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        let AccountSource::OpenCodeGo { auth_file, .. } = account else {
            return None;
        };
        file_fingerprint(ctx, auth_file)
    }
}

/// An `auth.json` entry: only the kind and the key are read.
#[derive(Deserialize, Default)]
struct AuthEntry {
    #[serde(rename = "type", default)]
    kind: Text,
    #[serde(default)]
    key: Text,
}

/// The Go key OpenCode keeps in `auth_file` (read-only; the bytes are overwritten once parsed),
/// `None` when there is no file, no `opencode-go` API entry, or the file is not understood.
fn key_of_opencode(ctx: &FetchContext<'_>, auth_file: &Path) -> Option<SecretString> {
    let mut bytes = ctx.fs.read(auth_file, MAX_AUTH_BYTES).ok()?;
    let parsed = serde_json::from_slice::<BTreeMap<String, Lenient>>(&bytes);
    wipe(&mut bytes);
    let mut store = parsed.ok()?;
    let mut entry = store.remove(AUTH_ENTRY)?.0?;
    if non_empty(&entry.kind.0) != Some("api") {
        return None;
    }
    let key = entry.key.0.take()?;
    let key = SecretString::new(key.trim().to_owned());
    (!key.is_empty()).then_some(key)
}

/// An entry of another shape (another provider's OAuth sign-in) reads as `None`, so it never
/// fails the file.
struct Lenient(Option<AuthEntry>);

impl<'de> Deserialize<'de> for Lenient {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Lenient, D::Error> {
        // Every field of `AuthEntry` is lenient, so an object always reads; anything else
        // (a string, a list) is skipped.
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Lenient;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an auth entry")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Lenient, A::Error> {
                AuthEntry::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(|e| Lenient(Some(e)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Lenient, E> {
                Ok(Lenient(None))
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<Lenient, E> {
                Ok(Lenient(None))
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<Lenient, E> {
                Ok(Lenient(None))
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<Lenient, E> {
                Ok(Lenient(None))
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<Lenient, E> {
                Ok(Lenient(None))
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<Lenient, E> {
                Ok(Lenient(None))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Lenient, A::Error> {
                while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {}
                Ok(Lenient(None))
            }
        }
        deserializer.deserialize_any(V)
    }
}

/// `GET USAGE_URL` with `Authorization: Bearer <key>` (a secret header).
pub fn request(key: &SecretString, timeout: Duration) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        url: USAGE_URL.to_owned(),
        headers: vec![
            (
                "Authorization".to_owned(),
                HeaderValue::Secret(SecretString::new(format!("Bearer {}", key.expose()))),
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

/// 200 → [`parse_usage`]; 401 → Auth (the key is wrong or revoked); 403 with an
/// `EntitlementError` → Unavailable (no Go subscription), other 403 and 429 → RateLimited; 5xx →
/// Transient; anything else → Parse.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<Vec<Window>, FetchError> {
    match response.status {
        200..=299 => parse_usage(&response.body),
        401 => Err(FetchError::Auth { from: SOURCE }),
        403 if error_type(&response.body).as_deref() == Some("EntitlementError") => {
            Err(FetchError::Unavailable {
                detail: "no OpenCode Go subscription on this key".to_owned(),
            })
        }
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

/// `error.type` of an error body.
fn error_type(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value.get("error")?.get("type")?.as_str().map(str::to_owned)
}

fn parse_error(detail: &str) -> FetchError {
    FetchError::Parse {
        from: SOURCE,
        detail: detail.to_owned(),
    }
}

/// The windows of an answer, in meter order. Tolerant of unknown and missing fields; a window
/// without a usable percent is left out (never 0). Errors when the body is not JSON or has no
/// window at all.
pub fn parse_usage(body: &[u8]) -> Result<Vec<Window>, FetchError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| parse_error("body is not JSON"))?;
    let top = value
        .as_object()
        .ok_or_else(|| parse_error("body is not a JSON object"))?;
    let usage: &Map<String, Value> = top.get("usage").and_then(Value::as_object).unwrap_or(top);
    let windows: Vec<Window> = WINDOWS
        .iter()
        .filter_map(|(key, id)| {
            let window = usage.get(*key)?.as_object()?;
            let percent = window
                .get("percent")
                .and_then(Value::as_f64)
                .filter(|p| p.is_finite())?;
            let used = (percent / 100.0).max(0.0);
            let rate_limited = window.get("status").and_then(Value::as_str) == Some("rate-limited");
            Some(Window {
                id,
                used,
                limited: rate_limited || used >= 1.0,
                resets_at: window.get("resetsAt").and_then(instant_of),
            })
        })
        .collect();
    if windows.is_empty() {
        return Err(parse_error("no usage window in the answer"));
    }
    Ok(windows)
}

/// The snapshot of a reading: each window with its catalogue entry's names and length.
pub fn to_snapshot(windows: &[Window], account: AccountKey, fetched_at: Timestamp) -> Snapshot {
    let meters = windows
        .iter()
        .filter_map(|w| {
            let def = catalog(ProviderId::OpenCodeGo)
                .iter()
                .find(|d| d.id == w.id)?;
            let lastdrop_core::model::MeterKind::Window {
                window,
                starts_on_use,
            } = def.kind
            else {
                return None;
            };
            Some(Meter {
                id: def.meter_id(),
                names: def.names(),
                order: def.order,
                used: w.used,
                limited: w.limited,
                resets_at: w.resets_at,
                window,
                starts_on_use,
            })
        })
        .collect();
    Snapshot {
        provider: ProviderId::OpenCodeGo,
        account: AccountRef {
            key: account,
            ..AccountRef::primary()
        },
        source: SOURCE,
        // A live reading: the values are true when read.
        measured_at: fetched_at,
        fetched_at,
        meters,
        balances: Vec::new(),
        health: Health::Ok,
    }
}
