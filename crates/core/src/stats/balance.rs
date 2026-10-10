//! Balance meters (R§3.6).

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::history::Sample;
use crate::consts::{BAL_MIN_HISTORY, BAL_REF_LOOKBACK, BAL_SPAN, BAL_TOPUP_MIN_SHARE};
use crate::model::TopUp;
use crate::util::{add, duration_from_hours, sub};

/// Days in `BAL_SPAN` (`burn = sum of decreases over the last 7 days / 7`).
const BAL_SPAN_DAYS: f64 = 7.0;

/// The history with the current reading appended when the history does not end with it (the
/// app records the reading before evaluating, tests may not).
fn with_current(history: &[Sample], current: f64, now: Timestamp) -> Vec<Sample> {
    let mut all: Vec<Sample> = history.to_vec();
    let ends_with_current = all
        .last()
        .is_some_and(|s| s.value.to_bits() == current.to_bits());
    if !ends_with_current {
        let t = all.last().map_or(now, |s| s.t.max(now));
        all.push(Sample::balance(t, current));
    }
    all
}

/// An increase from `before` to `after` is a top-up when it is at least 1 % of the balance
/// after it (`BAL_TOPUP_MIN_SHARE`, R§3.6).
pub(super) fn is_top_up(before: f64, after: f64) -> bool {
    after > before && after - before >= BAL_TOPUP_MIN_SHARE * after
}

/// Statistics of a balance meter (R§3.6). A balance meter has no tick (no window) and no reset.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalanceStats {
    /// `ref`: balance right after the most recent top-up, else the highest balance in the last
    /// 30 days; never below the current balance ([`balance_reference`]).
    pub reference: f64,
    /// `u = 1 − balance / ref`: share of the last top-up spent, drawn like a window fill.
    pub used_share: f64,
    /// `burn`: sum of decreases over the last 7 days / 7, units per day; increases (top-ups)
    /// ignored. `0.0` without decreases.
    pub burn_per_day: f64,
    /// `valid = history covers ≥ 48 h` (`BAL_MIN_HISTORY`).
    pub valid: bool,
    /// `days_left = balance / burn` when `valid && burn > 0`, else none ("no recent use" or
    /// "not enough history yet").
    pub days_left: Option<f64>,
    /// `t_out = now + days_left`.
    pub t_out: Option<Timestamp>,
    /// `exhausted = balance ≤ 0`.
    pub exhausted: bool,
}

/// `ref` (R§3.6): the balance right after the most recent top-up (an increase ≥ 1 % of the
/// balance, `BAL_TOPUP_MIN_SHARE`; or `reported` when the service reports top-ups), else the
/// highest balance seen in the last 30 days (`BAL_REF_LOOKBACK`); never below `current`.
pub fn balance_reference(
    current: f64,
    reported: Option<TopUp>,
    history: &[Sample],
    now: Timestamp,
) -> f64 {
    let all = with_current(history, current, now);
    // The most recent top-up detected in the history: (when, balance right after it).
    let detected = all
        .windows(2)
        .rev()
        .find(|w| is_top_up(w[0].value, w[1].value))
        .map(|w| (w[1].t, w[1].value));
    let top_up = match (reported, detected) {
        (Some(r), Some((t, v))) => Some(if t > r.at { v } else { r.balance_after }),
        (Some(r), None) => Some(r.balance_after),
        (None, Some((_, v))) => Some(v),
        (None, None) => None,
    };
    let reference = top_up.unwrap_or_else(|| {
        let since = sub(now, BAL_REF_LOOKBACK);
        all.iter()
            .filter(|s| s.t >= since)
            .map(|s| s.value)
            .fold(current, f64::max)
    });
    reference.max(current)
}

/// `burn` (R§3.6): sum of decreases between consecutive samples in the last 7 days
/// (`BAL_SPAN`) divided by 7, units per day; increases are ignored.
pub fn burn_per_day(history: &[Sample], now: Timestamp) -> f64 {
    let since = sub(now, BAL_SPAN);
    let spent: f64 = history
        .windows(2)
        .filter(|w| w[0].t >= since && w[1].t <= now)
        .map(|w| (w[0].value - w[1].value).max(0.0))
        .sum();
    spent / BAL_SPAN_DAYS
}

