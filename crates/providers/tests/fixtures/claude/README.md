# Fixtures: claude

Test inputs for the `claude` provider: Claude Desktop's `plan-usage-history.json`, the OAuth
usage endpoint's JSON body, Claude Code's `.credentials.json` and the cached usage in
`.claude.json`.

**Every fixture here is invented.** Write it by hand from the documented or observed *shape* of
the data, with made-up values. Never copy a real file, response or log from a machine, and never
paste a real token, API key, cookie, account id, e-mail address, organisation name or a path that
identifies a person, not even partly redacted (docs/ARCHITECTURE.md: "Fixtures in tests are
invented data, never copied from a real machine").

- Tokens: obviously fake strings such as `test-token-not-real`.
- Ids: `00000000-0000-4000-8000-00000000000N`; e-mail addresses only under `example.invalid`.
- Times: around the readout-spec §13 clock (`2026-10-10T12:42:00+02:00`) unless a test needs
  others. Epoch milliseconds used: `1791628920000` = 10:42Z (now), `1791628320000` = 10:32Z,
  `1791618120000` = 07:42Z, `1791655200000` = 18:00Z, `1791619200000` = 08:00Z.
- Paths: under `C:\Users\tester\` (see `lastdrop_providers::fakes::Fakes::new`).
- One file per case, named after what it shows.

Tests load fixtures into `FakeFs` or `FakeHttp` (`src/fakes.rs`); no test touches the real file
system or network. `tests/claude.rs` confirms the R§2 meter definitions (`L`, `starts_on_use`)
against these fixtures.

## Shapes (key names as observed; values invented)

- `plan-usage-history.json` (Claude Desktop, `%APPDATA%\Claude`): `{version: 2, samples: [{t:
  epoch ms, org, u: {fh, sd, so, sn, oa, cw, om, op, xu}}]}`, percentages, **no reset times**.
- `GET /api/oauth/usage` body: `five_hour` / `seven_day` `{utilization, resets_at, …}` or null;
  `limits[] {kind: session | weekly_all | weekly_scoped, group, percent, severity, resets_at,
  scope: null | {model: {id, display_name}, surface}, is_active}`; legacy `seven_day_opus` /
  `seven_day_sonnet`; plus keys Lastdrop ignores (`extra_usage`, `spend`, `iguana_necktie`,
  `seven_day_omelette`, `seven_day_breakdown`, `weekly_scoped_shares`, codenames).
- `.credentials.json`: `claudeAiOauth {accessToken, refreshToken, expiresAt (epoch ms),
  refreshTokenExpiresAt, scopes[], subscriptionType, rateLimitTier}`, `organizationUuid`,
  `mcpOAuth {…}`. Claude Code hosted by Claude Desktop leaves `accessToken: ""`, `expiresAt: 0`.
- `.claude.json`: `cachedUsageUtilization {fetchedAtMs, accountUuid, utilization: {usage body}}`
  among `projects`, `oauthAccount` and many other keys.

## Files

| File | Shows |
|---|---|
| `usage-200-two-windows.json` | minimal body: 5-hour and weekly |
| `usage-200-current.json` | today's shape: `limits[]` with a `Fable` weekly, a surface scope, an unknown entry, `extra_usage`, `spend`, codenames, an unknown window |
| `usage-200-five-hour-null-reset.json` | between sessions: 5-hour at 0 % with `resets_at: null`; unused per-model weekly |
| `usage-200-five-hour-null.json` | `five_hour: null` (omitted, never 0 %); weekly over 100 % |
| `usage-200-legacy-opus.json` | the old shape: `seven_day_opus` / `seven_day_sonnet`, no `limits` |
| `usage-200-limits-only.json` | no top-level windows: `limits[]` only, with the generic "All models" scope |
| `usage-200-no-windows.json` | a 200 with no R§2 window (Parse error) |
| `usage-401.json`, `usage-429.json` | error bodies |
| `credentials-valid.json` | usable token, `max` plan, plus refresh and MCP tokens that must never surface |
| `credentials-expired.json` | expired token, `pro` plan |
| `credentials-empty-token.json` | the Desktop-hosted layout: empty token, `expiresAt: 0` |
| `credentials-inference-only.json` | a `setup-token`-style token without `user:profile` |
| `credentials-no-claude-sign-in.json` | only MCP entries |
| `plan-usage-history-v2.json` | three ascending samples with ignored keys |
| `plan-usage-history-messy.json` | unordered samples, nulls, strings, a newer sample without R§2 windows, an untimed sample, a newer `version` |
| `plan-usage-history-empty.json` | no samples yet |
| `claude-state.json` | `.claude.json` with account details and project history around the cache |
| `claude-state-no-cache.json` | no `cachedUsageUtilization` |
| `claude-state-flat-cache.json` | the cached usage without the `utilization` wrapper or `fetchedAtMs` |
