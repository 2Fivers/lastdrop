//! Source: Claude Desktop's HTTP cache (no token needed).
//!
//! Claude Desktop (Electron) asks claude.ai for the plan's usage itself —
//! `GET https://claude.ai/api/organizations/<organisation>/usage`, sometimes with a query — and
//! Chromium keeps the last answer in Desktop's HTTP disk cache, `Cache\Cache_Data` in the desktop
//! data folder (each of [`super::desktop::candidates`]: `%APPDATA%\Claude`, the Microsoft Store
//! build's `LocalCache\Roaming\Claude`). The body has the OAuth usage endpoint's shape
//! (`five_hour`, `seven_day`, `limits[]` with the `weekly_scoped` per-model windows, `resets_at`)
//! and goes through the same parser ([`super::oauth::parse`]). Unlike Desktop's history it has
//! reset times and the per-model windows, and it is as fresh as Desktop's last look.
//!
//! Steps: open the block-file cache ([`super::blockfile`]); walk its index for live entries whose
//! key ends in a usage URL ([`is_usage_key`]); read each one's response info
//! ([`super::response_info`]) and keep complete 2xx answers; take the newest by response time;
//! read its body (≤ [`MAX_STORED_BODY`]), undo the cache's and the server's compression
//! ([`super::response_info::decode_body`], ≤ [`MAX_DECODED_BODY`]) and parse it. When that
//! answer cannot be read, the next newest is tried. `measured_at` is the cached **response time**
//! (when Desktop received the answer), so a reading is exactly as old as Desktop's last fetch.
//! With several cache folders, the newest reading wins; folders that are the same files seen
//! through two paths (the Store build's redirected `%APPDATA%`) are read once.
//!
//! **Privacy.** The cache holds the owner's conversations and the key holds the organisation id.
//! Only usage entries' response info and body are read; keys are compared in place and never
//! kept, logged or returned; the response info's buffer (its headers may hold cookies) is wiped
//! after parsing; nothing but the usage windows and the content coding leaves this module.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use lastdrop_core::model::SourceKind;

use super::blockfile::{BlockfileCache, CacheError, Entry};
use super::response_info::{self, ResponseInfo};
use super::{ClaudeUsage, desktop, most_telling, oauth};
use crate::FetchContext;
use crate::error::FetchError;
use crate::io::{FileMeta, MAX_BODY_BYTES};

/// The URL prefix of the usage endpoint, followed by the organisation id and `/usage`.
pub const USAGE_URL_PREFIX: &str = "https://claude.ai/api/organizations/";

/// Largest response info read (stream 0: headers and the certificate chain, a few KiB).
pub const MAX_INFO_BYTES: u64 = 256 << 10;
/// Largest stored body read (stream 1), as for a live response ([`MAX_BODY_BYTES`]).
pub const MAX_STORED_BODY: u64 = MAX_BODY_BYTES;
/// Largest decoded body.
pub const MAX_DECODED_BODY: usize = 1 << 20;
/// Most usage entries considered (one per organisation and query; more is not expected).
pub const MAX_CANDIDATES: usize = 16;

const FROM: SourceKind = SourceKind::ClaudeDesktopCache;

/// The cache folder of a desktop data folder: `<desktop_dir>\Cache\Cache_Data`.
pub fn cache_dir(desktop_dir: &Path) -> PathBuf {
    desktop_dir.join("Cache").join("Cache_Data")
}

/// The URL of an HTTP cache key. `HttpCache::GenerateCacheKey` (`net/http/http_cache.cc`) writes
/// `<credentials 0|1>/<upload id>/`, then, when the cache is split by site, an isolation key
/// ending in a space (`_dk_<top-frame site> <frame site> `), then the URL. Claude Desktop's
/// cache is not split: its keys read `1/0/https://…`.
pub fn key_url(key: &[u8]) -> &[u8] {
    let last = key.rsplit(|b| *b == b' ').next().unwrap_or(key);
    let mut rest = last;
    for _ in 0..2 {
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        match rest.get(digits) {
            Some(b'/') if digits > 0 => rest = &rest[digits + 1..],
            _ => return last,
        }
    }
    rest
}

