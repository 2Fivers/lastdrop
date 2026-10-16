//! The Codex provider against invented fixtures (tests/fixtures/codex/README.md): app-server
//! JSON-RPC transcripts through `FakeSpawner`, rollout tails through `FakeFs`.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::grammar::English;
use lastdrop_core::grammar::meters::localized;
use lastdrop_core::model::{
    AccountKey, AccountRef, Health, MeterId, ProviderId, SourceKind, WindowLen,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::settings::CodexOptions;
use lastdrop_providers::codex::{
    self, CodexProvider, CodexUsage, CodexWindow, app_server, meter, rollout, to_snapshot,
};
use lastdrop_providers::fakes::{FakeLine, FakeSpawner, Fakes};
use lastdrop_providers::io::{ChildIo, Clock, CredentialName, SpawnSpec, Spawner};
use lastdrop_providers::{AccountSource, FetchContext, FetchError, Provider};
use serde_json::Value;

// ---- Fixtures and helpers -------------------------------------------------------------------------

const RATE_LIMITS: &str = include_str!("fixtures/codex/app-server-rate-limits.jsonl");
const WEEKLY_ONLY: &str = include_str!("fixtures/codex/app-server-weekly-only.jsonl");
const NOT_SIGNED_IN: &str = include_str!("fixtures/codex/app-server-not-signed-in.jsonl");
const AUTH_ERROR: &str = include_str!("fixtures/codex/app-server-rate-limits-auth-error.jsonl");
const SERVER_ERROR: &str = include_str!("fixtures/codex/app-server-rate-limits-server-error.jsonl");
const API_KEY: &str = include_str!("fixtures/codex/app-server-api-key.jsonl");

const ROLLOUT_TWO_WINDOWS: &str = include_str!("fixtures/codex/rollout-tail-two-windows.jsonl");
const ROLLOUT_CUT_FIRST_LINE: &str =
    include_str!("fixtures/codex/rollout-tail-cut-first-line.jsonl");
const ROLLOUT_FIRST_LINE_ONLY: &str = include_str!("fixtures/codex/rollout-first-line-only.jsonl");
const ROLLOUT_PLAN_NULL: &str = include_str!("fixtures/codex/rollout-plan-null.jsonl");
const ROLLOUT_RESETS_IN_SECONDS: &str =
    include_str!("fixtures/codex/rollout-legacy-resets-in-seconds.jsonl");
const ROLLOUT_FLAT: &str = include_str!("fixtures/codex/rollout-legacy-flat.jsonl");
const ROLLOUT_UNKNOWN_SHAPE: &str = include_str!("fixtures/codex/rollout-unknown-shape.jsonl");
const ROLLOUT_SESSION_START: &str = include_str!("fixtures/codex/rollout-session-start.jsonl");

const LOCAL_APP_DATA: &str = r"C:\Users\tester\AppData\Local";

fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

/// The readout-spec §13 clock, 12:42 at +02:00.
fn now() -> Timestamp {
    ts("2026-10-10T10:42:00Z")
}

fn codex_home() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\.codex")
}

fn account() -> AccountSource {
    AccountSource::Codex {
        account: AccountKey::primary(),
        codex_home: codex_home(),
    }
}

fn bundled_exe(hash: &str) -> PathBuf {
    Path::new(LOCAL_APP_DATA)
        .join(r"OpenAI\Codex\bin")
        .join(hash)
        .join("codex.exe")
}

fn day_dir(date: &str) -> PathBuf {
    codex_home().join("sessions").join(date)
}

/// Fakes with `%LOCALAPPDATA%`, a Codex folder and one bundled `codex.exe`.
fn fakes() -> Fakes {
    let mut f = Fakes::new(now());
    f.env = f.env.clone().with_var("LOCALAPPDATA", LOCAL_APP_DATA);
    f.fs.add_file(
        codex_home().join("config.toml"),
        "model = \"gpt-fixture\"\n",
        now() - SignedDuration::from_hours(48),
    );
    f.fs.add_file(
        bundled_exe("a1b2c3d4e5f60718"),
        b"MZ-fixture".to_vec(),
        now() - SignedDuration::from_hours(24),
    );
    f
}

fn script(transcript: &str) -> Vec<FakeLine> {
    transcript
        .lines()
        .map(|l| FakeLine::Line(l.to_owned()))
        .collect()
}

fn budget() -> SignedDuration {
    SignedDuration::from_secs(30)
}

fn provider() -> CodexProvider {
    CodexProvider::new(CodexOptions::default())
}

fn provider_without_fallback() -> CodexProvider {
    CodexProvider::new(CodexOptions {
        rollout_fallback: false,
        ..CodexOptions::default()
    })
}

fn add_rollout(f: &Fakes, date: &str, name: &str, data: &str, modified: Timestamp) -> PathBuf {
    let path = day_dir(date).join(name);
    f.fs.add_file(&path, data, modified);
    path
}

fn json(line: &str) -> Value {
    serde_json::from_str(line).expect("JSON line")
}

fn assert_close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} != {b}");
}

// ---- App-server: the happy path -----------------------------------------------------------------

#[test]
fn app_server_reads_both_windows_by_length_and_the_plan() {
    let f = fakes();
    f.spawner.script(script(RATE_LIMITS));
    let snap = provider()
        .fetch(&f.context(budget()), &account())
        .expect("fetch");

    assert_eq!(snap.provider, ProviderId::Codex);
    assert_eq!(snap.source, SourceKind::CodexAppServer);
    assert_eq!(snap.health, Health::Ok);
    assert_eq!(snap.measured_at, now(), "the app-server answers live");
    assert_eq!(snap.fetched_at, now());
    assert_eq!(snap.account.key, AccountKey::primary());
    assert_eq!(snap.account.plan.as_deref(), Some("Plus"));
    assert!(snap.balances.is_empty());

    assert_eq!(snap.meters.len(), 2);
    let five = &snap.meters[0];
    assert_eq!(five.id, MeterId::new(MeterId::FIVE_HOUR));
    assert_eq!(
        (
            five.names.short_label.as_str(),
            five.names.name.as_str(),
            five.order
        ),
        ("5h", "5-hour session", 0)
    );
    assert_close(five.used, 0.42);
    assert!(!five.limited);
    assert_eq!(five.resets_at, Some(ts("2026-10-10T12:56:00Z")));
    assert_eq!(five.window, WindowLen::Fixed(SignedDuration::from_hours(5)));
    assert!(
        five.starts_on_use,
        "R§2: Codex's 5-hour session starts on use"
    );

    let weekly = &snap.meters[1];
    assert_eq!(weekly.id, MeterId::new(MeterId::WEEKLY));
    assert_eq!(
        (
            weekly.names.short_label.as_str(),
            weekly.names.name.as_str()
        ),
        ("Wk", "Weekly")
    );
    assert_close(weekly.used, 0.61);
    assert_eq!(weekly.resets_at, Some(ts("2026-10-15T07:00:00Z")));
    assert_eq!(
        weekly.window,
        WindowLen::Fixed(SignedDuration::from_hours(7 * 24))
    );
    assert!(!weekly.starts_on_use);

    // Never keeps or prints the account's e-mail address.
    let printed = format!("{snap:?}");
    assert!(!printed.contains("example.invalid"), "{printed}");
}

#[test]
fn app_server_conversation_is_initialize_initialized_account_rate_limits_then_kill() {
    let f = fakes();
    f.spawner.script(script(RATE_LIMITS));
    provider()
        .fetch(&f.context(budget()), &account())
        .expect("fetch");

    let spawned = f.spawner.spawned();
    assert_eq!(spawned.len(), 1);
    let spec = &spawned[0];
    assert_eq!(spec.program, bundled_exe("a1b2c3d4e5f60718"));
    assert_eq!(
        spec.args,
        ["app-server", "-c", "features.plugins=false"],
        "plugins off: no marketplace sync at start-up"
    );
    assert_eq!(
        spec.env,
        [("CODEX_HOME".into(), Some(codex_home().into_os_string()))]
    );
    assert_eq!(spec.cwd.as_deref(), Some(codex_home().as_path()));

    let written = f.spawner.written();
    assert_eq!(written.len(), 4, "{written:?}");
    let init = json(&written[0]);
    assert_eq!(init["method"], "initialize");
    assert_eq!(init["id"], 1);
    assert_eq!(init["params"]["clientInfo"]["name"], "lastdrop");
    assert_eq!(
        init["params"]["clientInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(init.get("jsonrpc").is_none(), "Codex's wire omits jsonrpc");

    let initialized = json(&written[1]);
    assert_eq!(initialized["method"], "initialized");
    assert!(initialized.get("id").is_none(), "a notification has no id");

    let account_read = json(&written[2]);
    assert_eq!(account_read["method"], "account/read");
    assert_eq!(account_read["params"]["refreshToken"], false);

    let limits = json(&written[3]);
    assert_eq!(limits["method"], app_server::METHOD_RATE_LIMITS);
    assert_eq!(limits["params"]["excludeResetCreditDetails"], true);
    assert!(
        limits["params"].get("supportsLunaReserve").is_none(),
        "never supportsLunaReserve"
    );
    assert_ne!(limits["id"], account_read["id"]);

    assert_eq!(f.spawner.killed(), 1, "the child is always killed");
}

#[test]
fn a_plan_without_a_five_hour_limit_reports_weekly_in_primary_and_it_stays_weekly() {
    let f = fakes();
    f.spawner.script(script(WEEKLY_ONLY));
    let snap = provider()
        .fetch(&f.context(budget()), &account())
        .expect("fetch");
    let ids: Vec<&str> = snap.meters.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, [MeterId::WEEKLY], "mapped by length, not position");
    assert_close(snap.meters[0].used, 0.05);
    assert!(!snap.meters[0].starts_on_use);
    assert_eq!(snap.account.plan.as_deref(), Some("Pro Lite"));
}

// ---- App-server: failures -----------------------------------------------------------------------

#[test]
fn signed_out_is_a_sign_in_error_without_asking_for_rate_limits() {
    let f = fakes();
    f.spawner.script(script(NOT_SIGNED_IN));
    let err = provider_without_fallback()
        .fetch(&f.context(budget()), &account())
        .expect_err("signed out");
    assert_eq!(
        err,
        FetchError::Auth {
            from: SourceKind::CodexAppServer
        }
    );
    let methods: Vec<Value> = f
        .spawner
        .written()
        .iter()
        .map(|l| json(l)["method"].clone())
        .collect();
    assert_eq!(methods, ["initialize", "initialized", "account/read"]);
    assert_eq!(f.spawner.killed(), 1);
}

#[test]
fn signed_out_with_a_rollout_shows_the_last_reading_marked_sign_in_required() {
    let f = fakes();
    f.spawner.script(script(NOT_SIGNED_IN));
    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-20-00-00000000-0000-4000-8000-000000000001.jsonl",
        ROLLOUT_TWO_WINDOWS,
        ts("2026-10-10T10:30:03Z"),
    );
    let snap = provider()
        .fetch(&f.context(budget()), &account())
        .expect("rollout");
    assert_eq!(snap.source, SourceKind::CodexRollout);
    assert_eq!(snap.health, Health::SignInRequired);
    assert_eq!(snap.measured_at, ts("2026-10-10T10:30:01Z"));
    assert_eq!(snap.fetched_at, now());
    assert_close(snap.meters[0].used, 0.42);
}

