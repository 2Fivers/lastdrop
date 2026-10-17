# Releasing Lastdrop

How a version goes from main to users, all in the browser: GitHub's runners build it, a
maintainer tests the draft and publishes it, and winget picks it up. The mechanics are in
[updates-spec](design/updates-spec.md) §2–5.

## 1. Before the release

- The version is bumped in the workspace `Cargo.toml` (`[workspace.package] version`) through a
  pull request, and main is green.
- The release checks of [ARCHITECTURE](ARCHITECTURE.md) ("Quality bar"): `tools/measure` within
  budget, a 24-hour soak with no growth in memory, GDI or USER handles, and the manual
  multi-monitor matrix.

## 2. Build the draft

On GitHub: **Actions ▸ release ▸ Run workflow**, *Use workflow from* **main**, *Version*
`0.1.0`, **Run workflow**. [release.yml](../.github/workflows/release.yml):

1. checks that the version is the workspace version, that it runs on main and that the tag
   `v0.1.0` does not exist yet;
2. builds `lastdrop.exe` from main's commit (no cache) and copies it to `Lastdrop-Setup.exe`,
   writes `SHA256SUMS`;
3. runs [install-test.ps1](../tools/release/install-test.ps1) on the runner, which is a clean
   Windows user: `--install --quiet` and the "Installed apps" entry's `QuietUninstallString`,
   exactly what winget runs, with the program, shortcut, registry entry and running app checked
   after each step;
4. attests the installer's build provenance (`gh attestation verify` checks it);
5. creates a **draft** release `v0.1.0` with both files, pointing at the commit it built. The
   tag does not exist yet. Running Lastdrops see published releases only.

Run without a version, it is a dry run (steps 1–3, no attestation, no release); pull requests
that touch the installer or the release tooling run that dry run too.

A draft that turns out bad is deleted on GitHub (the release's *Delete*); run the workflow again
after the fix is on main.

## 3. Test the draft

Download the draft's `Lastdrop-Setup.exe` from its page under **Releases** (drafts are visible
to maintainers only).

Windows Sandbox would be the easy clean machine, but it needs Windows 11 Pro or Enterprise. The
installer is per user, so a second local account is a clean machine for Lastdrop too.

**A. Upgrade on your own account** (the path existing users take):

- [ ] Run `Lastdrop-Setup.exe` by double-click. SmartScreen warns while releases are unsigned:
      *More info → Run anyway*. The question names the version and
      `%LOCALAPPDATA%\Programs\Lastdrop`.
- [ ] The running Lastdrop quits, the new one starts, and "Installed" is reported.
- [ ] Settings, accounts, history and themes are as before.
- [ ] About Lastdrop shows the new version. Its title bar, Settings' and Usage history's show
      the app icon, and so do Alt+Tab and the taskbar button.
- [ ] Settings ▸ Apps ▸ Installed apps lists Lastdrop with the version, Adinosaur AB and the
      icon; the Start menu entry has the icon.

**B. A clean Windows user** (first run):

- [ ] Create a local standard account: Settings ▸ Accounts ▸ Other users ▸ *Add account* ▸
      *I don't have this person's sign-in information* ▸ *Add a user without a Microsoft
      account*. Sign in to it and copy the installer there.
- [ ] Install by double-click. The tray icon appears; the flyout opens next to it and shows the
      first-run state (no accounts: sign-in hints, found folders none).
- [ ] Settings, About and Usage history open, with the icon, in the account's display language.
- [ ] *Start with Windows* works: sign out and in again, Lastdrop starts without opening
      anything.
- [ ] Uninstall from Installed apps: it asks, quits Lastdrop, says it is done; the Start menu
      entry and `%LOCALAPPDATA%\Programs\Lastdrop` are gone (settings in `%APPDATA%\Lastdrop`
      stay, by design).
- [ ] Remove the test account afterwards (Settings ▸ Accounts ▸ Other users ▸ *Remove*).

**C. From the next release on:** the update path. On a machine with the previous release, the
flyout footer offers the new version; *Install* downloads it, checks it and restarts Lastdrop
on the new version without opening anything.

Then publish the draft on GitHub (*Edit ▸ Publish release*). Publishing creates the tag
`v0.1.0` on the built commit; only repository admins may create `v*` tags (the "release tags"
ruleset). A published release cannot be changed (release immutability): a fix is a new
version.

## 4. winget

The one step outside the browser, for now. Once the release is published:

```
pwsh tools\release\winget.ps1 -Version 0.1.0
```

It downloads the published files, checks the checksum and the provenance attestation, and
writes the manifests to `dist\winget` (`winget validate` passes on them). Optionally install
from them before submitting: `winget settings --enable LocalManifestFiles` once in an
administrator terminal, then in a normal terminal (the install is per user)
`winget install --manifest dist\winget` and `winget uninstall Lastdrop`.

Submit with `wingetcreate submit dist\winget` (`winget install Microsoft.WingetCreate`; the
first run signs in to GitHub). The first submission's pull request in `microsoft/winget-pkgs`
asks for Microsoft's CLA: answer
`@microsoft-github-policy-service agree company="Adinosaur AB"`. A moderator merges it after
the automated validation.

## Code signing

Releases are unsigned until a certificate exists. The plan is SignPath Foundation, which signs
open-source builds made in public CI: its signing step goes in `release.yml`'s build job, before
the checksums, and its secrets in the `release` environment.
