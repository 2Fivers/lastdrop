//! The settings window's **Accounts** section (accounts-spec §6–§7): what it lists and the edits
//! it makes to the draft. Pure functions over [`AccountsInfo`] — who is signed in to each listed
//! folder and the folders found on this PC — which [`AccountsInfo::read`] gathers when the window
//! opens (the dialog process may wait on the disk, ARCHITECTURE "Processes").
//!
//! E-mail addresses are read into [`Identity`] values (secrets) and shown only as accounts-spec
//! §4 says; nothing here logs or writes them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use lastdrop_core::grammar::strings::text;
use lastdrop_core::grammar::{Msg, Strings};
use lastdrop_core::model::{AccountKey, Identity, ProviderId, SourceKey};
use lastdrop_core::settings::{AccountSettings, ReadoutContent, Settings, SourceSettings};
use lastdrop_providers::discover::{self, Candidate};
use lastdrop_providers::io::{Env, Fs};
use lastdrop_providers::{AccountSource, identity};
use slint::{ModelRc, SharedString, VecModel};

use crate::ui;

/// The services whose accounts the window manages, in this order (accounts-spec §7).
pub const SERVICES: [ProviderId; 2] = [ProviderId::Claude, ProviderId::Codex];

/// A folder as Windows compares it: lower case, `\` separators, no trailing separator.
pub fn folder_id(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// The answer for a folder nobody read.
static UNKNOWN: Identity = Identity::Unknown;

/// What the section knows beyond the draft.
#[derive(Clone, Debug, Default)]
pub struct AccountsInfo {
    /// Who is signed in, by [`folder_id`].
    pub identities: BTreeMap<String, Identity>,
    /// Each service's folders on this PC (`discover::candidates`).
    pub found: BTreeMap<ProviderId, Vec<Candidate>>,
    /// Each service's default folder, which a `Default` source means.
    pub defaults: BTreeMap<ProviderId, PathBuf>,
    /// The profile folder, where new accounts' folders go.
    pub home: Option<PathBuf>,
}

impl AccountsInfo {
    /// Reads it on this PC: the default folders, the folders found, and who is signed in to each
    /// account's folder and each found one.
    pub fn read(settings: &Settings) -> AccountsInfo {
        AccountsInfo::read_with(
            settings,
            &lastdrop_providers::real::RealEnv,
            &lastdrop_providers::real::RealFs,
        )
    }

    /// [`AccountsInfo::read`] with the given environment and files (tests).
    pub fn read_with(settings: &Settings, env: &dyn Env, fs: &dyn Fs) -> AccountsInfo {
        let mut info = AccountsInfo {
            home: env.home_dir(),
            ..AccountsInfo::default()
        };
        for provider in SERVICES {
            if let Some(folder) = discover::default_folder(provider, env) {
                info.defaults.insert(provider, folder);
            }
            let found = discover::candidates(provider, env, fs);
            for candidate in &found {
                let account = AccountSettings {
                    source: candidate.source.clone(),
                    ..AccountSettings::primary()
                };
                info.learn(provider, &account, env, fs);
            }
            info.found.insert(provider, found);
            for account in settings.provider(provider).map_or(&[][..], |p| &p.accounts) {
                info.learn(provider, account, env, fs);
            }
        }
        info
    }

    /// Reads who is signed in to one more account's folder (a folder just added), unless it was
    /// read already.
    pub fn learn(
        &mut self,
        provider: ProviderId,
        account: &AccountSettings,
        env: &dyn Env,
        fs: &dyn Fs,
    ) {
        let Ok(source) = AccountSource::resolve(provider, account, env) else {
            return;
        };
        let Some(folder) = source.folder() else {
            return;
        };
        self.identities
            .entry(folder_id(folder))
            .or_insert_with(|| identity::read(fs, &source));
    }

    /// The folder an account's source names: the default folder for `Default`.
    pub fn folder_of(&self, provider: ProviderId, account: &AccountSettings) -> Option<PathBuf> {
        match &account.source {
            SourceSettings::Default => self.defaults.get(&provider).cloned(),
            other => other.folder().map(Path::to_path_buf),
        }
    }

    /// Who is signed in to `folder`, as read; `Unknown` when it was not read.
    pub fn identity(&self, folder: &Path) -> &Identity {
        self.identities.get(&folder_id(folder)).unwrap_or(&UNKNOWN)
    }

    /// Who is signed in to an account's folder.
    fn identity_of(&self, provider: ProviderId, account: &AccountSettings) -> &Identity {
        self.folder_of(provider, account)
            .map_or(&UNKNOWN, |folder| self.identity(&folder))
    }
}

/// A service's accounts in the draft.
fn accounts_of(settings: &Settings, provider: ProviderId) -> &[AccountSettings] {
    settings.provider(provider).map_or(&[][..], |p| &p.accounts)
}

/// The earlier account of the service that `index` is signed in to as well (accounts-spec §3):
/// the first one with the same known identity. `None` when it is the first, or nobody is known
/// to be signed in.
pub fn duplicate_of(
    settings: &Settings,
    info: &AccountsInfo,
    provider: ProviderId,
    index: usize,
) -> Option<usize> {
    let accounts = accounts_of(settings, provider);
    let id = info.identity_of(provider, accounts.get(index)?).id()?;
    accounts[..index]
        .iter()
        .position(|a| info.identity_of(provider, a).id() == Some(id))
}

/// The accounts the tray shows, in settings order: every one that is not a duplicate.
fn shown(settings: &Settings, info: &AccountsInfo, provider: ProviderId) -> Vec<usize> {
    (0..accounts_of(settings, provider).len())
        .filter(|i| duplicate_of(settings, info, provider, *i).is_none())
        .collect()
}

/// An account's name in the section and in the meter list (accounts-spec §4, §7): its alias,
/// else its e-mail address (unless personal info is hidden), else `Account {n}`, `n` its place
/// among the accounts the tray shows (duplicates are numbered as the account they duplicate), as
/// the tray numbers them.
pub fn name_of(
    settings: &Settings,
    info: &AccountsInfo,
    provider: ProviderId,
    index: usize,
    strings: &dyn Strings,
) -> String {
    let Some(account) = accounts_of(settings, provider).get(index) else {
        return String::new();
    };
    if let Some(alias) = account.alias.as_deref().filter(|a| !a.trim().is_empty()) {
        return alias.to_owned();
    }
    if !settings.hide_personal_info
        && let Some(email) = info.identity_of(provider, account).email()
    {
        return email.expose().to_owned();
    }
    let first = duplicate_of(settings, info, provider, index).unwrap_or(index);
    let n = shown(settings, info, provider)
        .iter()
        .position(|i| *i == first)
        .map_or(index + 1, |p| p + 1);
    text(strings, Msg::AccountN, &[("n", &n.to_string())])
}

/// The line under an account (accounts-spec §7): `Same account as {name}` when an earlier account
/// of the service is signed in to the same account, else who is signed in.
pub fn status_of(
    settings: &Settings,
    info: &AccountsInfo,
    provider: ProviderId,
    index: usize,
    strings: &dyn Strings,
) -> String {
    let accounts = accounts_of(settings, provider);
    let Some(account) = accounts.get(index) else {
        return String::new();
    };
    let identity = info.identity_of(provider, account);
    if let Some(first) = duplicate_of(settings, info, provider, index) {
        let name = name_of(settings, info, provider, first, strings);
        return text(strings, Msg::SettingsAccountsSame, &[("name", &name)]);
    }
    // An account named by its address does not say it twice.
    let named_by_address = account.alias.as_deref().is_none_or(|a| a.trim().is_empty());
    identity_line(
        identity,
        settings.hide_personal_info || named_by_address,
        strings,
    )
}

/// Who is signed in, as one line (accounts-spec §7).
fn identity_line(identity: &Identity, hide: bool, strings: &dyn Strings) -> String {
    match identity {
        Identity::SignedIn { email, .. } => match email.as_ref().filter(|_| !hide) {
            Some(email) => text(
                strings,
                Msg::SettingsAccountsSignedIn,
                &[("email", email.expose())],
            ),
            None => strings
                .template(Msg::SettingsAccountsSignedInHidden)
                .to_owned(),
        },
        Identity::SignedOut => strings.template(Msg::SettingsAccountsSignedOut).to_owned(),
        Identity::Unknown => strings.template(Msg::SettingsAccountsUnknown).to_owned(),
    }
}

/// How the meter list names and leaves out accounts (accounts-spec §7), as the tray does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MeterAccounts {
    /// For each service the tray shows two or more accounts of, every shown account's
    /// [`name_of`] (`Claude · Work weekly`).
    pub names: BTreeMap<SourceKey, String>,
    /// The duplicates: never evaluated, so their meters are not listed.
    pub duplicates: BTreeSet<SourceKey>,
}

