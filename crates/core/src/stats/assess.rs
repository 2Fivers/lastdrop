//! The assessment of every meter and the evaluation of all services in one pass (ARCHITECTURE
//! "Assessment"; R§3, R§4.1).
//!
//! [`evaluate`] is the single entry point the app calls on every new reading and on every minute
//! tick (R§12). Its [`Evaluation`] is the input of every surface builder
//! ([`crate::readout`], [`crate::flyout`]).

use std::collections::{BTreeMap, BTreeSet};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use super::balance::{BalanceStats, balance_stats, is_top_up};
use super::forecast::{Forecast, current_instance, forecast, runway_minutes, same_window_instance};
use super::freshness::{Freshness, ServiceStatus, StaleReason, freshness};
use super::history::{HistoryView, Sample};
use super::rank::{AutoRows, Candidate, DisplayClass, rank, select_rows};
use super::reset::{EffectiveWindow, ResetView, effective_window};
use super::severity::{
    BalanceSeverityInput, CriticalCause, Severity, SeverityMemory, SeverityOutcome,
    WindowSeverityInput, balance_severity, window_severity,
};
use super::window::{Pace, display_pct, elapsed, is_exhausted, pace};
use crate::consts::{AUTO_ROWS, BAL_REF_LOOKBACK, SAME_WINDOW_TOLERANCE};
use crate::model::{
    AccountRef, Balance, Meter, MeterId, MeterKey, MeterKind, MeterNames, ProviderId, Snapshot,
    SourceKey, Unit, catalog,
};
use crate::settings::{BalanceAlert, ReadoutContent, Settings};
use crate::util::{sane_share, sub};

/// How far back the history of a window without a known start is read (the store keeps the
/// current instance, ≤ 31 days, in memory).
const INSTANCE_LOOKBACK: SignedDuration = SignedDuration::from_hours(32 * 24);

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
    /// Meters this source reported earlier (in this run, or in an earlier one the app restored
    /// into [`EvalMemory::seen`]) that its latest reading lacks (not hidden): a per-model weekly
    /// that only the OAuth source reports while Claude Desktop's history answers. They *can*
    /// appear, so the readout's widths keep room for them (R§6.3: widths never change with data).
    pub absent: Vec<AbsentMeter>,
}

/// A meter a source reported before but not in its latest reading ([`ServiceEval::absent`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbsentMeter {
    /// The meter.
    pub key: MeterKey,
    /// Its names as last reported.
    pub names: MeterNames,
    /// It is a balance meter.
    pub balance: bool,
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
    /// Every meter a source has reported, with its names and whether it is a balance
    /// ([`ServiceEval::absent`]): in this run, plus what the app restored from its state file
    /// (earlier runs, R§6.3).
    pub seen: BTreeMap<MeterKey, (MeterNames, bool)>,
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
/// - A reading without a reset time that was taken before the reset of the instance shown so far
///   (history or `memory.instance`) belongs to that instance and keeps its `R`
///   ([`super::instance_of`]); it is not a window reset.
pub fn evaluate(input: &EvalInput<'_>, memory: &mut EvalMemory) -> Evaluation {
    let mut changes = Vec::new();
    let services: Vec<ServiceEval> = input
        .services
        .iter()
        .enumerate()
        .map(|(order, svc)| evaluate_service(input, svc, order, memory, &mut changes))
        .collect();

    let candidates: Vec<Candidate> = services
        .iter()
        .flat_map(|s| s.meters.iter())
        .map(candidate)
        .collect();
    let order = rank(&candidates);
    let ranked: Vec<MeterKey> = order.iter().map(|&i| candidates[i].key.clone()).collect();
    let winner = ranked.first().cloned();

    let (readout_rows, pinned) = readout_rows(input, &services, &candidates, memory);
    let multi_service = if pinned {
        distinct_providers(readout_rows.iter().map(|k| k.provider)) > 1
    } else {
        distinct_providers(
            services
                .iter()
                .filter(|s| can_appear(input.settings, s))
                .map(|s| s.source.provider),
        ) > 1
    };

    Evaluation {
        now: input.now,
        services,
        ranked,
        winner,
        readout_rows,
        multi_service,
        changes,
    }
}

/// The number of different services in `providers`.
fn distinct_providers(providers: impl Iterator<Item = ProviderId>) -> usize {
    providers.collect::<BTreeSet<_>>().len()
}

/// The service has a meter that can appear in the auto readout (R§6.4 "decided from what *can*
/// appear, not what does"): any meter it reported, or a catalogue meter that is not hidden.
fn can_appear(settings: &Settings, service: &ServiceEval) -> bool {
    let hidden = hidden_meters(settings, service.source.provider);
    !service.meters.is_empty()
        || catalog(service.source.provider)
            .iter()
            .any(|d| !hidden.iter().any(|h| h.0 == d.id))
}

