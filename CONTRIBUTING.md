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
accounts: `lastdrop.exe --demo --preview-flyout flyout.png` (and the other previews below).
Keep `--demo`: without it the previews read your own accounts and history.

### Previews

A preview renders a window headless (nothing appears on screen, nothing takes the focus) and
saves it as a PNG. With `--demo` the data is invented and no file is read or written.

| Flag | Renders |
|---|---|
| `--preview-flyout <png>` | the flyout |
| `--preview-hovercard <png>` | the hover card |
| `--preview-settings <png>` | the settings window: a PNG per view and size (below) |
| `--preview-about <png>` | the about window |
| `--preview-history <png>` | the usage history window |
| `--preview-theme-studio <png>` | Theme Studio, starting from `--theme` |

| Option | Effect |
|---|---|
| `--language <tag>` | the language of a file in `crates/core/locales` (`en`, `sv`, …); with `--demo` its regional format too. Default: English with `--demo`. |
| `--dark` | Windows' dark app mode. |
| `--scale <factor>` | the display scale, 0.5 to 4: `1.5` is 150 %, `2` is 200 %. The PNG has that many pixels per logical pixel. |
| `--contrast` | the settings window in a contrast theme (Windows' "Night sky" colours). |
| `--theme <id>` | the flyout's and the hover card's theme (themes-spec). |
| `--demo-scenario <normal\|warning\|stale\|signin\|lastcredit>` | which state the demo's services are in (`lastcredit`: Meshy's last credit). |

`--preview-settings <dir>\settings.png` writes `<dir>\settings-<view>-<w>x<h>.png` for every view
at the window's preferred size, 880 × 680, and its smallest, 680 × 520 (logical px; the files
are `--scale` times that):

| View | Shows |
|---|---|
| `services`, `display`, `notifications`, `general` | each page as it opens |
| `accounts` | the first run's deep link: the accounts with a folder found on this PC |
| `keys` | Meshy's and OpenCode Go's API keys expanded |
| `pins` | the tray menu's "Pinned meters…": the taskbar's meters being chosen |
| `errors` | General with a refused shortcut and the InfoBar of a save that failed |
| `updates` | the end of General: the shortcut on and what Check now found |

For example, the Swedish settings window in dark mode at 150 %:

```
lastdrop.exe --demo --language sv --dark --scale 1.5 --preview-settings out\settings.png
```

The README's screenshots come from the same demo data. `pwsh tools\screenshots\screenshots.ps1`
builds the app and renders them again into `docs\assets\screenshots`, light and dark (the first
run installs a pinned [oxipng](https://github.com/shssoichiro/oxipng) into `target\tools`); with
`-Check` it only tells whether they are up to date. When a change alters the flyout, the hover
card, the history window or the demo data, commit the new pictures with it. The `screenshots`
check in CI (not a required one) runs `-Check` on pull requests that touch the crates, and
uploads the pictures it rendered when they differ.

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
