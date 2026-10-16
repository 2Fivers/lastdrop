//! Described services (custom-services-spec §5) against the fakes, on invented definitions and
//! answers. A test binary of its own: the registry of described services is per process. No test
//! touches the network or Credential Manager.

use std::sync::Once;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::custom;
use lastdrop_core::model::{AccountKey, ProviderId, SourceKind, Unit, WindowLen};
use lastdrop_core::settings::{AccountSettings, Settings};
use lastdrop_providers::fakes::{FakeEnv, Fakes};
use lastdrop_providers::io::{CredentialName, HeaderValue, HttpRequest, HttpResponse, Method};
use lastdrop_providers::{AccountSource, FetchError, Provider, custom::CustomProvider, providers};

/// A balance in a string, its currency in the answer, the default `Authorization: Bearer` key.
const SHOP: &str = r#"{
    "format": 1, "id": "shop", "name": "Shop", "url": "https://api.shop.invalid/v1/balance",
    "usage_page": "https://shop.invalid/billing",
    "balances": [ { "id": "balance", "label": "Bal", "short_name": "balance", "name": "Balance",
        "amount": "/balance_infos/0/total_balance", "currency_at": "/balance_infos/0/currency" } ]
}"#;

/// Credits less usage in cents, a percentage meter and a used-of-limit meter in a header of its
/// own without a prefix.
const STUDIO: &str = r#"{
    "format": 1, "id": "studio", "name": "Studio", "monogram": "St",
    "url": "https://api.studio.invalid/v1/me", "key_header": "x-api-key",
    "meters": [
        { "id": "chars", "label": "Chr", "short_name": "characters", "name": "Characters",
          "used": "/chars/used", "limit": "/chars/limit", "resets_at": "/chars/reset_unix",
          "window": "month" },
        { "id": "daily", "label": "Day", "short_name": "daily", "name": "Daily",
          "used_percent": "/daily/percent", "resets_at": "/daily/resets", "window": "day" }
    ],
    "balances": [ { "id": "credits", "label": "Cr", "short_name": "credits", "name": "Credits",
        "amount": "/credits/total", "minus": "/credits/spent", "scale": 0.01,
        "currency": "EUR" } ]
}"#;

const KEY: &str = "test-described-key-not-real";

fn setup() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        custom::install(vec![
            custom::parse(SHOP).expect("shop"),
            custom::parse(STUDIO).expect("studio"),
        ]);
    });
}

fn now() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("timestamp")
}

fn budget() -> SignedDuration {
    SignedDuration::from_secs(20)
}

fn id(name: &str) -> ProviderId {
    setup();
    ProviderId::from_id(name).expect("loaded")
}

fn provider(name: &str) -> CustomProvider {
    match id(name) {
        ProviderId::Custom(c) => CustomProvider::new(c),
        other => panic!("{other:?} is not described"),
    }
}

fn credential(name: &str) -> CredentialName {
    CredentialName {
        provider: id(name),
        account: AccountKey::primary(),
        name: "api_key".to_owned(),
    }
}

fn account(name: &str) -> AccountSource {
    AccountSource::Custom {
        account: AccountKey::primary(),
        provider: id(name),
        credential: credential(name),
    }
}

fn with_key(name: &str) -> Fakes {
    let mut f = Fakes::new(now());
    f.secrets = f.secrets.with(&credential(name), KEY);
    f
}

fn header<'a>(request: &'a HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| match v {
            HeaderValue::Plain(p) => p.as_str(),
            HeaderValue::Secret(s) => s.expose(),
        })
}

#[test]
fn reads_a_balance_with_the_stored_key() {
    let f = with_key("shop");
    f.http.respond(
        Method::Get,
        "https://api.shop.invalid/v1/balance",
        HttpResponse::new(
            200,
            r#"{ "is_available": true, "balance_infos": [
                { "currency": "cny", "total_balance": "110.00", "granted_balance": "10.00" } ] }"#,
        ),
    );
    let snap = provider("shop")
        .fetch(&f.context(budget()), &account("shop"))
        .expect("snapshot");
    let sent = f.http.requests();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].method, Method::Get);
    assert_eq!(
        header(&sent[0], "Authorization"),
        Some("Bearer test-described-key-not-real")
    );
    assert!(matches!(
        sent[0].headers.iter().find(|(n, _)| n == "Authorization"),
        Some((_, HeaderValue::Secret(_)))
    ));
    assert_eq!(header(&sent[0], "Accept"), Some("application/json"));
    assert_eq!(snap.provider, id("shop"));
    assert_eq!(snap.source, SourceKind::CustomApi);
    assert!(snap.meters.is_empty());
    assert_eq!(snap.balances.len(), 1);
    assert!((snap.balances[0].amount - 110.0).abs() < 1e-9);
    assert_eq!(
        snap.balances[0].unit,
        Unit::Currency {
            code: "CNY".to_owned()
        }
    );
    assert_eq!(snap.measured_at, now());
}

