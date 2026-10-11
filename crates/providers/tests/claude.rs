//! The Claude provider and its OAuth, Desktop-history and `.claude.json` sources against invented
//! fixtures (tests/fixtures/claude/README.md); the Desktop cache source is in `desktop_cache.rs`.
//! No test touches the real file system or network.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::consts::UNDOCUMENTED_MIN_INTERVAL;
use lastdrop_core::model::{
    AccountKey, AccountRef, Health, MeterId, ProviderId, SourceKind, WindowLen,
};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::settings::ClaudeOptions;
use lastdrop_providers::claude::{
    self, ClaudeProvider, ClaudeUsage, ClaudeWindow, WindowReading, cache, desktop, oauth,
};
use lastdrop_providers::fakes::Fakes;
use lastdrop_providers::io::{CredentialName, HeaderValue, HttpError, HttpResponse, Method};
use lastdrop_providers::{AccountSource, FetchError, Provider, SecretString};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!("fixtures/claude/", $name))
    };
}

const TOKEN: &str = "test-token-not-real";

fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

/// The readout-spec §13 clock, 12:42 at UTC+2.
fn now() -> Timestamp {
    ts("2026-10-10T10:42:00Z")
}

fn mins(m: i64) -> SignedDuration {
    SignedDuration::from_mins(m)
}

fn config_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\.claude")
}

fn state_file() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\.claude.json")
}

fn desktop_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\AppData\Roaming\Claude")
}

fn account() -> AccountSource {
    AccountSource::Claude {
        account: AccountKey::primary(),
        config_dir: config_dir(),
        state_file: state_file(),
        desktop_dir: Some(desktop_dir()),
    }
}

fn credentials_path() -> PathBuf {
    oauth::credentials_path(&config_dir())
}

fn with_desktop(f: &Fakes, data: &str) {
    f.fs.add_file(
        desktop::path(&desktop_dir()),
        data,
        ts("2026-10-10T10:32:05Z"),
    );
}

fn with_credentials(f: &Fakes, data: &str, modified: Timestamp) {
    f.fs.add_file(credentials_path(), data, modified);
}

fn with_cache(f: &Fakes, data: &str) {
    f.fs.add_file(state_file(), data, ts("2026-10-10T10:40:00Z"));
}

fn respond(f: &Fakes, status: u16, body: &str, headers: &[(&str, &str)]) {
    let mut response = HttpResponse::new(status, body);
    response.headers = headers
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    f.http.respond(Method::Get, oauth::USAGE_URL, response);
}

fn provider() -> ClaudeProvider {
    ClaudeProvider::new(ClaudeOptions::default())
}

fn fetch(p: &ClaudeProvider, f: &Fakes) -> Result<lastdrop_core::model::Snapshot, FetchError> {
    p.fetch(&f.context(SignedDuration::from_secs(30)), &account())
}

fn ids(snapshot: &lastdrop_core::model::Snapshot) -> Vec<String> {
    snapshot.meters.iter().map(|m| m.id.0.clone()).collect()
}

fn window<'a>(usage: &'a ClaudeUsage, w: &ClaudeWindow) -> &'a WindowReading {
    usage
        .windows
        .iter()
        .find(|r| &r.window == w)
        .unwrap_or_else(|| panic!("no {w:?} in {usage:?}"))
}

