//! Cursor, Antigravity, OpenCode Go and Grok against the fakes, on invented data
//! (`tests/fixtures/cursor/README.md`; the JSON bodies below are written by hand from the shapes
//! the reference tools and the vendors' own open-source code document, with made-up values).
//! No test touches the network, Credential Manager, a real file or a real process.

use std::path::PathBuf;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountKey, MeterId, PoolWindow, ProviderId, Unit, WindowLen};
use lastdrop_core::secret::SecretString;
use lastdrop_providers::fakes::{FakeLine, Fakes};
use lastdrop_providers::io::{CredentialName, HeaderValue, HttpResponse, Method};
use lastdrop_providers::{
    AccountSource, FetchError, Provider, antigravity, cursor, grok, opencode_go,
};

fn now() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("timestamp")
}

fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

fn home() -> PathBuf {
    PathBuf::from(r"C:\Users\tester")
}

fn budget() -> SignedDuration {
    SignedDuration::from_secs(20)
}

fn header<'a>(request: &'a lastdrop_providers::io::HttpRequest, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| match v {
            HeaderValue::Plain(p) => p.as_str(),
            HeaderValue::Secret(s) => s.expose(),
        })
}

// ---- Cursor ---------------------------------------------------------------------------------------

/// The invented session in `state-signed-in.vscdb` (`sub` `github|user_test0000000000000000`,
/// expires 2100-01-01).
const CURSOR_TOKEN: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiAiZ2l0aHVifHVzZXJfdGVzdDAwMDAwMDAwMDAwMDAwMDAiLCAiZXhwIjogNDEwMjQ0NDgwMCwgImlzcyI6ICJodHRwczovL2F1dGhlbnRpY2F0aW9uLmN1cnNvci5zaCIsICJ0eXBlIjogInNlc3Npb24ifQ.test-signature-not-real";

fn cursor_db() -> PathBuf {
    home().join(r"AppData\Roaming\Cursor\User\globalStorage\state.vscdb")
}

fn cursor_account() -> AccountSource {
    AccountSource::Cursor {
        account: AccountKey::primary(),
        state_db: cursor_db(),
    }
}

fn cursor_fakes(db: &[u8]) -> Fakes {
    let f = Fakes::new(now());
    f.fs.add_file(cursor_db(), db.to_vec(), now());
    f
}

const SIGNED_IN: &[u8] = include_bytes!("fixtures/cursor/state-signed-in.vscdb");
const SIGNED_OUT: &[u8] = include_bytes!("fixtures/cursor/state-signed-out.vscdb");
const WAL_DB: &[u8] = include_bytes!("fixtures/cursor/state-wal.vscdb");
const WAL_LOG: &[u8] = include_bytes!("fixtures/cursor/state-wal.vscdb-wal");
const CYCLE: &[u8] = include_bytes!("fixtures/cursor/state-cycle.vscdb");

#[test]
fn cursor_reads_a_session_still_in_the_write_ahead_log() {
    // Without its log the file has no session yet: signed out.
    let f = cursor_fakes(WAL_DB);
    let ctx = f.context(budget());
    assert!(matches!(
        cursor::read_token(&ctx, &cursor_db()),
        Err(FetchError::Auth { .. })
    ));
    // With it, the committed frames win over the file's pages.
    let mut log = cursor_db().into_os_string();
    log.push("-wal");
    f.fs.add_file(PathBuf::from(log.clone()), WAL_LOG.to_vec(), now());
    assert_eq!(
        cursor::read_token(&ctx, &cursor_db())
            .expect("token")
            .expose(),
        CURSOR_TOKEN
    );
    // A log whose last frame is damaged keeps only what was committed before it.
    let mut torn = WAL_LOG.to_vec();
    let last = torn.len() - 10;
    torn[last] ^= 0xff;
    f.fs.add_file(PathBuf::from(log), torn, now());
    assert!(cursor::read_token(&ctx, &cursor_db()).is_err());
}

