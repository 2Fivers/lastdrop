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
file system or starts a process.
