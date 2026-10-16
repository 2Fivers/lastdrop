//! The config folders of Claude Code and Codex on this PC (accounts-spec §6), for the settings
//! window's found folders and the first run. Read-only and cheap: one listing of the profile
//! folder and a few `metadata` calls; no file is opened.

use std::path::{Path, PathBuf};

use lastdrop_core::model::ProviderId;
use lastdrop_core::settings::{AccountSettings, SourceSettings};

use crate::AccountSource;
use crate::io::{Env, Fs};

/// One folder that holds a service's data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The folder.
    pub folder: PathBuf,
    /// What an account for it has as its source: `Default` for the service's default folder,
    /// else a folder source (`ClaudeConfigDir`, `CodexHome`).
    pub source: SourceSettings,
}

/// The folder-name prefixes of extra config folders in the profile folder.
fn prefixes(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Claude => &[".claude-", ".claude_"],
        ProviderId::Codex => &[".codex-", ".codex_"],
        ProviderId::Copilot | ProviderId::Meshy => &[],
    }
}

/// What in a folder shows that the service uses it: one of these, a file or a folder.
fn markers(provider: ProviderId) -> &'static [&'static str] {
    match provider {
        ProviderId::Claude => &[
            ".credentials.json",
            ".claude.json",
            "settings.json",
            "projects",
        ],
        ProviderId::Codex => &["auth.json", "config.toml", "sessions"],
        ProviderId::Copilot | ProviderId::Meshy => &[],
    }
}

/// Whether `folder` holds one of the service's [`markers`].
pub fn holds_data(provider: ProviderId, folder: &Path, fs: &dyn Fs) -> bool {
    markers(provider)
        .iter()
        .any(|m| fs.metadata(&folder.join(m)).is_ok())
}

/// The service's default folder (`CLAUDE_CONFIG_DIR` or `~\.claude`; `CODEX_HOME` or
/// `~\.codex`); `None` for services without folders or without a profile folder.
pub fn default_folder(provider: ProviderId, env: &dyn Env) -> Option<PathBuf> {
    AccountSource::resolve(provider, &AccountSettings::primary(), env)
        .ok()
        .and_then(|s| s.folder().map(Path::to_path_buf))
}

/// Whether two paths name the same folder, as Windows compares them: case-insensitive, trailing
/// separators and `/` against `\` ignored.
pub fn same_folder(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// The service's folders on this PC (accounts-spec §6): the default folder when it holds data,
/// then every `~\.claude-*` / `~\.claude_*` (`~\.codex-*` / `~\.codex_*`) folder that holds data,
/// sorted by path. Empty for services without folders.
pub fn candidates(provider: ProviderId, env: &dyn Env, fs: &dyn Fs) -> Vec<Candidate> {
    let mut out = Vec::new();
    let default = default_folder(provider, env);
    if let Some(folder) = &default
        && holds_data(provider, folder, fs)
    {
        out.push(Candidate {
            folder: folder.clone(),
            source: SourceSettings::Default,
        });
    }
    let Some(home) = env.home_dir() else {
        return out;
    };
    let Ok(entries) = fs.list(&home) else {
        return out;
    };
    for entry in entries {
        if !entry.meta.is_dir {
            continue;
        }
        let Some(name) = entry
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
        else {
            continue;
        };
        let prefixed = prefixes(provider)
            .iter()
            .any(|p| name.len() > p.len() && name.starts_with(p));
        if !prefixed
            || default
                .as_deref()
                .is_some_and(|d| same_folder(d, &entry.path))
            || !holds_data(provider, &entry.path, fs)
        {
            continue;
        }
        if let Some(source) = SourceSettings::folder_of(provider, entry.path.clone()) {
            out.push(Candidate {
                folder: entry.path,
                source,
            });
        }
    }
    out
}

/// The folder an account's settings point at, resolved like its source (the default folder for
/// `Default`); `None` for services without folders.
pub fn folder_of(
    provider: ProviderId,
    account: &AccountSettings,
    env: &dyn Env,
) -> Option<PathBuf> {
    AccountSource::resolve(provider, account, env)
        .ok()
        .and_then(|s| s.folder().map(Path::to_path_buf))
}

/// The candidates that no account of `accounts` uses already (compared with [`same_folder`]).
pub fn unused(
    provider: ProviderId,
    candidates: Vec<Candidate>,
    accounts: &[AccountSettings],
    env: &dyn Env,
) -> Vec<Candidate> {
    let used: Vec<PathBuf> = accounts
        .iter()
        .filter_map(|a| folder_of(provider, a, env))
        .collect();
    candidates
        .into_iter()
        .filter(|c| !used.iter().any(|u| same_folder(u, &c.folder)))
        .collect()
}