#[test]
fn cursor_stops_at_a_page_that_is_reached_twice() {
    let f = cursor_fakes(CYCLE);
    let ctx = f.context(budget());
    assert!(matches!(
        cursor::read_token(&ctx, &cursor_db()),
        Err(FetchError::Transient { .. })
    ));
}

#[test]
fn cursor_always_has_a_credential_fingerprint() {
    let provider = cursor::CursorProvider::new();
    let signed_in = cursor_fakes(SIGNED_IN);
    let ctx = signed_in.context(budget());
    let current = provider
        .credential_fingerprint(&ctx, &cursor_account())
        .expect("fingerprint");
    assert!(current.0.starts_with("token:") && current.0 != "token:none");
    // Signed out: a fingerprint of its own, so signing in lifts an auth stop.
    let signed_out = cursor_fakes(SIGNED_OUT);
    let ctx = signed_out.context(budget());
    assert_eq!(
        provider
            .credential_fingerprint(&ctx, &cursor_account())
            .map(|f| f.0),
        Some("token:none".to_owned())
    );
    // Unreadable for a moment: the last one read, never none.
    let provider = cursor::CursorProvider::new();
    let ctx = signed_in.context(budget());
    let known = provider.credential_fingerprint(&ctx, &cursor_account());
    let torn = cursor_fakes(b"SQLite format 3\0 but cut short");
    let ctx = torn.context(budget());
    assert_eq!(
        provider.credential_fingerprint(&ctx, &cursor_account()),
        known
    );
}

#[test]
fn cursor_reads_its_session_from_the_database_read_only() {
    let f = cursor_fakes(SIGNED_IN);
    let ctx = f.context(budget());
    let token = cursor::read_token(&ctx, &cursor_db()).expect("token");
    assert_eq!(token.expose(), CURSOR_TOKEN);

    let f = cursor_fakes(SIGNED_OUT);
    let ctx = f.context(budget());
    assert!(matches!(
        cursor::read_token(&ctx, &cursor_db()),
        Err(FetchError::Auth { .. })
    ));

    // Not a database, or cut short: a passing failure, not a crash.
    let mut cut = SIGNED_IN.to_vec();
    cut.truncate(5000);
    for bytes in [b"not a database at all, just text".to_vec(), cut] {
        let f = cursor_fakes(&bytes);
        let ctx = f.context(budget());
        assert!(matches!(
            cursor::read_token(&ctx, &cursor_db()),
            Err(FetchError::Transient { .. })
        ));
    }
    let f = Fakes::new(now());
    let ctx = f.context(budget());
    assert!(matches!(
        cursor::read_token(&ctx, &cursor_db()),
        Err(FetchError::Unavailable { .. })
    ));
}

#[test]
fn cursor_builds_the_cookie_from_the_token_and_refuses_an_expired_one() {
    let cookie =
        cursor::session(&SecretString::new(CURSOR_TOKEN.to_owned()), now()).expect("session");
    assert_eq!(
        cookie.expose(),
        format!("user_test0000000000000000%3A%3A{CURSOR_TOKEN}")
    );
    // exp 2026-10-10T10:00:00Z: before now.
    let expired = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJnaXRodWJ8dXNlcl94IiwiZXhwIjoxNzkxNjI2NDAwfQ.sig";
    assert!(matches!(
        cursor::session(&SecretString::new(expired.to_owned()), now()),
        Err(FetchError::Auth { .. })
    ));
    assert!(matches!(
        cursor::session(&SecretString::new("not-a-jwt".to_owned()), now()),
        Err(FetchError::Auth { .. })
    ));
}