/// The meters the user hid for a service (F§2.2).
fn hidden_meters(settings: &Settings, provider: ProviderId) -> &[MeterId] {
    settings
        .provider(provider)
        .map_or(&[], |p| p.hidden_meters.as_slice())
}

/// The readout rows (R§4.3): the pins that are evaluated, in the user's order; else the auto rows
/// with hysteresis (R§3.10). Judgement call: pins of which none is available (service disabled,
/// a per-model meter not reported yet) fall back to the auto rows instead of hiding the readout.
/// Returns the rows and whether they are pins.
fn readout_rows(
    input: &EvalInput<'_>,
    services: &[ServiceEval],
    candidates: &[Candidate],
    memory: &mut EvalMemory,
) -> (Vec<MeterKey>, bool) {
    if let ReadoutContent::Pinned(pins) = &input.settings.readout.content {
        let mut rows: Vec<MeterKey> = Vec::new();
        for pin in pins {
            let present = services
                .iter()
                .any(|s| s.meters.iter().any(|a| &a.key == pin));
            if present && !rows.contains(pin) && rows.len() < AUTO_ROWS {
                rows.push(pin.clone());
            }
        }
        if !rows.is_empty() {
            return (rows, true);
        }
    }
    let auto = select_rows(candidates, AUTO_ROWS, input.now, &memory.auto_rows);
    let rows = auto.shown.clone();
    memory.auto_rows = auto;
    (rows, false)
}

/// Whether a meter id of a provider is a balance in the catalogue (for LOADING meters, which
/// carry no reading yet).
fn catalogue_is_balance(key: &MeterKey) -> bool {
    catalog(key.provider)
        .iter()
        .any(|d| d.id == key.meter.0 && d.kind == MeterKind::Balance)
}

/// The rank candidate of an assessment (R§3.10).
pub(crate) fn candidate(a: &Assessment) -> Candidate {
    let (is_balance, resets_at, t_out, days_left) = match &a.meter {
        AssessedMeter::Window(w) => (
            false,
            w.resets_at,
            // A run-out after the reset does not count: critical by level then ranks by `R`.
            w.forecast.filter(|f| f.runs_out).and_then(|f| f.t_out),
            None,
        ),
        AssessedMeter::Balance(b) => (true, None, b.stats.t_out, b.stats.days_left),
        AssessedMeter::Loading => (catalogue_is_balance(&a.key), None, None, None),
    };
    Candidate {
        key: a.key.clone(),
        class: a.class,
        is_balance,
        resets_at,
        t_out,
        pressure: a.pressure,
        days_left,
        service_order: a.service_order,
        meter_order: a.meter_order,
    }
}

/// What every meter of one service is assessed with.
struct MeterContext<'a> {
    now: Timestamp,
    /// `t_r` of the snapshot.
    reading_at: Timestamp,
    /// `Some` when the service is stale.
    stale: Option<StaleReason>,
    service_order: usize,
    history: &'a dyn HistoryView,
    alert: Option<&'a BalanceAlert>,
}

