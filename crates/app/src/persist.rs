//! Settings and folders (ARCHITECTURE "Persistence"): settings in
//! `%APPDATA%\Lastdrop\settings.json` (atomic write, no secrets); data in
//! `%LOCALAPPDATA%\Lastdrop\` (history, logs).
//!
//! Threading: main thread (settings are small; a save is one atomic file replace).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::path::{Path, PathBuf};

use lastdrop_core::settings::Settings;

/// Lastdrop's folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `%APPDATA%\Lastdrop\settings.json`.
    pub settings: PathBuf,
    /// `%LOCALAPPDATA%\Lastdrop`.
    pub data: PathBuf,
    /// `%LOCALAPPDATA%\Lastdrop\history`: one JSONL file per meter.
    pub history: PathBuf,
    /// `%LOCALAPPDATA%\Lastdrop\logs`: rotated, local only.
    pub logs: PathBuf,
}

impl Paths {
    /// The folders from `%APPDATA%` and `%LOCALAPPDATA%`, created when missing.
    pub fn from_env() -> std::io::Result<Paths> {
        todo!("M1: resolve and create the folders")
    }
}

/// What [`load_settings`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadOutcome {
    /// Read as is.
    Loaded,
    /// No file yet: first run, defaults.
    Missing,
    /// An older schema version was migrated (the file is rewritten on the next save).
    Migrated {
        /// The version found.
        from: u32,
    },
    /// The file could not be read or parsed: it was renamed to `backup` and defaults are used,
    /// so the user's file is never overwritten silently.
    Corrupt {
        /// Where the unreadable file went (`settings.json.bad-<timestamp>`).
        backup: PathBuf,
    },
}

/// Loads settings; never fails (defaults on any problem, see [`LoadOutcome`]). Unknown fields are
/// kept (core's schema flattens them).
pub fn load_settings(path: &Path) -> (Settings, LoadOutcome) {
    todo!("M1: load + migrate settings")
}

/// Migrates a settings document of an older version to [`lastdrop_core::settings::SETTINGS_VERSION`].
pub fn migrate(document: serde_json::Value) -> Result<Settings, serde_json::Error> {
    todo!("M1: settings migration")
}

/// Saves settings atomically ([`write_atomic`], pretty JSON).
pub fn save_settings(path: &Path, settings: &Settings) -> std::io::Result<()> {
    todo!("M1: save settings")
}

/// Writes `bytes` to `path` atomically: a temporary file in the same folder, flushed, then
/// `ReplaceFileW` (or `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)` when the
/// target does not exist yet).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    todo!("M1: atomic write")
}
