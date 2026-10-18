# Troubleshooting

## A card shows a message

### Sign-in expired

*Sign-in expired. Lastdrop never signs in for you.* means the tool Lastdrop reads through is signed
out, or its sign-in has expired. Lastdrop never renews another tool's sign-in, because renewing it
would sign that tool out. Sign in again with the tool itself. *Open sign-in…* on the card opens a
terminal with the right command:

| Service | What *Open sign-in…* runs | Or do this yourself |
|---|---|---|
| Claude | `claude` in the account's folder | Start Claude Code and use it once; it renews its own sign-in. |
| Codex | `codex login` in the account's folder | Sign in to Codex. |
| GitHub Copilot | `gh auth login --hostname github.com --web` | Sign in to the GitHub CLI. |
| Cursor | `cursor` | Open the Cursor app and sign in. |
| Grok | `grok login` | Sign in to the Grok CLI. |
| Antigravity | `agy` | Sign in to Antigravity's CLI. |

Once the tool has signed in again, Lastdrop notices the changed sign-in file and reads again by
itself, at the latest after one refresh interval. **Refresh now** in the tray menu tries at once.

Claude has more than one source. If Claude Code's sign-in cannot be used, Lastdrop still shows the
numbers Claude Desktop last received, or the ones Claude Code cached, so the card can show numbers
that are older than usual. The same happens if your Claude Code keeps its sign-in in Windows
Credential Manager instead of `.credentials.json`: Lastdrop does not read other tools' entries
there.

### API key missing or rejected

