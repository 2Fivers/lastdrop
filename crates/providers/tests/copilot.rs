//! The Copilot provider against `FakeHttp` and `FakeSpawner` (`gh auth token`) with the invented
//! fixtures in `tests/fixtures/copilot/`. No test touches the network, the GitHub CLI or
//! Credential Manager.

use std::path::PathBuf;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::grammar::English;
use lastdrop_core::grammar::meters::localized;
use lastdrop_core::model::{
    AccountKey, Health, MeterId, MeterNames, ProviderId, Snapshot, SourceKind, WindowLen, catalog,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::settings::AccountSettings;
use lastdrop_providers::copilot::{self, CopilotProvider};
use lastdrop_providers::fakes::{FakeEnv, FakeLine, Fakes};
use lastdrop_providers::io::{HeaderValue, HttpResponse, Method};
use lastdrop_providers::{AccountSource, FetchError, Provider};

const TOKEN: &str = "gho_test_token_not_real";

fn now() -> Timestamp {
    "2026-10-11T10:42:00Z".parse().expect("timestamp")
}

fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

fn config_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\AppData\Roaming\GitHub CLI")
}

fn gh() -> PathBuf {
    PathBuf::from(r"C:\Tools\gh\gh.exe")
}

fn account() -> AccountSource {
    AccountSource::Copilot {
        account: AccountKey::primary(),
        config_dir: config_dir(),
    }
}

/// Fakes with `gh.exe` on `PATH`.
fn with_gh() -> Fakes {
    let mut f = Fakes::new(now());
    f.env = FakeEnv::new(r"C:\Users\tester").with_var("PATH", r"C:\Windows;C:\Tools\gh");
    f.fs.add_file(gh(), "MZ", now());
    f
}

/// Fakes with `gh.exe` on `PATH` that prints the invented token.
fn signed_in() -> Fakes {
    let f = with_gh();
    f.spawner.script(vec![FakeLine::Line(TOKEN.to_owned())]);
    f
}

fn fixture(name: &str) -> HttpResponse {
    let body = match name {
        "user-200-free.json" => include_str!("fixtures/copilot/user-200-free.json"),
        "user-200-paid.json" => include_str!("fixtures/copilot/user-200-paid.json"),
        "user-200-overage.json" => include_str!("fixtures/copilot/user-200-overage.json"),
        "user-200-used-up.json" => include_str!("fixtures/copilot/user-200-used-up.json"),
        "user-200-no-snapshots.json" => {
            include_str!("fixtures/copilot/user-200-no-snapshots.json")
        }
        "user-401.json" => include_str!("fixtures/copilot/user-401.json"),
        "user-404.json" => include_str!("fixtures/copilot/user-404.json"),
        "user-429.json" => include_str!("fixtures/copilot/user-429.json"),
        "user-503.txt" => include_str!("fixtures/copilot/user-503.txt"),
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

fn fetch(f: &Fakes) -> Result<Snapshot, FetchError> {
    CopilotProvider::new().fetch(&f.context(SignedDuration::from_secs(30)), &account())
}

fn fetch_answer(response: HttpResponse) -> Result<Snapshot, FetchError> {
    let f = signed_in();
    f.http.respond(Method::Get, copilot::USER_URL, response);
    let result = fetch(&f);
    assert_eq!(f.http.requests().len(), 1);
    result
}

/// `(id, used, limited)` of every meter, in order.
fn meters(snap: &Snapshot) -> Vec<(String, f64, bool)> {
    snap.meters
        .iter()
        .map(|m| (m.id.0.clone(), m.used, m.limited))
        .collect()
}

#[test]
fn reads_copilot_free_with_the_gh_token() {
    let f = signed_in();
    f.http.respond(
        Method::Get,
        copilot::USER_URL,
        fixture("user-200-free.json"),
    );
    let snap = fetch(&f).expect("fetch");

    assert_eq!(snap.provider, ProviderId::Copilot);
    assert_eq!(snap.source, SourceKind::CopilotApi);
    assert_eq!(snap.account.key, AccountKey::primary());
    assert_eq!(snap.account.plan.as_deref(), Some("Free"));
    assert_eq!((snap.measured_at, snap.fetched_at), (now(), now()));
    assert_eq!(snap.health, Health::Ok);
    assert!(snap.balances.is_empty());
    // Premium requests have no entitlement on Copilot Free; the unknown quota is ignored.
    assert_eq!(
        meters(&snap),
        vec![
            ("chat".to_owned(), 0.1, false),
            ("completions".to_owned(), 0.25, false),
        ]
    );
    let chat = &snap.meters[0];
    assert_eq!(
        (chat.names.clone(), chat.order),
        (MeterNames::new("Chat", "chat", "Chat messages"), 1)
    );
    assert_eq!(chat.resets_at, Some(ts("2026-11-01T00:00:00Z")));
    assert_eq!(chat.window, WindowLen::CalendarMonth);
    assert!(!chat.starts_on_use);
    assert_eq!(
        chat.window.length_ending_at(ts("2026-11-01T00:00:00Z")),
        Some(SignedDuration::from_hours(31 * 24))
    );

    // gh auth token, no window, the account's gh folder, no update check.
    let spawned = f.spawner.spawned();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].program, gh());
    assert_eq!(
        spawned[0].args,
        ["auth", "token", "--hostname", "github.com"].map(std::ffi::OsString::from)
    );
    assert!(
        spawned[0]
            .env
            .contains(&("GH_CONFIG_DIR".into(), Some(config_dir().into_os_string())))
    );
    assert_eq!(f.spawner.killed(), 1, "gh is not left running");

    let requests = f.http.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, Method::Get);
    assert_eq!(request.url, "https://api.github.com/copilot_internal/user");
    assert_eq!(request.body, None);
    assert_eq!(request.timeout, copilot::TIMEOUT);
    let auth = request
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v)
        .expect("Authorization header");
    assert!(
        matches!(auth, HeaderValue::Secret(_)),
        "the token is a secret"
    );
    assert_eq!(auth.as_str(), format!("token {TOKEN}"));
    assert!(
        !format!("{request:?}").contains(TOKEN),
        "Debug never shows the token"
    );
}

