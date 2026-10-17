# Updates, installer and releases (v0.1)

ROADMAP M5: updates, code signing, installer. Decided 2026-10-16: releases on GitHub Releases,
unsigned until a certificate exists, with the signing step ready in the release script.

References: ARCHITECTURE "App", ROADMAP "Principles" (no telemetry).

## 1. Checking

- **Where:** `GET https://api.github.com/repos/2Fivers/lastdrop-releases/releases/latest`
  (`lastdrop_core::update::RELEASES_REPO`), unauthenticated. The source repository is private
  and GitHub serves a private repository's releases only with a token, so releases go to a
  public repository of their own that holds nothing but releases.
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
  same without asking.
- **What it does**, per user, no elevation: asks a running Lastdrop to quit (the settings-changed
  message with `Changes::quit`) and waits up to 15 s for its mutex; copies itself to
  `%LOCALAPPDATA%\Programs\Lastdrop\lastdrop.exe` (retrying for 10 s while a scanner holds the
  file); writes `Lastdrop.lnk` in the Start menu's Programs folder and the "Installed apps" entry
  (`HKCU\…\Uninstall\Lastdrop`: name, version, publisher Adinosaur AB, icon, folder, size,
  `UninstallString` = `"…\lastdrop.exe" --uninstall`); points "Start with Windows" at the
  installed copy when it was on for another copy; starts the installed app and says it is done.
- **`--uninstall`** (from "Installed apps"): asks, quits the app, removes the Run value (if it is
  the installed copy's), the shortcut and the entry, and has a hidden `cmd.exe` delete the
  install folder two seconds after the uninstaller ends. Settings, history, themes and logs in
  `%APPDATA%\Lastdrop` and `%LOCALAPPDATA%\Lastdrop` stay.
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

`tools\release\release.ps1 -Version 0.1.0 [-Publish]`: checks the version against Cargo.toml and
a clean tree, runs CI's steps, builds, copies the binary to `dist\Lastdrop-Setup.exe`, signs it
when `LASTDROP_SIGN_THUMBPRINT` or `LASTDROP_SIGN_PFX` is set (signtool, SHA-256, RFC 3161
timestamp; otherwise a warning: unsigned builds make SmartScreen warn), writes `dist\SHA256SUMS`,
and creates the GitHub release `v0.1.0` in `2Fivers/lastdrop-releases` with both files, as a
draft unless `-Publish`.

## 5. Not decided here

- Licensing (`LicenseRef-Proprietary` today) and an end-user licence text for the installer.
- A code-signing certificate (Azure Trusted Signing or an OV/EV certificate).