const CURSOR_POOLS: &str = r#"{
    "billingCycleStart": "2026-10-01T00:00:00.000Z",
    "billingCycleEnd": "2026-11-01T00:00:00.000Z",
    "membershipType": "pro",
    "limitType": "user",
    "isUnlimited": false,
    "individualUsage": {
        "plan": { "enabled": true, "used": 1200, "limit": 2000, "remaining": 800,
                  "autoPercentUsed": 35.5, "apiPercentUsed": 72, "totalPercentUsed": 60 },
        "onDemand": { "enabled": true, "used": 250, "limit": 1000, "remaining": 750 }
    },
    "teamUsage": {}
}"#;

#[test]
fn cursor_reads_its_pools_and_on_demand_spend() {
    let f = cursor_fakes(SIGNED_IN);
    f.http.respond(
        Method::Get,
        cursor::USAGE_URL,
        HttpResponse::new(200, CURSOR_POOLS),
    );
    let ctx = f.context(budget());
    let snap = cursor::CursorProvider::new()
        .fetch(&ctx, &cursor_account())
        .expect("snapshot");
    assert_eq!(snap.account.plan.as_deref(), Some("Pro"));
    let ids: Vec<&str> = snap.meters.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, [MeterId::AUTO, MeterId::API, MeterId::ON_DEMAND]);
    assert!((snap.meters[0].used - 0.355).abs() < 1e-9);
    assert!((snap.meters[1].used - 0.72).abs() < 1e-9);
    assert!((snap.meters[2].used - 0.25).abs() < 1e-9);
    assert_eq!(snap.meters[0].resets_at, Some(ts("2026-11-01T00:00:00Z")));
    assert_eq!(
        snap.meters[0].window,
        WindowLen::Fixed(SignedDuration::from_hours(31 * 24))
    );
    let sent = f.http.requests();
    let cookie = header(&sent[0], "Cookie").expect("cookie");
    assert!(cookie.starts_with("WorkosCursorSessionToken=user_test0000000000000000%3A%3A"));
}

#[test]
fn cursor_falls_back_to_one_total_and_maps_errors() {
    let total = r#"{ "billingCycleEnd": "2026-11-01T00:00:00Z",
        "individualUsage": { "plan": { "used": 500, "limit": 2000 },
                             "onDemand": { "enabled": false, "used": 0, "limit": 0 } } }"#;
    let usage = cursor::parse_summary(total.as_bytes()).expect("usage");
    assert_eq!(usage.pools.len(), 1);
    assert_eq!(usage.pools[0].id, MeterId::INCLUDED);
    assert!((usage.pools[0].used - 0.25).abs() < 1e-9);
    let enterprise =
        r#"{ "individualUsage": { "overall": { "enabled": true, "used": 900, "limit": 1000 } } }"#;
    assert_eq!(
        cursor::parse_summary(enterprise.as_bytes())
            .expect("usage")
            .pools[0]
            .id,
        MeterId::INCLUDED
    );
    assert!(matches!(
        cursor::parse_summary(b"{}"),
        Err(FetchError::Parse { .. })
    ));
    for (status, auth) in [(401, true), (403, false), (429, false)] {
        let r = cursor::interpret(&HttpResponse::new(status, "{}"), now());
        assert_eq!(matches!(r, Err(FetchError::Auth { .. })), auth, "{status}");
    }
}

// ---- Grok -----------------------------------------------------------------------------------------

fn grok_file() -> PathBuf {
    home().join(r".grok\auth.json")
}

fn grok_account() -> AccountSource {
    AccountSource::Grok {
        account: AccountKey::primary(),
        auth_file: grok_file(),
    }
}

