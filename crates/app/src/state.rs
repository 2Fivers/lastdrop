//! The main thread's state (ARCHITECTURE "Threads": the main thread owns the app state). Held as
//! `Rc<RefCell<AppState>>` by the event handlers in [`crate::wiring`]; borrows are short and never
//! span a Win32 or Slint call that can re-enter.
//!
//! The logic is in free functions over the pieces it needs ([`build_sources`],
//! [`apply_to_source`], [`evaluate_sources`]) so it is testable without a history store on disk;
//! the [`AppState`] methods put the pieces together.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use jiff::Timestamp;
use lastdrop_core::grammar::{English, LocaleFormat, TextContext};
use lastdrop_core::model::{AccountRef, Health, MeterKey, ProviderId, Snapshot, SourceKey};
use lastdrop_core::readout::ReadoutForm;
use lastdrop_core::refresh::{Activity, AttemptResult, CredentialFingerprint, SourceState, record};
use lastdrop_core::settings::{AccountSettings, Settings};
use lastdrop_core::stats::{
    EvalInput, EvalMemory, Evaluation, FailureKind, HistoryView, ServiceInput, ServiceStatus,
    evaluate,
};
use lastdrop_providers::real::RealEnv;
use lastdrop_providers::{AccountSource, FetchError, Provider};
use lastdrop_win::flyout_pos::MonitorInfo;
use lastdrop_win::geom::Rect;
use lastdrop_win::placement::TaskbarEdge;
use lastdrop_win::theme::ThemeInfo;

use crate::history::{HistoryStore, IN_MEMORY_SPAN};
use crate::persist::{self, SavedState, SeenMap};
use crate::scheduler::{JobResult, poll_interval, reading_pace};

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

impl Default for ReadoutState {
    /// Before the first report: hidden, no rows, bottom taskbar.
    fn default() -> ReadoutState {
        ReadoutState {
            form: ReadoutForm::Hidden,
            rows: Vec::new(),
            monitor: None,
            taskbar: Rect::default(),
            edge: TaskbarEdge::Bottom,
        }
    }
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
    /// The provider instances in use. Kept across settings changes that leave a provider's
    /// options alone ([`reuse_providers`]): their memory (the Claude OAuth source's 401 stop,
    /// 5-minute rest and `Retry-After` back-off) must not reset on every Apply.
    pub providers: Vec<Arc<dyn Provider>>,
    /// `memory.seen` as the state file has it ([`AppState::state_to_save`]).
    pub saved_seen: SeenMap,
}

/// The providers for `after`: the existing instance of a provider whose options did not change
/// between `before` and `after`, else the new one from `fresh`.
pub fn reuse_providers(
    existing: &[Arc<dyn Provider>],
    fresh: Vec<Arc<dyn Provider>>,
    before: &Settings,
    after: &Settings,
) -> Vec<Arc<dyn Provider>> {
    fresh
        .into_iter()
        .map(|p| {
            let unchanged = match p.id() {
                ProviderId::Claude => {
                    before.provider_options.claude == after.provider_options.claude
                }
                ProviderId::Codex => before.provider_options.codex == after.provider_options.codex,
                ProviderId::Meshy => true,
            };
            match existing.iter().find(|e| e.id() == p.id()) {
                Some(kept) if unchanged => kept.clone(),
                _ => p,
            }
        })
        .collect()
}

/// Resolves an account's settings into an [`AccountSource`] (production:
/// `AccountSource::resolve` with the real environment).
pub type Resolver<'a> =
    &'a dyn Fn(ProviderId, &AccountSettings) -> Result<AccountSource, FetchError>;

/// The real resolver: `AccountSource::resolve` with the process environment.
pub fn resolve_real(
    provider: ProviderId,
    account: &AccountSettings,
) -> Result<AccountSource, FetchError> {
    AccountSource::resolve(provider, account, &RealEnv)
}

