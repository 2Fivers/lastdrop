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
                                   models, notices, text grammar, settings schema, refresh
                                   policy; locales/ holds the strings, one TOML file per
                                   language
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
- **The dialog process** (`lastdrop.exe --settings`, `--settings-pins`, `--settings-accounts`,
  `--history[=<tag>]` or `--about`) shows the settings window, the usage history window
  ([charts-spec](design/charts-spec.md)) or the about window and ends when its last window
  closes. The tray process starts it for "Settings…", "Usage history…", "Taskbar readout ▸ Pinned
  meters…", "About Lastdrop", the flyout's gear, a card's "Settings" link and "Usage history…"
  (with the card's service as the tag) and, on the first run, when Claude Code or Codex has more
  folders than its default one (`--settings-accounts`, accounts-spec §6). It
  does not wait for it and keeps no handle to it; it passes
  its foreground right on (`AllowSetForegroundWindow`) so the dialog comes to the front.

Why: while Settings ran in the tray process, it measured 10.1–10.3 MB fresh and 11.9–12.25 MB
once Settings had been opened. Slint's own caches kept only about 0.2 MB of that. The rest, about
1.4 MB, is what Windows sets up once per thread for a real decorated window with text fields
(text services and IME, DWM, theme, UI Automation), and it stays as long as the process lives. In
the dialog process it goes away with the dialog. Theme Studio (M4) will run there too.

Measured on 2026-10-16 with `tools/measure/measure.ps1 -Variants lastdrop-app` (the app's own
`--bench` timeline, five runs; Windows 11 build 26200 at 100 % scale, readout visible, four
services signed in). Tray process only, private working set:

| Moment on the bench timeline | MB, median (min–max) |
|---|---|
| Fresh: readout visible, flyout built but not yet shown | 8.95 (8.91–9.19) |
| Flyout shown, then hidden | 9.55 (9.53–9.70) |
| Shown and hidden a second time | 9.50 (9.47–9.59) |
| Settings open (the dialog process: 4.9 MB of its own) | 9.65 (9.57–9.70) |
| Settings closed again | 9.62 (9.57–9.70) |

Every moment is under the 10 MB budget: by 0.35 MB at the highest median (Settings open) and by
0.13 MB at the highest single sample (9.87 MB, during start-up's first fetches). Hiding the flyout
changes nothing or lowers it a little, and opening and closing Settings moves the tray process by
about 0.1 MB, so the split does its job.

The heaps give their free blocks back (`HeapOptimizeResources`: a real decommit, not a
working-set trim) a second after a refresh round settles and after the flyout is shown or a popup
hidden (`wiring::release_free_heap_soon`). Start-up's fetches and parsing leave about 1.4 MB of
free blocks committed, and drawing the flyout's first frame a little more; the call gives back
about 0.6 MB of it. Without it the same timeline measured 9.5 MB fresh and 10.0–10.2 MB once the
flyout had been shown (up to 10.4 in single runs); with the call only after a settled round, phase
medians of 9.75–9.87 MB, one run reaching 10.05.

Where the rest goes (`tools/measure/map-app.ps1` on a 10.2 MB run before that call, the flyout
hidden after two shows): the process heap 5.6 MB (4.2 MB live blocks and the free blocks above;
the run-to-run spread is here), other heaps 0.7 MB, private pages of DLL images 1.8 MB, the
flyout's and hover card's pixel buffers 1.0 MB (kept while the windows are hidden; they grow with
the square of the scale, so above 100 % the margin shrinks or goes), 20 thread stacks 0.5 MB, the
rest 0.5 MB. By feature, the taskbar readout costs about 2.5 MB (`--no-readout`: 7.6–7.8 MB)
against the 2 MB the roadmap allows it: about 0.7 MB for the occupancy thread's UI Automation, the
rest on the readout thread (its own Direct2D software target and DirectWrite text, its windows).

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
| **readout** (one per taskbar with "every taskbar" on, [every-taskbar-spec](design/every-taskbar-spec.md); only the primary one forwards system broadcasts) | the taskbar readout: a hidden top-level window that receives `TaskbarCreated`, the layered readout window (a topmost `WS_POPUP` over the taskbar, never parented into it; readout-spec §6.1), the WinEvent hooks on the taskbar, its own message loop and drawing | I/O, waiting on locks held by other threads, synchronous cross-process calls while holding a lock |
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

The dialog process has its main thread (Slint event loop, settings, usage history and about
windows), the log's writer thread and, while the usage history window reads Claude Code's or
Codex's session logs for tokens and cost, a `tokens` thread that reads up to four files at once
([tokens-spec](design/tokens-spec.md) §4). As a dialog it may wait on the disk: it reads and writes
`settings.json` on its main thread, and the usage history window reads the selected service's
history files there (read-only, while the tray process may append to them; a line still being
written does not parse and is skipped).

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
- **Notices** — after each evaluation, what is new to announce: a level reached, a window used
  up or reset, a balance running low ([`docs/design/notifications-spec.md`](design/notifications-spec.md)).
  A pure function of the evaluation and the memory of what was announced, worded into one
  `NotificationModel` per evaluation.
- **Strings** — every user-visible string is a resource (`grammar::strings`), one table per
  language: English and Swedish. Each table is a TOML file in `crates/core/locales`, with
  `en.toml` as the source. Core's build script checks every file and compiles them into
  `&'static str` arrays: each key of `en.toml` is there, with the English placeholders and the
  plural forms of the language's CLDR rule (`grammar::plural`). So a language is one file and
  costs nothing at run time ([`crates/core/locales/README.md`](../crates/core/locales/README.md)).
  The language is the user's setting, else the first of Windows' display languages that has a
  table, else English; numbers, dates and times follow the regional format on their own. Providers name meters in English (that is what readings, the
  history and the state file keep), and the app renames them in the chosen language on every
  evaluation (`grammar::meters`). The Slint windows have no text of their own: Rust passes
  each one its texts (the flyout's labels, the settings window's `SettingsTexts`, the about
  window's `AboutTexts`), and the settings window switches language when Apply changes it.
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
| Cursor | `GET https://cursor.com/api/usage-summary` (the dashboard's own, undocumented) with the session cookie built from the token the Cursor app keeps in `state.vscdb` (`ItemTable`, `cursorAuth/accessToken`), read with Lastdrop's own read-only SQLite reader (`lastdrop_providers::sqlite`: page by page through `Fs::open_at`, never the whole file, never a copy, the file can be 1.6 GB; the committed frames of `state.vscdb-wal` win, as Cursor keeps the database in WAL mode; a read torn by Cursor writing is a passing failure); an expired session stops until the token changes (its hash is the credential fingerprint; signed out has one too, and a failed read keeps the last); 403 and 429 back off; at most every 5 min |
| Antigravity | `agy -p /usage --output-format json --print-timeout 15s` (the CLI's official print mode, agy ≥ 1.1.11, checked once per executable with `agy --version`), without a window in a kill-on-close job, `ANTIGRAVITY_OAUTH_CREDENTIALS_JSON` removed; agy renews its own sign-in; at most every 10 min |
| OpenCode Go | `GET https://opencode.ai/zen/go/v1/usage` (OpenCode's own console, undocumented) with the Go API key from Credential Manager, else the one in OpenCode's `auth.json` (read-only); at most every 5 min |
| Grok | `GET https://cli-chat-proxy.grok.com/v1/billing?format=credits` (the Grok CLI's own `/usage` request, undocumented) with the latest subscription sign-in of the CLI's `auth.json` (read-only, never renewed); at most every 5 min |
| Described services | `GET` the file's `url` with the key from Credential Manager (`Lastdrop:<id>:<account>:api_key`) in the file's header, numbers taken at the file's JSON pointers ([custom-services-spec](design/custom-services-spec.md)); the bundled files and those in `%APPDATA%\Lastdrop\providers` are read at start-up into `lastdrop_core::custom`'s registry (`ProviderId::Custom`); 401 stops until the key is saved again, 403/429 back off; at most every 5 min |

Where these come from, with the sources checked: [providers-research](design/providers-research.md).

**Several accounts** ([accounts-spec](design/accounts-spec.md)): every config folder of Claude
Code or Codex is a source of its own (an entry of the service's `accounts`). `Provider::identity`
reads who is signed in there from the folder's own files (`lastdrop_providers::identity`: Claude's
`oauthAccount`, the claims of Codex's ID token), read-only, and keeps only a 16-hex-digit hash
(`AccountIdentity`) and, in memory, the e-mail address as a secret. The refresh worker reads it on
a source's first job, after its credential changed and at most every 30 minutes. A later folder
signed in to the same account is a duplicate: no card, never fetched, only its identity rechecked.
A folder signed in to another account starts over: its history files are retired
(`<file>.<seconds>.retired`, deleted after the retention), and what the evaluations and
notifications remembered about its meters is forgotten. `lastdrop_providers::discover` lists the
`~\.claude-*` and `~\.codex-*` folders with data for the settings window and the first run.

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
- **Notifications:** `NIF_INFO` on our own tray icon, which Windows shows as a toast; selecting
  one (`NIN_BALLOONUSERCLICK`) opens the flyout (notifications-spec §4). No WinRT toasts: they
  need a registered AppUserModelID and load WinRT into the tray process.
- **Theme:** taskbar light/dark (`SystemUsesLightTheme`), app light/dark, accent, transparency,
  high contrast; change notifications re-render.
- **Regional format and language:** `GetLocaleInfoEx` for the format and
  `GetUserPreferredUILanguages` for the display languages, read again on `WM_SETTINGCHANGE`
  "intl".
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
  secrets); the user's described services in `%APPDATA%\Lastdrop\providers\*.json`
  (only read, by both processes at start-up); the user's token prices in
  `%APPDATA%\Lastdrop\prices.json` (only read, by the usage history window, over the bundled
  `crates/core/prices.json`); history in `%LOCALAPPDATA%\Lastdrop\` (thinned to one sample per meter per
  5 minutes, 90 days); `%LOCALAPPDATA%\Lastdrop\state.json` (what the app remembers that is
  not a setting: the meters a source has reported, so their room in the readout survives a
  restart, what notifications announced, so a restart repeats none, and the hash of who each
  source was last signed in to, so a restart knows the duplicates and notices a folder signed in
  to someone else meanwhile; meter keys, names, levels, reset times and identity hashes only, no
  secrets); logs in `%LOCALAPPDATA%\Lastdrop\logs`
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
