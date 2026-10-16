//! Cursor: the plan's included usage per pool and on-demand spend, per billing cycle.
//!
//! Source (ARCHITECTURE provider table): `GET https://cursor.com/api/usage-summary`, the
//! dashboard's own (undocumented) endpoint, with the session the Cursor app keeps: the token
//! under `cursorAuth/accessToken` in `ItemTable` of its `state.vscdb`, read with Lastdrop's own
//! read-only SQLite reader ([`crate::sqlite`]: the file can be 1.6 GB, so it is never read whole
//! or copied). The cookie is `WorkosCursorSessionToken=<user>%3A%3A<token>`, `<user>` being the
//! token's `sub` after its last `|`. The token is used in memory for the one request and dropped;
//! Lastdrop never renews it (the Cursor app does, when it runs).
//!
//! The answer: `billingCycleStart`, `billingCycleEnd`, `membershipType`, `individualUsage.plan
//! {autoPercentUsed, apiPercentUsed, totalPercentUsed, used, limit}` (percents 0–100, money in
//! cents), `individualUsage.onDemand {enabled, used, limit}` and, for enterprise members,
//! `individualUsage.overall`. The two pools (Auto and Cursor's models; other models) become the
//! meters `auto` and `api`; a plan that reports one total becomes `included`; on-demand spend
//! against its limit becomes `on_demand`. All reset at the end of the billing cycle.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{
    AccountKey, AccountRef, Health, Meter, MeterId, MeterNames, ProviderId, Snapshot, SourceKind,
    WindowLen, fnv1a64,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::secret::SecretString;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::account::AccountSource;
use crate::claude::json::instant_of;
use crate::error::FetchError;
use crate::io::{HeaderValue, HttpRequest, HttpResponse, Method, TerminalCommand};
use crate::lenient::{jwt_payload, wipe};
use crate::sqlite::Db;
use crate::{FetchContext, Provider};

/// Cursor's state database, relative to `%APPDATA%`.
pub const STATE_DB: &str = r"Cursor\User\globalStorage\state.vscdb";

/// The table and key that hold the session token.
pub const TABLE: &str = "ItemTable";
/// See [`TABLE`].
pub const TOKEN_KEY: &str = "cursorAuth/accessToken";

/// The usage endpoint (GET).
pub const USAGE_URL: &str = "https://cursor.com/api/usage-summary";

/// The dashboard's usage page, opened from a card.
pub const USAGE_PAGE: &str = "https://cursor.com/dashboard?tab=usage";

/// Timeout of the usage request (capped by the fetch deadline).
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// Lower bound between fetches: 5 minutes, the rule for undocumented endpoints (ARCHITECTURE
/// "Rules for every provider").
pub const MIN_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// A token this close to its expiry is treated as expired.
pub const EXPIRY_MARGIN: SignedDuration = SignedDuration::from_secs(60);

/// The source every Cursor reading and error carries.
const SOURCE: SourceKind = SourceKind::CursorApi;

/// One meter as Cursor reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct Pool {
    /// The meter id ([`MeterId::AUTO`], [`MeterId::API`], [`MeterId::INCLUDED`],
    /// [`MeterId::ON_DEMAND`]).
    pub id: &'static str,
    /// The share used, 0–1 (may exceed 1).
    pub used: f64,
}

/// What the endpoint returned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorUsage {
    /// The plan (`Pro`, `Pro+`, `Ultra`, …) when Cursor names one Lastdrop knows.
    pub plan: Option<String>,
    /// When the billing cycle started.
    pub cycle_start: Option<Timestamp>,
    /// When it ends: every meter resets then.
    pub resets_at: Option<Timestamp>,
    /// The meters, in meter order.
    pub pools: Vec<Pool>,
}

/// The Cursor provider. Remembers the last session fingerprint it read per database, so a read
/// that fails while Cursor writes keeps the fingerprint an auth stop is waiting on.
#[derive(Debug, Default)]
pub struct CursorProvider {
    fingerprints: Mutex<HashMap<PathBuf, CredentialFingerprint>>,
}

impl CursorProvider {
    /// The provider.
    pub fn new() -> CursorProvider {
        CursorProvider::default()
    }
}