Meshy, OpenCode Go and the services described in JSON are read with an API key. The message (in
the flyout's header: *Meshy: API key missing or rejected*) means there is none, or the service
refused it (HTTP 401). Choose *Enter key…* to open the settings, open the service's row with the
arrow at its end, paste the key and choose **Save key**. Lastdrop reads the service again at once.

OpenCode Go can also use the key OpenCode itself keeps, so it needs no key in Lastdrop if you use
OpenCode on this PC.

### Not answering, Offline or No new reading

The card keeps the last numbers it has, dimmed with dashed bars, and says when it tries next:

- *Offline*: Windows says the PC has no internet connection.
- *Not answering*: the service gave an error, did not answer in time, or asked Lastdrop to wait.
  Lastdrop waits 5 minutes, then longer after each failure, up to an hour, or as long as the
  service asks.
- *No new reading*: the service answers, but its numbers are old.

These usually go away by themselves. If one stays, see the log below.

### Couldn't read a service

*Couldn't read …* with *Retry* means the very first reading failed, so there are no numbers yet.
The text after it names the reason. Common ones:

- **Copilot** needs the [GitHub CLI](https://cli.github.com) installed and signed in
  (`gh auth login`). Lastdrop looks for `gh.exe` on your `PATH` and in the GitHub CLI's install
  folders.
- **Codex** needs `codex.exe`: the Codex app, or the Codex CLI on your `PATH`.
- **Antigravity** needs agy 1.1.11 or later. An older one says *agy … is too old; update it to
  1.1.11 or later*.
- **OpenCode Go** needs a Go subscription. A key without one is refused.

### Refresh now did nothing

Each service has a minimum time between readings (5 minutes for most), and a service that just
failed waits before it tries again. A refresh skips those services until their time has come. The
age on the card says when it was last read. Codex's limits come from its `app-server` at most every
15 minutes, so between those they come from Codex's session files.

## Where Lastdrop keeps its files

| What | Where |
|---|---|
| Settings | `%APPDATA%\Lastdrop\settings.json` |
| Services you describe | `%APPDATA%\Lastdrop\providers\*.json` |
| Themes | `%APPDATA%\Lastdrop\themes\*.json` |
| Your token prices | `%APPDATA%\Lastdrop\prices.json` |
| Usage history | `%LOCALAPPDATA%\Lastdrop\history\` |
| What Lastdrop remembers between runs | `%LOCALAPPDATA%\Lastdrop\state.json` |
| Logs | `%LOCALAPPDATA%\Lastdrop\logs\` |
| Downloaded updates | `%LOCALAPPDATA%\Lastdrop\updates\` |
| The program | `%LOCALAPPDATA%\Programs\Lastdrop\lastdrop.exe` |
| API keys | Windows Credential Manager, `Lastdrop:…` entries |

Type a folder into the File Explorer address bar to open it. [What Lastdrop
reads](what-lastdrop-reads.md#what-stays-on-your-pc) says what is in each.

## The logs

Lastdrop writes three logs in `%LOCALAPPDATA%\Lastdrop\logs`:

- `lastdrop.log`: the tray icon, the readout, the flyout and every reading. Each attempt to read a
  service is logged with its outcome.
- `settings.log`: the settings, usage history and about windows.
- `install.log`: installing, updating and uninstalling, also when winget does it quietly.

Each log is rotated at 1 MB, and the two before it are kept (`lastdrop.1.log`, `lastdrop.2.log`).
Tokens, keys, cookies and e-mail addresses are masked before a line is written. Still, read a log
before you attach it to an issue.

To report a problem, open an issue at
[github.com/2Fivers/lastdrop/issues](https://github.com/2Fivers/lastdrop/issues). Report security
problems privately as described in [SECURITY.md](../../SECURITY.md).

## Starting over

There is no reset button, but you can delete Lastdrop's files. Quit Lastdrop first (*Quit
Lastdrop* in the tray menu), so it does not write them again.

- **Settings**: delete `%APPDATA%\Lastdrop\settings.json`. The next start is a first run: only
  Claude and Codex are on, and accounts, aliases, pins and notification choices are back to their
  defaults. Saved API keys keep working.
- **History**: delete `%LOCALAPPDATA%\Lastdrop\history`. Charts, forecasts and burn rates start
  again from nothing.
- **What Lastdrop remembers**: delete `%LOCALAPPDATA%\Lastdrop\state.json`. A notification may be
  shown again, and two folders signed in to the same account may both show until they are read.
- **API keys**: open Control Panel ▸ Credential Manager ▸ Windows Credentials and remove the generic
  credentials whose names start with `Lastdrop:`, or choose **Remove** next to each key in the
  settings.

To remove Lastdrop completely, uninstall it, then delete `%APPDATA%\Lastdrop`,
`%LOCALAPPDATA%\Lastdrop` and the `Lastdrop:` credentials.

If `settings.json` cannot be read, for example after you edited it by hand, Lastdrop moves it aside
as `settings.json.bad-<number>` and starts with the defaults. Your old file is still there to
compare with.

## Lastdrop does not start with Windows

Check that *Start Lastdrop when I sign in to Windows* is on in the settings. If it is on and
Lastdrop still does not start, turn it off and on again: that writes Windows' start-up entry
again. Uninstalling removes the entry but keeps the setting, and installing again puts the entry
back while the setting is on.

## Language

Lastdrop speaks English, German, French, Spanish and Swedish. By default it follows Windows: the
first of your Windows display languages that Lastdrop has, else English. German, French and
Spanish serve every region of the language (Austria, Switzerland, Canada, Belgium, Mexico, …). To
choose, use **Language** in the settings: *Same as Windows*, then each language by its own name
(*English*, *Deutsch*, *Español*, *Français*, *Svenska*).

Numbers, dates and times always follow the regional format in Windows (Settings ▸ Time & language ▸
Language & region), whatever language you pick. Meter names from a
[service you describe](custom-services.md) are shown as the file writes them.

## A proxy

Lastdrop does not use the proxy set in Windows' settings. If your network needs a proxy, set the
`HTTPS_PROXY` environment variable (or `ALL_PROXY`) for your user and start Lastdrop again.

## Questions

**Can Lastdrop sign me out of Claude Code or another tool?** It is built not to. It never writes,
renews or copies another tool's sign-in, and it reads their files without locking them.

**Why does a percentage differ a little from the service's own page?** Lastdrop shows whole
percentages, never 0 % for something used, and 100 % only when the service says the limit is used
up. The reading may also be a few minutes old: the card shows its age.

**Does *Pause updates* stop Lastdrop's own update check?** No. It pauses reading your usage. The
update check has its own setting under **Updates**.

**Can I see the readout on every monitor?** Yes: turn on *Show it on every taskbar* under
**Taskbar readout** in the settings.

**Does Lastdrop work without Claude or Codex?** Yes. Turn them off in the settings and turn on the
services you use.