/// One service: LOADING meters before the first reading, else every meter of the snapshot.
fn evaluate_service(
    input: &EvalInput<'_>,
    svc: &ServiceInput<'_>,
    order: usize,
    memory: &mut EvalMemory,
    changes: &mut Vec<LiveChange>,
) -> ServiceEval {
    let hidden = hidden_meters(input.settings, svc.provider);
    let source = SourceKey {
        provider: svc.provider,
        account: svc.account.key.clone(),
    };
    let mut account = svc.account.clone();

    let Some(snapshot) = svc.snapshot else {
        let failed = !svc.fetching && svc.status.last_failure.is_some();
        let meters = if failed {
            Vec::new()
        } else {
            catalog(svc.provider)
                .iter()
                .filter(|d| !hidden.iter().any(|h| h.0 == d.id))
                .map(|d| Assessment {
                    key: MeterKey {
                        provider: svc.provider,
                        account: svc.account.key.clone(),
                        meter: d.meter_id(),
                    },
                    names: d.names(),
                    service_order: order,
                    meter_order: d.order,
                    meter: AssessedMeter::Loading,
                    severity: None,
                    state: RowState::Loading,
                    class: DisplayClass::Loading,
                    pressure: 0.0,
                })
                .collect()
        };
        let absent = absent_meters(memory, &source, &meters, hidden);
        return ServiceEval {
            source,
            account,
            service_order: order,
            freshness: Freshness::Fresh,
            loading: !failed,
            first_fetch_failed: failed,
            status: svc.status.clone(),
            reading_at: None,
            meters,
            absent,
        };
    };

    if snapshot.account.plan.is_some() {
        account.plan.clone_from(&snapshot.account.plan);
    }
    let reading_at = snapshot.reading_at();
    let mut status = svc.status.clone();
    if status.last_success.is_none() {
        status.last_success = Some(reading_at);
    }
    let fresh = freshness(input.now, &status);
    if let Some(previous) = memory.last_freshness.insert(source.clone(), fresh)
        && (previous == Freshness::Fresh) != (fresh == Freshness::Fresh)
    {
        changes.push(LiveChange::FreshnessChanged(source.clone()));
    }

    let cx = MeterContext {
        now: input.now,
        reading_at,
        stale: match fresh {
            Freshness::Stale(reason) => Some(reason),
            Freshness::Fresh => None,
        },
        service_order: order,
        history: input.history,
        alert: input
            .settings
            .provider(svc.provider)
            .and_then(|p| p.balance_alert.as_ref()),
    };
    let key = |id: &MeterId| MeterKey {
        provider: svc.provider,
        account: svc.account.key.clone(),
        meter: id.clone(),
    };
    let mut meters: Vec<Assessment> = Vec::new();
    for meter in snapshot.meters.iter().filter(|m| !hidden.contains(&m.id)) {
        meters.push(assess_window(meter, key(&meter.id), &cx, memory, changes));
    }
    for balance in snapshot.balances.iter().filter(|b| !hidden.contains(&b.id)) {
        meters.push(assess_balance(
            balance,
            key(&balance.id),
            &cx,
            memory,
            changes,
        ));
    }
    meters.sort_by_key(|a| a.meter_order);
    for meter in &snapshot.meters {
        memory
            .seen
            .insert(key(&meter.id), (meter.names.clone(), false));
    }
    for balance in &snapshot.balances {
        memory
            .seen
            .insert(key(&balance.id), (balance.names.clone(), true));
    }
    let absent = absent_meters(memory, &source, &meters, hidden);

    ServiceEval {
        source,
        account,
        service_order: order,
        freshness: fresh,
        loading: false,
        first_fetch_failed: false,
        status,
        reading_at: Some(reading_at),
        meters,
        absent,
    }
}

/// [`ServiceEval::absent`]: the meters of `source` in `memory.seen` that `reported` lacks and the
/// user did not hide.
fn absent_meters(
    memory: &EvalMemory,
    source: &SourceKey,
    reported: &[Assessment],
    hidden: &[MeterId],
) -> Vec<AbsentMeter> {
    memory
        .seen
        .iter()
        .filter(|(k, _)| k.provider == source.provider && k.account == source.account)
        .filter(|(k, _)| !hidden.contains(&k.meter) && !reported.iter().any(|a| &a.key == *k))
        .map(|(k, (names, balance))| AbsentMeter {
            key: k.clone(),
            names: names.clone(),
            balance: *balance,
        })
        .collect()
}

/// The readings of the window instance the meter's reading belongs to, ending with that reading
/// (added when the history does not have it yet).
fn instance_samples(
    history: &dyn HistoryView,
    key: &MeterKey,
    meter: &Meter,
    eff: &EffectiveWindow,
    reading_at: Timestamp,
) -> Vec<Sample> {
    let since = match (eff.resets_at, eff.window) {
        (Some(r), Some(len)) => sub(sub(r, len), SAME_WINDOW_TOLERANCE),
        _ => sub(reading_at, INSTANCE_LOOKBACK),
    };
    let mut all: Vec<Sample> = history
        .samples(key, since)
        .iter()
        .filter(|s| s.t <= reading_at)
        .copied()
        .collect();
    if all.last().is_none_or(|s| s.t < reading_at) {
        all.push(Sample::window(reading_at, meter.used, meter.resets_at));
    }
    current_instance(&all).to_vec()
}

/// The reset time of a reading that has none but was taken inside a known window instance
/// ([`super::instance_of`]): the `R` of the newest earlier reading that has one (from the
/// history, or the instance the previous evaluation showed), when the reading precedes it.
///
/// A source without reset times (Claude Desktop's history standing in while the OAuth source
/// rests after an error) must not look like a window reset: that would clear the hysteresis,
/// announce a reset and cut the forecast's history (R§3.7, R§3.9, R§11). **Not from the spec.**
fn carried_reset(
    history: &dyn HistoryView,
    key: &MeterKey,
    reading_at: Timestamp,
    shown_before: Option<Timestamp>,
) -> Option<Timestamp> {
    let samples = history.samples(key, sub(reading_at, INSTANCE_LOOKBACK));
    let upto = &samples[..samples.partition_point(|s| s.t <= reading_at)];
    let known = upto.iter().rev().find_map(|s| s.resets_at);
    known
        .into_iter()
        .chain(shown_before)
        .filter(|&r| reading_at < r)
        .max()
}

