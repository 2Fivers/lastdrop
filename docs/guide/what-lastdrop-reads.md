# What Lastdrop reads

This page lists, for each service, which files Lastdrop reads, which programs it starts, which web
address it asks and with what key, and how often. It is written from the code, so it describes
what Lastdrop does, not what it might do one day.

## The short version

- Lastdrop talks only to the services whose usage it shows, and to GitHub to check for its own
  updates. There is no telemetry and no account with Lastdrop.
- It reads other tools' sign-in files but never writes, renews or copies them. Their files stay
  usable by their own tools while Lastdrop reads them.
- A token it reads is used for one request, in memory, and then dropped. API keys you give
  Lastdrop are kept in Windows Credential Manager, never in a file.
- What it keeps on your PC is your settings, the usage numbers it read, and logs. The logs mask
  tokens, keys and e-mail addresses.
- A service is read only while it is turned on. On a new install only Claude and Codex are on.

`%USERPROFILE%`, `%APPDATA%` and `%LOCALAPPDATA%` below are your user folder, its `AppData\Roaming`
and its `AppData\Local`.

## How it connects

- Only `https://` addresses. Lastdrop follows no redirects, except to GitHub's own download servers
  when it downloads an update.
- It identifies itself as `Lastdrop/<version>`, except for Cursor, as noted below.
- It uses a proxy only if one is set in the `ALL_PROXY`, `HTTPS_PROXY` or `HTTP_PROXY` environment
  variable. It does not use the proxy set in Windows' own settings.
- Programs it starts run without a window and are stopped if Lastdrop quits.
- When a service refuses a request or asks it to wait (HTTP 403 or 429), Lastdrop backs off: 5
  minutes, doubling up to an hour, or as long as the service asks, up to a day. When a sign-in is
  rejected (HTTP 401), it stops asking until that sign-in changes.

## Claude

Lastdrop reads Claude's limits from up to four places and shows the most recent reading:

1. **Anthropic's usage endpoint** with Claude Code's sign-in.
   - Reads the access token from Claude Code's `.credentials.json` in its config folder
     (`%USERPROFILE%\.claude`, or the folder in `CLAUDE_CONFIG_DIR`). The refresh token in the
     same file is skipped.
   - Asks `GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <token>`.
   - Not when the token has expired: Lastdrop never renews it. Claude Code renews it the next time
     you use it.
2. **Claude Desktop's own last usage answer**, from its web cache in `%APPDATA%\Claude\Cache`
   (or the Microsoft Store app's copy under `%LOCALAPPDATA%\Packages`). Only answers from
   `claude.ai`'s usage address are looked at. No token is used.
3. **Claude Desktop's `plan-usage-history.json`** in the same folder. No token is used.
4. **Claude Code's cached usage** in `.claude.json` (`%USERPROFILE%\.claude.json`, or in the
   config folder when `CLAUDE_CONFIG_DIR` is set). No token is used.

How often: at most every 5 minutes. Claude Desktop's files belong to the default account only.

For [several accounts](accounts.md), Lastdrop also reads who is signed in from the `oauthAccount`
part of `.claude.json`: the account and organisation ids and the e-mail address.

The usage history window also reads Claude Code's session logs (`projects\` in the config folder)
for token counts. It keeps only the counts, never a prompt, an answer or a file name.

## Codex

1. **Codex's session files**: the end of the newest `rollout-*.jsonl` files in
   `%USERPROFILE%\.codex\sessions` (or in the folder in `CODEX_HOME`), where Codex writes its
   current limits. While these are less than 10 minutes old, nothing else is needed.
2. **`codex app-server`**: otherwise Lastdrop starts Codex's own app server,
   `codex app-server -c features.plugins=false`, asks it for the account's limits
   (`account/rateLimits/read`) and closes it. It looks for `codex.exe` in the Codex app's folder
   under `%LOCALAPPDATA%\OpenAI\Codex\bin`, then on your `PATH`.

How often: the session files at most every minute; the app server at most every 15 minutes per
folder, unless you refresh by hand or a limit has just reset.

Lastdrop itself makes no web request for Codex and reads no token. The app server is Codex's own
program: it talks to OpenAI, and like Codex itself it may renew Codex's sign-in and update
`auth.json` while it runs. Lastdrop only looks at the size and date of `auth.json` to notice a new
sign-in, and reads who is signed in from the ID token in it.

The usage history window also reads Codex's session files (`sessions\` and `archived_sessions\`)
for token counts, and keeps only the counts.

## GitHub Copilot