impl Provider for CursorProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Cursor
    }

    /// Session from `state.vscdb` → request → interpret.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        let AccountSource::Cursor { account, state_db } = account else {
            return Err(FetchError::Unavailable {
                detail: "not a Cursor account".to_owned(),
            });
        };
        let result = read_token(ctx, state_db)
            .and_then(|token| session(&token, ctx.clock.now()))
            .and_then(|cookie| {
                let timeout = TIMEOUT.min(ctx.remaining());
                if timeout.is_zero() {
                    return Err(FetchError::Transient {
                        from: SOURCE,
                        detail: "timed out".to_owned(),
                        retry_after: None,
                    });
                }
                let response = ctx.http.request(&request(&cookie, timeout));
                drop(cookie);
                response
                    .map_err(|e| FetchError::from_http(SOURCE, &e))
                    .and_then(|r| {
                        log::debug!("cursor: usage-summary HTTP {}", r.status);
                        interpret(&r, ctx.clock.now())
                    })
            });
        match result {
            Ok(usage) => Ok(to_snapshot(&usage, account.clone(), ctx.clock.now())),
            Err(e) => {
                log::warn!("cursor: {e}");
                Err(e)
            }
        }
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    /// A hash of the session token itself: `state.vscdb` changes all the time while Cursor runs,
    /// the token only when Cursor signs in again or renews it, which lifts the auth stop. Signed
    /// out (no session, or Cursor not installed) is `token:none`, so signing in lifts the stop
    /// too; a read that fails while Cursor writes keeps the last fingerprint read (else
    /// `token:unreadable`), never none, which would stop the source for good.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        let AccountSource::Cursor { state_db, .. } = account else {
            return None;
        };
        let read = match read_token(ctx, state_db) {
            Ok(token) => {
                let hash = fnv1a64(&["cursor", token.expose()]);
                Some(CredentialFingerprint(format!("token:{hash:016x}")))
            }
            Err(FetchError::Auth { .. } | FetchError::Unavailable { .. }) => {
                Some(CredentialFingerprint("token:none".to_owned()))
            }
            Err(_) => None,
        };
        let mut known = self.fingerprints.lock().ok();
        match read {
            Some(fingerprint) => {
                if let Some(known) = known.as_mut() {
                    known.insert(state_db.clone(), fingerprint.clone());
                }
                Some(fingerprint)
            }
            None => Some(
                known
                    .as_ref()
                    .and_then(|k| k.get(state_db).cloned())
                    .unwrap_or_else(|| CredentialFingerprint("token:unreadable".to_owned())),
            ),
        }
    }

    /// `cursor`: the Cursor app renews its session when it starts.
    fn sign_in_command(&self, _account: &AccountSource) -> Option<TerminalCommand> {
        Some(TerminalCommand {
            program: "cursor".into(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
        })
    }

    fn usage_page(&self) -> Option<&'static str> {
        Some(USAGE_PAGE)
    }
}

// ---- The session --------------------------------------------------------------------------------

/// The session token from `state_db` and its write-ahead log `<state_db>-wal` (read-only):
/// - no database → [`FetchError::Unavailable`] (Cursor not installed);
/// - no token in it (signed out), twice in a row → [`FetchError::Auth`];
/// - unreadable, or written while it was read (Cursor at work, or a damaged file) →
///   [`FetchError::Transient`].
pub fn read_token(ctx: &FetchContext<'_>, state_db: &Path) -> Result<SecretString, FetchError> {
    match read_once(ctx, state_db)? {
        Some(token) => Ok(token),
        // A row missing for a moment (a checkpoint copying pages back) is read again once.
        None => read_once(ctx, state_db)?.ok_or_else(|| {
            log::info!("cursor: not signed in (no session in state.vscdb)");
            FetchError::Auth { from: SOURCE }
        }),
    }
}

