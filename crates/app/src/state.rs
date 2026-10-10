//! The main thread's state (ARCHITECTURE "Threads": the main thread owns the app state). Held as
//! `Rc<RefCell<AppState>>` by the event handlers in [`crate::wiring`]; borrows are short and never
//! span a Win32 or Slint call that can re-enter.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use jiff::Timestamp;
use lastdrop_core::grammar::{English, LocaleFormat, TextContext};
use lastdrop_core::model::{AccountRef, MeterKey, Snapshot, SourceKey};
use lastdrop_core::readout::ReadoutForm;
use lastdrop_core::refresh::{CredentialFingerprint, SourceState};
use lastdrop_core::settings::Settings;
use lastdrop_core::stats::{EvalMemory, Evaluation, ServiceStatus};
use lastdrop_providers::{AccountSource, Provider};
use lastdrop_win::flyout_pos::MonitorInfo;
use lastdrop_win::geom::Rect;
use lastdrop_win::placement::TaskbarEdge;
use lastdrop_win::theme::ThemeInfo;

use crate::history::HistoryStore;
use crate::scheduler::JobResult;

/// Everything known about one credential source.
pub struct SourceRuntime {
    /// The account as shown (alias, plan from the latest snapshot).
    pub account: AccountRef,
    /// Where its data is.
    pub source: AccountSource,
    /// Its provider.
    pub provider: Arc<dyn Provider>,
    /// The latest successful reading.
    pub latest: Option<Snapshot>,
    /// Freshness inputs (R§3.8).
    pub status: ServiceStatus,
    /// Scheduling memory (`lastdrop_core::refresh`).
    pub refresh: SourceState,
    /// The credential fingerprint a worker last reported (`JobResult::credential`), the
    /// `PolicyInput::credential` of the next plan.
    pub credential: Option<CredentialFingerprint>,
}

/// The flyout's show/hide bookkeeping (F§1.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlyoutState {
    /// Shown.
    pub open: bool,
    /// When it last hid because it lost activation (300 ms toggle debounce).
    pub last_auto_hide: Option<Instant>,
    /// Opened by keyboard (`NIN_KEYSELECT`): focus the first card.
    pub opened_by_keyboard: bool,
}

/// The readout as the readout thread last reported it (`ReadoutEvent::PlacementChanged`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadoutState {
    /// The form drawn.
    pub form: ReadoutForm,
    /// The rows it shows (the compact form shows one), for the hover card (R§8.2).
    pub rows: Vec<MeterKey>,
    /// The taskbar's monitor, work area and DPI (tray icon size R§9.1; hover card and flyout
    /// placement). `None` until the first report.
    pub monitor: Option<MonitorInfo>,
    /// The taskbar's rectangle.
    pub taskbar: Rect,
    /// The taskbar's edge.
    pub edge: TaskbarEdge,
}

/// The app state, main thread only.
pub struct AppState {
    /// Settings as loaded or last applied.
    pub settings: Settings,
    /// One entry per enabled credential source. Keyed (and so iterated) by [`SourceKey`], which is
    /// **not** the user's service order: build `EvalInput::services` and plans by walking
    /// `settings.enabled_providers()` (F§2.2, R§3.10 tie break).
    pub sources: BTreeMap<SourceKey, SourceRuntime>,
    /// Hysteresis between evaluations.
    pub memory: EvalMemory,
    /// The latest evaluation.
    pub evaluation: Option<Evaluation>,
    /// History of every meter.
    pub history: HistoryStore,
    /// Theme.
    pub theme: ThemeInfo,
    /// Regional format and time zone.
    pub locale: LocaleFormat,
    /// Strings (English in M1).
    pub strings: English,
    /// Flyout bookkeeping.
    pub flyout: FlyoutState,
    /// Readout as reported.
    pub readout: ReadoutState,
    /// A manual refresh is running (the flyout's refresh button shows progress, F§3).
    pub refreshing: bool,
}

impl AppState {
    /// Builds the sources from settings (enabled providers × accounts, resolved with the real
    /// environment).
    pub fn new(
        settings: Settings,
        history: HistoryStore,
        theme: ThemeInfo,
        locale: LocaleFormat,
    ) -> AppState {
        todo!("M1: initial state")
    }

    /// Applies new settings: adds and removes sources, keeps readings of sources that remain.
    pub fn apply_settings(&mut self, settings: Settings) {
        todo!("M1: apply settings")
    }

    /// Records a worker's result: snapshot → `latest` and history; status and refresh memory via
    /// `lastdrop_core::refresh::record`.
    ///
    /// - Success: `status.last_success` = the snapshot's `measured_at` (`t_r`, never
    ///   `fetched_at`), `last_failure` cleared; `Health::SignInRequired` sets
    ///   `status.sign_in_required` (cleared by any other health); the snapshot's plan updates
    ///   `account.plan`.
    /// - Failure: `last_failure` = (`finished`, `FetchError::kind()`, or `FailureKind::Offline`
    ///   when `result.internet == Some(false)`), `failure_detail` = `FetchError::detail()`.
    /// - Probe that found the credential unchanged (`outcome: None`): only `credential` and
    ///   `in_flight` change; no attempt is recorded.
    /// - Always: `credential` = `result.credential`; `refresh` via
    ///   `lastdrop_core::refresh::record` (not for an unchanged probe), `in_flight` cleared,
    ///   `status.next_try` from the next plan.
    pub fn apply_result(&mut self, result: JobResult) {
        todo!("M1: apply a fetch result")
    }

    /// Re-evaluates at `now` (`lastdrop_core::stats::evaluate`) and stores the result.
    pub fn evaluate(&mut self, now: Timestamp) -> &Evaluation {
        todo!("M1: evaluate")
    }

    /// The text context at `now` for the surface builders.
    pub fn text_context(&self, now: Timestamp) -> TextContext<'_> {
        TextContext {
            now,
            time_style: self.settings.readout.time_style,
            locale: &self.locale,
            strings: &self.strings,
        }
    }
}
