//! Elapsed share (R§3.2), display percent and exhaustion (R§3.3), pace (R§3.4).

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::consts::TOL_PACE;
use crate::util::{round_half_up, sane_share};

/// Elapsed share of a window, R§3.2:
///
/// ```text
/// e(now) = clamp(1 − (R − now) / L, 0, 1)
/// ```
///
/// Example: `R − now = 2 h 14 min`, `L = 5 h` → `0.5533`. For a stale meter pass `now = t_r`
/// (frozen at the reading, R§3.8).
pub fn elapsed_share(now: Timestamp, resets_at: Timestamp, window: SignedDuration) -> f64 {
    let len = window.as_secs_f64();
    let remaining = resets_at.duration_since(now).as_secs_f64();
    if len <= 0.0 {
        // A zero-length window is over the moment it starts.
        return if remaining > 0.0 { 0.0 } else { 1.0 };
    }
    (1.0 - remaining / len).clamp(0.0, 1.0)
}

/// `e` or **none** (R§3.2): none when `R` is none or `L` is unknown — then there is no tick, no
/// gaps and no pace.
pub fn elapsed(
    resets_at: Option<Timestamp>,
    window: Option<SignedDuration>,
    at: Timestamp,
) -> Option<f64> {
    Some(elapsed_share(at, resets_at?, window?))
}

/// `exhausted = limited || u ≥ 1.0` (R§3.3).
pub fn is_exhausted(used: f64, limited: bool) -> bool {
    limited || used >= 1.0
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
    let u = sane_share(used);
    let lo = if u > 0.0 { 1.0 } else { 0.0 };
    let hi = if exhausted { 100.0 } else { 99.0 };
    // `clamp` keeps the value in 0..=100, so the cast is exact.
    round_half_up(u * 100.0 + 1e-9).clamp(lo, hi) as u8
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
    if used < elapsed - TOL_PACE {
        Pace::Under
    } else if used > elapsed + TOL_PACE {
        Pace::Ahead
    } else {
        Pace::About
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(secs: i64) -> Timestamp {
        Timestamp::from_second(secs).expect("valid")
    }

    #[test]
    fn elapsed_is_clamped_and_none_without_r_or_l() {
        let h5 = SignedDuration::from_hours(5);
        // Before the window started and after it ended.
        assert_eq!(elapsed_share(ts(0), ts(10 * 3600), h5), 0.0);
        assert_eq!(elapsed_share(ts(11 * 3600), ts(10 * 3600), h5), 1.0);
        assert_eq!(elapsed_share(ts(0), ts(0), SignedDuration::ZERO), 1.0);
        assert_eq!(elapsed(None, Some(h5), ts(0)), None);
        assert_eq!(elapsed(Some(ts(3600)), None, ts(0)), None);
        assert!(elapsed(Some(ts(3600)), Some(h5), ts(0)).is_some());
    }

    #[test]
    fn percent_edges() {
        // 0 %, 100 %, used > 1, negative and NaN input.
        assert_eq!(display_pct(0.0, false), 0);
        assert_eq!(display_pct(1.0, true), 100);
        assert_eq!(display_pct(1.37, true), 100);
        assert_eq!(display_pct(-0.2, false), 0);
        assert_eq!(display_pct(f64::NAN, false), 0);
        // Something used is never 0 %, nearly all is 99 % until the provider agrees.
        assert_eq!(display_pct(1e-6, false), 1);
        assert_eq!(display_pct(0.999, false), 99);
        assert!(is_exhausted(1.37, false));
        assert!(is_exhausted(0.2, true));
        assert!(!is_exhausted(0.9999, false));
    }

    #[test]
    fn pace_uses_the_tolerance() {
        assert_eq!(pace(0.42, 0.5533), Pace::Under);
        assert_eq!(pace(0.55, 0.5533), Pace::About);
        assert_eq!(pace(0.58, 0.5533), Pace::About);
        assert_eq!(pace(0.59, 0.5533), Pace::Ahead);
    }
}