#[test]
fn reads_meters_and_a_balance_by_the_file() {
    let f = with_key("studio");
    f.http.respond(
        Method::Get,
        "https://api.studio.invalid/v1/me",
        HttpResponse::new(
            200,
            r#"{ "chars": { "used": 25000, "limit": 100000, "reset_unix": 1792800000 },
                 "daily": { "percent": 100, "resets": "2026-10-11T00:00:00Z" },
                 "credits": { "total": 5000, "spent": "1250" } }"#,
        ),
    );
    let snap = provider("studio")
        .fetch(&f.context(budget()), &account("studio"))
        .expect("snapshot");
    let sent = f.http.requests();
    assert_eq!(
        header(&sent[0], "x-api-key"),
        Some("test-described-key-not-real"),
        "a header of its own has no prefix"
    );
    assert_eq!(header(&sent[0], "Authorization"), None);

    let ids: Vec<&str> = snap.meters.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, ["chars", "daily"]);
    assert!((snap.meters[0].used - 0.25).abs() < 1e-9);
    assert!(!snap.meters[0].limited);
    assert_eq!(
        snap.meters[0].resets_at,
        Some(Timestamp::from_second(1_792_800_000).expect("instant"))
    );
    assert_eq!(snap.meters[0].window, WindowLen::CalendarMonth);
    assert!(snap.meters[1].limited, "100 % is used up");
    assert_eq!(
        snap.meters[1].resets_at,
        Some("2026-10-11T00:00:00Z".parse().expect("instant"))
    );
    assert_eq!(snap.meters[0].names.short_label, "Chr");
    assert_eq!(snap.balances[0].id.0, "credits");
    assert!(
        (snap.balances[0].amount - 37.5).abs() < 1e-9,
        "(5000 − 1250) × 0.01"
    );
    assert_eq!(
        snap.balances[0].unit,
        Unit::Currency {
            code: "EUR".to_owned()
        }
    );
    assert_eq!(snap.balances[0].order, 2, "after the file's meters");
}

#[test]
fn leaves_out_what_the_answer_lacks_and_never_reads_zero() {
    let f = with_key("studio");
    f.http.respond(
        Method::Get,
        "https://api.studio.invalid/v1/me",
        HttpResponse::new(
            200,
            r#"{ "chars": { "used": 10, "limit": 0 }, "daily": { "percent": "n/a" },
                 "credits": { "total": 20 } }"#,
        ),
    );
    let result = provider("studio").fetch(&f.context(budget()), &account("studio"));
    assert!(
        matches!(result, Err(FetchError::Parse { .. })),
        "a limit of 0, a word and a missing minus give nothing: {result:?}"
    );

    let f = with_key("shop");
    f.http.respond(
        Method::Get,
        "https://api.shop.invalid/v1/balance",
        HttpResponse::new(
            200,
            r#"{ "balance_infos": [ { "currency": "dollars", "total_balance": 5 } ] }"#,
        ),
    );
    let result = provider("shop").fetch(&f.context(budget()), &account("shop"));
    assert!(
        matches!(result, Err(FetchError::Parse { .. })),
        "not a currency code: {result:?}"
    );
}

#[test]
fn maps_errors_and_asks_for_a_key() {
    // No key stored: the API-key message, and nothing is sent.
    let f = Fakes::new(now());
    let result = provider("shop").fetch(&f.context(budget()), &account("shop"));
    assert!(matches!(result, Err(FetchError::Auth { .. })), "{result:?}");
    assert!(f.http.requests().is_empty());

    let cases: [(u16, &str); 5] = [
        (401, "auth"),
        (403, "rate"),
        (429, "rate"),
        (503, "transient"),
        (404, "parse"),
    ];
    for (status, expected) in cases {
        let f = with_key("shop");
        let mut response = HttpResponse::new(status, "{}");
        response
            .headers
            .push(("Retry-After".to_owned(), "120".to_owned()));
        f.http
            .respond(Method::Get, "https://api.shop.invalid/v1/balance", response);
        let result = provider("shop").fetch(&f.context(budget()), &account("shop"));
        let got = match &result {
            Err(FetchError::Auth { .. }) => "auth",
            Err(FetchError::RateLimited { retry_after, .. }) => {
                assert_eq!(*retry_after, Some(Duration::from_secs(120)));
                "rate"
            }
            Err(FetchError::Transient { .. }) => "transient",
            Err(FetchError::Parse { .. }) => "parse",
            other => panic!("{status}: {other:?}"),
        };
        assert_eq!(got, expected, "HTTP {status}");
    }

    let f = with_key("shop");
    f.http.respond(
        Method::Get,
        "https://api.shop.invalid/v1/balance",
        HttpResponse::new(200, "<html>sign in</html>"),
    );
    assert!(matches!(
        provider("shop").fetch(&f.context(budget()), &account("shop")),
        Err(FetchError::Parse { .. })
    ));
}

#[test]
fn described_services_are_providers_like_the_others() {
    let shop = id("shop");
    let studio = id("studio");
    let settings = Settings::default();
    let all = providers(&settings);
    let ids: Vec<ProviderId> = all.iter().map(|p| p.id()).collect();
    assert_eq!(
        &ids[ids.len() - 2..],
        [shop, studio],
        "after the built-in ones"
    );
    let p = provider("shop");
    assert_eq!(p.min_interval(), Duration::from_secs(5 * 60));
    assert_eq!(p.usage_page(), Some("https://shop.invalid/billing"));
    assert_eq!(provider("studio").usage_page(), None);

    // Its account: the API key stored here, under the usual name.
    let env = FakeEnv::new(r"C:\Users\tester");
    let source = AccountSource::resolve(shop, &AccountSettings::primary(), &env).expect("account");
    assert_eq!(source, account("shop"));
    assert_eq!(source.provider(), shop);
    assert_eq!(
        credential("shop").target(),
        "Lastdrop:shop:default:api_key",
        "named by the file id"
    );
}
