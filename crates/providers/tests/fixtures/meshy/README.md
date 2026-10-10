# Fixtures: meshy

Test inputs for the `meshy` provider: bodies of `GET https://api.meshy.ai/openapi/v1/balance`
(200 with a balance, 200 without one, 401, 429 with `Retry-After`, 5xx).

**Every fixture here is invented.** Write it by hand from the documented shape of the response,
with made-up values. Never copy a real response from a machine, and never paste a real API key
or account detail, not even partly redacted (docs/ARCHITECTURE.md: "Fixtures in tests are
invented data, never copied from a real machine").

- Keys: obviously fake strings such as `msy-test-key-not-real`, stored in `FakeSecrets`.
- One file per case, named after what it shows: `balance-200.json`,
  `balance-200-missing-field.json`, `balance-401.json`.

Tests load fixtures into `FakeHttp` (`src/fakes.rs`); no test touches the network or Credential
Manager.