/// The sources of `settings`: enabled providers × their accounts, resolved with `resolve`. A
/// source that cannot be resolved (no home folder) is left out and logged. `existing` sources
/// with the same key keep their readings and scheduling memory (their provider and account names
/// are updated).
pub fn build_sources(
    settings: &Settings,
    providers: &[Arc<dyn Provider>],
    resolve: Resolver<'_>,
    mut existing: BTreeMap<SourceKey, SourceRuntime>,
) -> BTreeMap<SourceKey, SourceRuntime> {
    let mut sources = BTreeMap::new();
    for (_, p) in settings.enabled_providers() {
        let Some(provider) = providers.iter().find(|x| x.id() == p.id) else {
            continue;
        };
        for account in &p.accounts {
            let key = SourceKey {
                provider: p.id,
                account: account.key.clone(),
            };
            let resolved = match resolve(p.id, account) {
                Ok(source) => source,
                Err(e) => {
                    log::warn!("{:?}/{}: {e}", p.id, account.key.0);
                    continue;
                }
            };
            let poll = poll_interval(settings, provider.as_ref(), Activity::default());
            let pace = reading_pace(provider.as_ref());
            let runtime = match existing.remove(&key) {
                Some(mut kept) => {
                    kept.provider = provider.clone();
                    kept.source = resolved;
                    kept.account.alias.clone_from(&account.alias);
                    kept.account.tag.clone_from(&account.tag);
                    kept.status.poll_interval = poll;
                    kept.status.reading_pace = pace;
                    kept
                }
                None => SourceRuntime {
                    account: AccountRef {
                        key: account.key.clone(),
                        alias: account.alias.clone(),
                        tag: account.tag.clone(),
                        plan: None,
                    },
                    source: resolved,
                    provider: provider.clone(),
                    latest: None,
                    status: ServiceStatus {
                        poll_interval: poll,
                        reading_pace: pace,
                        ..ServiceStatus::default()
                    },
                    refresh: SourceState::default(),
                    credential: None,
                },
            };
            sources.insert(key, runtime);
        }
    }
    sources
}

/// Applies a worker's result to its source (see [`AppState::apply_result`]). Returns the
/// snapshot to record in history when a newer reading arrived.
pub fn apply_to_source(source: &mut SourceRuntime, result: JobResult) -> Option<Snapshot> {
    let min_interval = source.provider.min_interval();
    source.credential.clone_from(&result.credential);
    let Some(outcome) = result.outcome else {
        // A probe found the credential unchanged: nothing was attempted.
        source.refresh.in_flight = false;
        return None;
    };
    match outcome {
        Ok(snapshot) => {
            record(
                &mut source.refresh,
                result.started,
                result.finished,
                &AttemptResult::Success,
                min_interval,
            );
            let status = &mut source.status;
            status.last_failure = None;
            status.failure_detail = None;
            status.sign_in_required = snapshot.health == Health::SignInRequired;
            if snapshot.account.plan.is_some() {
                source.account.plan.clone_from(&snapshot.account.plan);
            }
            // A fallback source can answer with an older reading than one already shown (a
            // cached usage after a live one): keep the newer, never go back in time.
            let newer = source
                .latest
                .as_ref()
                .is_none_or(|l| snapshot.measured_at >= l.measured_at);
            if !newer {
                return None;
            }
            status.last_success = Some(
                status
                    .last_success
                    .map_or(snapshot.measured_at, |t| t.max(snapshot.measured_at)),
            );
            source.latest = Some(snapshot.clone());
            Some(snapshot)
        }
        Err(error) => {
            let kind = if result.internet == Some(false) {
                FailureKind::Offline
            } else {
                error.kind()
            };
            record(
                &mut source.refresh,
                result.started,
                result.finished,
                &error.attempt_result(result.credential.clone()),
                min_interval,
            );
            source.status.last_failure = Some((result.finished, kind));
            source.status.failure_detail = Some(error.detail());
            None
        }
    }
}

