//! A tiny file logger for the `log` facade (no heavy logging stacks): one line per record to
//! `%LOCALAPPDATA%\Lastdrop\logs\lastdrop.log`, rotated by size, local only, never sent anywhere.
//!
//! **Never log a secret** (ARCHITECTURE): callers log no tokens, keys, cookies, account ids or
//! e-mail addresses; [`redact`] masks known shapes as a second line of defence on every line.
//!
//! Threading: any thread may log, **including the readout thread**, which must never wait on a
//! lock held by another thread or do I/O (ARCHITECTURE "Threads"). So `log` calls only format,
//! redact and push the line into an unbounded `std::sync::mpsc` channel (no file access, no lock
//! that another thread holds across I/O); one writer thread owns the file, writes, rotates and
//! flushes. The writer is joined at shutdown after the channel closes.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::borrow::Cow;
use std::path::Path;

/// Rotate when the current file reaches 1 MiB.
pub const MAX_FILE_BYTES: u64 = 1 << 20;

/// Files kept: `lastdrop.log`, `lastdrop.1.log`, `lastdrop.2.log`.
pub const KEEP_FILES: usize = 3;

/// The log file name.
pub const FILE_NAME: &str = "lastdrop.log";

/// Installs the logger at `level` (`log::set_boxed_logger`) and starts its writer thread, which
/// writes into `dir`.
pub fn init(dir: &Path, level: log::LevelFilter) -> std::io::Result<()> {
    todo!("M1: file logger")
}

/// Masks known secret shapes in `line`: `Bearer <token>`, `sk-ant-…`, JWT-like `eyJ…` runs,
/// `api_key=…`/`"accessToken": "…"` values, e-mail addresses, and long hex/base64 runs (≥ 32
/// characters). Returns the line unchanged (borrowed) when nothing matched.
pub fn redact(line: &str) -> Cow<'_, str> {
    todo!("M1: redaction")
}

#[cfg(test)]
mod tests {
    use super::redact;

    // Invented secret-shaped strings only.
    #[test]
    #[ignore = "M1 implementation"]
    fn masks_known_shapes() {
        assert_eq!(
            redact("Authorization: Bearer test-token-not-real-0123456789"),
            "Authorization: Bearer [redacted]"
        );
        assert!(!redact("key sk-ant-test-0000000000000000000000000000").contains("sk-ant-test"));
        assert!(
            !redact("user tester@example.invalid signed in").contains("tester@example.invalid")
        );
        assert_eq!(
            redact("Claude 5-hour 42% at 14:56"),
            "Claude 5-hour 42% at 14:56"
        );
    }
}
