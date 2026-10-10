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
- Times: around the readout-spec §13 clock (`2026-10-10T12:42:00+02:00`) unless a test needs
  others.
- Paths: under `C:\Users\tester\` (see `lastdrop_providers::fakes::Fakes::new`).
- One file per case, named after what it shows: `usage-200-two-windows.json`,
  `usage-200-five-hour-null-reset.json`, `usage-401.json`, `credentials-expired.json`.

Tests load fixtures into `FakeFs` or `FakeHttp` (`src/fakes.rs`); no test touches the real file
system or network. Each provider confirms its R§2 meter definitions (`L`, `starts_on_use`) in a
unit test against such a fixture.