/// An invented `auth.json`: an API key (skipped) and two OIDC sign-ins, the later one current.
const GROK_AUTH: &str = r#"{
    "xai::api_key": { "key": "xai-test-api-key-not-real", "auth_mode": "api_key",
                      "create_time": "2026-09-01T00:00:00Z", "user_id": "" },
    "https://auth.x.ai::client-a": { "key": "test-old-token-not-real", "auth_mode": "oidc",
        "create_time": "2026-09-01T00:00:00Z", "user_id": "user-0001",
        "refresh_token": "test-refresh-not-real", "expires_at": "2026-10-01T00:00:00Z" },
    "https://auth.x.ai::client-b": { "key": "test-token-not-real", "auth_mode": "oidc",
        "create_time": "2026-10-09T00:00:00Z", "user_id": "user-0001",
        "email": "tester@example.invalid", "refresh_token": "test-refresh-not-real",
        "expires_at": "2026-10-16T00:00:00Z" }
}"#;

const GROK_WEEKLY: &str = r#"{ "config": {
    "creditUsagePercent": 41.5,
    "currentPeriod": { "type": "USAGE_PERIOD_TYPE_WEEKLY",
                       "start": "2026-10-06T00:00:00Z", "end": "2026-10-13T00:00:00Z" },
    "onDemandCap": { "val": "2000" }, "onDemandUsed": { "val": 500 },
    "prepaidBalance": { "val": "1250" },
    "isUnifiedBillingUser": true,
    "history": [ { "billingCycle": "invented", "totalUsed": { "val": 10 } } ]
} }"#;

#[test]
fn grok_reads_the_latest_sign_in_and_the_pool() {
    let f = Fakes::new(now());
    f.fs.add_file(grok_file(), GROK_AUTH, now());
    f.http.respond(
        Method::Get,
        grok::BILLING_URL,
        HttpResponse::new(200, GROK_WEEKLY),
    );
    let ctx = f.context(budget());
    let snap = grok::GrokProvider::new()
        .fetch(&ctx, &grok_account())
        .expect("snapshot");
    let sent = f.http.requests();
    assert_eq!(
        header(&sent[0], "Authorization"),
        Some("Bearer test-token-not-real")
    );
    assert_eq!(header(&sent[0], "X-XAI-Token-Auth"), Some(grok::TOKEN_AUTH));
    assert_eq!(header(&sent[0], "x-userid"), Some("user-0001"));

    let pool = &snap.meters[0];
    assert_eq!(pool.id.0, MeterId::WEEKLY);
    assert!((pool.used - 0.415).abs() < 1e-9);
    assert_eq!(pool.resets_at, Some(ts("2026-10-13T00:00:00Z")));
    assert_eq!(
        pool.window,
        WindowLen::Fixed(SignedDuration::from_hours(7 * 24))
    );
    let extra = &snap.meters[1];
    assert_eq!(extra.id.0, MeterId::ON_DEMAND);
    assert!((extra.used - 0.25).abs() < 1e-9);
    assert_eq!(snap.balances.len(), 1);
    assert!((snap.balances[0].amount - 12.5).abs() < 1e-9);
    assert_eq!(
        snap.balances[0].unit,
        Unit::Currency {
            code: "USD".to_owned()
        }
    );
}