- Runs `gh auth token --hostname github.com` (the [GitHub CLI](https://cli.github.com)) to get the
  token you signed in with (`gh auth login`). It looks for `gh.exe` on your `PATH`, then in the
  GitHub CLI's install folders.
- Asks `GET https://api.github.com/copilot_internal/user` with that token. This is the address
  Copilot's editor extensions use; GitHub does not document it.
- Looks only at the size and date of `hosts.yml` in the GitHub CLI's config folder, to notice a new
  sign-in. It does not read the file.

How often: at most every 5 minutes. Off until you turn it on.

## Cursor

- Reads the sign-in token from the Cursor app's database,
  `%APPDATA%\Cursor\User\globalStorage\state.vscdb`, with Lastdrop's own read-only reader. It
  looks up only the `cursorAuth/accessToken` entry and never copies the file.
- Asks `GET https://cursor.com/api/usage-summary`, the address Cursor's dashboard uses, with that
  token as a session cookie. Cursor does not document it. This request sends a browser's
  `User-Agent` (`Mozilla/5.0`), as the dashboard does.

How often: at most every 5 minutes. Off until you turn it on.

## Google Antigravity

- Runs Antigravity's command-line tool, `agy -p /usage --output-format json` (agy 1.1.11 or
  later; it checks the version once with `agy --version`). It looks for `agy` in
  `ANTIGRAVITY_CLI_PATH`, then `%LOCALAPPDATA%\agy\bin\agy.exe`, then your `PATH`.
- Lastdrop reads no file and makes no web request for Antigravity: `agy` talks to Google and
  renews its own sign-in.

How often: at most every 10 minutes. Off until you turn it on.

## OpenCode Go

- Uses the API key you save in its row of Settings ▸ **Services**. Without one, it reads the
  OpenCode Go key that OpenCode keeps in `%USERPROFILE%\.local\share\opencode\auth.json`.
- Asks `GET https://opencode.ai/zen/go/v1/usage` with `Authorization: Bearer <key>`. This is the
  address OpenCode's console uses; it is not documented.

How often: at most every 5 minutes. Off until you turn it on.

## Grok

- Reads the sign-in of the Grok CLI (`grok login`) from `%USERPROFILE%\.grok\auth.json` (or the
  file in `GROK_AUTH_PATH`, or `auth.json` in `GROK_HOME`). The refresh token is skipped and API-key
  entries are left alone.
- Asks `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`, the request the Grok CLI
  makes for `/usage`, with the same headers: the token, and the user id from the same file.
- Not when the sign-in has expired: Lastdrop never renews it. Sign in again with the Grok CLI.

How often: at most every 5 minutes. Off until you turn it on.

## Meshy

- Uses the API key you save in its row of Settings ▸ **Services**.
- Asks `GET https://api.meshy.ai/openapi/v1/balance` with `Authorization: Bearer <key>`.

How often: at most every 5 minutes. Off until you save a key.

## Services described in JSON

DeepSeek, ElevenLabs, OpenRouter and any service you [describe yourself](custom-services.md):

- Uses the API key you save in its row of Settings ▸ **Services**.
- Asks `GET` the address in the service's file, with the key in the header the file names.

| Service | Address |
|---|---|
| DeepSeek | `https://api.deepseek.com/user/balance` |
| ElevenLabs | `https://api.elevenlabs.io/v1/user/subscription` |
| OpenRouter | `https://openrouter.ai/api/v1/credits` |

A file you add can name any `https://` address, so only add files you trust with the key you give
them. How often: at most every 5 minutes. Off until you turn it on or save a key.

## API keys

The keys you save in Settings ▸ **Services** are stored in Windows Credential Manager as generic
credentials named `Lastdrop:<service>:default:api_key`, for example
`Lastdrop:meshy:default:api_key`. Lastdrop reads only its own entries there. You can see and
delete them in Control Panel ▸ Credential Manager ▸ Windows Credentials.

## Updates

Two minutes after it starts, and then once a day, Lastdrop asks
`GET https://api.github.com/repos/2Fivers/lastdrop/releases/latest` for its newest version. The
request has no sign-in and sends nothing about you beyond what any web request does (your IP
address, and `Lastdrop/<version>` as the user agent). If you choose **Install**, it downloads the
installer and its `SHA256SUMS` from the GitHub release. Turn the check off under **Updates** in the
settings.

## What stays on your PC

| What | Where |
|---|---|
| Settings (no keys or tokens) | `%APPDATA%\Lastdrop\settings.json` |
| Your service files, themes and prices | `%APPDATA%\Lastdrop\providers`, `themes`, `prices.json` |
| Usage history: the numbers only, 90 days | `%LOCALAPPDATA%\Lastdrop\history` |
| What the app remembers between runs: which meters it saw, what it notified (the last update it announced too), and a short hash of who each Claude Code or Codex folder is signed in to | `%LOCALAPPDATA%\Lastdrop\state.json` |
| Logs, with tokens, keys and e-mail addresses masked | `%LOCALAPPDATA%\Lastdrop\logs` |
| A downloaded update | `%LOCALAPPDATA%\Lastdrop\updates` |
| API keys you saved | Windows Credential Manager |

E-mail addresses are never written to a file. Lastdrop reads them when it needs them and shows
them as account names when a service has more than one account, unless you turn on **Hide e-mail
addresses** ([Accounts](accounts.md#hiding-e-mail-addresses)).