fn fable() -> ClaudeWindow {
    ClaudeWindow::WeeklyModel("Fable".to_owned())
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// ---- Meter definitions (R§2) --------------------------------------------------------------------

#[test]
fn readings_map_to_the_r2_meter_definitions() {
    let five = claude::meter(
        &WindowReading {
            window: ClaudeWindow::FiveHour,
            used: 0.42,
            resets_at: Some(ts("2026-10-10T12:56:00Z")),
        },
        0,
    );
    assert_eq!(five.id, MeterId::new(MeterId::FIVE_HOUR));
    assert_eq!(
        (
            five.names.short_label.as_str(),
            five.names.short_name.as_str(),
            five.names.name.as_str()
        ),
        ("5h", "5-hour", "5-hour session")
    );
    assert_eq!(five.window, WindowLen::Fixed(SignedDuration::from_hours(5)));
    assert!(
        five.starts_on_use,
        "R§2: Claude's 5-hour session starts on use"
    );
    assert!(!five.limited);
    assert_eq!(five.resets_at, Some(ts("2026-10-10T12:56:00Z")));

    let weekly = claude::meter(
        &WindowReading {
            window: ClaudeWindow::Weekly,
            used: 1.2,
            resets_at: None,
        },
        1,
    );
    assert_eq!(weekly.id, MeterId::new(MeterId::WEEKLY));
    assert_eq!(weekly.names.name, "Weekly · all models");
    assert_eq!(
        weekly.window,
        WindowLen::Fixed(SignedDuration::from_hours(7 * 24))
    );
    assert!(!weekly.starts_on_use);
    assert!(close(weekly.used, 1.2), "used may exceed 1 (R§3.1)");
    assert_eq!(weekly.order, 1);

    let model = claude::meter(
        &WindowReading {
            window: fable(),
            used: -0.5,
            resets_at: None,
        },
        2,
    );
    assert_eq!(model.id, MeterId::weekly_model("fable"));
    assert_eq!(
        (
            model.names.short_label.as_str(),
            model.names.short_name.as_str(),
            model.names.name.as_str()
        ),
        ("Fable", "Fable weekly", "Weekly · Fable")
    );
    assert_eq!(
        model.window,
        WindowLen::Fixed(SignedDuration::from_hours(7 * 24))
    );
    assert!(!model.starts_on_use);
    assert_eq!(model.used, 0.0, "R§3.1: never negative");

    let long = claude::meter(
        &WindowReading {
            window: ClaudeWindow::WeeklyModel("Opus 4.5 Max".to_owned()),
            used: 0.1,
            resets_at: None,
        },
        3,
    );
    assert_eq!(long.id, MeterId::weekly_model("opus-4-5-max"));
    assert_eq!(long.names.short_label, "Opus…");
    assert!(
        long.names.short_label.chars().count() <= 5,
        "R§2: short_label ≤ 5 characters"
    );
    assert_eq!(long.names.name, "Weekly · Opus 4.5 Max");
}

#[test]
fn snapshots_order_meters_and_keep_the_reading_time() {
    let usage = ClaudeUsage {
        windows: vec![
            WindowReading {
                window: fable(),
                used: 0.2,
                resets_at: None,
            },
            WindowReading {
                window: ClaudeWindow::Weekly,
                used: 0.6,
                resets_at: None,
            },
            WindowReading {
                window: ClaudeWindow::FiveHour,
                used: 0.4,
                resets_at: None,
            },
            WindowReading {
                window: fable(),
                used: 0.9,
                resets_at: None,
            },
            WindowReading {
                window: ClaudeWindow::WeeklyModel("Opus".to_owned()),
                used: 0.1,
                resets_at: None,
            },
        ],
        plan: Some("Max".to_owned()),
        measured_at: Some(now() - mins(30)),
    };
    let snap = claude::to_snapshot(
        &usage,
        SourceKind::ClaudeCodeCache,
        AccountRef::primary(),
        now(),
    );
    assert_eq!(snap.provider, ProviderId::Claude);
    assert_eq!(
        ids(&snap),
        ["five_hour", "weekly", "weekly:fable", "weekly:opus"]
    );
    assert_eq!(
        snap.meters.iter().map(|m| m.order).collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    assert!(
        close(snap.meters[2].used, 0.2),
        "the first reading of a window is kept"
    );
    assert_eq!(snap.measured_at, now() - mins(30));
    assert_eq!(snap.fetched_at, now());
    assert_eq!(snap.account.plan.as_deref(), Some("Max"));
    assert_eq!(snap.health, Health::Ok);
    assert!(snap.balances.is_empty());

    let future = ClaudeUsage {
        measured_at: Some(now() + mins(60)),
        ..usage
    };
    let snap = claude::to_snapshot(
        &future,
        SourceKind::ClaudeDesktopHistory,
        AccountRef::primary(),
        now(),
    );
    assert_eq!(
        snap.measured_at,
        now(),
        "measured_at is never later than fetched_at"
    );
}

// ---- Source 2 parsing: the usage endpoint body -------------------------------------------------------

#[test]
fn usage_body_in_the_current_format_gives_three_windows() {
    let usage = oauth::parse(fixture!("usage-200-current.json").as_bytes()).expect("parse");
    assert_eq!(
        usage
            .windows
            .iter()
            .map(|w| w.window.clone())
            .collect::<Vec<_>>(),
        [ClaudeWindow::FiveHour, ClaudeWindow::Weekly, fable()],
        "extra usage, cloud credits, Claude Design, surface scopes and unknown keys are skipped"
    );
    let five = window(&usage, &ClaudeWindow::FiveHour);
    assert!(close(five.used, 0.42));
    assert_eq!(five.resets_at, Some(ts("2026-10-10T12:56:00.123456Z")));
    assert!(close(window(&usage, &ClaudeWindow::Weekly).used, 0.61));
    let model = window(&usage, &fable());
    assert!(close(model.used, 0.23));
    assert_eq!(model.resets_at, Some(ts("2026-10-12T07:00:00Z")));
    assert_eq!(usage.measured_at, None, "the body carries no time");
    assert_eq!(usage.plan, None);

    let snap = claude::to_snapshot(
        &usage,
        SourceKind::ClaudeOAuthUsage,
        AccountRef::primary(),
        now(),
    );
    assert_eq!(ids(&snap), ["five_hour", "weekly", "weekly:fable"]);
}

#[test]
fn usage_body_nulls_are_missing_never_zero() {
    let usage = oauth::parse(fixture!("usage-200-five-hour-null.json").as_bytes()).expect("parse");
    assert_eq!(
        usage.windows.len(),
        1,
        "five_hour: null is omitted, not 0 %"
    );
    let weekly = window(&usage, &ClaudeWindow::Weekly);
    assert!(close(weekly.used, 1.075), "over the limit stays over");

    let usage =
        oauth::parse(fixture!("usage-200-five-hour-null-reset.json").as_bytes()).expect("parse");
    let five = window(&usage, &ClaudeWindow::FiveHour);
    assert_eq!(
        (five.used, five.resets_at),
        (0.0, None),
        "between sessions: 0 % and no reset"
    );
    let model = window(&usage, &fable());
    assert_eq!(
        (model.used, model.resets_at),
        (0.0, None),
        "an unused per-model weekly"
    );
}

#[test]
fn usage_body_in_the_old_format_reads_the_per_model_fields() {
    let usage = oauth::parse(fixture!("usage-200-legacy-opus.json").as_bytes()).expect("parse");
    let opus = ClaudeWindow::WeeklyModel("Opus".to_owned());
    let sonnet = ClaudeWindow::WeeklyModel("Sonnet".to_owned());
    assert_eq!(
        usage
            .windows
            .iter()
            .map(|w| w.window.clone())
            .collect::<Vec<_>>(),
        [
            ClaudeWindow::FiveHour,
            ClaudeWindow::Weekly,
            opus.clone(),
            sonnet.clone()
        ]
    );
    assert!(close(window(&usage, &opus).used, 0.71));
    assert_eq!(window(&usage, &sonnet).resets_at, None);
    let snap = claude::to_snapshot(
        &usage,
        SourceKind::ClaudeOAuthUsage,
        AccountRef::primary(),
        now(),
    );
    assert_eq!(
        ids(&snap),
        ["five_hour", "weekly", "weekly:opus", "weekly:sonnet"]
    );
    assert_eq!(snap.meters[3].names.short_label, "Sonn…");
}

#[test]
fn usage_body_with_only_limits_falls_back_to_them() {
    let usage = oauth::parse(fixture!("usage-200-limits-only.json").as_bytes()).expect("parse");
    assert_eq!(
        usage
            .windows
            .iter()
            .map(|w| w.window.clone())
            .collect::<Vec<_>>(),
        [ClaudeWindow::FiveHour, ClaudeWindow::Weekly, fable()],
        "the generic \"All models\" scope is the weekly row, not a model"
    );
    assert!(close(window(&usage, &ClaudeWindow::FiveHour).used, 0.09));
    assert_eq!(
        window(&usage, &ClaudeWindow::Weekly).resets_at,
        Some(ts("2026-10-12T07:00:00Z"))
    );
    assert!(close(window(&usage, &fable()).used, 0.31));
}

#[test]
fn usage_body_without_windows_or_json_is_a_parse_error() {
    for body in [
        fixture!("usage-200-no-windows.json"),
        "{}",
        "[]",
        "not json",
        r#"{"five_hour": {"utilization": "secret-looking-text"}}"#,
    ] {
        let err = oauth::parse(body.as_bytes()).expect_err(body);
        assert!(
            matches!(
                err,
                FetchError::Parse {
                    from: SourceKind::ClaudeOAuthUsage,
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(
            !err.to_string().contains("secret-looking-text"),
            "no values in details: {err}"
        );
    }
}

// ---- Source 2: statuses, request, token --------------------------------------------------------------

#[test]
fn statuses_map_to_fetch_errors() {
    let with = |status: u16, body: &str, retry: Option<&str>| {
        let mut response = HttpResponse::new(status, body);
        if let Some(value) = retry {
            response
                .headers
                .push(("retry-after".to_owned(), value.to_owned()));
        }
        oauth::interpret(&response, now())
    };
    assert!(with(200, fixture!("usage-200-two-windows.json"), None).is_ok());
    assert_eq!(
        with(401, fixture!("usage-401.json"), None),
        Err(FetchError::Auth {
            from: SourceKind::ClaudeOAuthUsage
        })
    );
    assert_eq!(
        with(429, fixture!("usage-429.json"), Some("120")),
        Err(FetchError::RateLimited {
            from: SourceKind::ClaudeOAuthUsage,
            retry_after: Some(Duration::from_secs(120)),
        })
    );
    assert_eq!(
        with(403, "", Some("Sat, 10 Oct 2026 10:52:00 GMT")),
        Err(FetchError::RateLimited {
            from: SourceKind::ClaudeOAuthUsage,
            retry_after: Some(Duration::from_secs(600)),
        }),
        "Retry-After as an HTTP-date"
    );
    assert_eq!(
        with(429, "", Some("Sat, 10 Oct 2026 10:00:00 GMT")),
        Err(FetchError::RateLimited {
            from: SourceKind::ClaudeOAuthUsage,
            retry_after: Some(Duration::ZERO),
        }),
        "a date in the past is zero"
    );
    assert_eq!(
        with(429, "", Some("soon")),
        Err(FetchError::RateLimited {
            from: SourceKind::ClaudeOAuthUsage,
            retry_after: None
        })
    );
    assert_eq!(
        with(429, "", Some("99999999")),
        Err(FetchError::RateLimited {
            from: SourceKind::ClaudeOAuthUsage,
            retry_after: Some(lastdrop_providers::io::MAX_RETRY_AFTER),
        }),
        "capped"
    );
    assert_eq!(
        with(503, "", Some("30")),
        Err(FetchError::Transient {
            from: SourceKind::ClaudeOAuthUsage,
            detail: "HTTP 503".to_owned(),
            retry_after: Some(Duration::from_secs(30)),
        })
    );
    assert_eq!(
        with(404, "", None),
        Err(FetchError::Parse {
            from: SourceKind::ClaudeOAuthUsage,
            detail: "HTTP 404".to_owned(),
        })
    );
}

#[test]
fn the_request_carries_a_secret_bearer_and_the_beta_header() {
    let token = SecretString::new(TOKEN.to_owned());
    let request = oauth::request(&token, Duration::from_secs(7));
    assert_eq!(request.method, Method::Get);
    assert_eq!(request.url, "https://api.anthropic.com/api/oauth/usage");
    assert_eq!(request.timeout, Duration::from_secs(7));
    assert_eq!(request.body, None);
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    let auth = header("Authorization").expect("authorization");
    assert!(
        matches!(auth, HeaderValue::Secret(_)),
        "the bearer is a secret header"
    );
    assert_eq!(auth.as_str(), format!("Bearer {TOKEN}"));
    assert_eq!(
        header("anthropic-beta"),
        Some(HeaderValue::Plain("oauth-2025-04-20".to_owned()))
    );
    assert_eq!(
        header("Accept"),
        Some(HeaderValue::Plain("application/json".to_owned()))
    );
    assert!(
        !format!("{request:?}").contains(TOKEN),
        "Debug redacts the token"
    );
}

#[test]
fn the_token_is_read_only_when_usable() {
    let f = Fakes::new(now());
    let ctx = f.context(SignedDuration::from_secs(30));
    let read = |data: &str| {
        f.fs.add_file(credentials_path(), data, now());
        oauth::read_token(&ctx, &config_dir())
    };
    assert_eq!(
        read(fixture!("credentials-valid.json"))
            .expect("valid")
            .expose(),
        TOKEN
    );
    assert_eq!(
        read(fixture!("credentials-expired.json")),
        Err(FetchError::Auth {
            from: SourceKind::ClaudeOAuthUsage
        }),
        "expired: Claude Code renews it, Lastdrop never does"
    );
    for name in ["empty", "inference-only", "no-claude-sign-in"] {
        let data = match name {
            "empty" => fixture!("credentials-empty-token.json"),
            "inference-only" => fixture!("credentials-inference-only.json"),
            _ => fixture!("credentials-no-claude-sign-in.json"),
        };
        assert!(
            matches!(read(data), Err(FetchError::Unavailable { .. })),
            "{name}: nothing to use, nothing to sign in to"
        );
    }
    // A token that expires within the margin is not used.
    let soon =
        r#"{"claudeAiOauth": {"accessToken": "test-token-not-real", "expiresAt": 1791628950000}}"#;
    assert!(matches!(read(soon), Err(FetchError::Auth { .. })));
    // No expiry and no scopes: try it.
    assert!(read(r#"{"claudeAiOauth": {"accessToken": "test-token-not-real"}}"#).is_ok());
    // Broken JSON that contains the token: the error names a position, never the value.
    let broken = r#"{"claudeAiOauth": {"accessToken": "test-token-not-real", "refreshToken": "test-refresh-token-not-real""#;
    let err = read(broken).expect_err("broken");
    assert!(matches!(err, FetchError::Parse { .. }));
    assert!(!format!("{err} {err:?}").contains("not-real"), "{err:?}");

    f.fs.remove(&credentials_path());
    assert!(matches!(
        oauth::read_token(&ctx, &config_dir()),
        Err(FetchError::Unavailable { .. })
    ));
}

// ---- Source 1: Claude Desktop's history --------------------------------------------------------------

#[test]
fn desktop_history_gives_the_newest_sample_without_reset_times() {
    let usage =
        desktop::parse(fixture!("plan-usage-history-v2.json").as_bytes(), now()).expect("parse");
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T10:32:00Z")));
    assert_eq!(
        usage.windows,
        [
            WindowReading {
                window: ClaudeWindow::FiveHour,
                used: 0.37,
                resets_at: None
            },
            WindowReading {
                window: ClaudeWindow::Weekly,
                used: 0.58,
                resets_at: None
            },
        ],
        "xu (extra usage), oa, cw, om and op have no R§2 meter"
    );
    assert!(
        !format!("{usage:?}").contains("00000000-0000-4000-8000"),
        "the org id is not kept"
    );
}

#[test]
fn desktop_history_tolerates_messy_samples() {
    let usage =
        desktop::parse(fixture!("plan-usage-history-messy.json").as_bytes(), now()).expect("parse");
    assert_eq!(
        usage.measured_at,
        Some(ts("2026-10-10T10:32:00Z")),
        "unordered: the latest t wins"
    );
    assert_eq!(
        usage.windows,
        [
            WindowReading {
                window: ClaudeWindow::FiveHour,
                used: 0.375,
                resets_at: None
            },
            WindowReading {
                window: ClaudeWindow::Weekly,
                used: 0.58,
                resets_at: None
            },
            WindowReading {
                window: ClaudeWindow::WeeklyModel("Opus".to_owned()),
                used: 0.71,
                resets_at: None,
            },
        ],
        "a newer sample without R§2 windows, nulls, non-objects and an untimed sample are skipped"
    );

    let untimed = r#"{"samples": [{"u": {"fh": 12}}]}"#;
    let usage = desktop::parse(untimed.as_bytes(), now() - mins(1)).expect("untimed");
    assert_eq!(
        usage.measured_at,
        Some(now() - mins(1)),
        "fetched_at stands in"
    );

    let seconds = r#"{"samples": [{"t": 1791628320, "u": {"fh": 12, "sd": {"utilization": 30, "resets_at": "2026-10-12T07:00:00Z"}}}]}"#;
    let usage = desktop::parse(seconds.as_bytes(), now()).expect("seconds");
    assert_eq!(
        usage.measured_at,
        Some(ts("2026-10-10T10:32:00Z")),
        "epoch seconds work too"
    );
    assert_eq!(
        window(&usage, &ClaudeWindow::Weekly).resets_at,
        Some(ts("2026-10-12T07:00:00Z")),
        "a sample that does carry a reset time keeps it"
    );
}

#[test]
fn desktop_history_missing_empty_or_broken() {
    for data in [
        fixture!("plan-usage-history-empty.json"),
        r#"{"version": 2}"#,
        r#"{"samples": null}"#,
    ] {
        assert!(
            matches!(
                desktop::parse(data.as_bytes(), now()),
                Err(FetchError::Unavailable { .. })
            ),
            "{data}"
        );
    }
    for data in ["", "{\"samples\": [", r#"{"samples": 5}"#] {
        assert!(
            matches!(
                desktop::parse(data.as_bytes(), now()),
                Err(FetchError::Parse {
                    from: SourceKind::ClaudeDesktopHistory,
                    ..
                })
            ),
            "{data}"
        );
    }
    let f = Fakes::new(now());
    let ctx = f.context(SignedDuration::from_secs(30));
    assert!(matches!(
        desktop::read(&ctx, &desktop_dir()),
        Err(FetchError::Unavailable { .. })
    ));
    f.fs.add_file(
        desktop::path(&desktop_dir()),
        vec![b' '; (desktop::MAX_BYTES + 1) as usize],
        now(),
    );
    assert!(matches!(
        desktop::read(&ctx, &desktop_dir()),
        Err(FetchError::Parse { .. })
    ));
}

/// The Store build's private copy of `%APPDATA%\Claude` (tests' home is `C:\Users\tester`).
fn store_desktop_dir() -> PathBuf {
    PathBuf::from(
        r"C:\Users\tester\AppData\Local\Packages\Claude_testpublisher\LocalCache\Roaming\Claude",
    )
}

#[test]
fn desktop_history_of_the_store_build_is_found_in_its_package() {
    let f = Fakes::new(now());
    f.fs.add_file(
        desktop::path(&store_desktop_dir()),
        fixture!("plan-usage-history-v2.json"),
        ts("2026-10-10T10:32:05Z"),
    );
    // Another package whose name only starts alike is not Claude Desktop's.
    f.fs.add_file(
        r"C:\Users\tester\AppData\Local\Packages\ClaudeLike.App_x\LocalCache\Roaming\Claude\plan-usage-history.json",
        "{}",
        now(),
    );
    let ctx = f.context(SignedDuration::from_secs(30));
    assert_eq!(
        desktop::candidates(&ctx, &desktop_dir()),
        [desktop_dir(), store_desktop_dir()]
    );
    let usage = desktop::read(&ctx, &desktop_dir()).expect("read from the package");
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T10:32:00Z")));
    assert_eq!(window(&usage, &ClaudeWindow::Weekly).used, 0.58);
}

#[test]
fn desktop_history_takes_the_newer_of_appdata_and_the_package() {
    let f = Fakes::new(now());
    // An old file from an earlier install in %APPDATA%, the live one in the package.
    f.fs.add_file(
        desktop::path(&desktop_dir()),
        r#"{"samples": [{"t": 1790000000000, "u": {"fh": 1, "sd": 2}}]}"#,
        ts("2026-09-21T14:13:20Z"),
    );
    f.fs.add_file(
        desktop::path(&store_desktop_dir()),
        fixture!("plan-usage-history-v2.json"),
        ts("2026-10-10T10:32:05Z"),
    );
    let ctx = f.context(SignedDuration::from_secs(30));
    let usage = desktop::read(&ctx, &desktop_dir()).expect("read");
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T10:32:00Z")));

    // And the other way round.
    f.fs.add_file(
        desktop::path(&desktop_dir()),
        fixture!("plan-usage-history-v2.json"),
        now(),
    );
    f.fs.add_file(
        desktop::path(&store_desktop_dir()),
        r#"{"samples": [{"t": 1790000000000, "u": {"fh": 1, "sd": 2}}]}"#,
        ts("2026-09-21T14:13:20Z"),
    );
    let usage = desktop::read(&ctx, &desktop_dir()).expect("read");
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T10:32:00Z")));
}

// ---- Source 3: the cached usage in .claude.json --------------------------------------------------------

#[test]
fn cached_usage_is_streamed_and_only_usage_is_kept() {
    let usage = cache::parse(fixture!("claude-state.json").as_bytes(), Some(now())).expect("parse");
    assert_eq!(
        usage.measured_at,
        Some(ts("2026-10-10T07:42:00Z")),
        "fetchedAtMs, not the file time"
    );
    assert_eq!(
        usage.windows,
        [
            WindowReading {
                window: ClaudeWindow::FiveHour,
                used: 0.2,
                resets_at: Some(ts("2026-10-10T11:00:00Z")),
            },
            WindowReading {
                window: ClaudeWindow::Weekly,
                used: 0.55,
                resets_at: Some(ts("2026-10-12T07:00:00Z")),
            },
            WindowReading {
                window: fable(),
                used: 0.21,
                resets_at: Some(ts("2026-10-12T07:00:00Z"))
            },
        ]
    );
    let kept = format!("{usage:?}");
    for private in [
        "tester@example.invalid",
        "00000000-0000-4000-8000",
        "Invented Org",
        "alpha",
    ] {
        assert!(!kept.contains(private), "{private} kept: {kept}");
    }
}

#[test]
fn cached_usage_after_a_large_project_history() {
    // Project history before the cache, as in a long-used `.claude.json`.
    let mut data = String::from(r#"{"projects": {"#);
    for i in 0..20_000 {
        if i > 0 {
            data.push(',');
        }
        data.push_str(&format!(
            r#""C:/Users/tester/src/p{i}": {{"history": [{{"display": "invented prompt {i}"}}], "lastCost": {i}.5}}"#
        ));
    }
    data.push_str(r#"}, "cachedUsageUtilization": {"fetchedAtMs": 1791618120000, "utilization": {"five_hour": {"utilization": 5, "resets_at": null}}}}"#);
    assert!(data.len() > 1 << 20);
    let usage = cache::parse(data.as_bytes(), None).expect("parse");
    assert_eq!(usage.windows.len(), 1);
    assert_eq!(usage.measured_at, Some(ts("2026-10-10T07:42:00Z")));
}

#[test]
fn cached_usage_missing_flat_or_broken() {
    assert!(matches!(
        cache::parse(
            fixture!("claude-state-no-cache.json").as_bytes(),
            Some(now())
        ),
        Err(FetchError::Unavailable { .. })
    ));
    assert!(matches!(
        cache::parse(
            r#"{"cachedUsageUtilization": null}"#.as_bytes(),
            Some(now())
        ),
        Err(FetchError::Unavailable { .. })
    ));
    let flat = cache::parse(
        fixture!("claude-state-flat-cache.json").as_bytes(),
        Some(now() - mins(90)),
    )
    .expect("an older flat layout");
    assert_eq!(
        flat.measured_at,
        Some(now() - mins(90)),
        "no fetchedAtMs: the file time"
    );
    assert_eq!(flat.windows.len(), 2);
    assert!(matches!(
        cache::parse(
            r#"{"cachedUsageUtilization": {"utilization": {"extra_usage": {}}}}"#.as_bytes(),
            None
        ),
        Err(FetchError::Parse {
            from: SourceKind::ClaudeCodeCache,
            ..
        })
    ));
    assert!(matches!(
        cache::parse("{\"projects\": {".as_bytes(), None),
        Err(FetchError::Parse {
            from: SourceKind::ClaudeCodeCache,
            ..
        })
    ));

    let f = Fakes::new(now());
    let ctx = f.context(SignedDuration::from_secs(30));
    assert!(matches!(
        cache::read(&ctx, &state_file()),
        Err(FetchError::Unavailable { .. })
    ));
    with_cache(&f, fixture!("claude-state-flat-cache.json"));
    let read = cache::read(&ctx, &state_file()).expect("read");
    assert_eq!(read.measured_at, Some(ts("2026-10-10T10:40:00Z")));
}

// ---- The provider ----------------------------------------------------------------------------------------

#[test]
fn the_fresh_oauth_reading_wins_and_names_the_plan() {
    let f = Fakes::new(now());
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    with_cache(&f, fixture!("claude-state.json"));
    respond(&f, 200, fixture!("usage-200-current.json"), &[]);
    let snap = fetch(&provider(), &f).expect("fetch");
    assert_eq!(snap.source, SourceKind::ClaudeOAuthUsage);
    assert_eq!(snap.measured_at, now());
    assert_eq!(snap.health, Health::Ok);
    assert_eq!(snap.account.plan.as_deref(), Some("Max"));
    assert_eq!(snap.account.key, AccountKey::primary());
    assert_eq!(ids(&snap), ["five_hour", "weekly", "weekly:fable"]);
    assert_eq!(f.http.pending(), 0);
    let requests = f.http.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url, oauth::USAGE_URL);
    assert!(requests[0].timeout <= oauth::TIMEOUT);
    assert!(
        !format!("{requests:?} {snap:?}").contains("not-real"),
        "no secret in Debug"
    );
}

#[test]
fn a_401_falls_back_with_sign_in_required_until_the_token_file_changes() {
    let f = Fakes::new(now());
    let p = provider();
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    respond(&f, 401, fixture!("usage-401.json"), &[]);

    let snap = fetch(&p, &f).expect("the desktop history answers");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopHistory);
    assert_eq!(snap.health, Health::SignInRequired);
    assert_eq!(snap.account.plan.as_deref(), Some("Max"));
    assert_eq!(f.http.requests().len(), 1);

    f.clock.advance(mins(10));
    let snap = fetch(&p, &f).expect("still the desktop history");
    assert_eq!(snap.health, Health::SignInRequired, "the stop holds");
    assert_eq!(
        f.http.requests().len(),
        1,
        "no request with the rejected token"
    );

    // Claude Code renews the token: the file changes, the stop lifts.
    with_credentials(&f, fixture!("credentials-valid.json"), now() + mins(9));
    respond(&f, 200, fixture!("usage-200-two-windows.json"), &[]);
    f.clock.advance(mins(10));
    let snap = fetch(&p, &f).expect("OAuth again");
    assert_eq!(snap.source, SourceKind::ClaudeOAuthUsage);
    assert_eq!(snap.health, Health::Ok);
    assert_eq!(f.http.requests().len(), 2);
}

#[test]
fn a_401_with_no_other_source_is_an_auth_error_and_the_fingerprint_says_when_to_retry() {
    let f = Fakes::new(now());
    let p = provider();
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    respond(&f, 401, fixture!("usage-401.json"), &[]);
    let err = fetch(&p, &f).expect_err("nothing else answers");
    assert_eq!(
        err,
        FetchError::Auth {
            from: SourceKind::ClaudeOAuthUsage
        }
    );
    let before = p.credential_fingerprint(&f.context(SignedDuration::from_secs(30)), &account());
    assert!(before.is_some());

    f.clock.advance(mins(10));
    assert_eq!(
        fetch(&p, &f),
        Err(FetchError::Auth {
            from: SourceKind::ClaudeOAuthUsage
        })
    );
    assert_eq!(
        f.http.requests().len(),
        1,
        "a manual retry does not resend the rejected token"
    );

    with_credentials(&f, fixture!("credentials-valid.json"), now());
    let after = p.credential_fingerprint(&f.context(SignedDuration::from_secs(30)), &account());
    assert_ne!(
        before, after,
        "a renewed token file changes the fingerprint"
    );
}

#[test]
fn a_429_backs_off_with_retry_after_while_other_sources_answer() {
    let f = Fakes::new(now());
    let p = provider();
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    respond(
        &f,
        429,
        fixture!("usage-429.json"),
        &[("Retry-After", "1800")],
    );

    let snap = fetch(&p, &f).expect("desktop");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopHistory);
    assert_eq!(
        snap.health,
        Health::Ok,
        "a throttle is not a sign-in problem"
    );

    f.clock.advance(mins(6));
    let snap = fetch(&p, &f).expect("desktop");
    assert_eq!(f.http.requests().len(), 1, "Retry-After is honoured");
    assert_eq!(
        snap.health,
        Health::Stale,
        "a 16-minute-old history reading"
    );

    f.clock.advance(mins(25));
    respond(&f, 200, fixture!("usage-200-two-windows.json"), &[]);
    let snap = fetch(&p, &f).expect("OAuth again");
    assert_eq!(f.http.requests().len(), 2);
    assert_eq!(snap.source, SourceKind::ClaudeOAuthUsage);
}

#[test]
fn after_a_failed_request_the_last_good_answer_still_lends_its_reset_times() {
    let f = Fakes::new(now());
    let p = provider();
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    respond(&f, 200, fixture!("usage-200-two-windows.json"), &[]);
    let snap = fetch(&p, &f).expect("OAuth");
    assert_eq!(snap.source, SourceKind::ClaudeOAuthUsage);

    // Six minutes later the endpoint fails and a newer desktop-history sample (no reset times)
    // is the freshest reading: the reset times of the same windows come from the last good
    // OAuth answer, not from the failed request.
    f.clock.advance(mins(6));
    respond(&f, 503, "", &[]);
    with_desktop(
        &f,
        r#"{"version": 2, "samples": [{"t": 1791629220000, "u": {"fh": 45, "sd": 62}}]}"#,
    );
    let snap = fetch(&p, &f).expect("desktop");
    assert_eq!(f.http.requests().len(), 2);
    assert_eq!(snap.source, SourceKind::ClaudeDesktopHistory);
    assert_eq!(ids(&snap), ["five_hour", "weekly"]);
    assert_eq!(
        snap.meters[0].resets_at,
        Some(ts("2026-10-10T12:56:00.123456Z"))
    );
    assert_eq!(snap.meters[1].resets_at, Some(ts("2026-10-12T07:00:00Z")));
}

#[test]
fn a_429_with_no_other_source_is_rate_limited() {
    let f = Fakes::new(now());
    let p = provider();
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    respond(&f, 429, "", &[("Retry-After", "600")]);
    assert_eq!(
        fetch(&p, &f),
        Err(FetchError::RateLimited {
            from: SourceKind::ClaudeOAuthUsage,
            retry_after: Some(Duration::from_secs(600)),
        })
    );
    f.clock.advance(mins(5));
    assert!(
        matches!(fetch(&p, &f), Err(FetchError::RateLimited { .. })),
        "while backing off the last answer stands"
    );
    assert_eq!(f.http.requests().len(), 1);
}

#[test]
fn the_oauth_source_rests_five_minutes_and_reuses_its_last_answer() {
    let f = Fakes::new(now());
    let p = provider();
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    respond(&f, 200, fixture!("usage-200-two-windows.json"), &[]);
    respond(&f, 200, fixture!("usage-200-legacy-opus.json"), &[]);

    assert_eq!(fetch(&p, &f).expect("first").measured_at, now());
    f.clock.advance(mins(2));
    let snap = fetch(&p, &f).expect("second");
    assert_eq!(f.http.requests().len(), 1, "at most every 5 minutes");
    assert_eq!(snap.source, SourceKind::ClaudeOAuthUsage);
    assert_eq!(
        snap.measured_at,
        now(),
        "the reused reading keeps its own time"
    );

    f.clock.advance(mins(3));
    let snap = fetch(&p, &f).expect("third");
    assert_eq!(
        f.http.requests().len(),
        2,
        "5 minutes after the last request"
    );
    assert_eq!(snap.measured_at, now() + mins(5));
    assert_eq!(snap.meters.len(), 4);
    assert_eq!(p.min_interval(), UNDOCUMENTED_MIN_INTERVAL);
}

#[test]
fn an_expired_token_skips_the_request_without_asking_to_sign_in() {
    let f = Fakes::new(now());
    let p = provider();
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_credentials(&f, fixture!("credentials-expired.json"), now() - mins(200));
    let snap = fetch(&p, &f).expect("desktop");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopHistory);
    assert_eq!(snap.health, Health::Ok);
    assert_eq!(snap.account.plan.as_deref(), Some("Pro"));
    assert!(f.http.requests().is_empty(), "never refreshed, never sent");

    f.fs.remove(&desktop::path(&desktop_dir()));
    assert_eq!(
        fetch(&p, &f),
        Err(FetchError::Auth {
            from: SourceKind::ClaudeOAuthUsage
        }),
        "alone, an expired token says: open Claude Code"
    );
    assert!(f.http.requests().is_empty());
}

#[test]
fn a_token_kept_outside_the_file_skips_the_source_quietly() {
    // Claude Code hosted by Claude Desktop keeps the token in Credential Manager and leaves an
    // empty one in the file; Lastdrop does not read other tools' Credential Manager entries.
    let f = Fakes::new(now());
    let p = provider();
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_credentials(
        &f,
        fixture!("credentials-empty-token.json"),
        now() - mins(60),
    );
    let snap = fetch(&p, &f).expect("desktop");
    assert_eq!(
        (snap.source, snap.health),
        (SourceKind::ClaudeDesktopHistory, Health::Ok)
    );
    assert_eq!(snap.account.plan.as_deref(), Some("Max"));
    assert!(f.http.requests().is_empty());

    f.fs.remove(&desktop::path(&desktop_dir()));
    assert!(matches!(fetch(&p, &f), Err(FetchError::Unavailable { .. })));

    with_credentials(&f, fixture!("credentials-inference-only.json"), now());
    assert!(matches!(fetch(&p, &f), Err(FetchError::Unavailable { .. })));
    assert!(
        f.http.requests().is_empty(),
        "an inference-only token is not tried"
    );
}

#[test]
fn turned_off_sources_are_not_read() {
    let f = Fakes::new(now());
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    with_cache(&f, fixture!("claude-state.json"));
    let only_cache = ClaudeProvider::new(ClaudeOptions {
        desktop_history: false,
        oauth_usage: false,
        ..ClaudeOptions::default()
    });
    let snap = fetch(&only_cache, &f).expect("cache");
    assert_eq!(snap.source, SourceKind::ClaudeCodeCache);
    assert_eq!(
        snap.health,
        Health::Stale,
        "a three-hour-old cached reading"
    );
    assert_eq!(snap.account.plan, None, "the credentials file is not read");
    assert!(f.http.requests().is_empty());
    assert_eq!(ids(&snap), ["five_hour", "weekly", "weekly:fable"]);

    let none = ClaudeProvider::new(ClaudeOptions {
        desktop_history: false,
        oauth_usage: false,
        cached_usage: false,
        ..ClaudeOptions::default()
    });
    assert!(matches!(
        fetch(&none, &f),
        Err(FetchError::Unavailable { .. })
    ));
}

#[test]
fn the_freshest_reading_wins_and_borrows_reset_times_of_the_same_window() {
    let f = Fakes::new(now());
    let p = provider();
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_cache(&f, fixture!("claude-state.json"));
    let snap = fetch(&p, &f).expect("fetch");
    assert_eq!(
        snap.source,
        SourceKind::ClaudeDesktopHistory,
        "10:32 beats the cache's 07:42"
    );
    assert_eq!(snap.measured_at, ts("2026-10-10T10:32:00Z"));
    assert_eq!(ids(&snap), ["five_hour", "weekly"]);
    assert!(close(snap.meters[0].used, 0.37));
    assert_eq!(
        snap.meters[0].resets_at,
        Some(ts("2026-10-10T11:00:00Z")),
        "the cached session still runs at 10:32 and used less: the same window"
    );
    assert_eq!(snap.meters[1].resets_at, Some(ts("2026-10-12T07:00:00Z")));
    assert_eq!(snap.health, Health::Ok);

    // A cached session that used more than the history shows ended in between: no borrowing.
    let higher =
        fixture!("claude-state.json").replace(r#""utilization": 20.0"#, r#""utilization": 50.0"#);
    with_cache(&f, &higher);
    let snap = fetch(&p, &f).expect("fetch");
    assert_eq!(snap.meters[0].resets_at, None, "R§3.2: no reset time");
    assert_eq!(snap.meters[1].resets_at, Some(ts("2026-10-12T07:00:00Z")));

    // A cached session that ended before the history sample: no borrowing either.
    let ended = fixture!("claude-state.json").replace(
        "2026-10-10T11:00:00.000000+00:00",
        "2026-10-10T10:00:00+00:00",
    );
    with_cache(&f, &ended);
    assert_eq!(fetch(&p, &f).expect("fetch").meters[0].resets_at, None);

    // An older history sample loses to the cache.
    let old = fixture!("plan-usage-history-v2.json").replace("1791628320000", "1791600000000");
    let old = old
        .replace("1791627420000", "1791599000000")
        .replace("1791626520000", "1791598000000");
    with_desktop(&f, &old);
    with_cache(&f, fixture!("claude-state.json"));
    let snap = fetch(&p, &f).expect("fetch");
    assert_eq!(snap.source, SourceKind::ClaudeCodeCache);
    assert_eq!(snap.measured_at, ts("2026-10-10T07:42:00Z"));
}

#[test]
fn a_future_sample_time_is_clamped_to_the_fetch() {
    let f = Fakes::new(now());
    let future = fixture!("plan-usage-history-v2.json").replace("1791628320000", "1791640000000");
    with_desktop(&f, &future);
    let snap = fetch(&provider(), &f).expect("fetch");
    assert_eq!(snap.measured_at, snap.fetched_at);
    assert!(snap.measured_at <= now());
}

#[test]
fn transport_failures_and_the_deadline() {
    let f = Fakes::new(now());
    let p = provider();
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    f.http
        .fail(Method::Get, oauth::USAGE_URL, HttpError::Unreachable);
    assert_eq!(
        fetch(&p, &f),
        Err(FetchError::Offline {
            from: SourceKind::ClaudeOAuthUsage
        })
    );

    // A fetch whose deadline has passed sends nothing.
    let f = Fakes::new(now());
    with_desktop(&f, fixture!("plan-usage-history-v2.json"));
    with_credentials(&f, fixture!("credentials-valid.json"), now() - mins(60));
    let snap = provider()
        .fetch(&f.context(SignedDuration::ZERO), &account())
        .expect("desktop");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopHistory);
    assert!(f.http.requests().is_empty());
}

#[test]
fn the_most_telling_failure_is_reported() {
    let f = Fakes::new(now());
    with_desktop(&f, "{\"samples\": [");
    let err = fetch(&provider(), &f).expect_err("nothing readable");
    assert!(
        matches!(
            err,
            FetchError::Parse {
                from: SourceKind::ClaudeDesktopHistory,
                ..
            }
        ),
        "a broken history over missing files: {err:?}"
    );

    let f = Fakes::new(now());
    let err = fetch(&provider(), &f).expect_err("nothing at all");
    assert!(matches!(err, FetchError::Unavailable { .. }), "{err:?}");

    let not_installed = AccountSource::Claude {
        account: AccountKey::primary(),
        config_dir: config_dir(),
        state_file: state_file(),
        desktop_dir: None,
    };
    let err = provider()
        .fetch(&f.context(SignedDuration::from_secs(30)), &not_installed)
        .expect_err("nothing");
    assert!(matches!(err, FetchError::Unavailable { .. }));

    let meshy = AccountSource::Meshy {
        account: AccountKey::primary(),
        credential: CredentialName {
            provider: ProviderId::Meshy,
            account: AccountKey::primary(),
            name: "api_key".to_owned(),
        },
    };
    assert!(matches!(
        provider().fetch(&f.context(SignedDuration::from_secs(30)), &meshy),
        Err(FetchError::Unavailable { .. })
    ));
    assert_eq!(
        provider().credential_fingerprint(&f.context(SignedDuration::from_secs(30)), &meshy),
        None
    );
}

#[test]
fn the_fingerprint_follows_the_credentials_file() {
    let f = Fakes::new(now());
    let p = provider();
    let ctx = f.context(SignedDuration::from_secs(30));
    assert_eq!(
        p.credential_fingerprint(&ctx, &account()),
        Some(CredentialFingerprint("absent".to_owned())),
        "a missing file has a fingerprint, so its appearance lifts a stop"
    );
    with_credentials(&f, fixture!("credentials-valid.json"), now());
    let first = p.credential_fingerprint(&ctx, &account()).expect("some");
    with_credentials(&f, fixture!("credentials-valid.json"), now() + mins(1));
    let touched = p.credential_fingerprint(&ctx, &account()).expect("some");
    assert_ne!(first, touched);
    assert!(
        !touched.0.contains("not-real"),
        "a fingerprint never holds the secret"
    );
}

#[test]
fn sign_in_opens_claude_with_the_accounts_folder() {
    let p = provider();
    let default = p.sign_in_command(&account()).expect("command");
    assert_eq!(default.program, "claude");
    assert!(default.args.is_empty());
    assert_eq!(
        default.env,
        [(OsString::from("CLAUDE_CONFIG_DIR"), None)],
        "the default folder: the variable is removed, not set"
    );

    let custom_dir = Path::new(r"D:\claude-work");
    let custom = AccountSource::Claude {
        account: AccountKey("work".to_owned()),
        config_dir: custom_dir.to_path_buf(),
        state_file: custom_dir.join(".claude.json"),
        desktop_dir: None,
    };
    let command = p.sign_in_command(&custom).expect("command");
    assert_eq!(
        command.env,
        [(
            OsString::from("CLAUDE_CONFIG_DIR"),
            Some(custom_dir.as_os_str().to_owned())
        )]
    );
    assert_eq!(p.id(), ProviderId::Claude);
    assert_eq!(p.usage_page(), Some("https://claude.ai/settings/usage"));
}