/// One read of the session token: `None` when there is no session row.
fn read_once(ctx: &FetchContext<'_>, state_db: &Path) -> Result<Option<SecretString>, FetchError> {
    let file = ctx.fs.open_at(state_db).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FetchError::Unavailable {
                detail: "Cursor's state.vscdb not found".to_owned(),
            }
        } else {
            FetchError::Transient {
                from: SOURCE,
                detail: "cannot open Cursor's state.vscdb".to_owned(),
                retry_after: None,
            }
        }
    })?;
    let unreadable = |e: std::io::Error| {
        log::debug!("cursor: state.vscdb: {e}");
        FetchError::Transient {
            from: SOURCE,
            detail: "cannot read Cursor's state.vscdb".to_owned(),
            retry_after: None,
        }
    };
    let mut wal_path = OsString::from(state_db.as_os_str());
    wal_path.push("-wal");
    let wal = match ctx.fs.open_at(Path::new(&wal_path)) {
        Ok(wal) => Some(wal),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(unreadable(e)),
    };
    let mut db = Db::open(file, wal).map_err(unreadable)?;
    let Some(root) = db.table_root(TABLE).map_err(unreadable)? else {
        return Err(unreadable(std::io::Error::other("no ItemTable")));
    };
    let value = db.lookup(root, TOKEN_KEY).map_err(unreadable)?;
    if db.changed().map_err(unreadable)? {
        return Err(unreadable(std::io::Error::other("written while read")));
    }
    let Some(mut value) = value else {
        return Ok(None);
    };
    let token = value
        .bytes()
        .and_then(|b| std::str::from_utf8(b).ok())
        // VS Code's storage may keep a string JSON-quoted.
        .map(|t| SecretString::new(t.trim().trim_matches('"').to_owned()));
    value.wipe();
    Ok(token.filter(|t| !t.is_empty()))
}

/// The JWT claims the cookie needs.
#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    exp: Option<f64>,
}

/// The session cookie's value (`<user>%3A%3A<token>`) for `token` at `now`: the token must be a
/// JWT with a `sub`; one that expires within [`EXPIRY_MARGIN`] is [`FetchError::Auth`].
pub fn session(token: &SecretString, now: Timestamp) -> Result<SecretString, FetchError> {
    let mut payload = jwt_payload(token.expose()).ok_or(FetchError::Auth { from: SOURCE })?;
    let claims = serde_json::from_slice::<Claims>(&payload);
    wipe(&mut payload);
    let claims = claims.map_err(|_| FetchError::Auth { from: SOURCE })?;
    if let Some(exp) = claims
        .exp
        .filter(|e| e.is_finite())
        .and_then(|e| Timestamp::from_second(e as i64).ok())
        && exp.duration_since(now) <= EXPIRY_MARGIN
    {
        log::info!("cursor: the session has expired; open Cursor to renew it");
        return Err(FetchError::Auth { from: SOURCE });
    }
    let sub = claims.sub.unwrap_or_default();
    let user = sub.rsplit('|').next().unwrap_or("").trim();
    if user.is_empty() {
        return Err(FetchError::Auth { from: SOURCE });
    }
    Ok(SecretString::new(format!("{user}%3A%3A{}", token.expose())))
}

// ---- The request --------------------------------------------------------------------------------

/// `GET USAGE_URL` with the session cookie (a secret header).
pub fn request(session: &SecretString, timeout: Duration) -> HttpRequest {
    HttpRequest {
        method: Method::Get,
        url: USAGE_URL.to_owned(),
        headers: vec![
            (
                "Cookie".to_owned(),
                HeaderValue::Secret(SecretString::new(format!(
                    "WorkosCursorSessionToken={}",
                    session.expose()
                ))),
            ),
            (
                "Accept".to_owned(),
                HeaderValue::Plain("application/json".to_owned()),
            ),
            (
                "User-Agent".to_owned(),
                HeaderValue::Plain("Mozilla/5.0".to_owned()),
            ),
        ],
        body: None,
        timeout,
    }
}

