//! The refresh scheduler (ARCHITECTURE "Refresh scheduler"): a small pool of worker threads that
//! run provider fetches, one job per credential source in flight, with the timing decided by the
//! pure policy in `lastdrop_core::refresh`.
//!
//! The main thread asks [`plan`] which sources are due, submits [`Job`]s, and arms one Slint timer
//! for the next wake-up. Workers run `Provider::fetch` with real I/O ([`WorkerIo`]) and hand each
//! [`JobResult`] to the sink, which forwards it to the main thread with
//! `slint::invoke_from_event_loop`. Workers never touch UI objects.

use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use jiff::{SignedDuration, Timestamp};
use lastdrop_core::model::{Snapshot, SourceKey};
use lastdrop_core::refresh::{
    Activity, CredentialFingerprint, Due, PolicyInput, effective_interval, next_due,
};
use lastdrop_core::secret::SecretString;
use lastdrop_core::settings::{RefreshInterval, Settings};
use lastdrop_providers::io::{Clock, CredentialName, Secrets, SecretsError};
use lastdrop_providers::real::{RealClock, RealEnv, RealFs, RealSpawner, UreqHttp};
use lastdrop_providers::{AccountSource, FetchContext, FetchError, Provider};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::state::{AppState, SourceRuntime};

/// Worker threads: 2 (**not from the spec**: three M1 services, each fetch mostly waits).
pub const WORKERS: usize = 2;

/// The deadline of one fetch (`FetchContext::deadline`): 20 s, enough for the app-server's 8 s
/// start plus a call, and HTTP timeouts.
pub const FETCH_BUDGET: Duration = Duration::from_secs(20);

/// How long [`Scheduler::shutdown`] waits for fetches that are already running (**not from the
/// spec**): long enough for a file read or an HTTP answer, short enough that a quick relaunch
/// does not find the dying instance still holding the single-instance mutex. Workers still
/// running after it end with the process (a `codex` child dies with its kill-on-close job).
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// One fetch to run.
pub struct Job {
    /// The source.
    pub source: SourceKey,
    /// Its provider.
    pub provider: Arc<dyn Provider>,
    /// Where its data is.
    pub account: AccountSource,
    /// Started by the user (Refresh now, F5, Retry) rather than the timer.
    pub manual: bool,
    /// A credential probe of an auth-stopped source (`Due::ProbeCredential`): the worker reads
    /// `Provider::credential_fingerprint` and fetches only when it differs from this stopped
    /// fingerprint. `None` = a normal fetch.
    pub probe: Option<CredentialFingerprint>,
}

/// The result of one job.
pub struct JobResult {
    /// The source.
    pub source: SourceKey,
    /// When the job started.
    pub started: Timestamp,
    /// When it ended.
    pub finished: Timestamp,
    /// The snapshot or the error; `None` when a probe found the credential unchanged (nothing
    /// was fetched, nothing is recorded as an attempt).
    pub outcome: Option<Result<Snapshot, FetchError>>,
    /// The credential fingerprint after the attempt (auth stop, `lastdrop_core::refresh`).
    pub credential: Option<CredentialFingerprint>,
    /// Network List Manager's answer after a failed attempt
    /// (`lastdrop_win::network::connected_to_internet`, asked on the worker; `None` after a
    /// success or when NLM fails). `Some(false)` turns any failure into OFFLINE (R§3.8: "no
    /// internet per Network List Manager, or DNS/connect failures").
    pub internet: Option<bool>,
}

/// Receives results on a worker thread; must not block.
pub type ResultSink = Arc<dyn Fn(JobResult) + Send + Sync + 'static>;

/// The real I/O handed to providers through `FetchContext`.
pub struct WorkerIo {
    /// System clock.
    pub clock: RealClock,
    /// `ureq` with SChannel.
    pub http: UreqHttp,
    /// Read-only files.
    pub fs: RealFs,
    /// Credential Manager.
    pub secrets: CredentialManager,
    /// Processes with `CREATE_NO_WINDOW`.
    pub spawner: RealSpawner,
    /// Environment.
    pub env: RealEnv,
}

