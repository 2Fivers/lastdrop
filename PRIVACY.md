# Privacy policy

Last updated: 17 October 2026

Lastdrop is a Windows tray app from 2Fivers that shows how much of your AI services' usage is
left. In short, Lastdrop has no servers of its own and sends no telemetry. It reads the sign-ins
of tools you already use and sends each one only to the service it belongs to. Everything else
stays on your PC. 2Fivers receives no data from Lastdrop.

## What Lastdrop reads on your PC

To ask a service how much usage is left, Lastdrop reuses the sign-in of that service's own tool.
It reads these sign-ins without changing them, keeps them in memory only for the request, and
never writes, renews, copies or logs them.

| Service | What Lastdrop reads |
|---|---|
| Claude | Claude Code's sign-in in its config folder. It also reads the usage Claude Code and Claude Desktop have already saved, which contains no sign-in. |
| Codex | Codex's session files. It can also ask the Codex CLI (`codex app-server`), which uses its own sign-in. |
| GitHub Copilot | The GitHub CLI's sign-in (`gh auth token`). |
| Cursor | The sign-in the Cursor app keeps in its local database. |
| Google Antigravity | Nothing. It asks the Antigravity CLI (`agy`), which uses its own sign-in. |
| OpenCode Go | The API key you saved in Lastdrop, or else the one in OpenCode's `auth.json`. |
| Grok | The Grok CLI's sign-in (`auth.json`). |
| Meshy, DeepSeek, ElevenLabs, OpenRouter and services you add | Only the API key you saved in Lastdrop. |

From the same files Lastdrop also reads the e-mail address you are signed in with, so that it
can tell your accounts apart. The address is kept in memory only. Lastdrop saves only a hash of
the account, which it uses to recognise the account again. Turn on *Hide e-mail addresses* in
Settings to show *Account 1*, *Account 2* instead.

On the first start, Lastdrop reads only the tools' default folders. If it finds more Claude Code
or Codex folders, it lists them in Settings, and it reads none of them until you add it.

## What Lastdrop sends, and to whom

**To each service you use, its own usage request.** Each request goes over HTTPS, carries only
that service's own sign-in or key, and asks only how much usage is left:

| Service | Sent to |
|---|---|
| Claude | `api.anthropic.com` |
| Codex | OpenAI, through the Codex CLI |
| GitHub Copilot | `api.github.com` |
| Cursor | `cursor.com` |
| Google Antigravity | Google, through the Antigravity CLI |
| OpenCode Go | `opencode.ai` |
| Grok | `cli-chat-proxy.grok.com` |
| Meshy | `api.meshy.ai` |
| DeepSeek, ElevenLabs, OpenRouter | `api.deepseek.com`, `api.elevenlabs.io`, `openrouter.ai` |
| Services you add | The `https://` address in the service's file |

Each service handles these requests under its own privacy policy. It sees the same kind of
request it already gets from its own tools.

**To GitHub, a daily update check.** Lastdrop asks
`api.github.com/repos/2Fivers/lastdrop/releases/latest` once a day whether there is a newer
version. The request is unauthenticated and carries nothing about you. GitHub sees your IP
address and `User-Agent: Lastdrop/<version>`. Turn off *Check for updates once a day* in
Settings to stop it. When you choose *Install*, Lastdrop downloads the new version from that
release on GitHub.

**Nothing else.** Lastdrop has no telemetry, analytics, crash reports, ads or tracking. Links
you click in Lastdrop, such as release notes or the support link, open in your browser.

## What Lastdrop stores on your PC

- `%APPDATA%\Lastdrop`: your settings (no secrets), the services you added, your token prices
  and your themes.
- `%LOCALAPPDATA%\Lastdrop`:
  - Usage history: one reading per meter every 5 minutes, kept for 90 days.
  - A state file with meter names, levels, reset times and account hashes, but no secrets.
  - Logs, rotated at 1 MB. Lastdrop never writes tokens, keys, cookies, account ids or e-mail
    addresses to them.
- Windows Credential Manager: the API keys you enter, as `Lastdrop:<service>:<account>:<name>`.
  Windows protects them for your user account.

Nothing of this leaves your PC. Uninstalling Lastdrop removes the program but keeps these
files. To remove everything, delete the two `Lastdrop` folders and the `Lastdrop:` entries in
Credential Manager.

## Your data and 2Fivers

2Fivers does not collect, receive or process personal data through Lastdrop. There is nothing
for us to hand over, correct or delete. What each AI service stores about you is covered by
that service's own privacy policy.

## Changes

Any change to this policy is made in this file. Its history is in the repository.

## Contact

Ask in an [issue](https://github.com/2Fivers/lastdrop/issues). Report security problems
privately as described in [SECURITY.md](SECURITY.md).
