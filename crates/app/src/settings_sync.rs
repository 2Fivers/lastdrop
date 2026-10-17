//! Settings write coordination (settings-spec §6). Three writers change `settings.json`: the tray
//! process (pause, readout auto or off, a meter pinned from a card), the settings window and Theme
//! Studio. Each write is one read-merge-write under the named mutex
//! [`lastdrop_win::named_lock::SETTINGS_LOCK`] ([`SettingsFile::update`]): the writer reads the
//! file as it is at that moment and applies only its own change onto it ([`apply_onto`]), so no
//! writer replaces a change it never read.
//!
//! - **Tray process**: its changes are queued for its writer thread as the settings the change
//!   started from and the new settings ([`TraySync::save`]); the writer thread rebases them onto
//!   the file when it runs the job ([`SettingsFile::rebase_write`]), not when it was queued.
//! - **Re-reading in the tray**: when the dialog process says it wrote the file, the tray re-reads
//!   it only after every writer job queued before the message is done — through a barrier job
//!   ([`TraySync::barrier`]) whose result is posted back to the tray's event loop, never a wait on
//!   the UI thread. Tray edits made while the barrier is pending are queued after it and survive:
//!   the file read at the barrier gets them rebased onto it ([`TraySync::reload`]).
//! - **Dialog process**: the settings window and Theme Studio's "Save and use" do their
//!   read-merge-write on its main thread.
//!
//! Nobody waits for a writer thread while holding the lock; it is held for one read-merge-write.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::time::Duration;

use lastdrop_core::settings::Settings;
use lastdrop_win::named_lock::{NamedLock, SETTINGS_LOCK};
use serde_json::Value;

use crate::persist::{self, LoadOutcome};
use crate::writer::WriterHandle;

/// How long a writer waits for the lock before it writes without it (**not from the spec**): far
/// longer than a read-merge-write takes. A lock held longer belongs to a stuck process; losing the
/// write would be worse than the race the lock prevents.
pub const LOCK_WAIT: Duration = Duration::from_secs(5);

/// `settings.json` and the lock around its read-merge-writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsFile {
    /// The file.
    pub path: PathBuf,
    /// The named mutex ([`SETTINGS_LOCK`]; tests use names of their own).
    pub lock: String,
}

impl SettingsFile {
    /// The file at `path` with Lastdrop's settings lock.
    pub fn new(path: PathBuf) -> SettingsFile {
        SettingsFile::with_lock(path, SETTINGS_LOCK)
    }

    /// The file at `path` with the lock `lock` (tests, so they never touch a running Lastdrop).
    pub fn with_lock(path: PathBuf, lock: &str) -> SettingsFile {
        SettingsFile {
            path,
            lock: lock.to_owned(),
        }
    }

    /// Takes the lock for one read-merge-write; `None` (logged) when it cannot be had within
    /// [`LOCK_WAIT`], and the caller goes on without it.
    pub fn lock(&self) -> Option<NamedLock> {
        match NamedLock::acquire(&self.lock, LOCK_WAIT) {
            Ok(lock) => Some(lock),
            Err(e) => {
                log::warn!("settings lock: {e}; writing without it");
                None
            }
        }
    }

    /// The file as a merge reads it: its settings, or `fallback` when there is no file or it
    /// cannot be read (a broken file is set aside first, as [`persist::load_settings`] does, so
    /// the write that follows never overwrites it).
    pub fn read(&self, fallback: &Settings) -> Settings {
        let (settings, outcome) = persist::load_settings(&self.path);
        match outcome {
            LoadOutcome::Loaded | LoadOutcome::Migrated { .. } => settings,
            LoadOutcome::Missing | LoadOutcome::Corrupt { .. } => fallback.clone(),
        }
    }

    /// One read-merge-write under the lock: `change` gets the file's settings ([`Self::read`]
    /// with `fallback`) and returns the settings to write, which are written atomically and
    /// returned.
    pub fn update(
        &self,
        fallback: &Settings,
        change: impl FnOnce(Settings) -> Settings,
    ) -> std::io::Result<Settings> {
        let _lock = self.lock();
        let merged = change(self.read(fallback));
        persist::save_settings(&self.path, &merged)?;
        Ok(merged)
    }

