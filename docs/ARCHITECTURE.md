# Lastdrop architecture

How the app is put together, why, and the rules every part follows. Decisions with real
trade-offs live in [`docs/adr/`](adr); the visual and behavioural spec lives in
[`docs/design/`](design).

## Goals that shape everything

1. **Memory.** Under 10 MB private working set (Task Manager's "Memory") with the readout
   visible and the flyout built, flat after the flyout hides. Checked with
   [`tools/measure`](../tools/measure) before every release.
2. **Never disturb the shell or other tools.** Nothing we do may hang Explorer, steal focus,
   cover taskbar buttons, or sign the user out of Claude Code, Codex or anything else.
3. **Honest data.** Every number carries its source and age. Unknown is shown as unknown,
   never as 0 %.
4. **Testable without Windows UI.** All logic that decides *what* to show is pure Rust in
   `lastdrop-core` and unit-tested; the Windows layers only draw what they are given.

## Workspace

```
Cargo.toml                 workspace: shared dependency versions, lints, release profile
crates/
  core/       lastdrop-core        pure: model, pace and forecast, severity, readout and tray
                                   models, text grammar, settings schema, refresh policy
  providers/  lastdrop-providers   one module per service; I/O behind small traits so tests
                                   run on fixtures; no UI, no Win32 windows
  win/        lastdrop-win         Win32: drawing (Direct2D + DirectWrite), tray icon image,
                                   taskbar readout window and placement, taskbar watcher,
                                   flyout placement, backdrop and theme, autostart,
                                   Credential Manager, single instance
  app/        lastdrop (binary)    Slint UI (flyout, hover card, settings, about), wiring,
                                   refresh scheduler, settings and history persistence
spikes/                     the stack spikes (reference only, not built by the workspace)
tools/measure/              memory and latency measurement
docs/
```

Dependency direction: `app → win, providers → core`. `core` depends on nothing
Windows-specific and has no I/O.

## Threads

| Thread | Owns | Never does |
|---|---|---|
| **main** | Slint event loop: flyout, hover card, settings and about windows; the tray icon (our own `Shell_NotifyIcon`, version 4); the app state | blocking I/O |
| **readout** | the taskbar readout: a hidden top-level window that receives `TaskbarCreated`, the layered readout window (a topmost `WS_POPUP` over the taskbar, never parented into it; readout-spec §6.1), the WinEvent hooks on the taskbar, its own message loop and drawing | I/O, waiting on locks held by other threads, synchronous cross-process calls while holding a lock |
| **occupancy** | a UI Automation client (MTA) that measures where the taskbar's app buttons are, with connection and transaction timeouts | anything else |
| **refresh workers** | provider fetches (files, HTTP, `codex app-server`), one job per credential source at a time | touching UI objects |

Why a separate readout thread: the readout follows Explorer's taskbar (WinEvent hooks, geometry
reads, re-raising itself when the taskbar comes to the top of the topmost band) and must react
whatever the UI thread is doing; a slow Slint frame never delays it, and a hung Explorer never
blocks the flyout. The readout is never parented into the taskbar: a cross-process child window
attaches its thread's input queue to Explorer's, which in the reference tool froze the taskbar's
input (its issues #22 and #92). The readout thread does nothing but draw a small model and handle
a handful of messages.

Messages between threads: workers send results to the main thread with
`slint::invoke_from_event_loop`; the main thread hands the readout a new immutable
`ReadoutModel` (an `Arc`) and `PostMessage`s it to wake up; the readout posts user actions
(hover, click, right-click) back the same way. No lock is ever held across a Win32 call.

## Data model (`lastdrop-core`)

- **Snapshot** — one reading of one account of one service: provider, account (label, plan),
  `measured_at` (when the service computed it), `fetched_at`, `source` (for example
  `ClaudeDesktopHistory`, `ClaudeOAuthUsage`, `CodexAppServer`, `CodexRollout`, `MeshyApi`),
  `meters`, `balances`, and `health` (`Ok`, `Stale`, `SignInRequired`, `Error`).
- **Meter** — a usage window: stable id (`five_hour`, `weekly`, `weekly:fable`, …), label,
  `used` as a share (1.0 = the limit; may exceed 1 when the service says so), whether the limit
  is reached, window length (fixed, calendar month or unknown), `resets_at` (optional) and
  whether the window starts on use (Claude's 5-hour session).
- **Balance** — an amount that only goes down until topped up: id, label, amount, unit
  (credits or a currency).
- **Assessment** — pure function of a meter, the clock and (later) history: elapsed share,
  on/ahead of pace, forecast from the recent rate, run-out time, severity (`Ok`, `Warning`,
  `Critical`, `Exhausted`) with hysteresis; unknown stays unknown. The formulas and thresholds are the ones in
  [`docs/design/readout-spec.md`](design/readout-spec.md); each is a unit test.
- **ReadoutModel / TrayModel / FlyoutModel** — exactly what each surface draws, including
  every string (already formatted by the text grammar) and the UI Automation name. Surfaces
  contain no formatting or business logic.
- **Settings** — versioned `serde` schema with defaults; unknown fields are kept.

## Providers (`lastdrop-providers`)

```rust
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    /// One fetch for one account. Must finish or fail within the context's deadline.
    fn fetch(&self, ctx: &FetchContext, account: &AccountSource) -> Result<Snapshot, FetchError>;
    /// Lower bound between fetches for one credential source (rate limits, politeness).
    fn min_interval(&self) -> Duration;
}
```

`FetchContext` gives the clock, an HTTP client, file access, the secret store and a process
spawner — all traits, so tests replace them with fixtures. Each provider tries its sources in a
fixed order and returns the freshest success; every attempt is logged (without secrets).

| Provider | Sources, in order |
|---|---|
| Claude | Claude Desktop's `plan-usage-history.json` (no token) → OAuth usage endpoint with Claude Code's token, read-only, never refreshed, at most every 5 min, `Retry-After` honoured → Claude Code's cached usage in `.claude.json` |
| Codex | `codex app-server` (`account/rateLimits/read`, newest bundled or PATH `codex.exe`, no window, 8 s init / 3 s per call) → newest rollout `token_count.rate_limits` |
| Meshy | `GET https://api.meshy.ai/openapi/v1/balance` with the key from Credential Manager |

Rules for every provider:
- **Never write, refresh or copy another tool's credentials.** Read, use in memory, drop.
- **Never log a secret** (tokens, keys, cookies, account ids, e-mail addresses). The logger
  redacts known shapes as a second line of defence.
- Undocumented endpoints are polled no more than every 5 minutes per credential, back off on
  429/403, and stop on 401 until the credential file changes.
- Parsers tolerate unknown and missing fields; a missing value is `None`, never 0.
- Fixtures in tests are invented data, never copied from a real machine.

## Windows layer (`lastdrop-win`)

- **Drawing:** Direct2D on a WIC bitmap (software) with DirectWrite text, premultiplied BGRA,
  pushed with `UpdateLayeredWindow`. Same engine as the taskbar clock; grayscale antialiasing.
  Used by the readout and by the tray icon renderer (16/20/24/32 px images).
- **Readout placement** is a pure function `place(geometry, size, prefs) -> Placement`
  (`Docked(rect)`, `Floating(rect)`, `Hidden`) from recorded taskbar geometry: taskbar and tray
  rects per monitor, DPI, auto-hide, alignment, app-button extents from the occupancy thread.
  It is unit-tested against a matrix of recorded layouts. Gathering geometry uses
  `GetWindowRect`, never `SHAppBarMessage` (which can deadlock when Explorer hangs).
- **Taskbar events:** `EVENT_OBJECT_LOCATIONCHANGE` (out of context, filtered to the taskbar
  and tray), `EVENT_SYSTEM_FOREGROUND` and `EVENT_OBJECT_REORDER` on the taskbar (re-assert
  topmost), `TaskbarCreated`, `WM_DISPLAYCHANGE`, `WM_SETTINGCHANGE` and `WM_DPICHANGED` (the
  readout is a top-level window, so `WM_DPICHANGED_AFTERPARENT` never reaches it; readout-spec
  §6.9), debounced; no polling loop for geometry. After an Explorer restart the readout is
  rebuilt in-process.
- **Flyout placement:** `Shell_NotifyIconGetRect` → `CalculatePopupWindowPosition` with the
  work area (from the spike).
- **Theme:** taskbar light/dark (`SystemUsesLightTheme`), app light/dark, accent, transparency,
  high contrast; change notifications re-render.
- **Autostart:** `HKCU\…\Run\Lastdrop`. **Secrets:** Credential Manager, generic credentials
  named `Lastdrop:<provider>:<account>:<name>`. **Single instance:** `Local\Lastdrop` mutex; a
  second launch asks the first to open the flyout.

## App (`lastdrop`)

- Slint with the **software renderer only** and the `fluent` style ([ADR 0001](adr/0001-rust-and-slint-software-renderer.md)).
  Static CRT. The About window shows Slint's `AboutSlint` (licence condition).
- **Slint licence rule:** Slint is used under its Royalty-free License 2.0, which stays free
  whether Lastdrop is closed or later open-sourced, but forbids an app that exposes Slint's APIs.
  So themes, layouts and provider definitions are **data** (JSON), never `.slint` code loaded
  at runtime, and nothing user-supplied reaches a Slint interpreter.
- **Refresh scheduler:** per account, the provider's `min_interval` and the user's interval;
  adaptive (faster while the flyout is open, slower when idle); poll again shortly after a
  window's reset time; one job per credential source in flight.
- **Persistence:** settings in `%APPDATA%\Lastdrop\settings.json` (atomic write, no
  secrets); history in `%LOCALAPPDATA%\Lastdrop\` (thinned to one sample per meter per
  5 minutes, 90 days); logs in `%LOCALAPPDATA%\Lastdrop\logs` (rotated, local only).
- **No telemetry.** The only network traffic is to the services whose usage is being read.

## Quality bar

- `cargo fmt`, `cargo clippy -D warnings`, `cargo test` on every change; CI on
  `windows-latest`.
- Core logic and parsers are covered by unit tests; placement by the layout matrix.
- Before a release: `tools/measure` within budget, a 24-hour soak with no growth in memory,
  GDI or USER handles, and the manual multi-monitor matrix.