/// [`MeterAccounts`] of the draft.
pub fn meter_accounts(
    settings: &Settings,
    info: &AccountsInfo,
    strings: &dyn Strings,
) -> MeterAccounts {
    let mut out = MeterAccounts::default();
    for provider in SERVICES {
        let accounts = accounts_of(settings, provider);
        let shown = shown(settings, info, provider);
        for (i, account) in accounts.iter().enumerate() {
            let key = SourceKey {
                provider,
                account: account.key.clone(),
            };
            if !shown.contains(&i) {
                out.duplicates.insert(key);
            } else if shown.len() >= 2 {
                out.names
                    .insert(key, name_of(settings, info, provider, i, strings));
            }
        }
    }
    out
}

/// The folders found on this PC that no account of the service uses (accounts-spec §6).
pub fn unused(settings: &Settings, info: &AccountsInfo, provider: ProviderId) -> Vec<Candidate> {
    let used: Vec<String> = accounts_of(settings, provider)
        .iter()
        .filter_map(|a| info.folder_of(provider, a))
        .map(|f| folder_id(&f))
        .collect();
    info.found
        .get(&provider)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|c| !used.contains(&folder_id(&c.folder)))
        .cloned()
        .collect()
}

/// The section's blocks for the window: one per service the settings have.
pub fn blocks(
    settings: &Settings,
    info: &AccountsInfo,
    strings: &dyn Strings,
) -> Vec<ui::AccountBlock> {
    SERVICES
        .iter()
        .filter(|p| settings.provider(**p).is_some())
        .map(|&provider| {
            let accounts = accounts_of(settings, provider);
            let rows: Vec<ui::AccountRow> = accounts
                .iter()
                .enumerate()
                .map(|(i, account)| ui::AccountRow {
                    name: name_of(settings, info, provider, i, strings).into(),
                    folder: info
                        .folder_of(provider, account)
                        .map(|f| f.display().to_string())
                        .unwrap_or_default()
                        .into(),
                    status: status_of(settings, info, provider, i, strings).into(),
                    alias: account.alias.clone().unwrap_or_default().into(),
                    removable: accounts.len() > 1,
                })
                .collect();
            let found: Vec<ui::FoundFolder> = unused(settings, info, provider)
                .iter()
                .map(|c| ui::FoundFolder {
                    folder: c.folder.display().to_string().into(),
                    status: identity_line(
                        info.identity(&c.folder),
                        settings.hide_personal_info,
                        strings,
                    )
                    .into(),
                })
                .collect();
            ui::AccountBlock {
                title: SharedString::from(
                    provider.client_name().unwrap_or(provider.display_name()),
                ),
                accounts: ModelRc::new(VecModel::from(rows)),
                found: ModelRc::new(VecModel::from(found)),
            }
        })
        .collect()
}

