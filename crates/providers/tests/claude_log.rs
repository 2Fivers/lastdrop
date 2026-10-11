//! What the Claude provider logs: every source's outcome at INFO — why it was skipped, or what it
//! read and how old that is — and which reading is used, without secrets, ids or paths. Its own
//! test binary, because a process has one logger.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountKey, SourceKind};
use lastdrop_core::settings::ClaudeOptions;
use lastdrop_providers::claude::{ClaudeProvider, desktop_cache, oauth};
use lastdrop_providers::fakes::Fakes;
use lastdrop_providers::fakes::chromium_cache::{FakeChromiumCache, FakeEntry, FakeResponse};
use lastdrop_providers::io::{HttpResponse, Method};
use lastdrop_providers::{AccountSource, Provider};
use log::{Level, LevelFilter, Log, Metadata, Record};

const ORG: &str = "00000000-0000-4000-8000-000000000009";

/// Every record logged, with its level.
struct Capture(Mutex<Vec<(Level, String)>>);

impl Log for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

/// The records logged since the last call.
fn take() -> Vec<(Level, String)> {
    std::mem::take(&mut *CAPTURE.0.lock().unwrap_or_else(PoisonError::into_inner))
}

fn now() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("timestamp")
}

fn mins(m: i64) -> SignedDuration {
    SignedDuration::from_mins(m)
}

fn config_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\.claude")
}

fn desktop_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\AppData\Roaming\Claude")
}

fn account() -> AccountSource {
    AccountSource::Claude {
        account: AccountKey::primary(),
        config_dir: config_dir(),
        state_file: PathBuf::from(r"C:\Users\tester\.claude.json"),
        desktop_dir: Some(desktop_dir()),
    }
}

fn with_cached_usage(f: &Fakes, at: Timestamp, body: Vec<u8>) {
    let key = format!("1/0/https://claude.ai/api/organizations/{ORG}/usage?skip_spend=1");
    FakeChromiumCache::new()
        .with(FakeEntry::new(
            &format!("1/0/https://claude.ai/api/organizations/{ORG}/chat_conversations"),
            FakeResponse::ok(at, "identity").pickle(),
            b"[]".to_vec(),
            at,
        ))
        .with(FakeEntry::new(
            &key,
            FakeResponse::ok(at, "zstd").pickle(),
            body,
            at,
        ))
        .write(&f.fs, &desktop_cache::cache_dir(&desktop_dir()), at);
}

fn usage_zstd() -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(
        include_bytes!("fixtures/claude/usage-200-two-windows.json").as_slice(),
        ruzstd::encoding::CompressionLevel::Fastest,
    )
}

fn with_credentials(f: &Fakes, data: &str) {
    f.fs.add_file(
        oauth::credentials_path(&config_dir()),
        data,
        now() - mins(60),
    );
}

fn fetch(p: &ClaudeProvider, f: &Fakes) {
    // The outcome is tested elsewhere; here only what it logs.
    let _ = p.fetch(&f.context(SignedDuration::from_secs(30)), &account());
}

/// The INFO line about `source`, which must be there exactly once.
fn line_about<'a>(records: &'a [(Level, String)], about: &str) -> &'a str {
    let found: Vec<&(Level, String)> = records
        .iter()
        .filter(|(_, text)| text.starts_with(&format!("claude: {about}")))
        .collect();
    assert_eq!(found.len(), 1, "one line about {about}: {records:#?}");
    assert_eq!(found[0].0, Level::Info, "{about} at INFO");
    &found[0].1
}

fn assert_private(records: &[(Level, String)]) {
    for (_, text) in records {
        for private in [
            ORG,
            "not-real",
            "tester",
            "test-cookie",
            "example.invalid",
            "claude.ai/api",
            r"C:\",
        ] {
            assert!(!text.contains(private), "{private} logged: {text}");
        }
    }
}

