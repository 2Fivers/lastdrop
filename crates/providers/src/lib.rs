//! `lastdrop-providers`: one module per service, with every kind of I/O behind a small trait so
//! tests run on invented fixtures (ARCHITECTURE "Providers").
//!
//! - [`Provider`]: the trait every service implements (ARCHITECTURE, verbatim plus two optional
//!   methods with defaults).
//! - [`FetchContext`]: the clock, HTTP client, file access, secret store, process spawner and
//!   environment a fetch may use — all traits ([`io`]); [`real`] has the production
//!   implementations, [`fakes`] the test ones.
//! - [`AccountSource`]: where one account's data and credentials are.
//! - [`FetchError`]: why a fetch failed, classified for freshness (R§3.8) and back-off.
//! - [`claude`], [`codex`], [`meshy`]: the M1 services, sources per the ARCHITECTURE table;
//!   [`copilot`], [`cursor`], [`antigravity`], [`opencode_go`], [`grok`] (M2).
//! - [`identity`]: who is signed in to a source; [`discover`]: the config folders on this PC
//!   (accounts-spec §2, §6).
//!
//! Rules for every provider (ARCHITECTURE "Rules for every provider"):
//! - **Never write, refresh or copy another tool's credentials.** Read, use in memory, drop.
//!   [`io::Fs`] is read-only on purpose.
//! - **Never log a secret** (tokens, keys, cookies, account ids, e-mail addresses). Secrets are
//!   [`SecretString`]s, which print as `[redacted]`.
//! - Undocumented endpoints are polled no more than every 5 minutes per credential, back off on
//!   429/403 (`Retry-After` honoured up to 24 h), and stop on 401 until the credential file
//!   changes (or appears: a missing file has a fingerprint too, so signing in lifts the stop).
//! - Parsers tolerate unknown and missing fields; a missing value is `None`, never 0.
//! - **Never panic.** Release builds abort on panic (`panic = "abort"`), so no `expect`,
//!   `unwrap`, `unreachable!` or unchecked indexing on anything read or looked up.
//! - No network traffic except to the documented endpoints of the service being read; no
//!   telemetry.
//! - Fixtures in tests are invented data, never copied from a real machine
//!   (`tests/fixtures/<provider>/README.md`).
//!
//! Threading: providers run on the app's refresh worker threads, one job per credential source
//! at a time; they never touch UI objects (ARCHITECTURE "Threads").

pub mod account;
pub mod antigravity;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod custom;
pub mod discover;
pub mod error;
pub mod fakes;
pub mod fingerprint;
pub mod grok;
pub mod identity;
pub mod io;
mod lenient;
pub mod meshy;
pub mod opencode_go;
pub mod real;
pub mod release;
pub mod sqlite;
pub mod tokens;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{Identity, ProviderId, Snapshot};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::settings::Settings;

pub use account::AccountSource;
pub use error::FetchError;
pub use lastdrop_core::secret::SecretString;

/// A service Lastdrop reads (ARCHITECTURE "Providers").
pub trait Provider: Send + Sync {
    /// Which service.
    fn id(&self) -> ProviderId;

    /// One fetch for one account. Tries the provider's sources in their fixed order and returns
    /// the freshest success, or the most telling error. Must finish or fail within
    /// `ctx.deadline`. Every attempt is logged, without secrets.
    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError>;

    /// Lower bound between fetches for one credential source (rate limits, politeness).
    fn min_interval(&self) -> Duration;

    /// The longest the provider lets a live answer stand in for new ones when it paces a source
    /// itself, below the poll interval (Codex's app-server, at most every 15 minutes). The
    /// service's freshness allows for it (`lastdrop_core::stats::stale_after_paced`). `None`: every
    /// fetch reads anew, or the provider's pace is not longer than its `min_interval`.
    fn reading_pace(&self) -> Option<Duration> {
        None
    }

