//! Source 3: Claude Code's cached usage in `.claude.json` (read-only; the file also holds
//! account details, which are never logged or kept).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::io::Read;
use std::path::Path;

use jiff::Timestamp;

use super::ClaudeUsage;
use crate::FetchContext;
use crate::error::FetchError;

/// Largest `.claude.json` read (it grows with project history). Read as a stream
/// ([`crate::io::Fs::open`]), never whole: memory stays flat whatever its size.
pub const MAX_BYTES: u64 = 64 << 20;

/// Reads the cached usage from `state_file` (`AccountSource::Claude::state_file`) through
/// `ctx.fs.open` and [`parse`]. `FetchError::Unavailable` when the file or the cached usage is
/// missing.
pub fn read(ctx: &FetchContext<'_>, state_file: &Path) -> Result<ClaudeUsage, FetchError> {
    todo!("M1: read the cached usage")
}

/// Parses the cached usage as a stream (`serde_json::from_reader` into a struct that keeps only
/// the usage fields and skips everything else with `IgnoredAny`, so project history and account
/// details are never held in memory); `measured_at` is the cache's own time when present, else
/// the file's modification time `modified`. Tests pass a byte slice.
pub fn parse(reader: impl Read, modified: Option<Timestamp>) -> Result<ClaudeUsage, FetchError> {
    todo!("M1: parse the cached usage")
}
