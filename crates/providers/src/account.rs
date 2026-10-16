//! Where one account's data and credentials are (ROADMAP "Multiple accounts": a **source** is a
//! config folder; an **account** is who is signed in there).

use std::path::{Path, PathBuf};

use lastdrop_core::model::{AccountKey, ProviderId};
use lastdrop_core::settings::{AccountSettings, SourceSettings};

use crate::error::FetchError;
use crate::io::{CredentialName, Env};

/// Claude Code's state file (cached usage) next to the default folder or inside a custom one.
const CLAUDE_STATE_FILE: &str = ".claude.json";

/// An environment variable as a path, `None` when unset or empty.
fn non_empty_var(env: &dyn Env, name: &str) -> Option<PathBuf> {
    env.var(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// `%APPDATA%` (else the profile's `AppData\Roaming`), or Unavailable.
fn roaming(env: &dyn Env) -> Result<PathBuf, FetchError> {
    non_empty_var(env, "APPDATA")
        .or_else(|| env.home_dir().map(|h| h.join("AppData").join("Roaming")))
        .ok_or_else(|| FetchError::Unavailable {
            detail: "no user profile folder".to_owned(),
        })
}

/// The user's profile folder, or Unavailable.
fn home(env: &dyn Env) -> Result<PathBuf, FetchError> {
    env.home_dir().ok_or_else(|| FetchError::Unavailable {
        detail: "no user profile folder".to_owned(),
    })
}

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
        /// `plan-usage-history.json`. The Store build's private copy of it is found at fetch time
        /// (`claude::desktop::candidates`).
        desktop_dir: Option<PathBuf>,
    },
    /// A Codex account.
    Codex {
        /// The account key from settings.
        account: AccountKey,
        /// `CODEX_HOME`, else `~\.codex`: `auth.json`, `sessions\…\rollout-*.jsonl`.
        codex_home: PathBuf,
    },
    /// A GitHub Copilot account, read with the GitHub CLI's sign-in.
    Copilot {
        /// The account key from settings.
        account: AccountKey,
        /// The GitHub CLI's config folder (`hosts.yml`): `GH_CONFIG_DIR`, else
        /// `%XDG_CONFIG_HOME%\gh`, else `%APPDATA%\GitHub CLI`, as gh looks for it.
        config_dir: PathBuf,
    },
    /// A Meshy account.
    Meshy {
        /// The account key from settings.
        account: AccountKey,
        /// The API key's name in Credential Manager (`Lastdrop:meshy:<account>:api_key`).
        credential: CredentialName,
    },
    /// A Cursor account: the session the Cursor app keeps in its `state.vscdb`.
    Cursor {
        /// The account key from settings.
        account: AccountKey,
        /// `%APPDATA%\Cursor\User\globalStorage\state.vscdb` (read-only; it can be 1.6 GB).
        state_db: PathBuf,
    },
    /// An Antigravity account: the `agy` CLI's own sign-in, which agy keeps and renews itself.
    Antigravity {
        /// The account key from settings.
        account: AccountKey,
    },
    /// An OpenCode Go account: the Go API key stored in Lastdrop, else the one OpenCode keeps.
    OpenCodeGo {
        /// The account key from settings.
        account: AccountKey,
        /// The API key's name in Credential Manager (`Lastdrop:opencode_go:<account>:api_key`).
        credential: CredentialName,
        /// OpenCode's `auth.json` (`~\.local\share\opencode\auth.json`), whose `opencode-go`
        /// entry holds the key when Lastdrop has none (read-only).
        auth_file: PathBuf,
    },
    /// A described service's account (custom-services-spec): its API key.
    Custom {
        /// The account key from settings.
        account: AccountKey,
        /// The service.
        provider: ProviderId,
        /// The API key's name in Credential Manager (`Lastdrop:<id>:<account>:api_key`).
        credential: CredentialName,
    },
    /// A Grok account: the Grok CLI's sign-in.
    Grok {
        /// The account key from settings.
        account: AccountKey,
        /// The Grok CLI's `auth.json`: `GROK_AUTH_PATH`, else `<GROK_HOME>\auth.json`, else
        /// `~\.grok\auth.json` (read-only, never refreshed or rewritten).
        auth_file: PathBuf,
    },
}