    /// Writes what `new` changed from `base` onto the file as it is now ([`apply_onto`]), under
    /// the lock. Without a usable file, `new` is written as it is.
    pub fn rebase_write(&self, base: &Settings, new: &Settings) -> std::io::Result<Settings> {
        self.update(base, |disk| apply_onto(base, new.clone(), &disk))
    }
}

/// A three-way merge: `disk` (the file as it is now) with what `changed` changed from `base` (the
/// settings the change started from) applied onto it. Changes another writer made meanwhile
/// survive unless `changed` changed the same setting, in which case `changed` wins.
///
/// The merge works on the JSON form ([`merge_changes`]); should the merged JSON not read back as
/// settings (it always should), `changed` is written with the file's pause.
pub fn apply_onto(base: &Settings, changed: Settings, disk: &Settings) -> Settings {
    let merged = (|| {
        let base = serde_json::to_value(base).ok()?;
        let changed = serde_json::to_value(&changed).ok()?;
        let mut merged = serde_json::to_value(disk).ok()?;
        merge_changes(&base, &changed, &mut merged);
        serde_json::from_value::<Settings>(merged).ok()
    })();
    merged.unwrap_or_else(|| {
        log::warn!("settings: cannot merge a change; writing it whole");
        Settings {
            pause: disk.pause,
            ..changed
        }
    })
}

/// Applies onto `target` what `changed` changed from `base`, recursively: an unchanged value keeps
/// `target`'s, a changed one replaces it. Objects are merged key by key only when `base`,
/// `changed` and `target` have the same keys, so a struct's fields merge one by one while an
/// enum that changed its variant (or a section with a newer build's extra field) is replaced
/// whole and the result always reads back. Arrays and scalars are single values.
fn merge_changes(base: &Value, changed: &Value, target: &mut Value) {
    if base == changed {
        return;
    }
    if let (Value::Object(b), Value::Object(c), Value::Object(t)) = (base, changed, &mut *target)
        && b.len() == c.len()
        && c.len() == t.len()
        && c.keys().all(|k| b.contains_key(k) && t.contains_key(k))
    {
        for (key, value) in c {
            if let (Some(base_value), Some(target_value)) = (b.get(key), t.get_mut(key)) {
                merge_changes(base_value, value, target_value);
            }
        }
        return;
    }
    target.clone_from(changed);
}

/// The settings to apply after the dialog process wrote `settings.json`: the file's, with the tray
/// process's pause (the tray menu owns it). `None` when the file is missing or cannot be read or
/// understood: a broken file never replaces the running settings with defaults (the next start
/// sets it aside).
pub fn reloaded_settings(
    current: &Settings,
    read: std::io::Result<(Settings, LoadOutcome)>,
) -> Option<Settings> {
    match read {
        Ok((settings, LoadOutcome::Loaded | LoadOutcome::Migrated { .. })) => Some(Settings {
            pause: current.pause,
            ..settings
        }),
        Ok((_, LoadOutcome::Missing | LoadOutcome::Corrupt { .. })) | Err(_) => None,
    }
}

/// What a barrier's job read: the file right after every earlier job was done.
pub type BarrierRead = std::io::Result<(Settings, LoadOutcome)>;

/// The tray process's side of the coordination (main thread).
pub struct TraySync {
    /// `settings.json`.
    file: SettingsFile,
    /// The tray's writer thread.
    writer: WriterHandle,
    /// The tray's settings as of its last queued save or reload: the base of its next change.
    saved: RefCell<Settings>,
    /// The latest barrier: its token and the tray's settings when it was queued.
    barrier: RefCell<Option<(u64, Settings)>>,
    /// The latest barrier's token.
    token: Cell<u64>,
}

