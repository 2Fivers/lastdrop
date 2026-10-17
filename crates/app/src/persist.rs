//! Settings, the state file and folders (ARCHITECTURE "Persistence"): settings in
//! `%APPDATA%\Lastdrop\settings.json` (atomic write, no secrets); data in
//! `%LOCALAPPDATA%\Lastdrop\` (history, logs, `state.json`).
//!
//! Threading: loading runs on the main thread at start-up; the tray process's saves are written by
//! the writer thread ([`crate::writer`], [`write_atomic`]; settings rebased onto the file under the
//! settings lock, [`crate::settings_sync`]).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use lastdrop_core::model::{AccountIdentity, MeterKey, MeterNames, SourceKey, Unit};
use lastdrop_core::notify::{Announced, NotifyMemory};
use lastdrop_core::settings::{SETTINGS_VERSION, Settings};
use lastdrop_core::update::Version;
use serde::{Deserialize, Serialize};
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
    /// `%APPDATA%\Lastdrop\providers`: the user's described services (custom-services-spec §3).
    pub providers: PathBuf,
    /// `%APPDATA%\Lastdrop\themes`: the user's themes (themes-spec §3), one JSON file each.
    pub themes: PathBuf,
    /// `%APPDATA%\Lastdrop\prices.json`: the user's token prices over the bundled ones
    /// (tokens-spec §2); only read, never written.
    pub prices: PathBuf,
    /// `%LOCALAPPDATA%\Lastdrop`.
    pub data: PathBuf,
    /// `%LOCALAPPDATA%\Lastdrop\history`: one JSONL file per meter.
    pub history: PathBuf,
    /// `%LOCALAPPDATA%\Lastdrop\logs`: rotated, local only.
    pub logs: PathBuf,
    /// `%LOCALAPPDATA%\Lastdrop\state.json`: what the app remembers between runs that is not a
    /// setting ([`SavedState`]).
    pub state: PathBuf,
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
            providers: roaming.join("providers"),
            prices: roaming.join("prices.json"),
            themes: roaming.join("themes"),
            history: data.join("history"),
            logs: data.join("logs"),
            state: data.join("state.json"),
            data,
        }
    }

    /// Creates every folder (no error when they exist).
    pub fn create(&self) -> std::io::Result<()> {
        if let Some(dir) = self.settings.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Made so the user finds where described services go; not needed to run.
        let _ = std::fs::create_dir_all(&self.providers);
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
    match read_settings(path) {
        Ok(loaded) => loaded,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::InvalidData {
                log::error!("settings not understood ({e}); using defaults");
            } else {
                log::error!("settings unreadable: {e}");
            }
            (Settings::default(), set_aside(path))
        }
    }
}

/// Reads settings without side effects: defaults with [`LoadOutcome::Missing`] when there is no
/// file, [`LoadOutcome::Loaded`] or [`LoadOutcome::Migrated`] otherwise, and an error
/// (`InvalidData` when the file is not understood) that leaves the file where it is. Services
/// added since the file was written get an entry, turned off (`Settings::add_new_providers`). The tray
/// process re-reads with it when the dialog process says the file changed; [`load_settings`]
/// adds setting a broken file aside.
pub fn read_settings(path: &Path) -> std::io::Result<(Settings, LoadOutcome)> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok((Settings::default(), LoadOutcome::Missing));
        }
        Err(e) => return Err(e),
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .and_then(|doc| {
            let version = version_of(&doc);
            if version < SETTINGS_VERSION {
                migrate(doc).map(|s| (s, LoadOutcome::Migrated { from: version }))
            } else {
                // The current version, or a newer build's file: unknown fields are kept.
                serde_json::from_value::<Settings>(doc).map(|s| (s, LoadOutcome::Loaded))
            }
        })
        .map(|(mut settings, outcome)| {
            if settings.add_new_providers() {
                log::info!("settings: added the services this build reads, turned off");
            }
            (settings, outcome)
        })
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
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

/// The settings file's bytes (pretty JSON).
pub fn settings_bytes(settings: &Settings) -> std::io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Saves settings atomically ([`write_atomic`], pretty JSON) on the calling thread; callers hold
/// the settings lock ([`crate::settings_sync::SettingsFile::update`]).
pub fn save_settings(path: &Path, settings: &Settings) -> std::io::Result<()> {
    write_atomic(path, &settings_bytes(settings)?)
}

/// The version of the state file's schema.
pub const STATE_VERSION: u32 = 1;