/// Records the severity of a fresh meter and raises [`LiveChange::SeverityRose`] on a rise.
fn record_severity(
    key: &MeterKey,
    outcome: &SeverityOutcome,
    memory: &mut EvalMemory,
    changes: &mut Vec<LiveChange>,
) {
    memory.severity.insert(key.clone(), outcome.memory);
    let previous = memory
        .last_severity
        .insert(key.clone(), outcome.severity)
        .unwrap_or(Severity::Ok);
    if outcome.severity > previous {
        changes.push(LiveChange::SeverityRose(key.clone()));
    }
}

/// The display class of a fresh severity.
fn class_of(severity: Severity) -> DisplayClass {
    match severity {
        Severity::Ok => DisplayClass::Ok,
        Severity::Warning => DisplayClass::Warning,
        Severity::Critical => DisplayClass::Critical,
        Severity::Exhausted => DisplayClass::Exhausted,
    }
}

/// R§3.2–R§3.5, R§3.7, R§3.9 and R§4.1 for one window meter.
fn assess_window(
    meter: &Meter,
    key: MeterKey,
    cx: &MeterContext<'_>,
    memory: &mut EvalMemory,
    changes: &mut Vec<LiveChange>,
) -> Assessment {
    // A reading without a reset time inside a known instance keeps that instance's `R`.
    let with_reset;
    let meter = match meter.resets_at {
        None => match carried_reset(
            cx.history,
            &key,
            cx.reading_at,
            memory.instance.get(&key).copied().flatten(),
        ) {
            Some(r) => {
                with_reset = Meter {
                    resets_at: Some(r),
                    ..meter.clone()
                };
                &with_reset
            }
            None => meter,
        },
        Some(_) => meter,
    };
    let eff = effective_window(meter, cx.now);
    let live = eff.view == ResetView::Live;
    let used = sane_share(eff.used);
    let exhausted = is_exhausted(used, meter.limited && live);
    let pct = display_pct(used, exhausted);
    // Stale meters are frozen at the reading; one drawn as reset shows the new window now.
    let e_at = if cx.stale.is_some() && live {
        cx.reading_at
    } else {
        cx.now
    };
    let elapsed = elapsed(eff.resets_at, eff.window, e_at);
    let pace = elapsed.map(|e| pace(used, e));

    let samples = if live {
        instance_samples(cx.history, &key, meter, &eff, cx.reading_at)
    } else {
        Vec::new()
    };
    let forecast = if live && cx.stale.is_none() {
        forecast(&samples, eff.resets_at, eff.window)
    } else {
        None
    };
    let runway_min = forecast
        .and_then(|f| f.t_out)
        .map(|t| runway_minutes(cx.now, t));
    let exhausted_since = if live && exhausted {
        // The start of the trailing run of used-up readings of this instance, known only when
        // the history also has the reading before it (the moment it ran out was observed).
        let run = samples.iter().rev().take_while(|s| s.value >= 1.0).count();
        (run > 0 && run < samples.len()).then(|| samples[samples.len() - run].t)
    } else {
        None
    };

    // A new window instance clears the hysteresis (R§3.7, R§3.9); a reset time that passed or
    // changed is announced (R§11), a window that merely starts (none → some) is not.
    let previous = memory.instance.insert(key.clone(), eff.resets_at);
    let new_instance = match previous {
        Some(Some(prev)) => !same_window_instance(Some(prev), eff.resets_at),
        Some(None) => eff.resets_at.is_some(),
        None => false,
    };
    if new_instance {
        memory.severity.remove(&key);
        if matches!(previous, Some(Some(_))) {
            changes.push(LiveChange::WindowReset(key.clone()));
        }
    }

    let outcome = if cx.stale.is_none() {
        let input = WindowSeverityInput {
            pct,
            exhausted,
            valid: forecast.is_some_and(|f| f.valid),
            runs_out: forecast.is_some_and(|f| f.runs_out),
            projected: forecast.map(|f| f.projected),
            runway_min,
        };
        let previous = memory.severity.get(&key).copied().unwrap_or_default();
        let outcome = window_severity(&input, previous);
        record_severity(&key, &outcome, memory, changes);
        Some(outcome)
    } else {
        None
    };

    let state = match (cx.stale, eff.view, outcome) {
        (Some(reason), _, _) => RowState::Stale(reason),
        (None, ResetView::JustReset { .. }, _) => RowState::JustReset,
        (None, ResetView::IdleAfterReset { .. }, _) => RowState::Idle,
        (None, ResetView::Live, Some(o)) => match (o.severity, o.cause) {
            (Severity::Exhausted, _) => RowState::Exhausted,
            (Severity::Critical, Some(CriticalCause::Runway)) => RowState::CriticalRunway,
            (Severity::Critical, _) => RowState::CriticalLevel,
            (Severity::Warning, _) => RowState::Warning,
            (Severity::Ok, _) if eff.resets_at.is_some() => RowState::Ok,
            (Severity::Ok, _) if used <= 0.0 && meter.starts_on_use => RowState::Idle,
            (Severity::Ok, _) => RowState::OkNoReset,
        },
        (None, ResetView::Live, None) => RowState::OkNoReset,
    };
    let class = match (cx.stale, outcome) {
        (Some(_), _) => DisplayClass::Stale,
        (None, Some(o)) => class_of(o.severity),
        (None, None) => DisplayClass::Ok,
    };
    let pressure = if state == RowState::Idle {
        0.0
    } else {
        let p = forecast.filter(|f| f.valid).map_or(used, |f| f.projected);
        used.max(p)
    };

    Assessment {
        key,
        names: meter.names.clone(),
        service_order: cx.service_order,
        meter_order: meter.order,
        meter: AssessedMeter::Window(WindowAssessment {
            used,
            pct,
            exhausted,
            resets_at: eff.resets_at,
            window: eff.window,
            starts_on_use: meter.starts_on_use,
            elapsed,
            pace,
            reset: eff.view,
            forecast,
            runway_min,
            cause: outcome.and_then(|o| o.cause),
            reading_at: cx.reading_at,
            exhausted_since,
        }),
        severity: outcome.map(|o| o.severity),
        state,
        class,
        pressure,
    }
}