#[test]
fn signed_out_without_a_rollout_reports_the_sign_in_error_not_the_missing_file() {
    let f = fakes();
    f.spawner.script(script(NOT_SIGNED_IN));
    let err = provider()
        .fetch(&f.context(budget()), &account())
        .expect_err("no data");
    assert!(matches!(err, FetchError::Auth { .. }), "{err:?}");
}

#[test]
fn a_rate_limits_authentication_error_is_a_sign_in_error() {
    let f = fakes();
    f.spawner.script(script(AUTH_ERROR));
    let err = provider_without_fallback()
        .fetch(&f.context(budget()), &account())
        .expect_err("auth");
    assert!(matches!(err, FetchError::Auth { .. }), "{err:?}");
    assert_eq!(f.spawner.killed(), 1);
}

#[test]
fn a_server_error_is_transient_and_its_message_is_not_repeated() {
    let f = fakes();
    f.spawner.script(script(SERVER_ERROR));
    let err = provider_without_fallback()
        .fetch(&f.context(budget()), &account())
        .expect_err("server error");
    assert_eq!(
        err,
        FetchError::Transient {
            from: SourceKind::CodexAppServer,
            detail: "app-server error -32603".to_owned(),
            // Not before the app-server may start again (15 min less 30 s).
            retry_after: Some(Duration::from_secs(14 * 60 + 30)),
        }
    );
}

#[test]
fn an_api_key_sign_in_has_no_plan_limits() {
    let f = fakes();
    f.spawner.script(script(API_KEY));
    let err = provider_without_fallback()
        .fetch(&f.context(budget()), &account())
        .expect_err("api key");
    assert!(matches!(err, FetchError::Unavailable { .. }), "{err:?}");
    assert_eq!(f.spawner.written().len(), 3, "no rate-limit request");
}

#[test]
fn an_initialize_timeout_is_transient_and_kills_the_child() {
    let f = fakes();
    f.spawner.script(vec![FakeLine::Timeout]);
    let err = provider_without_fallback()
        .fetch(&f.context(budget()), &account())
        .expect_err("timeout");
    assert_eq!(err.detail(), "codex app-server timed out");
    assert!(matches!(err, FetchError::Transient { .. }));
    assert_eq!(f.spawner.killed(), 1);
    assert_eq!(f.spawner.written().len(), 1, "only initialize was sent");
}

#[test]
fn a_rate_limits_timeout_falls_back_to_the_rollout() {
    let f = fakes();
    let mut lines = script(RATE_LIMITS);
    lines.truncate(4); // initialize, notification, log line, account/read
    lines.push(FakeLine::Timeout);
    f.spawner.script(lines);
    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-20-00-a.jsonl",
        ROLLOUT_TWO_WINDOWS,
        ts("2026-10-10T10:30:03Z"),
    );
    let snap = provider()
        .fetch(&f.context(budget()), &account())
        .expect("rollout");
    assert_eq!(snap.source, SourceKind::CodexRollout);
    assert_eq!(snap.health, Health::Stale, "cached values, not live");
    assert_eq!(f.spawner.killed(), 1);
}

#[test]
fn a_child_that_exits_early_is_transient() {
    let f = fakes();
    f.spawner.script(vec![FakeLine::Eof]);
    let err = provider_without_fallback()
        .fetch(&f.context(budget()), &account())
        .expect_err("exited");
    assert_eq!(err.detail(), "codex app-server exited");
    assert_eq!(f.spawner.killed(), 1);
}

#[test]
fn no_time_left_fails_before_starting_a_process() {
    let f = fakes();
    f.spawner.script(script(RATE_LIMITS));
    let err = provider_without_fallback()
        .fetch(&f.context(SignedDuration::ZERO), &account())
        .expect_err("deadline");
    assert!(matches!(err, FetchError::Transient { .. }), "{err:?}");
    assert!(f.spawner.spawned().is_empty());
}

#[test]
fn without_codex_exe_the_rollout_answers_and_no_process_starts() {
    let mut f = Fakes::new(now());
    f.env = f.env.clone().with_var("LOCALAPPDATA", LOCAL_APP_DATA);
    let err = provider_without_fallback()
        .fetch(&f.context(budget()), &account())
        .expect_err("no exe");
    assert_eq!(
        err,
        FetchError::Unavailable {
            detail: "codex.exe not found".to_owned()
        }
    );

    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-20-00-a.jsonl",
        ROLLOUT_TWO_WINDOWS,
        ts("2026-10-10T10:30:03Z"),
    );
    let snap = provider()
        .fetch(&f.context(budget()), &account())
        .expect("rollout");
    assert_eq!(snap.source, SourceKind::CodexRollout);
    assert!(f.spawner.spawned().is_empty());
}

#[test]
fn a_missing_codex_folder_is_unavailable_and_starts_nothing() {
    let f = fakes();
    f.spawner.script(script(RATE_LIMITS));
    let elsewhere = AccountSource::Codex {
        account: AccountKey("work".to_owned()),
        codex_home: PathBuf::from(r"C:\Users\tester\.codex-work"),
    };
    let err = provider()
        .fetch(&f.context(budget()), &elsewhere)
        .expect_err("no folder");
    assert!(matches!(err, FetchError::Unavailable { .. }), "{err:?}");
    assert!(f.spawner.spawned().is_empty());
}

#[test]
fn a_non_codex_account_is_unavailable() {
    let f = fakes();
    let meshy = AccountSource::Meshy {
        account: AccountKey::primary(),
        credential: CredentialName {
            provider: ProviderId::Meshy,
            account: AccountKey::primary(),
            name: "api_key".to_owned(),
        },
    };
    assert!(matches!(
        provider().fetch(&f.context(budget()), &meshy),
        Err(FetchError::Unavailable { .. })
    ));
    assert_eq!(
        provider().credential_fingerprint(&f.context(budget()), &meshy),
        None
    );
    assert_eq!(provider().sign_in_command(&meshy), None);
}

// ---- Finding codex.exe --------------------------------------------------------------------------

#[test]
fn the_newest_bundled_codex_exe_wins_and_folders_without_one_are_skipped() {
    let f = fakes();
    f.fs.add_file(
        bundled_exe("ffffffffffffffff"),
        b"MZ-new".to_vec(),
        now() - SignedDuration::from_hours(2),
    );
    f.fs.add_file(
        bundled_exe("0000000000000000"),
        b"MZ-old".to_vec(),
        now() - SignedDuration::from_hours(200),
    );
    // A newer sibling folder with only a helper tool.
    f.fs.add_file(
        Path::new(LOCAL_APP_DATA).join(r"OpenAI\Codex\bin\9999999999999999\rg.exe"),
        b"MZ-rg".to_vec(),
        now(),
    );
    let ctx = f.context(budget());
    assert_eq!(
        app_server::find_codex_exe(&ctx, None),
        Some(bundled_exe("ffffffffffffffff"))
    );
}

#[test]
fn the_configured_codex_exe_wins_when_it_exists() {
    let f = fakes();
    let custom = PathBuf::from(r"D:\tools\codex\codex.exe");
    let ctx = f.context(budget());
    assert_eq!(
        app_server::find_codex_exe(&ctx, Some(&custom)),
        Some(bundled_exe("a1b2c3d4e5f60718")),
        "a missing override falls back to the search"
    );
    f.fs.add_file(&custom, b"MZ-custom".to_vec(), now());
    assert_eq!(
        app_server::find_codex_exe(&ctx, Some(&custom)),
        Some(custom)
    );
}

