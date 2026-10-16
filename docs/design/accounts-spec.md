# Lastdrop — several accounts

Implementation spec (M2, ROADMAP "Multiple accounts"). Companions:
[readout-spec.md](readout-spec.md) (`R§`; R§6.4 tags), [flyout-spec.md](flyout-spec.md) (`F§`;
F§4.2 card names, F§4.3 `Open sign-in…`) and [notifications-spec.md](notifications-spec.md)
(`N§`). Code: `lastdrop_providers::identity` and `lastdrop_providers::discover` (§2, §6),
`lastdrop_app::state` (§3–§5), `lastdrop_app::settings_window` (§7).

Claude Code and Codex can each be signed in to a different account in every config folder.
Copilot and Meshy keep one account in M2.

## 1. Terms

- **Source**: one config folder, one entry of a service's `accounts` in `settings.json`
  (`key`, `alias`, `tag`, `source`). Claude: `CLAUDE_CONFIG_DIR` or `~\.claude` (the default,
  `source: default`) or any folder (`claude_config_dir`). Codex: `CODEX_HOME` or `~\.codex`
  (`default`) or any folder (`codex_home`).
- **Account**: who is signed in to a source, as the service knows them.
- **Identity**: a 16-hex-digit hash of what names the account (§2). It is the only form of
  the account that Lastdrop keeps (`state.json`); it is never shown.
- **E-mail**: the address the source is signed in with. Read with the identity, held in memory
  as a secret (`SecretString`: never in a log, never in a file), shown only as §4 says.

## 2. Who is signed in

`Provider::identity` reads it from the source's own files, read-only, with the same rules as a
fetch (ARCHITECTURE "Rules for every provider"). The answer is one of:

| Answer | Meaning |
|---|---|
| `SignedIn { identity, email }` | the files name an account |
| `SignedOut` | the files say nobody is signed in |
| `Unknown` | the files cannot tell (missing, unreadable, or a layout Lastdrop does not know) |

- **Claude**: `.claude.json` of the source (`~\.claude.json` for the default folder,
  `<folder>\.claude.json` otherwise), streamed like the cached usage (everything but
  `oauthAccount` skipped): `oauthAccount.accountUuid`, `oauthAccount.organizationUuid` and
  `oauthAccount.emailAddress`. Identity = hash of `claude`, the organisation and the account
  (one person in two organisations has two sets of limits). No `oauthAccount` and no
  `claudeAiOauth` sign-in in `.credentials.json`: `SignedOut`. A sign-in without `oauthAccount`:
  `Unknown`.
- **Codex**: `<CODEX_HOME>\auth.json`: the payload of `tokens.id_token` (a JWT; read, not
  verified: it only names the account, it grants nothing here) gives `email` (else
  `https://api.openai.com/profile.email`), `https://api.openai.com/auth.chatgpt_account_id` (the
  workspace) and `chatgpt_user_id` (else `user_id`). Identity = hash of `codex`, the workspace and
  the user (else the e-mail). `auth.json` with neither `tokens` nor an API key: `SignedOut`. An
  API-key sign-in: `Unknown` (no person to tell apart, and no plan windows). No `auth.json`:
  `Unknown`. Codex can keep its sign-in in the Windows credential store instead, and Lastdrop
  never reads another tool's credential store entries.
- **Hash**: FNV-1a 64 over the UTF-8 parts joined by `\n`, as 16 lowercase hex digits. Stable
  across builds and machines. It is not a secret, but it is never logged either.

**When it is read** (tray process, on the refresh worker, as part of a job): on a source's first
job in a run, after its credential fingerprint changed, and otherwise at most every 30 minutes
(`ACCOUNT_RECHECK`). The settings window reads it for every listed folder when it opens.

## 3. Same account in two folders

Two enabled sources of one service with the same known identity are **one account**. The first
in the settings' order is the account's source; each later one is a **duplicate**:

- It has no card, no readout row, no hover-card row and no notices.
- It is not read: its usage is the first source's. Its identity is still checked every
  `ACCOUNT_RECHECK` (an identity-only job), so it becomes an account of its own as soon as it is
  signed in to someone else.
- Settings show it as `Same account as {name}`.
- Duplicates are worked out again after every identity change and every settings change. The
  identities kept in `state.json` (§5) mean a restart knows them before the first job, so a
  duplicate never shows for a moment and is never read at start-up.

A source whose identity is `SignedOut` or `Unknown` is never a duplicate.

