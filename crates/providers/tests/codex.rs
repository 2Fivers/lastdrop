//! The Codex provider against invented fixtures (tests/fixtures/codex/README.md): app-server
//! JSON-RPC transcripts through `FakeSpawner`, rollout tails through `FakeFs`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{
    AccountKey, AccountRef, Health, MeterId, ProviderId, SourceKind, WindowLen,
};
use lastdrop_core::settings::CodexOptions;
use lastdrop_providers::codex::{
    self, CodexProvider, CodexUsage, CodexWindow, app_server, meter, rollout, to_snapshot,
};
use lastdrop_providers::fakes::{FakeLine, Fakes};
use lastdrop_providers::io::CredentialName;
use lastdrop_providers::{AccountSource, FetchError, Provider};
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
            retry_after: None,
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
        )
        .with_var("ComSpec", r"C:\Windows\System32\cmd.exe");
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

    let spec = app_server::launch_spec(&ctx, &script_path, &codex_home());
    assert_eq!(spec.program, PathBuf::from(r"C:\Windows\System32\cmd.exe"));
    assert_eq!(
        spec.args,
        [
            "/d".into(),
            "/c".into(),
            script_path.clone().into_os_string(),
            "app-server".into(),
            "-c".into(),
            "features.plugins=false".into()
        ]
    );
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

#[test]
fn the_command_interpreter_falls_back_to_system_root() {
    let mut f = Fakes::new(now());
    f.env = f.env.clone().with_var("SystemRoot", r"C:\Windows");
    assert_eq!(
        app_server::command_interpreter(&f.context(budget())),
        PathBuf::from(r"C:\Windows\System32\cmd.exe")
    );
    let spec = app_server::launch_spec(
        &f.context(budget()),
        Path::new(r"C:\x\codex.exe"),
        &codex_home(),
    );
    assert_eq!(spec.program, PathBuf::from(r"C:\x\codex.exe"));
    assert_eq!(spec.args, ["app-server", "-c", "features.plugins=false"]);
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
    assert_eq!(provider().credential_fingerprint(&ctx, &account()), None);

    let auth = codex_home().join("auth.json");
    f.fs.add_file(&auth, r#"{"auth_mode":"fixture"}"#, now());
    let first = provider()
        .credential_fingerprint(&ctx, &account())
        .expect("fingerprint");
    assert!(!first.0.contains("fixture"), "never the contents");
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
