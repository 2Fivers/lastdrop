# Accounts

Claude Code and Codex keep their sign-in in a config folder. You can have several folders, each
signed in to a different account: one for work and one for home, say. Lastdrop reads each folder
you add as an account of its own, with its own card, meters, history and notifications.

Other services have one account in Lastdrop.

## The folders

| Tool | Default folder | Other folders |
|---|---|---|
| Claude Code | `%USERPROFILE%\.claude`, or the folder in `CLAUDE_CONFIG_DIR` when that is set | any folder you start Claude Code in with `CLAUDE_CONFIG_DIR` |
| Codex | `%USERPROFILE%\.codex`, or the folder in `CODEX_HOME` when that is set | any folder you start Codex in with `CODEX_HOME` |

Lastdrop reads who is signed in from the folder's own files. It never signs in, never renews a
sign-in and never changes anything in the folder.

## Adding an account

Open **Settings ▸ Services**. The rows of Claude and Codex say how many accounts they have; open
one with the arrow at the end of its row to see its accounts, then:

- **Found on this PC**: folders Lastdrop found in your user folder that are not accounts yet, each
  with **Add**. It looks for `.claude-*` and `.claude_*` folders that hold Claude Code data, and
  `.codex-*` and `.codex_*` folders that hold Codex data.
- **Add folder…**: pick any folder yourself. A folder that is already an account is not added
  twice.
- **New account…**: type a name, such as `Work`, and choose **Create and sign in**. Lastdrop
  creates `%USERPROFILE%\.claude-work` (or `.codex-work`), adds it with the name as its alias, and
  opens a terminal with Claude Code's or Codex's own sign-in for that folder. Sign in there.

A new account is saved at once and read from then on.

On the first run, if Lastdrop finds more folders than the default ones, it opens this part of the
settings by itself. Only the default folders are read until you add the others.

## What the settings show for each account

Each account row has its name, one of these, and its folder:

- *Signed in as* and the e-mail address.
- *Signed in*, when e-mail addresses are hidden or the folder does not name one.
- *Not signed in*.
- *Can't tell who is signed in*: the folder's files do not say, for example a Codex folder signed
  in with an API key instead of a ChatGPT account.
- *Same account as …*: see below.

The **Alias** box gives the account a name of up to 24 characters. **Remove** takes the account out
of Lastdrop; the folder itself stays. The last account of a service cannot be removed: turn the
service off instead.

## How accounts are named

- An account with an alias is shown as, for example, *Claude · Work*.
- Without an alias, when a service has two or more accounts, it is shown with its e-mail address:
  *Claude · ada@example.com*. If the address is hidden or unknown, it is *Claude · Account 1*,
  *Claude · Account 2* and so on, numbered in the order of the settings.
- A service with one account is shown by the service name alone.

## The same account in two folders

If two folders are signed in to the same account, Lastdrop shows it once. The first folder in the
settings is read; the other is listed as *Same account as …* and has no card, no readout row and no
notifications. Lastdrop still checks every half hour who that folder is signed in to, so if you
sign it in to another account, it becomes an account of its own.

## A folder signed in to someone else

If you sign a folder out of one account and in to another, Lastdrop notices it the next time it
checks who is signed in there. The old account's history for that folder is set aside, so pace and
forecasts start again from the new account's readings, and notifications start afresh. The alias,
pins and hidden meters stay with the folder.

A sign-in that only expired is not a new account. The card says *Sign-in expired* until you sign in
again; see [Troubleshooting](troubleshooting.md#sign-in-expired).

## Hiding e-mail addresses

Turn on **Hide e-mail addresses** under **Privacy** in the settings to show *Account 1*, *Account
2* instead of e-mail addresses everywhere: in the flyout, the hover card, the readout, tooltips,
notifications and the settings window. Folder paths still show in the settings window. This helps
when you share your screen or take screenshots.

Lastdrop does not store e-mail addresses. It reads them from the folder when it needs them and
keeps them in memory only. What it does keep, in `%LOCALAPPDATA%\Lastdrop\state.json`, is a short
hash of who each folder is signed in to, so that a restart still knows which folders are the same
account.
