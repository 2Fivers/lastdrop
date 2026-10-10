//! Balance meters (R§3.6).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::history::Sample;
use crate::model::TopUp;

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
    todo!("M1: R§3.6 ref")
}

/// `burn` (R§3.6): sum of decreases between consecutive samples in the last 7 days
/// (`BAL_SPAN`) divided by 7, units per day; increases are ignored.
pub fn burn_per_day(history: &[Sample], now: Timestamp) -> f64 {
    todo!("M1: R§3.6 burn")
}

/// All of R§3.6 for the current balance and its history (ascending, may include the current
/// reading).
pub fn balance_stats(
    current: f64,
    reported: Option<TopUp>,
    history: &[Sample],
    now: Timestamp,
) -> BalanceStats {
    todo!("M1: R§3.6")
}
