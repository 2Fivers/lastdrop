# Getting started

Lastdrop runs on Windows 11, for one Windows user at a time. It needs no administrator rights.

## Install

### With winget

```powershell
winget install lastdrop
```

The package is `2Fivers.Lastdrop`. It is on its way into winget, so if winget does not find it
yet, install by hand.

### By hand

1. Download `Lastdrop-Setup.exe` from the
   [latest release](https://github.com/2Fivers/lastdrop/releases/latest).
2. Run it. It asks whether to install Lastdrop for this Windows user, then copies itself to
   `%LOCALAPPDATA%\Programs\Lastdrop`, adds Lastdrop to the Start menu and to Settings ▸ Apps ▸
   Installed apps, and starts it.

The installer is not code-signed yet, so the first time you run it Windows SmartScreen says
*Windows protected your PC*. Choose **More info**, then **Run anyway**.

If you want to be sure the file is the one GitHub built from this repository, check its build
attestation with the [GitHub CLI](https://cli.github.com) before you run it:

```powershell
gh attestation verify Lastdrop-Setup.exe --repo 2Fivers/lastdrop
```

## First run

When the install is done, Lastdrop's icon is in the notification area at the right end of the
taskbar, and the flyout opens. (After a winget install, Lastdrop starts quietly instead: click its
icon to open the flyout.) If you do not see the icon, it may be in the overflow behind the **^**
arrow; drag it onto the taskbar to keep it in sight.

On the first run, Claude and Codex are on and every other service is off. Claude Code and Codex
keep their own sign-in files on your PC, so Lastdrop can read them without any setup. Turn the
other services on in **Settings ▸ Services**; [What Lastdrop reads](what-lastdrop-reads.md) says
what each one needs.

If Claude Code or Codex has more than one config folder on your PC, the first time you start
Lastdrop yourself it also opens its accounts in **Settings ▸ Services**, so you can choose which
of them to read. See [Accounts](accounts.md).

Lastdrop never signs in to a service for you. If a service is not signed in, sign in with its own
app or command-line tool, and Lastdrop picks it up.

## Where Lastdrop shows up

### The tray icon

The tray icon draws the meter that is closest to running out: its percentage used over a small
bar while all is well, an amber tile when it will run out before its reset, a red tile when it is
nearly used up or about to run out, and a red tile with a drop when it is used up.

- **Click** it to open the flyout. Click it again, or press **Esc**, to close it. With the
  keyboard: **Win+B**, the arrow keys to reach the icon, then **Enter**.
- **Rest the pointer** on it for the [hover card](#the-hover-card).
- **Right-click** it for the menu:
  - *Refresh now*
  - *Pause updates*: for 1 hour, or until you choose *Resume updates*. This pauses reading your
    usage, not Lastdrop's own update check.
  - *Taskbar readout*: *Auto*, *Pinned meters…* or *Off*
  - *Usage history…*
  - *Settings…*
  - *About Lastdrop*
  - *Quit Lastdrop*

Starting Lastdrop again while it runs (from the Start menu, for example) opens the flyout of the
copy that is already running.

### The taskbar readout

The readout is a small panel in the taskbar, to the left of the notification area. It shows up to
two meters, each as a short label, a bar, the percentage used and when it resets, so you can see
the most important numbers without a click.

By default it shows the two meters that are most constrained right now (*Auto*). To choose them
yourself, use *Taskbar readout ▸ Pinned meters…* in the tray menu, which opens the settings at the
meter list, or *Show in taskbar readout* in a card's right-click menu in the flyout. You can pin up
to two; pinning a third replaces the one you pinned first. Choosing *Auto* again forgets your pins.
*Off* hides the readout; the tray icon stays.

- **Rest the pointer** on it for the hover card.
- **Click** it to open or close the flyout.
- **Right-click** it for the same menu as the tray icon.

### The flyout

The flyout opens above the tray icon, like the volume and network flyouts. It has one card per
service and account, with every meter and balance, a reset time, and an explanation under any
meter that is in trouble. [Reading the flyout](reading-the-flyout.md) explains what is on it.

### The hover card

The hover card opens when you rest the pointer on the taskbar readout or the tray icon. It says in
a sentence or two whether you will make it to the next reset at your recent pace, and shows the
numbers behind that for each meter in the readout (over the tray icon with the readout off, for
the most constrained meter). Click it to close it.

### A keyboard shortcut

You can open the flyout from anywhere with a keyboard shortcut of your own. It is off by default;
set one under **Keyboard shortcut** in the settings. In the flyout, **F5** or **Ctrl+R** refreshes,
**Ctrl+H** opens the usage history and **Esc** closes it.

## Start with Windows

Lastdrop does not start with Windows until you ask it to. Turn on *Start Lastdrop when I sign in
to Windows* in Settings. It then starts quietly when you sign in, without opening the flyout.

## Updates

Lastdrop asks GitHub for its newest release two minutes after it starts and then once a day. When
a newer version is out, it shows a notification, once for each version (a restart does not repeat
it), and the flyout's footer says *Lastdrop x.y.z is available · Install*. Nothing is installed until you choose **Install**. Lastdrop then downloads
the new installer, checks its size and its SHA-256 checksum against the release's `SHA256SUMS`,
installs it and starts again. Your settings and history stay.

To turn the daily check off, clear *Check for updates once a day* in **Settings ▸ Updates**.
**Check now** next to it checks at once. The check sends nothing about you; see
[What Lastdrop reads](what-lastdrop-reads.md#updates).

If you installed with winget, `winget upgrade lastdrop` works too.

## Uninstall

Uninstall Lastdrop from **Settings ▸ Apps ▸ Installed apps** in Windows. It quits Lastdrop,
removes the program, the Start menu shortcut and the start-with-Windows entry.

Your settings, history, themes and logs stay, in case you install again. API keys you saved stay
in Windows Credential Manager. To remove those as well, see
[Troubleshooting](troubleshooting.md#starting-over).