/// Why a folder was not added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotAdded {
    /// An account of the service uses it already.
    AlreadyUsed,
    /// The settings have no entry for the service.
    NoService,
}

/// Adds `folder` as an account of `provider` with `alias` (accounts-spec §7): the service's
/// default folder becomes the `Default` source (keyed `default` when that key is free), any other
/// a folder source keyed by [`AccountKey::for_folder`]. Returns the new account's key.
pub fn add(
    settings: &mut Settings,
    info: &AccountsInfo,
    provider: ProviderId,
    folder: &Path,
    alias: Option<&str>,
) -> Result<AccountKey, NotAdded> {
    let id = folder_id(folder);
    let used = accounts_of(settings, provider)
        .iter()
        .filter_map(|a| info.folder_of(provider, a))
        .any(|f| folder_id(&f) == id);
    if used {
        return Err(NotAdded::AlreadyUsed);
    }
    let is_default = info
        .defaults
        .get(&provider)
        .is_some_and(|d| folder_id(d) == id);
    let Some(p) = settings.provider_mut(provider) else {
        return Err(NotAdded::NoService);
    };
    let taken: Vec<AccountKey> = p.accounts.iter().map(|a| a.key.clone()).collect();
    let account = if is_default && !taken.contains(&AccountKey::primary()) {
        AccountSettings {
            alias: alias.and_then(lastdrop_core::settings::clean_alias),
            ..AccountSettings::primary()
        }
    } else {
        let source = if is_default {
            SourceSettings::Default
        } else {
            match SourceSettings::folder_of(provider, folder.to_path_buf()) {
                Some(source) => source,
                None => return Err(NotAdded::NoService),
            }
        };
        let mut account = AccountSettings::for_folder(source, alias, &taken);
        if is_default {
            // `for_folder` hashes the folder of a folder source; the default one has none.
            account.key = AccountKey::for_folder(&folder.to_string_lossy(), &taken);
        }
        account
    };
    let key = account.key.clone();
    p.accounts.push(account);
    Ok(key)
}

