//! Severity with hysteresis (R§3.7).

use serde::{Deserialize, Serialize};

use crate::consts::{
    BAL_CRIT_DAYS, BAL_CRIT_EXIT_DAYS, BAL_WARN_DAYS, BAL_WARN_EXIT_DAYS, CRIT_LEVEL_EXIT_PCT,
    CRIT_LEVEL_PCT, CRIT_RUNWAY_EXIT_MIN, CRIT_RUNWAY_MIN, WARN_EXIT_P,
};
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
    let level_critical = if memory.level_critical {
        input.pct > CRIT_LEVEL_EXIT_PCT
    } else {
        input.pct >= CRIT_LEVEL_PCT
    };
    let runway_critical = input.runs_out
        && input.runway_min.is_some_and(|n| {
            if memory.runway_critical {
                n < CRIT_RUNWAY_EXIT_MIN
            } else {
                n <= CRIT_RUNWAY_MIN
            }
        });
    let warning = if memory.warning {
        input.valid && input.projected.is_some_and(|p| p > WARN_EXIT_P)
    } else {
        input.runs_out
    };
    let memory = SeverityMemory {
        level_critical,
        runway_critical,
        warning,
    };
    let (severity, cause) = if input.exhausted {
        (Severity::Exhausted, None)
    } else if level_critical || runway_critical {
        // CRITICAL_RUNWAY whenever the forecast runs out, even when the level rule also holds
        // (R§4.1: 91 % that runs out in 6 h counts down).
        let cause = if input.runs_out {
            CriticalCause::Runway
        } else {
            CriticalCause::Level
        };
        (Severity::Critical, Some(cause))
    } else if warning {
        (Severity::Warning, None)
    } else {
        (Severity::Ok, None)
    };
    SeverityOutcome {
        severity,
        cause,
        memory,
    }
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
    // A user threshold only ever raises the entry point; its exit keeps the default margin
    // (2 → 2.5 and 7 → 8 days). Judgement call: R§3.7 gives no exit for user thresholds.
    let crit_enter = alert
        .and_then(|a| a.critical_days)
        .map_or(BAL_CRIT_DAYS, |d| d.max(BAL_CRIT_DAYS));
    let warn_enter = alert
        .and_then(|a| a.warn_days)
        .map_or(BAL_WARN_DAYS, |d| d.max(BAL_WARN_DAYS));
    let crit_exit = crit_enter + (BAL_CRIT_EXIT_DAYS - BAL_CRIT_DAYS);
    let warn_exit = warn_enter + (BAL_WARN_EXIT_DAYS - BAL_WARN_DAYS);
    let (critical, warning) = match input.days_left {
        Some(d) => (
            if memory.runway_critical {
                d < crit_exit
            } else {
                d < crit_enter
            },
            if memory.warning {
                d < warn_exit
            } else {
                d < warn_enter
            },
        ),
        None => (false, false),
    };
    let memory = SeverityMemory {
        level_critical: false,
        runway_critical: critical,
        warning,
    };
    let (severity, cause) = if input.exhausted {
        (Severity::Exhausted, None)
    } else if critical {
        (Severity::Critical, Some(CriticalCause::Runway))
    } else if warning {
        (Severity::Warning, None)
    } else {
        (Severity::Ok, None)
    };
    SeverityOutcome {
        severity,
        cause,
        memory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(pct: u8, exhausted: bool) -> WindowSeverityInput {
        WindowSeverityInput {
            pct,
            exhausted,
            valid: false,
            runs_out: false,
            projected: None,
            runway_min: None,
        }
    }

    #[test]
    fn exhausted_wins_and_leaves_with_the_reading() {
        let a = window_severity(&window(100, true), SeverityMemory::default());
        assert_eq!((a.severity, a.cause), (Severity::Exhausted, None));
        // The level rule is tracked underneath, so a reading at 95 % is critical at once.
        let b = window_severity(&window(95, false), a.memory);
        assert_eq!(
            (b.severity, b.cause),
            (Severity::Critical, Some(CriticalCause::Level))
        );
        let c = window_severity(&window(10, false), b.memory);
        assert_eq!(c.severity, Severity::Ok);
    }

    #[test]
    fn warning_leaves_when_the_forecast_turns_invalid() {
        let entered = WindowSeverityInput {
            pct: 60,
            exhausted: false,
            valid: true,
            runs_out: true,
            projected: Some(1.2),
            runway_min: Some(120),
        };
        let a = window_severity(&entered, SeverityMemory::default());
        assert_eq!(a.severity, Severity::Warning);
        let invalid = WindowSeverityInput {
            valid: false,
            runs_out: false,
            ..entered
        };
        assert_eq!(window_severity(&invalid, a.memory).severity, Severity::Ok);
    }

    #[test]
    fn runway_critical_leaves_when_it_no_longer_runs_out() {
        let entered = WindowSeverityInput {
            pct: 60,
            exhausted: false,
            valid: true,
            runs_out: true,
            projected: Some(1.5),
            runway_min: Some(20),
        };
        let a = window_severity(&entered, SeverityMemory::default());
        assert_eq!(a.cause, Some(CriticalCause::Runway));
        let lasts = WindowSeverityInput {
            runs_out: false,
            projected: Some(0.99),
            ..entered
        };
        // Still WARNING by hysteresis (p = 0.99 > 0.97), no longer critical.
        assert_eq!(
            window_severity(&lasts, a.memory).severity,
            Severity::Warning
        );
    }

    fn balance(days_left: Option<f64>) -> BalanceSeverityInput {
        BalanceSeverityInput {
            exhausted: false,
            days_left,
        }
    }

    #[test]
    fn balance_thresholds_and_hysteresis() {
        let ok = balance_severity(&balance(Some(7.0)), SeverityMemory::default(), None);
        assert_eq!(ok.severity, Severity::Ok);
        let w = balance_severity(&balance(Some(6.9)), SeverityMemory::default(), None);
        assert_eq!(w.severity, Severity::Warning);
        let still = balance_severity(&balance(Some(7.9)), w.memory, None);
        assert_eq!(still.severity, Severity::Warning);
        let cleared = balance_severity(&balance(Some(8.0)), still.memory, None);
        assert_eq!(cleared.severity, Severity::Ok);

        let c = balance_severity(&balance(Some(1.9)), SeverityMemory::default(), None);
        assert_eq!(
            (c.severity, c.cause),
            (Severity::Critical, Some(CriticalCause::Runway))
        );
        let c2 = balance_severity(&balance(Some(2.4)), c.memory, None);
        assert_eq!(c2.severity, Severity::Critical);
        let c3 = balance_severity(&balance(Some(2.5)), c2.memory, None);
        assert_eq!(c3.severity, Severity::Warning);
        // No forecast: OK, memory cleared.
        let none = balance_severity(&balance(None), c3.memory, None);
        assert_eq!(none.severity, Severity::Ok);
        assert_eq!(none.memory, SeverityMemory::default());
        let empty = BalanceSeverityInput {
            exhausted: true,
            days_left: Some(0.0),
        };
        assert_eq!(
            balance_severity(&empty, SeverityMemory::default(), None).severity,
            Severity::Exhausted
        );
    }

    #[test]
    fn user_thresholds_raise_never_lower() {
        let earlier = BalanceAlert {
            warn_days: Some(14.0),
            critical_days: Some(5.0),
        };
        let s = balance_severity(
            &balance(Some(10.0)),
            SeverityMemory::default(),
            Some(&earlier),
        );
        assert_eq!(s.severity, Severity::Warning);
        let s = balance_severity(
            &balance(Some(4.0)),
            SeverityMemory::default(),
            Some(&earlier),
        );
        assert_eq!(s.severity, Severity::Critical);
        let s = balance_severity(&balance(Some(5.4)), s.memory, Some(&earlier));
        assert_eq!(s.severity, Severity::Critical, "exit at 5.5");
        let later = BalanceAlert {
            warn_days: Some(1.0),
            critical_days: Some(0.5),
        };
        let s = balance_severity(&balance(Some(1.5)), SeverityMemory::default(), Some(&later));
        assert_eq!(s.severity, Severity::Critical, "never lowered below 2 days");
    }
}
