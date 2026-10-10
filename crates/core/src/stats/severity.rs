//! Severity with hysteresis (R§3.7).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use serde::{Deserialize, Serialize};

use crate::settings::BalanceAlert;

/// Severity of a meter from fresh data (R§3.7). Stale meters have none (R§3.8).
///
/// Ordered by precedence: `Ok < Warning < Critical < Exhausted`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// Nothing wrong.
    Ok,
    /// Runs out before the reset at the recent pace (`runs_out`, equivalently `p > 1`); balance:
    /// `days_left < 7`.
    Warning,
    /// `pct ≥ 90` (level) or `runs_out && N ≤ 30` (runway); balance: `days_left < 2`.
    Critical,
    /// Used up: `limited || u ≥ 1`; balance `≤ 0`.
    Exhausted,
}

/// Why a meter is CRITICAL, which decides its row look (R§4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CriticalCause {
    /// CRITICAL_LEVEL: `pct ≥ 90` while `!runs_out` — shows `92%` semibold + `↺16:30`.
    Level,
    /// CRITICAL_RUNWAY: CRITICAL with `runs_out` (the level rule may also hold: 91 % that runs
    /// out in 6 h counts down, `6 h left`). Balances that are CRITICAL use this look too.
    Runway,
}

/// Hysteresis state of one meter: which rules are currently "entered" (R§3.7 "keep the previous
/// severity per meter to apply them"). Cleared on a window reset (R§3.9) and kept in
/// [`super::EvalMemory`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SeverityMemory {
    /// Critical by level entered and not yet left (`pct ≤ 87` leaves).
    pub level_critical: bool,
    /// Critical by runway entered and not yet left (`N ≥ 36` or `!runs_out` leaves).
    pub runway_critical: bool,
    /// Warning entered and not yet left (`p ≤ 0.97` or `!valid` leaves). For balances: the
    /// warning rule; `runway_critical` holds the balance critical rule.
    pub warning: bool,
}

/// Inputs of [`window_severity`] for one evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowSeverityInput {
    /// Shown percent ([`super::display_pct`]).
    pub pct: u8,
    /// [`super::is_exhausted`].
    pub exhausted: bool,
    /// `valid` of the forecast (false without one).
    pub valid: bool,
    /// `runs_out` of the forecast (false without one).
    pub runs_out: bool,
    /// `p` of the forecast, if any.
    pub projected: Option<f64>,
    /// `N` at `now` when `t_out` exists ([`super::runway_minutes`]).
    pub runway_min: Option<i64>,
}

/// Result of a severity evaluation: the severity, why it is critical, and the hysteresis state to
/// keep for the next evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeverityOutcome {
    /// The severity.
    pub severity: Severity,
    /// Set when `severity` is CRITICAL.
    pub cause: Option<CriticalCause>,
    /// State to pass to the next evaluation of the same meter.
    pub memory: SeverityMemory,
}

/// Window meter severity, R§3.7:
///
/// | Severity  | enters when                                   | leaves when                                       |
/// |-----------|-----------------------------------------------|---------------------------------------------------|
/// | EXHAUSTED | `exhausted`                                   | a reading shows `!exhausted`, or the window resets |
/// | CRITICAL  | `pct ≥ 90` (level), or `runs_out && N ≤ 30` (runway) | level: `pct ≤ 87`; runway: `N ≥ 36` or `!runs_out` |
/// | WARNING   | `runs_out` (equivalently `p > 1`)             | `p ≤ 0.97` or `!valid`                            |
/// | OK        | otherwise                                     |                                                   |
///
/// Precedence EXHAUSTED > CRITICAL > WARNING > OK. Test vectors (R§13): WARNING at `p = 1.02`
/// stays at `0.98`, clears at `0.97`; runway CRITICAL entered at N 30 stays at N 35, clears at
/// N 36 (N 31 never enters); level CRITICAL at 90 stays at 88, clears at 87.
pub fn window_severity(input: &WindowSeverityInput, memory: SeverityMemory) -> SeverityOutcome {
    todo!("M1: R§3.7 window severity")
}

/// Inputs of [`balance_severity`].
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalanceSeverityInput {
    /// `balance ≤ 0`.
    pub exhausted: bool,
    /// `days_left` (R§3.6), none without a forecast.
    pub days_left: Option<f64>,
}

/// Balance meter severity, R§3.7: EXHAUSTED at `balance ≤ 0`; CRITICAL at `days_left < 2`
/// (leaves at `≥ 2.5`); WARNING at `days_left < 7` (leaves at `≥ 8`); a top-up re-evaluates at
/// once. `alert` (settings) may raise WARNING/CRITICAL earlier, never lower them.
pub fn balance_severity(
    input: &BalanceSeverityInput,
    memory: SeverityMemory,
    alert: Option<&BalanceAlert>,
) -> SeverityOutcome {
    todo!("M1: R§3.7 balance severity")
}