impl WorkerIo {
    /// The production I/O.
    pub fn new() -> WorkerIo {
        WorkerIo {
            clock: RealClock,
            http: UreqHttp::new(),
            fs: RealFs,
            secrets: CredentialManager,
            spawner: RealSpawner,
            env: RealEnv,
        }
    }
}

impl Default for WorkerIo {
    fn default() -> WorkerIo {
        WorkerIo::new()
    }
}

/// `lastdrop_providers::io::Secrets` over `lastdrop_win::credentials` (Windows Credential Manager).
#[derive(Clone, Copy, Debug, Default)]
pub struct CredentialManager;

impl Secrets for CredentialManager {
    fn get(&self, name: &CredentialName) -> Result<Option<SecretString>, SecretsError> {
        lastdrop_win::credentials::read(&name.target()).map_err(|e| SecretsError::Store(e.0))
    }

    fn set(&self, name: &CredentialName, secret: &SecretString) -> Result<(), SecretsError> {
        lastdrop_win::credentials::write(&name.target(), secret)
            .map_err(|e| SecretsError::Store(e.0))
    }

    fn delete(&self, name: &CredentialName) -> Result<(), SecretsError> {
        lastdrop_win::credentials::delete(&name.target()).map_err(|e| SecretsError::Store(e.0))
    }
}

/// Runs one job with `io`'s clock and the given I/O, on the calling thread: a probe reads the
/// credential fingerprint and fetches only when it changed; a fetch is followed by a fingerprint
/// read; a failure asks `internet` whether the machine is online. A panicking provider is
/// reported as [`FetchError::Unavailable`] (debug builds; release builds abort on panic).
pub fn run_job(
    job: &Job,
    clock: &dyn Clock,
    io: &FetchContext<'_>,
    internet: &dyn Fn() -> Option<bool>,
) -> JobResult {
    let started = clock.now();
    let deadline = started
        .checked_add(SignedDuration::try_from(FETCH_BUDGET).unwrap_or(SignedDuration::ZERO))
        .unwrap_or(started);
    let ctx = FetchContext {
        deadline,
        manual: job.manual && job.probe.is_none(),
        ..*io
    };
    let guarded = |f: &dyn Fn() -> Result<Snapshot, FetchError>| {
        catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
            Err(FetchError::Unavailable {
                detail: "internal error".to_owned(),
            })
        })
    };
    let fingerprint = || {
        catch_unwind(AssertUnwindSafe(|| {
            job.provider.credential_fingerprint(&ctx, &job.account)
        }))
        .ok()
        .flatten()
    };
    let fetch = || guarded(&|| job.provider.fetch(&ctx, &job.account));

    let (outcome, credential) = match &job.probe {
        Some(stopped) => {
            let current = fingerprint();
            match current {
                Some(c) if &c != stopped => {
                    let outcome = fetch();
                    (Some(outcome), fingerprint().or(Some(c)))
                }
                // Unchanged (or unreadable): nothing is fetched.
                other => (None, other),
            }
        }
        None => {
            let outcome = fetch();
            (Some(outcome), fingerprint())
        }
    };
    let internet = match &outcome {
        Some(Err(_)) => internet(),
        _ => None,
    };
    match &outcome {
        Some(Ok(s)) => log::info!(
            "{:?}/{}: read from {:?}",
            job.source.provider,
            job.source.account.0,
            s.source
        ),
        Some(Err(e)) => log::warn!("{:?}/{}: {e}", job.source.provider, job.source.account.0),
        None => log::debug!(
            "{:?}/{}: credential unchanged",
            job.source.provider,
            job.source.account.0
        ),
    }
    JobResult {
        source: job.source.clone(),
        started,
        finished: clock.now(),
        outcome,
        credential,
        internet,
    }
}

/// The worker pool.
pub struct Scheduler {
    workers: Vec<JoinHandle<()>>,
    queue: Option<Sender<Job>>,
    /// Set by [`Scheduler::shutdown`]: workers start no further job.
    stop: Arc<AtomicBool>,
}

