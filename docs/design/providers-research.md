# Lastdrop — where the M2 services' numbers come from

Research notes behind the Cursor, Antigravity, OpenCode Go and Grok providers (2026-10-10, checked
again on 2026-10-11). Each read path was taken from the vendor's own open-source code or official
docs where one exists, else from the reference tools' source. Lastdrop's code is its own: nothing
of the reference tools was copied, so their notices do not apply to it.

| Source | Commit | Licence |
|---|---|---|
| steipete/CodexBar | 6e118bd | MIT |
| nesszer/Win-CodexBar | 603681a | MIT (with a NOTICE for code ported from codexcontrol) |
| CodeZeno/Claude-Code-Usage-Monitor | 623e891 | MIT |
| xai-org/grok-build (xAI's own CLI) | 2bdd1d6 | Apache-2.0 |
| anomalyco/opencode (OpenCode's own console) | 055d95b | MIT |

## Grok (SuperGrok, Grok Build)

- `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits`: the request xAI's own CLI makes
  for its `/usage` panel (`xai-grok-shell/src/extensions/billing.rs`). Not a documented public API.
- Headers: `Authorization: Bearer <key>`, `X-XAI-Token-Auth: xai-grok-cli`, `x-userid`. The CLI
  also sends its version and a client mode (`interactive` or `headless`); other tools leave those
  out.
- Sign-in: `auth.json` in `GROK_HOME` (`~\.grok`), or `GROK_AUTH_PATH`. It is an `AuthStore`
  (`xai-grok-login/src/model.rs`): scope → `GrokAuth {key, auth_mode (oidc | api_key | external
  | web_login), user_id, expires_at (chrono, RFC 3339), refresh_token, email, …}`. The scope
  `xai::api_key` holds a developer key, which cannot read the subscription's pool. The CLI renews
  the token itself; Lastdrop never does.
- Answer: protobuf-JSON `BillingConfig` (zero values left out): `creditUsagePercent` (0–100),
  `currentPeriod {type: USAGE_PERIOD_TYPE_WEEKLY | _MONTHLY, start, end}`, `onDemandCap`,
  `onDemandUsed`, `prepaidBalance` as `{val}` in US cents (a string or a number; `{}` is 0),
  `history`. A current period without a percent is 0 %, as the CLI reads it (proto3); other tools
  differ on this.

## OpenCode Go

- `GET https://opencode.ai/zen/go/v1/usage` with `Authorization: Bearer <Go API key>`
  (`packages/console/app/src/routes/zen/go/v1/usage.ts`). Not in the public docs.
- Answer: `{usage: {rolling | weekly | monthly: {status: "ok" | "rate-limited", percent,
  resetsAt}}}`. `percent` is 0–100 and may be fractional for keys the closed backend answers
  (new `oc_sk_` keys and migrated workspaces go through `proxyInference`). The weekly window
  resets on Monday 00:00 UTC, the monthly one on the subscription's day; an unused rolling window
  reports its reset a whole window from now.
- Errors: 401 `AuthError`, 403 `EntitlementError` (no Go subscription), 503 when the backend is
  down, a backend 404 rewritten to 400.
- Key: the user's, or OpenCode's own `%USERPROFILE%\.local\share\opencode\auth.json` entry
  `"opencode-go": {"type": "api", "key"}` (official troubleshooting docs). Reading that file is
  Lastdrop's own choice; the reference tools ask for the key.
- Plans (docs): Go $10/month, Go Plus $40/month; per-model monthly caps, the 5-hour window 20 %
  and the week 50 % of the month.

## Google Antigravity

- `agy -p /usage --output-format json` (print mode since agy 1.1.11, official changelog; UTC
  times since 1.2.15). agy renews its own sign-in (Windows Credential Manager), so Lastdrop
  handles no Google token.
- Shape (from the reference tools' parsers; not documented): `{status: "SUCCESS", command: {name:
  "usage", data: {groups: [{name | displayName, buckets: [{id | bucketId, name | displayName,
  window: "5h" | "weekly", remaining_fraction | remainingFraction | remaining.remainingFraction |
  remaining {case: "remainingFraction", value}, reset_time | resetTime, disabled}]}]}}}`. A
  disabled bucket or one without a fraction is unknown, not 0 %.
- Install: `%LOCALAPPDATA%\agy\bin\agy.exe` (Google's `install.ps1`); `-d` installs elsewhere, so
  `PATH` too; `ANTIGRAVITY_CLI_PATH` names it outright. An older agy is refused: it could take
  `-p /usage` as a prompt.
- Not used: the desktop app's local language server (CSRF-gated since agy 1.2.2) and Google's
  `v1internal` API, which other tools call with an OAuth client secret scraped from the binaries.
- Plans (docs): Ultra and AI Pro refresh every 5 hours up to a weekly limit; other individual
  users have one weekly quota.

## Cursor

- `GET https://cursor.com/api/usage-summary` with `Cookie: WorkosCursorSessionToken=<user>%3A%3A<token>`,
  the dashboard's own endpoint. `<user>` is the token's `sub` after its last `|` (CodexBar and
  CodeZeno; Win-CodexBar sends the whole `sub`).
- Token: `%APPDATA%\Cursor\User\globalStorage\state.vscdb`, `ItemTable` key
  `cursorAuth/accessToken`, an HS256 JWT that lives 60 days. Nobody renews it but the Cursor app.
  The file can be 1.6 GB: Lastdrop reads it page by page with its own read-only SQLite reader,
  never copies it (CodeZeno copies the whole file to `%TEMP%` on any read error).
- Answer: `billingCycleStart`, `billingCycleEnd`, `membershipType`, `individualUsage.plan
  {used, limit (cents), autoPercentUsed, apiPercentUsed, totalPercentUsed}`,
  `individualUsage.onDemand {enabled, used, limit}`, `individualUsage.overall` (enterprise
  members), `teamUsage`. Percents are 0–100. Pools (pricing page): "Cursor Models" (Auto) and
  "Other Models"; the India-only Start plan has no second pool.
- Teams: the official Admin API (`POST https://api.cursor.com/teams/spend`, an admin key with
  `admin:*`) is for team admins only; not used.

## Open questions

- OpenCode Go: does a Go Plus key answer with the same three windows?
- Cursor: must the cookie's user be the bare `user_…` or the whole `sub`?
- Grok: how do X Premium subscriptions map onto the credit pool?