/// 200 → [`parse_summary`]; 401 → Auth (the session is no longer valid); 403/429 →
/// RateLimited (ARCHITECTURE "back off on 429/403"); 5xx → Transient; anything else → Parse.
pub fn interpret(response: &HttpResponse, now: Timestamp) -> Result<CursorUsage, FetchError> {
    match response.status {
        200..=299 => parse_summary(&response.body),
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

fn number(object: &Map<String, Value>, key: &str) -> Option<f64> {
    object.get(key)?.as_f64().filter(|n| n.is_finite())
}

/// used / limit of an amount object, when it is enabled (or does not say) and has a limit.
fn share(object: Option<&Map<String, Value>>) -> Option<f64> {
    let object = object?;
    if object.get("enabled").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let limit = number(object, "limit").filter(|l| *l > 0.0)?;
    Some((number(object, "used")?.max(0.0)) / limit)
}

/// The plan badge of a `membershipType`; `None` for one Lastdrop does not know.
fn plan_name(membership: &str) -> Option<String> {
    let name = match membership.trim().to_ascii_lowercase().as_str() {
        "free" | "hobby" => "Hobby",
        "pro" => "Pro",
        "pro_plus" | "pro-plus" | "proplus" => "Pro+",
        "ultra" => "Ultra",
        "team" | "business" => "Teams",
        "enterprise" => "Enterprise",
        _ => return None,
    };
    Some(name.to_owned())
}

/// The usage of an answer. Tolerant of unknown and missing fields; a pool without a usable share
/// is left out (never 0). Errors when the body is not a JSON object or reports no pool.
pub fn parse_summary(body: &[u8]) -> Result<CursorUsage, FetchError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| parse_error("body is not JSON"))?;
    let top = value
        .as_object()
        .ok_or_else(|| parse_error("body is not a JSON object"))?;
    let individual = top.get("individualUsage").and_then(Value::as_object);
    let part = |key: &str| {
        individual
            .and_then(|i| i.get(key))
            .and_then(Value::as_object)
    };
    let plan = part("plan").filter(|p| p.get("enabled").and_then(Value::as_bool) != Some(false));
    let percent = |key: &str| {
        plan.and_then(|p| number(p, key))
            .map(|p| (p / 100.0).max(0.0))
    };
    let mut pools = Vec::new();
    let (auto, api) = (percent("autoPercentUsed"), percent("apiPercentUsed"));
    if auto.is_some() || api.is_some() {
        pools.extend(auto.map(|used| Pool {
            id: MeterId::AUTO,
            used,
        }));
        pools.extend(api.map(|used| Pool {
            id: MeterId::API,
            used,
        }));
    } else if let Some(used) = percent("totalPercentUsed")
        .or_else(|| share(plan))
        .or_else(|| share(part("overall")))
    {
        pools.push(Pool {
            id: MeterId::INCLUDED,
            used,
        });
    }
    if let Some(used) = share(part("onDemand")) {
        pools.push(Pool {
            id: MeterId::ON_DEMAND,
            used,
        });
    }
    if pools.is_empty() {
        return Err(parse_error("no usage pool in the answer"));
    }
    Ok(CursorUsage {
        plan: top
            .get("membershipType")
            .and_then(Value::as_str)
            .and_then(plan_name),
        cycle_start: top.get("billingCycleStart").and_then(instant_of),
        resets_at: top.get("billingCycleEnd").and_then(instant_of),
        pools,
    })
}

// ---- The snapshot -------------------------------------------------------------------------------

/// The names and order of a Cursor meter (English; the surfaces show
/// `lastdrop_core::grammar::meters::localized`).
pub fn meter_names(id: &str) -> (MeterNames, u16) {
    match id {
        MeterId::AUTO => (
            MeterNames::new("Auto", "Auto", "Included · Auto and Cursor models"),
            0,
        ),
        MeterId::API => (MeterNames::new("API", "API", "Included · other models"), 1),
        MeterId::INCLUDED => (
            MeterNames::new("Incl", "included usage", "Included usage"),
            0,
        ),
        _ => (MeterNames::new("Extra", "on-demand", "On-demand spend"), 2),
    }
}

/// The snapshot of a reading: every pool is a window of the billing cycle (its own length when
/// both ends are known, else a calendar month).
pub fn to_snapshot(usage: &CursorUsage, account: AccountKey, fetched_at: Timestamp) -> Snapshot {
    let window = match (usage.cycle_start, usage.resets_at) {
        (Some(start), Some(end)) if end > start => WindowLen::Fixed(end.duration_since(start)),
        _ => WindowLen::CalendarMonth,
    };
    let meters = usage
        .pools
        .iter()
        .map(|p| {
            let (names, order) = meter_names(p.id);
            Meter {
                id: MeterId::new(p.id),
                names,
                order,
                used: p.used,
                limited: p.used >= 1.0,
                resets_at: usage.resets_at,
                window,
                starts_on_use: false,
            }
        })
        .collect();
    Snapshot {
        provider: ProviderId::Cursor,
        account: AccountRef {
            key: account,
            plan: usage.plan.clone(),
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