impl TraySync {
    /// The coordination for a tray process that runs with `settings` (as read at start-up).
    pub fn new(file: SettingsFile, writer: WriterHandle, settings: &Settings) -> TraySync {
        TraySync {
            file,
            writer,
            saved: RefCell::new(settings.clone()),
            barrier: RefCell::new(None),
            token: Cell::new(0),
        }
    }

    /// Queues the tray's change since its last save or reload (`current`, its settings now): the
    /// writer thread applies it onto the file as it is when the job runs.
    pub fn save(&self, current: &Settings) {
        let base = self.saved.replace(current.clone());
        self.writer
            .save_settings(self.file.clone(), base, current.clone());
    }

    /// The dialog process wrote the file: queues a barrier behind every writer job queued so far
    /// and returns its token. When the writer thread reaches it, it reads the file and runs `done`
    /// with the token and what it read (the tray posts that to its event loop, then calls
    /// [`Self::reload`]). A later barrier supersedes this one.
    pub fn barrier(
        &self,
        current: &Settings,
        done: impl FnOnce(u64, BarrierRead) + Send + 'static,
    ) -> u64 {
        let token = self.token.get() + 1;
        self.token.set(token);
        *self.barrier.borrow_mut() = Some((token, current.clone()));
        let path = self.file.path.clone();
        self.writer.barrier(Box::new(move || {
            done(token, persist::read_settings(&path));
        }));
        token
    }

    /// The barrier `token` was reached and read the file (`read`): the settings the tray runs with
    /// from now on — the file's ([`reloaded_settings`]) with the tray's own changes since the
    /// barrier was queued rebased onto them (they are queued after it, so the file may lack them).
    /// `None` when a later barrier is pending (its reload follows) or the file was missing or
    /// unreadable (the tray keeps its settings).
    pub fn reload(&self, token: u64, current: &Settings, read: BarrierRead) -> Option<Settings> {
        let at_barrier = {
            let mut barrier = self.barrier.borrow_mut();
            match barrier.take() {
                Some((t, settings)) if t == token => settings,
                other => {
                    *barrier = other;
                    return None;
                }
            }
        };
        let Some(disk) = reloaded_settings(current, read) else {
            log::warn!("settings.json is missing or unreadable; keeping the current settings");
            return None;
        };
        let merged = apply_onto(&at_barrier, current.clone(), &disk);
        self.saved.replace(merged.clone());
        Some(merged)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use lastdrop_core::grammar::TimeStyle;
    use lastdrop_core::model::{AccountKey, MeterId, MeterKey, ProviderId};
    use lastdrop_core::settings::{Hotkey, Pause, ReadoutContent, RefreshInterval};

    use super::*;
    use crate::writer::Writer;

    /// A fresh folder under the system temp folder for one test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-sync-test-{name}-{}-{}",
            std::process::id(),
            jiff::Timestamp::now().as_nanosecond()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// `settings.json` in a fresh folder with a lock of the test's own.
    fn scratch_file(name: &str) -> SettingsFile {
        SettingsFile::with_lock(
            temp_dir(name).join("settings.json"),
            &format!(
                r"Local\Lastdrop.Test.Settings.{name}.{}",
                std::process::id()
            ),
        )
    }

    fn claude(meter: MeterId) -> MeterKey {
        MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter,
        }
    }

    fn set_pins(settings: &mut Settings, pins: Vec<MeterKey>) {
        settings.readout.content = if pins.is_empty() {
            ReadoutContent::Auto
        } else {
            ReadoutContent::Pinned(pins)
        };
    }

    /// A change of the interval, the readout, a hotkey, autostart and a pinned meter.
    fn edits(base: &Settings) -> Settings {
        let mut draft = base.clone();
        draft.start_with_windows = true;
        draft.refresh.interval = RefreshInterval::Minutes(15);
        draft.readout.enabled = false;
        draft.hotkey = Some(Hotkey {
            ctrl: true,
            alt: true,
            shift: false,
            win: false,
            key: "L".to_owned(),
        });
        set_pins(&mut draft, vec![claude(MeterId::new(MeterId::WEEKLY))]);
        draft
    }