impl Scheduler {
    /// Starts [`WORKERS`] threads sharing one job queue. Each worker initializes COM (MTA) for
    /// Network List Manager and runs jobs until [`Scheduler::shutdown`].
    pub fn start(io: Arc<WorkerIo>, sink: ResultSink) -> std::io::Result<Scheduler> {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let stop = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::with_capacity(WORKERS);
        for n in 0..WORKERS {
            let (io, sink, rx, stop) = (io.clone(), sink.clone(), rx.clone(), stop.clone());
            let handle = std::thread::Builder::new()
                .name(format!("lastdrop-refresh-{n}"))
                .spawn(move || worker(&io, &sink, &rx, &stop))?;
            workers.push(handle);
        }
        Ok(Scheduler {
            workers,
            queue: Some(tx),
            stop,
        })
    }

    /// Queues a job. The caller ensures one job per source in flight (`SourceState::in_flight`).
    pub fn submit(&self, job: Job) {
        let sent = self.queue.as_ref().is_some_and(|q| q.send(job).is_ok());
        if !sent {
            log::error!("refresh queue closed; job dropped");
        }
    }

    /// Stops the workers: queued jobs are dropped without running (the channel would otherwise
    /// deliver every buffered job before it reports closed), running fetches get
    /// [`SHUTDOWN_WAIT`] to finish, and workers still busy after that are left to end with the
    /// process.
    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::Release);
        self.queue = None;
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while self.workers.iter().any(|h| !h.is_finished()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        for handle in self.workers.drain(..) {
            if !handle.is_finished() {
                log::info!("a refresh worker is still fetching; it ends with the process");
                continue;
            }
            if handle.join().is_err() {
                log::error!("a refresh worker panicked");
            }
        }
    }
}

/// One worker: takes jobs until the queue closes or the scheduler stops.
fn worker(io: &WorkerIo, sink: &ResultSink, rx: &Mutex<Receiver<Job>>, stop: &AtomicBool) {
    // SAFETY: called once at the start of this thread, before any COM use on it; balanced by
    // CoUninitialize below when it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let ctx = FetchContext {
        clock: &io.clock,
        http: &io.http,
        fs: &io.fs,
        secrets: &io.secrets,
        spawner: &io.spawner,
        env: &io.env,
        deadline: Timestamp::now(),
        manual: false,
    };
    loop {
        // The lock is held only while waiting for the next job (no other work).
        let job = match rx.lock() {
            Ok(guard) => guard.recv(),
            Err(poisoned) => poisoned.into_inner().recv(),
        };
        let Ok(job) = job else {
            break;
        };
        if stop.load(Ordering::Acquire) {
            // Shutting down: a queued job is dropped, not run.
            break;
        }
        let result = run_job(&job, &io.clock, &ctx, &|| {
            catch_unwind(lastdrop_win::network::connected_to_internet)
                .ok()
                .flatten()
        });
        sink(result);
    }
    if com {
        // SAFETY: balances the successful CoInitializeEx of this thread.
        unsafe { CoUninitialize() };
    }
}

/// What the timer should do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Sources to fetch now.
    pub due: Vec<SourceKey>,
    /// Auth-stopped sources whose credential fingerprint a worker should re-read now
    /// (`Due::ProbeCredential`).
    pub probes: Vec<SourceKey>,
    /// When to look again (`None` = only on events).
    pub next_wake: Option<Timestamp>,
    /// The next planned attempt per source (`ServiceStatus::next_try`, `next try 13:30` in
    /// F§4.3); sources due now have `now`, sources without a plan are absent.
    pub next_try: BTreeMap<SourceKey, Timestamp>,
}

/// The interval in effect for a service: its own, else the global one (ROADMAP "Refresh
/// interval per provider").
pub fn interval_of(
    settings: &Settings,
    provider: lastdrop_core::model::ProviderId,
) -> RefreshInterval {
    settings
        .provider(provider)
        .and_then(|p| p.interval)
        .unwrap_or(settings.refresh.interval)
}

/// Runs `lastdrop_core::refresh::next_due` for every source of `state` (effective interval,
/// adaptive `activity`, reset refreshes with `jitter_unit()` per source, back-off, auth stop and
/// credential probes).
pub fn plan(
    state: &AppState,
    now: Timestamp,
    activity: Activity,
    jitter_unit: &mut dyn FnMut() -> f64,
) -> Plan {
    plan_sources(&state.settings, &state.sources, now, activity, jitter_unit)
}

