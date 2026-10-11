//! What the Codex app-server source logs about `codex.exe`: its file name, never its folder,
//! which holds the Windows user name. Its own test binary, because a process has one logger.

use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use jiff::{SignedDuration, Timestamp};
use lastdrop_providers::codex::app_server;
use lastdrop_providers::fakes::Fakes;
use log::{LevelFilter, Log, Metadata, Record};

/// Every record logged.
struct Capture(Mutex<Vec<String>>);

impl Log for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &Record<'_>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(record.args().to_string());
    }

    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

fn now() -> Timestamp {
    "2026-10-10T10:42:00Z".parse().expect("timestamp")
}

#[test]
fn codex_exe_is_logged_by_its_file_name_only() {
    log::set_logger(&CAPTURE).expect("the only logger of this test binary");
    log::set_max_level(LevelFilter::Trace);

    let f = Fakes::new(now());
    let codex_home = PathBuf::from(r"C:\Users\tester\.codex");
    f.fs.add_file(codex_home.join("config.toml"), "", now());
    let ctx = f.context(SignedDuration::from_secs(30));

    // A configured codex.exe that is gone, then a start (the fake spawner has no script, so the
    // start fails as if the file were missing).
    let missing = PathBuf::from(r"C:\Users\tester\Tools\codex.exe");
    assert_eq!(app_server::find_codex_exe(&ctx, Some(&missing)), None);
    let npm = PathBuf::from(r"C:\Users\tester\AppData\Roaming\npm\codex.cmd");
    assert!(app_server::fetch_with(&ctx, &npm, &codex_home).is_err());

    let records = std::mem::take(&mut *CAPTURE.0.lock().unwrap_or_else(PoisonError::into_inner));
    assert!(
        records.iter().any(|r| r.contains("codex.exe")),
        "{records:?}"
    );
    assert!(
        records.iter().any(|r| r.contains("codex.cmd")),
        "{records:?}"
    );
    for record in &records {
        assert!(!record.contains("tester"), "a path in the log: {record}");
    }
}