#[test]
fn without_a_bundled_one_codex_exe_then_codex_cmd_come_from_path() {
    let mut f = Fakes::new(now());
    f.env = f
        .env
        .clone()
        .with_var("LOCALAPPDATA", LOCAL_APP_DATA)
        .with_var(
            "PATH",
            r"relative\bin;C:\Windows\System32;C:\Users\tester\AppData\Roaming\npm;C:\Tools",
        );
    f.fs.add_file(
        r"C:\Users\tester\AppData\Roaming\npm\codex.cmd",
        "@ECHO off\r\n",
        now(),
    );
    f.fs.add_file(r"relative\bin\codex.exe", b"MZ-rel".to_vec(), now());
    let ctx = f.context(budget());
    let script_path = PathBuf::from(r"C:\Users\tester\AppData\Roaming\npm\codex.cmd");
    assert_eq!(
        app_server::find_codex_exe(&ctx, None),
        Some(script_path.clone()),
        "relative PATH entries are ignored"
    );

    // Started like an executable: the spawner runs a `.cmd` through cmd.exe itself.
    let spec = app_server::spawn_spec(&script_path, &codex_home());
    assert_eq!(spec.program, script_path);
    assert_eq!(spec.args, ["app-server", "-c", "features.plugins=false"]);
    assert_eq!(
        spec.env,
        [("CODEX_HOME".into(), Some(codex_home().into_os_string()))]
    );

    f.fs.add_file(r"C:\Tools\codex.exe", b"MZ-tools".to_vec(), now());
    assert_eq!(
        app_server::find_codex_exe(&f.context(budget()), None),
        Some(PathBuf::from(r"C:\Tools\codex.exe")),
        "codex.exe anywhere on PATH before codex.cmd"
    );
}

#[test]
fn local_app_data_defaults_to_the_profile_folder() {
    let f = Fakes::new(now());
    f.fs.add_file(bundled_exe("a1b2c3d4e5f60718"), b"MZ".to_vec(), now());
    assert_eq!(
        app_server::find_codex_exe(&f.context(budget()), None),
        Some(bundled_exe("a1b2c3d4e5f60718"))
    );
}

// ---- App-server: line parsing -------------------------------------------------------------------

#[test]
fn only_the_response_with_the_awaited_id_is_parsed() {
    let skip = [
        r#"{"method":"account/rateLimits/updated","params":{}}"#,
        // A server request may reuse a client id; only responses lack a method.
        r#"{"id":3,"method":"item/tool/requestUserInput","params":{}}"#,
        r#"{"id":2,"result":{"rateLimits":{}}}"#,
        "2026-10-10T10:42:00Z INFO codex_app_server: listening",
        "",
        "{not json",
    ];
    for line in skip {
        assert_eq!(
            app_server::parse_rate_limits(line, 3).expect("skip"),
            None,
            "{line}"
        );
    }
    // The placeholder line of tests/providers.rs (with "jsonrpc") parses the same way.
    let line = r#"{"jsonrpc":"2.0","id":"3","result":{"rateLimits":{"primary":{"usedPercent":12.0,"windowDurationMins":300,"resetsAt":1760104560},"secondary":{"usedPercent":5.0,"windowDurationMins":10080,"resetsAt":1760624760}}}}"#;
    let usage = app_server::parse_rate_limits(line, 3)
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
        usage.primary.and_then(|w| w.resets_at),
        Some(Timestamp::from_second(1_760_104_560).expect("ts"))
    );
    assert_eq!(usage.measured_at, None);
}

