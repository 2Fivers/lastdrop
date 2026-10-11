//! The Claude Desktop cache source against invented Chromium block-file caches built by
//! `lastdrop_providers::fakes::chromium_cache` (never a copy of a real cache, which holds a
//! person's conversations): the block-file reader, the response info, body decoding, the source
//! and its place among the provider's sources. No test touches the real file system.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{AccountKey, Health, SourceKind};
use lastdrop_core::settings::ClaudeOptions;
use lastdrop_providers::claude::blockfile::{
    self, Addr, BlockfileCache, CacheError, MAX_VISITS, chromium_time,
};
use lastdrop_providers::claude::desktop_cache::{self, CachedUsage};
use lastdrop_providers::claude::response_info::{self, DecodeError, InfoError, ResponseInfo};
use lastdrop_providers::claude::{
    ClaudeProvider, ClaudeUsage, ClaudeWindow, WindowReading, desktop, oauth,
};
use lastdrop_providers::fakes::Fakes;
use lastdrop_providers::fakes::chromium_cache::{
    FakeChromiumCache, FakeEntry, FakeResponse, chromium_micros, write_files,
};
use lastdrop_providers::io::{HttpResponse, Method};
use lastdrop_providers::{AccountSource, FetchError, Provider};

/// An invented organisation id; it must never leave the source.
const ORG: &str = "00000000-0000-4000-8000-000000000007";

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

fn desktop_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\tester\AppData\Roaming\Claude")
}

fn store_desktop_dir() -> PathBuf {
    PathBuf::from(
        r"C:\Users\tester\AppData\Local\Packages\Claude_testpublisher\LocalCache\Roaming\Claude",
    )
}

fn cache_dir() -> PathBuf {
    desktop_cache::cache_dir(&desktop_dir())
}

fn account() -> AccountSource {
    AccountSource::Claude {
        account: AccountKey::primary(),
        config_dir: PathBuf::from(r"C:\Users\tester\.claude"),
        state_file: PathBuf::from(r"C:\Users\tester\.claude.json"),
        desktop_dir: Some(desktop_dir()),
    }
}

fn usage_key(query: &str) -> String {
    format!("1/0/https://claude.ai/api/organizations/{ORG}/usage{query}")
}

/// An invented usage body: 5-hour and weekly with reset times.
fn body(five: f64, weekly: f64) -> String {
    format!(
        r#"{{"five_hour": {{"utilization": {five}, "resets_at": "2026-10-10T12:56:00+00:00"}},
"seven_day": {{"utilization": {weekly}, "resets_at": "2026-10-12T07:00:00+00:00"}}}}"#
    )
}

fn zstd(data: &[u8]) -> Vec<u8> {
    ruzstd::encoding::compress_to_vec(data, ruzstd::encoding::CompressionLevel::Fastest)
}

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).expect("gzip");
    e.finish().expect("gzip")
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).expect("zlib");
    e.finish().expect("zlib")
}

fn raw_deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).expect("deflate");
    e.finish().expect("deflate")
}

/// `usage-200-current.json` as brotli: .NET's `BrotliStream` at `Optimal` (fixtures README).
const CURRENT_BR: &[u8] = include_bytes!("fixtures/claude/usage-200-current.json.br");
const CURRENT_JSON: &str = include_str!("fixtures/claude/usage-200-current.json");

/// A live usage entry received at `at`, `coding` applied to `json` as the server would.
fn usage_entry(query: &str, at: Timestamp, coding: &str, json: &str) -> FakeEntry {
    let stored = match coding {
        "identity" => json.as_bytes().to_vec(),
        "zstd" => zstd(json.as_bytes()),
        "gzip" => gzip(json.as_bytes()),
        "br" if json == CURRENT_JSON => CURRENT_BR.to_vec(),
        other => panic!("no encoder for {other} in these tests"),
    };
    FakeEntry::new(
        &usage_key(query),
        FakeResponse::ok(at, coding).pickle(),
        stored,
        at - SignedDuration::from_millis(200),
    )
}

/// A conversation-like entry that must never be read as usage.
fn other_entry(key: &str, at: Timestamp, json: &str) -> FakeEntry {
    FakeEntry::new(
        key,
        FakeResponse::ok(at, "identity").pickle(),
        json.as_bytes().to_vec(),
        at,
    )
}

fn with_cache(f: &Fakes, dir: &Path, cache: &FakeChromiumCache) {
    cache.write(&f.fs, dir, now());
}

fn read(f: &Fakes) -> Result<CachedUsage, FetchError> {
    desktop_cache::read_detailed(&f.context(SignedDuration::from_secs(30)), &desktop_dir())
}

