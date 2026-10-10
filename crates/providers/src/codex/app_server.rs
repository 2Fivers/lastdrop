//! Source 1: `codex app-server` (JSON-RPC 2.0, one JSON object per line over stdio).
//!
//! Sequence: spawn (no window, `CODEX_HOME` set) → `initialize` request (8 s) → `initialized`
//! notification → `account/rateLimits/read` (3 s) → kill. Lines that are not the awaited response
//! (notifications, logs) are skipped. The app-server renews Codex's own sign-in; Lastdrop never
//! touches `auth.json`.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::CodexUsage;
use crate::FetchContext;
use crate::error::FetchError;
use crate::io::SpawnSpec;

/// Time allowed for `initialize` (ARCHITECTURE).
pub const INIT_TIMEOUT: Duration = Duration::from_secs(8);

/// Time allowed per call after initialization (ARCHITECTURE).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(3);

/// The rate-limits method.
pub const METHOD_RATE_LIMITS: &str = "account/rateLimits/read";

/// Client name sent in `initialize` (`clientInfo.name`).
pub const CLIENT_NAME: &str = "lastdrop";

/// Finds `codex.exe`: the settings override when given and present, else the newest bundled
/// one (e.g. inside the Codex VS Code extension or desktop app folders), else the first on
/// `PATH`. `None` when there is none.
pub fn find_codex_exe(ctx: &FetchContext<'_>, override_path: Option<&Path>) -> Option<PathBuf> {
    todo!("M1: locate codex.exe")
}

/// `codex app-server` with `CODEX_HOME` = `codex_home`.
pub fn spawn_spec(exe: &Path, codex_home: &Path) -> SpawnSpec {
    todo!("M1: app-server spawn spec")
}

/// The `initialize` request line with JSON-RPC id `id`.
pub fn initialize_request(id: u64) -> String {
    todo!("M1: initialize request")
}

/// The `initialized` notification line.
pub fn initialized_notification() -> String {
    todo!("M1: initialized notification")
}

/// The `account/rateLimits/read` request line with id `id`.
pub fn rate_limits_request(id: u64) -> String {
    todo!("M1: rate limits request")
}

/// Parses one stdout line: `Ok(None)` when it is not the response to `id`; the usage when it is;
/// an error response maps to Auth (not signed in) or Transient.
pub fn parse_rate_limits(line: &str, id: u64) -> Result<Option<CodexUsage>, FetchError> {
    todo!("M1: parse rate limits response")
}

/// The whole source: find → spawn → initialize → read → kill, within the timeouts and the
/// fetch deadline.
pub fn fetch(
    ctx: &FetchContext<'_>,
    codex_home: &Path,
    override_exe: Option<&Path>,
) -> Result<CodexUsage, FetchError> {
    todo!("M1: app-server source")
}
