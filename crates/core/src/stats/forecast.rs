//! The forecast: "at the rate you have been using it recently, when is it empty?" (R§3.5).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::history::Sample;

/// The forecast of a window meter, anchored at the latest reading `t_n` (R§3.5).
///
/// Computed per reading; between readings only `N` ([`runway_minutes`]) changes, re-evaluated on
/// every minute tick (R§12).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Forecast {
    /// `t_n`: the latest reading.
    pub anchor: Timestamp,
    /// `u_n`: used share at `t_n`.
    pub latest_used: f64,
    /// `S`: the trailing span ([`trailing_span`]).
    pub trailing: SignedDuration,
    /// `span = t_n − t_a`.
    pub span: SignedDuration,
    /// `rate = max(0, (u_n − u_a) / span)`, share per hour. A decrease inside a window instance
    /// (provider correction) gives 0.
    pub rate_per_hour: f64,
    /// `p = u_n + rate × (R − t_n)`: projected share used at the reset.
    pub projected: f64,
    /// `t_out = (rate > 0 && u_n < 1) ? t_n + (1 − u_n) / rate : none`.
    pub t_out: Option<Timestamp>,
    /// `valid = R ≠ none && span ≥ FC_MIN_SPAN && t_n − start ≥ FC_FRESH_GUARD`. Not valid → the
    /// meter can be OK or critical by level, never warning or critical by runway.
    pub valid: bool,
    /// `runs_out = valid && t_out ≠ none && t_out < R`.
    pub runs_out: bool,
}

/// `S = clamp(L / 10, 30 min, 24 h)`; `L` unknown → 1 h (R§3.5, R§3.11).
/// 5 h → 30 min, 7 d → 16.8 h, a month → 24 h.
pub fn trailing_span(window: Option<SignedDuration>) -> SignedDuration {
    todo!("M1: R§3.5 S")
}

/// Two readings belong to the same window instance when their `R` differ by at most 2 minutes
/// (R§3.1). Two `None`s are the same instance (a window without reset times); `None` and `Some`
/// are not.
pub fn same_window_instance(a: Option<Timestamp>, b: Option<Timestamp>) -> bool {
    todo!("M1: R§3.1 same window instance")
}

/// The longest suffix of `samples` (ascending) whose readings are in the same window instance as
/// the last one (R§3.1).
pub fn current_instance(samples: &[Sample]) -> &[Sample] {
    todo!("M1: R§3.1")
}

/// The forecast of R§3.5 from the readings of one window instance (ascending, latest last;
/// see [`current_instance`]):
///
/// ```text
/// S     = clamp(L / 10, 30 min, 24 h)                 // L unknown: 1 h
/// start = R − L                                       // L unknown: t of the first reading
/// a     = latest reading with t ≤ t_n − S, else the oldest reading of the instance
/// span  = t_n − t_a
/// valid = R ≠ none && span ≥ FC_MIN_SPAN && t_n − start ≥ FC_FRESH_GUARD
/// rate  = max(0, (u_n − u_a) / span)                  // share per hour
/// p     = u_n + rate × (R − t_n)
/// t_out = (rate > 0 && u_n < 1) ? t_n + (1 − u_n) / rate : none
/// ```
///
/// Returns `None` when `instance` is empty or `R` is none (no reset time → no forecast, R§3.2).
/// `window` is `L` for this instance (`None` = unknown).
pub fn forecast(
    instance: &[Sample],
    resets_at: Option<Timestamp>,
    window: Option<SignedDuration>,
) -> Option<Forecast> {
    todo!("M1: R§3.5")
}

/// `N`: whole minutes from `floor_min(now)` to `round_min(t_out)`, at least 1 — the runway as
/// shown (R§3.5, R§5.0). Runway thresholds compare `N`, so `30 min left` is always red and
/// `31 min left` never is.
pub fn runway_minutes(now: Timestamp, t_out: Timestamp) -> i64 {
    todo!("M1: R§3.5 N, R§5.0")
}
