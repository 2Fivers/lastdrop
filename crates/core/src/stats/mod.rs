//! Meter statistics: elapsed share, display percent, pace, forecast, balances, severity with
//! hysteresis, freshness, window resets, ranking, and the per-meter [`Assessment`] that puts
//! them together (R§3, normative; "each item is a unit test").
//!
//! Every function is pure: inputs are values, `now` is a parameter, state that must survive
//! between evaluations (severity hysteresis, auto-row hysteresis) is passed in and returned
//! ([`SeverityMemory`], [`AutoRows`], [`EvalMemory`]).
//!
//! The constants live in [`crate::consts`] (R§3.11).

mod assess;
mod balance;
mod forecast;
mod freshness;
mod history;
mod rank;
mod reset;
mod severity;
mod window;

pub(crate) use assess::candidate;
pub use assess::{
    AbsentMeter, AssessedMeter, Assessment, BalanceAssessment, EvalInput, EvalMemory, Evaluation,
    LiveChange, RowState, ServiceEval, ServiceInput, WindowAssessment, evaluate,
};
pub use balance::{BalanceStats, balance_reference, balance_stats, burn_per_day};
pub use forecast::{
    Forecast, current_instance, forecast, instance_of, latest_instance, runway_minutes,
    same_window_instance, trailing_span,
};
pub use freshness::{
    FailureKind, Freshness, ServiceStatus, StaleReason, freshness, stale_after, stale_after_paced,
};
pub use history::{HistoryView, NoHistory, Sample};
pub use rank::{
    AutoRows, Candidate, DisplayClass, compare, rank, select_rows, select_rows_ranked, winner,
};
pub use reset::{EffectiveWindow, ResetView, effective_window, reset_refresh_at};
pub use severity::{
    BalanceSeverityInput, CriticalCause, Severity, SeverityMemory, SeverityOutcome,
    WindowSeverityInput, balance_severity, window_severity,
};
pub use window::{Pace, display_pct, elapsed, elapsed_share, is_exhausted, pace};
