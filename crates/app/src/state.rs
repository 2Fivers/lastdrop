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

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::grammar::meters::localize;
use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Language, LocaleFormat, Msg, Strings, TextContext};
use lastdrop_core::model::{
    AccountIdentity, AccountRef, Health, Identity, MeterKey, ProviderId, Snapshot, SourceKey,
};
use lastdrop_core::notify::{self, NotificationModel, NotifyMemory, build_notification, notices};
use lastdrop_core::readout::ReadoutForm;
use lastdrop_core::refresh::{Activity, AttemptResult, CredentialFingerprint, SourceState, record};
use lastdrop_core::secret::SecretString;
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
use crate::persist::{self, IdentityMap, SavedState, SeenMap};
use crate::scheduler::{IdentityRead, JobResult, poll_interval, reading_pace};

/// How often a source's identity is read again when nothing else asks for it (accounts-spec §2
/// "When it is read"), and how often a duplicate's identity is rechecked (§3): 30 minutes.
pub const ACCOUNT_RECHECK: SignedDuration = SignedDuration::from_mins(30);

/// When a source's identity was last read, and with which credential fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityChecked {
    /// When the job that read it ended.
    pub at: Timestamp,
    /// The credential fingerprint after that job.
    pub credential: Option<CredentialFingerprint>,
}

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
    /// Who is signed in, as last read in this run (accounts-spec §2); `None` before the first
    /// read.
    pub identity: Option<Identity>,
    /// The account last known to be signed in: kept through a sign-out, restored from the state
    /// file (accounts-spec §5).
    pub known_identity: Option<AccountIdentity>,
    /// When the identity was last read ([`identity_read_for`]).
    pub identity_checked: Option<IdentityChecked>,
    /// The earlier source of the same account, when this one is a duplicate (accounts-spec §3,
    /// [`mark_duplicates`]): no card, never fetched.
    pub duplicate_of: Option<SourceKey>,
}

/// When a duplicate's identity is due for its recheck ([`ACCOUNT_RECHECK`] after the last read;
/// `now` when it was never read in this run).
pub fn identity_due(source: &SourceRuntime, now: Timestamp) -> Timestamp {
    source
        .identity_checked
        .as_ref()
        .and_then(|c| c.at.checked_add(ACCOUNT_RECHECK).ok())
        .unwrap_or(now)
}

/// Whether a fetch job of `source` reads who is signed in (accounts-spec §2): always on its first
/// job in this run and once the last read is [`ACCOUNT_RECHECK`] old, else when the credential
/// changed since that read.
pub fn identity_read_for(source: &SourceRuntime, now: Timestamp) -> IdentityRead {
    match &source.identity_checked {
        Some(checked) if identity_due(source, now) > now => {
            IdentityRead::IfCredentialChanged(checked.credential.clone())
        }
        _ => IdentityRead::Always,
    }
}

/// Takes a job's identity reading into `source` at `at`. Returns `true` when the folder is signed
/// in to another account now (accounts-spec §5: a known identity followed by another known one);
/// a sign-out or an unknown answer keeps the last known identity.
pub fn apply_identity(
    source: &mut SourceRuntime,
    identity: Identity,
    at: Timestamp,
    credential: Option<CredentialFingerprint>,
) -> bool {
    source.identity_checked = Some(IdentityChecked { at, credential });
    let switched = match (identity.id(), &source.known_identity) {
        (Some(now), Some(before)) => now != before,
        _ => false,
    };
    if let Some(id) = identity.id() {
        source.known_identity = Some(id.clone());
    }
    source.identity = Some(identity);
    switched
}

/// Starts a source over for the account its folder holds now (accounts-spec §5): no reading, no
/// freshness, no refresh memory, so it is read again at once. Its credential fingerprint and
/// identity stay.
pub fn reset_for_new_account(source: &mut SourceRuntime) {
    source.latest = None;
    source.account.plan = None;
    source.status = ServiceStatus {
        poll_interval: source.status.poll_interval,
        reading_pace: source.status.reading_pace,
        ..ServiceStatus::default()
    };
    let in_flight = source.refresh.in_flight;
    source.refresh = SourceState {
        in_flight,
        ..SourceState::default()
    };
}

/// The account a source counts as signed in to for [`mark_duplicates`]: its answer in this run
/// (none when it said signed out or unknown), else the identity restored from the state file.
pub fn signed_in(source: &SourceRuntime) -> Option<&AccountIdentity> {
    match &source.identity {
        Some(identity) => identity.id(),
        None => source.known_identity.as_ref(),
    }
}