#[test]
fn the_codex_bucket_is_preferred_and_unreadable_results_are_parse_errors() {
    let line = r#"{"id":3,"result":{"rateLimits":{"primary":{"usedPercent":90,"windowDurationMins":300}},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":10,"windowDurationMins":300},"planType":"team"}}}}"#;
    let usage = app_server::parse_rate_limits(line, 3)
        .expect("parse")
        .expect("response");
    assert_eq!(usage.primary.map(|w| w.used_percent), Some(10.0));
    assert_eq!(usage.plan.as_deref(), Some("Team"));

    for bad in [
        r#"{"id":3,"result":{}}"#,
        r#"{"id":3,"result":null}"#,
        r#"{"id":3,"result":{"rateLimits":{"primary":null,"secondary":null}}}"#,
        r#"{"id":3,"result":{"rateLimits":{"primary":{"windowDurationMins":300}}}}"#,
    ] {
        assert!(
            matches!(
                app_server::parse_rate_limits(bad, 3),
                Err(FetchError::Parse { .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn error_responses_are_classified_without_their_text() {
    let cases = [
        (
            "chatgpt authentication required to read rate limits",
            "auth",
        ),
        (
            "failed to fetch codex rate limits: 401 Unauthorized",
            "auth",
        ),
        (
            "Your access token could not be refreshed because your refresh token was already used. Please log out and sign in again.",
            "auth",
        ),
        (
            "failed to fetch codex rate limits: 429 Too Many Requests",
            "throttled",
        ),
        ("failed to fetch codex rate limits: timed out", "transient"),
    ];
    for (message, want) in cases {
        let line = serde_json::json!({ "id": 3, "error": { "code": -32603, "message": message } })
            .to_string();
        let err = app_server::parse_rate_limits(&line, 3).expect_err(message);
        let got = match &err {
            FetchError::Auth { .. } => "auth",
            FetchError::RateLimited { .. } => "throttled",
            FetchError::Transient { .. } => "transient",
            _ => "other",
        };
        assert_eq!(got, want, "{message}");
        assert!(!err.detail().contains("rate limits"), "{err:?}");
    }
}

#[test]
fn account_read_gives_the_sign_in_state_and_only_the_plan() {
    let cases = [
        (
            r#"{"id":2,"result":{"account":{"type":"chatgpt","email":"tester@example.invalid","planType":"pro"},"requiresOpenaiAuth":true}}"#,
            app_server::AccountState::ChatGpt {
                plan: Some("Pro".to_owned()),
            },
        ),
        (
            r#"{"id":2,"result":{"account":{"type":"chatgpt","email":null,"planType":"unknown"},"requiresOpenaiAuth":true}}"#,
            app_server::AccountState::ChatGpt { plan: None },
        ),
        (
            r#"{"id":2,"result":{"account":null,"requiresOpenaiAuth":true}}"#,
            app_server::AccountState::SignedOut,
        ),
        (
            r#"{"id":2,"result":{"account":{"type":"apiKey"},"requiresOpenaiAuth":false}}"#,
            app_server::AccountState::ApiKey,
        ),
        (
            r#"{"id":2,"result":{"account":{"type":"amazonBedrock"},"requiresOpenaiAuth":false}}"#,
            app_server::AccountState::Other,
        ),
    ];
    for (line, want) in cases {
        assert_eq!(
            app_server::parse_account(line, 2).expect("parse"),
            Some(want),
            "{line}"
        );
    }
    assert_eq!(
        app_server::parse_account(r#"{"id":3,"result":{}}"#, 2).expect("skip"),
        None
    );
}

#[test]
fn request_lines_are_single_json_objects() {
    for line in [
        app_server::initialize_request(1),
        app_server::initialized_notification(),
        app_server::account_request(2),
        app_server::rate_limits_request(3),
    ] {
        assert!(!line.contains('\n'), "{line}");
        assert!(json(&line).is_object());
    }
    assert_eq!(json(&app_server::rate_limits_request(7))["id"], 7);
}

// ---- Meters -------------------------------------------------------------------------------------

fn window(used_percent: f64, window_minutes: Option<i64>) -> CodexWindow {
    CodexWindow {
        used_percent,
        window_minutes,
        resets_at: Some(ts("2026-10-10T12:56:00Z")),
    }
}

#[test]
fn windows_are_named_by_length_with_the_r2_definitions() {
    let cases: [(Option<i64>, bool, &str, &str, u16); 9] = [
        (Some(300), false, MeterId::FIVE_HOUR, "5h", 0),
        (Some(300), true, MeterId::FIVE_HOUR, "5h", 0),
        (Some(297), true, MeterId::FIVE_HOUR, "5h", 0),
        (Some(303), false, MeterId::FIVE_HOUR, "5h", 0),
        (Some(10080), false, MeterId::WEEKLY, "Wk", 1),
        (Some(10083), true, MeterId::WEEKLY, "Wk", 1),
        (None, false, MeterId::FIVE_HOUR, "5h", 0),
        (None, true, MeterId::WEEKLY, "Wk", 1),
        (Some(0), true, MeterId::WEEKLY, "Wk", 1),
    ];
    for (minutes, secondary, id, label, order) in cases {
        let m = meter(&window(10.0, minutes), secondary);
        assert_eq!(m.id.0, id, "{minutes:?} {secondary}");
        assert_eq!(m.names.short_label, label);
        assert_eq!(m.order, order);
        assert_eq!(m.starts_on_use, id == MeterId::FIVE_HOUR);
    }
}

#[test]
fn other_lengths_are_fixed_windows_named_by_their_length() {
    let daily = meter(&window(25.0, Some(1440)), false);
    assert_eq!(daily.id, MeterId("window:1440".to_owned()));
    assert_eq!(
        (
            daily.names.short_label.as_str(),
            daily.names.short_name.as_str(),
            daily.names.name.as_str()
        ),
        ("24h", "24-hour", "24-hour window")
    );
    assert_eq!(
        daily.window,
        WindowLen::Fixed(SignedDuration::from_hours(24))
    );
    assert_eq!(daily.order, 2);
    assert!(!daily.starts_on_use);

    let monthly = meter(&window(25.0, Some(43_200)), true);
    assert_eq!(monthly.names.short_label, "30d");
    assert_eq!(monthly.names.name, "30-day window");
    assert_eq!(monthly.order, 3);

    let hour = meter(&window(1.0, Some(60)), false);
    assert_eq!(hour.names.short_label, "1h");
    for m in [daily, monthly, hour] {
        assert!(m.names.short_label.chars().count() <= 5);
        // The surfaces show `grammar::meters::localized` names; in English they are these.
        assert_eq!(
            localized(ProviderId::Codex, &m.id, &m.names, &English),
            m.names
        );
    }
}

#[test]
fn used_share_is_percent_over_100_never_negative_and_may_exceed_1() {
    assert_close(meter(&window(42.0, Some(300)), false).used, 0.42);
    assert_close(meter(&window(-3.0, Some(300)), false).used, 0.0);
    assert_close(meter(&window(104.0, Some(300)), false).used, 1.04);
}

fn usage(primary: Option<CodexWindow>, secondary: Option<CodexWindow>) -> CodexUsage {
    CodexUsage {
        primary,
        secondary,
        plan: None,
        measured_at: None,
    }
}

#[test]
fn snapshots_keep_meter_order_and_drop_duplicate_windows() {
    // Weekly reported first, 5-hour second: meters still in meter order.
    let u = usage(Some(window(5.0, Some(10080))), Some(window(9.0, Some(300))));
    let snap = to_snapshot(&u, SourceKind::CodexAppServer, AccountRef::primary(), now());
    let ids: Vec<&str> = snap.meters.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, [MeterId::FIVE_HOUR, MeterId::WEEKLY]);

    // An unknown-length primary next to a 5-hour secondary becomes the weekly meter.
    let u = usage(Some(window(5.0, None)), Some(window(9.0, Some(300))));
    let snap = to_snapshot(&u, SourceKind::CodexAppServer, AccountRef::primary(), now());
    let ids: Vec<&str> = snap.meters.iter().map(|m| m.id.0.as_str()).collect();
    assert_eq!(ids, [MeterId::FIVE_HOUR, MeterId::WEEKLY]);
    assert_close(snap.meters[1].used, 0.05);

    // Two windows of the same known length: the first is kept.
    let u = usage(Some(window(5.0, Some(300))), Some(window(9.0, Some(300))));
    let snap = to_snapshot(&u, SourceKind::CodexAppServer, AccountRef::primary(), now());
    assert_eq!(snap.meters.len(), 1);
    assert_close(snap.meters[0].used, 0.05);

    // No windows: no meters (a missing value is never 0).
    let snap = to_snapshot(
        &CodexUsage::default(),
        SourceKind::CodexAppServer,
        AccountRef::primary(),
        now(),
    );
    assert!(snap.meters.is_empty());
}

#[test]
fn snapshot_times_plan_and_health() {
    let mut u = usage(Some(window(5.0, Some(300))), None);
    u.measured_at = Some(now() - SignedDuration::from_mins(12));
    u.plan = Some("Plus".to_owned());
    let account = AccountRef {
        plan: Some("Old".to_owned()),
        ..AccountRef::primary()
    };
    let snap = to_snapshot(&u, SourceKind::CodexRollout, account.clone(), now());
    assert_eq!(snap.measured_at, now() - SignedDuration::from_mins(12));
    assert_eq!(snap.fetched_at, now());
    assert_eq!(snap.reading_at(), snap.measured_at);
    assert_eq!(snap.health, Health::Stale);
    assert_eq!(snap.account.plan.as_deref(), Some("Plus"));

    // A clock that ran ahead never makes a reading newer than its fetch.
    u.measured_at = Some(now() + SignedDuration::from_mins(5));
    u.plan = None;
    let snap = to_snapshot(&u, SourceKind::CodexAppServer, account, now());
    assert_eq!(snap.measured_at, now());
    assert_eq!(snap.health, Health::Ok);
    assert_eq!(
        snap.account.plan.as_deref(),
        Some("Old"),
        "kept when the source names none"
    );
}

#[test]
fn plan_names() {
    for (raw, want) in [
        ("plus", Some("Plus")),
        ("pro", Some("Pro")),
        ("prolite", Some("Pro Lite")),
        ("business", Some("Business")),
        ("self_serve_business_usage_based", Some("Business")),
        ("enterprise_cbp_automation", Some("Enterprise")),
        ("edu_plus", Some("Edu Plus")),
        ("new_shiny_tier", Some("New Shiny Tier")),
        ("unknown", None),
        ("", None),
    ] {
        assert_eq!(codex::plan_name(raw).as_deref(), want, "{raw}");
    }
}

// ---- Rollout ------------------------------------------------------------------------------------

#[test]
fn rollout_tail_gives_the_newest_codex_event_skipping_nulls_and_other_buckets() {
    let usage = rollout::parse_tail(ROLLOUT_TWO_WINDOWS.as_bytes())
        .expect("parse")
        .expect("event");
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T10:30:01Z")));
    assert_eq!(usage.plan.as_deref(), Some("Plus"));
    let primary = usage.primary.expect("primary");
    assert_close(primary.used_percent, 42.0);
    assert_eq!(primary.window_minutes, Some(300));
    assert_eq!(primary.resets_at, Some(ts("2026-10-10T12:56:00Z")));
    let secondary = usage.secondary.expect("secondary");
    assert_close(secondary.used_percent, 61.0);
    assert_eq!(secondary.resets_at, Some(ts("2026-10-15T07:00:00Z")));
}

#[test]
fn rollout_tail_skips_the_cut_first_line() {
    let usage = rollout::parse_tail(ROLLOUT_CUT_FIRST_LINE.as_bytes())
        .expect("parse")
        .expect("event");
    assert_close(usage.primary.expect("primary").used_percent, 7.0);
    assert_eq!(usage.plan.as_deref(), Some("Pro"));

    // The first line of a tail is always dropped, even when it looks whole …
    assert_eq!(
        rollout::parse_tail(ROLLOUT_FIRST_LINE_ONLY.as_bytes()).expect("parse"),
        None
    );
    // … but a file shorter than the tail is read whole, first line included.
    let f = fakes();
    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-10-00-a.jsonl",
        ROLLOUT_FIRST_LINE_ONLY,
        ts("2026-10-10T10:10:00Z"),
    );
    let usage = rollout::fetch(&f.context(budget()), &codex_home()).expect("whole file");
    assert_close(usage.primary.expect("primary").used_percent, 15.0);
}

#[test]
fn rollout_tail_with_crlf_and_without_events() {
    let crlf = ROLLOUT_TWO_WINDOWS.replace('\n', "\r\n");
    let usage = rollout::parse_tail(crlf.as_bytes())
        .expect("parse")
        .expect("event");
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T10:30:01Z")));
    assert_eq!(
        rollout::parse_tail(ROLLOUT_SESSION_START.as_bytes()).expect("parse"),
        None
    );
    assert_eq!(rollout::parse_tail(b"").expect("parse"), None);
}

#[test]
fn a_null_plan_is_not_a_plan_change() {
    let usage = rollout::parse_tail(ROLLOUT_PLAN_NULL.as_bytes())
        .expect("parse")
        .expect("event");
    assert_close(usage.primary.as_ref().expect("primary").used_percent, 21.0);
    assert_eq!(usage.plan.as_deref(), Some("Pro"), "from the earlier event");
    assert_eq!(
        meter(usage.primary.as_ref().expect("primary"), false).id.0,
        MeterId::WEEKLY
    );
}

#[test]
fn legacy_resets_in_seconds_count_from_the_line_time() {
    let usage = rollout::parse_tail(ROLLOUT_RESETS_IN_SECONDS.as_bytes())
        .expect("parse")
        .expect("event");
    let primary = usage.primary.expect("primary");
    let secondary = usage.secondary.expect("secondary");
    assert_eq!(primary.resets_at, Some(ts("2026-10-10T12:00:00Z")));
    assert_eq!(secondary.resets_at, Some(ts("2026-10-11T10:00:00Z")));
    assert_eq!(meter(&primary, false).id.0, MeterId::FIVE_HOUR);
    assert_eq!(meter(&secondary, true).id.0, MeterId::WEEKLY);
    assert_eq!(usage.plan, None);
}

#[test]
fn legacy_flat_rate_limits_have_no_reset_times() {
    let usage = rollout::parse_tail(ROLLOUT_FLAT.as_bytes())
        .expect("parse")
        .expect("event");
    assert_eq!(
        usage.primary,
        Some(CodexWindow {
            used_percent: 12.0,
            window_minutes: Some(300),
            resets_at: None
        })
    );
    assert_eq!(
        usage.secondary,
        Some(CodexWindow {
            used_percent: 34.0,
            window_minutes: Some(10080),
            resets_at: None
        })
    );
}

#[test]
fn an_unknown_rate_limits_shape_is_a_parse_error() {
    assert!(matches!(
        rollout::parse_tail(ROLLOUT_UNKNOWN_SHAPE.as_bytes()),
        Err(FetchError::Parse {
            from: SourceKind::CodexRollout,
            ..
        })
    ));
    let f = fakes();
    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-00-00-a.jsonl",
        ROLLOUT_UNKNOWN_SHAPE,
        ts("2026-10-10T10:00:00Z"),
    );
    assert!(matches!(
        rollout::fetch(&f.context(budget()), &codex_home()),
        Err(FetchError::Parse { .. })
    ));
}

#[test]
fn rollout_files_newest_written_first_across_day_folders() {
    let f = fakes();
    // Today: a session that just started (no event yet) and an older one.
    let started = add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-40-00-b.jsonl",
        ROLLOUT_SESSION_START,
        ts("2026-10-10T10:40:01Z"),
    );
    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T08-00-00-a.jsonl",
        ROLLOUT_FLAT,
        ts("2026-10-10T10:00:00Z"),
    );
    // Yesterday's session, resumed today: the newest event lives in an older folder.
    let resumed = add_rollout(
        &f,
        r"2026\10\09",
        "rollout-2026-10-09T22-00-00-c.jsonl",
        ROLLOUT_TWO_WINDOWS,
        ts("2026-10-10T10:30:03Z"),
    );
    // A compressed rollout and a stray file are ignored.
    f.fs.add_file(
        day_dir(r"2026\10\10").join("rollout-2026-10-01T00-00-00-z.jsonl.zst"),
        b"\x28\xb5\x2f\xfd".to_vec(),
        now(),
    );
    f.fs.add_file(day_dir(r"2026\10\10").join("notes.txt"), "x", now());
    f.fs.add_file(
        codex_home().join(r"sessions\misc\rollout-x.jsonl"),
        ROLLOUT_PLAN_NULL,
        now(),
    );

    let ctx = f.context(budget());
    assert_eq!(rollout::newest(&ctx, &codex_home()), Some(started));
    let names: Vec<PathBuf> = rollout::candidates(&ctx, &codex_home())
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[1], resumed);

    let usage = rollout::fetch(&ctx, &codex_home()).expect("fetch");
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T10:30:01Z")));
    assert_close(usage.primary.expect("primary").used_percent, 42.0);
}

#[test]
fn rollout_without_sessions_is_unavailable() {
    let f = fakes();
    assert!(matches!(
        rollout::fetch(&f.context(budget()), &codex_home()),
        Err(FetchError::Unavailable { .. })
    ));
    assert_eq!(rollout::newest(&f.context(budget()), &codex_home()), None);
    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-40-00-b.jsonl",
        ROLLOUT_SESSION_START,
        now(),
    );
    assert_eq!(
        rollout::fetch(&f.context(budget()), &codex_home()),
        Err(FetchError::Unavailable {
            detail: "no rate limits in recent Codex sessions".to_owned()
        })
    );
}