/// [`plan`] over the pieces of the state it reads.
pub fn plan_sources(
    settings: &Settings,
    sources: &BTreeMap<SourceKey, SourceRuntime>,
    now: Timestamp,
    activity: Activity,
    jitter_unit: &mut dyn FnMut() -> f64,
) -> Plan {
    let paused = settings
        .pause
        .is_some_and(|p| p.until.is_none_or(|until| now < until));
    let mut out = Plan {
        due: Vec::new(),
        probes: Vec::new(),
        next_wake: None,
        next_try: BTreeMap::new(),
    };
    let wake = |t: Timestamp, out: &mut Plan| {
        out.next_wake = Some(out.next_wake.map_or(t, |w| w.min(t)));
    };
    // Service order first (R§3.10 tie break), then accounts.
    for (_, provider) in settings.enabled_providers() {
        for (key, source) in sources.iter().filter(|(k, _)| k.provider == provider.id) {
            let resets: Vec<Timestamp> = source
                .latest
                .iter()
                .flat_map(|s| s.meters.iter().filter_map(|m| m.resets_at))
                .collect();
            let due = next_due(&PolicyInput {
                now,
                min_interval: source.provider.min_interval(),
                interval: interval_of(settings, provider.id),
                adaptive: settings.refresh.adaptive,
                activity,
                paused,
                resets: &resets,
                jitter_unit: jitter_unit(),
                credential: source.credential.as_ref(),
                state: &source.refresh,
            });
            match due {
                Due::At(t) if t <= now => {
                    out.due.push(key.clone());
                    out.next_try.insert(key.clone(), now);
                }
                Due::At(t) => {
                    out.next_try.insert(key.clone(), t);
                    wake(t, &mut out);
                }
                Due::ProbeCredential(t) if t <= now => out.probes.push(key.clone()),
                Due::ProbeCredential(t) => wake(t, &mut out),
                Due::InFlight | Due::Never => {}
            }
        }
    }
    // A pause that ends by itself wakes the timer at its end.
    if let Some(until) = settings.pause.and_then(|p| p.until).filter(|u| *u > now) {
        wake(until, &mut out);
    }
    out
}

/// The pace a provider keeps for its live reading (`ServiceStatus::reading_pace`,
/// `lastdrop_core::stats::stale_after_paced`).
pub fn reading_pace(provider: &dyn Provider) -> Option<SignedDuration> {
    provider
        .reading_pace()
        .and_then(|pace| SignedDuration::try_from(pace).ok())
}

