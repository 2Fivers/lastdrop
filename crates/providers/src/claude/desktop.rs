//! Source 1: Claude Desktop's `plan-usage-history.json` (no token needed).
//!
//! The file lives in Claude Desktop's data folder (`AccountSource::Claude::desktop_dir`). Its
//! exact location and shape are confirmed by the implementer against a real install; fixtures are
//! written by hand from that shape with invented values (tests/fixtures/claude/README.md).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::path::{Path, PathBuf};

use jiff::Timestamp;

use super::ClaudeUsage;
use crate::FetchContext;
use crate::error::FetchError;

/// File name of the history in the desktop data folder.
pub const FILE_NAME: &str = "plan-usage-history.json";

/// Largest file read (the history is small; anything bigger is not it).
pub const MAX_BYTES: u64 = 4 << 20;

/// The history file of a desktop data folder.
pub fn path(desktop_dir: &Path) -> PathBuf {
    desktop_dir.join(FILE_NAME)
}

/// Reads and parses the newest entry of the history. `FetchError::Unavailable` when the file does
/// not exist (Claude Desktop not installed or never opened).
pub fn read(ctx: &FetchContext<'_>, desktop_dir: &Path) -> Result<ClaudeUsage, FetchError> {
    todo!("M1: read plan-usage-history.json")
}

/// Parses the file; tolerant of unknown and missing fields. `fetched_at` stands in for
/// `measured_at` when the entry carries no time.
pub fn parse(bytes: &[u8], fetched_at: Timestamp) -> Result<ClaudeUsage, FetchError> {
    todo!("M1: parse plan-usage-history.json")
}