/// Marks the duplicates (accounts-spec §3): an enabled source whose known identity an earlier
/// enabled source of the same service (settings order) has too is a duplicate of the first one.
/// Sources whose identity is not known (signed out, unknown, never read) are never duplicates.
/// Returns whether any source changed.
pub fn mark_duplicates(
    settings: &Settings,
    sources: &mut BTreeMap<SourceKey, SourceRuntime>,
) -> bool {
    let mut changed = false;
    for (_, p) in settings.enabled_providers() {
        let mut first: Vec<(AccountIdentity, SourceKey)> = Vec::new();
        for account in &p.accounts {
            let key = SourceKey {
                provider: p.id,
                account: account.key.clone(),
            };
            let Some(source) = sources.get_mut(&key) else {
                continue;
            };
            let signed_in = signed_in(source);
            // An entry listed twice (a hand-edited file) is never a duplicate of itself.
            let duplicate = signed_in.and_then(|id| {
                first
                    .iter()
                    .find(|(seen, k)| seen == id && *k != key)
                    .map(|(_, k)| k.clone())
            });
            if duplicate.is_none()
                && let Some(id) = signed_in
            {
                first.push((id.clone(), key.clone()));
            }
            if source.duplicate_of != duplicate {
                if let Some(of) = &duplicate {
                    log::info!(
                        "{:?}/{}: same account as {}, shown once",
                        key.provider,
                        key.account.0,
                        of.account.0
                    );
                }
                source.duplicate_of = duplicate;
                changed = true;
            }
        }
    }
    changed
}

/// Names each account for the surfaces (accounts-spec §4): when a service shows two or more
/// accounts, every one without an alias gets a label, its e-mail address (unless
/// `hide_personal_info`, or the address is unknown) or `Account {n}`, `{n}` its place among the
/// shown accounts in settings order. A service showing one account has no labels.
pub fn label_accounts(
    settings: &Settings,
    sources: &mut BTreeMap<SourceKey, SourceRuntime>,
    strings: &dyn Strings,
) {
    for (_, p) in settings.enabled_providers() {
        let shown: Vec<SourceKey> = p
            .accounts
            .iter()
            .map(|a| SourceKey {
                provider: p.id,
                account: a.key.clone(),
            })
            .filter(|k| sources.get(k).is_some_and(|s| s.duplicate_of.is_none()))
            .collect();
        let several = shown.len() >= 2;
        for (i, key) in shown.iter().enumerate() {
            let Some(source) = sources.get_mut(key) else {
                continue;
            };
            source.account.label = several.then(|| {
                let email = source
                    .identity
                    .as_ref()
                    .and_then(Identity::email)
                    .filter(|_| !settings.hide_personal_info);
                match email {
                    Some(email) => email.clone(),
                    None => SecretString::new(text(
                        strings,
                        Msg::AccountN,
                        &[("n", &(i + 1).to_string())],
                    )),
                }
            });
        }
    }
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
    /// Windows' preferred display languages (`lastdrop_win::locale::ui_languages`), which the
    /// language setting follows by default.
    pub ui_languages: Vec<String>,
    /// The strings of the language the settings and `ui_languages` resolve to
    /// ([`language_of`]).
    pub strings: &'static dyn Strings,
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
    /// What notifications announced (N§1), kept across restarts in the state file.
    pub notified: NotifyMemory,
    /// `notified` as the state file has it ([`AppState::state_to_save`]).
    pub saved_notified: NotifyMemory,
    /// Every identity known (accounts-spec §5): the state file's, then what this run learned,
    /// kept for sources that are not enabled now too.
    pub identities: IdentityMap,
    /// The known identities as the state file has them ([`AppState::state_to_save`]).
    pub saved_identities: IdentityMap,
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
                // No options of their own.
                _ => true,
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
                        label: None,
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
                    identity: None,
                    known_identity: None,
                    identity_checked: None,
                    duplicate_of: None,
                },
            };
            sources.insert(key, runtime);
        }
    }
    mark_duplicates(settings, &mut sources);
    sources
}

/// Lifts the sign-in stop of every source of `provider` (auth stop, back-off, last failure and its
/// detail, sign-in required), so the next plan reads it again. A new Meshy API key does this:
/// Meshy has no credential fingerprint, so nothing else can lift its stop.
pub fn lift_sign_in_stop(sources: &mut BTreeMap<SourceKey, SourceRuntime>, provider: ProviderId) {
    for (key, source) in sources.iter_mut() {
        if key.provider == provider {
            source.refresh.auth_stopped = None;
            source.refresh.backoff_until = None;
            source.status.last_failure = None;
            source.status.failure_detail = None;
            source.status.sign_in_required = false;
        }
    }
}