/// The poll interval in effect for a source (`ServiceStatus::poll_interval`, R§3.8 `stale_after`).
pub fn poll_interval(
    settings: &Settings,
    provider: &dyn Provider,
    activity: Activity,
) -> Option<SignedDuration> {
    effective_interval(
        interval_of(settings, provider.id()),
        settings.refresh.adaptive,
        activity,
        provider.min_interval(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::{FakeProvider, settings_of, sources_of};
    use lastdrop_core::model::ProviderId;
    use lastdrop_core::refresh::SourceState;
    use lastdrop_core::settings::Pause;
    use lastdrop_providers::fakes::Fakes;

    fn ts(s: &str) -> Timestamp {
        s.parse().expect("valid timestamp")
    }

    #[test]
    fn first_round_fetches_everything_then_waits_for_the_interval() {
        let now = ts("2026-10-10T10:42:00Z");
        let settings = settings_of(&[ProviderId::Claude, ProviderId::Codex]);
        let mut sources = sources_of(&settings, &[ProviderId::Claude, ProviderId::Codex]);
        let plan1 = plan_sources(&settings, &sources, now, Activity::default(), &mut || 0.5);
        assert_eq!(plan1.due.len(), 2);
        assert!(plan1.probes.is_empty());
        assert_eq!(plan1.next_wake, None);
        assert_eq!(plan1.due[0].provider, ProviderId::Claude, "service order");

        // After attempts: next one 5 minutes later (the default interval).
        for source in sources.values_mut() {
            source.refresh = SourceState {
                last_attempt: Some(now),
                last_success: Some(now),
                ..SourceState::default()
            };
        }
        let plan2 = plan_sources(&settings, &sources, now, Activity::default(), &mut || 0.5);
        assert!(plan2.due.is_empty());
        let next = ts("2026-10-10T10:47:00Z");
        assert_eq!(plan2.next_wake, Some(next));
        assert!(plan2.next_try.values().all(|t| *t == next));
    }

    #[test]
    fn in_flight_and_paused_sources_wait() {
        let now = ts("2026-10-10T10:42:00Z");
        let mut settings = settings_of(&[ProviderId::Claude]);
        let mut sources = sources_of(&settings, &[ProviderId::Claude]);
        for source in sources.values_mut() {
            source.refresh.in_flight = true;
        }
        let p = plan_sources(&settings, &sources, now, Activity::default(), &mut || 0.5);
        assert!(p.due.is_empty());

        for source in sources.values_mut() {
            source.refresh.in_flight = false;
        }
        let end = ts("2026-10-10T11:42:00Z");
        settings.pause = Some(Pause { until: Some(end) });
        let p = plan_sources(&settings, &sources, now, Activity::default(), &mut || 0.5);
        assert!(p.due.is_empty());
        assert_eq!(p.next_wake, Some(end), "wakes when the pause ends");
    }

    #[test]
    fn jobs_run_probes_and_report_failures() {
        let now = ts("2026-10-10T10:42:00Z");
        let fakes = Fakes::new(now);
        let ctx = fakes.context(SignedDuration::from_secs(20));
        let ok = FakeProvider::ok(ProviderId::Claude);
        let source = crate::state::tests::source_of(ProviderId::Claude);
        let job = |provider: Arc<dyn Provider>, probe: Option<CredentialFingerprint>| Job {
            source: SourceKey {
                provider: ProviderId::Claude,
                account: lastdrop_core::model::AccountKey::primary(),
            },
            provider,
            account: source.clone(),
            manual: false,
            probe,
        };
        let offline = || Some(false);

        let r = run_job(&job(ok.clone(), None), &fakes.clock, &ctx, &offline);
        assert!(matches!(r.outcome, Some(Ok(_))));
        assert_eq!(r.internet, None, "NLM is asked only after a failure");
        assert_eq!(r.credential, Some(CredentialFingerprint("v1".to_owned())));

        // A probe with the same fingerprint fetches nothing.
        let same = Some(CredentialFingerprint("v1".to_owned()));
        let r = run_job(&job(ok.clone(), same), &fakes.clock, &ctx, &offline);
        assert!(r.outcome.is_none());
        // A changed fingerprint fetches.
        let old = Some(CredentialFingerprint("v0".to_owned()));
        let r = run_job(&job(ok, old), &fakes.clock, &ctx, &offline);
        assert!(matches!(r.outcome, Some(Ok(_))));

        let failing = FakeProvider::failing(ProviderId::Claude);
        let r = run_job(&job(failing, None), &fakes.clock, &ctx, &offline);
        assert!(matches!(r.outcome, Some(Err(_))));
        assert_eq!(r.internet, Some(false));
    }

    #[test]
    fn a_worker_pool_runs_jobs_and_shuts_down() {
        // The pool itself, with a provider that needs no real I/O.
        let (tx, rx) = mpsc::channel();
        let sink: ResultSink = Arc::new(move |r: JobResult| {
            let _ = tx.send(r.source);
        });
        let scheduler = Scheduler::start(Arc::new(WorkerIo::new()), sink).expect("threads");
        let source = crate::state::tests::source_of(ProviderId::Codex);
        scheduler.submit(Job {
            source: SourceKey {
                provider: ProviderId::Codex,
                account: lastdrop_core::model::AccountKey::primary(),
            },
            provider: FakeProvider::ok(ProviderId::Codex),
            account: source,
            manual: true,
            probe: None,
        });
        let done = rx.recv_timeout(Duration::from_secs(10)).expect("result");
        assert_eq!(done.provider, ProviderId::Codex);
        scheduler.shutdown();
    }

    /// A provider that records whether each fetch was a manual one (`FetchContext::manual`).
    struct ManualRecorder(Mutex<Vec<bool>>);

    impl Provider for ManualRecorder {
        fn id(&self) -> ProviderId {
            ProviderId::Codex
        }

        fn fetch(
            &self,
            ctx: &FetchContext<'_>,
            _account: &AccountSource,
        ) -> Result<Snapshot, FetchError> {
            self.0.lock().expect("lock").push(ctx.manual);
            Err(FetchError::Unavailable {
                detail: "recording test provider".to_owned(),
            })
        }

        fn min_interval(&self) -> Duration {
            Duration::from_secs(60)
        }
    }

    #[test]
    fn a_manual_job_tells_the_provider() {
        let fakes = Fakes::new(ts("2026-10-10T10:42:00Z"));
        let ctx = fakes.context(SignedDuration::from_secs(20));
        let recorder = Arc::new(ManualRecorder(Mutex::new(Vec::new())));
        let source = crate::state::tests::source_of(ProviderId::Codex);
        for manual in [true, false] {
            let job = Job {
                source: SourceKey {
                    provider: ProviderId::Codex,
                    account: lastdrop_core::model::AccountKey::primary(),
                },
                provider: recorder.clone(),
                account: source.clone(),
                manual,
                probe: None,
            };
            run_job(&job, &fakes.clock, &ctx, &|| None);
        }
        assert_eq!(*recorder.0.lock().expect("lock"), [true, false]);
    }

    /// A provider whose fetch takes `delay` and counts its calls.
    struct SlowProvider {
        delay: Duration,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Provider for SlowProvider {
        fn id(&self) -> ProviderId {
            ProviderId::Codex
        }

        fn fetch(
            &self,
            _ctx: &FetchContext<'_>,
            _account: &AccountSource,
        ) -> Result<Snapshot, FetchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            Err(FetchError::Unavailable {
                detail: "slow test provider".to_owned(),
            })
        }

        fn min_interval(&self) -> Duration {
            Duration::from_secs(60)
        }

        fn credential_fingerprint(
            &self,
            _ctx: &FetchContext<'_>,
            _account: &AccountSource,
        ) -> Option<CredentialFingerprint> {
            None
        }
    }

    #[test]
    fn shutdown_drops_queued_jobs_and_does_not_wait_for_long_fetches() {
        let sink: ResultSink = Arc::new(|_: JobResult| {});
        let scheduler = Scheduler::start(Arc::new(WorkerIo::new()), sink).expect("threads");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let provider: Arc<dyn Provider> = Arc::new(SlowProvider {
            delay: Duration::from_millis(300),
            calls: calls.clone(),
        });
        let source = crate::state::tests::source_of(ProviderId::Codex);
        for _ in 0..6 {
            scheduler.submit(Job {
                source: SourceKey {
                    provider: ProviderId::Codex,
                    account: lastdrop_core::model::AccountKey::primary(),
                },
                provider: provider.clone(),
                account: source.clone(),
                manual: false,
                probe: None,
            });
        }
        // Let both workers pick up a job, then quit.
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        scheduler.shutdown();
        assert!(started.elapsed() < SHUTDOWN_WAIT + Duration::from_secs(1));
        assert!(
            calls.load(Ordering::SeqCst) <= WORKERS,
            "only the running fetches ran, not the queue: {}",
            calls.load(Ordering::SeqCst)
        );

        // A fetch longer than the wait is left behind instead of holding up the quit.
        let sink: ResultSink = Arc::new(|_: JobResult| {});
        let scheduler = Scheduler::start(Arc::new(WorkerIo::new()), sink).expect("threads");
        let stuck: Arc<dyn Provider> = Arc::new(SlowProvider {
            delay: SHUTDOWN_WAIT + Duration::from_secs(2),
            calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        scheduler.submit(Job {
            source: SourceKey {
                provider: ProviderId::Codex,
                account: lastdrop_core::model::AccountKey::primary(),
            },
            provider: stuck,
            account: source,
            manual: false,
            probe: None,
        });
        std::thread::sleep(Duration::from_millis(50));
        let started = Instant::now();
        scheduler.shutdown();
        assert!(started.elapsed() < SHUTDOWN_WAIT + Duration::from_secs(1));
    }
}
