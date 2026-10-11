//! Source 2: the newest rollout file, `<codex_home>\sessions\YYYY\MM\DD\rollout-*.jsonl`; the
//! last event with `token_count.rate_limits` in its tail (offline fallback, `measured_at` = the
//! event's time).
//!
//! Line shape (Codex ≥ 0.48): `{"timestamp": RFC 3339, "type": "event_msg", "payload": {"type":
//! "token_count", "info": …, "rate_limits": {"limit_id": "codex", "primary": {"used_percent",
//! "window_minutes", "resets_at" (Unix s)} | null, "secondary": …, "plan_type": …} | null}}`.
//! Older shapes still read: 0.44–0.47 wrote `resets_in_seconds` (relative to the line's
//! `timestamp`) instead of `resets_at`; ≤ 0.42 wrote a flat `{primary_used_percent,
//! primary_window_minutes, secondary_used_percent, secondary_window_minutes}` without reset times.
//! Events with `rate_limits: null` and other buckets (`limit_id: "premium"`, model-specific ones)
//! are skipped. A `null` `plan_type` is not a plan change: the plan comes from the newest earlier
//! event in the tail that names one.
//!
//! Compressed rollouts (`.jsonl.zst`) are not read: Codex only compresses files older than a
//! week, which never hold the newest event.

use std::path::{Path, PathBuf};

use jiff::Timestamp;
use lastdrop_core::model::SourceKind;
use serde_json::Value;

use super::{CodexUsage, CodexWindow, plan_name, window_from};
use crate::FetchContext;
use crate::error::FetchError;
use crate::io::DirEntry;

/// The sessions folder in `CODEX_HOME`.
pub const SESSIONS_DIR: &str = "sessions";

/// Rollout file name pattern.
pub const FILE_PATTERN: &str = "rollout-*.jsonl";

/// Bytes read from the end of the file; rollouts can be large, the newest event is at the end.
pub const TAIL_BYTES: u64 = 256 << 10;

/// Day folders searched, newest first. A resumed session keeps writing to the file in the
/// folder of the day it started, so the newest event can be in an older folder.
pub const MAX_DAY_DIRS: usize = 31;

/// Rollout files whose tails are read per fetch, newest modification first (a just-started
/// session has no `token_count` event yet).
pub const MAX_FILES_READ: usize = 8;

/// The rate-limit bucket of the plan's own windows.
const CODEX_LIMIT_ID: &str = "codex";

/// Numeric folder names (`2026`, `10`, `09`) under `dir`, highest first.
fn numbered_dirs(ctx: &FetchContext<'_>, dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = ctx.fs.list(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<(u32, PathBuf)> = entries
        .into_iter()
        .filter(|e| e.meta.is_dir)
        .filter_map(|e| {
            let n = e.path.file_name()?.to_str()?.parse::<u32>().ok()?;
            Some((n, e.path))
        })
        .collect();
    dirs.sort_by(|a, b| b.cmp(a));
    dirs.into_iter().map(|(_, p)| p).collect()
}

/// Rollout files of the newest [`MAX_DAY_DIRS`] day folders, newest modification first (ties:
/// path, descending).
pub fn candidates(ctx: &FetchContext<'_>, codex_home: &Path) -> Vec<DirEntry> {
    let sessions = codex_home.join(SESSIONS_DIR);
    let mut days = Vec::new();
    'walk: for year in numbered_dirs(ctx, &sessions) {
        for month in numbered_dirs(ctx, &year) {
            for day in numbered_dirs(ctx, &month) {
                days.push(day);
                if days.len() >= MAX_DAY_DIRS {
                    break 'walk;
                }
            }
        }
    }
    let mut files: Vec<DirEntry> = days
        .iter()
        .filter_map(|day| ctx.fs.glob(day, FILE_PATTERN).ok())
        .flatten()
        .filter(|e| !e.meta.is_dir)
        .collect();
    files.sort_by(|a, b| {
        b.meta
            .modified
            .cmp(&a.meta.modified)
            .then_with(|| b.path.cmp(&a.path))
    });
    files
}

