//! Source 3: Claude Code's cached usage in `.claude.json` (read-only; the file also holds
//! account details, which are never logged or kept).
//!
//! Claude Code stores the last body of the usage endpoint it fetched as
//! `cachedUsageUtilization: {fetchedAtMs, accountUuid, utilization: {…the usage body…}}`. The copy
//! can be days old (Claude Code refreshes it only when it fetches usage itself), so readings from
//! here are judged by their own `fetchedAtMs`.

use std::io::Read;
use std::path::Path;

use jiff::Timestamp;
use lastdrop_core::model::SourceKind;
use serde::Deserialize;
use serde_json::Value;

use super::json::{UsageBody, instant_of, json_detail};
use super::{ClaudeUsage, file_error};
use crate::FetchContext;
use crate::error::FetchError;

/// Largest `.claude.json` read (it grows with project history). Read as a stream
/// ([`crate::io::Fs::open`]), never whole: memory stays flat whatever its size.
pub const MAX_BYTES: u64 = 64 << 20;

const FROM: SourceKind = SourceKind::ClaudeCodeCache;
const NAME: &str = ".claude.json";

/// Reads the cached usage from `state_file` (`AccountSource::Claude::state_file`) through
/// `ctx.fs.open` and [`parse`]. `FetchError::Unavailable` when the file or the cached usage is
/// missing.
pub fn read(ctx: &FetchContext<'_>, state_file: &Path) -> Result<ClaudeUsage, FetchError> {
    let meta = ctx
        .fs
        .metadata(state_file)
        .map_err(|e| file_error(FROM, &e, NAME))?;
    let reader = ctx
        .fs
        .open(state_file, MAX_BYTES)
        .map_err(|e| file_error(FROM, &e, NAME))?;
    parse(reader, meta.modified)
}

/// Parses the cached usage as a stream (`serde_json::from_reader` into a struct that keeps only
/// the usage fields and skips everything else with `IgnoredAny`, so project history and account
/// details are never held in memory); `measured_at` is the cache's own time when present, else
/// the file's modification time `modified`. Tests pass a byte slice.
///
/// Tolerant: `utilization` may be missing, in which case the cached object itself is read as the
/// usage body (an older layout), and unknown keys anywhere are ignored. `FetchError::Unavailable`
/// when there is no cached usage; `FetchError::Parse` when the file is not JSON or the cached
/// usage holds no R§2 window.
pub fn parse(reader: impl Read, modified: Option<Timestamp>) -> Result<ClaudeUsage, FetchError> {
    let state: StateFile = serde_json::from_reader(reader).map_err(|e| {
        if e.is_io() {
            FetchError::Transient {
                from: FROM,
                detail: format!("cannot read {NAME}"),
                retry_after: None,
            }
        } else {
            FetchError::Parse {
                from: FROM,
                detail: json_detail(NAME, &e),
            }
        }
    })?;
    let Some(Value::Object(mut cached)) = state.cached else {
        return Err(FetchError::Unavailable {
            detail: format!("no cached usage in {NAME}"),
        });
    };
    let measured_at = cached
        .get("fetchedAtMs")
        .or_else(|| cached.get("fetchedAt"))
        .and_then(instant_of)
        .or(modified);
    // `accountUuid` stays in `cached` and is dropped with it.
    let body = match cached.remove("utilization") {
        Some(utilization @ Value::Object(_)) => utilization,
        _ => Value::Object(cached),
    };
    let body = UsageBody::deserialize(body).map_err(|_| FetchError::Parse {
        from: FROM,
        detail: format!("unexpected cached usage in {NAME}"),
    })?;
    let windows = body.windows();
    if windows.is_empty() {
        return Err(FetchError::Parse {
            from: FROM,
            detail: format!("no usage windows in the cached usage of {NAME}"),
        });
    }
    Ok(ClaudeUsage {
        windows,
        plan: None,
        measured_at,
    })
}

/// The top level of `.claude.json`: everything but the cached usage is skipped while streaming.
#[derive(Deserialize)]
struct StateFile {
    #[serde(rename = "cachedUsageUtilization", default)]
    cached: Option<Value>,
}