#[test]
fn grok_reads_zero_from_an_empty_period_and_stops_on_a_missing_sign_in() {
    // protobuf-JSON leaves zero out: a current monthly period without a percent is unused.
    let monthly = r#"{ "currentPeriod": { "type": "USAGE_PERIOD_TYPE_MONTHLY",
        "end": "2026-11-01T00:00:00Z" }, "prepaidBalance": {} }"#;
    let usage = grok::parse_billing(monthly.as_bytes()).expect("usage");
    assert_eq!(usage.pool, Some((grok::Period::Monthly, 0.0)));
    assert_eq!(usage.prepaid, None, "no prepaid credit");
    assert!(matches!(
        grok::parse_billing(b"{}"),
        Err(FetchError::Parse { .. })
    ));

    // No file, only an API key, or an expired sign-in: sign in again with the Grok CLI.
    let f = Fakes::new(now());
    let ctx = f.context(budget());
    assert!(matches!(
        grok::read_sign_in(&ctx, &grok_file()),
        Err(FetchError::Auth { .. })
    ));
    for auth in [
        r#"{ "xai::api_key": { "key": "xai-test-api-key-not-real", "auth_mode": "api_key" } }"#,
        r#"{ "https://auth.x.ai::c": { "key": "t", "auth_mode": "oidc", "expires_at": "2026-10-10T10:42:30Z" } }"#,
    ] {
        f.fs.add_file(grok_file(), auth, now());
        assert!(matches!(
            grok::read_sign_in(&ctx, &grok_file()),
            Err(FetchError::Auth { .. })
        ));
    }
    f.fs.add_file(grok_file(), "not json", now());
    assert!(matches!(
        grok::read_sign_in(&ctx, &grok_file()),
        Err(FetchError::Parse { .. })
    ));

    // An expired sign-in never wins over one without an expiry; a value that is not a sign-in
    // (a version number) is skipped, not fatal.
    f.fs.add_file(
        grok_file(),
        r#"{ "version": 2,
             "https://auth.x.ai::old": { "key": "test-old-not-real", "auth_mode": "oidc",
                                        "expires_at": "2026-10-01T00:00:00Z" },
             "https://accounts.x.ai/sign-in": { "key": "test-legacy-not-real", "user_id": " " } }"#,
        now(),
    );
    let sign_in = grok::read_sign_in(&ctx, &grok_file()).expect("sign-in");
    assert_eq!(sign_in.token.expose(), "test-legacy-not-real");
    assert!(sign_in.user_id.is_none(), "a blank user id is none");
}

// ---- OpenCode Go ----------------------------------------------------------------------------------

fn opencode_credential() -> CredentialName {
    CredentialName {
        provider: ProviderId::OpenCodeGo,
        account: AccountKey::primary(),
        name: "api_key".to_owned(),
    }
}

fn opencode_account() -> AccountSource {
    AccountSource::OpenCodeGo {
        account: AccountKey::primary(),
        credential: opencode_credential(),
        auth_file: home().join(opencode_go::AUTH_FILE),
    }
}

const OPENCODE_USAGE: &str = r#"{ "usage": {
    "rolling": { "status": "ok", "percent": 12, "resetsAt": "2026-10-10T14:00:00Z" },
    "weekly": { "status": "ok", "percent": 48.5, "resetsAt": "2026-10-12T00:00:00Z" },
    "monthly": { "status": "rate-limited", "percent": 100, "resetsAt": "2026-10-21T08:15:00Z" }
} }"#;

#[test]
fn opencode_go_reads_three_windows_with_the_stored_key() {
    let mut f = Fakes::new(now());
    f.secrets = f
        .secrets
        .with(&opencode_credential(), "test-go-key-not-real");
    f.http.respond(
        Method::Get,
        opencode_go::USAGE_URL,
        HttpResponse::new(200, OPENCODE_USAGE),
    );
    let ctx = f.context(budget());
    let snap = opencode_go::OpenCodeGoProvider::new()
        .fetch(&ctx, &opencode_account())
        .expect("snapshot");
    let sent = f.http.requests();
    assert_eq!(
        header(&sent[0], "Authorization"),
        Some("Bearer test-go-key-not-real")
    );
    let ids: Vec<&str> = snap.meters.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, [MeterId::FIVE_HOUR, MeterId::WEEKLY, MeterId::MONTHLY]);
    assert!(
        snap.meters[0].starts_on_use,
        "the rolling window starts on use"
    );
    assert!((snap.meters[1].used - 0.485).abs() < 1e-9);
    assert!(snap.meters[2].limited, "rate-limited");
    assert_eq!(snap.meters[2].window, WindowLen::CalendarMonth);
}

