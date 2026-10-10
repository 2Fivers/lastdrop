//! Percent and numbers (R§5.6) and balance durations (R§5.5, F§4.5).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use super::locale::LocaleFormat;
use super::strings::Strings;
use crate::stats::BalanceStats;

/// A percent with the locale's pattern (R§5.6): en `42%`; sv-SE `42 %` with U+202F. Digits are
/// tabular in the fonts used, so no padding is added.
pub fn percent(pct: u8, locale: &LocaleFormat) -> String {
    todo!("M1: R§5.6 percent")
}

/// A share shown as a whole percent, half up (`e = 0.5533` → `55` in `55% of time gone`;
/// `p = 0.621` → `62` in `on course for 62% at reset`, clamped to 99 there, R§8.2).
pub fn share_pct(share: f64) -> u8 {
    todo!("M1: R§8.2 share as percent")
}

/// An amount as a whole number with the locale's digit grouping: `1,240`; sv-SE `1 240` with
/// U+202F (R§5.6). Credits are whole; fractions are rounded half up.
pub fn amount(value: f64, locale: &LocaleFormat) -> String {
    todo!("M1: R§5.6 amount")
}

/// The readout's balance duration after `≈` (R§5.5): `days_left < 2` → `Nh` (floor hours, ≥ 1);
/// `< 14` → `Nd` (floor); `< 63` → `Nw` (floor weeks); `≥ 63` → `9w+`. `None` (shown as `–`) when
/// there was no burn in 7 days or less than 48 h of history.
pub fn balance_duration(stats: &BalanceStats, strings: &dyn Strings) -> Option<String> {
    todo!("M1: R§5.5")
}

/// The flyout's balance forecast (F§4.5): `≈ 9 days left`, `≈ 20 hours left`, `≈ 6 weeks left`,
/// `More than 8 weeks left`, `No use in 7 days`, `Not enough history yet` (< 48 h); thresholds
/// and flooring as R§5.5. EXHAUSTED is handled by the caller (`Empty`).
pub fn balance_forecast_long(stats: &BalanceStats, strings: &dyn Strings) -> String {
    todo!("M1: F§4.5")
}

/// The hover card's balance duration words (`9 days`, `20 hours`, `6 weeks`) for
/// `lasts ≈ {duration}` (R§8.2).
pub fn balance_duration_words(stats: &BalanceStats, strings: &dyn Strings) -> Option<String> {
    todo!("M1: R§8.2 balance forecast")
}
