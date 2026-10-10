//! Source 2: the newest rollout file, `<codex_home>\sessions\YYYY\MM\DD\rollout-*.jsonl`; the
//! last event with `token_count.rate_limits` in its tail (offline fallback, `measured_at` = the
//! event's time).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::path::{Path, PathBuf};

use super::CodexUsage;
use crate::FetchContext;
use crate::error::FetchError;

/// The sessions folder in `CODEX_HOME`.
pub const SESSIONS_DIR: &str = "sessions";

/// Rollout file name pattern.
pub const FILE_PATTERN: &str = "rollout-*.jsonl";

/// Bytes read from the end of the file; rollouts can be large, the newest event is at the end.
pub const TAIL_BYTES: u64 = 256 << 10;

/// The newest rollout file: walks `sessions\YYYY\MM\DD` newest first, then by modification time.
pub fn newest(ctx: &FetchContext<'_>, codex_home: &Path) -> Option<PathBuf> {
    todo!("M1: newest rollout file")
}

/// The last `token_count` event with `rate_limits` in `tail` (whole lines only; the first,
/// possibly cut line is skipped). `Ok(None)` when the tail has none.
pub fn parse_tail(tail: &[u8]) -> Result<Option<CodexUsage>, FetchError> {
    todo!("M1: parse rollout tail")
}

/// The whole source: newest file → tail → parse. `FetchError::Unavailable` without a file or
/// event.
pub fn fetch(ctx: &FetchContext<'_>, codex_home: &Path) -> Result<CodexUsage, FetchError> {
    todo!("M1: rollout source")
}