#[test]
fn rollout_fallback_can_be_turned_off() {
    let mut f = Fakes::new(now());
    f.env = f.env.clone().with_var("LOCALAPPDATA", LOCAL_APP_DATA);
    add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-20-00-a.jsonl",
        ROLLOUT_TWO_WINDOWS,
        now(),
    );
    assert!(matches!(
        provider_without_fallback().fetch(&f.context(budget()), &account()),
        Err(FetchError::Unavailable { .. })
    ));
}

// ---- Pace: a fresh rollout first, the app-server at most every 15 minutes -----------------------

/// The time of `ROLLOUT_TWO_WINDOWS`' last `codex` event.
const EVENT_AT: &str = "2026-10-10T10:30:01Z";

/// `ROLLOUT_TWO_WINDOWS` as today's session, last written two seconds after its last event.
fn with_rollout(f: &Fakes) {
    add_rollout(
        f,
        r"2026\10\10",
        "rollout-2026-10-10T10-20-00-a.jsonl",
        ROLLOUT_TWO_WINDOWS,
        ts("2026-10-10T10:30:03Z"),
    );
}

fn at(f: &Fakes, when: &str) -> SignedDuration {
    f.clock.set(ts(when));
    budget()
}

/// Runs `hook` whenever it starts a child, then starts it like `FakeSpawner`: what happens while
/// an app-server runs (Codex renewing its sign-in, the user signing in, another fetch).
struct HookSpawner<'a, H> {
    inner: &'a FakeSpawner,
    hook: H,
}

impl<H: Fn() + Send + Sync> Spawner for HookSpawner<'_, H> {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>> {
        (self.hook)();
        self.inner.spawn(spec)
    }
}

/// `f.context(budget())` with another spawner.
fn context_with<'a>(f: &'a Fakes, spawner: &'a dyn Spawner) -> FetchContext<'a> {
    FetchContext {
        spawner,
        ..f.context(budget())
    }
}

/// An `auth.json` written a day before [`now`].
fn with_auth_json(f: &Fakes) -> PathBuf {
    let auth = codex_home().join("auth.json");
    f.fs.add_file(
        &auth,
        r#"{"auth_mode":"fixture"}"#,
        now() - SignedDuration::from_hours(24),
    );
    auth
}

/// A spawner that rewrites `auth.json` (dated `now()`) as it starts the child.
fn rewriting_auth_json<'a>(
    f: &'a Fakes,
    auth: &'a Path,
) -> HookSpawner<'a, impl Fn() + Send + Sync> {
    HookSpawner {
        inner: &f.spawner,
        hook: move || {
            f.fs.add_file(auth, r#"{"auth_mode":"fixture","renewed":true}"#, now());
        },
    }
}

#[test]
fn a_rollout_event_younger_than_ten_minutes_is_used_without_starting_the_app_server() {
    let f = fakes();
    with_rollout(&f);
    f.spawner.script(script(WEEKLY_ONLY));
    let p = provider();

    let budget = at(&f, "2026-10-10T10:39:59Z");
    let snap = p.fetch(&f.context(budget), &account()).expect("rollout");
    assert_eq!(snap.source, SourceKind::CodexRollout);
    assert_eq!(snap.health, Health::Ok, "a fresh event is not cached data");
    assert_eq!(snap.measured_at, ts(EVENT_AT));
    assert_eq!(snap.fetched_at, ts("2026-10-10T10:39:59Z"));
    assert_close(snap.meters[0].used, 0.42);
    assert!(f.spawner.spawned().is_empty(), "no process started");

    // Ten minutes after the event it is no longer fresh: the app-server is asked.
    let budget = at(&f, "2026-10-10T10:40:01Z");
    let snap = p.fetch(&f.context(budget), &account()).expect("app-server");
    assert_eq!(snap.source, SourceKind::CodexAppServer);
    assert_eq!(snap.account.plan.as_deref(), Some("Pro Lite"));
    assert_eq!(f.spawner.spawned().len(), 1);
}

#[test]
fn a_fresh_rollout_is_not_read_when_the_rollout_is_off() {
    let f = fakes();
    with_rollout(&f);
    f.spawner.script(script(RATE_LIMITS));
    let budget = at(&f, "2026-10-10T10:31:00Z");
    let snap = provider_without_fallback()
        .fetch(&f.context(budget), &account())
        .expect("app-server");
    assert_eq!(snap.source, SourceKind::CodexAppServer);
    assert_eq!(f.spawner.spawned().len(), 1);
}

#[test]
fn freshness_is_judged_from_the_event_time_with_a_minute_of_tolerance_ahead() {
    let event = CodexUsage {
        measured_at: Some(ts("2026-10-10T10:30:00Z")),
        ..usage(Some(window(10.0, Some(300))), None)
    };
    let age_at = |now: &str| codex::fresh_age(&event, ts(now));
    assert_eq!(age_at("2026-10-10T10:30:00Z"), Some(SignedDuration::ZERO));
    assert_eq!(
        age_at("2026-10-10T10:39:59Z"),
        Some(SignedDuration::from_secs(599))
    );
    assert_eq!(
        age_at("2026-10-10T10:40:00Z"),
        None,
        "ten minutes is not younger than ten minutes"
    );
    assert_eq!(
        age_at("2026-10-10T10:29:00Z"),
        Some(SignedDuration::ZERO),
        "up to a minute ahead of the clock counts as now"
    );
    assert_eq!(
        age_at("2026-10-10T10:28:59Z"),
        None,
        "further ahead is not trusted to be fresh"
    );
    let undated = CodexUsage {
        measured_at: None,
        ..event.clone()
    };
    assert_eq!(codex::fresh_age(&undated, ts("2026-10-10T10:30:00Z")), None);
    assert_eq!(codex::ROLLOUT_FRESH, SignedDuration::from_mins(10));
    assert_eq!(codex::APP_SERVER_PACE, Duration::from_secs(15 * 60));
}