#[test]
fn opencode_go_takes_opencodes_own_key_and_tells_a_missing_subscription() {
    let f = Fakes::new(now());
    f.fs.add_file(
        home().join(opencode_go::AUTH_FILE),
        r#"{ "anthropic": { "type": "oauth", "refresh": "test-refresh-not-real", "access": "x", "expires": 1 },
             "opencode-go": { "type": "api", "key": "test-file-key-not-real" },
             "odd": "a string entry" }"#,
        now(),
    );
    f.http.respond(
        Method::Get,
        opencode_go::USAGE_URL,
        HttpResponse::new(
            403,
            r#"{"type":"error","error":{"type":"EntitlementError","message":"OpenCode Go subscription required."}}"#,
        ),
    );
    let ctx = f.context(budget());
    let result = opencode_go::OpenCodeGoProvider::new().fetch(&ctx, &opencode_account());
    assert!(
        matches!(result, Err(FetchError::Unavailable { .. })),
        "{result:?}"
    );
    assert_eq!(
        header(&f.http.requests()[0], "Authorization"),
        Some("Bearer test-file-key-not-real")
    );

    // No key anywhere: the API-key message.
    let f = Fakes::new(now());
    let ctx = f.context(budget());
    assert!(matches!(
        opencode_go::OpenCodeGoProvider::new().fetch(&ctx, &opencode_account()),
        Err(FetchError::Auth { .. })
    ));
    assert!(matches!(
        opencode_go::parse_usage(br#"{ "usage": { "weekly": { "status": "ok" } } }"#),
        Err(FetchError::Parse { .. })
    ));

    // The fingerprint follows the key in use: the stored one when there is one.
    let provider = opencode_go::OpenCodeGoProvider::new();
    let from_file = provider.credential_fingerprint(&ctx, &opencode_account());
    let mut f = Fakes::new(now());
    f.secrets = f
        .secrets
        .with(&opencode_credential(), "test-go-key-not-real");
    let ctx = f.context(budget());
    let stored = provider
        .credential_fingerprint(&ctx, &opencode_account())
        .expect("fingerprint");
    assert!(stored.0.starts_with("key:"));
    assert_ne!(Some(stored), from_file);
}

// ---- Antigravity ----------------------------------------------------------------------------------

fn agy() -> PathBuf {
    home().join(r"AppData\Local\agy\bin\agy.exe")
}

fn agy_fakes() -> Fakes {
    let mut f = Fakes::new(now());
    f.env = f
        .env
        .clone()
        .with_var("LOCALAPPDATA", home().join(r"AppData\Local"));
    f.fs.add_file(agy(), "MZ", now());
    f
}

const AGY_REPORT: &str = r#"{"status":"SUCCESS","response":"","command":{"name":"usage","data":{"description":"Quota","groups":[
 {"name":"Gemini Models","buckets":[
   {"id":"gemini-5h","name":"5-hour","window":"5h","remaining_fraction":0.75,"reset_time":"2026-10-10T13:00:00Z"},
   {"id":"gemini-weekly","name":"Weekly","window":"weekly","remainingFraction":0.4,"resetTime":"2026-10-14T00:00:00Z"}]},
 {"displayName":"Claude and GPT models","buckets":[
   {"bucketId":"cg-5h","displayName":"5-hour","window":"5h","remaining":{"case":"remainingFraction","value":0.1}},
   {"bucketId":"cg-weekly","window":"weekly","disabled":true,"remainingFraction":0.9}]}
]}}}"#;