impl AccountSource {
    /// Resolves an account's settings (`SourceSettings::Default` = the service's default
    /// location from the environment) into paths and credential names. Does no file I/O;
    /// a missing folder shows up as [`FetchError::Unavailable`] at fetch time.
    ///
    /// - **Claude**, default: `CLAUDE_CONFIG_DIR` (when set and not empty) with
    ///   `<dir>\.claude.json`, else `~\.claude` with `~\.claude.json`; Claude Desktop's folder is
    ///   `%APPDATA%\Claude` (else `~\AppData\Roaming\Claude`). A `ClaudeConfigDir` folder has
    ///   its own `.claude.json` and no Desktop folder: Claude Desktop is one per Windows user and
    ///   belongs to the default account.
    /// - **Codex**: `CODEX_HOME` (when set and not empty), else `~\.codex`; or the
    ///   `CodexHome` folder.
    /// - **Copilot**: gh's config folder: `GH_CONFIG_DIR`, else `%XDG_CONFIG_HOME%\gh`, else
    ///   `%APPDATA%\GitHub CLI` (else `~\AppData\Roaming\GitHub CLI`).
    /// - **Meshy**: the credential `Lastdrop:meshy:<account>:api_key`, or the `ApiKey` name.
    ///
    /// A source kind that does not belong to the provider (a Codex home for Claude) or a missing
    /// home folder is [`FetchError::Unavailable`].
    pub fn resolve(
        provider: ProviderId,
        account: &AccountSettings,
        env: &dyn Env,
    ) -> Result<AccountSource, FetchError> {
        let key = account.key.clone();
        match (provider, &account.source) {
            (ProviderId::Claude, SourceSettings::Default) => {
                let (config_dir, state_file) = match non_empty_var(env, "CLAUDE_CONFIG_DIR") {
                    Some(dir) => {
                        let state_file = dir.join(CLAUDE_STATE_FILE);
                        (dir, state_file)
                    }
                    None => {
                        let home = home(env)?;
                        (home.join(".claude"), home.join(CLAUDE_STATE_FILE))
                    }
                };
                let desktop_dir = non_empty_var(env, "APPDATA")
                    .or_else(|| env.home_dir().map(|h| h.join("AppData").join("Roaming")))
                    .map(|roaming| roaming.join("Claude"));
                Ok(AccountSource::Claude {
                    account: key,
                    config_dir,
                    state_file,
                    desktop_dir,
                })
            }
            (ProviderId::Claude, SourceSettings::ClaudeConfigDir { path }) => {
                Ok(AccountSource::Claude {
                    account: key,
                    config_dir: path.clone(),
                    state_file: path.join(CLAUDE_STATE_FILE),
                    desktop_dir: None,
                })
            }
            (ProviderId::Codex, SourceSettings::Default) => {
                let codex_home = match non_empty_var(env, "CODEX_HOME") {
                    Some(dir) => dir,
                    None => home(env)?.join(".codex"),
                };
                Ok(AccountSource::Codex {
                    account: key,
                    codex_home,
                })
            }
            (ProviderId::Codex, SourceSettings::CodexHome { path }) => Ok(AccountSource::Codex {
                account: key,
                codex_home: path.clone(),
            }),
            (ProviderId::Copilot, SourceSettings::Default) => {
                let config_dir = match non_empty_var(env, crate::copilot::GH_CONFIG_DIR_VAR) {
                    Some(dir) => dir,
                    None => match non_empty_var(env, "XDG_CONFIG_HOME") {
                        Some(xdg) => xdg.join("gh"),
                        None => non_empty_var(env, "APPDATA")
                            .or_else(|| env.home_dir().map(|h| h.join("AppData").join("Roaming")))
                            .ok_or_else(|| FetchError::Unavailable {
                                detail: "no user profile folder".to_owned(),
                            })?
                            .join("GitHub CLI"),
                    },
                };
                Ok(AccountSource::Copilot {
                    account: key,
                    config_dir,
                })
            }
            (ProviderId::Meshy, SourceSettings::Default) => Ok(AccountSource::Meshy {
                credential: CredentialName {
                    provider: ProviderId::Meshy,
                    account: key.clone(),
                    name: crate::meshy::API_KEY_NAME.to_owned(),
                },
                account: key,
            }),
            (ProviderId::Meshy, SourceSettings::ApiKey { name }) if !name.is_empty() => {
                Ok(AccountSource::Meshy {
                    credential: CredentialName {
                        provider: ProviderId::Meshy,
                        account: key.clone(),
                        name: name.clone(),
                    },
                    account: key,
                })
            }
            (ProviderId::Cursor, SourceSettings::Default) => Ok(AccountSource::Cursor {
                account: key,
                state_db: roaming(env)?.join(crate::cursor::STATE_DB),
            }),
            (ProviderId::Antigravity, SourceSettings::Default) => {
                Ok(AccountSource::Antigravity { account: key })
            }
            (ProviderId::OpenCodeGo, SourceSettings::Default | SourceSettings::ApiKey { .. }) => {
                let name = match &account.source {
                    SourceSettings::ApiKey { name } if !name.is_empty() => name.clone(),
                    _ => crate::meshy::API_KEY_NAME.to_owned(),
                };
                Ok(AccountSource::OpenCodeGo {
                    credential: CredentialName {
                        provider: ProviderId::OpenCodeGo,
                        account: key.clone(),
                        name,
                    },
                    account: key,
                    auth_file: home(env)?.join(crate::opencode_go::AUTH_FILE),
                })
            }
            (ProviderId::Custom(_), SourceSettings::Default | SourceSettings::ApiKey { .. }) => {
                let name = match &account.source {
                    SourceSettings::ApiKey { name } if !name.is_empty() => name.clone(),
                    _ => crate::meshy::API_KEY_NAME.to_owned(),
                };
                Ok(AccountSource::Custom {
                    credential: CredentialName {
                        provider,
                        account: key.clone(),
                        name,
                    },
                    provider,
                    account: key,
                })
            }
            (ProviderId::Grok, SourceSettings::Default) => {
                let auth_file = match non_empty_var(env, crate::grok::AUTH_PATH_VAR) {
                    Some(file) => file,
                    None => match non_empty_var(env, crate::grok::HOME_VAR) {
                        Some(dir) => dir.join(crate::grok::AUTH_FILE),
                        None => home(env)?.join(".grok").join(crate::grok::AUTH_FILE),
                    },
                };
                Ok(AccountSource::Grok {
                    account: key,
                    auth_file,
                })
            }
            _ => Err(FetchError::Unavailable {
                detail: format!(
                    "the {} account's source is not a {} source",
                    key.0,
                    provider.display_name()
                ),
            }),
        }
    }

