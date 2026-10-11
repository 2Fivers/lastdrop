//! Settings and folders (ARCHITECTURE "Persistence"): settings in
//! `%APPDATA%\Lastdrop\settings.json` (atomic write, no secrets); data in
//! `%LOCALAPPDATA%\Lastdrop\` (history, logs).
//!
//! Threading: loading runs on the main thread at start-up; saves are encoded on the main thread
//! and written by the writer thread ([`crate::writer`], [`write_atomic`]).

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use lastdrop_core::settings::{SETTINGS_VERSION, Settings};
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW,
};
use windows::core::HSTRING;

/// The folder name under `%APPDATA%` and `%LOCALAPPDATA%`.
pub const APP_FOLDER: &str = "Lastdrop";

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
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("%{name}% is not set"),
                    )
                })
        };
        let paths = Paths::under(var("APPDATA")?, var("LOCALAPPDATA")?);
        paths.create()?;
        Ok(paths)
    }

    /// The folder layout under a roaming and a local application-data folder (no I/O).
    pub fn under(appdata: impl Into<OsString>, local_appdata: impl Into<OsString>) -> Paths {
        let roaming = PathBuf::from(appdata.into()).join(APP_FOLDER);
        let data = PathBuf::from(local_appdata.into()).join(APP_FOLDER);
        Paths {
            settings: roaming.join("settings.json"),
            history: data.join("history"),
            logs: data.join("logs"),
            data,
        }
    }

    /// Creates every folder (no error when they exist).
    pub fn create(&self) -> std::io::Result<()> {
        if let Some(dir) = self.settings.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::create_dir_all(&self.history)?;
        std::fs::create_dir_all(&self.logs)
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
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (Settings::default(), LoadOutcome::Missing);
        }
        Err(e) => {
            log::error!("settings unreadable: {e}");
            return (Settings::default(), set_aside(path));
        }
    };
    let parsed = serde_json::from_slice::<serde_json::Value>(&bytes).and_then(|doc| {
        let version = version_of(&doc);
        if version < SETTINGS_VERSION {
            migrate(doc).map(|s| (s, LoadOutcome::Migrated { from: version }))
        } else {
            // The current version, or a newer build's file: unknown fields are kept.
            serde_json::from_value::<Settings>(doc).map(|s| (s, LoadOutcome::Loaded))
        }
    });
    match parsed {
        Ok(loaded) => loaded,
        Err(e) => {
            log::error!("settings not understood ({e}); using defaults");
            (Settings::default(), set_aside(path))
        }
    }
}

/// The `version` of a settings document; a document without one is version 1 (the first
/// schema, which always wrote it).
fn version_of(doc: &serde_json::Value) -> u32 {
    doc.get("version")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(1)
}

/// Renames an unreadable settings file to `settings.json.bad-<unix seconds>` (copies it when the
/// rename fails) so the next save cannot overwrite it.
fn set_aside(path: &Path) -> LoadOutcome {
    let stamp = jiff::Timestamp::now().as_second();
    let mut name = path.file_name().map(OsString::from).unwrap_or_default();
    name.push(format!(".bad-{stamp}"));
    let backup = path.with_file_name(name);
    if let Err(e) = std::fs::rename(path, &backup) {
        log::error!("cannot set the settings file aside ({e}); copying it");
        if let Err(e) = std::fs::copy(path, &backup) {
            log::error!("cannot copy the settings file either: {e}");
        }
    }
    LoadOutcome::Corrupt { backup }
}

/// Migrates a settings document of an older version to [`lastdrop_core::settings::SETTINGS_VERSION`].
///
/// Version 1 is the first schema, so there is nothing to convert yet: the document is read with
/// the current schema's defaults for missing fields and stamped with the current version. Later
/// versions add their steps here, oldest first.
pub fn migrate(document: serde_json::Value) -> Result<Settings, serde_json::Error> {
    let mut settings: Settings = serde_json::from_value(document)?;
    settings.version = SETTINGS_VERSION;
    Ok(settings)
}