/// Evaluates every source of `settings`' enabled services in service order at `now`
/// (`lastdrop_core::stats::evaluate`), with `paused` from the settings.
pub fn evaluate_sources(
    settings: &Settings,
    sources: &BTreeMap<SourceKey, SourceRuntime>,
    history: &dyn HistoryView,
    memory: &mut EvalMemory,
    now: Timestamp,
) -> Evaluation {
    let paused = settings
        .pause
        .is_some_and(|p| p.until.is_none_or(|until| now < until));
    let ordered: Vec<(&SourceRuntime, ServiceStatus)> = settings
        .enabled_providers()
        .flat_map(|(_, p)| {
            p.accounts.iter().filter_map(move |a| {
                sources.get(&SourceKey {
                    provider: p.id,
                    account: a.key.clone(),
                })
            })
        })
        .map(|s| {
            let mut status = s.status.clone();
            status.paused = paused;
            (s, status)
        })
        .collect();
    let services: Vec<ServiceInput<'_>> = ordered
        .iter()
        .map(|(s, status)| ServiceInput {
            provider: s.provider.id(),
            account: &s.account,
            snapshot: s.latest.as_ref(),
            status,
            fetching: s.refresh.in_flight,
        })
        .collect();
    evaluate(
        &EvalInput {
            now,
            settings,
            services: &services,
            history,
        },
        memory,
    )
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
        let providers = lastdrop_providers::providers(&settings);
        let sources = build_sources(&settings, &providers, &resolve_real, BTreeMap::new());
        AppState {
            providers,
            settings,
            sources,
            memory: EvalMemory::default(),
            evaluation: None,
            history,
            theme,
            locale,
            strings: English,
            flyout: FlyoutState::default(),
            readout: ReadoutState::default(),
            refreshing: false,
            saved_seen: SeenMap::new(),
        }
    }

    /// Applies new settings: adds and removes sources, keeps readings of sources that remain.
    pub fn apply_settings(&mut self, settings: Settings) {
        let providers = reuse_providers(
            &self.providers,
            lastdrop_providers::providers(&settings),
            &self.settings,
            &settings,
        );
        let existing = std::mem::take(&mut self.sources);
        self.sources = build_sources(&settings, &providers, &resolve_real, existing);
        self.settings = settings;
        self.providers = providers;
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
        let Some(source) = self.sources.get_mut(&result.source) else {
            // The service was turned off while its fetch ran.
            return;
        };
        if let Some(snapshot) = apply_to_source(source, result) {
            self.history.record(&snapshot);
        }
    }

    /// Restores the meters earlier runs saw (the state file; `EvalMemory::seen`, so a meter a
    /// later reading lacks keeps its room in the readout, R§6.3), keeping only those the history
    /// has a reading of from the last [`IN_MEMORY_SPAN`] (31 days): a meter that no source has
    /// reported for a month stops holding room. The first evaluation rewrites the file when
    /// entries were dropped ([`AppState::state_to_save`]).
    pub fn restore_seen(&mut self, saved: &SavedState, now: Timestamp) {
        let since = now.checked_sub(IN_MEMORY_SPAN).unwrap_or(Timestamp::MIN);
        let all = saved.seen_map();
        self.memory.seen = all
            .iter()
            .filter(|(key, _)| !self.history.samples(key, since).is_empty())
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect();
        self.saved_seen = all;
    }

    /// The state file's new bytes when `EvalMemory::seen` changed since the last save (or the
    /// restore); `None` otherwise. The caller queues them for the writer thread.
    pub fn state_to_save(&mut self) -> Option<Vec<u8>> {
        if self.memory.seen == self.saved_seen {
            return None;
        }
        self.saved_seen = self.memory.seen.clone();
        match persist::state_bytes(&SavedState::from_seen(&self.memory.seen)) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                log::warn!("cannot encode the state file: {e}");
                None
            }
        }
    }

    /// Re-evaluates at `now` (`lastdrop_core::stats::evaluate`) and stores the result.
    pub fn evaluate(&mut self, now: Timestamp) -> &Evaluation {
        let eval = evaluate_sources(
            &self.settings,
            &self.sources,
            &self.history,
            &mut self.memory,
            now,
        );
        self.evaluation.insert(eval)
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    use lastdrop_core::model::{AccountKey, Meter, MeterId, ProviderId, SourceKind, WindowLen};
    use lastdrop_core::settings::{ProviderEntry, ProviderSettings};
    use lastdrop_core::stats::{Freshness, RowState, Sample, StaleReason};
    use lastdrop_providers::FetchContext;
    use lastdrop_providers::io::CredentialName;

    /// An invented provider: answers with a fixed snapshot or a fixed error.
    pub(crate) struct FakeProvider {
        id: ProviderId,
        fail: bool,
    }

    impl FakeProvider {
        pub(crate) fn ok(id: ProviderId) -> Arc<dyn Provider> {
            Arc::new(FakeProvider { id, fail: false })
        }

        pub(crate) fn failing(id: ProviderId) -> Arc<dyn Provider> {
            Arc::new(FakeProvider { id, fail: true })
        }
    }

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            self.id
        }

        fn fetch(
            &self,
            ctx: &FetchContext<'_>,
            _account: &AccountSource,
        ) -> Result<Snapshot, FetchError> {
            if self.fail {
                return Err(FetchError::Transient {
                    from: SourceKind::ClaudeOAuthUsage,
                    detail: "HTTP 503".to_owned(),
                    retry_after: None,
                });
            }
            Ok(snapshot(self.id, ctx.clock.now(), 0.42))
        }

        fn min_interval(&self) -> Duration {
            Duration::from_secs(60)
        }

        fn credential_fingerprint(
            &self,
            _ctx: &FetchContext<'_>,
            _account: &AccountSource,
        ) -> Option<CredentialFingerprint> {
            Some(CredentialFingerprint("v1".to_owned()))
        }
    }

    pub(crate) fn snapshot(provider: ProviderId, at: Timestamp, used: f64) -> Snapshot {
        Snapshot {
            provider,
            account: AccountRef {
                plan: Some("Max".to_owned()),
                ..AccountRef::primary()
            },
            source: SourceKind::ClaudeOAuthUsage,
            measured_at: at,
            fetched_at: at,
            meters: vec![Meter {
                id: MeterId::new(MeterId::WEEKLY),
                names: lastdrop_core::model::MeterNames::new("Wk", "weekly", "Weekly"),
                order: 1,
                used,
                limited: false,
                resets_at: at.checked_add(jiff::SignedDuration::from_hours(48)).ok(),
                window: WindowLen::Fixed(jiff::SignedDuration::from_hours(7 * 24)),
                starts_on_use: false,
            }],
            balances: Vec::new(),
            health: Health::Ok,
        }
    }

    /// An invented account source of `provider` (paths under a fake home).
    pub(crate) fn source_of(provider: ProviderId) -> AccountSource {
        let home = PathBuf::from(r"C:\Users\tester");
        match provider {
            ProviderId::Claude => AccountSource::Claude {
                account: AccountKey::primary(),
                config_dir: home.join(".claude"),
                state_file: home.join(".claude.json"),
                desktop_dir: None,
            },
            ProviderId::Codex => AccountSource::Codex {
                account: AccountKey::primary(),
                codex_home: home.join(".codex"),
            },
            ProviderId::Meshy => AccountSource::Meshy {
                account: AccountKey::primary(),
                credential: CredentialName {
                    provider: ProviderId::Meshy,
                    account: AccountKey::primary(),
                    name: "api_key".to_owned(),
                },
            },
        }
    }

    /// Settings with `providers` enabled, in that order.
    pub(crate) fn settings_of(providers: &[ProviderId]) -> Settings {
        Settings {
            providers: providers
                .iter()
                .map(|p| ProviderEntry::Known(ProviderSettings::new(*p, true)))
                .collect(),
            ..Settings::default()
        }
    }

    /// Sources of `providers` with fake providers, built directly (no environment lookup).
    pub(crate) fn sources_of(
        settings: &Settings,
        providers: &[ProviderId],
    ) -> BTreeMap<SourceKey, SourceRuntime> {
        providers
            .iter()
            .map(|p| {
                let provider = FakeProvider::ok(*p);
                (
                    SourceKey {
                        provider: *p,
                        account: AccountKey::primary(),
                    },
                    SourceRuntime {
                        account: AccountRef::primary(),
                        source: source_of(*p),
                        status: ServiceStatus {
                            poll_interval: poll_interval(
                                settings,
                                provider.as_ref(),
                                Activity::default(),
                            ),
                            ..ServiceStatus::default()
                        },
                        provider,
                        latest: None,
                        refresh: SourceState::default(),
                        credential: None,
                    },
                )
            })
            .collect()
    }

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("valid timestamp")
    }

    /// An app state over `history` with fake sources of `providers` (no environment lookup).
    fn app_state(providers: &[ProviderId], history: HistoryStore) -> AppState {
        use lastdrop_win::palette::Argb;
        use lastdrop_win::theme::AccentShades;
        let settings = settings_of(providers);
        let accent = Argb(0xFF8E_3AA7);
        AppState {
            sources: sources_of(&settings, providers),
            settings,
            memory: EvalMemory::default(),
            evaluation: None,
            history,
            theme: ThemeInfo {
                taskbar_dark: true,
                apps_dark: true,
                accent: AccentShades {
                    accent,
                    dark1: accent,
                    dark2: accent,
                    light2: accent,
                    light3: accent,
                },
                transparency: false,
                high_contrast: None,
            },
            locale: LocaleFormat::en_gb(jiff::tz::TimeZone::UTC),
            strings: English,
            flyout: FlyoutState::default(),
            readout: ReadoutState::default(),
            refreshing: false,
            providers: Vec::new(),
            saved_seen: SeenMap::new(),
        }
    }

    #[test]
    fn seen_meters_survive_a_restart_while_their_history_is_recent() {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-state-test-{}-{}",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        let state_file = dir.join("state.json");
        let writer = crate::writer::Writer::start().expect("writer");
        let now = ts("2026-10-10T10:42:00Z");
        let claude = SourceKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
        };
        let key = |meter: &str| MeterKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
            meter: MeterId::new(meter),
        };
        let earlier = now - jiff::SignedDuration::from_hours(1);
        let mut with_fable = snapshot(ProviderId::Claude, earlier, 0.3);
        let mut fable = with_fable.meters[0].clone();
        fable.id = MeterId::new("weekly:fable");
        fable.names =
            lastdrop_core::model::MeterNames::new("Fable", "Fable weekly", "Weekly · Fable");
        fable.order = 2;
        with_fable.meters.push(fable);

        // First run: the OAuth source reports the per-model weekly; it is recorded and saved.
        let history = HistoryStore::open(&dir.join("history"), now, writer.handle()).expect("open");
        let mut first = app_state(&[ProviderId::Claude], history);
        first.history.record(&with_fable);
        if let Some(source) = first.sources.get_mut(&claude) {
            source.latest = Some(with_fable.clone());
        }
        first.evaluate(now);
        assert!(first.memory.seen.contains_key(&key("weekly:fable")));
        let bytes = first.state_to_save().expect("seen changed");
        assert_eq!(first.state_to_save(), None, "saved once");
        // A meter an older run saw and that has no history any more rides along in the file.
        let mut saved: SavedState = serde_json::from_slice(&bytes).expect("state json");
        saved.seen.push(persist::SeenMeter {
            key: key("weekly:retired"),
            names: lastdrop_core::model::MeterNames::new("Old", "old weekly", "Weekly · Old"),
            balance: false,
        });
        writer.handle().replace(
            state_file.clone(),
            persist::state_bytes(&saved).expect("encode"),
            "state",
        );
        writer.handle().flush();

        // Restart: a reading without the per-model weekly (Claude Desktop's history answers).
        let history = HistoryStore::open(&dir.join("history"), now, writer.handle()).expect("open");
        let mut second = app_state(&[ProviderId::Claude], history);
        second.restore_seen(&persist::load_state(&state_file), now);
        assert!(second.memory.seen.contains_key(&key("weekly:fable")));
        assert!(
            !second.memory.seen.contains_key(&key("weekly:retired")),
            "no reading for a month: dropped"
        );
        if let Some(source) = second.sources.get_mut(&claude) {
            source.latest = Some(snapshot(ProviderId::Claude, now, 0.31));
        }
        let eval = second.evaluate(now);
        let absent: Vec<&MeterKey> = eval.services[0].absent.iter().map(|a| &a.key).collect();
        assert_eq!(
            absent,
            vec![&key("weekly:fable")],
            "the width keeps its room"
        );
        // The dropped entry is written out once.
        let pruned = second.state_to_save().expect("pruned");
        let pruned: SavedState = serde_json::from_slice(&pruned).expect("state json");
        let names: Vec<&str> = pruned.seen.iter().map(|m| m.key.meter.0.as_str()).collect();
        assert_eq!(names, vec!["weekly", "weekly:fable"]);
        assert_eq!(second.state_to_save(), None);
        writer.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn result(source: &SourceKey, outcome: Option<Result<Snapshot, FetchError>>) -> JobResult {
        let t = ts("2026-10-10T10:42:00Z");
        JobResult {
            source: source.clone(),
            started: t,
            finished: t,
            outcome,
            credential: Some(CredentialFingerprint("v1".to_owned())),
            internet: None,
        }
    }

    #[test]
    fn providers_survive_an_apply_that_leaves_their_options_alone() {
        let before = settings_of(&[ProviderId::Claude, ProviderId::Codex]);
        let existing = lastdrop_providers::providers(&before);
        let mut after = before.clone();
        after.provider_options.codex.rollout_fallback =
            !after.provider_options.codex.rollout_fallback;
        let kept = reuse_providers(
            &existing,
            lastdrop_providers::providers(&after),
            &before,
            &after,
        );
        let find = |list: &[Arc<dyn Provider>], id: ProviderId| {
            list.iter()
                .find(|p| p.id() == id)
                .cloned()
                .expect("provider")
        };
        // Claude's memory (401 stop, back-off) is kept; Codex got its new options.
        assert!(Arc::ptr_eq(
            &find(&existing, ProviderId::Claude),
            &find(&kept, ProviderId::Claude)
        ));
        assert!(Arc::ptr_eq(
            &find(&existing, ProviderId::Meshy),
            &find(&kept, ProviderId::Meshy)
        ));
        assert!(!Arc::ptr_eq(
            &find(&existing, ProviderId::Codex),
            &find(&kept, ProviderId::Codex)
        ));
    }

    #[test]
    fn a_success_updates_reading_status_and_plan() {
        let settings = settings_of(&[ProviderId::Claude]);
        let mut sources = sources_of(&settings, &[ProviderId::Claude]);
        let key = sources.keys().next().cloned().expect("one source");
        let source = sources.get_mut(&key).expect("source");
        source.refresh.in_flight = true;
        // A cached reading measured earlier than it was fetched: t_r is measured_at.
        let mut snap = snapshot(ProviderId::Claude, ts("2026-10-10T10:00:00Z"), 0.42);
        snap.fetched_at = ts("2026-10-10T10:42:00Z");
        let recorded = apply_to_source(source, result(&key, Some(Ok(snap))));
        assert!(recorded.is_some());
        assert_eq!(source.status.last_success, Some(ts("2026-10-10T10:00:00Z")));
        assert_eq!(source.account.plan.as_deref(), Some("Max"));
        assert!(!source.refresh.in_flight);
        assert_eq!(
            source.credential,
            Some(CredentialFingerprint("v1".to_owned()))
        );

        // An older reading does not replace a newer one.
        let older = snapshot(ProviderId::Claude, ts("2026-10-10T09:00:00Z"), 0.30);
        assert!(apply_to_source(source, result(&key, Some(Ok(older)))).is_none());
        assert_eq!(source.latest.as_ref().map(|s| s.meters[0].used), Some(0.42));
    }

    #[test]
    fn failures_classify_and_probes_record_nothing() {
        let settings = settings_of(&[ProviderId::Claude]);
        let mut sources = sources_of(&settings, &[ProviderId::Claude]);
        let key = sources.keys().next().cloned().expect("one source");
        let source = sources.get_mut(&key).expect("source");
        let error = FetchError::Transient {
            from: SourceKind::ClaudeOAuthUsage,
            detail: "HTTP 503".to_owned(),
            retry_after: None,
        };
        let mut r = result(&key, Some(Err(error.clone())));
        r.internet = Some(false);
        apply_to_source(source, r);
        assert_eq!(
            source.status.last_failure.map(|(_, k)| k),
            Some(FailureKind::Offline)
        );
        assert_eq!(source.status.failure_detail.as_deref(), Some("HTTP 503"));
        assert_eq!(source.refresh.consecutive_failures, 1);
        assert!(source.refresh.backoff_until.is_some());

        apply_to_source(source, result(&key, Some(Err(error))));
        assert_eq!(
            source.status.last_failure.map(|(_, k)| k),
            Some(FailureKind::NoData)
        );

        let attempts = source.refresh.last_attempt;
        source.refresh.in_flight = true;
        apply_to_source(source, result(&key, None));
        assert!(!source.refresh.in_flight);
        assert_eq!(source.refresh.last_attempt, attempts, "no attempt recorded");
    }

    #[test]
    fn auth_failures_stop_until_the_credential_changes() {
        let settings = settings_of(&[ProviderId::Codex]);
        let mut sources = sources_of(&settings, &[ProviderId::Codex]);
        let key = sources.keys().next().cloned().expect("one source");
        let source = sources.get_mut(&key).expect("source");
        let auth = FetchError::Auth {
            from: SourceKind::CodexAppServer,
        };
        apply_to_source(source, result(&key, Some(Err(auth))));
        assert_eq!(
            source.status.last_failure.map(|(_, k)| k),
            Some(FailureKind::Auth)
        );
        assert_eq!(
            source.refresh.auth_stopped,
            Some(CredentialFingerprint("v1".to_owned()))
        );
    }

    #[test]
    fn evaluation_walks_the_service_order_and_applies_the_pause() {
        let mut settings = settings_of(&[ProviderId::Codex, ProviderId::Claude]);
        let mut sources = sources_of(&settings, &[ProviderId::Claude, ProviderId::Codex]);
        let now = ts("2026-10-10T10:42:00Z");
        for (key, source) in &mut sources {
            source.latest = Some(snapshot(key.provider, now, 0.2));
            source.status.last_success = Some(now);
        }
        let history: BTreeMap<MeterKey, Vec<Sample>> = BTreeMap::new();
        let mut memory = EvalMemory::default();
        let eval = evaluate_sources(&settings, &sources, &history, &mut memory, now);
        let order: Vec<ProviderId> = eval.services.iter().map(|s| s.source.provider).collect();
        assert_eq!(order, [ProviderId::Codex, ProviderId::Claude]);
        assert_eq!(eval.services[0].freshness, Freshness::Fresh);

        settings.pause = Some(lastdrop_core::settings::Pause { until: None });
        let eval = evaluate_sources(&settings, &sources, &history, &mut memory, now);
        assert_eq!(
            eval.services[0].freshness,
            Freshness::Stale(StaleReason::Paused)
        );
        assert!(matches!(
            eval.services[0].meters[0].state,
            RowState::Stale(StaleReason::Paused)
        ));
    }

    #[test]
    fn rebuilding_sources_keeps_readings_and_drops_disabled_services() {
        let settings = settings_of(&[ProviderId::Claude, ProviderId::Codex]);
        let providers: Vec<Arc<dyn Provider>> = vec![
            FakeProvider::ok(ProviderId::Claude),
            FakeProvider::ok(ProviderId::Codex),
        ];
        let mut existing = sources_of(&settings, &[ProviderId::Claude, ProviderId::Codex]);
        let now = ts("2026-10-10T10:42:00Z");
        for (key, s) in &mut existing {
            s.latest = Some(snapshot(key.provider, now, 0.5));
        }
        let only_claude = settings_of(&[ProviderId::Claude]);
        let resolve = |p: ProviderId, _: &AccountSettings| Ok(source_of(p));
        let sources = build_sources(&only_claude, &providers, &resolve, existing);
        assert_eq!(sources.len(), 1);
        let claude = sources.values().next().expect("Claude");
        assert_eq!(claude.provider.id(), ProviderId::Claude);
        assert!(claude.latest.is_some(), "the reading survives");

        // A source that cannot be resolved is left out.
        let unresolved = |_: ProviderId, _: &AccountSettings| {
            Err(FetchError::Unavailable {
                detail: "no home folder".to_owned(),
            })
        };
        assert!(build_sources(&settings, &providers, &unresolved, BTreeMap::new()).is_empty());
    }

    #[test]
    fn a_source_that_paces_its_live_reading_tells_the_freshness_rule() {
        let settings = settings_of(&[ProviderId::Claude, ProviderId::Codex]);
        let providers = lastdrop_providers::providers(&settings);
        let resolve = |p: ProviderId, _: &AccountSettings| Ok(source_of(p));
        let sources = build_sources(&settings, &providers, &resolve, BTreeMap::new());
        let pace = |p: ProviderId| {
            sources
                .iter()
                .find(|(k, _)| k.provider == p)
                .map(|(_, s)| s.status.reading_pace)
                .expect("a source")
        };
        assert_eq!(
            pace(ProviderId::Codex),
            Some(jiff::SignedDuration::from_mins(15)),
            "the app-server's pace"
        );
        assert_eq!(pace(ProviderId::Claude), None);
    }
}