/// Removes the account at `index` of `provider`, unless it is the service's last one (turn the
/// service off instead), and the readout pins of its meters. Returns whether it was removed.
pub fn remove(settings: &mut Settings, provider: ProviderId, index: usize) -> bool {
    let Some(p) = settings.provider_mut(provider) else {
        return false;
    };
    if p.accounts.len() < 2 || index >= p.accounts.len() {
        return false;
    }
    let removed = p.accounts.remove(index);
    if let ReadoutContent::Pinned(pins) = &mut settings.readout.content {
        pins.retain(|k| !(k.provider == provider && k.account == removed.key));
        if pins.is_empty() {
            settings.readout.content = ReadoutContent::Auto;
        }
    }
    true
}

/// A name as a folder suffix (accounts-spec §7): lower case, every character outside
/// `a-z 0-9 - _` replaced by `-`, runs of `-` as one, none at the ends; `account` when nothing is
/// left.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars().flat_map(char::to_lowercase) {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            c
        } else {
            '-'
        };
        if c == '-' && (out.is_empty() || out.ends_with('-')) {
            continue;
        }
        out.push(c);
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() {
        "account".to_owned()
    } else {
        out
    }
}

/// The folder for a new account named `name` (accounts-spec §7): `~\.claude-{slug}` or
/// `~\.codex-{slug}`, then `-2`, `-3` … while `exists` says the folder is there. `None` for
/// services without folders.
pub fn new_folder(
    home: &Path,
    provider: ProviderId,
    name: &str,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let prefix = match provider {
        ProviderId::Claude => ".claude-",
        ProviderId::Codex => ".codex-",
        _ => return None,
    };
    let base = format!("{prefix}{}", slug(name));
    let mut folder = home.join(&base);
    let mut n = 2;
    while exists(&folder) {
        folder = home.join(format!("{base}-{n}"));
        n += 1;
    }
    Some(folder)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::Timestamp;
    use lastdrop_core::grammar::{English, Swedish};
    use lastdrop_core::model::{AccountIdentity, MeterId, MeterKey};
    use lastdrop_core::secret::SecretString;
    use lastdrop_providers::fakes::{FakeEnv, FakeFs};

    fn home() -> PathBuf {
        PathBuf::from(r"C:\Users\tester")
    }

    fn now() -> Timestamp {
        "2026-10-10T10:42:00Z".parse().expect("valid timestamp")
    }

    fn signed_in(name: &str, email: Option<&str>) -> Identity {
        Identity::SignedIn {
            id: AccountIdentity::of(&["claude", "org", name]),
            email: email.map(|e| SecretString::new(e.to_owned())),
        }
    }

    /// Claude with the default folder and `~\.claude-work`, who is signed in to each, and an
    /// unused `~\.claude-old` found on the PC.
    fn two_claude(first: Identity, second: Identity) -> (Settings, AccountsInfo) {
        let mut settings = Settings::default();
        let work = SourceSettings::ClaudeConfigDir {
            path: home().join(".claude-work"),
        };
        if let Some(p) = settings.provider_mut(ProviderId::Claude) {
            let taken: Vec<AccountKey> = p.accounts.iter().map(|a| a.key.clone()).collect();
            p.accounts
                .push(AccountSettings::for_folder(work.clone(), None, &taken));
        }
        let mut info = AccountsInfo {
            home: Some(home()),
            ..AccountsInfo::default()
        };
        info.defaults
            .insert(ProviderId::Claude, home().join(".claude"));
        info.identities
            .insert(folder_id(&home().join(".claude")), first);
        info.identities
            .insert(folder_id(&home().join(".claude-work")), second);
        info.identities
            .insert(folder_id(&home().join(".claude-old")), Identity::SignedOut);
        info.found.insert(
            ProviderId::Claude,
            vec![
                Candidate {
                    folder: home().join(".claude"),
                    source: SourceSettings::Default,
                },
                Candidate {
                    folder: home().join(".Claude-Work"),
                    source: work,
                },
                Candidate {
                    folder: home().join(".claude-old"),
                    source: SourceSettings::ClaudeConfigDir {
                        path: home().join(".claude-old"),
                    },
                },
            ],
        );
        (settings, info)
    }

    #[test]
    fn accounts_are_named_and_say_who_is_signed_in() {
        let (mut settings, info) = two_claude(
            signed_in("one", Some("one@example.invalid")),
            Identity::Unknown,
        );
        let name = |s: &Settings, i| name_of(s, &info, ProviderId::Claude, i, &English);
        let status = |s: &Settings, i| status_of(s, &info, ProviderId::Claude, i, &English);
        assert_eq!(name(&settings, 0), "one@example.invalid");
        assert_eq!(
            status(&settings, 0),
            "Signed in",
            "named by its address already"
        );
        assert_eq!(name(&settings, 1), "Account 2");
        assert_eq!(status(&settings, 1), "Can't tell who is signed in");

        if let Some(p) = settings.provider_mut(ProviderId::Claude) {
            p.accounts[0].alias = Some("Work".to_owned());
        }
        assert_eq!(status(&settings, 0), "Signed in as one@example.invalid");
        settings.hide_personal_info = true;
        assert_eq!(name(&settings, 0), "Work");
        assert_eq!(status(&settings, 0), "Signed in", "the address is hidden");
        assert_eq!(
            status_of(&settings, &info, ProviderId::Claude, 0, &Swedish),
            "Inloggad"
        );

        let accounts = meter_accounts(&settings, &info, &English);
        assert_eq!(accounts.names.len(), 2);
        assert!(accounts.names.values().any(|l| l == "Work"));
        assert!(accounts.duplicates.is_empty());
    }

    #[test]
    fn duplicates_are_numbered_and_left_out_as_the_tray_does() {
        // Default and `.claude-work` on the same account, a third folder on another one.
        let same = signed_in("one", None);
        let (mut settings, mut info) = two_claude(same.clone(), same);
        settings.hide_personal_info = true;
        let third = home().join(".claude-third");
        info.identities
            .insert(folder_id(&third), signed_in("two", None));
        let third_key = add(&mut settings, &info, ProviderId::Claude, &third, None).expect("added");
        let name = |i| name_of(&settings, &info, ProviderId::Claude, i, &English);
        assert_eq!(name(0), "Account 1");
        assert_eq!(name(1), "Account 1", "the same account, the same number");
        assert_eq!(
            name(2),
            "Account 2",
            "as the tray and the readout tag number it"
        );
        assert_eq!(
            duplicate_of(&settings, &info, ProviderId::Claude, 1),
            Some(0)
        );
        assert_eq!(duplicate_of(&settings, &info, ProviderId::Claude, 2), None);

        let accounts = meter_accounts(&settings, &info, &English);
        let work = accounts_of(&settings, ProviderId::Claude)[1].key.clone();
        assert!(accounts.duplicates.contains(&SourceKey {
            provider: ProviderId::Claude,
            account: work,
        }));
        assert_eq!(
            accounts.names.get(&SourceKey {
                provider: ProviderId::Claude,
                account: third_key,
            }),
            Some(&"Account 2".to_owned())
        );

        // Two folders on one account: the tray shows one, so the meter list names none.
        let same = signed_in("one", None);
        let (settings, info) = two_claude(same.clone(), same);
        let accounts = meter_accounts(&settings, &info, &English);
        assert!(accounts.names.is_empty());
        assert_eq!(accounts.duplicates.len(), 1);
    }

    #[test]
    fn a_second_folder_of_the_same_account_says_so() {
        let same = signed_in("one", Some("one@example.invalid"));
        let (settings, info) = two_claude(same.clone(), same);
        assert_eq!(
            status_of(&settings, &info, ProviderId::Claude, 1, &English),
            "Same account as one@example.invalid, shown once"
        );
        let (settings, info) = two_claude(Identity::SignedOut, Identity::SignedOut);
        assert_eq!(
            status_of(&settings, &info, ProviderId::Claude, 1, &English),
            "Not signed in",
            "nobody signed in is never the same account"
        );
    }

    #[test]
    fn found_folders_leave_out_the_ones_in_use() {
        let (settings, info) = two_claude(Identity::SignedOut, Identity::SignedOut);
        let unused = unused(&settings, &info, ProviderId::Claude);
        let folders: Vec<&PathBuf> = unused.iter().map(|c| &c.folder).collect();
        assert_eq!(
            folders,
            [&home().join(".claude-old")],
            "case does not matter"
        );
    }

    #[test]
    fn folders_are_added_once_and_the_default_one_as_the_default_source() {
        let (mut settings, info) = two_claude(Identity::SignedOut, Identity::SignedOut);
        assert_eq!(
            add(
                &mut settings,
                &info,
                ProviderId::Claude,
                &home().join(r".CLAUDE-work\"),
                None
            ),
            Err(NotAdded::AlreadyUsed)
        );
        let key = add(
            &mut settings,
            &info,
            ProviderId::Claude,
            &home().join(".claude-old"),
            Some(" Old "),
        )
        .expect("added");
        let accounts = accounts_of(&settings, ProviderId::Claude);
        let added = accounts.last().expect("added");
        assert_eq!(added.key, key);
        assert_eq!(added.alias.as_deref(), Some("Old"));
        assert_eq!(
            added.source,
            SourceSettings::ClaudeConfigDir {
                path: home().join(".claude-old"),
            }
        );

        // The default folder, removed and added again, is the default source once more.
        assert!(remove(&mut settings, ProviderId::Claude, 0));
        let key = add(
            &mut settings,
            &info,
            ProviderId::Claude,
            &home().join(".claude"),
            None,
        )
        .expect("added");
        assert_eq!(key, AccountKey::primary());
        let accounts = accounts_of(&settings, ProviderId::Claude);
        assert_eq!(
            accounts.last().map(|a| &a.source),
            Some(&SourceSettings::Default)
        );
        assert_eq!(
            add(
                &mut settings,
                &info,
                ProviderId::Meshy,
                &home().join("x"),
                None
            ),
            Err(NotAdded::NoService),
            "Meshy has no folders"
        );
    }

    #[test]
    fn removing_keeps_the_last_account_and_drops_its_pins() {
        let (mut settings, _) = two_claude(Identity::SignedOut, Identity::SignedOut);
        let work = accounts_of(&settings, ProviderId::Claude)[1].key.clone();
        let pin = |account: AccountKey| MeterKey {
            provider: ProviderId::Claude,
            account,
            meter: MeterId::new(MeterId::WEEKLY),
        };
        settings.readout.content =
            ReadoutContent::Pinned(vec![pin(work), pin(AccountKey::primary())]);
        assert!(remove(&mut settings, ProviderId::Claude, 1));
        assert_eq!(
            settings.readout.content,
            ReadoutContent::Pinned(vec![pin(AccountKey::primary())])
        );
        assert!(
            !remove(&mut settings, ProviderId::Claude, 0),
            "the last one stays"
        );
        assert!(!remove(&mut settings, ProviderId::Claude, 5));
    }

    #[test]
    fn new_accounts_get_a_folder_of_their_own() {
        assert_eq!(slug("Work"), "work");
        assert_eq!(slug("  Min Jobb!  "), "min-jobb");
        assert_eq!(slug("Ålands konto"), "lands-konto");
        assert_eq!(slug("a__b--c"), "a__b-c");
        assert_eq!(slug("***"), "account");
        let taken = [home().join(".claude-work"), home().join(".claude-work-2")];
        let exists = |p: &Path| taken.iter().any(|t| t == p);
        assert_eq!(
            new_folder(&home(), ProviderId::Claude, "Work", &exists),
            Some(home().join(".claude-work-3"))
        );
        assert_eq!(
            new_folder(&home(), ProviderId::Codex, "Work", &exists),
            Some(home().join(".codex-work"))
        );
        assert_eq!(
            new_folder(&home(), ProviderId::Meshy, "Work", &exists),
            None
        );
    }

    #[test]
    fn reading_finds_folders_and_who_is_signed_in() {
        let fs = FakeFs::new();
        let env = FakeEnv::new(home());
        fs.add_file(
            home().join(".claude.json"),
            r#"{ "oauthAccount": { "accountUuid": "00000000-0000-4000-8000-000000000002" } }"#,
            now(),
        );
        fs.add_file(home().join(r".claude\settings.json"), "{}", now());
        fs.add_file(home().join(r".claude-work\settings.json"), "{}", now());
        let info = AccountsInfo::read_with(&Settings::default(), &env, &fs);
        assert_eq!(
            info.defaults.get(&ProviderId::Claude),
            Some(&home().join(".claude"))
        );
        assert_eq!(info.found[&ProviderId::Claude].len(), 2);
        assert!(info.identity(&home().join(".claude")).id().is_some());
        assert_eq!(
            info.identity(&home().join(".claude-work")),
            &Identity::SignedOut
        );
        assert_eq!(
            info.identity(&home().join(".nowhere")),
            &Identity::Unknown,
            "not read"
        );
    }
}
