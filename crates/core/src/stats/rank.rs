//! Ranking: auto rows and the winner (R§3.10).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::cmp::Ordering;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::MeterKey;

/// Display class, the first rank key (R§3.10): EXHAUSTED 5 > CRITICAL 4 > WARNING 3 > STALE 2 >
/// OK 1 > LOADING 0. Ordered so that `>` means "more constrained".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DisplayClass {
    /// No reading yet.
    Loading = 0,
    /// Fresh and OK (IDLE and JUST_RESET included).
    Ok = 1,
    /// The service is stale.
    Stale = 2,
    /// WARNING.
    Warning = 3,
    /// CRITICAL (level or runway).
    Critical = 4,
    /// EXHAUSTED.
    Exhausted = 5,
}

/// One rankable meter (R§3.10 "Candidates: every meter of every enabled service").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// The meter.
    pub key: MeterKey,
    /// Display class.
    pub class: DisplayClass,
    /// A balance meter: a candidate only when WARNING or worse, otherwise a filler (R§3.10
    /// "Too few candidates").
    pub is_balance: bool,
    /// `R` (EXHAUSTED: later first; `None` counts as latest. CRITICAL by level without `t_out`:
    /// earlier first; `None` last).
    pub resets_at: Option<Timestamp>,
    /// `t_out` (CRITICAL and WARNING: earlier first).
    pub t_out: Option<Timestamp>,
    /// Pressure `q = max(u, p)`; `p = u` without a valid forecast; IDLE `q = 0` (OK and STALE:
    /// higher first).
    pub pressure: f64,
    /// Balance `days_left` for the filler rule (fewest first; none last).
    pub days_left: Option<f64>,
    /// Service order from settings (tie break 1).
    pub service_order: usize,
    /// Meter order within the service (tie break 2).
    pub meter_order: u16,
}

/// Rank comparison, most constrained first (`Less` = `a` ranks before `b`), R§3.10:
///
/// 1. display class, higher first;
/// 2. within EXHAUSTED: later `R` first (`None` = latest); within CRITICAL and WARNING: earlier
///    `t_out` first (CRITICAL by level without `t_out`: use `R`; `R` none: last); within OK and
///    STALE: higher `q` first;
/// 3. ties: service order, then meter order.
pub fn compare(a: &Candidate, b: &Candidate) -> Ordering {
    todo!("M1: R§3.10 rank key")
}

/// Indices of `candidates` in rank order, applying candidate eligibility (balances only when
/// WARNING or worse) and the "too few candidates" rule (empty places go to OK balance meters,
/// fewest `days_left` first, no forecast last). LOADING meters rank last.
pub fn rank(candidates: &[Candidate]) -> Vec<usize> {
    todo!("M1: R§3.10 ranking")
}

/// The winner (tray icon) = rank 1 over all candidates, regardless of what the readout shows
/// (R§3.10). `None` when there is no meter at all (the tray shows the drop).
pub fn winner(candidates: &[Candidate]) -> Option<usize> {
    todo!("M1: R§3.10 winner")
}

/// Auto-row state carried between evaluations (R§3.10 "Auto hysteresis").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AutoRows {
    /// The meters shown, in display order (service/meter order, not rank order).
    pub shown: Vec<MeterKey>,
    /// When the last same-class swap happened (at most one per `AUTO_SWAP_INTERVAL`).
    pub last_same_class_swap: Option<Timestamp>,
}

/// Auto rows (R§3.10): the top `n` (2, or 1 in the compact form) **displayed in service/meter
/// order**, with hysteresis: a shown row is replaced only when the challenger has a higher
/// display class, or the same class and `q ≥ q_shown + AUTO_SWAP_MARGIN`; same-class swaps
/// happen at most once per `AUTO_SWAP_INTERVAL`. Returns the new state.
pub fn select_rows(
    candidates: &[Candidate],
    n: usize,
    now: Timestamp,
    prev: &AutoRows,
) -> AutoRows {
    todo!("M1: R§3.10 auto rows")
}
