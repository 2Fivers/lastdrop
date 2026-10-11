# Fixtures: copilot

Test inputs for the `copilot` provider: bodies of `GET https://api.github.com/copilot_internal/user`
(Copilot Free, a paid plan, overage, a used-up quota, a body without `quota_snapshots`, 401, 404,
429 with `Retry-After`, 5xx).

**Every fixture here is invented.** Write it by hand from the shape of the response, with made-up
values. Never copy a real response from a machine, and never paste a real token, login, user id,
tracking id or e-mail address, not even partly redacted (docs/ARCHITECTURE.md: "Fixtures in
tests are invented data, never copied from a real machine").

- Tokens: obviously fake strings such as `gho_test_token_not_real`, printed by `FakeSpawner` as
  the output of `gh auth token`.
- One file per case, named after what it shows: `user-200-free.json`, `user-401.json`.

Tests load fixtures into `FakeHttp` (`src/fakes.rs`); no test touches the network, the GitHub CLI
or Credential Manager.
