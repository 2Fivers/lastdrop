//! Claude plans: 5-hour session, weekly all models, weekly per model (R§2 table).
//!
//! Sources, in order (ARCHITECTURE provider table):
//! 1. [`desktop`]: Claude Desktop's `plan-usage-history.json` (no token).
//! 2. [`oauth`]: the OAuth usage endpoint with Claude Code's token — read-only, never refreshed,
//!    at most every 5 minutes per credential, `Retry-After` honoured, stop on 401 until the
//!    token file changes.
//! 3. [`cache`]: Claude Code's cached usage in `.claude.json`.
//!
//! The provider returns the freshest success (by `measured_at`). Each source parses into a
//! [`ClaudeUsage`]; [`to_snapshot`] turns it into meters with the R§2 definitions.
//!
//! **401 from the OAuth source** stops only that source, not the account: the desktop history and
//! the cache need no token and keep being read. The provider remembers, per account, the
//! `.credentials.json` fingerprint that got the 401 (interior `Mutex`, the trait takes `&self`;
//! locked only to read or update that map, never across file or HTTP I/O) and skips the OAuth
//! source until the fingerprint changes. When another source answers, the
//! snapshot carries `Health::SignInRequired` (the service shows STALE(SIGN_IN), R§3.8); when none
//! does, the fetch fails with `FetchError::Auth` and the scheduler stops the whole source until
//! [`Provider::credential_fingerprint`] changes.
#![expect(
    unused_variables,
    reason = "M1 contract stubs: remove once no todo!() is left here"
)]

pub mod cache;
pub mod desktop;
pub mod oauth;

use std::time::Duration;

use jiff::Timestamp;
use lastdrop_core::consts::UNDOCUMENTED_MIN_INTERVAL;
use lastdrop_core::model::{AccountRef, Meter, ProviderId, Snapshot, SourceKind};
use lastdrop_core::refresh::CredentialFingerprint;
use lastdrop_core::settings::ClaudeOptions;

use crate::account::AccountSource;
use crate::error::FetchError;
use crate::io::TerminalCommand;
use crate::{FetchContext, Provider};

/// A Claude usage window as the service names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ClaudeWindow {
    /// The 5-hour session (`five_hour`): `5h` / `5-hour` / `5-hour session`, L 5 h, starts on use.
    FiveHour,
    /// Weekly, all models (`weekly`): `Wk` / `weekly` / `Weekly · all models`, L 7 d.
    Weekly,
    /// Weekly for one model (`weekly:<model>`): `<Model>` / `<Model> weekly` /
    /// `Weekly · <Model>`, L 7 d. The model's display name as the service gives it (`Fable`).
    WeeklyModel(String),
}

/// One window reading from any Claude source.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowReading {
    /// Which window.
    pub window: ClaudeWindow,
    /// Used share (`utilization / 100`), ≥ 0. A window whose utilization is missing is omitted
    /// by the parser, never reported as 0.
    pub used: f64,
    /// `resets_at`; `None` when the service sends null (a 5-hour window between sessions, an
    /// unused per-model weekly).
    pub resets_at: Option<Timestamp>,
}

/// What a Claude source returned.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClaudeUsage {
    /// The windows read.
    pub windows: Vec<WindowReading>,
    /// Plan name when the source says (`Max`, `Pro`).
    pub plan: Option<String>,
    /// When the service computed the values, when the source says.
    pub measured_at: Option<Timestamp>,
}

/// The Claude provider.
#[derive(Clone, Debug)]
pub struct ClaudeProvider {
    options: ClaudeOptions,
}

impl ClaudeProvider {
    /// A provider with the user's source options.
    pub fn new(options: ClaudeOptions) -> ClaudeProvider {
        ClaudeProvider { options }
    }

    /// The source options in use.
    pub fn options(&self) -> &ClaudeOptions {
        &self.options
    }
}

impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Claude
    }

    fn fetch(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Result<Snapshot, FetchError> {
        todo!("M1: desktop history → OAuth usage → .claude.json cache, freshest success")
    }

    /// 5 minutes: the OAuth usage endpoint is undocumented (ARCHITECTURE).
    fn min_interval(&self) -> Duration {
        UNDOCUMENTED_MIN_INTERVAL
    }

    /// Size + mtime of `<config_dir>\.credentials.json`.
    fn credential_fingerprint(
        &self,
        ctx: &FetchContext<'_>,
        account: &AccountSource,
    ) -> Option<CredentialFingerprint> {
        todo!("M1: fingerprint of .credentials.json")
    }

    /// `claude` in a terminal with `CLAUDE_CONFIG_DIR` set to the account's folder (the user
    /// signs in with `/login`).
    fn sign_in_command(&self, account: &AccountSource) -> Option<TerminalCommand> {
        todo!("M1: claude sign-in command")
    }

    // `usage_page` (F§9 "Enter on a card": claude.ai settings → usage): override with the
    // confirmed URL when implementing; the default `None` makes Enter do nothing.
}

/// The meter of a window reading with the R§2 definition (id, names, `L`, `starts_on_use`,
/// order: 5-hour 0, weekly 1, per-model weekly 2… in the service's order). Names and windows of
/// `five_hour` and `weekly` come from `lastdrop_core::model::catalog`, per-model names from
/// `MeterNames::weekly_model`, so the readout's LOADING rows and widths match the readings.
pub fn meter(reading: &WindowReading, order: u16) -> Meter {
    todo!("M1: R§2 Claude meter definitions")
}

/// The snapshot of a usage reading from `source`.
pub fn to_snapshot(
    usage: &ClaudeUsage,
    source: SourceKind,
    account: AccountRef,
    fetched_at: Timestamp,
) -> Snapshot {
    todo!("M1: Claude snapshot")
}
