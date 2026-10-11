//! The Meshy provider against `FakeHttp` with the invented fixtures in `tests/fixtures/meshy/`.
//! No test touches the network or Credential Manager.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountKey, Health, MeterId, ProviderId, SourceKind, Unit, catalog};
use lastdrop_core::secret::SecretString;
use lastdrop_providers::fakes::{FakeClock, FakeEnv, FakeFs, FakeHttp, FakeSpawner, Fakes};
use lastdrop_providers::io::{
    CredentialName, HeaderValue, HttpError, HttpResponse, Method, Secrets, SecretsError,
};
use lastdrop_providers::meshy::{self, MeshyProvider};
use lastdrop_providers::{AccountSource, FetchContext, FetchError, Provider};

const KEY: &str = "msy-test-key-not-real";

fn now() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("timestamp")
}

fn credential() -> CredentialName {
    CredentialName {
        provider: ProviderId::Meshy,
        account: AccountKey::primary(),
        name: meshy::API_KEY_NAME.to_owned(),
    }
}

fn account() -> AccountSource {
    AccountSource::Meshy {
        account: AccountKey::primary(),
        credential: credential(),
    }
}

/// Fakes with the invented key stored.
fn with_key() -> Fakes {
    let mut f = Fakes::new(now());
    f.secrets = f.secrets.with(&credential(), KEY);
    f
}

fn fixture(name: &str) -> HttpResponse {
    let body = match name {
        "balance-200.json" => include_str!("fixtures/meshy/balance-200.json"),
        "balance-200-extra-fields.json" => {
            include_str!("fixtures/meshy/balance-200-extra-fields.json")
        }
        "balance-200-missing-field.json" => {
            include_str!("fixtures/meshy/balance-200-missing-field.json")
        }
        "balance-200-null.json" => include_str!("fixtures/meshy/balance-200-null.json"),
        "balance-200-not-a-number.json" => {
            include_str!("fixtures/meshy/balance-200-not-a-number.json")
        }
        "balance-200-not-json.html" => include_str!("fixtures/meshy/balance-200-not-json.html"),
        "balance-401.json" => include_str!("fixtures/meshy/balance-401.json"),
        "balance-429.json" => include_str!("fixtures/meshy/balance-429.json"),
        "balance-503.txt" => include_str!("fixtures/meshy/balance-503.txt"),
        other => panic!("no fixture {other}"),
    };
    let status = name
        .split('-')
        .nth(1)
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse().ok())
        .expect("status in the fixture name");
    HttpResponse::new(status, body)
}

fn with_header(mut response: HttpResponse, name: &str, value: &str) -> HttpResponse {
    response.headers.push((name.to_owned(), value.to_owned()));
    response
}

fn fetch(f: &Fakes) -> Result<lastdrop_core::model::Snapshot, FetchError> {
    MeshyProvider::new().fetch(&f.context(SignedDuration::from_secs(30)), &account())
}

fn fetch_answer(response: HttpResponse) -> Result<lastdrop_core::model::Snapshot, FetchError> {
    let f = with_key();
    f.http.respond(Method::Get, meshy::BALANCE_URL, response);
    let result = fetch(&f);
    assert_eq!(f.http.requests().len(), 1);
    result
}

#[test]
fn reads_the_balance_with_the_stored_key() {
    let f = with_key();
    f.http
        .respond(Method::Get, meshy::BALANCE_URL, fixture("balance-200.json"));
    let snap = fetch(&f).expect("fetch");

    assert_eq!(snap.provider, ProviderId::Meshy);
    assert_eq!(snap.source, SourceKind::MeshyApi);
    assert_eq!(snap.account.key, AccountKey::primary());
    assert_eq!(snap.account.plan, None);
    assert_eq!((snap.measured_at, snap.fetched_at), (now(), now()));
    assert_eq!(snap.health, Health::Ok);
    assert!(snap.meters.is_empty(), "no window meter: no reset date");
    assert_eq!(snap.balances.len(), 1);
    let balance = &snap.balances[0];
    let def = &catalog(ProviderId::Meshy)[0];
    assert_eq!(balance.id, MeterId::new(MeterId::CREDITS));
    assert_eq!(balance.names, def.names());
    assert_eq!(balance.order, def.order);
    assert_eq!(balance.amount, 1240.0);
    assert_eq!(balance.unit, Unit::Credits);
    assert_eq!(balance.last_top_up, None);
    assert_eq!(f.http.pending(), 0);

    let requests = f.http.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, Method::Get);
    assert_eq!(request.url, "https://api.meshy.ai/openapi/v1/balance");
    assert_eq!(request.body, None);
    assert_eq!(request.timeout, meshy::TIMEOUT);
    let auth = request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v)
        .expect("Authorization header");
    assert!(
        matches!(auth, HeaderValue::Secret(_)),
        "the key is a secret"
    );
    assert_eq!(auth.as_str(), format!("Bearer {KEY}"));
    assert!(
        !format!("{request:?}").contains(KEY),
        "Debug never shows the key"
    );
}

