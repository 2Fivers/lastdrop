//! Placement against the recorded layouts in `tests/layouts/` (ARCHITECTURE "Readout placement").

use std::path::PathBuf;

use lastdrop_core::readout::ReadoutLayout;
use lastdrop_win::placement::{Placement, PlacementPrefs, TaskbarGeometry, place};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    #[expect(dead_code, reason = "documentation for people reading the JSON")]
    description: String,
    #[expect(dead_code, reason = "documentation for people reading the JSON")]
    synthetic: bool,
    geometry: TaskbarGeometry,
    layout: ReadoutLayout,
    prefs: PlacementPrefs,
    expected: Placement,
}

fn cases() -> Vec<(PathBuf, Case)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("layouts");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("tests/layouts") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).expect("read layout");
            let case: Case =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            out.push((path, case));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn every_layout_file_parses_and_is_named_after_its_file() {
    let cases = cases();
    assert!(!cases.is_empty(), "no layouts recorded");
    for (path, case) in &cases {
        let stem = path.file_stem().and_then(|s| s.to_str()).expect("stem");
        assert_eq!(case.name, stem, "{}", path.display());
    }
}

#[test]
#[ignore = "M1 implementation"]
fn placement_matches_every_recorded_layout() {
    for (path, case) in cases() {
        let got = place(&case.geometry, &case.layout, &case.prefs);
        assert_eq!(got, case.expected, "{}", path.display());
    }
}