#[test]
fn a_paid_plan_shows_premium_requests_only() {
    let snap = fetch_answer(fixture("user-200-paid.json")).expect("fetch");
    assert_eq!(meters(&snap), vec![("premium".to_owned(), 0.6, false)]);
    assert_eq!(
        snap.meters[0].names,
        MeterNames::new("Prem", "premium", "Premium requests")
    );
    assert_eq!(snap.meters[0].order, 0);
    assert_eq!(
        snap.account.plan, None,
        "the answer does not say Pro or Pro+"
    );
}

#[test]
fn overage_goes_past_the_limit_without_being_limited() {
    let snap = fetch_answer(fixture("user-200-overage.json")).expect("fetch");
    let (id, used, limited) = &meters(&snap)[0];
    assert_eq!(id, "premium");
    assert!((used - 1.1).abs() < 1e-9, "used {used}");
    assert!(!limited, "overage is allowed");
    assert_eq!(snap.account.plan.as_deref(), Some("Business"));
}

#[test]
fn a_used_up_quota_is_limited_and_a_quota_without_a_share_is_left_out() {
    let snap = fetch_answer(fixture("user-200-used-up.json")).expect("fetch");
    assert_eq!(meters(&snap), vec![("chat".to_owned(), 1.0, true)]);
    // Only the date: 00:00 UTC; a cycle that started mid-month is a month long.
    let reset = ts("2026-10-20T00:00:00Z");
    assert_eq!(snap.meters[0].resets_at, Some(reset));
    assert_eq!(
        snap.meters[0].window.length_ending_at(reset),
        Some(SignedDuration::from_hours(30 * 24))
    );
}

#[test]
fn copilot_has_no_catalogue_meters() {
    assert!(catalog(ProviderId::Copilot).is_empty());
    for id in [copilot::PREMIUM, copilot::CHAT, copilot::COMPLETIONS] {
        let (names, _) = copilot::meter_names(id).expect("named");
        assert!(names.short_label.chars().count() <= 5);
    }
    assert_eq!(copilot::meter_names(MeterId::WEEKLY), None);
}

#[test]
fn http_errors_are_classified() {
    assert_eq!(
        fetch_answer(fixture("user-401.json")),
        Err(FetchError::Auth {
            from: SourceKind::CopilotApi
        })
    );
    assert!(matches!(
        fetch_answer(fixture("user-404.json")),
        Err(FetchError::Unavailable { .. })
    ));
    let mut limited = fixture("user-429.json");
    limited
        .headers
        .push(("Retry-After".to_owned(), "120".to_owned()));
    assert_eq!(
        fetch_answer(limited),
        Err(FetchError::RateLimited {
            from: SourceKind::CopilotApi,
            retry_after: Some(std::time::Duration::from_secs(120)),
        })
    );
    assert!(matches!(
        fetch_answer(fixture("user-503.txt")),
        Err(FetchError::Transient { .. })
    ));
    assert!(matches!(
        fetch_answer(fixture("user-200-no-snapshots.json")),
        Err(FetchError::Parse { .. })
    ));
    assert!(matches!(
        fetch_answer(HttpResponse::new(200, "<html>")),
        Err(FetchError::Parse { .. })
    ));
}

#[test]
fn without_gh_nothing_is_started_or_sent() {
    let f = Fakes::new(now());
    let result = fetch(&f);
    assert!(
        matches!(&result, Err(FetchError::Unavailable { detail }) if detail.contains("gh")),
        "{result:?}"
    );
    assert!(f.spawner.spawned().is_empty());
    assert!(f.http.requests().is_empty());
}