    /// The service.
    pub fn provider(&self) -> ProviderId {
        match self {
            AccountSource::Claude { .. } => ProviderId::Claude,
            AccountSource::Codex { .. } => ProviderId::Codex,
            AccountSource::Copilot { .. } => ProviderId::Copilot,
            AccountSource::Meshy { .. } => ProviderId::Meshy,
            AccountSource::Cursor { .. } => ProviderId::Cursor,
            AccountSource::Antigravity { .. } => ProviderId::Antigravity,
            AccountSource::OpenCodeGo { .. } => ProviderId::OpenCodeGo,
            AccountSource::Grok { .. } => ProviderId::Grok,
            AccountSource::Custom { provider, .. } => *provider,
        }
    }

    /// The config folder of a Claude or Codex source (`config_dir`, `codex_home`); `None` for
    /// Copilot and Meshy, whose sign-in is not a folder of their own.
    pub fn folder(&self) -> Option<&Path> {
        match self {
            AccountSource::Claude { config_dir, .. } => Some(config_dir),
            AccountSource::Codex { codex_home, .. } => Some(codex_home),
            _ => None,
        }
    }

    /// The account key.
    pub fn account(&self) -> &AccountKey {
        match self {
            AccountSource::Claude { account, .. }
            | AccountSource::Codex { account, .. }
            | AccountSource::Copilot { account, .. }
            | AccountSource::Meshy { account, .. }
            | AccountSource::Cursor { account, .. }
            | AccountSource::Antigravity { account }
            | AccountSource::OpenCodeGo { account, .. }
            | AccountSource::Grok { account, .. }
            | AccountSource::Custom { account, .. } => account,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use lastdrop_core::model::{AccountKey, ProviderId};
    use lastdrop_core::settings::{AccountSettings, SourceSettings};

    use super::AccountSource;
    use crate::error::FetchError;
    use crate::fakes::FakeEnv;
    use crate::io::Env;

    fn home() -> PathBuf {
        PathBuf::from(r"C:\Users\tester")
    }

    fn with_source(source: SourceSettings) -> AccountSettings {
        AccountSettings {
            source,
            ..AccountSettings::primary()
        }
    }

    #[test]
    fn claude_defaults_to_the_profile_folder() {
        let env = FakeEnv::new(home()).with_var("APPDATA", r"C:\Users\tester\AppData\Roaming");
        let source = AccountSource::resolve(ProviderId::Claude, &AccountSettings::primary(), &env)
            .expect("resolve");
        assert_eq!(
            source,
            AccountSource::Claude {
                account: AccountKey::primary(),
                config_dir: home().join(".claude"),
                state_file: home().join(".claude.json"),
                desktop_dir: Some(home().join(r"AppData\Roaming\Claude")),
            }
        );
    }

    #[test]
    fn claude_config_dir_moves_the_state_file_inside() {
        let env = FakeEnv::new(home()).with_var("CLAUDE_CONFIG_DIR", r"D:\claude-work");
        let AccountSource::Claude {
            config_dir,
            state_file,
            desktop_dir,
            ..
        } = AccountSource::resolve(ProviderId::Claude, &AccountSettings::primary(), &env)
            .expect("resolve")
        else {
            panic!("not a Claude source");
        };
        assert_eq!(config_dir, PathBuf::from(r"D:\claude-work"));
        assert_eq!(state_file, PathBuf::from(r"D:\claude-work\.claude.json"));
        assert_eq!(
            desktop_dir,
            Some(home().join(r"AppData\Roaming\Claude")),
            "without APPDATA the profile's roaming folder is used"
        );

        let custom = with_source(SourceSettings::ClaudeConfigDir {
            path: PathBuf::from(r"E:\other"),
        });
        let AccountSource::Claude {
            state_file,
            desktop_dir,
            ..
        } = AccountSource::resolve(ProviderId::Claude, &custom, &env).expect("resolve")
        else {
            panic!("not a Claude source");
        };
        assert_eq!(state_file, PathBuf::from(r"E:\other\.claude.json"));
        assert_eq!(desktop_dir, None);
    }

    #[test]
    fn empty_variables_count_as_unset() {
        let env = FakeEnv::new(home())
            .with_var("CLAUDE_CONFIG_DIR", "")
            .with_var("CODEX_HOME", "");
        let claude = AccountSource::resolve(ProviderId::Claude, &AccountSettings::primary(), &env)
            .expect("resolve");
        assert!(matches!(
            claude,
            AccountSource::Claude { ref config_dir, .. } if *config_dir == home().join(".claude")
        ));
        let codex = AccountSource::resolve(ProviderId::Codex, &AccountSettings::primary(), &env)
            .expect("resolve");
        assert_eq!(
            codex,
            AccountSource::Codex {
                account: AccountKey::primary(),
                codex_home: home().join(".codex"),
            }
        );
    }

    #[test]
    fn codex_home_and_meshy_key_names() {
        let env = FakeEnv::new(home()).with_var("CODEX_HOME", r"D:\codex");
        assert_eq!(
            AccountSource::resolve(ProviderId::Codex, &AccountSettings::primary(), &env)
                .expect("resolve"),
            AccountSource::Codex {
                account: AccountKey::primary(),
                codex_home: PathBuf::from(r"D:\codex"),
            }
        );
        let meshy = AccountSource::resolve(ProviderId::Meshy, &AccountSettings::primary(), &env)
            .expect("resolve");
        let AccountSource::Meshy { credential, .. } = &meshy else {
            panic!("not a Meshy source");
        };
        assert_eq!(credential.target(), "Lastdrop:meshy:default:api_key");
        let named = with_source(SourceSettings::ApiKey {
            name: "studio".to_owned(),
        });
        let AccountSource::Meshy { credential, .. } =
            AccountSource::resolve(ProviderId::Meshy, &named, &env).expect("resolve")
        else {
            panic!("not a Meshy source");
        };
        assert_eq!(credential.target(), "Lastdrop:meshy:default:studio");
    }

    #[test]
    fn mismatched_sources_and_a_missing_profile_are_unavailable() {
        let env = FakeEnv::new(home());
        let codex_dir = with_source(SourceSettings::CodexHome {
            path: PathBuf::from(r"D:\codex"),
        });
        assert!(matches!(
            AccountSource::resolve(ProviderId::Claude, &codex_dir, &env),
            Err(FetchError::Unavailable { .. })
        ));
        assert!(matches!(
            AccountSource::resolve(ProviderId::Meshy, &codex_dir, &env),
            Err(FetchError::Unavailable { .. })
        ));
        assert!(matches!(
            AccountSource::resolve(ProviderId::Codex, &AccountSettings::primary(), &NoHome),
            Err(FetchError::Unavailable { .. })
        ));
    }

    /// An environment without a profile folder or variables.
    struct NoHome;

    impl Env for NoHome {
        fn var(&self, _name: &str) -> Option<OsString> {
            None
        }

        fn home_dir(&self) -> Option<PathBuf> {
            None
        }
    }
}