fn reading(usage: &ClaudeUsage, w: &ClaudeWindow) -> WindowReading {
    usage
        .windows
        .iter()
        .find(|r| &r.window == w)
        .cloned()
        .unwrap_or_else(|| panic!("no {w:?} in {usage:?}"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// 1601-01-01 UTC, `base::Time`'s epoch.
fn windows_epoch() -> Timestamp {
    Timestamp::from_microsecond(-11_644_473_600_000_000).expect("in range")
}

// ---- Keys --------------------------------------------------------------------------------------

#[test]
fn only_usage_endpoint_keys_match() {
    for key in [
        usage_key(""),
        usage_key("?skip_spend=1"),
        usage_key("?source=desktop&skip_spend=true"),
        format!("https://claude.ai/api/organizations/{ORG}/usage"),
        format!(
            "1/0/_dk_https://claude.ai https://claude.ai https://claude.ai/api/organizations/\
             {ORG}/usage"
        ),
    ] {
        assert!(desktop_cache::is_usage_key(key.as_bytes()), "{key}");
    }
    for key in [
        format!("1/0/https://claude.ai/api/organizations/{ORG}/usage/extra"),
        format!("1/0/https://claude.ai/api/organizations/{ORG}/usage_report"),
        format!("1/0/https://claude.ai/api/organizations/{ORG}/chat_conversations/x"),
        format!("1/0/https://claude.ai/api/organizations/{ORG}"),
        "1/0/https://claude.ai/api/organizations//usage".to_owned(),
        "1/0/https://claude.ai/api/organizations/a/../usage".to_owned(),
        format!("1/0/https://claude.ai.example.invalid/api/organizations/{ORG}/usage"),
        format!("1/0/http://claude.ai/api/organizations/{ORG}/usage"),
        format!("1/0/https://example.invalid/?u=https://claude.ai/api/organizations/{ORG}/usage"),
        String::new(),
    ] {
        assert!(!desktop_cache::is_usage_key(key.as_bytes()), "{key}");
    }
    assert_eq!(
        desktop_cache::key_url(b"1/0/https://claude.ai/x"),
        b"https://claude.ai/x"
    );
    assert_eq!(
        desktop_cache::key_url(b"https://claude.ai/x"),
        b"https://claude.ai/x"
    );
}

// ---- The block-file reader ---------------------------------------------------------------------

#[test]
fn addresses_decode_as_chromium_lays_them_out() {
    // BLOCK_256, 2 blocks, data_1, block 5.
    let a = Addr(0xA101_0005);
    assert!(a.is_initialized() && a.is_sane() && a.is_sane_for_entry());
    assert_eq!(
        (
            a.file_type(),
            a.num_blocks(),
            a.file_number(),
            a.start_block()
        ),
        (2, 2, 1, 5)
    );
    assert_eq!(a.block_size(), Some(256));
    // EXTERNAL f_00002a.
    let f = Addr(0x8000_002A);
    assert!(f.is_separate_file() && f.is_sane() && !f.is_sane_for_entry());
    assert_eq!(f.file_number(), 0x2A);
    assert_eq!(f.block_size(), None);
    // Not initialized; reserved bits; a type Chromium never shipped.
    assert!(!Addr(0x2001_0005).is_sane());
    assert!(!Addr(0xA501_0005).is_sane());
    assert!(!Addr(0xD001_0005).is_sane());
    assert!(!Addr(0xB001_0005).is_sane_for_entry(), "BLOCK_1K");

    let t = ts("2026-10-10T10:31:21.002446Z");
    assert_eq!(chromium_time(chromium_micros(t)), Some(t));
    assert_eq!(chromium_time(0), None, "a null base::Time");
}

#[test]
fn entries_are_found_along_chains_and_streams_read_from_every_kind_of_file() {
    let f = Fakes::new(now());
    // One bucket: every entry is chained behind the one before.
    let mut cache = FakeChromiumCache::new();
    cache.table_len = 1;
    let sizes = [300usize, 2_000, 10_000, 20_000];
    for (i, size) in sizes.iter().enumerate() {
        let mut json = body(10.0 + i as f64, 50.0).into_bytes();
        json.resize(*size, b' ');
        let mut entry = FakeEntry::new(
            &usage_key(&format!("?n={i}")),
            FakeResponse::ok(now() - mins(10 - i as i64), "identity").pickle(),
            json,
            now() - mins(10 - i as i64),
        );
        entry.external_body = i == 0;
        cache = cache.with(entry);
    }
    let files = cache.files();
    assert!(files.contains_key("f_000001"), "the forced external body");
    assert!(files.contains_key("f_000002"), "the 20 000-byte body");
    write_files(&f.fs, &cache_dir(), &files, now());

    let mut cache = BlockfileCache::open(&f.fs, &cache_dir()).expect("open");
    assert_eq!(cache.num_entries(), 4);
    let mut keys = 0;
    let found = cache
        .find(
            &mut |key| {
                keys += 1;
                desktop_cache::is_usage_key(key)
            },
            16,
        )
        .expect("walk");
    assert_eq!((found.visited, found.skipped, keys), (4, 0, 4));
    assert_eq!(found.entries.len(), 4);
    let mut lens: Vec<u64> = found
        .entries
        .iter()
        .map(|e| e.stream_len(1).expect("len"))
        .collect();
    lens.sort_unstable();
    assert_eq!(lens, [300, 2_000, 10_000, 20_000]);
    for entry in &found.entries {
        let stored = cache.read_stream(entry, 1, 1 << 20).expect("body");
        assert_eq!(stored.len() as u64, entry.stream_len(1).expect("len"));
        oauth::parse(&stored).expect("a usage body");
        let info = cache.read_stream(entry, 0, 1 << 16).expect("info");
        response_info::parse(&info).expect("response info");
        assert!(matches!(
            cache.read_stream(entry, 1, 100),
            Err(CacheError::TooLarge(100))
        ));
    }

    // At most `limit` entries are kept: those created last.
    let found = cache.find(&mut |_| true, 2).expect("walk");
    let mut created: Vec<_> = found.entries.iter().map(|e| e.created).collect();
    created.sort();
    assert_eq!(created, [Some(now() - mins(8)), Some(now() - mins(7))]);
}

#[test]
fn a_chain_that_loops_is_damage_not_a_hang() {
    let f = Fakes::new(now());
    let mut cache = FakeChromiumCache::new();
    cache.table_len = 1;
    let cache = cache.with(usage_entry("", now(), "identity", &body(1.0, 2.0)));
    let mut files = cache.files();
    // The bucket's first entry points back at itself.
    let head = files["index"][368..372].to_vec();
    let block = u32::from_le_bytes(head.clone().try_into().expect("4 bytes")) & 0xFFFF;
    let at = 8192 + block as usize * 256 + 4;
    files.get_mut("data_1").expect("data_1")[at..at + 4].copy_from_slice(&head);
    write_files(&f.fs, &cache_dir(), &files, now());

    let mut reader = BlockfileCache::open(&f.fs, &cache_dir()).expect("open");
    assert_eq!(
        reader.find(&mut |_| false, 16),
        Err(CacheError::Corrupt("index chains do not end"))
    );
    // A full default table (kIndexTablesize buckets) is walked before the walk gives up.
    const { assert!(MAX_VISITS >= blockfile::DEFAULT_TABLE_LEN) };
    assert!(matches!(read(&f), Err(FetchError::Transient { .. })));
}

#[test]
fn the_simple_backend_is_recognised_and_not_read() {
    let f = Fakes::new(now());
    f.fs.add_file(
        cache_dir().join("index-dir").join("the-real-index"),
        vec![0u8; 64],
        now(),
    );
    f.fs.add_file(cache_dir().join("0123456789abcdef_0"), vec![0u8; 64], now());
    assert!(blockfile::is_simple_backend(&f.fs, &cache_dir()));
    assert!(matches!(
        BlockfileCache::open(&f.fs, &cache_dir()),
        Err(CacheError::SimpleBackend)
    ));
    let err = read(&f).expect_err("simple format");
    assert!(
        matches!(&err, FetchError::Unavailable { detail } if detail.contains("simple format")),
        "{err:?}"
    );

    // Its `index` file starts with kSimpleInitialMagicNumber.
    let f = Fakes::new(now());
    let mut index = blockfile::SIMPLE_INITIAL_MAGIC.to_le_bytes().to_vec();
    index.resize(400, 0);
    f.fs.add_file(cache_dir().join("index"), index, now());
    assert!(matches!(
        BlockfileCache::open(&f.fs, &cache_dir()),
        Err(CacheError::SimpleBackend)
    ));
}

// ---- Response info and bodies ------------------------------------------------------------------

#[test]
fn the_response_info_gives_times_status_and_codings_and_nothing_else() {
    let at = ts("2026-10-10T10:31:21.002446Z");
    let mut response = FakeResponse::ok(at, "identity");
    response.original_response_time = Some(at - mins(60));
    response
        .headers
        .push(("Content-Encoding".to_owned(), "GZIP, br".to_owned()));
    response
        .headers
        .push(("content-encoding".to_owned(), " zstd ".to_owned()));
    response.disk_zstd_size = Some(3213);
    let info = response_info::parse(&response.pickle()).expect("parse");
    assert_eq!(
        info,
        ResponseInfo {
            request_time: Some(at - SignedDuration::from_millis(180)),
            response_time: Some(at),
            status: Some(200),
            content_encoding: vec!["gzip".to_owned(), "br".to_owned(), "zstd".to_owned()],
            truncated: false,
            disk_zstd_size: Some(3213),
        }
    );
    assert_eq!(info.content_encoding_label(), "gzip, br, zstd");
    assert!(
        !format!("{info:?}").contains("test-cookie"),
        "no header but codings is kept"
    );
    assert_eq!(
        response_info::parse(&FakeResponse::ok(at, "identity").pickle())
            .expect("parse")
            .content_encoding_label(),
        "identity"
    );

    let mut truncated = FakeResponse::ok(at, "br");
    truncated.truncated = true;
    truncated.status_line = "HTTP/1.1 206 Partial Content".to_owned();
    let info = response_info::parse(&truncated.pickle()).expect("parse");
    assert!(info.truncated);
    assert_eq!(info.status, Some(206));

    let mut old = FakeResponse::ok(at, "br");
    old.version = 2;
    assert_eq!(
        response_info::parse(&old.pickle()),
        Err(InfoError::Version(2))
    );

    let pickle = FakeResponse::ok(at, "br").pickle();
    for cut in [0, 3, 4, 8, 20, 30, pickle.len() - 1] {
        assert!(
            matches!(
                response_info::parse(&pickle[..cut]),
                Err(InfoError::Malformed(_))
            ),
            "cut at {cut}"
        );
    }
    // A payload size that claims more than the stream holds.
    let mut lying = pickle.clone();
    lying[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        response_info::parse(&lying),
        Err(InfoError::Malformed(_))
    ));
}

fn info_with(codings: &[&str], disk_zstd: Option<u64>) -> ResponseInfo {
    ResponseInfo {
        content_encoding: codings.iter().map(|c| (*c).to_owned()).collect(),
        disk_zstd_size: disk_zstd,
        ..ResponseInfo::default()
    }
}

#[test]
fn every_content_coding_decodes_in_pure_rust() {
    let json = CURRENT_JSON.as_bytes();
    let max = desktop_cache::MAX_DECODED_BODY;
    let cases: Vec<(&str, Vec<u8>, ResponseInfo)> = vec![
        ("identity", json.to_vec(), info_with(&[], None)),
        (
            "identity named",
            json.to_vec(),
            info_with(&["identity"], None),
        ),
        ("br", CURRENT_BR.to_vec(), info_with(&["br"], None)),
        ("zstd", zstd(json), info_with(&["zstd"], None)),
        ("gzip", gzip(json), info_with(&["gzip"], None)),
        ("x-gzip", gzip(json), info_with(&["x-gzip"], None)),
        ("deflate (zlib)", zlib(json), info_with(&["deflate"], None)),
        (
            "deflate (raw)",
            raw_deflate(json),
            info_with(&["deflate"], None),
        ),
        (
            "gzip then zstd",
            zstd(&gzip(json)),
            info_with(&["gzip", "zstd"], None),
        ),
        (
            "br, then zstd by the cache",
            zstd(CURRENT_BR),
            info_with(&["br"], Some(CURRENT_BR.len() as u64)),
        ),
        (
            "two zstd frames",
            [zstd(&json[..1000]), zstd(&json[1000..])].concat(),
            info_with(&["zstd"], None),
        ),
    ];
    for (name, stored, info) in cases {
        let decoded = response_info::decode_body(&stored, &info, max)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(decoded, json, "{name}");
    }
}

#[test]
fn bad_or_oversized_bodies_are_errors() {
    let json = CURRENT_JSON.as_bytes();
    let decode = |stored: &[u8], codings: &[&str], max: usize| {
        response_info::decode_body(stored, &info_with(codings, None), max)
    };
    assert_eq!(
        decode(&zstd(json), &["zstd"], 100),
        Err(DecodeError::TooLarge(100))
    );
    assert_eq!(
        decode(CURRENT_BR, &["br"], 100),
        Err(DecodeError::TooLarge(100))
    );
    assert_eq!(
        decode(&gzip(json), &["gzip"], 100),
        Err(DecodeError::TooLarge(100))
    );
    assert_eq!(
        decode(json, &[], 100),
        Ok(json.to_vec()),
        "identity bodies are capped by the stream read, not here"
    );
    assert_eq!(
        response_info::decode_body(&zstd(json), &info_with(&[], Some(1 << 30)), 1 << 20),
        Err(DecodeError::TooLarge(1 << 20)),
        "the recorded size is checked before decoding"
    );

    let mut bad_checksum = zstd(json);
    let last = bad_checksum.len() - 1;
    bad_checksum[last] ^= 0xFF;
    assert_eq!(
        decode(&bad_checksum, &["zstd"], 1 << 20),
        Err(DecodeError::Corrupt("zstd"))
    );
    assert_eq!(
        decode(&zstd(json)[..40], &["zstd"], 1 << 20),
        Err(DecodeError::Corrupt("zstd"))
    );
    assert_eq!(
        decode(b"", &["zstd"], 1 << 20),
        Err(DecodeError::Corrupt("zstd"))
    );
    assert_eq!(
        decode(json, &["zstd"], 1 << 20),
        Err(DecodeError::Corrupt("zstd"))
    );
    assert_eq!(
        decode(&CURRENT_BR[..200], &["br"], 1 << 20),
        Err(DecodeError::Corrupt("br"))
    );
    assert_eq!(
        decode(json, &["gzip"], 1 << 20),
        Err(DecodeError::Corrupt("gzip"))
    );
    assert_eq!(
        response_info::decode_body(json, &info_with(&[], Some(10)), 1 << 20),
        Err(DecodeError::Corrupt("disk zstd"))
    );
    assert_eq!(
        decode(json, &["compress"], 1 << 20),
        Err(DecodeError::Unsupported("compress".to_owned()))
    );
    assert_eq!(
        decode(json, &["x; secret=1"], 1 << 20),
        Err(DecodeError::Unsupported("?".to_owned())),
        "an odd coding name is not repeated"
    );
}

// ---- The source --------------------------------------------------------------------------------

/// A cache like Desktop's: conversations, other API answers, several usage answers.
fn realistic_cache() -> FakeChromiumCache {
    let conversation = r#"{"uuid": "invented", "name": "invented conversation", "five_hour": {"utilization": 99.0}}"#;
    FakeChromiumCache::new()
        .with(other_entry(
            &format!("1/0/https://claude.ai/api/organizations/{ORG}/chat_conversations/x"),
            now() - mins(1),
            conversation,
        ))
        .with(other_entry(
            &format!("1/0/https://claude.ai/api/organizations/{ORG}/usage/history"),
            now() - mins(1),
            &body(97.0, 97.0),
        ))
        .with(other_entry(
            &format!("1/0/https://claude.ai.example.invalid/api/organizations/{ORG}/usage"),
            now() - mins(1),
            &body(98.0, 98.0),
        ))
        .with(usage_entry(
            "?skip_spend=1",
            now() - mins(30),
            "br",
            CURRENT_JSON,
        ))
        .with(usage_entry("", now() - mins(11), "zstd", &body(5.0, 91.0)))
        .with(other_entry(
            "1/0/https://claude.ai/favicon.ico",
            now(),
            "not json",
        ))
}

#[test]
fn the_newest_usage_response_is_read_with_its_response_time() {
    let f = Fakes::new(now());
    with_cache(&f, &cache_dir(), &realistic_cache());
    let cached = read(&f).expect("read");
    assert_eq!(cached.usage.measured_at, Some(now() - mins(11)));
    assert_eq!(cached.content_encoding, "zstd");
    assert!(!cached.disk_zstd);
    assert_eq!((cached.responses, cached.visited), (2, 6));
    assert_eq!(cached.usage.windows.len(), 2);
    let five = reading(&cached.usage, &ClaudeWindow::FiveHour);
    assert!(close(five.used, 0.05));
    assert_eq!(five.resets_at, Some(ts("2026-10-10T12:56:00Z")));
    assert!(close(
        reading(&cached.usage, &ClaudeWindow::Weekly).used,
        0.91
    ));
    assert_eq!(
        cached.note(),
        "content-encoding zstd, 2 usage response(s) among 6 entries"
    );
    let shown = format!("{cached:?} {}", cached.note());
    for private in [ORG, "test-cookie", "invented conversation", "claude.ai"] {
        assert!(!shown.contains(private), "{private} in {shown}");
    }

    // Without the newest one, the older brotli answer with its per-model window.
    let mut cache = realistic_cache();
    cache.entries.remove(4);
    with_cache(&f, &cache_dir(), &cache);
    let cached = read(&f).expect("read");
    assert_eq!(cached.content_encoding, "br");
    assert_eq!(cached.usage.measured_at, Some(now() - mins(30)));
    let fable = reading(
        &cached.usage,
        &ClaudeWindow::WeeklyModel("Fable".to_owned()),
    );
    assert_eq!(fable.resets_at, Some(ts("2026-10-12T07:00:00Z")));
}

#[test]
fn the_response_time_decides_not_the_entry_creation() {
    let f = Fakes::new(now());
    // Created long ago but revalidated a minute ago, against one created and answered 20
    // minutes ago.
    let mut revalidated = usage_entry("", now() - mins(1), "identity", &body(7.0, 70.0));
    revalidated.created = now() - mins(600);
    let cache = FakeChromiumCache::new()
        .with(usage_entry(
            "?skip_spend=1",
            now() - mins(20),
            "identity",
            &body(6.0, 60.0),
        ))
        .with(revalidated);
    with_cache(&f, &cache_dir(), &cache);
    let cached = read(&f).expect("read");
    assert_eq!(cached.usage.measured_at, Some(now() - mins(1)));
    assert!(close(
        reading(&cached.usage, &ClaudeWindow::FiveHour).used,
        0.07
    ));
}

#[test]
fn a_body_the_cache_compressed_is_read() {
    let f = Fakes::new(now());
    let json = body(12.0, 34.0);
    let mut response = FakeResponse::ok(now() - mins(2), "gzip");
    response.disk_zstd_size = Some(gzip(json.as_bytes()).len() as u64);
    let entry = FakeEntry::new(
        &usage_key(""),
        response.pickle(),
        zstd(&gzip(json.as_bytes())),
        now() - mins(2),
    );
    with_cache(&f, &cache_dir(), &FakeChromiumCache::new().with(entry));
    let cached = read(&f).expect("read");
    assert!(cached.disk_zstd);
    assert_eq!(cached.content_encoding, "gzip");
    assert!(close(
        reading(&cached.usage, &ClaudeWindow::Weekly).used,
        0.34
    ));
    assert!(cached.note().contains("(zstd on disk)"));
}

#[test]
fn unfinished_or_failed_answers_are_passed_over() {
    let good = usage_entry(
        "?skip_spend=1",
        now() - mins(30),
        "identity",
        &body(6.0, 60.0),
    );
    let newer = |change: &dyn Fn(&mut FakeEntry, &mut FakeResponse)| {
        let mut response = FakeResponse::ok(now() - mins(1), "identity");
        let mut entry = FakeEntry::new(
            &usage_key(""),
            Vec::new(),
            body(9.0, 99.0).into_bytes(),
            now() - mins(1),
        );
        change(&mut entry, &mut response);
        entry.info = response.pickle();
        entry
    };
    let cases: Vec<(&str, FakeEntry)> = vec![
        ("evicted", newer(&|e, _| e.state = 1)),
        ("doomed", newer(&|e, _| e.state = 2)),
        ("truncated", newer(&|_, r| r.truncated = true)),
        (
            "a 503",
            newer(&|_, r| r.status_line = "HTTP/1.1 503 Service Unavailable".to_owned()),
        ),
        ("not written yet", newer(&|e, _| e.body_not_written = true)),
        ("not JSON", newer(&|e, _| e.body = b"<html>".to_vec())),
        // A null `base::Time` (0 microseconds after 1601-01-01).
        ("no time", newer(&|_, r| r.response_time = windows_epoch())),
    ];
    for (name, bad) in cases {
        let f = Fakes::new(now());
        let cache = FakeChromiumCache::new()
            .with(good.clone())
            .with(bad.clone());
        with_cache(&f, &cache_dir(), &cache);
        let cached = read(&f).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(cached.usage.measured_at, Some(now() - mins(30)), "{name}");

        // Alone, each says why.
        let f = Fakes::new(now());
        with_cache(&f, &cache_dir(), &FakeChromiumCache::new().with(bad));
        let err = read(&f).expect_err(name);
        let expected = match name {
            "evicted" | "doomed" => "no usage response",
            "truncated" => "incomplete",
            "a 503" => "HTTP 503",
            "not written yet" => "not on disk yet",
            "not JSON" => "usage response",
            _ => "without a time",
        };
        assert!(err.detail().contains(expected), "{name}: {err:?}");
    }
}

#[test]
fn no_cache_or_no_usage_response_is_unavailable() {
    let f = Fakes::new(now());
    let err = read(&f).expect_err("no cache");
    assert_eq!(
        err,
        FetchError::Unavailable {
            detail: "Claude Desktop's HTTP cache not found".to_owned()
        }
    );

    let cache = FakeChromiumCache::new().with(other_entry(
        &format!("1/0/https://claude.ai/api/organizations/{ORG}/chat_conversations/x"),
        now(),
        "{}",
    ));
    with_cache(&f, &cache_dir(), &cache);
    assert_eq!(
        read(&f),
        Err(FetchError::Unavailable {
            detail: "no usage response among the 1 entries of Claude Desktop's HTTP cache"
                .to_owned()
        })
    );

    // A cache folder without an index.
    let f = Fakes::new(now());
    f.fs.add_file(cache_dir().join("data_0"), vec![0u8; 8192], now());
    assert!(matches!(read(&f), Err(FetchError::Unavailable { .. })));
}

#[test]
fn the_store_builds_cache_is_read_and_the_newest_folder_wins() {
    let f = Fakes::new(now());
    let store_cache = desktop_cache::cache_dir(&store_desktop_dir());
    let one = |at: Timestamp, five: f64| {
        FakeChromiumCache::new().with(usage_entry("", at, "zstd", &body(five, 30.0)))
    };
    one(now() - mins(3), 3.0).write(&f.fs, &store_cache, now() - mins(3));
    let cached = read(&f).expect("from the package");
    assert_eq!(cached.usage.measured_at, Some(now() - mins(3)));

    // An older cache in %APPDATA% (an earlier install) loses; a newer one wins.
    one(now() - mins(90), 1.0).write(&f.fs, &cache_dir(), now() - mins(90));
    assert_eq!(
        read(&f).expect("read").usage.measured_at,
        Some(now() - mins(3))
    );
    one(now() - mins(1), 4.0).write(&f.fs, &cache_dir(), now() - mins(1));
    assert_eq!(
        read(&f).expect("read").usage.measured_at,
        Some(now() - mins(1))
    );

    // The same files seen through both paths (equal sizes and times) are read once, and
    // still read.
    one(now() - mins(1), 4.0).write(&f.fs, &store_cache, now() - mins(1));
    assert_eq!(
        read(&f).expect("read").usage.measured_at,
        Some(now() - mins(1))
    );
    one(now() - mins(3), 3.0).write(&f.fs, &store_cache, now() - mins(3));

    // A damaged folder does not hide a good one.
    f.fs.add_file(cache_dir().join("index"), vec![0xAB; 1000], now());
    assert_eq!(
        read(&f).expect("read").usage.measured_at,
        Some(now() - mins(3))
    );
}

#[test]
fn damaged_files_fail_gracefully() {
    let good = realistic_cache().files();
    let check =
        |name: &str, files: &BTreeMap<String, Vec<u8>>, want: &dyn Fn(&FetchError) -> bool| {
            let f = Fakes::new(now());
            write_files(&f.fs, &cache_dir(), files, now());
            match read(&f) {
                Ok(_) => panic!("{name}: read a damaged cache"),
                Err(e) => assert!(want(&e), "{name}: {e:?}"),
            }
        };
    let altered = |file: &str, change: &dyn Fn(&mut Vec<u8>)| {
        let mut files = good.clone();
        change(files.get_mut(file).expect("file"));
        files
    };
    let transient = |e: &FetchError| matches!(e, FetchError::Transient { .. });
    let parse = |e: &FetchError| matches!(e, FetchError::Parse { .. });
    let unavailable = |e: &FetchError| matches!(e, FetchError::Unavailable { .. });

    check(
        "index cut in its header",
        &altered("index", &|d| d.truncate(100)),
        &transient,
    );
    check(
        "index cut in its table",
        &altered("index", &|d| d.truncate(368 + 100)),
        &transient,
    );
    check("index magic", &altered("index", &|d| d[0] ^= 1), &parse);
    check("index version", &altered("index", &|d| d[6] = 5), &parse);
    check(
        "index table length",
        &altered("index", &|d| {
            d[28..32].copy_from_slice(&i32::MAX.to_le_bytes())
        }),
        &transient,
    );
    check(
        "entries file cut",
        &altered("data_1", &|d| d.truncate(8192)),
        &unavailable,
    );
    check(
        "entries file magic",
        &altered("data_1", &|d| d[0] ^= 1),
        &unavailable,
    );
    check(
        "entries file gone",
        &{
            let mut files = good.clone();
            files.remove("data_1");
            files
        },
        &unavailable,
    );

    // The newest usage body lives in data_1 (zstd, under 1 KiB): corrupt every body byte.
    let f = Fakes::new(now());
    write_files(&f.fs, &cache_dir(), &good, now());
    let newest = read(&f).expect("good");
    assert_eq!(newest.content_encoding, "zstd");
    let mut files = good.clone();
    let compressed = zstd(body(5.0, 91.0).as_bytes());
    let data_1 = files.get_mut("data_1").expect("data_1");
    let at = data_1
        .windows(compressed.len())
        .position(|w| w == compressed.as_slice())
        .expect("the stored body");
    data_1[at + compressed.len() - 1] ^= 0xFF;
    let f = Fakes::new(now());
    write_files(&f.fs, &cache_dir(), &files, now());
    let older = read(&f).expect("the older answer stands in");
    assert_eq!(older.content_encoding, "br");

    // Alone, a body that does not decode is a parse error; a missing or short f_ file is
    // damage.
    let json = body(5.0, 91.0);
    let mut alone = usage_entry("", now(), "zstd", &json);
    alone.body[10] ^= 0xFF;
    alone.body[11] ^= 0xFF;
    let files = FakeChromiumCache::new().with(alone).files();
    check("a body that does not decode", &files, &parse);
    let mut external = usage_entry("", now(), "identity", &json);
    external.external_body = true;
    let files = FakeChromiumCache::new().with(external).files();
    assert!(files.contains_key("f_000001"));
    let mut gone = files.clone();
    gone.remove("f_000001");
    check("f_ file gone", &gone, &transient);
    let mut short = files.clone();
    short.get_mut("f_000001").expect("f_").truncate(10);
    check("f_ file short", &short, &transient);
    let mut info = usage_entry("", now(), "identity", &json);
    info.info = vec![0xFF; 64];
    check(
        "response info that does not parse",
        &FakeChromiumCache::new().with(info).files(),
        &parse,
    );
}

#[test]
fn no_byte_change_or_cut_makes_the_reader_panic() {
    let good = realistic_cache().files();
    let f = Fakes::new(now());
    // A small deterministic generator (xorshift), so a failure can be replayed.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let names: Vec<String> = good.keys().cloned().collect();
    for round in 0..1500 {
        let mut files = good.clone();
        let name = &names[(next() % names.len() as u64) as usize];
        let data = files.get_mut(name).expect("file");
        if data.is_empty() {
            continue;
        }
        let at = (next() % data.len() as u64) as usize;
        if round % 5 == 0 {
            data.truncate(at);
        } else {
            data[at] ^= (next() % 255 + 1) as u8;
        }
        write_files(&f.fs, &cache_dir(), &files, now());
        // Any outcome but a panic; a reading must still be a usage reading.
        if let Ok(cached) = read(&f) {
            assert!(!cached.usage.windows.is_empty(), "round {round}");
        }
    }
}

#[test]
fn a_fetch_out_of_time_reads_nothing() {
    let f = Fakes::new(now());
    with_cache(&f, &cache_dir(), &realistic_cache());
    assert!(matches!(
        desktop_cache::read(&f.context(SignedDuration::ZERO), &desktop_dir()),
        Err(FetchError::Transient { .. })
    ));
}

// ---- Among the provider's sources ---------------------------------------------------------------

fn with_history(f: &Fakes, sample_at: Timestamp, five: u32, weekly: u32) {
    let data = format!(
        r#"{{"version": 2, "samples": [{{"t": {}, "u": {{"fh": {five}, "sd": {weekly}}}}}]}}"#,
        sample_at.as_millisecond()
    );
    f.fs.add_file(desktop::path(&desktop_dir()), data, sample_at);
}

fn with_cached_response(f: &Fakes, at: Timestamp, json: &str) {
    with_cache(
        f,
        &cache_dir(),
        &FakeChromiumCache::new()
            .with(other_entry("1/0/https://claude.ai/favicon.ico", now(), "x"))
            .with(usage_entry("", at, "zstd", json)),
    );
}

fn fetch(p: &ClaudeProvider, f: &Fakes) -> Result<lastdrop_core::model::Snapshot, FetchError> {
    p.fetch(&f.context(SignedDuration::from_secs(30)), &account())
}

fn ids(snapshot: &lastdrop_core::model::Snapshot) -> Vec<String> {
    snapshot.meters.iter().map(|m| m.id.0.clone()).collect()
}

#[test]
fn a_newer_cached_response_beats_the_desktop_history() {
    let f = Fakes::new(now());
    with_history(&f, now() - mins(18), 30, 50);
    with_cached_response(&f, now() - mins(9), CURRENT_JSON);
    let snap = fetch(&ClaudeProvider::new(ClaudeOptions::default()), &f).expect("fetch");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopCache);
    assert_eq!(snap.measured_at, now() - mins(9));
    assert_eq!(snap.health, Health::Ok, "no token needed");
    assert_eq!(ids(&snap), ["five_hour", "weekly", "weekly:fable"]);
    assert_eq!(
        snap.meters[0].resets_at,
        Some(ts("2026-10-10T12:56:00.123456Z"))
    );
    assert!(f.http.requests().is_empty());

    // Turned off, it is not read.
    let off = ClaudeProvider::new(ClaudeOptions {
        desktop_cache: false,
        ..ClaudeOptions::default()
    });
    assert_eq!(
        fetch(&off, &f).expect("history").source,
        SourceKind::ClaudeDesktopHistory
    );
}

#[test]
fn a_history_sample_moments_after_the_cached_response_does_not_replace_it() {
    // Desktop writes a history sample when it receives the response its cache keeps.
    let f = Fakes::new(now());
    with_cached_response(&f, now() - mins(10), &body(37.0, 58.0));
    with_history(&f, now() - mins(10) + SignedDuration::from_secs(40), 37, 58);
    let snap = fetch(&ClaudeProvider::new(ClaudeOptions::default()), &f).expect("fetch");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopCache);
    assert_eq!(snap.meters[1].resets_at, Some(ts("2026-10-12T07:00:00Z")));
}