#[test]
fn unknown_fields_and_fractions_are_fine() {
    let snap = fetch_answer(fixture("balance-200-extra-fields.json")).expect("fetch");
    assert_eq!(snap.balances[0].amount, 987.5);
}

#[test]
fn without_a_key_it_asks_for_one_and_sends_nothing() {
    let f = Fakes::new(now());
    let err = fetch(&f).expect_err("no key");
    assert_eq!(
        err,
        FetchError::Auth {
            from: SourceKind::MeshyApi
        }
    );
    assert!(f.http.requests().is_empty());

    let mut f = Fakes::new(now());
    f.secrets = f.secrets.with(&credential(), "");
    assert!(matches!(fetch(&f), Err(FetchError::Auth { .. })));
    assert!(f.http.requests().is_empty(), "an empty key is no key");
}

#[test]
fn a_rejected_key_is_an_auth_error() {
    let err = fetch_answer(fixture("balance-401.json")).expect_err("401");
    assert!(matches!(err, FetchError::Auth { .. }));
}

#[test]
fn throttling_honours_retry_after_in_seconds_and_as_a_date() {
    let err = fetch_answer(with_header(
        fixture("balance-429.json"),
        "Retry-After",
        "120",
    ))
    .expect_err("429");
    assert_eq!(
        err,
        FetchError::RateLimited {
            from: SourceKind::MeshyApi,
            retry_after: Some(Duration::from_secs(120)),
        }
    );

    // now() is 10:42:00 GMT.
    let err = fetch_answer(with_header(
        fixture("balance-429.json"),
        "retry-after",
        "Sat, 10 Oct 2026 10:45:30 GMT",
    ))
    .expect_err("429");
    assert_eq!(err.retry_after(), Some(Duration::from_secs(210)));

    let err = fetch_answer(fixture("balance-429.json")).expect_err("429");
    assert_eq!(
        err,
        FetchError::RateLimited {
            from: SourceKind::MeshyApi,
            retry_after: None,
        }
    );

    // A date in the past means "now"; garbage means "not given".
    let past = with_header(
        HttpResponse::new(429, ""),
        "Retry-After",
        "Sat, 10 Oct 2026 09:00:00 GMT",
    );
    assert_eq!(
        meshy::interpret(&past, now())
            .expect_err("429")
            .retry_after(),
        Some(Duration::ZERO)
    );
    let garbage = with_header(HttpResponse::new(429, ""), "Retry-After", "soon");
    assert_eq!(
        meshy::interpret(&garbage, now())
            .expect_err("429")
            .retry_after(),
        None
    );
}

#[test]
fn forbidden_is_treated_as_throttling() {
    let err = fetch_answer(HttpResponse::new(403, "{}")).expect_err("403");
    assert!(matches!(err, FetchError::RateLimited { .. }));
}

#[test]
fn server_errors_are_transient() {
    let err = fetch_answer(with_header(fixture("balance-503.txt"), "Retry-After", "30"))
        .expect_err("503");
    assert_eq!(
        err,
        FetchError::Transient {
            from: SourceKind::MeshyApi,
            detail: "HTTP 503".to_owned(),
            retry_after: Some(Duration::from_secs(30)),
        }
    );
}