## 4. Names

The card name (F§4.2), the hover card's service row (R§8.2), the tooltip, UI Automation and
notifications all use one `{service}` (R§6.4):

1. The alias when the account has one: `Claude · Work`.
2. Else, when the service shows two or more accounts: `Claude · {e-mail}`, or
   `Claude · Account {n}` when the e-mail is unknown or **Hide personal info** is on. `{n}` is the
   account's place among the service's shown accounts (settings order), from 1.
3. Else the service name alone: `Claude`.

The readout keeps R§6.4's tags: the account's tag, else the alias's first letter, else `{n}`.

**Hide personal info** (`hide_personal_info`, default off): no e-mail address is shown
anywhere, the settings window included. Folder paths still show in the settings window.

## 5. A folder signed in to someone else

When a source's identity changes from one known identity to another (`SignedIn` A, then
`SignedIn` B, in this run or since the identity kept in `state.json`), the folder now holds
another account. Lastdrop:

- drops the source's reading, freshness and refresh memory, and reads it again now;
- retires the source's history: its meter files are renamed `<file>.<seconds>.retired` (the
  Unix time of the change), so pace, forecasts and the sparkline start again from the new
  account's readings. Retired files are never loaded, and the daily compaction deletes them once
  they are older than the retention (90 days);
- forgets what notifications announced for the source's meters (N§1), so the new account's
  levels are announced afresh, and the severity memory of those meters (R§3.7);
- keeps the folder's alias, tag, pins and hidden meters: they belong to the folder entry;
- logs `{provider}/{key}: signed in to another account` (no identities, no e-mail).

`SignedIn` followed by `SignedOut` or `Unknown` is not a change of account: an expired or
removed sign-in is F§4.3's `SIGN_IN` card. The last known identity is kept.

`state.json` keeps `identities: [{provider, account, identity}]`, one per source whose identity
is known, written with the rest of the state.

## 6. Finding folders

`discover(provider)` lists candidate folders, read-only, cheap (a directory listing of the
profile folder and a few `metadata` calls):

- Claude: the default folder, then every `~\.claude-*` and `~\.claude_*` folder holding
  `.credentials.json`, `.claude.json`, `settings.json` or `projects\`.
- Codex: the default folder, then every `~\.codex-*` and `~\.codex_*` folder holding
  `auth.json`, `config.toml` or `sessions\`.

A candidate already used by an account (the same folder, compared case-insensitively after
resolving the default) is not offered again.

**First run** (no `settings.json` yet): the default folders are used as in M1. When discovery
finds more folders, the settings window opens at its **Accounts** section with them listed as
found folders, so nothing beyond the defaults is read before the user adds it.

## 7. Settings window

An **Accounts** section after **Services**, with a block for Claude and one for Codex:

- One row per account: its name (§4; `Account {n}` when nothing else), the folder, and its state:
  `Signed in as {email}` (or `Signed in` when hidden or unknown), `Not signed in`,
  `Can't tell who is signed in`, or `Same account as {name}`. An **Alias** box (up to 24
  characters; empty = none) and **Remove**. Removing the last account is not offered: turn the
  service off instead.
- **Found folders** (§6) not yet used, each with **Add**.
- **Add folder…**: a folder picker (`IFileOpenDialog`, folders only). A folder already used is
  not added twice.
- **New account…** with a name box: creates `~\.claude-{name}` or `~\.codex-{name}` (the name
  in lower case, characters outside `a-z 0-9 - _` replaced by `-`; `-2`, `-3` … when taken), adds
  it as an account with the name as its alias, and opens the vendor's own sign-in in a terminal
  with the folder set (`Provider::sign_in_command`: `claude` / `codex login`). Lastdrop never
  signs in itself.
- **Hide personal info** check box.

New accounts get the key `f-` and 8 hex digits of the folder's hash (`-2` … on a clash); the
default account keeps `default`. Keys never change, so history, pins and notifications follow the
folder entry. The meter list under **Taskbar readout** names the account when its service has
two or more (`Claude · Work weekly`).

## 8. Tests

- `crates/providers/tests/identity.rs`: every §2 answer from invented `.claude.json`,
  `.credentials.json` and `auth.json` fixtures; the hash's stability (fixed vectors); discovery
  (§6) on a fake file system.
- `crates/app`: duplicates (§3), names (§4) and switches (§5) on the state's free functions, the
  plan skipping duplicates, the retirement of history files, the identities in `state.json`.