#[test]
fn antigravity_runs_agy_in_print_mode_and_reads_its_pools() {
    let f = agy_fakes();
    f.spawner
        .script(vec![FakeLine::Line("agy 1.2.15".to_owned()), FakeLine::Eof]);
    f.spawner.script(
        AGY_REPORT
            .lines()
            .map(|l| FakeLine::Line(l.to_owned()))
            .chain([FakeLine::Eof])
            .collect(),
    );
    let ctx = f.context(budget());
    let provider = antigravity::AntigravityProvider::new();
    let account = AccountSource::Antigravity {
        account: AccountKey::primary(),
    };
    let snap = provider.fetch(&ctx, &account).expect("snapshot");
    let spawned = f.spawner.spawned();
    assert_eq!(spawned[0].program, agy());
    assert_eq!(spawned[0].args, ["--version"]);
    assert_eq!(spawned[1].args, ["-p", "/usage", "--output-format", "json"]);
    assert!(
        spawned[1]
            .env
            .iter()
            .any(|(name, value)| name == antigravity::CREDENTIALS_VAR && value.is_none())
    );

    let ids: Vec<&str> = snap.meters.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(
        ids,
        [
            "gemini_models:five_hour",
            "gemini_models:weekly",
            "claude_and_gpt_models:five_hour"
        ],
        "the disabled bucket is unknown, not 0 %"
    );
    assert!((snap.meters[0].used - 0.25).abs() < 1e-9);
    assert!((snap.meters[2].used - 0.9).abs() < 1e-9);
    assert_eq!(snap.meters[0].names.short_label, "G5h");
    assert_eq!(
        snap.meters[2].names.pool_of(),
        Some(("Claude and GPT models", PoolWindow::FiveHour))
    );
    assert_eq!(snap.meters[1].resets_at, Some(ts("2026-10-14T00:00:00Z")));
}

#[test]
fn antigravity_takes_a_complete_report_without_waiting_for_the_pipe_to_close() {
    let f = agy_fakes();
    f.spawner.script(vec![
        FakeLine::Line("agy 1.2.15".to_owned()),
        FakeLine::Timeout,
    ]);
    f.spawner.script(
        AGY_REPORT
            .lines()
            .map(|l| FakeLine::Line(l.to_owned()))
            .chain([FakeLine::Timeout])
            .collect(),
    );
    let ctx = f.context(budget());
    let account = AccountSource::Antigravity {
        account: AccountKey::primary(),
    };
    let snap = antigravity::AntigravityProvider::new()
        .fetch(&ctx, &account)
        .expect("snapshot");
    assert_eq!(snap.meters.len(), 3);
}

#[test]
fn antigravity_refuses_an_old_agy_and_reads_errors() {
    let f = agy_fakes();
    f.spawner
        .script(vec![FakeLine::Line("1.1.10".to_owned()), FakeLine::Eof]);
    let ctx = f.context(budget());
    let account = AccountSource::Antigravity {
        account: AccountKey::primary(),
    };
    assert!(matches!(
        antigravity::AntigravityProvider::new().fetch(&ctx, &account),
        Err(FetchError::Unavailable { .. })
    ));
    assert_eq!(f.spawner.spawned().len(), 1, "never asked for usage");

    assert_eq!(antigravity::parse_version("agy 1.2.15"), Some((1, 2, 15)));
    assert_eq!(antigravity::parse_version("v1.1.11"), Some((1, 1, 11)));
    assert_eq!(antigravity::parse_version("1.2"), None);
    // Another program's version after agy's never counts.
    assert_eq!(
        antigravity::parse_version("agy/1.0.3 win32-x64 node-v20.11.1"),
        Some((1, 0, 3))
    );
    assert_eq!(antigravity::parse_version("built with node-v20.11.1"), None);
    assert!(matches!(
        antigravity::parse_report(r#"{"status":"ERROR","message":"Please sign in to continue"}"#),
        Err(FetchError::Auth { .. })
    ));
    assert!(matches!(
        antigravity::parse_report("not json"),
        Err(FetchError::Parse { .. })
    ));
    // A log line before the JSON is skipped.
    assert!(antigravity::parse_report(&format!("starting…\n{AGY_REPORT}")).is_ok());
    assert_eq!(
        antigravity::pool_slug("Claude and GPT models"),
        "claude_and_gpt_models"
    );

    // No agy at all.
    let f = Fakes::new(now());
    let ctx = f.context(budget());
    assert!(matches!(
        antigravity::AntigravityProvider::new().fetch(&ctx, &account),
        Err(FetchError::Unavailable { .. })
    ));
}
