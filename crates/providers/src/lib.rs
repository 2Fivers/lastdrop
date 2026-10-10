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
//! - [`claude`], [`codex`], [`meshy`]: the M1 services, sources per the ARCHITECTURE table.
//!
//! Rules for every provider (ARCHITECTURE "Rules for every provider"):
//! - **Never write, refresh or copy another tool's credentials.** Read, use in memory, drop.
//!   [`io::Fs`] is read-only on purpose.
//! - **Never log a secret** (tokens, keys, cookies, account ids, e-mail addresses). Secrets are
//!   [`SecretString`]s, which print as `[redacted]`.
//! - Undocumented endpoints are polled no more than every 5 minutes per credential, back off on
//!   429/403, and stop on 401 until the credential file changes.
//! - Parsers tolerate unknown and missing fields; a missing value is `None`, never 0.
//! - No network traffic except to the documented endpoints of the service being read; no
//!   telemetry.
//! - Fixtures in tests are invented data, never copied from a real machine
//!   (`tests/fixtures/<provider>/README.md`).
//!
//! Threading: providers run on the app's refresh worker threads, one job per credential source
//! at a time; they never touch UI objects (ARCHITECTURE "Threads").

pub mod account;
pub mod claude;
pub mod codex;
pub mod error;
pub mod fakes;
pub mod io;
pub mod meshy;
pub mod real;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{ProviderId, Snapshot};
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

    /// A fingerprint of the account's credential (token file size + mtime), so the scheduler
    /// can lift an auth stop when it changes (ARCHITECTURE: "stop on 401 until the credential
    /// file changes"). `None` when the provider cannot tell.
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
}

impl FetchContext<'_> {
    /// Time left until the deadline (zero when past), to cap per-call timeouts.
    pub fn remaining(&self) -> Duration {
        let left = self.deadline.duration_since(self.clock.now());
        Duration::try_from(left).unwrap_or(Duration::ZERO)
    }
}

/// The M1 providers configured from settings, in [`ProviderId::ALL`] order. Recreated when the
/// provider options change. `Arc` because the main thread keeps one per source and hands clones
/// to worker jobs (ARCHITECTURE "Threads").
pub fn providers(settings: &Settings) -> Vec<Arc<dyn Provider>> {
    let options = &settings.provider_options;
    vec![
        Arc::new(claude::ClaudeProvider::new(options.claude.clone())),
        Arc::new(codex::CodexProvider::new(options.codex.clone())),
        Arc::new(meshy::MeshyProvider::new()),
    ]
}
