//! The assessment of every meter and the evaluation of all services in one pass (ARCHITECTURE
//! "Assessment"; R§3, R§4.1).
//!
//! [`evaluate`] is the single entry point the app calls on every new reading and on every minute
//! tick (R§12). Its [`Evaluation`] is the input of every surface builder
//! ([`crate::readout`], [`crate::flyout`]).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::collections::BTreeMap;

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::balance::BalanceStats;
use super::forecast::Forecast;
use super::freshness::{Freshness, ServiceStatus, StaleReason};
use super::history::HistoryView;
use super::rank::{AutoRows, DisplayClass};
use super::reset::ResetView;
use super::severity::{CriticalCause, Severity, SeverityMemory};
use super::window::Pace;
use crate::model::{AccountRef, MeterKey, MeterNames, ProviderId, Snapshot, SourceKey, Unit};
use crate::settings::Settings;

/// Row display state of one meter (R§4.1). The row's look is chosen by R§5.7's table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RowState {
    /// No reading yet: track only, `Reading…` / `–`.
    Loading,
    /// Fresh, OK, with a reset time: `42%` + `↺14:56`.
    Ok,
    /// Fresh, OK, the provider gave no reset time (or `L` unknown): `42%` + `–`, no tick.
    OkNoReset,
    /// `u = 0` with no reset time, window not started (R§3.2): `0%` + `–`, track only.
    Idle,
    /// A balance meter that is OK: `62%` + `≈9d`.
    OkBalance,
    /// WARNING: ▲ + `Out 15:20`, or `45 min left` when `N ≤ 59`.
    Warning,
    /// CRITICAL by level while `!runs_out`: `92%` semibold + `↺16:30`.
    CriticalLevel,
    /// CRITICAL with `runs_out` (and CRITICAL balances): ● + countdown, always.
    CriticalRunway,
    /// EXHAUSTED: ● + `Back 16:30` (no reset time or balance: ● + `Empty`).
    Exhausted,
    /// A fixed-schedule window whose `R` passed with no newer reading: `0%` + `↺7d` (R§3.9).
    JustReset,
    /// The service is stale: the last reading frozen at `t_r`, dashed, stale colour (R§3.8).
    Stale(StaleReason),
}

/// One window meter as assessed at `now` (R§3.2–R§3.5, R§3.7, R§3.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowAssessment {
    /// `u` to show (after reset handling, R§3.9).
    pub used: f64,
    /// `pct` (R§3.3).
    pub pct: u8,
    /// `exhausted` (R§3.3).
    pub exhausted: bool,
    /// `R` to show (after reset handling).
    pub resets_at: Option<Timestamp>,
    /// `L` of the shown instance, `None` = unknown.
    pub window: Option<SignedDuration>,
    /// The meter definition's `starts_on_use`.
    pub starts_on_use: bool,
    /// `e` at `now` (fresh) or at `t_r` (stale); none without `R` or `L` (R§3.2).
    pub elapsed: Option<f64>,
    /// Pace when `e` exists (R§3.4).
    pub pace: Option<Pace>,
    /// Reset handling that applied (R§3.9).
    pub reset: ResetView,
    /// The forecast (R§3.5); `None` for stale meters, without `R`, or after a reset.
    pub forecast: Option<Forecast>,
    /// `N` at `now` when the forecast has a `t_out`.
    pub runway_min: Option<i64>,
    /// Why it is CRITICAL, when it is.
    pub cause: Option<CriticalCause>,
    /// `t_r` of the reading.
    pub reading_at: Timestamp,
    /// The first reading of this window instance that was exhausted, if history has it
    /// (`Used up since 14:58`, F§4.4; omitted when unknown).
    pub exhausted_since: Option<Timestamp>,
}

/// One balance meter as assessed at `now` (R§3.6–R§3.7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BalanceAssessment {
    /// Current amount.
    pub amount: f64,
    /// Unit of the amount.
    pub unit: Unit,
    /// R§3.6 statistics.
    pub stats: BalanceStats,
    /// `pct` of `stats.used_share` (R§3.3 rules, the share of the top-up spent).
    pub pct: u8,
    /// `t_r` of the reading.
    pub reading_at: Timestamp,
}

/// The kind-specific part of an [`Assessment`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum AssessedMeter {
    /// A window meter.
    Window(WindowAssessment),
    /// A balance meter.
    Balance(BalanceAssessment),
    /// No reading yet (the service's first fetch is running).
    Loading,
}

/// A meter as assessed at `now`: everything the grammar and the surface builders need, so they
/// contain no statistics of their own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assessment {
    /// The meter.
    pub key: MeterKey,
    /// Its names (R§2).
    pub names: MeterNames,
    /// Service order (settings).
    pub service_order: usize,
    /// Meter order within the service.
    pub meter_order: u16,
    /// Kind-specific values.
    pub meter: AssessedMeter,
    /// Severity from fresh data; `None` when stale or loading (R§3.7, R§3.8).
    pub severity: Option<Severity>,
    /// Row display state (R§4.1).
    pub state: RowState,
    /// Display class for ranking (R§3.10).
    pub class: DisplayClass,
    /// Pressure `q = max(u, p)` (R§3.10).
    pub pressure: f64,
}

/// One service as evaluated at `now`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServiceEval {
    /// The credential source.
    pub source: SourceKey,
    /// The account as shown (alias, plan).
    pub account: AccountRef,
    /// Service order (settings).
    pub service_order: usize,
    /// R§3.8 freshness (meaningless while `loading`).
    pub freshness: Freshness,
    /// No reading yet and the first fetch is running (R§4.1 LOADING).
    pub loading: bool,
    /// No reading yet and the first fetch failed (F§4.3 "Couldn't read Claude").
    pub first_fetch_failed: bool,
    /// The scheduler's view (last success, failure detail, next try) for message lines.
    pub status: ServiceStatus,
    /// `t_r` of the reading shown (the snapshot's `measured_at`).
    pub reading_at: Option<Timestamp>,
    /// Every meter of the service that is not hidden, in meter order.
    pub meters: Vec<Assessment>,
}