/// Whether an HTTP cache key is the usage endpoint's: its URL ([`key_url`]) is
/// `https://claude.ai/api/organizations/<id>/usage` with any query (Desktop asks with and
/// without `skip_spend`, `source` and other parameters, each its own entry).
pub fn is_usage_key(key: &[u8]) -> bool {
    let url = key_url(key);
    let Some(rest) = url.strip_prefix(USAGE_URL_PREFIX.as_bytes()) else {
        return false;
    };
    let path = rest
        .split(|b| *b == b'?' || *b == b'#')
        .next()
        .unwrap_or(rest);
    let Some(slash) = path.iter().position(|b| *b == b'/') else {
        return false;
    };
    let (org, tail) = path.split_at(slash);
    tail == b"/usage"
        && !org.is_empty()
        && org.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
}

/// What the source read, with the details the logs and the live smoke test show.
#[derive(Clone, Debug, PartialEq)]
pub struct CachedUsage {
    /// The usage, `measured_at` = the cached response time.
    pub usage: ClaudeUsage,
    /// The response's `Content-Encoding` (`br`, `identity`, …).
    pub content_encoding: String,
    /// Whether the cache had compressed the body with zstd for the disk.
    pub disk_zstd: bool,
    /// Usage responses found in the cache folder the reading came from.
    pub responses: usize,
    /// Entries visited in that folder's index.
    pub visited: u32,
}

impl CachedUsage {
    /// One line for the log: coding and counts, no ids.
    pub fn note(&self) -> String {
        format!(
            "content-encoding {}{}, {} usage response(s) among {} entries",
            self.content_encoding,
            if self.disk_zstd {
                " (zstd on disk)"
            } else {
                ""
            },
            self.responses,
            self.visited
        )
    }
}

/// The source: [`read_detailed`] without the details.
pub fn read(ctx: &FetchContext<'_>, desktop_dir: &Path) -> Result<ClaudeUsage, FetchError> {
    read_detailed(ctx, desktop_dir).map(|cached| cached.usage)
}

/// Reads the newest cached usage response of every cache folder of `desktop_dir` (module docs).
///
/// - `FetchError::Unavailable`: no cache folder, a cache in Chromium's simple format, no usage
///   response in it (Desktop has not opened its usage view, or the entry was evicted), or only
///   incomplete or failed responses.
/// - `FetchError::Transient`: a file that cannot be read now, or a cache Chromium is rewriting.
/// - `FetchError::Parse`: an unknown cache format, a body that cannot be decoded, or JSON that is
///   not a usage body.
pub fn read_detailed(
    ctx: &FetchContext<'_>,
    desktop_dir: &Path,
) -> Result<CachedUsage, FetchError> {
    if ctx.remaining().is_zero() {
        return Err(FetchError::Transient {
            from: FROM,
            detail: "timed out".to_owned(),
            retry_after: None,
        });
    }
    let dirs = cache_dirs(ctx, desktop_dir);
    if dirs.is_empty() {
        return Err(FetchError::Unavailable {
            detail: "Claude Desktop's HTTP cache not found".to_owned(),
        });
    }
    let mut best: Option<CachedUsage> = None;
    let mut failures = Vec::new();
    for dir in &dirs {
        match read_dir(ctx, dir) {
            Ok(cached) => {
                if best
                    .as_ref()
                    .is_none_or(|b| cached.usage.measured_at > b.usage.measured_at)
                {
                    best = Some(cached);
                }
            }
            Err(e) => failures.push(e),
        }
    }
    best.ok_or_else(|| most_telling(failures))
}

/// The cache folders of the desktop data folders that exist, each set of files once: two paths
/// whose `index` and `data_1` have the same size and time are the same cache.
fn cache_dirs(ctx: &FetchContext<'_>, desktop_dir: &Path) -> Vec<PathBuf> {
    let mut seen: Vec<(Option<FileMeta>, Option<FileMeta>)> = Vec::new();
    let mut dirs = Vec::new();
    for dir in desktop::candidates(ctx, desktop_dir) {
        let cache = cache_dir(&dir);
        if !ctx.fs.metadata(&cache).is_ok_and(|m| m.is_dir) {
            continue;
        }
        let files = (
            ctx.fs.metadata(&cache.join("index")).ok(),
            ctx.fs.metadata(&cache.join("data_1")).ok(),
        );
        if files.0.is_some() && seen.contains(&files) {
            continue;
        }
        seen.push(files);
        dirs.push(cache);
    }
    dirs
}