    #[test]
    fn a_change_is_written_onto_the_file_as_it_is_now() {
        let base = Settings::default();
        let draft = edits(&base);
        assert_eq!(
            apply_onto(&base, draft.clone(), &base),
            draft,
            "an unchanged file gets the change"
        );

        // Meanwhile the tray paused, and a field the change never touched changed.
        let mut disk = base.clone();
        disk.pause = Some(Pause { until: None });
        disk.theme_follows_system = false;
        assert_eq!(
            apply_onto(&base, draft.clone(), &disk),
            Settings {
                pause: disk.pause,
                theme_follows_system: false,
                ..draft
            }
        );
    }

    #[test]
    fn the_pause_on_disk_is_kept_unless_the_change_is_one() {
        // Paused when the change started, resumed by another writer meanwhile: not paused again.
        let paused = Settings {
            pause: Some(Pause { until: None }),
            ..Settings::default()
        };
        let resumed = Settings::default();
        assert_eq!(apply_onto(&paused, edits(&paused), &resumed).pause, None);
        // Paused by another writer meanwhile: stays paused.
        assert_eq!(
            apply_onto(&resumed, edits(&resumed), &paused).pause,
            paused.pause
        );
    }

    #[test]
    fn a_meter_pinned_elsewhere_survives_an_interval_changed_here() {
        let base = Settings::default();
        let mut draft = base.clone();
        draft.refresh.interval = RefreshInterval::Minutes(15);
        let five = claude(MeterId::new(MeterId::FIVE_HOUR));
        let mut disk = base.clone();
        set_pins(&mut disk, vec![five.clone()]);

        let applied = apply_onto(&base, draft, &disk);
        assert_eq!(applied.refresh.interval, RefreshInterval::Minutes(15));
        assert_eq!(applied.readout.content, ReadoutContent::Pinned(vec![five]));

        // The readout turned off elsewhere survives too.
        let mut disk = base.clone();
        disk.readout.enabled = false;
        let mut draft = base.clone();
        draft.readout.time_style = TimeStyle::Relative;
        let applied = apply_onto(&base, draft, &disk);
        assert!(!applied.readout.enabled);
        assert_eq!(applied.readout.time_style, TimeStyle::Relative);
    }

    #[test]
    fn the_change_wins_when_both_changed_the_same_setting() {
        let weekly = claude(MeterId::new(MeterId::WEEKLY));
        let five = claude(MeterId::new(MeterId::FIVE_HOUR));
        let mut base = Settings::default();
        set_pins(&mut base, vec![weekly.clone()]);

        // The change pinned a second meter; the file switched to auto.
        let mut draft = base.clone();
        set_pins(&mut draft, vec![weekly.clone(), five.clone()]);
        let mut disk = base.clone();
        disk.readout.content = ReadoutContent::Auto;
        assert_eq!(
            apply_onto(&base, draft, &disk).readout.content,
            ReadoutContent::Pinned(vec![weekly.clone(), five.clone()])
        );

        // The change switched to auto; the file pinned another meter.
        let mut draft = base.clone();
        draft.readout.content = ReadoutContent::Auto;
        let mut disk = base.clone();
        set_pins(&mut disk, vec![weekly, five]);
        assert_eq!(
            apply_onto(&base, draft, &disk).readout.content,
            ReadoutContent::Auto
        );

        // Both changed the interval.
        let mut draft = base.clone();
        draft.refresh.interval = RefreshInterval::Manual;
        let mut disk = base.clone();
        disk.refresh.interval = RefreshInterval::Minutes(30);
        assert_eq!(
            apply_onto(&base, draft, &disk).refresh.interval,
            RefreshInterval::Manual
        );
    }