impl ServiceEval {
    /// The service name with the account alias when there is one (`Claude · Work`, R§6.4).
    pub fn display_name(&self) -> String {
        match &self.account.alias {
            Some(alias) => format!("{} · {alias}", self.source.provider.display_name()),
            None => self.source.provider.display_name().to_owned(),
        }
    }
}

/// A change that the readout's UI Automation root announces with `LiveRegionChanged` (R§11:
/// "only when a row's severity rises, a service turns stale or fresh, or a window resets — not
/// on value changes"). The flyout announces severity rises the same way (F§10).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveChange {
    /// A meter's severity rose.
    SeverityRose(MeterKey),
    /// A service turned stale or fresh.
    FreshnessChanged(SourceKey),
    /// A window reset (R§3.9).
    WindowReset(MeterKey),
}

/// Inputs for one service.
#[derive(Clone, Copy, Debug)]
pub struct ServiceInput<'a> {
    /// The service.
    pub provider: ProviderId,
    /// The account as configured (alias, tag); the snapshot's plan is used when present.
    pub account: &'a AccountRef,
    /// The latest successful reading, if any.
    pub snapshot: Option<&'a Snapshot>,
    /// The scheduler's status of this source.
    pub status: &'a ServiceStatus,
    /// A fetch of this source is running.
    pub fetching: bool,
}

/// Everything [`evaluate`] reads.
#[derive(Clone, Copy)]
pub struct EvalInput<'a> {
    /// The clock (UTC).
    pub now: Timestamp,
    /// Settings: service order, hidden meters, pins, balance alerts.
    pub settings: &'a Settings,
    /// Every enabled service, in service order.
    pub services: &'a [ServiceInput<'a>],
    /// History of every meter.
    pub history: &'a dyn HistoryView,
}

/// State that survives between evaluations (owned by the app's state, main thread).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvalMemory {
    /// Severity hysteresis per meter (R§3.7); cleared for a meter on its window reset (R§3.9).
    pub severity: BTreeMap<MeterKey, SeverityMemory>,
    /// Auto-row hysteresis (R§3.10).
    pub auto_rows: AutoRows,
    /// Severity per meter at the previous evaluation, to detect rises ([`LiveChange`]).
    pub last_severity: BTreeMap<MeterKey, Severity>,
    /// Freshness per service at the previous evaluation.
    pub last_freshness: BTreeMap<SourceKey, Freshness>,
    /// The window instance each meter's severity memory belongs to: the `R` shown at the
    /// previous evaluation (the reading's `resets_at`, `R'` after a fixed-schedule reset, `None`
    /// without a reset time). A change of instance is a window reset (R§3.9).
    pub instance: BTreeMap<MeterKey, Option<Timestamp>>,
}

/// The result of one evaluation, input of every surface builder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    /// The `now` it was computed for.
    pub now: Timestamp,
    /// Every enabled service, in service order.
    pub services: Vec<ServiceEval>,
    /// Every candidate meter, most constrained first (R§3.10).
    pub ranked: Vec<MeterKey>,
    /// The winner: rank 1, drives the tray icon (R§3.10). `None` = no meter at all.
    pub winner: Option<MeterKey>,
    /// The readout rows in display order: the pins (R§4.3) or the auto rows (R§3.10), 0–2.
    pub readout_rows: Vec<MeterKey>,
    /// Rows can come from more than one service, so the readout shows service glyphs (R§6.4;
    /// decided from what *can* appear, not what does).
    pub multi_service: bool,
    /// Changes to announce (R§11).
    pub changes: Vec<LiveChange>,
}

impl Evaluation {
    /// The assessment of a meter, if it is in the evaluation.
    pub fn assessment(&self, key: &MeterKey) -> Option<(&ServiceEval, &Assessment)> {
        self.services
            .iter()
            .find_map(|s| s.meters.iter().find(|a| &a.key == key).map(|a| (s, a)))
    }
}

/// Assesses every meter of every enabled service at `input.now` and ranks them (R§3, R§4.1,
/// R§4.3). Updates `memory` (severity and auto-row hysteresis, previous severities for
/// [`LiveChange`]).
///
/// Order of work per meter: reset handling (R§3.9) → `pct`, `e`, pace → forecast from the current
/// window instance's history, unless stale (R§3.5, R§3.8) → `N` at `now` → severity with
/// hysteresis (R§3.7) → row state (R§4.1) → class and pressure (R§3.10). Then ranking, winner and
/// readout rows.
///
/// - `t_r` is the snapshot's `measured_at` ([`crate::model::Snapshot::reading_at`]); the history
///   must already contain the latest reading (the app records it before evaluating).
/// - A service without a snapshot gets one [`AssessedMeter::Loading`] assessment per catalogue
///   meter ([`crate::model::catalog`], hidden meters omitted) while `fetching` or before its
///   first attempt, so LOADING rows have labels (R§3.10 "the readout shows LOADING rows while a
///   first fetch runs").
/// - Window resets: when a meter's shown window instance differs from
///   `memory.instance` (R§3.1 `same_window_instance` is false, or [`ResetView`] is not `Live`
///   for the first time), its [`SeverityMemory`] is cleared (R§3.7 "On a window reset … cleared")
///   and [`LiveChange::WindowReset`] is raised.
pub fn evaluate(input: &EvalInput<'_>, memory: &mut EvalMemory) -> Evaluation {
    todo!("M1: R§3 / R§4 evaluation")
}
