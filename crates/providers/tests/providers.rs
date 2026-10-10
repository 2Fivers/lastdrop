//! Provider skeleton tests: one per source, `#[ignore = "M1 implementation"]` until the provider
//! is implemented. Inputs are invented (tests/fixtures/*/README.md); add fixture files there and
//! more cases per source as the parsers grow.

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountKey, MeterId, ProviderId, SourceKind, WindowLen};
use lastdrop_providers::fakes::Fakes;
use lastdrop_providers::io::{CredentialName, HttpResponse, Method};
use lastdrop_providers::{AccountSource, FetchError, Provider, claude, codex, meshy};

fn now() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("timestamp")
}

fn meshy_account() -> AccountSource {
    AccountSource::Meshy {
        account: AccountKey::primary(),
        credential: CredentialName {
            provider: ProviderId::Meshy,
            account: AccountKey::primary(),
            name: meshy::API_KEY_NAME.to_owned(),
        },
    }
}

#[test]
#[ignore = "M1 implementation"]
fn meshy_reads_the_balance_with_the_stored_key() {
    let mut f = Fakes::new(now());
    let AccountSource::Meshy { credential, .. } = meshy_account() else {
        unreachable!()
    };
    f.secrets = f.secrets.with(&credential, "msy-test-key-not-real");
    f.http.respond(
        Method::Get,
        meshy::BALANCE_URL,
        HttpResponse::new(200, r#"{"balance": 1240}"#),
    );
    let snap = meshy::MeshyProvider::new()
        .fetch(&f.context(SignedDuration::from_secs(30)), &meshy_account())
        .expect("fetch");
    assert_eq!(snap.source, SourceKind::MeshyApi);
    assert_eq!(snap.balances.len(), 1);
    assert_eq!(snap.balances[0].amount, 1240.0);
    assert_eq!(f.http.pending(), 0);
}

#[test]
#[ignore = "M1 implementation"]
fn meshy_without_a_key_is_a_sign_in_error_and_makes_no_request() {
    let f = Fakes::new(now());
    let err = meshy::MeshyProvider::new()
        .fetch(&f.context(SignedDuration::from_secs(30)), &meshy_account())
        .expect_err("no key");
    assert!(matches!(err, FetchError::Auth { .. }));
    assert!(f.http.requests().is_empty());
}

#[test]
#[ignore = "M1 implementation"]
fn claude_oauth_usage_body_gives_the_r2_meters() {
    // Invented body in the endpoint's shape: utilization in percent, resets_at RFC 3339 or null.
    let body = br#"{
        "five_hour": { "utilization": 42.0, "resets_at": "2026-10-10T12:56:00Z" },
        "seven_day": { "utilization": 61.0, "resets_at": "2026-10-12T07:00:00Z" }
    }"#;
    let usage = claude::oauth::parse(body).expect("parse");
    assert_eq!(usage.windows.len(), 2);
    let five = claude::meter(&usage.windows[0], 0);
    assert_eq!(five.id, MeterId::new(MeterId::FIVE_HOUR));
    assert_eq!(five.window, WindowLen::Fixed(SignedDuration::from_hours(5)));
    assert!(
        five.starts_on_use,
        "R§2: Claude's 5-hour session starts on use"
    );
    assert!((five.used - 0.42).abs() < 1e-12);
}

#[test]
#[ignore = "M1 implementation"]
fn codex_app_server_answers_rate_limits() {
    // Invented line. The field names are placeholders: confirm them against the app-server
    // protocol (`codex app-server generate-json-schema` or its docs) and fix this fixture first.
    let line = r#"{"jsonrpc":"2.0","id":2,"result":{"rateLimits":{"primary":{"usedPercent":12.0,"windowDurationMins":300,"resetsAt":1760104560},"secondary":{"usedPercent":5.0,"windowDurationMins":10080,"resetsAt":1760624760}}}}"#;
    let usage = codex::app_server::parse_rate_limits(line, 2)
        .expect("parse")
        .expect("response");
    assert_eq!(
        usage.primary.as_ref().and_then(|w| w.window_minutes),
        Some(300)
    );
    assert_eq!(
        usage.secondary.as_ref().and_then(|w| w.window_minutes),
        Some(10080)
    );
    assert_eq!(
        codex::app_server::parse_rate_limits(r#"{"jsonrpc":"2.0","method":"note"}"#, 2)
            .expect("skip"),
        None
    );
}