#[test]
fn gh_is_found_where_its_installer_puts_it() {
    let mut f = Fakes::new(now());
    f.env = FakeEnv::new(r"C:\Users\tester")
        .with_var("PATH", r"C:\Windows")
        .with_var("ProgramFiles", r"C:\Program Files");
    let installed = PathBuf::from(r"C:\Program Files\GitHub CLI\gh.exe");
    f.fs.add_file(&installed, "MZ", now());
    f.spawner.script(vec![FakeLine::Line(TOKEN.to_owned())]);
    f.http.respond(
        Method::Get,
        copilot::USER_URL,
        fixture("user-200-free.json"),
    );
    fetch(&f).expect("fetch");
    assert_eq!(f.spawner.spawned()[0].program, installed);
}

#[test]
fn gh_signed_out_is_a_sign_in_error_and_sends_nothing() {
    for output in [vec![FakeLine::Eof], vec![FakeLine::Line("   ".to_owned())]] {
        let f = with_gh();
        f.spawner.script(output);
        assert_eq!(
            fetch(&f),
            Err(FetchError::Auth {
                from: SourceKind::CopilotApi
            })
        );
        assert!(f.http.requests().is_empty());
    }
}

#[test]
fn a_silent_gh_times_out() {
    let f = with_gh();
    f.spawner.script(vec![FakeLine::Timeout]);
    assert!(matches!(fetch(&f), Err(FetchError::Transient { .. })));
    assert!(f.http.requests().is_empty());
    assert_eq!(f.spawner.killed(), 1);
}

#[test]
fn the_source_is_gh_s_config_folder() {
    let resolve = |env: &FakeEnv| match AccountSource::resolve(
        ProviderId::Copilot,
        &AccountSettings::primary(),
        env,
    ) {
        Ok(AccountSource::Copilot { config_dir, .. }) => config_dir,
        other => panic!("not a Copilot source: {other:?}"),
    };
    let home = FakeEnv::new(r"C:\Users\tester");
    assert_eq!(resolve(&home), config_dir());
    let roaming = home.clone().with_var("APPDATA", r"D:\Roaming");
    assert_eq!(resolve(&roaming), PathBuf::from(r"D:\Roaming\GitHub CLI"));
    let xdg = roaming.clone().with_var("XDG_CONFIG_HOME", r"D:\xdg");
    assert_eq!(resolve(&xdg), PathBuf::from(r"D:\xdg\gh"));
    let explicit = xdg.with_var("GH_CONFIG_DIR", r"D:\gh-work");
    assert_eq!(resolve(&explicit), PathBuf::from(r"D:\gh-work"));
}

#[test]
fn signing_in_again_changes_the_fingerprint() {
    let f = Fakes::new(now());
    let provider = CopilotProvider::new();
    let ctx = f.context(SignedDuration::from_secs(30));
    let absent = provider.credential_fingerprint(&ctx, &account());
    assert_eq!(
        absent,
        Some(CredentialFingerprint("absent".to_owned())),
        "never signed in: a fingerprint all the same, so the first sign-in lifts an auth stop"
    );
    let hosts = config_dir().join("hosts.yml");
    f.fs.add_file(&hosts, "github.com:\n", now());
    let first = provider.credential_fingerprint(&ctx, &account());
    assert!(first.is_some());
    assert_ne!(first, absent, "signing in changes it");
    f.fs.add_file(&hosts, "github.com:\n    user: x\n", now());
    assert_ne!(provider.credential_fingerprint(&ctx, &account()), first);
}

#[test]
fn sign_in_opens_gh_auth_login() {
    let command = CopilotProvider::new()
        .sign_in_command(&account())
        .expect("a sign-in command");
    assert_eq!(command.program, "gh");
    assert_eq!(
        command.args,
        ["auth", "login", "--hostname", "github.com", "--web"].map(std::ffi::OsString::from)
    );
    assert_eq!(
        command.env,
        vec![("GH_CONFIG_DIR".into(), Some(config_dir().into_os_string()))]
    );
    assert_eq!(
        CopilotProvider::new().usage_page(),
        Some("https://github.com/settings/copilot")
    );
}

#[test]
fn quota_names_are_the_english_meter_names_of_core() {
    // The surfaces show `grammar::meters::localized` names; in English they must be these.
    for id in [
        copilot::PREMIUM,
        copilot::CHAT,
        copilot::COMPLETIONS,
        "agents",
    ] {
        let quota = copilot::Quota {
            id,
            used: 0.5,
            limited: false,
        };
        let m = copilot::meter(&quota, None);
        assert_eq!(
            localized(ProviderId::Copilot, &m.id, &m.names, &English),
            m.names,
            "{id}"
        );
    }
}