/// The newest rollout file: walks `sessions\YYYY\MM\DD` newest first, then by modification time.
pub fn newest(ctx: &FetchContext<'_>, codex_home: &Path) -> Option<PathBuf> {
    candidates(ctx, codex_home)
        .into_iter()
        .next()
        .map(|e| e.path)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// A `token_count` event of the plan's own bucket: the line's time and its `rate_limits`.
struct RateLimitsEvent {
    at: Option<Timestamp>,
    rate_limits: Value,
}

fn rate_limits_event(line: &[u8]) -> Option<RateLimitsEvent> {
    // Cheap filter before parsing: most lines are conversation items.
    if !contains(line, b"token_count") || !contains(line, b"rate_limits") {
        return None;
    }
    let mut v: Value = serde_json::from_slice(line.trim_ascii()).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = v.get_mut("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let rate_limits = payload.get_mut("rate_limits").map(Value::take)?;
    if !rate_limits.is_object() {
        return None;
    }
    if let Some(id) = rate_limits.get("limit_id").and_then(Value::as_str)
        && id != CODEX_LIMIT_ID
    {
        return None;
    }
    let at = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<Timestamp>().ok());
    Some(RateLimitsEvent { at, rate_limits })
}

fn plan_of(rate_limits: &Value) -> Option<String> {
    rate_limits
        .get("plan_type")
        .and_then(Value::as_str)
        .and_then(plan_name)
}

/// The windows of one event; `Err(())` when it has window data in no readable shape.
fn usage_of(event: &RateLimitsEvent) -> Result<Option<CodexUsage>, ()> {
    let rl = &event.rate_limits;
    let (primary, secondary, had_data) = if rl.get("primary_used_percent").is_some() {
        // ≤ 0.42: flat, no reset times.
        let flat = |used: &str, mins: &str| -> Option<CodexWindow> {
            let used_percent = rl.get(used).and_then(super::number)?;
            Some(CodexWindow {
                used_percent,
                window_minutes: rl.get(mins).and_then(super::minutes),
                resets_at: None,
            })
        };
        (
            flat("primary_used_percent", "primary_window_minutes"),
            flat("secondary_used_percent", "secondary_window_minutes"),
            true,
        )
    } else {
        let keys = ("used_percent", "window_minutes", "resets_at");
        let relative = Some(("resets_in_seconds", event.at));
        let window = |key: &str| rl.get(key).and_then(|w| window_from(w, keys, relative));
        let had_data = ["primary", "secondary"]
            .iter()
            .any(|k| rl.get(*k).is_some_and(|w| !w.is_null()));
        (window("primary"), window("secondary"), had_data)
    };
    if primary.is_none() && secondary.is_none() {
        return if had_data { Err(()) } else { Ok(None) };
    }
    Ok(Some(CodexUsage {
        primary,
        secondary,
        plan: plan_of(rl),
        measured_at: event.at,
    }))
}

/// [`parse_tail`], optionally keeping the first line (when the tail is the whole file).
fn parse_lines(tail: &[u8], skip_first: bool) -> Result<Option<CodexUsage>, FetchError> {
    let mut lines: Vec<&[u8]> = tail.split(|&b| b == b'\n').collect();
    if skip_first && !lines.is_empty() {
        lines.remove(0);
    }
    let mut found: Option<CodexUsage> = None;
    let mut unreadable = false;
    for line in lines.iter().rev() {
        let Some(event) = rate_limits_event(line) else {
            continue;
        };
        match found.as_mut() {
            None => match usage_of(&event) {
                Ok(Some(usage)) => {
                    let has_plan = usage.plan.is_some();
                    found = Some(usage);
                    if has_plan {
                        break;
                    }
                }
                Ok(None) => {}
                Err(()) => unreadable = true,
            },
            // Found the windows; only looking back for the plan.
            Some(usage) => {
                if let Some(plan) = plan_of(&event.rate_limits) {
                    usage.plan = Some(plan);
                    break;
                }
            }
        }
    }
    match found {
        Some(usage) => Ok(Some(usage)),
        None if unreadable => Err(FetchError::Parse {
            from: SourceKind::CodexRollout,
            detail: "rollout rate_limits in an unknown shape".to_owned(),
        }),
        None => Ok(None),
    }
}

/// The last `token_count` event with `rate_limits` in `tail` (whole lines only; the first,
/// possibly cut line is skipped). `Ok(None)` when the tail has none.
///
/// `FetchError::Parse` when the tail has rate-limit events of the `codex` bucket but none in a
/// readable shape (the format changed).
pub fn parse_tail(tail: &[u8]) -> Result<Option<CodexUsage>, FetchError> {
    parse_lines(tail, true)
}

/// The whole source: newest file → tail → parse. `FetchError::Unavailable` without a file or
/// event.
///
/// Reads the tails of up to [`MAX_FILES_READ`] files, newest modification first, and keeps the
/// newest event; stops as soon as the next file was last written before that event. An event
/// without a `timestamp` is dated by its file's modification time.
pub fn fetch(ctx: &FetchContext<'_>, codex_home: &Path) -> Result<CodexUsage, FetchError> {
    read_files(ctx, &candidates(ctx, codex_home)).0
}

/// A rollout read remembered with the files it could look at: the newest [`MAX_FILES_READ`]
/// rollout files' paths, sizes and modification times. Paths and the usage only, never file
/// contents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    files: Vec<(PathBuf, u64, Timestamp)>,
    result: Option<Result<CodexUsage, FetchError>>,
}