/// All of R§3.6 for the current balance and its history (ascending, may include the current
/// reading).
pub fn balance_stats(
    current: f64,
    reported: Option<TopUp>,
    history: &[Sample],
    now: Timestamp,
) -> BalanceStats {
    let all = with_current(history, current, now);
    let reference = balance_reference(current, reported, history, now);
    let used_share = if reference > 0.0 {
        (1.0 - current / reference).clamp(0.0, 1.0)
    } else if current <= 0.0 {
        1.0
    } else {
        0.0
    };
    let burn = burn_per_day(&all, now);
    let covered = match (all.first(), all.last()) {
        (Some(a), Some(b)) => b.t.duration_since(a.t),
        _ => SignedDuration::ZERO,
    };
    let valid = covered >= BAL_MIN_HISTORY;
    let days_left = (valid && burn > 0.0).then(|| (current / burn).max(0.0));
    let t_out = days_left
        .and_then(|d| duration_from_hours(d * 24.0))
        .map(|d| add(now, d));
    BalanceStats {
        reference,
        used_share,
        burn_per_day: burn,
        valid,
        days_left,
        t_out,
        exhausted: current <= 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: i64) -> Timestamp {
        add(
            Timestamp::from_second(1_800_000_000).expect("valid"),
            SignedDuration::from_hours(24 * d),
        )
    }

    #[test]
    fn reference_prefers_the_latest_top_up_and_never_drops_below_the_balance() {
        let h = [
            Sample::balance(day(0), 1000.0),
            Sample::balance(day(1), 400.0),
            Sample::balance(day(2), 900.0), // top-up
            Sample::balance(day(3), 700.0),
        ];
        assert_eq!(balance_reference(700.0, None, &h, day(3)), 900.0);
        // A reported top-up older than the detected one loses; a newer one wins.
        let old = TopUp {
            at: day(0),
            balance_after: 5000.0,
        };
        let new = TopUp {
            at: day(3),
            balance_after: 800.0,
        };
        assert_eq!(balance_reference(700.0, Some(old), &h, day(3)), 900.0);
        assert_eq!(balance_reference(700.0, Some(new), &h, day(3)), 800.0);
        // No top-up: the highest balance in the last 30 days.
        let flat = [
            Sample::balance(day(-40), 9000.0),
            Sample::balance(day(-20), 3000.0),
            Sample::balance(day(0), 2000.0),
        ];
        assert_eq!(balance_reference(1500.0, None, &flat, day(0)), 3000.0);
        // Never below the current balance; a top-up to the current reading counts.
        assert_eq!(balance_reference(4000.0, None, &flat, day(0)), 4000.0);
        // A rise of less than 1 % is noise, not a top-up.
        let noise = [
            Sample::balance(day(0), 1000.0),
            Sample::balance(day(1), 1005.0),
        ];
        assert_eq!(balance_reference(1005.0, None, &noise, day(1)), 1005.0);
    }

    #[test]
    fn burn_ignores_top_ups_and_old_decreases() {
        let h = [
            Sample::balance(day(-9), 2000.0),
            Sample::balance(day(-8), 1500.0), // outside the 7 days
            Sample::balance(day(-6), 1400.0), // straddles the boundary: not counted
            Sample::balance(day(-5), 1300.0),
            Sample::balance(day(-4), 3000.0), // top-up ignored
            Sample::balance(day(0), 2300.0),
        ];
        assert!((burn_per_day(&h, day(0)) - 800.0 / 7.0).abs() < 1e-9);
    }

    #[test]
    fn short_history_or_no_burn_has_no_forecast() {
        let short = [Sample::balance(day(0), 100.0)];
        let s = balance_stats(
            90.0,
            None,
            &short,
            add(day(0), SignedDuration::from_hours(47)),
        );
        assert!(!s.valid);
        assert_eq!(s.days_left, None);
        assert_eq!(s.t_out, None);
        let idle = [
            Sample::balance(day(0), 100.0),
            Sample::balance(day(3), 100.0),
        ];
        let s = balance_stats(100.0, None, &idle, day(3));
        assert!(s.valid);
        assert_eq!(s.burn_per_day, 0.0);
        assert_eq!(s.days_left, None);
    }

    #[test]
    fn empty_balance_is_exhausted() {
        let h = [Sample::balance(day(0), 100.0), Sample::balance(day(3), 0.0)];
        let s = balance_stats(0.0, None, &h, day(3));
        assert!(s.exhausted);
        assert_eq!(s.used_share, 1.0);
        assert_eq!(s.days_left, Some(0.0));
        let none = balance_stats(0.0, None, &[], day(3));
        assert_eq!(none.used_share, 1.0);
    }
}