/// Applies a worker's result to its source (see [`AppState::apply_result`]). Returns the
/// snapshot to record in history when a newer reading arrived.
pub fn apply_to_source(source: &mut SourceRuntime, mut result: JobResult) -> Option<Snapshot> {
    let min_interval = source.provider.min_interval();
    // A provider that had a fingerprint and could not read one this time (a file being written)
    // keeps the last one: none would stop an auth-stopped source for good (`Due::Never`).
    if result.credential.is_none() {
        result.credential.clone_from(&source.credential);
    }
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

/// The language of every string: the setting, else the first of Windows' display languages
/// with a table, else English (`Language::resolve`).
pub fn language_of(settings: &Settings, ui_languages: &[String]) -> Language {
    Language::resolve(settings.language.as_deref(), ui_languages)
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
                sources
                    .get(&SourceKey {
                        provider: p.id,
                        account: a.key.clone(),
                    })
                    // A duplicate's account is shown through its first source (accounts-spec §3).
                    .filter(|s| s.duplicate_of.is_none())
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
    /// environment); the strings in the language of the settings and Windows' display languages
    /// (`ui_languages`).
    pub fn new(
        settings: Settings,
        history: HistoryStore,
        theme: ThemeInfo,
        locale: LocaleFormat,
        ui_languages: Vec<String>,
    ) -> AppState {
        let providers = lastdrop_providers::providers(&settings);
        let sources = build_sources(&settings, &providers, &resolve_real, BTreeMap::new());
        let strings = language_of(&settings, &ui_languages).strings();
        AppState {
            providers,
            settings,
            sources,
            memory: EvalMemory::default(),
            evaluation: None,
            history,
            theme,
            locale,
            ui_languages,
            strings,
            flyout: FlyoutState::default(),
            readout: ReadoutState::default(),
            refreshing: false,
            saved_seen: SeenMap::new(),
            notified: NotifyMemory::new(),
            saved_notified: NotifyMemory::new(),
            identities: IdentityMap::new(),
            saved_identities: IdentityMap::new(),
        }
    }

    /// Applies new settings: adds and removes sources, keeps readings of sources that remain;
    /// switches the strings when the language changed.
    pub fn apply_settings(&mut self, settings: Settings) {
        let providers = reuse_providers(
            &self.providers,
            lastdrop_providers::providers(&settings),
            &self.settings,
            &settings,
        );
        // What the sources learned stays known when a source is dropped (a service turned off).
        self.identities = self.known_identities();
        let existing = std::mem::take(&mut self.sources);
        self.sources = build_sources(&settings, &providers, &resolve_real, existing);
        self.strings = language_of(&settings, &self.ui_languages).strings();
        self.settings = settings;
        self.providers = providers;
        // A source added since start-up (a service turned on again, a folder added back) knows
        // who its folder was last signed in to, so a change since is still a switch (§5) and its
        // duplicates are known before the first job (§3).
        self.restore_identities();
    }

    /// Gives every source that has read nothing in this run the identity the state file (or this
    /// run, before the source was rebuilt) last knew for it, and marks the duplicates again.
    fn restore_identities(&mut self) {
        let known = self.known_identities();
        for (key, source) in &mut self.sources {
            if source.identity_checked.is_none() && source.known_identity.is_none() {
                source.known_identity = known.get(key).cloned();
            }
        }
        mark_duplicates(&self.settings, &mut self.sources);
    }

    /// Takes Windows' display languages anew (after a regional settings change) and the
    /// strings they resolve to.
    pub fn set_ui_languages(&mut self, ui_languages: Vec<String>) {
        self.strings = language_of(&self.settings, &ui_languages).strings();
        self.ui_languages = ui_languages;
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
    /// - Always: `credential` = `result.credential` (the previous one when the job read none);
    ///   `refresh` via `lastdrop_core::refresh::record` (not for an unchanged probe), `in_flight`
    ///   cleared, `status.next_try` from the next plan.
    ///
    /// The job's identity reading (accounts-spec §2) is taken first: when the folder is signed in
    /// to another account now (§5), the source starts over, its history is retired and what the
    /// evaluations and notifications remembered about its meters is forgotten, so the reading of
    /// the same job lands in a clean history. Duplicates are marked again (§3).
    pub fn apply_result(&mut self, mut result: JobResult) {
        let key = result.source.clone();
        let Some(source) = self.sources.get_mut(&key) else {
            // The service was turned off while its fetch ran.
            return;
        };
        let mut identity_changed = false;
        if let Some(identity) = result.identity.take() {
            let before = signed_in(source).cloned();
            let switched =
                apply_identity(source, identity, result.finished, result.credential.clone());
            identity_changed = before.as_ref() != signed_in(source);
            if switched {
                log::info!(
                    "{:?}/{}: signed in to another account",
                    key.provider,
                    key.account.0
                );
                reset_for_new_account(source);
                self.history.retire(&key, result.finished);
                self.memory.forget_source(&key);
                notify::forget_source(&mut self.notified, &key);
            }
        }
        if let Some(snapshot) = apply_to_source(source, result) {
            self.history.record(&snapshot);
        }
        if identity_changed {
            mark_duplicates(&self.settings, &mut self.sources);
        }
    }

    /// Restores what earlier runs remembered (the state file): the meters they saw
    /// (`EvalMemory::seen`, so a meter a later reading lacks keeps its room in the readout, R§6.3),
    /// keeping only those the history has a reading of from the last [`IN_MEMORY_SPAN`] (31 days),
    /// so a meter that no source has reported for a month stops holding room; and what
    /// notifications announced (N§1). The first evaluation rewrites the file when entries were
    /// dropped ([`AppState::state_to_save`]).
    pub fn restore_saved(&mut self, saved: &SavedState, now: Timestamp) {
        let since = now.checked_sub(IN_MEMORY_SPAN).unwrap_or(Timestamp::MIN);
        let all = saved.seen_map();
        self.memory.seen = all
            .iter()
            .filter(|(key, _)| !self.history.samples(key, since).is_empty())
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect();
        self.saved_seen = all;
        self.notified = saved.notified_map();
        self.saved_notified = self.notified.clone();
        self.identities = saved.identity_map();
        self.saved_identities = self.identities.clone();
        self.restore_identities();
    }

    /// The identity each source is last known to be signed in to (the state file's
    /// `identities`): what the file had and what this run learned (a service turned off keeps
    /// its folders' identities), with the sources' current knowledge on top. An entry is never
    /// dropped: a sign-out keeps the last known account (§5).
    fn known_identities(&self) -> IdentityMap {
        let mut out = self.identities.clone();
        for (key, source) in &self.sources {
            if let Some(id) = &source.known_identity {
                out.insert(key.clone(), id.clone());
            }
        }
        out
    }

    /// The state file's new bytes when `EvalMemory::seen` or `notified` changed since the last
    /// save (or the restore); `None` otherwise. The caller queues them for the writer thread.
    pub fn state_to_save(&mut self) -> Option<Vec<u8>> {
        let identities = self.known_identities();
        if self.memory.seen == self.saved_seen
            && self.notified == self.saved_notified
            && identities == self.saved_identities
        {
            return None;
        }
        self.saved_seen = self.memory.seen.clone();
        self.saved_notified = self.notified.clone();
        self.saved_identities = identities;
        let state = SavedState::new(&self.memory.seen, &self.notified, &self.saved_identities);
        match persist::state_bytes(&state) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                log::warn!("cannot encode the state file: {e}");
                None
            }
        }
    }

    /// Re-evaluates at `now` (`lastdrop_core::stats::evaluate`) and stores the result, with
    /// every meter named in the language of the strings (`grammar::meters::localize`).
    pub fn evaluate(&mut self, now: Timestamp) -> &Evaluation {
        label_accounts(&self.settings, &mut self.sources, self.strings);
        let mut eval = evaluate_sources(
            &self.settings,
            &self.sources,
            &self.history,
            &mut self.memory,
            now,
        );
        localize(&mut eval, self.strings);
        self.evaluation.insert(eval)
    }

    /// The notification for what the latest evaluation raised that `notified` has not announced
    /// yet (`lastdrop_core::notify::notices`, N§1), worded at `now` (N§3); `notified` takes the
    /// new notices. `None` when there is nothing new or notifications are off.
    pub fn take_notification(&mut self, now: Timestamp) -> Option<NotificationModel> {
        let eval = self.evaluation.as_ref()?;
        let found = notices(eval, &self.settings.notifications, &mut self.notified);
        for notice in &found {
            let key = &notice.key;
            log::info!(
                "notify: {:?} {:?}/{}/{}",
                notice.kind,
                key.provider,
                key.account.0,
                key.meter.0
            );
        }
        if found.is_empty() {
            return None;
        }
        build_notification(&found, eval, &self.text_context(now))
    }

    /// The text context at `now` for the surface builders.
    pub fn text_context(&self, now: Timestamp) -> TextContext<'_> {
        TextContext {
            now,
            time_style: self.settings.readout.time_style,
            locale: &self.locale,
            strings: self.strings,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    use lastdrop_core::model::{AccountKey, Meter, MeterId, ProviderId, SourceKind, WindowLen};
    use lastdrop_core::settings::{ProviderEntry, ProviderSettings, SourceSettings};
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
                resets_at: at.checked_add(SignedDuration::from_hours(48)).ok(),
                window: WindowLen::Fixed(SignedDuration::from_hours(7 * 24)),
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
            ProviderId::Copilot => AccountSource::Copilot {
                account: AccountKey::primary(),
                config_dir: home.join(r"AppData\Roaming\GitHub CLI"),
            },
            ProviderId::Meshy => AccountSource::Meshy {
                account: AccountKey::primary(),
                credential: CredentialName {
                    provider: ProviderId::Meshy,
                    account: AccountKey::primary(),
                    name: "api_key".to_owned(),
                },
            },
            ProviderId::Cursor => AccountSource::Cursor {
                account: AccountKey::primary(),
                state_db: home.join(r"AppData\Roaming\Cursor\User\globalStorage\state.vscdb"),
            },
            ProviderId::Antigravity => AccountSource::Antigravity {
                account: AccountKey::primary(),
            },
            ProviderId::OpenCodeGo => AccountSource::OpenCodeGo {
                account: AccountKey::primary(),
                credential: CredentialName {
                    provider: ProviderId::OpenCodeGo,
                    account: AccountKey::primary(),
                    name: "api_key".to_owned(),
                },
                auth_file: home.join(r".local\share\opencode\auth.json"),
            },
            ProviderId::Grok => AccountSource::Grok {
                account: AccountKey::primary(),
                auth_file: home.join(r".grok\auth.json"),
            },
            ProviderId::Custom(_) => AccountSource::Custom {
                account: AccountKey::primary(),
                provider,
                credential: CredentialName {
                    provider,
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
                (
                    SourceKey {
                        provider: *p,
                        account: AccountKey::primary(),
                    },
                    runtime_of(settings, *p, AccountKey::primary()),
                )
            })
            .collect()
    }

    /// The source of account `key` of `provider`, with a fake provider and the default account's
    /// invented folder.
    pub(crate) fn runtime_of(settings: &Settings, p: ProviderId, key: AccountKey) -> SourceRuntime {
        let provider = FakeProvider::ok(p);
        SourceRuntime {
            account: AccountRef {
                key,
                ..AccountRef::primary()
            },
            source: source_of(p),
            status: ServiceStatus {
                poll_interval: poll_interval(settings, provider.as_ref(), Activity::default()),
                ..ServiceStatus::default()
            },
            provider,
            latest: None,
            refresh: SourceState::default(),
            credential: None,
            identity: None,
            known_identity: None,
            identity_checked: None,
            duplicate_of: None,
        }
    }

    /// The key of Codex's second account in [`two_codex`].
    pub(crate) fn work() -> SourceKey {
        SourceKey {
            provider: ProviderId::Codex,
            account: AccountKey("f-work".to_owned()),
        }
    }

    /// The key of Codex's default account.
    pub(crate) fn codex() -> SourceKey {
        SourceKey {
            provider: ProviderId::Codex,
            account: AccountKey::primary(),
        }
    }

    /// Codex with its default folder and a second one ([`work`]), both enabled, and their sources.
    pub(crate) fn two_codex() -> (Settings, BTreeMap<SourceKey, SourceRuntime>) {
        let mut settings = settings_of(&[ProviderId::Codex]);
        if let Some(p) = settings.provider_mut(ProviderId::Codex) {
            p.accounts.push(AccountSettings {
                key: work().account,
                alias: None,
                tag: None,
                source: SourceSettings::CodexHome {
                    path: PathBuf::from(r"C:\Users\tester\.codex-work"),
                },
                extra: Default::default(),
            });
        }
        let mut sources = sources_of(&settings, &[ProviderId::Codex]);
        sources.insert(
            work(),
            runtime_of(&settings, ProviderId::Codex, work().account),
        );
        (settings, sources)
    }

    /// Someone signed in, named by `name`, with an invented address when `email` is given.
    pub(crate) fn signed_in_as(name: &str, email: Option<&str>) -> Identity {
        Identity::SignedIn {
            id: AccountIdentity::of(&["codex", "ws", name]),
            email: email.map(|e| SecretString::new(e.to_owned())),
        }
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
            ui_languages: Vec::new(),
            strings: &lastdrop_core::grammar::English,
            flyout: FlyoutState::default(),
            readout: ReadoutState::default(),
            refreshing: false,
            providers: Vec::new(),
            saved_seen: SeenMap::new(),
            notified: NotifyMemory::new(),
            saved_notified: NotifyMemory::new(),
            identities: IdentityMap::new(),
            saved_identities: IdentityMap::new(),
        }
    }

    #[test]
    fn a_new_key_lifts_only_that_services_sign_in_stop() {
        let providers = [ProviderId::Claude, ProviderId::Meshy];
        let settings = settings_of(&providers);
        let mut sources = sources_of(&settings, &providers);
        let at = ts("2026-10-10T10:42:00Z");
        for source in sources.values_mut() {
            source.refresh.auth_stopped = Some(CredentialFingerprint("old".to_owned()));
            source.refresh.backoff_until = Some(at);
            source.status.last_failure = Some((at, FailureKind::Auth));
            source.status.failure_detail = Some("HTTP 401".to_owned());
            source.status.sign_in_required = true;
        }
        lift_sign_in_stop(&mut sources, ProviderId::Meshy);
        for (key, source) in &sources {
            let lifted = key.provider == ProviderId::Meshy;
            assert_eq!(source.refresh.auth_stopped.is_none(), lifted, "{key:?}");
            assert_eq!(source.refresh.backoff_until.is_none(), lifted, "{key:?}");
            assert_eq!(source.status.last_failure.is_none(), lifted, "{key:?}");
            assert_eq!(source.status.failure_detail.is_none(), lifted, "{key:?}");
            assert_eq!(!source.status.sign_in_required, lifted, "{key:?}");
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
        let earlier = now - SignedDuration::from_hours(1);
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
        second.restore_saved(&persist::load_state(&state_file), now);
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

    #[test]
    fn a_notification_is_announced_once_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-notify-test-{}-{}",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        let writer = crate::writer::Writer::start().expect("writer");
        let now = ts("2026-10-10T10:42:00Z");
        let claude = SourceKey {
            provider: ProviderId::Claude,
            account: AccountKey::primary(),
        };
        let reading = snapshot(ProviderId::Claude, now, 0.82);
        let run = |state: &mut AppState| {
            state.history.record(&reading);
            if let Some(source) = state.sources.get_mut(&claude) {
                source.latest = Some(reading.clone());
                source.status.last_success = Some(now);
            }
            state.evaluate(now);
            state.take_notification(now)
        };

        let history = HistoryStore::open(&dir.join("history"), now, writer.handle()).expect("open");
        let mut first = app_state(&[ProviderId::Claude], history);
        let shown = run(&mut first).expect("a notification");
        assert_eq!(shown.title, "Claude weekly limit is 82% used");
        assert_eq!(run(&mut first), None, "announced once");
        let bytes = first.state_to_save().expect("notified changed");
        let saved: SavedState = serde_json::from_slice(&bytes).expect("state json");

        // Restart with the state file: nothing new to say.
        let history = HistoryStore::open(&dir.join("history"), now, writer.handle()).expect("open");
        let mut second = app_state(&[ProviderId::Claude], history);
        second.restore_saved(&saved, now);
        assert_eq!(run(&mut second), None);
        // Turned off: nothing, and the memory is left alone.
        second.settings.notifications.enabled = false;
        second.notified.clear();
        assert_eq!(run(&mut second), None);
        assert!(second.notified.is_empty());
        writer.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strings_and_meter_names_follow_the_setting_then_windows() {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-language-test-{}-{}",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        let writer = crate::writer::Writer::start().expect("writer");
        let now = ts("2026-10-10T10:42:00Z");
        let history = HistoryStore::open(&dir.join("history"), now, writer.handle()).expect("open");
        let mut state = app_state(&[ProviderId::Claude], history);
        assert_eq!(state.strings.language(), "en");

        // Windows in Swedish, no language picked: Swedish, meter names included.
        state.set_ui_languages(vec!["sv-SE".to_owned(), "en-US".to_owned()]);
        assert_eq!(state.strings.language(), "sv");
        let reading = snapshot(ProviderId::Claude, now, 0.82);
        state.history.record(&reading);
        if let Some(source) = state.sources.values_mut().next() {
            source.latest = Some(reading.clone());
            source.status.last_success = Some(now);
        }
        let eval = state.evaluate(now);
        let names = &eval.services[0].meters[0].names;
        assert_eq!(
            (names.short_label.as_str(), names.short_name.as_str()),
            ("Vk", "veckogränsen")
        );
        // Numbers keep the regional format (en-GB here), whatever the language.
        let notice = state.take_notification(now).expect("a notification");
        assert_eq!(notice.title, "Veckogränsen för Claude är till 82% använd");

        // English picked: English whatever Windows says.
        state.settings.language = Some("en".to_owned());
        state.set_ui_languages(vec!["sv-SE".to_owned()]);
        assert_eq!(state.strings.language(), "en");
        let eval = state.evaluate(now);
        assert_eq!(eval.services[0].meters[0].names.short_name, "weekly");
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
            identity: None,
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
            Some(SignedDuration::from_mins(15)),
            "the app-server's pace"
        );
        assert_eq!(pace(ProviderId::Claude), None);
    }

    // ---- Several accounts (accounts-spec §3–§5) ------------------------------------------------

    fn set_identity(
        sources: &mut BTreeMap<SourceKey, SourceRuntime>,
        key: &SourceKey,
        identity: Identity,
    ) -> bool {
        let at = ts("2026-10-10T10:42:00Z");
        let source = sources.get_mut(key).expect("source");
        apply_identity(source, identity, at, None)
    }

    #[test]
    fn the_same_account_in_two_folders_shows_once() {
        let (settings, mut sources) = two_codex();
        let now = ts("2026-10-10T10:42:00Z");
        let same = signed_in_as("one", Some("one@example.invalid"));
        set_identity(&mut sources, &codex(), same.clone());
        set_identity(&mut sources, &work(), same);
        assert!(mark_duplicates(&settings, &mut sources));
        assert_eq!(sources[&work()].duplicate_of, Some(codex()));
        assert_eq!(
            sources[&codex()].duplicate_of,
            None,
            "the first one is the account"
        );
        assert!(!mark_duplicates(&settings, &mut sources), "nothing changed");

        let eval = evaluate_sources(
            &settings,
            &sources,
            &lastdrop_core::stats::NoHistory,
            &mut EvalMemory::default(),
            now,
        );
        assert_eq!(eval.services.len(), 1);
        assert_eq!(eval.services[0].source, codex());

        // The second folder signs in to someone else: two accounts.
        set_identity(&mut sources, &work(), signed_in_as("two", None));
        assert!(mark_duplicates(&settings, &mut sources));
        assert_eq!(sources[&work()].duplicate_of, None);

        // Signed out or unknown is never a duplicate, whatever was known before.
        set_identity(&mut sources, &work(), signed_in_as("one", None));
        mark_duplicates(&settings, &mut sources);
        assert!(sources[&work()].duplicate_of.is_some());
        set_identity(&mut sources, &work(), Identity::SignedOut);
        mark_duplicates(&settings, &mut sources);
        assert_eq!(sources[&work()].duplicate_of, None);
        assert_eq!(
            sources[&work()].known_identity.as_ref(),
            signed_in_as("one", None).id(),
            "a sign-out keeps the last known account"
        );
    }

    #[test]
    fn identities_from_an_earlier_run_mark_duplicates_before_the_first_read() {
        let (settings, mut sources) = two_codex();
        let id = AccountIdentity::of(&["codex", "ws", "one"]);
        for key in [codex(), work()] {
            sources.get_mut(&key).expect("source").known_identity = Some(id.clone());
        }
        mark_duplicates(&settings, &mut sources);
        assert_eq!(sources[&work()].duplicate_of, Some(codex()));
        // Once read in this run, the answer counts.
        set_identity(&mut sources, &codex(), Identity::Unknown);
        mark_duplicates(&settings, &mut sources);
        assert_eq!(sources[&work()].duplicate_of, None);
    }

    #[test]
    fn accounts_are_named_when_a_service_shows_several() {
        let (mut settings, mut sources) = two_codex();
        let now = ts("2026-10-10T10:42:00Z");
        set_identity(
            &mut sources,
            &codex(),
            signed_in_as("one", Some("one@example.invalid")),
        );
        set_identity(
            &mut sources,
            &work(),
            signed_in_as("two", Some("two@example.invalid")),
        );
        let names = |settings: &Settings,
                     sources: &mut BTreeMap<SourceKey, SourceRuntime>,
                     strings: &dyn Strings| {
            label_accounts(settings, sources, strings);
            let eval = evaluate_sources(
                settings,
                sources,
                &lastdrop_core::stats::NoHistory,
                &mut EvalMemory::default(),
                now,
            );
            eval.services
                .iter()
                .map(|s| s.display_name().into_owned())
                .collect::<Vec<String>>()
        };
        let english = &lastdrop_core::grammar::English;
        assert_eq!(
            names(&settings, &mut sources, english),
            ["Codex · one@example.invalid", "Codex · two@example.invalid"]
        );

        // An alias wins; hiding personal info numbers the others.
        if let Some(p) = settings.provider_mut(ProviderId::Codex) {
            p.accounts[0].alias = Some("Work".to_owned());
        }
        for source in sources.values_mut() {
            let alias = settings
                .provider(ProviderId::Codex)
                .and_then(|p| p.accounts.iter().find(|a| a.key == source.account.key))
                .and_then(|a| a.alias.clone());
            source.account.alias = alias;
        }
        settings.hide_personal_info = true;
        assert_eq!(
            names(&settings, &mut sources, english),
            ["Codex · Work", "Codex · Account 2"]
        );
        assert_eq!(
            names(&settings, &mut sources, &lastdrop_core::grammar::Swedish),
            ["Codex · Work", "Codex · Konto 2"]
        );

        // No address known: numbered too.
        settings.hide_personal_info = false;
        set_identity(&mut sources, &work(), signed_in_as("two", None));
        assert_eq!(
            names(&settings, &mut sources, english),
            ["Codex · Work", "Codex · Account 2"]
        );

        // One account shown (the other is the same account): the service name alone.
        if let Some(p) = settings.provider_mut(ProviderId::Codex) {
            p.accounts[0].alias = None;
        }
        for source in sources.values_mut() {
            source.account.alias = None;
        }
        set_identity(&mut sources, &work(), signed_in_as("one", None));
        mark_duplicates(&settings, &mut sources);
        assert_eq!(names(&settings, &mut sources, english), ["Codex"]);
    }

    #[test]
    fn a_service_turned_off_and_on_again_remembers_who_was_signed_in() {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-offon-test-{}-{}",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        let writer = crate::writer::Writer::start().expect("writer");
        let now = ts("2026-10-10T10:42:00Z");
        let history = HistoryStore::open(&dir.join("history"), now, writer.handle()).expect("open");
        let mut state = app_state(&[ProviderId::Codex], history);
        let mut job = result(&codex(), None);
        job.identity = Some(signed_in_as("one", None));
        state.apply_result(job);

        // Off before anything was saved, then on again: the rebuilt source still knows "one".
        let mut off = state.settings.clone();
        if let Some(p) = off.provider_mut(ProviderId::Codex) {
            p.enabled = false;
        }
        state.apply_settings(off);
        assert!(state.sources.is_empty());
        let mut on = state.settings.clone();
        if let Some(p) = on.provider_mut(ProviderId::Codex) {
            p.enabled = true;
        }
        state.apply_settings(on);
        assert_eq!(
            state.sources[&codex()].known_identity.as_ref(),
            signed_in_as("one", None).id()
        );

        // Signed in to someone else meanwhile: a switch, so the old reading goes.
        state.sources.get_mut(&codex()).expect("source").latest =
            Some(snapshot(ProviderId::Codex, now, 0.5));
        let mut job = result(&codex(), None);
        job.identity = Some(signed_in_as("two", None));
        state.apply_result(job);
        assert!(state.sources[&codex()].latest.is_none(), "started over");

        // A sign-out keeps the last known account in the state file.
        let mut job = result(&codex(), None);
        job.identity = Some(Identity::SignedOut);
        state.apply_result(job);
        let saved = state.state_to_save().expect("identities changed");
        let saved: SavedState = serde_json::from_slice(&saved).expect("state json");
        assert_eq!(
            saved.identity_map().get(&codex()),
            signed_in_as("two", None).id()
        );
        writer.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_job_that_reads_no_credential_keeps_the_last_one() {
        let (_, mut sources) = two_codex();
        let source = sources.get_mut(&codex()).expect("source");
        apply_to_source(source, result(&codex(), None));
        assert_eq!(
            source.credential,
            Some(CredentialFingerprint("v1".to_owned()))
        );
        let mut unreadable = result(&codex(), None);
        unreadable.credential = None;
        apply_to_source(source, unreadable);
        assert_eq!(
            source.credential,
            Some(CredentialFingerprint("v1".to_owned())),
            "an auth-stopped source keeps being probed"
        );
    }

    #[test]
    fn an_account_listed_twice_is_not_its_own_duplicate() {
        let mut settings = settings_of(&[ProviderId::Codex]);
        if let Some(p) = settings.provider_mut(ProviderId::Codex) {
            p.accounts.push(AccountSettings::primary());
        }
        let mut sources = sources_of(&settings, &[ProviderId::Codex]);
        set_identity(&mut sources, &codex(), signed_in_as("one", None));
        mark_duplicates(&settings, &mut sources);
        assert_eq!(sources[&codex()].duplicate_of, None);
    }

    #[test]
    fn identity_reads_follow_the_credential_and_the_clock() {
        let (_, mut sources) = two_codex();
        let now = ts("2026-10-10T10:42:00Z");
        let source = sources.get_mut(&codex()).expect("source");
        assert_eq!(
            identity_read_for(source, now),
            IdentityRead::Always,
            "never read"
        );
        let v1 = Some(CredentialFingerprint("v1".to_owned()));
        apply_identity(source, Identity::Unknown, now, v1.clone());
        assert_eq!(
            identity_read_for(source, now + SignedDuration::from_mins(5)),
            IdentityRead::IfCredentialChanged(v1)
        );
        assert_eq!(
            identity_read_for(source, now + ACCOUNT_RECHECK),
            IdentityRead::Always,
            "30 minutes on"
        );
        assert_eq!(identity_due(source, now), now + ACCOUNT_RECHECK);
    }

    #[test]
    fn a_folder_signed_in_to_someone_else_starts_over() {
        let dir = std::env::temp_dir().join(format!(
            "lastdrop-switch-test-{}-{}",
            std::process::id(),
            Timestamp::now().as_nanosecond()
        ));
        let writer = crate::writer::Writer::start().expect("writer");
        let now = ts("2026-10-10T10:42:00Z");
        let history_dir = dir.join("history");
        let history = HistoryStore::open(&history_dir, now, writer.handle()).expect("open");
        let mut state = app_state(&[ProviderId::Codex], history);
        let key = codex();
        let meter = MeterKey {
            provider: ProviderId::Codex,
            account: AccountKey::primary(),
            meter: MeterId::new(MeterId::WEEKLY),
        };

        // The first account: read, recorded, announced.
        let first = snapshot(ProviderId::Codex, now - SignedDuration::from_mins(1), 0.85);
        let mut job = result(&key, Some(Ok(first)));
        job.identity = Some(signed_in_as("one", None));
        state.apply_result(job);
        state.evaluate(now);
        assert!(state.take_notification(now).is_some(), "85 % announced");
        assert!(!state.notified.is_empty());
        assert_eq!(state.history.samples(&meter, Timestamp::MIN).len(), 1);
        writer.handle().flush();

        // The folder now holds another account: its reading lands in a clean history.
        let second = snapshot(ProviderId::Codex, now, 0.1);
        let mut job = result(&key, Some(Ok(second.clone())));
        job.identity = Some(signed_in_as("two", None));
        state.apply_result(job);
        assert!(
            state.notified.is_empty(),
            "the new account's levels come afresh"
        );
        let samples = state.history.samples(&meter, Timestamp::MIN);
        assert_eq!(samples.len(), 1);
        assert!((samples[0].value - 0.1).abs() < 1e-9);
        let source = &state.sources[&key];
        assert_eq!(source.latest.as_ref(), Some(&second));
        assert_eq!(
            source.known_identity.as_ref(),
            signed_in_as("two", None).id()
        );

        // On disk the first account's file was retired before the new reading was appended.
        writer.handle().flush();
        let names: Vec<String> = std::fs::read_dir(&history_dir)
            .expect("list")
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .collect();
        let weekly = HistoryStore::file_name(&meter);
        assert!(names.contains(&weekly), "{names:?}");
        assert!(
            names
                .iter()
                .any(|n| n.starts_with(&weekly) && n.ends_with(".retired")),
            "{names:?}"
        );
        let text = std::fs::read_to_string(history_dir.join(&weekly)).expect("read");
        assert_eq!(text.lines().count(), 1, "{text}");

        // Signing out is not another account.
        let mut job = result(&key, None);
        job.identity = Some(Identity::SignedOut);
        state.apply_result(job);
        assert!(state.sources[&key].latest.is_some(), "kept");
        let saved = state.state_to_save().expect("identities changed");
        let saved: SavedState = serde_json::from_slice(&saved).expect("state json");
        assert_eq!(
            saved.identity_map().get(&key),
            signed_in_as("two", None).id()
        );
        writer.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