#[test]
fn a_later_history_sample_wins_and_borrows_the_cached_reset_times() {
    let f = Fakes::new(now());
    with_cached_response(&f, now() - mins(40), &body(30.0, 55.0));
    with_history(&f, now() - mins(10), 37, 58);
    let snap = fetch(&ClaudeProvider::new(ClaudeOptions::default()), &f).expect("fetch");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopHistory);
    assert_eq!(snap.measured_at, now() - mins(10));
    assert!(close(snap.meters[0].used, 0.37));
    assert_eq!(snap.meters[0].resets_at, Some(ts("2026-10-10T12:56:00Z")));
    assert_eq!(snap.meters[1].resets_at, Some(ts("2026-10-12T07:00:00Z")));
}

#[test]
fn a_fresh_oauth_answer_still_wins_and_a_rejected_token_leaves_the_cache() {
    let f = Fakes::new(now());
    let credentials = oauth::credentials_path(Path::new(r"C:\Users\tester\.claude"));
    f.fs.add_file(
        &credentials,
        include_str!("fixtures/claude/credentials-valid.json"),
        now() - mins(60),
    );
    with_cached_response(&f, now() - mins(1), CURRENT_JSON);
    f.http.respond(
        Method::Get,
        oauth::USAGE_URL,
        HttpResponse::new(
            200,
            include_str!("fixtures/claude/usage-200-two-windows.json"),
        ),
    );
    let p = ClaudeProvider::new(ClaudeOptions::default());
    let snap = fetch(&p, &f).expect("fetch");
    assert_eq!(
        snap.source,
        SourceKind::ClaudeOAuthUsage,
        "the live answer is the newest"
    );

    // A cached response stamped a minute ahead (a skewed clock) is the same moment: the
    // earlier source in the order still wins.
    f.clock.advance(mins(6));
    with_cached_response(&f, now() + mins(7), CURRENT_JSON);
    f.http.respond(
        Method::Get,
        oauth::USAGE_URL,
        HttpResponse::new(
            200,
            include_str!("fixtures/claude/usage-200-two-windows.json"),
        ),
    );
    let snap = fetch(&p, &f).expect("fetch");
    assert_eq!(snap.source, SourceKind::ClaudeOAuthUsage);
    assert_eq!(snap.measured_at, now() + mins(6));

    let f = Fakes::new(now());
    f.fs.add_file(
        &credentials,
        include_str!("fixtures/claude/credentials-valid.json"),
        now() - mins(60),
    );
    with_cached_response(&f, now() - mins(4), CURRENT_JSON);
    f.http.respond(
        Method::Get,
        oauth::USAGE_URL,
        HttpResponse::new(401, include_str!("fixtures/claude/usage-401.json")),
    );
    let fresh = ClaudeProvider::new(ClaudeOptions::default());
    let snap = fetch(&fresh, &f).expect("the cache answers");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopCache);
    assert_eq!(snap.health, Health::SignInRequired);
    assert_eq!(snap.account.plan.as_deref(), Some("Max"));
}

#[test]
fn an_old_cached_response_is_stale() {
    let f = Fakes::new(now());
    with_cached_response(&f, now() - mins(180), &body(5.0, 91.0));
    let snap = fetch(&ClaudeProvider::new(ClaudeOptions::default()), &f).expect("fetch");
    assert_eq!(snap.source, SourceKind::ClaudeDesktopCache);
    assert_eq!(snap.health, Health::Stale);
}