/// One cache folder (module docs, "Steps").
fn read_dir(ctx: &FetchContext<'_>, dir: &Path) -> Result<CachedUsage, FetchError> {
    let mut cache = BlockfileCache::open(ctx.fs, dir).map_err(cache_error)?;
    let found = cache
        .find(&mut |key| is_usage_key(key), MAX_CANDIDATES)
        .map_err(cache_error)?;
    log::debug!(
        "claude: Desktop cache: visited {} entries (the index lists {}), skipped {}, {} usage \
         responses",
        found.visited,
        cache.num_entries(),
        found.skipped,
        found.entries.len()
    );
    if found.entries.is_empty() {
        return Err(FetchError::Unavailable {
            detail: format!(
                "no usage response among the {} entries of Claude Desktop's HTTP cache",
                found.visited
            ),
        });
    }
    let responses = found.entries.len();
    let mut failures = Vec::new();
    let mut answers: Vec<(Timestamp, ResponseInfo, Entry)> = Vec::new();
    for entry in found.entries {
        match response_of(&mut cache, &entry) {
            Ok((at, info)) => answers.push((at, info, entry)),
            Err(e) => failures.push(e),
        }
    }
    answers.sort_by_key(|(at, ..)| std::cmp::Reverse(*at));
    for (at, info, entry) in answers {
        match usage_of(&mut cache, &entry, &info) {
            Ok(mut usage) => {
                usage.measured_at = Some(at);
                return Ok(CachedUsage {
                    usage,
                    content_encoding: info.content_encoding_label(),
                    disk_zstd: info.disk_zstd_size.is_some(),
                    responses,
                    visited: found.visited,
                });
            }
            Err(e) => {
                log::debug!("claude: Desktop cache: a usage response could not be used: {e}");
                failures.push(e);
            }
        }
    }
    Err(most_telling(failures))
}

/// The response info of a usage entry when it is a complete 2xx answer with a response time.
fn response_of(
    cache: &mut BlockfileCache<'_>,
    entry: &Entry,
) -> Result<(Timestamp, ResponseInfo), FetchError> {
    let mut bytes = cache
        .read_stream(entry, 0, MAX_INFO_BYTES)
        .map_err(cache_error)?;
    let info = response_info::parse(&bytes);
    // Best effort, like the credentials file's buffer: the headers may hold cookies.
    bytes.fill(0);
    std::hint::black_box(&bytes);
    let info = info.map_err(|e| FetchError::Parse {
        from: FROM,
        detail: format!("Claude Desktop's cached usage: {e}"),
    })?;
    let unusable = |why: &str| FetchError::Unavailable {
        detail: format!("Claude Desktop's cached usage response is {why}"),
    };
    if info.truncated {
        return Err(unusable("incomplete"));
    }
    match info.status {
        Some(200..=299) => {}
        Some(status) => return Err(unusable(&format!("HTTP {status}"))),
        None => return Err(unusable("without a status")),
    }
    let at = info
        .response_time
        .ok_or_else(|| unusable("without a time"))?;
    Ok((at, info))
}

/// The usage in a usage entry's body.
fn usage_of(
    cache: &mut BlockfileCache<'_>,
    entry: &Entry,
    info: &ResponseInfo,
) -> Result<ClaudeUsage, FetchError> {
    let stored = cache
        .read_stream(entry, 1, MAX_STORED_BODY)
        .map_err(cache_error)?;
    let body = response_info::decode_body(&stored, info, MAX_DECODED_BODY).map_err(|e| {
        FetchError::Parse {
            from: FROM,
            detail: format!("Claude Desktop's cached usage: {e}"),
        }
    })?;
    oauth::parse_from(&body, FROM)
}

/// A cache failure as a fetch failure (texts name no path).
fn cache_error(error: CacheError) -> FetchError {
    match error {
        CacheError::Missing => FetchError::Unavailable {
            detail: "Claude Desktop's HTTP cache not found".to_owned(),
        },
        CacheError::SimpleBackend => FetchError::Unavailable {
            detail: "Claude Desktop's HTTP cache uses Chromium's simple format, which Lastdrop \
                     does not read"
                .to_owned(),
        },
        CacheError::Io(std::io::ErrorKind::NotFound) => FetchError::Unavailable {
            detail: "Claude Desktop's HTTP cache not found".to_owned(),
        },
        e @ (CacheError::Unsupported(_) | CacheError::TooLarge(_)) => FetchError::Parse {
            from: FROM,
            detail: format!("Claude Desktop's HTTP cache: {e}"),
        },
        e @ (CacheError::Corrupt(_) | CacheError::NotWritten | CacheError::Io(_)) => {
            FetchError::Transient {
                from: FROM,
                detail: format!("Claude Desktop's HTTP cache: {e}"),
                retry_after: None,
            }
        }
    }
}