    #[test]
    fn objects_merge_key_by_key_only_when_their_keys_match() {
        let base = serde_json::json!({
            "a": {"x": 1, "y": 1},
            "e": {"mode": "auto"},
            "n": [1],
            "s": 1
        });
        let changed = serde_json::json!({
            "a": {"x": 2, "y": 1},
            "e": {"mode": "pinned", "meters": [1]},
            "n": [1],
            "s": 1
        });
        let mut target = serde_json::json!({
            "a": {"x": 1, "y": 3},
            "e": {"mode": "other", "k": 0},
            "n": [2],
            "s": 3
        });
        merge_changes(&base, &changed, &mut target);
        assert_eq!(
            target,
            serde_json::json!({
                "a": {"x": 2, "y": 3},
                "e": {"mode": "pinned", "meters": [1]},
                "n": [2],
                "s": 3
            })
        );
    }

    #[test]
    fn a_rebased_write_keeps_what_another_writer_wrote_and_creates_a_missing_file() {
        let file = scratch_file("rebase");
        let base = Settings::default();
        let mut new = base.clone();
        new.readout.enabled = false;
        // No file yet: the change is written as it is.
        assert_eq!(file.rebase_write(&base, &new).expect("write"), new);
        assert_eq!(persist::load_settings(&file.path).0, new);

        // Another writer changed the interval meanwhile; this one pauses.
        let mut other = new.clone();
        other.refresh.interval = RefreshInterval::Manual;
        persist::save_settings(&file.path, &other).expect("other writer");
        let mut paused = new.clone();
        paused.pause = Some(Pause { until: None });
        let written = file.rebase_write(&new, &paused).expect("write");
        assert_eq!(written.refresh.interval, RefreshInterval::Manual);
        assert_eq!(written.pause, paused.pause);
        assert_eq!(persist::load_settings(&file.path).0, written);
    }