/// The settings file's bytes (pretty JSON), for the writer thread.
pub fn settings_bytes(settings: &Settings) -> std::io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Saves settings atomically ([`write_atomic`], pretty JSON) on the calling thread.
pub fn save_settings(path: &Path, settings: &Settings) -> std::io::Result<()> {
    write_atomic(path, &settings_bytes(settings)?)
}

/// Writes `bytes` to `path` atomically: a temporary file in the same folder, flushed, then
/// `ReplaceFileW` (or `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)` when the
/// target does not exist yet).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp_name = path
        .file_name()
        .map(OsString::from)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let target = HSTRING::from(path.as_os_str());
    let source = HSTRING::from(tmp.as_os_str());
    let replaced = if path.exists() {
        // SAFETY: both strings are valid NUL-terminated wide strings that outlive the call; no
        // backup file, no reserved pointers.
        unsafe {
            ReplaceFileW(
                &target,
                &source,
                None,
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                None,
                None,
            )
        }
        .or_else(|_| {
            // ReplaceFileW can refuse (e.g. the target is open without sharing delete); a
            // replacing move is still atomic on NTFS.
            // SAFETY: as above.
            unsafe {
                MoveFileExW(
                    &source,
                    &target,
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
        })
    } else {
        // SAFETY: both strings are valid NUL-terminated wide strings that outlive the call.
        unsafe {
            MoveFileExW(
                &source,
                &target,
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    replaced.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        std::io::Error::other(e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lastdrop_core::settings::ReadoutContent;

    /// A fresh folder under the system temp folder for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-test-{name}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn layout_under_the_app_data_folders() {
        let p = Paths::under(
            r"C:\Users\tester\AppData\Roaming",
            r"C:\Users\tester\AppData\Local",
        );
        assert_eq!(
            p.settings,
            PathBuf::from(r"C:\Users\tester\AppData\Roaming\Lastdrop\settings.json")
        );
        assert_eq!(
            p.history,
            PathBuf::from(r"C:\Users\tester\AppData\Local\Lastdrop\history")
        );
        assert_eq!(
            p.logs,
            PathBuf::from(r"C:\Users\tester\AppData\Local\Lastdrop\logs")
        );
    }

    #[test]
    fn missing_then_saved_then_loaded() {
        let dir = temp_dir("settings");
        let path = dir.join("settings.json");
        let (defaults, outcome) = load_settings(&path);
        assert_eq!(outcome, LoadOutcome::Missing);
        assert_eq!(defaults, Settings::default());

        let mut s = Settings::default();
        s.readout.enabled = false;
        s.readout.content = ReadoutContent::Auto;
        save_settings(&path, &s).expect("first save (move)");
        s.start_with_windows = true;
        save_settings(&path, &s).expect("second save (replace)");
        let (back, outcome) = load_settings(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, s);
        // No temporary file is left behind.
        let names: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(names, [OsString::from("settings.json")]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_set_aside_not_overwritten() {
        let dir = temp_dir("corrupt");
        let path = dir.join("settings.json");
        std::fs::write(&path, b"{ not json").expect("write");
        let (s, outcome) = load_settings(&path);
        assert_eq!(s, Settings::default());
        let LoadOutcome::Corrupt { backup } = outcome else {
            panic!("corrupt expected");
        };
        assert!(!path.exists());
        assert_eq!(std::fs::read(&backup).expect("backup"), b"{ not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn older_versions_migrate_and_unknown_fields_survive() {
        let doc = serde_json::json!({ "version": 0, "start_with_windows": true, "future": 7 });
        let s = migrate(doc).expect("migrates");
        assert_eq!(s.version, SETTINGS_VERSION);
        assert!(s.start_with_windows);
        assert_eq!(s.extra["future"], 7);

        let dir = temp_dir("migrate");
        let path = dir.join("settings.json");
        std::fs::write(&path, br#"{ "version": 0 }"#).expect("write");
        assert_eq!(load_settings(&path).1, LoadOutcome::Migrated { from: 0 });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
