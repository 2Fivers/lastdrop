# Contributing to Lastdrop

Thank you for helping. Bug reports, translations, new services and fixes are all welcome.

## Before you start

- **Bugs:** open an issue with the Lastdrop version, the Windows version and what you saw.
  Logs are in `%LOCALAPPDATA%\Lastdrop\logs`; Lastdrop masks tokens in them, but read a log
  before you attach it and remove anything personal.
- **Security problems:** never in an issue; see [SECURITY.md](SECURITY.md).
- **Bigger changes** (a new service, a new window, a new dependency): open an issue first so we
  can agree on the approach before you spend time on it.
- **A service with a simple balance or usage API** may not need any Rust: see
  [custom-services-spec](docs/design/custom-services-spec.md) and the examples in
  `crates/app/providers`.
- **A new language:** see [crates/core/locales/README.md](crates/core/locales/README.md).

## How the project works

- [docs/ROADMAP.md](docs/ROADMAP.md) has the principles (light on memory, native Windows 11,
  no telemetry, data stays local, never break the user's other tools) and what is planned.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains the crates and their rules, and
  `docs/design` has a spec for each feature. Code points to the spec section it implements;
  when a change alters behaviour, update the spec in the same pull request.
- Code, comments, docs and commit messages are in English.
- Lastdrop runs on Windows 11 only, so you build and test it on Windows.

## Building and checking

The Rust version is pinned in `rust-toolchain.toml`; rustup installs it on the first build.

```
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release -p lastdrop --locked
```

All four must pass; CI runs the same on every pull request. To see the UI without real
accounts: `lastdrop.exe --demo --preview-flyout flyout.png` (and `--preview-hovercard`,
`--preview-settings`, `--preview-about`, `--preview-history`).

## Rules for changes

- **No real credentials anywhere:** not in tests, fixtures, screenshots or issues. Tests use
  invented tokens and answers in the shape of the real ones.
- **Never refresh or rewrite another tool's sign-in.** Lastdrop only reads them.
- **Nothing leaves the machine** except requests to the service whose usage is read, and the
  update check. No analytics, no telemetry, no new hosts without discussion.
- **Dependencies** are pinned to exact versions (`=x.y.z`) in the workspace `Cargo.toml`. Ask
  before adding one; every dependency is weighed against memory, binary size and supply-chain
  risk.
- **Slint licence rule:** themes, layouts and provider definitions are data, never `.slint`
  code loaded at run time, and the About window keeps the `AboutSlint` widget
  ([ARCHITECTURE](docs/ARCHITECTURE.md)).
- Keep pull requests focused: one change, with its tests and its spec update.

## Review

Pull requests from new contributors run CI only after a maintainer approves the run. Changes to
the build (`build.rs`, `.cargo`, `Cargo.toml`, `Cargo.lock`, the toolchain), to the workflows,
to how tokens are read or sent, to the provider definitions and to the updater or installer are
reviewed with extra care (see `.github/CODEOWNERS`), so expect questions there.

## Licence

Lastdrop is licensed under either of the Apache License 2.0 or the MIT license, at your option.
Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
Lastdrop by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