#[test]
fn the_app_server_starts_at_most_every_fifteen_minutes_and_its_answer_stands_in() {
    let f = fakes();
    // An older rollout event (10:30:01, 12 minutes before the first fetch): never fresh here,
    // and older than the app-server's answer.
    with_rollout(&f);
    f.spawner
        .script(script(RATE_LIMITS))
        .script(script(WEEKLY_ONLY));
    let p = provider();
    let first = p
        .fetch(&f.context(budget()), &account())
        .expect("first start");
    assert_eq!(first.source, SourceKind::CodexAppServer);
    assert_eq!(f.spawner.spawned().len(), 1);

    for later in [
        SignedDuration::from_mins(5),
        SignedDuration::from_secs(14 * 60 + 29),
    ] {
        f.clock.set(now() + later);
        let snap = p
            .clone()
            .fetch(&f.context(budget()), &account())
            .expect("stand-in");
        assert_eq!(snap.source, SourceKind::CodexAppServer, "{later:?}");
        assert_eq!(snap.measured_at, now(), "the answer keeps its own time");
        assert_eq!(snap.fetched_at, now() + later);
        assert_eq!(snap.health, Health::Ok);
        assert_eq!(snap.meters, first.meters);
        assert_eq!(snap.account.plan.as_deref(), Some("Plus"));
        assert_eq!(f.spawner.spawned().len(), 1, "not started at {later:?}");
    }

    // 15 minutes after the first fetch began, less the 30 s slack: started again.
    let second = now() + SignedDuration::from_secs(14 * 60 + 30);
    f.clock.set(second);
    let snap = p
        .fetch(&f.context(budget()), &account())
        .expect("second start");
    assert_eq!(f.spawner.spawned().len(), 2);
    assert_eq!(snap.measured_at, second);
    assert_eq!(
        snap.account.plan.as_deref(),
        Some("Pro Lite"),
        "a new answer"
    );
}

#[test]
fn while_the_app_server_rests_a_newer_rollout_event_wins() {
    let f = fakes();
    f.spawner.script(script(RATE_LIMITS));
    let p = provider();
    let budget = at(&f, "2026-10-10T10:29:00Z");
    p.fetch(&f.context(budget), &account()).expect("start");

    // Codex was used at 10:30:01. At 10:41 that event is 11 minutes old (not fresh) but newer
    // than the app-server's answer from 10:29, which rests until 10:43:30.
    with_rollout(&f);
    let budget = at(&f, "2026-10-10T10:41:00Z");
    let snap = p.fetch(&f.context(budget), &account()).expect("rollout");
    assert_eq!(snap.source, SourceKind::CodexRollout);
    assert_eq!(snap.measured_at, ts(EVENT_AT));
    assert_eq!(snap.health, Health::Stale, "cached values, not live");
    assert_eq!(f.spawner.spawned().len(), 1);
}

#[test]
fn the_pace_is_per_codex_folder_shared_by_accounts_and_clones() {
    let f = fakes();
    let work_home = PathBuf::from(r"C:\Users\tester\.codex-work");
    f.fs.add_file(
        work_home.join("config.toml"),
        "model = \"gpt-fixture\"\n",
        now(),
    );
    f.spawner
        .script(script(RATE_LIMITS))
        .script(script(WEEKLY_ONLY));
    let p = provider();
    p.fetch(&f.context(budget()), &account()).expect("home");

    // Another account entry on the same folder, through a clone: it rests.
    let alias = AccountSource::Codex {
        account: AccountKey("alias".to_owned()),
        codex_home: codex_home(),
    };
    f.clock.set(now() + SignedDuration::from_mins(1));
    let snap = p
        .clone()
        .fetch(&f.context(budget()), &alias)
        .expect("stand-in");
    assert_eq!(snap.account.key, AccountKey("alias".to_owned()));
    assert_eq!(snap.source, SourceKind::CodexAppServer);
    assert_eq!(f.spawner.spawned().len(), 1);

    // Another folder keeps its own pace.
    let work = AccountSource::Codex {
        account: AccountKey("work".to_owned()),
        codex_home: work_home.clone(),
    };
    let snap = p.fetch(&f.context(budget()), &work).expect("work");
    assert_eq!(snap.account.plan.as_deref(), Some("Pro Lite"));
    let spawned = f.spawner.spawned();
    assert_eq!(spawned.len(), 2);
    assert_eq!(spawned[1].cwd.as_deref(), Some(work_home.as_path()));
}

#[test]
fn a_new_sign_in_lifts_the_pace() {
    let f = fakes();
    let auth = codex_home().join("auth.json");
    f.fs.add_file(
        &auth,
        r#"{"auth_mode":"fixture"}"#,
        now() - SignedDuration::from_hours(24),
    );
    f.spawner
        .script(script(NOT_SIGNED_IN))
        .script(script(RATE_LIMITS));
    let p = provider_without_fallback();
    let err = p
        .fetch(&f.context(budget()), &account())
        .expect_err("signed out");
    assert!(matches!(err, FetchError::Auth { .. }), "{err:?}");

    // Two minutes later the app-server rests: the same sign-in error, nothing started.
    f.clock.set(now() + SignedDuration::from_mins(2));
    let again = p
        .fetch(&f.context(budget()), &account())
        .expect_err("resting");
    assert_eq!(again, err);
    assert_eq!(f.spawner.spawned().len(), 1);

    // The user signs in again (auth.json rewritten): started at once.
    f.fs.add_file(
        &auth,
        r#"{"auth_mode":"fixture","renewed":true}"#,
        now() + SignedDuration::from_mins(3),
    );
    f.clock.set(now() + SignedDuration::from_mins(4));
    let snap = p
        .fetch(&f.context(budget()), &account())
        .expect("signed in");
    assert_eq!(snap.source, SourceKind::CodexAppServer);
    assert_eq!(f.spawner.spawned().len(), 2);
}

#[test]
fn the_app_servers_own_sign_in_renewal_does_not_lift_the_pace() {
    let f = fakes();
    let auth = with_auth_json(&f);
    let before = provider().credential_fingerprint(&f.context(budget()), &account());
    f.spawner
        .script(script(RATE_LIMITS))
        .script(script(WEEKLY_ONLY));
    let p = provider_without_fallback();

    // The app-server renews Codex's sign-in while it runs and answers.
    let renewing = rewriting_auth_json(&f, &auth);
    let first = p
        .fetch(&context_with(&f, &renewing), &account())
        .expect("start");
    assert_eq!(first.account.plan.as_deref(), Some("Plus"));
    let after = p.credential_fingerprint(&f.context(budget()), &account());
    assert_ne!(after, before, "auth.json was rewritten");

    // Five minutes later the renewal is not taken for a new sign-in: the answer stands in.
    f.clock.set(now() + SignedDuration::from_mins(5));
    let snap = p.fetch(&f.context(budget()), &account()).expect("stand-in");
    assert_eq!(snap.measured_at, now());
    assert_eq!(snap.account.plan.as_deref(), Some("Plus"));
    assert_eq!(f.spawner.spawned().len(), 1);
}

#[test]
fn a_sign_in_while_a_signed_out_start_runs_lifts_the_pace() {
    let f = fakes();
    let auth = with_auth_json(&f);
    f.spawner
        .script(script(NOT_SIGNED_IN))
        .script(script(RATE_LIMITS));
    let p = provider_without_fallback();

    // auth.json changes while the app-server answers "not signed in": the user signing in.
    let signing_in = rewriting_auth_json(&f, &auth);
    let err = p
        .fetch(&context_with(&f, &signing_in), &account())
        .expect_err("signed out");
    assert!(matches!(err, FetchError::Auth { .. }), "{err:?}");

    // A minute later the changed file is a new sign-in: started at once.
    f.clock.set(now() + SignedDuration::from_mins(1));
    let snap = p
        .fetch(&f.context(budget()), &account())
        .expect("signed in");
    assert_eq!(snap.source, SourceKind::CodexAppServer);
    assert_eq!(f.spawner.spawned().len(), 2);
}

