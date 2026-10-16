//! Described services in the app (custom-services-spec §3, §6): the files read at start-up, the
//! API key rows and the key change sent to the tray process. A test binary of its own: the
//! registry of described services is per process.

use std::path::PathBuf;

use lastdrop_app::custom_services;
use lastdrop_app::dialog::Changes;
use lastdrop_app::settings_window::key_services;
use lastdrop_core::model::ProviderId;
use lastdrop_core::settings::Settings;

/// A fresh folder under the system temp folder.
fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lastdrop-providers-test-{}-{}",
        std::process::id(),
        jiff::Timestamp::now().as_nanosecond()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

const ACME: &str = r#"{
    "format": 1, "id": "acme", "name": "Acme", "url": "https://api.acme.invalid/balance",
    "balances": [ { "id": "balance", "label": "Bal", "short_name": "balance", "name": "Balance",
        "amount": "/balance", "currency": "USD" } ]
}"#;

#[test]
fn the_folder_is_read_at_start_up() {
    let dir = temp_dir();
    std::fs::write(dir.join("acme.json"), ACME).expect("write");
    std::fs::write(dir.join("broken.json"), "{ not json").expect("write");
    std::fs::write(dir.join("notes.txt"), "not read").expect("write");
    std::fs::write(dir.join("huge.json"), " ".repeat(65 * 1024)).expect("write");
    std::fs::create_dir_all(dir.join("folder.json")).expect("dir");
    let read: Vec<String> = custom_services::read_folder(&dir)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        read,
        ["acme.json", "broken.json"],
        "sorted, json files ≤ 64 KiB"
    );
    assert!(custom_services::read_folder(&dir.join("missing")).is_empty());

    custom_services::load(&dir);
    let names: Vec<&str> = ProviderId::every()
        .into_iter()
        .filter(|p| matches!(p, ProviderId::Custom(_)))
        .map(ProviderId::as_str)
        .collect();
    assert_eq!(names, ["deepseek", "elevenlabs", "openrouter", "acme"]);

    // Each has an API key row, after the built-in ones.
    let settings = Settings::default();
    let rows = key_services(&settings);
    let acme = ProviderId::from_id("acme").expect("loaded");
    assert_eq!(rows.first(), Some(&ProviderId::Meshy));
    assert_eq!(rows.last(), Some(&acme));

    // A saved key reaches the tray process by its file id, which finds its own service.
    let changes = Changes::key_of(acme);
    assert_eq!(changes.keys(), [acme]);
    let sent = changes.to_wparam();
    assert_eq!(Changes::from_wparam(sent), changes);
    assert_eq!(
        Changes::from_wparam(sent | 1).keys(),
        [acme],
        "with the settings flag too"
    );
    for p in ProviderId::every() {
        if p != acme && matches!(p, ProviderId::Custom(_)) {
            assert_ne!(
                Changes::from_wparam(Changes::key_of(p).to_wparam()).keys(),
                [acme]
            );
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
}