#[test]
fn every_source_says_why_it_was_skipped_or_what_it_read() {
    log::set_logger(&CAPTURE).expect("the only logger of this test binary");
    log::set_max_level(LevelFilter::Trace);
    let p = ClaudeProvider::new(ClaudeOptions::default());

    // An expired token, Desktop's cache nine minutes old, no history, no `.claude.json`.
    let f = Fakes::new(now());
    with_credentials(&f, include_str!("fixtures/claude/credentials-expired.json"));
    with_cached_usage(&f, now() - mins(9), usage_zstd());
    fetch(&p, &f);
    let records = take();
    assert_private(&records);
    let oauth_line = line_about(&records, "ClaudeOAuthUsage:");
    assert!(oauth_line.contains("skipped"), "{oauth_line}");
    assert!(oauth_line.contains("token has expired"), "{oauth_line}");
    let cache_line = line_about(&records, "ClaudeDesktopCache:");
    assert!(
        cache_line.contains("read 2 windows measured 2026-10-10T10:33:00Z (9 min old)"),
        "{cache_line}"
    );
    assert!(
        cache_line.contains("content-encoding zstd, 1 usage response(s) among 2 entries"),
        "{cache_line}"
    );
    assert_eq!(
        line_about(&records, "ClaudeDesktopHistory:"),
        "claude: ClaudeDesktopHistory: skipped: plan-usage-history.json not found"
    );
    assert!(line_about(&records, "ClaudeCodeCache:").contains("skipped: "));
    assert_eq!(
        line_about(&records, "using"),
        "claude: using ClaudeDesktopCache, measured 2026-10-10T10:33:00Z (9 min old), 2 meters, \
         health Ok"
    );

    // No token in the file (Claude Code inside Claude Desktop), and a cached body that does not
    // decode.
    let f = Fakes::new(now());
    with_credentials(
        &f,
        include_str!("fixtures/claude/credentials-empty-token.json"),
    );
    let mut broken = usage_zstd();
    let last = broken.len() - 1;
    broken[last] ^= 0xFF;
    with_cached_usage(&f, now() - mins(9), broken);
    fetch(&p, &f);
    let records = take();
    assert_private(&records);
    assert!(
        line_about(&records, "ClaudeOAuthUsage:").contains("skipped: no Claude Code token in"),
        "{records:#?}"
    );
    assert_eq!(
        line_about(&records, "ClaudeDesktopCache:"),
        "claude: ClaudeDesktopCache: skipped: unreadable: Claude Desktop's cached usage: invalid \
         zstd data"
    );

    // No Claude Desktop cache at all, and the endpoint rejects the token.
    let f = Fakes::new(now());
    with_credentials(&f, include_str!("fixtures/claude/credentials-valid.json"));
    f.http.respond(
        Method::Get,
        oauth::USAGE_URL,
        HttpResponse::new(401, include_str!("fixtures/claude/usage-401.json")),
    );
    fetch(&p, &f);
    let records = take();
    assert_private(&records);
    assert_eq!(
        line_about(&records, "ClaudeOAuthUsage:"),
        "claude: ClaudeOAuthUsage: skipped: sign-in expired or rejected; the endpoint rejected \
         the token (HTTP 401)"
    );
    assert_eq!(
        line_about(&records, "ClaudeDesktopCache:"),
        "claude: ClaudeDesktopCache: skipped: Claude Desktop's HTTP cache not found"
    );

    // A good answer, then the 5-minute rest reuses it and says so.
    let p = ClaudeProvider::new(ClaudeOptions::default());
    let f = Fakes::new(now());
    with_credentials(&f, include_str!("fixtures/claude/credentials-valid.json"));
    f.http.respond(
        Method::Get,
        oauth::USAGE_URL,
        HttpResponse::new(
            200,
            include_str!("fixtures/claude/usage-200-two-windows.json"),
        ),
    );
    fetch(&p, &f);
    let records = take();
    assert_private(&records);
    assert_eq!(
        line_about(&records, "ClaudeOAuthUsage:"),
        "claude: ClaudeOAuthUsage: read 2 windows measured 2026-10-10T10:42:00Z (0 s old)"
    );
    f.clock.advance(mins(2));
    fetch(&p, &f);
    let records = take();
    let rest = line_about(&records, "ClaudeOAuthUsage:");
    assert!(
        rest.contains("(2 min old); resting until 2026-10-10T10:46:30Z"),
        "{rest}"
    );
    assert!(
        line_about(&records, "using")
            .starts_with(&format!("claude: using {:?}", SourceKind::ClaudeOAuthUsage)),
        "{records:#?}"
    );
}