    #[test]
    fn a_reload_ignores_a_missing_or_broken_file_and_keeps_the_pause() {
        let current = Settings {
            pause: Some(Pause { until: None }),
            ..Settings::default()
        };
        let mut saved = Settings {
            start_with_windows: true,
            ..Settings::default()
        };
        saved.readout.enabled = false;

        let loaded = reloaded_settings(&current, Ok((saved.clone(), LoadOutcome::Loaded)))
            .expect("a clean load applies");
        assert_eq!(
            loaded,
            Settings {
                pause: current.pause,
                ..saved.clone()
            }
        );
        assert!(
            reloaded_settings(
                &current,
                Ok((saved.clone(), LoadOutcome::Migrated { from: 0 }))
            )
            .is_some()
        );
        assert_eq!(
            reloaded_settings(&current, Ok((Settings::default(), LoadOutcome::Missing))),
            None,
            "no file: keep the current settings"
        );
        assert_eq!(
            reloaded_settings(
                &current,
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "not json"
                ))
            ),
            None,
            "a broken file never brings the defaults"
        );
        assert_eq!(
            reloaded_settings(
                &current,
                Ok((
                    Settings::default(),
                    LoadOutcome::Corrupt {
                        backup: PathBuf::from("settings.json.bad-1")
                    }
                ))
            ),
            None
        );
        assert_eq!(
            reloaded_settings(
                &current,
                Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
            ),
            None
        );
    }

    #[test]
    fn the_tray_rereads_only_after_its_earlier_writes() {
        let file = scratch_file("barrier");
        let start = Settings::default();
        persist::save_settings(&file.path, &start).expect("seed");
        let writer = Writer::start().expect("writer");
        let tray = TraySync::new(file.clone(), writer.handle(), &start);
        let (release, until) = mpsc::channel();
        writer.handle().block(until);
        let mut state = start.clone();
        state.readout.enabled = false;
        tray.save(&state);
        let (reached_tx, reached) = mpsc::channel();
        let token = tray.barrier(&state, move |token, read| {
            let _ = reached_tx.send((token, read));
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            reached.try_recv().is_err(),
            "the barrier waits behind the busy writer"
        );
        release.send(()).expect("release");
        let (t, read) = reached.recv().expect("reached");
        assert_eq!(t, token);
        let (read, _) = read.expect("read");
        assert!(!read.readout.enabled, "the earlier save is in what it read");
        writer.shutdown();
    }

    #[test]
    fn only_the_latest_barrier_reloads() {
        let file = scratch_file("tokens");
        let start = Settings::default();
        persist::save_settings(&file.path, &start).expect("seed");
        let writer = Writer::start().expect("writer");
        let tray = TraySync::new(file.clone(), writer.handle(), &start);
        let (tx, rx) = mpsc::channel();
        let first_tx = tx.clone();
        let first = tray.barrier(&start, move |t, read| {
            let _ = first_tx.send((t, read));
        });
        let second = tray.barrier(&start, move |t, read| {
            let _ = tx.send((t, read));
        });
        let (t1, read1) = rx.recv().expect("first");
        let (t2, read2) = rx.recv().expect("second");
        assert_eq!((t1, t2), (first, second));
        assert_eq!(tray.reload(t1, &start, read1), None, "superseded");
        assert_eq!(tray.reload(t2, &start, read2), Some(start.clone()));
        assert_eq!(
            tray.reload(t2, &start, Ok((start.clone(), LoadOutcome::Loaded))),
            None,
            "reached once"
        );
        writer.shutdown();
    }

    /// Plan §3.6 / settings-spec §6: the tray's writer is blocked, the tray pauses (queued), the
    /// settings window commits a different change, the tray changes the readout while the
    /// barrier is pending, then the writer runs again: all three are on disk and in the tray.
    #[test]
    fn a_blocked_tray_writer_loses_neither_its_changes_nor_a_settings_commit() {
        let file = scratch_file("race");
        let start = Settings::default();
        persist::save_settings(&file.path, &start).expect("seed");
        let writer = Writer::start().expect("writer");
        let tray = TraySync::new(file.clone(), writer.handle(), &start);
        let mut tray_state = start.clone();

        // The tray's writer is busy (a slow disk).
        let (release, until) = mpsc::channel();
        writer.handle().block(until);
        // 1. The tray pauses: queued behind the busy job.
        tray_state.pause = Some(Pause { until: None });
        tray.save(&tray_state);
        // 2. The settings window commits a different change onto the file as it is (no pause
        //    in it yet)…
        let window_base = file.read(&start);
        assert_eq!(window_base.pause, None);
        let mut window_new = window_base.clone();
        window_new.refresh.interval = RefreshInterval::Minutes(15);
        file.rebase_write(&window_base, &window_new)
            .expect("commit");
        // …and tells the tray, which queues a barrier instead of re-reading now.
        let (reached_tx, reached) = mpsc::channel();
        let token = tray.barrier(&tray_state, move |token, read| {
            let _ = reached_tx.send((token, read));
        });
        // 3. The tray's event loop keeps running: another change while the barrier is pending.
        tray_state.readout.enabled = false;
        tray.save(&tray_state);
        assert!(reached.try_recv().is_err(), "the writer is still blocked");

        // 4. The disk comes back.
        release.send(()).expect("release");
        let (reached_token, read) = reached.recv().expect("barrier reached");
        assert_eq!(reached_token, token);
        if let Some(settings) = tray.reload(token, &tray_state, read) {
            tray_state = settings;
        }
        writer.handle().flush();
        let (on_disk, _) = persist::load_settings(&file.path);
        for (what, s) in [("disk", &on_disk), ("tray", &tray_state)] {
            assert_eq!(s.pause, Some(Pause { until: None }), "{what}: the pause");
            assert_eq!(
                s.refresh.interval,
                RefreshInterval::Minutes(15),
                "{what}: the window's commit"
            );
            assert!(!s.readout.enabled, "{what}: the change made while waiting");
        }

        // The tray's next change starts from what it reloaded: the window's commit stays.
        tray_state.readout.time_style = TimeStyle::Relative;
        tray.save(&tray_state);
        writer.handle().flush();
        let (on_disk, _) = persist::load_settings(&file.path);
        assert_eq!(on_disk, tray_state);
        writer.shutdown();
    }
}
