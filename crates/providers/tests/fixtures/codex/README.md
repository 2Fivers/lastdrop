# Fixtures: codex

Test inputs for the `codex` provider: `codex app-server` JSON-RPC lines (the `initialize` and
`account/rateLimits/read` responses, notifications in between, error responses) and rollout
`.jsonl` tails with `token_count.rate_limits` events.

**Every fixture here is invented.** Write it by hand from the documented or observed *shape* of
the data, with made-up values. Never copy a real file, session log or response from a machine,
and never paste a real token, account id, e-mail address, prompt text or a path that identifies a
person, not even partly redacted (docs/ARCHITECTURE.md: "Fixtures in tests are invented data,
never copied from a real machine"). Rollout files hold conversations: an invented rollout holds
only the events the parser needs.

- Times: around the readout-spec §13 clock (`2026-10-10T12:42:00+02:00`) unless a test needs
  others.
- Paths: under `C:\Users\tester\.codex\` (see `lastdrop_providers::fakes::Fakes::new`).
- One file per case, named after what it shows: `app-server-rate-limits.jsonl`,
  `app-server-not-signed-in.jsonl`, `rollout-tail-two-windows.jsonl`,
  `rollout-tail-cut-first-line.jsonl`.

Tests load fixtures into `FakeFs` or `FakeSpawner` (`src/fakes.rs`); no test touches the real
file system or starts a process. The one exception is `tests/codex_live.rs`, an `#[ignore]`d
manual smoke test that asks this machine's `codex app-server` and prints only the plan, window
lengths, used percentages and reset times.

The shapes follow `codex app-server generate-json-schema` (codex-cli 0.162) and the rollout
protocol in openai/codex (`RateLimitSnapshot`, `TokenCountEvent`). Each line must be valid JSON
unless the case is about a line that is not (`app-server-rate-limits.jsonl` line 3, the cut
first line of `rollout-tail-cut-first-line.jsonl`); Windows paths inside JSON strings need `\\`.

| File | Shows |
|---|---|
| `app-server-rate-limits.jsonl` | the whole conversation: `initialize`, notifications and a log line in between, `account/read` (ChatGPT, Plus), `account/rateLimits/read` with a 5-hour and a weekly window and a `premium` bucket |
| `app-server-weekly-only.jsonl` | a plan without a 5-hour limit: the weekly window arrives as `primary` |
| `app-server-not-signed-in.jsonl` | `account/read` with `account: null` |
| `app-server-rate-limits-auth-error.jsonl` | `account/rateLimits/read` answers "authentication required" |
| `app-server-rate-limits-server-error.jsonl` | `account/rateLimits/read` answers an internal error |
| `app-server-api-key.jsonl` | an API-key sign-in (no plan windows) |
| `rollout-tail-two-windows.jsonl` | a session: `rate_limits: null`, two `codex` events, a `premium` bucket after them |
| `rollout-tail-cut-first-line.jsonl` | a tail whose first line is cut |
| `rollout-first-line-only.jsonl` | a whole (short) file whose only event is its first line |
| `rollout-plan-null.jsonl` | `plan_type: null` after an event that names the plan |
| `rollout-legacy-resets-in-seconds.jsonl` | Codex 0.44–0.47: `resets_in_seconds` relative to the line time |
| `rollout-legacy-flat.jsonl` | Codex ≤ 0.42: flat `primary_used_percent` … without reset times |
| `rollout-unknown-shape.jsonl` | a `codex` event whose windows have no readable used percent |
| `rollout-session-start.jsonl` | a session that has started but has no `token_count` yet |
