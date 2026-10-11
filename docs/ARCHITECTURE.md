# Lastdrop architecture

How the app is put together, why, and the rules every part follows. Decisions with real
trade-offs live in [`docs/adr/`](adr); the visual and behavioural spec lives in
[`docs/design/`](design).

## Goals that shape everything

1. **Memory.** Under 10 MB private working set (Task Manager's "Memory") with the readout
   visible and the flyout built, flat after the flyout hides. Checked with
   [`tools/measure`](../tools/measure) before every release. Settings and About run in a
   process of their own so they never add to it ("Processes").
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
                                   refresh scheduler, settings and history persistence, the
                                   dialog process
spikes/                     the stack spikes (reference only, not built by the workspace)
tools/measure/              memory and latency measurement
docs/
```

Dependency direction: `app → win, providers → core`. `core` depends on nothing
Windows-specific and has no I/O.

## Processes

Two processes run the same `lastdrop.exe`:

- **The tray process** runs all the time: tray icon, readout, flyout, hover card, refreshes,
  history. Everything below ("Threads") is about it.
- **The dialog process** (`lastdrop.exe --settings`, `--settings-pins` or `--about`) shows the
  settings window or the about window and ends when its last window closes. The tray process
  starts it for "Settings…", "Taskbar readout ▸ Pinned meters…", "About Lastdrop", the flyout's
  gear and a card's "Settings" link. It does not wait for it and keeps no handle to it; it passes
  its foreground right on (`AllowSetForegroundWindow`) so the dialog comes to the front.

Why: the tray process measured 10.1–10.3 MB fresh and 11.9–12.25 MB once Settings had been
opened. Slint's own caches kept only about 0.2 MB of that. The rest, about 1.4 MB, is what
Windows sets up once per thread for a real decorated window with text fields (text services and
IME, DWM, theme, UI Automation), and it stays as long as the process lives. In the dialog process
it goes away with the dialog. Theme Studio (M4) will run there too.

Single instance and messages, all per session ("Single instance" below):

- The dialog process has its own mutex (`Local\Lastdrop.Dialog`). A second launch posts the
  dialog it wants (the message's `WPARAM`) to the running dialog process, which opens it or brings
  it to the front, and exits.
- After Apply the dialog process writes `settings.json` itself (atomic) and posts
  `Lastdrop.SettingsChanged` to the tray process. The tray process re-reads the file and applies
  it (hotkey, services, readout, refresh plan); a missing or broken file is ignored. It does not
  write the file again. It keeps its own pause: the tray menu owns the pause. Apply re-reads the
  file and writes only what the window changed since it opened (or since the last Apply) onto it,
  so what the tray changed meanwhile (pause, pinned meters, readout auto or off) survives; when
  both changed the same setting, the window wins.
- Saving or removing the Meshy key happens in the dialog process (Credential Manager); the same
  message, with another flag, makes the tray process lift Meshy's sign-in stop and read again.
- Autostart is set by the dialog process. A changed hotkey is tested there (registered and
  released at once) before it is saved; the tray process registers it for real.

The dialog process logs to `settings.log`, the tray process to `lastdrop.log`.

## Threads

| Thread | Owns | Never does |
|---|---|---|
| **main** | Slint event loop: flyout and hover card; the tray icon (our own `Shell_NotifyIcon`, version 4); the app state | blocking I/O |
| **readout** | the taskbar readout: a hidden top-level window that receives `TaskbarCreated`, the layered readout window (a topmost `WS_POPUP` over the taskbar, never parented into it; readout-spec §6.1), the WinEvent hooks on the taskbar, its own message loop and drawing | I/O, waiting on locks held by other threads, synchronous cross-process calls while holding a lock |
| **occupancy** | a UI Automation client (MTA) that measures where the taskbar's app buttons are, with connection and transaction timeouts; COM and UI Automation are set up on the first measurement, so a readout that is off never loads them | anything else |
| **refresh workers** | provider fetches (files, HTTP, `codex app-server`), one job per credential source at a time | touching UI objects |
| **writer** | every file write after start-up, one job at a time in the order the main thread queued them: history appends and the daily compaction, settings and state saves (atomic replace); the log has a writer thread of its own | anything but writing files |

Why a separate readout thread: the readout follows Explorer's taskbar (WinEvent hooks, geometry
reads, re-raising itself when the taskbar comes to the top of the topmost band) and must react
whatever the UI thread is doing; a slow Slint frame never delays it, and a hung Explorer never
blocks the flyout. The readout is never parented into the taskbar: a cross-process child window
attaches its thread's input queue to Explorer's, which in the reference tool froze the taskbar's
input (its issues #22 and #92). The readout thread does nothing but draw a small model and handle
a handful of messages.

The dialog process has only its main thread (Slint event loop, settings and about windows) and
the log's writer thread. As a dialog it may wait on the disk: it reads and writes `settings.json`
on its main thread.

Messages between threads: workers send results to the main thread with
`slint::invoke_from_event_loop`; the main thread hands the readout a new immutable
`ReadoutModel` (an `Arc`) and `PostMessage`s it to wake up; the readout posts user actions
(hover, click, right-click) back the same way. No lock is ever held across a Win32 call.

## Data model (`lastdrop-core`)

- **Snapshot** — one reading of one account of one service: provider, account (label, plan),
  `measured_at` (when the service computed it), `fetched_at`, `source` (for example
  `ClaudeOAuthUsage`, `ClaudeDesktopCache`, `ClaudeDesktopHistory`, `CodexAppServer`,
  `CodexRollout`, `CopilotApi`, `MeshyApi`),
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
| Claude | OAuth usage endpoint with Claude Code's token when the file holds a usable one, read-only, never refreshed, at most every 5 min, `Retry-After` honoured → Claude Desktop's own last usage response in its Chromium HTTP cache (`Cache\Cache_Data`, block-file format, read-only and shared, no token; only the answers of Claude Code's organisation when Claude Code names one) → Claude Desktop's `plan-usage-history.json` (no token) → Claude Code's cached usage in `.claude.json`; the most recently measured reading wins, readings under 2 min apart count as one and the earlier source wins |
| Codex | newest rollout `token_count.rate_limits` while younger than 10 min (nothing started) → `codex app-server -c features.plugins=false` (`account/rateLimits/read`, newest bundled or PATH `codex.exe`, no window, 8 s init / 3 s per call; plugins off so no marketplace sync starts; started at most every 15 min per `CODEX_HOME`, its last answer standing in between, earlier for a manual refresh or once a window of that answer has reset; so the service counts as fresh for 15 min plus the poll interval, not from the spec) → newest rollout `token_count.rate_limits` (session files read again only when they changed) |
| Copilot | `GET https://api.github.com/copilot_internal/user` (undocumented; what Copilot's editor clients read) with the GitHub CLI's token from `gh auth token --hostname github.com`, run without a window per fetch (`gh.exe` on `PATH`, else the installer's `GitHub CLI` folder), held in memory for the one request; at most every 5 min; `hosts.yml` in gh's config folder is the credential fingerprint. Each limited `quota_snapshots` entry becomes a calendar-month meter: `premium` (paid plans), `chat` and `completions` (Copilot Free); unlimited and zero-entitlement quotas are left out, so the catalogue has none |
| Meshy | `GET https://api.meshy.ai/openapi/v1/balance` with the key from Credential Manager |

Rules for every provider:
- **Never write, refresh or copy another tool's credentials.** Read, use in memory, drop.
- **Never log a secret** (tokens, keys, cookies, account ids, e-mail addresses). The logger
  redacts known shapes as a second line of defence.
- Undocumented endpoints are polled no more than every 5 minutes per credential, back off on
  429/403 (`Retry-After` honoured up to 24 h), and stop on 401 until the credential file
  changes (or appears: a missing file has a fingerprint too, so signing in lifts the stop).
- Parsers tolerate unknown and missing fields; a missing value is `None`, never 0.
- **Never panic.** Release builds abort on panic (`panic = "abort"`), so no `expect`,
  `unwrap`, `unreachable!` or unchecked indexing on anything read or looked up.
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
  second launch asks the first to open the flyout. The dialog process has its own mutex,
  `Local\Lastdrop.Dialog`. Each of the three channels (open the flyout, open a dialog, settings
  changed) is a message-only listener window of its own class with a registered message, found
  with `FindWindowExW(HWND_MESSAGE, …)`.

## App (`lastdrop`)

- Slint with the **software renderer only** and the `fluent` style ([ADR 0001](adr/0001-rust-and-slint-software-renderer.md)).
  Static CRT. The About window shows Slint's `AboutSlint` (licence condition).
- **Slint licence rule:** Slint is used under its Royalty-free License 2.0, which stays free
  whether Lastdrop is closed or later open-sourced, but forbids an app that exposes Slint's APIs.
  So themes, layouts and provider definitions are **data** (JSON), never `.slint` code loaded
  at runtime, and nothing user-supplied reaches a Slint interpreter.
- **Refresh scheduler:** per account, the provider's `min_interval` and the user's interval;
  adaptive (faster while the flyout is open, slower when idle: after 10 minutes without keyboard
  or mouse input, a locked session included, the interval × 3 up to 30 minutes; the minute tick
  notices the user going or coming back and replans); poll again shortly after a window's reset
  time; one job per credential source in flight.
- **Persistence:** settings in `%APPDATA%\Lastdrop\settings.json` (atomic write, no
  secrets); history in `%LOCALAPPDATA%\Lastdrop\` (thinned to one sample per meter per
  5 minutes, 90 days); `%LOCALAPPDATA%\Lastdrop\state.json` (what the app remembers that is
  not a setting: the meters a source has reported, so their room in the readout survives a
  restart; meter keys and names only, no secrets); logs in `%LOCALAPPDATA%\Lastdrop\logs`
  (rotated, local only; `lastdrop.log` for the tray process, `settings.log` for the dialog
  process). Start-up reads them on the main thread before the event loop; every later write is
  queued for the writer thread, so the UI thread never waits on the disk. The one later read is
  `settings.json` when the dialog process says it changed (a few kilobytes). The dialog process
  writes `settings.json` itself; it never writes the state file or history.
- **No telemetry.** The only network traffic is to the services whose usage is being read.

## Quality bar

- `cargo fmt`, `cargo clippy -D warnings`, `cargo test` on every change; CI on
  `windows-latest`.
- Core logic and parsers are covered by unit tests; placement by the layout matrix.
- Before a release: `tools/measure` within budget, a 24-hour soak with no growth in memory,
  GDI or USER handles, and the manual multi-monitor matrix.