#[test]
fn unexpected_answers_are_parse_errors_never_zero() {
    for name in [
        "balance-200-missing-field.json",
        "balance-200-null.json",
        "balance-200-not-a-number.json",
        "balance-200-not-json.html",
    ] {
        let err = fetch_answer(fixture(name)).expect_err(name);
        assert!(matches!(err, FetchError::Parse { .. }), "{name}: {err:?}");
    }
    // A redirect is never followed: the endpoint moved.
    let err = fetch_answer(HttpResponse::new(301, "")).expect_err("301");
    assert_eq!(
        err,
        FetchError::Parse {
            from: SourceKind::MeshyApi,
            detail: "HTTP 301".to_owned(),
        }
    );
    assert!(matches!(
        meshy::parse_balance(b"[1240]"),
        Err(FetchError::Parse { .. })
    ));
    assert_eq!(meshy::parse_balance(br#"{"balance": 0}"#), Ok(0.0));
}

#[test]
fn transport_failures_map_to_offline_and_transient() {
    let f = with_key();
    f.http
        .fail(Method::Get, meshy::BALANCE_URL, HttpError::Unreachable);
    assert_eq!(
        fetch(&f),
        Err(FetchError::Offline {
            from: SourceKind::MeshyApi
        })
    );

    let f = with_key();
    f.http
        .fail(Method::Get, meshy::BALANCE_URL, HttpError::Timeout);
    assert!(matches!(fetch(&f), Err(FetchError::Transient { .. })));
}

#[test]
fn the_request_timeout_is_capped_by_the_deadline() {
    let f = with_key();
    f.http
        .respond(Method::Get, meshy::BALANCE_URL, fixture("balance-200.json"));
    MeshyProvider::new()
        .fetch(&f.context(SignedDuration::from_secs(3)), &account())
        .expect("fetch");
    assert_eq!(f.http.requests()[0].timeout, Duration::from_secs(3));

    // Past the deadline: no request at all.
    let f = with_key();
    let err = MeshyProvider::new()
        .fetch(&f.context(SignedDuration::ZERO), &account())
        .expect_err("deadline");
    assert!(matches!(err, FetchError::Transient { .. }));
    assert!(f.http.requests().is_empty());
}

/// A secret store that always fails.
struct BrokenSecrets;

impl Secrets for BrokenSecrets {
    fn get(&self, _: &CredentialName) -> Result<Option<SecretString>, SecretsError> {
        Err(SecretsError::Store("access denied".to_owned()))
    }
    fn set(&self, _: &CredentialName, _: &SecretString) -> Result<(), SecretsError> {
        Err(SecretsError::Store("access denied".to_owned()))
    }
    fn delete(&self, _: &CredentialName) -> Result<(), SecretsError> {
        Err(SecretsError::Store("access denied".to_owned()))
    }
}

#[test]
fn a_failing_credential_store_is_transient_not_a_missing_key() {
    let clock = FakeClock::new(now());
    let http = FakeHttp::new();
    let (fs, spawner, env) = (FakeFs::new(), FakeSpawner::new(), FakeEnv::new("C:/home"));
    let ctx = FetchContext {
        clock: &clock,
        http: &http,
        fs: &fs,
        secrets: &BrokenSecrets,
        spawner: &spawner,
        env: &env,
        deadline: now() + SignedDuration::from_secs(30),
        manual: false,
    };
    let err = MeshyProvider::new()
        .fetch(&ctx, &account())
        .expect_err("store failed");
    assert!(matches!(err, FetchError::Transient { .. }));
    assert!(http.requests().is_empty());
}

#[test]
fn another_services_account_is_not_read() {
    let f = with_key();
    let codex = AccountSource::Codex {
        account: AccountKey::primary(),
        codex_home: "C:/Users/tester/.codex".into(),
    };
    let err = MeshyProvider::new()
        .fetch(&f.context(SignedDuration::from_secs(30)), &codex)
        .expect_err("wrong account");
    assert!(matches!(err, FetchError::Unavailable { .. }));
    assert!(f.http.requests().is_empty());
}

#[test]
fn provider_facts() {
    let p = MeshyProvider::new();
    assert_eq!(p.id(), ProviderId::Meshy);
    assert_eq!(p.min_interval(), Duration::from_secs(300));
    assert_eq!(p.sign_in_command(&account()), None, "Meshy has an API key");
    let request = meshy::request(&SecretString::new(KEY.to_owned()), Duration::from_secs(4));
    assert_eq!(request.timeout, Duration::from_secs(4));
    assert!(
        request
            .headers
            .iter()
            .any(|(k, v)| k == "Accept" && v.as_str() == "application/json")
    );
    let snap = meshy::to_snapshot(5.0, lastdrop_core::model::AccountRef::primary(), now());
    assert_eq!(snap.balances, vec![meshy::balance(5.0)]);
}