/// The state file (`%LOCALAPPDATA%\Lastdrop\state.json`): what the app remembers between runs
/// that is not a setting. A cache: an unreadable file is ignored and rewritten. **No secrets**:
/// meter keys (service, the account's key — `default` or `f-…`, never an e-mail address or an
/// account id — and the meter id), the meters' display names, what notifications announced
/// (levels, severities and reset times), who each source was last signed in to, as the hash of
/// `lastdrop_core::model::AccountIdentity` (accounts-spec §5), and the release version an update
/// notification announced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedState {
    /// Schema version ([`STATE_VERSION`]).
    pub version: u32,
    /// Every meter a source has reported (`lastdrop_core::stats::EvalMemory::seen`), so a meter
    /// that a later reading lacks still keeps its room in the readout after a restart (R§6.3).
    pub seen: Vec<SeenMeter>,
    /// What notifications announced per meter (`lastdrop_core::notify::NotifyMemory`), so a
    /// restart repeats nothing (N§1). Missing in a file of an older build: nothing announced.
    pub notified: Vec<NotifiedMeter>,
    /// Who each source was last known to be signed in to (accounts-spec §3, §5), so a restart
    /// knows the duplicates before the first job and notices a folder signed in to someone else
    /// meanwhile. Missing in a file of an older build: nothing known.
    pub identities: Vec<SavedIdentity>,
    /// The release version an update notification announced last (`0.2.0`, updates-spec §1), so
    /// a restart does not announce it again. Missing in a file of an older build, or not a
    /// version: nothing announced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_announced: Option<String>,
}

impl Default for SavedState {
    fn default() -> SavedState {
        SavedState {
            version: STATE_VERSION,
            seen: Vec::new(),
            notified: Vec::new(),
            identities: Vec::new(),
            update_announced: None,
        }
    }
}

/// One entry of [`SavedState::identities`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedIdentity {
    /// The source.
    #[serde(flatten)]
    pub source: SourceKey,
    /// [`AccountIdentity`]'s hex digits; an entry that is not one is ignored, never the whole
    /// file.
    pub identity: String,
}

/// Who each source was last known to be signed in to, as kept in memory.
pub type IdentityMap = BTreeMap<SourceKey, AccountIdentity>;

/// One entry of [`SavedState::notified`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotifiedMeter {
    /// The meter.
    #[serde(flatten)]
    pub key: MeterKey,
    /// What was announced.
    pub announced: Announced,
}

/// One entry of [`SavedState::seen`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenMeter {
    /// The meter.
    #[serde(flatten)]
    pub key: MeterKey,
    /// Its names as last reported.
    pub names: MeterNames,
    /// It is a balance meter.
    #[serde(default)]
    pub balance: bool,
    /// A balance's unit (charts-spec §4). Missing in a file of an older build: not known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<Unit>,
}

/// `EvalMemory::units` as kept in memory.
pub type UnitMap = BTreeMap<MeterKey, Unit>;

/// `EvalMemory::seen` as kept in memory.
pub type SeenMap = BTreeMap<MeterKey, (MeterNames, bool)>;

impl SavedState {
    /// The state of `seen` (with the `units` of its balances), `notified` and `identities`, with
    /// no update announced.
    pub fn new(
        seen: &SeenMap,
        units: &UnitMap,
        notified: &NotifyMemory,
        identities: &IdentityMap,
    ) -> SavedState {
        SavedState {
            version: STATE_VERSION,
            seen: seen
                .iter()
                .map(|(key, (names, balance))| SeenMeter {
                    key: key.clone(),
                    names: names.clone(),
                    balance: *balance,
                    unit: units.get(key).filter(|_| *balance).cloned(),
                })
                .collect(),
            notified: notified
                .iter()
                .map(|(key, announced)| NotifiedMeter {
                    key: key.clone(),
                    announced: *announced,
                })
                .collect(),
            identities: identities
                .iter()
                .map(|(source, identity)| SavedIdentity {
                    source: source.clone(),
                    identity: identity.as_str().to_owned(),
                })
                .collect(),
            update_announced: None,
        }
    }

    /// [`SavedState::update_announced`] as kept in memory.
    pub fn update_announced(&self) -> Option<Version> {
        self.update_announced.as_deref().and_then(Version::parse)
    }

    /// [`SavedState::identities`] as kept in memory, without entries that are not an identity.
    pub fn identity_map(&self) -> IdentityMap {
        self.identities
            .iter()
            .filter_map(|e| {
                let identity = AccountIdentity::try_from(e.identity.clone()).ok()?;
                Some((e.source.clone(), identity))
            })
            .collect()
    }

    /// [`SavedState::seen`] as kept in memory.
    pub fn seen_map(&self) -> SeenMap {
        self.seen
            .iter()
            .map(|m| (m.key.clone(), (m.names.clone(), m.balance)))
            .collect()
    }

    /// The units of [`SavedState::seen`]'s balances, as kept in memory.
    pub fn units_map(&self) -> UnitMap {
        self.seen
            .iter()
            .filter_map(|m| Some((m.key.clone(), m.unit.clone()?)))
            .collect()
    }

    /// [`SavedState::notified`] as kept in memory.
    pub fn notified_map(&self) -> NotifyMemory {
        self.notified
            .iter()
            .map(|m| (m.key.clone(), m.announced))
            .collect()
    }
}

/// Loads the state file; never fails: a missing or unreadable file gives the empty state (it is
/// a cache, rewritten on the next change).
pub fn load_state(path: &Path) -> SavedState {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return SavedState::default(),
        Err(e) => {
            log::warn!("state file unreadable: {e}");
            return SavedState::default();
        }
    };
    match serde_json::from_slice::<SavedState>(&bytes) {
        Ok(state) => state,
        Err(e) => {
            log::warn!("state file not understood ({e}); starting empty");
            SavedState::default()
        }
    }
}