#[test]
fn a_fetch_while_the_app_server_of_its_folder_runs_does_not_start_another() {
    let f = fakes();
    f.spawner
        .script(script(RATE_LIMITS))
        .script(script(WEEKLY_ONLY));
    let p = provider_without_fallback();
    let alias = AccountSource::Codex {
        account: AccountKey("alias".to_owned()),
        codex_home: codex_home(),
    };

    // While the first start runs, the other account on the same folder is fetched.
    let during = Mutex::new(None);
    let concurrent = HookSpawner {
        inner: &f.spawner,
        hook: || {
            let result = p.clone().fetch(&f.context(budget()), &alias);
            *during.lock().expect("lock") = Some(result);
        },
    };
    let first = p
        .fetch(&context_with(&f, &concurrent), &account())
        .expect("start");
    assert_eq!(first.account.plan.as_deref(), Some("Plus"));

    let during = during
        .into_inner()
        .expect("lock")
        .expect("fetched during the start");
    let err = during.expect_err("nothing to stand in yet");
    assert!(
        matches!(
            &err,
            FetchError::Transient {
                from: SourceKind::CodexAppServer,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(f.spawner.spawned().len(), 1, "one app-server per folder");

    // Afterwards the other account gets the first start's answer.
    f.clock.set(now() + SignedDuration::from_mins(1));
    let snap = p.fetch(&f.context(budget()), &alias).expect("stand-in");
    assert_eq!(snap.account.plan.as_deref(), Some("Plus"));
    assert_eq!(f.spawner.spawned().len(), 1);
}

#[test]
fn a_resting_sign_in_error_still_shows_the_rollout_marked_sign_in_required() {
    let f = fakes();
    with_rollout(&f);
    f.spawner.script(script(NOT_SIGNED_IN));
    let p = provider();
    let first = p.fetch(&f.context(budget()), &account()).expect("rollout");
    assert_eq!(first.health, Health::SignInRequired);

    f.clock.set(now() + SignedDuration::from_mins(5));
    let snap = p.fetch(&f.context(budget()), &account()).expect("rollout");
    assert_eq!(snap.source, SourceKind::CodexRollout);
    assert_eq!(snap.health, Health::SignInRequired, "as after the start");
    assert_eq!(f.spawner.spawned().len(), 1);
}

#[test]
fn a_failed_start_counts_toward_the_pace_but_a_missing_codex_exe_does_not() {
    let mut f = Fakes::new(now());
    f.env = f.env.clone().with_var("LOCALAPPDATA", LOCAL_APP_DATA);
    f.fs.add_file(codex_home().join("config.toml"), "model = \"x\"\n", now());
    let p = provider_without_fallback();
    let err = p
        .fetch(&f.context(budget()), &account())
        .expect_err("no exe");
    assert_eq!(err.detail(), "codex.exe not found");

    // Codex is installed a minute later: nothing was started before, so it starts now.
    f.fs.add_file(bundled_exe("a1b2c3d4e5f60718"), b"MZ".to_vec(), now());
    f.spawner.script(vec![FakeLine::Timeout]);
    f.clock.set(now() + SignedDuration::from_mins(1));
    let failed = p
        .fetch(&f.context(budget()), &account())
        .expect_err("timeout");
    assert_eq!(failed.detail(), "codex app-server timed out");
    assert_eq!(f.spawner.spawned().len(), 1);

    // The failed start counts: five minutes later its error stands in, nothing is started.
    // Both ask the scheduler to come back when the rest ends (15 min less 30 s after 1 min).
    assert_eq!(
        failed.retry_after(),
        Some(Duration::from_secs(14 * 60 + 30))
    );
    f.clock.set(now() + SignedDuration::from_mins(6));
    let again = p
        .fetch(&f.context(budget()), &account())
        .expect_err("resting");
    assert_eq!(again.detail(), failed.detail());
    assert_eq!(again.retry_after(), Some(Duration::from_secs(9 * 60 + 30)));
    assert_eq!(f.spawner.spawned().len(), 1);
}

/// Fails its first spawn as Windows does for an executable being replaced, then spawns like
/// `FakeSpawner`.
struct FailingFirstSpawn<'a> {
    inner: &'a FakeSpawner,
    failed: Mutex<bool>,
}

impl Spawner for FailingFirstSpawn<'_> {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Box<dyn ChildIo>> {
        let mut failed = self.failed.lock().expect("lock");
        if !*failed {
            *failed = true;
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "fixture: codex.exe is being replaced",
            ));
        }
        self.inner.spawn(spec)
    }
}

#[test]
fn a_process_that_could_not_be_started_does_not_count_toward_the_pace() {
    let f = fakes();
    f.spawner.script(script(RATE_LIMITS));
    let p = provider_without_fallback();
    let failing = FailingFirstSpawn {
        inner: &f.spawner,
        failed: Mutex::new(false),
    };
    let err = p
        .fetch(&context_with(&f, &failing), &account())
        .expect_err("spawn failed");
    assert_eq!(err.detail(), "could not start codex app-server");
    assert!(f.spawner.spawned().is_empty());

    // A minute later (Codex's update done) it is started at once, not held for 15 minutes.
    f.clock.set(now() + SignedDuration::from_mins(1));
    let snap = p
        .fetch(&context_with(&f, &failing), &account())
        .expect("started");
    assert_eq!(snap.source, SourceKind::CodexAppServer);
    assert_eq!(snap.measured_at, now() + SignedDuration::from_mins(1));
    assert_eq!(f.spawner.spawned().len(), 1);
}

#[test]
fn a_failed_start_is_one_failure_for_the_back_off() {
    use lastdrop_core::refresh::{Due, PolicyInput, SourceState, next_due, record};
    use lastdrop_core::settings::RefreshInterval;

    let f = fakes();
    f.spawner
        .script(script(SERVER_ERROR))
        .script(script(RATE_LIMITS));
    let p = provider_without_fallback();
    let mut state = SourceState::default();
    let mut fetches = 0;
    // The scheduler's policy at the default 5-minute interval, for 15 minutes.
    loop {
        let due = next_due(&PolicyInput {
            now: f.clock.now(),
            min_interval: p.min_interval(),
            interval: RefreshInterval::Minutes(5),
            adaptive: false,
            activity: Default::default(),
            paused: false,
            resets: &[],
            jitter_unit: 0.5,
            credential: None,
            state: &state,
        });
        let Due::At(at) = due else { panic!("{due:?}") };
        if at > now() + SignedDuration::from_mins(15) {
            break;
        }
        f.clock.set(at.max(f.clock.now()));
        fetches += 1;
        let result = p.fetch(&f.context(budget()), &account());
        let attempt = match &result {
            Ok(_) => lastdrop_core::refresh::AttemptResult::Success,
            Err(e) => e.attempt_result(None),
        };
        record(&mut state, at, f.clock.now(), &attempt, p.min_interval());
        if fetches == 1 {
            assert_eq!(state.consecutive_failures, 1);
        }
    }
    // The 503 at 0:00, then the next start when the rest ends (14:30): no fetch in between
    // replays the failure and counts it again.
    assert_eq!(fetches, 2, "no stand-in failure fetched in between");
    assert_eq!(f.spawner.spawned().len(), 2);
    assert_eq!(state.consecutive_failures, 0, "the second start answered");
}

#[test]
fn a_manual_refresh_or_a_reset_since_the_last_start_lifts_the_rest() {
    let f = fakes();
    f.spawner
        .script(script(RATE_LIMITS))
        .script(script(WEEKLY_ONLY))
        .script(script(RATE_LIMITS));
    let p = provider_without_fallback();
    // RATE_LIMITS' 5-hour window resets at 12:56.
    let budget = at(&f, "2026-10-10T12:50:00Z");
    p.fetch(&f.context(budget), &account()).expect("start");
    let budget = at(&f, "2026-10-10T12:55:00Z");
    p.fetch(&f.context(budget), &account()).expect("stand-in");
    assert_eq!(f.spawner.spawned().len(), 1, "resting before the reset");

    // The scheduler's refresh after the reset (R§3.9): the stand-in is outdated, so it starts.
    let budget = at(&f, "2026-10-10T12:57:00Z");
    let snap = p
        .fetch(&f.context(budget), &account())
        .expect("after the reset");
    assert_eq!(f.spawner.spawned().len(), 2);
    assert_eq!(snap.measured_at, ts("2026-10-10T12:57:00Z"));
    assert_eq!(
        snap.account.plan.as_deref(),
        Some("Pro Lite"),
        "a new answer"
    );

    // Its weekly window resets in days: resting again.
    let budget = at(&f, "2026-10-10T12:59:00Z");
    let snap = p.fetch(&f.context(budget), &account()).expect("stand-in");
    assert_eq!(snap.measured_at, ts("2026-10-10T12:57:00Z"));
    assert_eq!(f.spawner.spawned().len(), 2);

    // Refresh now (F§3) starts it at once.
    let ctx = FetchContext {
        manual: true,
        ..f.context(budget)
    };
    let snap = p.fetch(&ctx, &account()).expect("manual");
    assert_eq!(f.spawner.spawned().len(), 3);
    assert_eq!(snap.measured_at, ts("2026-10-10T12:59:00Z"));
}

#[test]
fn a_manual_refresh_does_not_start_a_second_app_server_while_one_runs() {
    let f = fakes();
    f.spawner
        .script(script(RATE_LIMITS))
        .script(script(WEEKLY_ONLY));
    let p = provider_without_fallback();
    let alias = AccountSource::Codex {
        account: AccountKey("alias".to_owned()),
        codex_home: codex_home(),
    };
    let during = Mutex::new(None);
    let concurrent = HookSpawner {
        inner: &f.spawner,
        hook: || {
            let ctx = FetchContext {
                manual: true,
                ..f.context(budget())
            };
            *during.lock().expect("lock") = Some(p.clone().fetch(&ctx, &alias));
        },
    };
    p.fetch(&context_with(&f, &concurrent), &account())
        .expect("start");
    let during = during.into_inner().expect("lock").expect("fetched");
    assert!(during.is_err(), "nothing to stand in yet");
    assert_eq!(f.spawner.spawned().len(), 1);
}

/// The time the app-server takes to answer in [`run_schedule`].
const ANSWER_TAKES: SignedDuration = SignedDuration::from_secs(5);

