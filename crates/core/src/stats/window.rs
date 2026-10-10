//! Elapsed share (R§3.2), display percent and exhaustion (R§3.3), pace (R§3.4).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

/// Elapsed share of a window, R§3.2:
///
/// ```text
/// e(now) = clamp(1 − (R − now) / L, 0, 1)
/// ```
///
/// Example: `R − now = 2 h 14 min`, `L = 5 h` → `0.5533`. For a stale meter pass `now = t_r`
/// (frozen at the reading, R§3.8).
pub fn elapsed_share(now: Timestamp, resets_at: Timestamp, window: SignedDuration) -> f64 {
    todo!("M1: R§3.2")
}

/// `e` or **none** (R§3.2): none when `R` is none or `L` is unknown — then there is no tick, no
/// gaps and no pace.
pub fn elapsed(
    resets_at: Option<Timestamp>,
    window: Option<SignedDuration>,
    at: Timestamp,
) -> Option<f64> {
    todo!("M1: R§3.2")
}

/// `exhausted = limited || u ≥ 1.0` (R§3.3).
pub fn is_exhausted(used: f64, limited: bool) -> bool {
    todo!("M1: R§3.3")
}

/// The integer percent shown to people, R§3.3:
///
/// ```text
/// pct = clamp(round(u × 100 + 1e-9), lo, hi)
///       lo = (u > 0) ? 1 : 0          // something used never shows as 0 %
///       hi = exhausted ? 100 : 99     // 100 % only when the provider agrees it is used up
/// ```
///
/// `round` is half-up; the `1e-9` makes `0.425` give 43 despite binary floats. Level thresholds
/// compare this `pct`, never `u`.
pub fn display_pct(used: f64, exhausted: bool) -> u8 {
    todo!("M1: R§3.3")
}

/// Pace of a window meter, R§3.4 (`d = u − e`):
///
/// ```text
/// under = u < e − TOL_PACE
/// ahead = u > e + TOL_PACE
/// ```
///
/// Pace alone never sets a severity (R§3.7); it is what the tick shows and what the hover card
/// says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pace {
    /// `under`: "slower than time passes"; the bar's fill stops visibly short of the tick.
    Under,
    /// Neither under nor ahead: "about as fast as time passes".
    About,
    /// `ahead`: "faster than time passes"; the fill runs visibly past the tick.
    Ahead,
}

/// [`Pace`] of used share `u` against elapsed share `e` (R§3.4, `TOL_PACE`).
pub fn pace(used: f64, elapsed: f64) -> Pace {
    todo!("M1: R§3.4")
}