/// The state file's bytes (pretty JSON), for the writer thread.
pub fn state_bytes(state: &SavedState) -> std::io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(state).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
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
        assert_eq!(
            p.state,
            PathBuf::from(r"C:\Users\tester\AppData\Local\Lastdrop\state.json")
        );
    }

    #[test]
    fn the_state_file_round_trips_and_tolerates_damage() {
        use lastdrop_core::model::{AccountKey, MeterId, ProviderId};
        let dir = temp_dir("state");
        let path = dir.join("state.json");
        assert_eq!(load_state(&path), SavedState::default(), "missing");

        let key = |provider, meter: &str| MeterKey {
            provider,
            account: AccountKey::primary(),
            meter: MeterId::new(meter),
        };
        let mut seen = SeenMap::new();
        seen.insert(
            key(ProviderId::Claude, "weekly:fable"),
            (
                MeterNames::new("Fable", "Fable weekly", "Weekly · Fable"),
                false,
            ),
        );
        seen.insert(
            key(ProviderId::Meshy, "credits"),
            (MeterNames::new("Cr", "credits", "Credits"), true),
        );
        let mut notified = NotifyMemory::new();
        notified.insert(
            key(ProviderId::Claude, "weekly"),
            Announced::Window {
                instance: Some(jiff::Timestamp::from_second(1_800_000_000).expect("valid")),
                level: 95,
            },
        );
        let mut identities = IdentityMap::new();
        let identity = AccountIdentity::of(&["claude", "org", "account"]);
        identities.insert(
            SourceKey {
                provider: ProviderId::Claude,
                account: AccountKey::primary(),
            },
            identity.clone(),
        );
        let mut units = UnitMap::new();
        units.insert(
            seen.keys().next().cloned().expect("a meter"),
            Unit::Currency {
                code: "USD".to_owned(),
            },
        );
        let state = SavedState {
            update_announced: Some("0.2.0".to_owned()),
            ..SavedState::new(&seen, &units, &notified, &identities)
        };
        write_atomic(&path, &state_bytes(&state).expect("encode")).expect("write");
        let back = load_state(&path);
        assert_eq!(back, state);
        assert_eq!(back.seen_map(), seen);
        assert_eq!(back.notified_map(), notified);
        assert_eq!(back.identity_map(), identities);
        assert_eq!(back.update_announced(), Version::parse("0.2.0"));
        // The file holds keys, names, what was announced and identity hashes only.
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(text.contains("\"meter\": \"weekly:fable\""), "{text}");
        assert!(text.contains("\"account\": \"default\""), "{text}");
        assert!(text.contains("\"kind\": \"window\""), "{text}");
        assert!(
            text.contains(&format!("\"identity\": \"{}\"", identity.as_str())),
            "{text}"
        );
        assert!(text.contains("\"update_announced\": \"0.2.0\""), "{text}");
        // An older build's file has no notifications, identities or update: nothing known.
        std::fs::write(&path, br#"{ "version": 1, "seen": [] }"#).expect("write");
        assert_eq!(load_state(&path).notified_map(), NotifyMemory::new());
        assert_eq!(load_state(&path).identity_map(), IdentityMap::new());
        assert_eq!(load_state(&path).update_announced(), None);
        // A value that is not a version is nothing announced, not a broken file.
        std::fs::write(&path, br#"{ "update_announced": "soon", "seen": [] }"#).expect("write");
        assert_eq!(load_state(&path).update_announced(), None);
        // An entry that is not an identity is dropped, not the file.
        std::fs::write(
            &path,
            br#"{ "identities": [
                { "provider": "claude", "account": "default", "identity": "tester@example.invalid" },
                { "provider": "codex", "account": "default", "identity": "0123456789abcdef" }
            ] }"#,
        )
        .expect("write");
        let kept = load_state(&path).identity_map();
        assert_eq!(kept.len(), 1);
        assert!(kept.keys().all(|k| k.provider == ProviderId::Codex));

        std::fs::write(&path, b"{ not json").expect("damage");
        assert_eq!(load_state(&path), SavedState::default());
        // Unknown and missing fields are fine (a newer or older build's file).
        std::fs::write(&path, br#"{ "future": 1, "seen": [] }"#).expect("write");
        assert_eq!(load_state(&path), SavedState::default());
        let _ = std::fs::remove_dir_all(&dir);
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
    fn reading_has_no_side_effects() {
        let dir = temp_dir("read");
        let path = dir.join("settings.json");
        let (s, outcome) = read_settings(&path).expect("missing is not an error");
        assert_eq!((s, outcome), (Settings::default(), LoadOutcome::Missing));
        std::fs::write(&path, b"{ not json").expect("write");
        let e = read_settings(&path).expect_err("corrupt");
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read(&path).expect("still there"),
            b"{ not json",
            "a broken file is left where it is"
        );
        let saved = Settings {
            start_with_windows: true,
            ..Settings::default()
        };
        save_settings(&path, &saved).expect("save");
        assert_eq!(
            read_settings(&path).expect("read"),
            (saved, LoadOutcome::Loaded)
        );
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
