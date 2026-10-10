//! Readout-spec §13 "Widths (en, 100 %) — 160 / 176 / 186 / 134 / 150 px as in §6.3", measured with
//! DirectWrite and Segoe UI Variable Small (needs Windows 11 fonts; run on the dev machine and CI's
//! `windows-latest`).

use lastdrop_core::readout::layout::BalanceWidthInputs;
use lastdrop_core::readout::{WidthInputs, compute_layout};
use lastdrop_win::render::{RenderFactory, TextMeasurer};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

/// The factories of the test thread (COM must be initialized on the thread, as on the readout
/// and main threads).
fn factory() -> RenderFactory {
    // SAFETY: initializes COM on the test thread; S_FALSE (already initialized) is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    RenderFactory::new().expect("factories")
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

/// The en strings of R§6.3 for Claude-style windows (`5h`, `Wk`), clock mode.
fn inputs(labels: &[&str], glyph_column: bool) -> WidthInputs {
    let times = [
        "00:00", "Today", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun", "88d",
    ];
    let prefixed = |prefix: &str| {
        times
            .iter()
            .map(|t| format!("{prefix}{t}"))
            .collect::<Vec<_>>()
    };
    let mut say = prefixed("Out ");
    say.extend(strings(&["59 min left", "47 h left", "29 d left"]));
    say.extend(prefixed("Back "));
    say.push("Empty".to_owned());
    let mut regular_at_zero = vec!["Reading…".to_owned()];
    regular_at_zero.extend(prefixed("Back "));
    WidthInputs {
        glyph_column,
        labels: strings(labels),
        time_tokens: strings(&times),
        statuses: strings(&[
            "Offline", "Sign in", "Paused", "No data", "59m ago", "47h ago", "–",
        ]),
        regular_at_zero,
        say_semibold: say,
        balance: None,
        compact_semibold: strings(&["59 min", "47 h", "29 d", "Empty", "99%"]),
        compact_regular: strings(&["99%"]),
    }
}

#[test]
fn widths_at_100_percent_match_r6_3() {
    let factory = factory();
    let measure = TextMeasurer::new(&factory);

    let one_service = compute_layout(&inputs(&["5h", "Wk"], false), 1.0, &measure);
    assert_eq!((one_service.label_w, one_service.right_w), (17, 79));
    assert_eq!(one_service.width_rows, 160);
    assert_eq!(one_service.compact_w, 53);
    assert_eq!(one_service.width_compact, 134);

    let several = compute_layout(&inputs(&["5h", "Wk"], true), 1.0, &measure);
    assert_eq!(several.width_rows, 176);
    assert_eq!(several.width_compact, 150);

    let per_model = compute_layout(&inputs(&["5h", "Wk", "Fable"], true), 1.0, &measure);
    assert_eq!(per_model.label_w, 27);
    assert_eq!(per_model.width_rows, 186);
}

#[test]
fn widths_scale_per_constant_at_125_percent() {
    // R§0: each constant converts on its own (6 → 8, 40 → 50, 12 → 15, 4 → 5); text is measured
    // at the em size × 1.25 and ceiled. The sum must equal the sum of the converted parts.
    let factory = factory();
    let measure = TextMeasurer::new(&factory);
    let mut inputs = inputs(&["5h", "Wk", "Cr"], true);
    inputs.balance = Some(BalanceWidthInputs {
        approx: "≈".to_owned(),
        durations: strings(&["47h", "13d", "8w", "9w+"]),
    });
    let l = compute_layout(&inputs, 1.25, &measure);
    assert_eq!(l.bar_w, 50);
    assert_eq!(l.x_glyph, Some(8));
    assert_eq!(l.x_label, 8 + 15 + 5);
    assert_eq!(
        l.width_rows,
        8 + 15 + 5 + l.label_w + 8 + 50 + 8 + l.right_w + 8
    );
}
