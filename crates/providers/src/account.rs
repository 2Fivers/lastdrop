//! Where one account's data and credentials are (ROADMAP "Multiple accounts": a **source** is a
//! config folder; an **account** is who is signed in there).

use std::path::PathBuf;

use lastdrop_core::model::{AccountKey, ProviderId};
use lastdrop_core::settings::AccountSettings;

use crate::error::FetchError;
use crate::io::{CredentialName, Env};

/// A resolved account source, built from settings and the environment by
/// [`AccountSource::resolve`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountSource {
    /// A Claude account.
    Claude {
        /// The account key from settings.
        account: AccountKey,
        /// Claude Code's config folder: `CLAUDE_CONFIG_DIR`, else `~\.claude`. Holds
        /// `.credentials.json` (the OAuth token, read-only).
        config_dir: PathBuf,
        /// Claude Code's `.claude.json` (cached usage): `~\.claude.json` with the default
        /// folder, `<config_dir>\.claude.json` with `CLAUDE_CONFIG_DIR`.
        state_file: PathBuf,
        /// Claude Desktop's data folder (`%APPDATA%\Claude`), when it exists; holds
        /// `plan-usage-history.json`.
        desktop_dir: Option<PathBuf>,
    },
    /// A Codex account.
    Codex {
        /// The account key from settings.
        account: AccountKey,
        /// `CODEX_HOME`, else `~\.codex`: `auth.json`, `sessions\…\rollout-*.jsonl`.
        codex_home: PathBuf,
    },
    /// A Meshy account.
    Meshy {
        /// The account key from settings.
        account: AccountKey,
        /// The API key's name in Credential Manager (`Lastdrop:meshy:<account>:api_key`).
        credential: CredentialName,
    },
}

impl AccountSource {
    /// Resolves an account's settings (`SourceSettings::Default` = the service's default
    /// location from the environment) into paths and credential names. Does no file I/O;
    /// a missing folder shows up as [`FetchError::Unavailable`] at fetch time.
    #[expect(unused_variables, reason = "M1 contract stub")]
    pub fn resolve(
        provider: ProviderId,
        account: &AccountSettings,
        env: &dyn Env,
    ) -> Result<AccountSource, FetchError> {
        todo!("M1: resolve CLAUDE_CONFIG_DIR / CODEX_HOME / credential names")
    }

    /// The service.
    pub fn provider(&self) -> ProviderId {
        match self {
            AccountSource::Claude { .. } => ProviderId::Claude,
            AccountSource::Codex { .. } => ProviderId::Codex,
            AccountSource::Meshy { .. } => ProviderId::Meshy,
        }
    }

    /// The account key.
    pub fn account(&self) -> &AccountKey {
        match self {
            AccountSource::Claude { account, .. }
            | AccountSource::Codex { account, .. }
            | AccountSource::Meshy { account, .. } => account,
        }
    }
}