    /// A fingerprint of the account's credential (token file size + mtime,
    /// [`fingerprint::file_fingerprint`]), so the scheduler can lift an auth stop when it changes
    /// (ARCHITECTURE: "stop on 401 until the credential file changes"). A missing file has one
    /// ([`fingerprint::ABSENT`]), so signing in lifts the stop. `None` when the provider cannot
    /// tell: the stop then lasts until a manual refresh.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        let _ = (ctx, account);
        None
    }

    /// The command that opens the vendor's own sign-in for this account in a terminal
    /// (F§4.3 `Open sign-in…`: `claude` / `codex login` with the account's folder set). Lastdrop
    /// never signs in itself. `None` when the service has no such flow (Meshy: the user pastes
    /// an API key in settings; the flyout shows `MessageKind::ApiKey` instead).
    fn sign_in_command(&self, account: &AccountSource) -> Option<io::TerminalCommand> {
        let _ = account;
        None
    }

    /// The service's own usage page, opened in the default browser from a flyout card (F§9
    /// "Enter on a card"). `None` when there is none.
    fn usage_page(&self) -> Option<&'static str> {
        None
    }

    /// Who is signed in to the account's source (accounts-spec §2), from its own files, read-only
    /// ([`identity::read`]). Cheap enough for a refresh worker; the app reads it on a source's
    /// first job, after its credential changed, and at most every 30 minutes otherwise.
    fn identity(&self, ctx: &FetchContext<'_>, account: &AccountSource) -> Identity {
        identity::read(ctx.fs, account)
    }
}

/// The I/O a fetch may use, all injectable (ARCHITECTURE "FetchContext").
#[derive(Clone, Copy)]
pub struct FetchContext<'a> {
    /// The clock.
    pub clock: &'a dyn io::Clock,
    /// HTTP client (TLS via SChannel in production).
    pub http: &'a dyn io::Http,
    /// Read-only file access.
    pub fs: &'a dyn io::Fs,
    /// Lastdrop's own secrets (Credential Manager in production).
    pub secrets: &'a dyn io::Secrets,
    /// Child processes (`codex app-server`).
    pub spawner: &'a dyn io::Spawner,
    /// Environment variables and the home folder.
    pub env: &'a dyn io::Env,
    /// The fetch must finish or fail by this instant (per the clock).
    pub deadline: Timestamp,
    /// The user asked for this fetch (Refresh now, F5, Retry; F§3), so a provider's own pace
    /// may give way (Codex's app-server). Scheduled fetches and probes: `false`.
    pub manual: bool,
}

impl FetchContext<'_> {
    /// Time left until the deadline (zero when past), to cap per-call timeouts.
    pub fn remaining(&self) -> Duration {
        let left = self.deadline.duration_since(self.clock.now());
        Duration::try_from(left).unwrap_or(Duration::ZERO)
    }
}

/// The providers configured from settings, in [`ProviderId::every`] order (the built-in ones,
/// then one per described service). Recreated when the
/// provider options change. `Arc` because the main thread keeps one per source and hands clones
/// to worker jobs (ARCHITECTURE "Threads").
pub fn providers(settings: &Settings) -> Vec<Arc<dyn Provider>> {
    let options = &settings.provider_options;
    let built_in: Vec<Arc<dyn Provider>> = vec![
        Arc::new(claude::ClaudeProvider::new(options.claude.clone())),
        Arc::new(codex::CodexProvider::new(options.codex.clone())),
        Arc::new(copilot::CopilotProvider::new()),
        Arc::new(meshy::MeshyProvider::new()),
        Arc::new(cursor::CursorProvider::new()),
        Arc::new(antigravity::AntigravityProvider::new()),
        Arc::new(opencode_go::OpenCodeGoProvider::new()),
        Arc::new(grok::GrokProvider::new()),
    ];
    built_in
        .into_iter()
        .chain(
            lastdrop_core::custom::all()
                .into_iter()
                .map(|id| Arc::new(custom::CustomProvider::new(id)) as Arc<dyn Provider>),
        )
        .collect()
}