/// Three hours of the scheduler's policy (`lastdrop_core::refresh`, adaptive off) against the
/// provider at a poll interval of `minutes`, every start answering after [`ANSWER_TAKES`]: the
/// times the app-server started, and the first moment the service was stale with `pace` as its
/// reading pace (judged when each fetch's answer can arrive at the latest, the oldest the
/// reading gets).
fn run_schedule(minutes: u16, pace: Option<SignedDuration>) -> (Vec<Timestamp>, Option<Timestamp>) {
    use lastdrop_core::refresh::{
        Activity, AttemptResult, Due, PolicyInput, SourceState, effective_interval, next_due,
        record,
    };
    use lastdrop_core::settings::RefreshInterval;
    use lastdrop_core::stats::{Freshness, ServiceStatus, freshness};

    let f = fakes();
    let p = provider();
    let answering = HookSpawner {
        inner: &f.spawner,
        hook: || {
            f.spawner.script(script(RATE_LIMITS));
            f.clock.advance(ANSWER_TAKES);
        },
    };
    let interval = RefreshInterval::Minutes(minutes);
    let mut state = SourceState::default();
    let mut status = ServiceStatus {
        poll_interval: effective_interval(interval, false, Activity::default(), p.min_interval()),
        reading_pace: pace,
        ..ServiceStatus::default()
    };
    let mut resets: Vec<Timestamp> = Vec::new();
    let mut starts = Vec::new();
    let mut stale = None;
    let end = now() + SignedDuration::from_hours(3);
    loop {
        let due = next_due(&PolicyInput {
            now: f.clock.now(),
            min_interval: p.min_interval(),
            interval,
            adaptive: false,
            activity: Activity::default(),
            paused: false,
            resets: &resets,
            jitter_unit: 0.5,
            credential: None,
            state: &state,
        });
        let Due::At(at) = due else {
            panic!("{minutes} min: {due:?}")
        };
        let at = at.max(f.clock.now());
        if at > end {
            break;
        }
        f.clock.set(at);
        let latest = at + ANSWER_TAKES;
        if stale.is_none() && freshness(latest, &status) != Freshness::Fresh {
            stale = Some(latest);
        }
        let spawned = f.spawner.spawned().len();
        let snap = p
            .fetch(&context_with(&f, &answering), &account())
            .unwrap_or_else(|e| panic!("{minutes} min at {at}: {e:?}"));
        if f.spawner.spawned().len() > spawned {
            starts.push(at);
        }
        record(
            &mut state,
            at,
            f.clock.now(),
            &AttemptResult::Success,
            p.min_interval(),
        );
        status.last_success = Some(
            status
                .last_success
                .map_or(snap.measured_at, |t| t.max(snap.measured_at)),
        );
        resets = snap.meters.iter().filter_map(|m| m.resets_at).collect();
    }
    (starts, stale)
}

#[test]
fn at_any_poll_interval_the_paced_app_server_keeps_the_service_fresh() {
    let pace = SignedDuration::try_from(codex::APP_SERVER_PACE).expect("pace");
    assert_eq!(provider().reading_pace(), Some(codex::APP_SERVER_PACE));
    // RATE_LIMITS' 5-hour window resets at 12:56, inside the three hours: one early start.
    let reset = ts("2026-10-10T12:56:00Z");
    for minutes in 1..=30 {
        let (starts, stale) = run_schedule(minutes, Some(pace));
        assert_eq!(stale, None, "every {minutes} min");
        assert!(starts.len() >= 2, "every {minutes} min: {starts:?}");
        for pair in starts.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            let after_reset = pair[0] < reset && reset <= pair[1];
            assert!(
                after_reset || gap >= SignedDuration::from_secs(14 * 60 + 30),
                "every {minutes} min: started {gap:?} apart at {}",
                pair[1]
            );
        }
    }
    // The spec's rule alone (no pace): polled every 7 minutes, the stand-in outlives 15 minutes.
    let (_, stale) = run_schedule(7, None);
    assert!(stale.is_some(), "the case the pace extension is for");
}

#[test]
fn unchanged_rollout_files_are_not_read_again() {
    let f = fakes();
    // An event 12 minutes old: not fresh, so the app-server starts and then rests.
    let path = add_rollout(
        &f,
        r"2026\10\10",
        "rollout-2026-10-10T10-20-00-a.jsonl",
        ROLLOUT_TWO_WINDOWS,
        ts("2026-10-10T10:30:03Z"),
    );
    f.spawner.script(script(RATE_LIMITS));
    let p = provider();
    p.fetch(&f.context(budget()), &account()).expect("start");
    assert_eq!(f.fs.tail_reads(), std::slice::from_ref(&path));

    // Resting fetches with the session file unchanged: no read; the result is the same.
    let mut seen = rollout::Seen::default();
    let first = rollout::fetch_seen(&f.context(budget()), &codex_home(), &mut seen);
    assert_eq!(f.fs.tail_reads().len(), 2);
    for minutes in [1, 5, 10] {
        f.clock.set(now() + SignedDuration::from_mins(minutes));
        let snap = p.fetch(&f.context(budget()), &account()).expect("stand-in");
        assert_eq!(snap.source, SourceKind::CodexAppServer);
        let again = rollout::fetch_seen(&f.context(budget()), &codex_home(), &mut seen);
        assert_eq!(again, first);
    }
    assert_eq!(f.fs.tail_reads().len(), 2, "no tail read again");

    // Codex writes to the session: read again, and its newer event wins over the stand-in.
    let newer = ROLLOUT_TWO_WINDOWS.replace("2026-10-10T10:30:01", "2026-10-10T10:51:00");
    f.fs.add_file(&path, newer, ts("2026-10-10T10:51:01Z"));
    let snap = p.fetch(&f.context(budget()), &account()).expect("rollout");
    assert_eq!(f.fs.tail_reads().len(), 3);
    assert_eq!(snap.source, SourceKind::CodexRollout);
    assert_eq!(snap.measured_at, ts("2026-10-10T10:51:00Z"));
}

#[test]
fn a_clock_set_back_does_not_hold_the_app_server_back() {
    let f = fakes();
    f.spawner
        .script(script(RATE_LIMITS))
        .script(script(RATE_LIMITS));
    let p = provider();
    p.fetch(&f.context(budget()), &account()).expect("first");
    f.clock.set(now() - SignedDuration::from_hours(1));
    p.fetch(&f.context(budget()), &account()).expect("second");
    assert_eq!(f.spawner.spawned().len(), 2);
}

// ---- Provider odds and ends ---------------------------------------------------------------------

#[test]
fn provider_basics() {
    let p = provider();
    assert_eq!(p.id(), ProviderId::Codex);
    assert_eq!(p.min_interval(), Duration::from_secs(60));
    assert_eq!(p.usage_page(), Some(codex::USAGE_PAGE));
    assert!(p.options().rollout_fallback);
}

#[test]
fn the_credential_fingerprint_follows_auth_json_metadata_only() {
    let f = fakes();
    let ctx = f.context(budget());
    let absent = provider().credential_fingerprint(&ctx, &account());
    assert_eq!(
        absent,
        Some(CredentialFingerprint("absent".to_owned())),
        "signed out: a fingerprint all the same, so signing in lifts an auth stop"
    );

    let auth = codex_home().join("auth.json");
    f.fs.add_file(&auth, r#"{"auth_mode":"fixture"}"#, now());
    let first = provider()
        .credential_fingerprint(&ctx, &account())
        .expect("fingerprint");
    assert!(!first.0.contains("fixture"), "never the contents");
    assert_ne!(Some(first.clone()), absent, "signing in changes it");
    f.fs.add_file(
        &auth,
        r#"{"auth_mode":"fixture"}"#,
        now() + SignedDuration::from_secs(1),
    );
    let second = provider()
        .credential_fingerprint(&ctx, &account())
        .expect("fingerprint");
    assert_ne!(first, second, "a new sign-in changes it");

    f.fs.remove(&auth);
    f.fs.add_file(
        codex_home().join(r"secrets\codex_auth.age"),
        b"age-fixture".to_vec(),
        now(),
    );
    assert!(
        provider()
            .credential_fingerprint(&ctx, &account())
            .is_some(),
        "keyring storage"
    );
}

#[test]
fn sign_in_runs_codex_login_in_the_account_folder() {
    let p = provider();
    let cmd = p.sign_in_command(&account()).expect("command");
    assert_eq!(cmd.program, "codex");
    assert_eq!(cmd.args, ["login"]);
    assert_eq!(
        cmd.env,
        [("CODEX_HOME".into(), Some(codex_home().into_os_string()))]
    );

    // After a fetch it uses the codex.exe it found (the bundled one is not on PATH) …
    let f = fakes();
    f.spawner.script(script(RATE_LIMITS));
    p.clone()
        .fetch(&f.context(budget()), &account())
        .expect("fetch");
    assert_eq!(
        PathBuf::from(p.sign_in_command(&account()).expect("command").program),
        bundled_exe("a1b2c3d4e5f60718"),
        "clones share what was found"
    );

    // … and the configured one first.
    let custom = PathBuf::from(r"D:\tools\codex.exe");
    let configured = CodexProvider::new(CodexOptions {
        codex_exe: Some(custom.clone()),
        ..CodexOptions::default()
    });
    assert_eq!(
        PathBuf::from(
            configured
                .sign_in_command(&account())
                .expect("command")
                .program
        ),
        custom
    );
}

#[test]
fn fixtures_hold_no_real_looking_secrets() {
    let all = [
        RATE_LIMITS,
        WEEKLY_ONLY,
        NOT_SIGNED_IN,
        AUTH_ERROR,
        SERVER_ERROR,
        API_KEY,
        ROLLOUT_TWO_WINDOWS,
        ROLLOUT_CUT_FIRST_LINE,
        ROLLOUT_FIRST_LINE_ONLY,
        ROLLOUT_PLAN_NULL,
        ROLLOUT_RESETS_IN_SECONDS,
        ROLLOUT_FLAT,
        ROLLOUT_UNKNOWN_SHAPE,
        ROLLOUT_SESSION_START,
    ];
    for text in all {
        for (i, _) in text.match_indices('@') {
            assert!(
                text[i..].starts_with("@example.invalid"),
                "only invented addresses"
            );
        }
        assert!(!text.contains("eyJ"), "no JWTs");
        assert!(!text.contains("sk-"), "no API keys");
        assert!(!text.contains("\"accountId\":\""), "no account ids");
    }
}
