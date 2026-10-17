# Security policy

Lastdrop reads the sign-in tokens of other tools (Claude Code, Codex, the GitHub CLI, Cursor and
others) to ask their services how much usage is left, and it updates itself from GitHub
Releases. A security problem in it can therefore expose more than Lastdrop's own data, and we
treat reports as urgent.

## Reporting a vulnerability

**Please do not open a public issue.** Report it privately through GitHub instead:
[Security ▸ Report a vulnerability](https://github.com/2Fivers/lastdrop/security/advisories/new).

Include what you found, how to reproduce it and the Lastdrop version (About Lastdrop in the
tray menu). Never include a real token, cookie or log with personal data: invented values that
show the shape are enough.

We aim to acknowledge a report within 7 days and to agree a disclosure date with you once a fix
is ready. We credit reporters in the advisory unless you prefer not to be named.

## Supported versions

Only the latest release gets security fixes. Lastdrop checks for updates daily (Settings ▸
Updates) and installs them with one click; winget users get them with `winget upgrade`.

## In scope

- Reading, storing or sending tokens, cookies and API keys: anything that could leak them to a
  host other than the service they belong to, write them to a log or a plain file, or refresh or
  rewrite another tool's sign-in.
- The custom service definitions (`providers/*.json`, `%APPDATA%\Lastdrop\providers`): a
  definition that could make Lastdrop send a credential somewhere it was not meant to go.
- The updater and installer: the download, the checksum, the install folder, the registry
  entries and the clean-up on uninstall.
- Anything else that runs code, reads files or makes requests the user did not ask for.

## Out of scope

- Problems in the services Lastdrop reads, or in their own tools; report those to their vendors.
- An attacker who already runs code as the same Windows user: they can read the same tokens
  Lastdrop reads without it.
- Unsigned releases making SmartScreen warn (known; releases will be signed).
