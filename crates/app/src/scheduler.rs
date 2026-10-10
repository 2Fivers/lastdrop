//! The refresh scheduler (ARCHITECTURE "Refresh scheduler"): a small pool of worker threads that
//! run provider fetches, one job per credential source in flight, with the timing decided by the
//! pure policy in `lastdrop_core::refresh`.
//!
//! The main thread asks [`plan`] which sources are due, submits [`Job`]s, and arms one Slint timer
//! for the next wake-up. Workers run `Provider::fetch` with real I/O ([`WorkerIo`]) and hand each
//! [`JobResult`] to the sink, which forwards it to the main thread with
//! `slint::invoke_from_event_loop`. Workers never touch UI objects.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{Snapshot, SourceKey};
use lastdrop_core::refresh::{Activity, CredentialFingerprint};
use lastdrop_core::secret::SecretString;
use lastdrop_providers::io::{CredentialName, Secrets, SecretsError};
use lastdrop_providers::real::{RealClock, RealEnv, RealFs, RealSpawner, UreqHttp};
use lastdrop_providers::{AccountSource, FetchError, Provider};

use crate::state::AppState;

/// Worker threads: 2 (**not from the spec**: three M1 services, each fetch mostly waits).
pub const WORKERS: usize = 2;

/// The deadline of one fetch (`FetchContext::deadline`): 20 s, enough for the app-server's 8 s
/// start plus a call, and HTTP timeouts.
pub const FETCH_BUDGET: Duration = Duration::from_secs(20);

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

/// The worker pool.
pub struct Scheduler {
    #[expect(dead_code, reason = "used once the pool is implemented (M1)")]
    workers: Vec<JoinHandle<()>>,
}

impl Scheduler {
    /// Starts [`WORKERS`] threads sharing one job queue. Each worker initializes COM (MTA) for
    /// Network List Manager and runs jobs until [`Scheduler::shutdown`].
    pub fn start(io: Arc<WorkerIo>, sink: ResultSink) -> std::io::Result<Scheduler> {
        todo!("M1: start workers")
    }

    /// Queues a job. The caller ensures one job per source in flight (`SourceState::in_flight`).
    pub fn submit(&self, job: Job) {
        todo!("M1: queue a job")
    }

    /// Closes the queue and joins the workers (running fetches finish within their deadline).
    pub fn shutdown(self) {
        todo!("M1: stop workers")
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
    todo!("M1: plan refreshes")
}