/// R§3.6–R§3.7 and R§4.1 for one balance meter.
fn assess_balance(
    balance: &Balance,
    key: MeterKey,
    cx: &MeterContext<'_>,
    memory: &mut EvalMemory,
    changes: &mut Vec<LiveChange>,
) -> Assessment {
    // Stale balances are frozen at the reading (R§3.8).
    let at = if cx.stale.is_some() {
        cx.reading_at
    } else {
        cx.now
    };
    let samples: Vec<Sample> = cx
        .history
        .samples(&key, sub(at, BAL_REF_LOOKBACK))
        .iter()
        .filter(|s| s.t <= at)
        .copied()
        .collect();
    let stats = balance_stats(balance.amount, balance.last_top_up, &samples, at);
    let pct = display_pct(stats.used_share, stats.exhausted);

    let outcome = if cx.stale.is_none() {
        // A top-up re-evaluates at once (R§3.7): no hysteresis carried over it.
        let topped_up = match samples.as_slice() {
            [.., before, last] if last.value.to_bits() == balance.amount.to_bits() => {
                is_top_up(before.value, last.value)
            }
            [.., last] => is_top_up(last.value, balance.amount),
            [] => false,
        };
        let previous = if topped_up {
            SeverityMemory::default()
        } else {
            memory.severity.get(&key).copied().unwrap_or_default()
        };
        let input = BalanceSeverityInput {
            exhausted: stats.exhausted,
            days_left: stats.days_left,
        };
        let outcome = balance_severity(&input, previous, cx.alert);
        record_severity(&key, &outcome, memory, changes);
        Some(outcome)
    } else {
        None
    };
    let state = match (cx.stale, outcome.map(|o| o.severity)) {
        (Some(reason), _) => RowState::Stale(reason),
        (None, Some(Severity::Exhausted)) => RowState::Exhausted,
        (None, Some(Severity::Critical)) => RowState::CriticalRunway,
        (None, Some(Severity::Warning)) => RowState::Warning,
        (None, _) => RowState::OkBalance,
    };
    let class = match (cx.stale, outcome) {
        (Some(_), _) => DisplayClass::Stale,
        (None, Some(o)) => class_of(o.severity),
        (None, None) => DisplayClass::Ok,
    };

    Assessment {
        key,
        names: balance.names.clone(),
        service_order: cx.service_order,
        meter_order: balance.order,
        meter: AssessedMeter::Balance(BalanceAssessment {
            amount: balance.amount,
            unit: balance.unit.clone(),
            pct,
            reading_at: cx.reading_at,
            stats,
        }),
        severity: outcome.map(|o| o.severity),
        state,
        class,
        pressure: stats.used_share,
    }
}
