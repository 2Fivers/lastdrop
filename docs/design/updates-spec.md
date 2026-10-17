# Updates, installer and releases (v0.1)

ROADMAP M5: updates, code signing, installer. Decided 2026-10-16: releases on GitHub Releases,
unsigned until a certificate exists, with the signing step ready in the release script.
Amended 2026-10-17: the source repository is public (MIT OR Apache-2.0), so releases go to it
directly, and winget installs and updates Lastdrop too (§5); releases are built by GitHub
Actions, started in the browser, with a provenance attestation (§4).

References: ARCHITECTURE "App", ROADMAP "Principles" (no telemetry).

## 1. Checking

- **Where:** `GET https://api.github.com/repos/2Fivers/lastdrop/releases/latest`
  (`lastdrop_core::update::RELEASES_REPO`), unauthenticated: the public source repository's
  own releases.
- **When:** two minutes after start-up, then every 24 hours while the app runs, if Settings ▸
  Updates ▸ *Check for updates once a day* is on (`updates.check`, default on). *Check now* in
  Settings asks the tray process to check at once (`Changes::check_updates`); a notification then
  says what was found, also when nothing was or the check failed.
- **Privacy:** the only request Lastdrop makes that is not to a service whose usage it reads.
  It sends nothing about the user: GitHub sees the address and `User-Agent: Lastdrop/<version>`.
  Turning the check off stops it entirely.
- **An offer** (`update::offer`, pure): the latest release, not a draft or pre-release, whose tag
  is a version (`v0.2.0`) newer than this build, with the files `Lastdrop-Setup.exe` (1 byte to
  64 MB) and `SHA256SUMS`, both on `https://github.com/`. Anything else is no offer (logged).
- **Telling the user:** one notification per new version (`Lastdrop 0.2.0 is available`; clicking
  it opens the flyout, N§4), and the flyout's footer `Lastdrop 0.2.0 is available · Install`
  (F§5) while no pause note is there.

## 2. Installing

The release's installer is the app itself: `Lastdrop-Setup.exe`.

- **Run under that name** (any file name starting `Lastdrop-Setup`): a message box asks
  "Install Lastdrop 0.1.0 for this Windows user?" and names the folder. **`--install`** does the
  same without asking. **`--install --quiet`** (what winget runs) shows no message box either:
  the exit code (0 or 1) and the log say how it went, and the installed app starts quietly
  (`--autostart`, nothing opens).
- **What it does**, per user, no elevation: asks a running Lastdrop to quit (the settings-changed
  message with `Changes::quit`) and waits up to 15 s for its mutex; copies itself to
  `%LOCALAPPDATA%\Programs\Lastdrop\lastdrop.exe` (retrying for 10 s while a scanner holds the
  file); writes `Lastdrop.lnk` in the Start menu's Programs folder and the "Installed apps" entry
  (`HKCU\…\Uninstall\Lastdrop`: name, version, publisher Adinosaur AB, icon, folder, size,
  `UninstallString` = `"…\lastdrop.exe" --uninstall`, `QuietUninstallString` = the same with
  `--quiet`); points "Start with Windows" at the installed copy when it was on for another copy;
  starts the installed app and says it is done.
- **`--uninstall`** (from "Installed apps"): asks, quits the app, removes the Run value (if it is
  the installed copy's), the shortcut and the entry, and has a hidden `cmd.exe` delete the
  install folder two seconds after the uninstaller ends. Settings, history, themes and logs in
  `%APPDATA%\Lastdrop` and `%LOCALAPPDATA%\Lastdrop` stay. **`--uninstall --quiet`** neither
  asks nor tells: only the exit code and the log.
- Failures are shown in a message box and the folder is left as it was before the failing step.

## 3. Updating

*Install* in the flyout footer (`CardAction::InstallUpdate`):

1. `SHA256SUMS` and `Lastdrop-Setup.exe` are downloaded on a thread of their own into
   `%LOCALAPPDATA%\Lastdrop\updates` (`lastdrop_providers::release::download`: streamed to the
   file, at most 64 MB; GitHub's redirect to its storage is followed only to `github.com` and
   `*.githubusercontent.com` over `https://`, at most three times). The footer says
   `Downloading Lastdrop 0.2.0…` meanwhile.
2. The installer's size must be the release's and its SHA-256 (BCrypt) the one `SHA256SUMS`
   lists; otherwise it is deleted and a notification says why.
3. The app starts `Lastdrop-Setup.exe --install --update` and quits. The installer waits for it,
   replaces `lastdrop.exe`, and starts it again with `--autostart` (nothing opens).

The checksum guards against a broken or truncated download, not against a compromised release
repository; Authenticode signing (§4) is what will. Once releases are signed, the app should also
check the installer's signature (`WinVerifyTrust`) before starting it: a follow-up.

## 4. Releasing

Built by GitHub's runners and started in the browser, never on a maintainer's machine
([RELEASING](../RELEASING.md)).

- **Actions ▸ release ▸ Run workflow** on main with the version runs
  `.github/workflows/release.yml`: the version must be the workspace version, the run must be on
  main and the tag `v<version>` must not exist; a release build without caches;
  `Lastdrop-Setup.exe` and `SHA256SUMS` (`<hex>  Lastdrop-Setup.exe`, LF);
  `tools/release/install-test.ps1` (`--install --quiet`, then the entry's
  `QuietUninstallString`, each step's result checked); a build provenance attestation of the
  installer; a **draft** release with both files whose tag `v<version>` (not created yet) points
  at the built commit, in the `release` environment, which only main may deploy to. Without a
  version, and on pull requests that touch the installer or the release tooling, it is a dry run
  (no attestation, no release).
- A maintainer tests the draft and publishes it. Publishing creates the tag; only repository
  admins may create `v*` tags (the "release tags" ruleset, which also forbids moving or deleting
  one). Published releases are immutable (the repository's *Release immutability* setting):
  their files and tag cannot be changed, so the files are uploaded while it is a draft.
- Unsigned until a certificate exists; signing becomes a step of the build job, before the
  checksums.

## 5. winget

Package `2Fivers.Lastdrop` in `microsoft/winget-pkgs`. The manifest templates are in
`packaging/winget` (`{{VERSION}}`, `{{SHA256}}`, `{{DATE}}`); `tools\release\winget.ps1` fills
them in.

- **Installer:** `InstallerType: exe`, `Scope: user`, the release's `Lastdrop-Setup.exe` with
  `--install --quiet` for both silent modes; `UpgradeBehavior: install` (the installer replaces
  a running copy, §2). `AppsAndFeaturesEntries` names the "Installed apps" entry (`ProductCode`
  `Lastdrop`, publisher Adinosaur AB), whose `DisplayVersion` is how winget sees the installed
  version, also after Lastdrop updated itself (§3).
- **Uninstall:** winget runs the entry's `QuietUninstallString`.
- **Submitting:** once the release is published (the manifests point at its download URL),
  `winget.ps1 -Version 0.1.0` downloads its files, checks the checksum and the
  attestation, and writes the manifests to `dist\winget`; `wingetcreate submit dist\winget`
  opens the pull request to `microsoft/winget-pkgs`; their validation installs and uninstalls
  it in a sandbox. Unsigned installers are accepted but are likelier to wait for a manual
  review.

## 6. Not decided here

- A code-signing certificate (SignPath Foundation, Azure Trusted Signing or an OV/EV certificate).