/// [`fetch`] that reads no tail when the newest [`MAX_FILES_READ`] files are the same files,
/// with the same sizes and modification times, as at the read `seen` remembers: the result is
/// then the remembered one (the read depends on nothing else). Otherwise the files are read and
/// `seen` remembers this read, unless a tail could not be read or a file has no modification
/// time (then the next fetch reads again).
pub fn fetch_seen(
    ctx: &FetchContext<'_>,
    codex_home: &Path,
    seen: &mut Seen,
) -> Result<CodexUsage, FetchError> {
    let files = candidates(ctx, codex_home);
    let looked_at: Option<Vec<(PathBuf, u64, Timestamp)>> = files
        .iter()
        .take(MAX_FILES_READ)
        .map(|f| Some((f.path.clone(), f.meta.len, f.meta.modified?)))
        .collect();
    if let (Some(looked_at), Some(result)) = (&looked_at, &seen.result)
        && *looked_at == seen.files
    {
        return result.clone();
    }
    let (result, complete) = read_files(ctx, &files);
    *seen = match looked_at {
        Some(files) if complete => Seen {
            files,
            result: Some(result.clone()),
        },
        _ => Seen::default(),
    };
    result
}

/// [`fetch`] over `files` (newest first); `false` beside the result when a tail could not be read.
fn read_files(
    ctx: &FetchContext<'_>,
    files: &[DirEntry],
) -> (Result<CodexUsage, FetchError>, bool) {
    if files.is_empty() {
        let none = FetchError::Unavailable {
            detail: "no Codex sessions".to_owned(),
        };
        return (Err(none), true);
    }
    let mut complete = true;
    let mut best: Option<CodexUsage> = None;
    let mut parse_error: Option<FetchError> = None;
    for file in files.iter().take(MAX_FILES_READ) {
        if let (Some(event_at), Some(written)) = (
            best.as_ref().and_then(|u| u.measured_at),
            file.meta.modified,
        ) && written < event_at
        {
            break;
        }
        let tail = match ctx.fs.read_tail(&file.path, TAIL_BYTES) {
            Ok(tail) => tail,
            Err(e) => {
                log::debug!("codex rollout: cannot read a session file: {e}");
                complete = false;
                continue;
            }
        };
        // A tail shorter than asked for is the whole file: its first line is complete.
        let whole_file = (tail.len() as u64) < TAIL_BYTES;
        match parse_lines(&tail, !whole_file) {
            Ok(Some(mut usage)) => {
                if usage.measured_at.is_none() {
                    usage.measured_at = file.meta.modified;
                }
                let newer = best
                    .as_ref()
                    .is_none_or(|b| usage.measured_at > b.measured_at);
                if newer {
                    best = Some(usage);
                }
            }
            Ok(None) => {}
            Err(e) => {
                parse_error.get_or_insert(e);
            }
        }
    }
    let result = best.ok_or_else(|| {
        parse_error.unwrap_or_else(|| FetchError::Unavailable {
            detail: "no rate limits in recent Codex sessions".to_owned(),
        })
    });
    (result, complete)
}
