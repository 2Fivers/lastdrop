//! Codex: 5-hour and weekly windows, whichever the plan has (R§2 table).
//!
//! Sources, in order (ARCHITECTURE provider table):
//! 1. [`app_server`]: `codex app-server`, JSON-RPC over stdio, `account/rateLimits/read` (the
//!    official way; Codex renews its own sign-in). Newest bundled or `PATH` `codex.exe`, no
//!    window, 8 s to initialize, 3 s per call.
//! 2. [`rollout`]: the newest rollout file's last `token_count.rate_limits` event (offline).
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

pub mod app_server;
pub mod rollout;

use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::model::{AccountRef, Meter, ProviderId, Snapshot, SourceKind};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::settings::CodexOptions;

use crate::account::AccountSource;
use crate::error::FetchError;
use crate::io::TerminalCommand;
use crate::{FetchContext, Provider};

/// One Codex rate-limit window as Codex reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct CodexWindow {
    /// `used_percent` (0–100, may exceed 100).
    pub used_percent: f64,
    /// `window_minutes` (300 = 5-hour, 10080 = weekly); `None` when not reported.
    pub window_minutes: Option<i64>,
    /// When it resets; `None` when not reported.
    pub resets_at: Option<Timestamp>,
}

/// What a Codex source returned: the primary and secondary windows, whichever exist.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CodexUsage {
    /// The primary window (usually the 5-hour session).
    pub primary: Option<CodexWindow>,
    /// The secondary window (usually weekly).
    pub secondary: Option<CodexWindow>,
    /// Plan name when the source says (`Plus`, `Pro`).
    pub plan: Option<String>,
    /// When the values were computed (the rollout event's timestamp; `None` from the app-server,
    /// which answers live).
    pub measured_at: Option<Timestamp>,
}

/// The Codex provider.
#[derive(Clone, Debug)]
pub struct CodexProvider {
    options: CodexOptions,
}

impl CodexProvider {
    /// A provider with the user's options (`codex.exe` override, rollout fallback).
    pub fn new(options: CodexOptions) -> CodexProvider {
        CodexProvider { options }
    }

    /// The options in use.
    pub fn options(&self) -> &CodexOptions {
        &self.options
    }
}

/// Lower bound between fetches: 1 minute (**not from the spec**: the app-server is official and
/// local, but each fetch starts a process).
pub const MIN_INTERVAL: Duration = Duration::from_secs(60);

impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Codex
    }

    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        todo!("M1: app-server → newest rollout")
    }

    fn min_interval(&self) -> Duration {
        MIN_INTERVAL
    }

    /// Size + mtime of `<codex_home>\auth.json`.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        todo!("M1: fingerprint of auth.json")
    }

    /// `codex login` in a terminal with `CODEX_HOME` set to the account's folder.
    fn sign_in_command(&self, account: &AccountSource) -> Option<TerminalCommand> {
        todo!("M1: codex login command")
    }
}

/// The meter of a Codex window with the R§2 definition: `window_minutes` 300 → `five_hour`
/// (`5h`, starts on use), 10080 → `weekly` (`Wk`), both with names and windows from
/// `lastdrop_core::model::catalog`; other lengths → a fixed window named by its length; unknown
/// length → primary = 5-hour, secondary = weekly.
pub fn meter(window: &CodexWindow, secondary: bool) -> Meter {
    todo!("M1: R§2 Codex meter definitions")
}

/// The snapshot of a usage reading from `source`.
pub fn to_snapshot(
    usage: &CodexUsage,
    source: SourceKind,
    account: AccountRef,
    fetched_at: Timestamp,
) -> Snapshot {
    todo!("M1: Codex snapshot")
}
