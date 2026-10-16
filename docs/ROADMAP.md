# Lastdrop roadmap

Living plan. Items move between milestones as we learn; data sources marked
_researching_ are being verified against primary sources before anything is built on them.

## Principles

- **Light on memory.** Lastdrop runs all day. Idle memory and flyout latency are measured,
  not guessed, and every feature is weighed against them.
- **Native Windows 11 by default.** The flyout sits next to the volume and network flyouts and
  should look like it belongs there. Other looks come from themes, not from the default.
- **No analytics, no telemetry.** Nothing about the user or their usage leaves the machine,
  except the requests to the service whose usage is being read.
- **Data stays local.** Settings, history and secrets live on the user's machine; secrets in
  Windows Credential Manager or DPAPI, never in plain files.
- **Never break the user's other tools.** Read other tools' tokens; never refresh or rewrite
  them (refresh tokens rotate, and refreshing one signs the vendor's own CLI out).
- **Be honest about freshness.** Every number shows its source and its age; stale data is
  labelled, not hidden. Undocumented endpoints are polled gently (at most every 5 minutes)
  and backed off on 429.
- **The user chooses what to see.** Providers, accounts, meters, widgets, order and layout.

## Providers

| Service | What Lastdrop shows | Where the data comes from | Status |
|---|---|---|---|
| **Claude Code / Claude plans** | 5-hour session, weekly, per-model weekly, extra usage; tokens and estimated cost per day/model/project | OAuth usage endpoint with Claude Code's token (read-only) → Claude Desktop's last usage response in its HTTP cache (no token) → Claude Desktop's plan-usage history (no token) → Claude Code's cached usage; local transcripts for statistics | 5-hour, weekly and per-model weekly windows built; extra usage and transcript statistics planned |
| **Codex** | 5-hour / weekly windows (whichever the plan has), credits; tokens per day/model | `codex app-server` `account/rateLimits/read` (official) → session logs offline | Windows built; credits and token statistics planned |
| **GitHub Copilot** | Monthly quotas used vs allowance, reset date (premium requests on paid plans; chat messages and code completions on Copilot Free); usage per day/model | `copilot_internal/user` with the `gh auth token`; official billing usage API for statistics. If a paid plan's answer stops giving its allowance (AI Credits since 2026-06-01), the user enters it once | Quotas built (Copilot Free verified live); paid plans from fixtures; statistics planned |
| **Meshy** | Credit balance, burn rate, days left | Official `GET /openapi/v1/balance` with an API key | Built (balance, burn rate, run-out forecast) |
| **Cursor** | Included usage (auto / API pools), on-demand spend, billing-cycle reset | Unofficial `cursor.com/api/usage-summary`; session cookie built from the token Cursor stores in `state.vscdb` (read-only, never copied — the file can be 1.6 GB). Teams: official Admin API with an admin key | Researched |
| **Google Antigravity** | 5-hour and weekly quota per pool ("Gemini models", "Claude and GPT models"), reset times | Official `agy -p /usage --output-format json` (agy ≥ 1.1.11; agy refreshes its own login) → the desktop app's local language server. Not the remote API: other tools scrape Google's OAuth client secret out of the binary to use it | Researched |
| **OpenCode Go** | 5-hour, weekly and monthly windows | `GET opencode.ai/zen/go/v1/usage` with the Go API key (from the user, or OpenCode's own `auth.json`). Undocumented; percent only | Researched |
| **Grok Build** | Credit usage % for the period, on-demand spend, prepaid balance, history | xAI's own CLI endpoint `cli-chat-proxy.grok.com/v1/billing?format=credits` with the Grok CLI login (`~/.grok/auth.json`). The xAI API's Management API for prepaid API credits is separate | Researched |
| **Simple balance APIs** (OpenRouter, ElevenLabs, DeepSeek, Tripo3D, Stability, Kling…) | Balance, limit, reset | Declarative provider files (JSON: endpoint, auth, field mapping) — no code needed | Planned |

### Multiple accounts

Claude Code and Codex support more than one account each. A **source** is a config folder
(`CLAUDE_CONFIG_DIR` for Claude Code, `CODEX_HOME` for Codex, Claude Desktop's data folder);
an **account** is who is actually signed in there. Cards are keyed by the account, not the
folder: two folders signed in to the same account become one card, and a folder that switches
account is noticed and recorded.

- **First run:** discover candidate folders (defaults, the environment, `~\.claude-*` and
  `~\.codex-*` folders that contain real Claude/Codex data) and show a checklist with who is
  signed in where, before anything is polled.
- **Add an account:** pick an existing folder, or let Lastdrop create one and open the vendor's
  own sign-in in a terminal (`claude` / `codex login` with the folder set). Lastdrop never builds
  its own Claude or ChatGPT login.
- **Expired login:** never refreshed by Lastdrop. The card keeps its last reading, marked stale,
  with "open Claude/Codex in this account to renew". Codex renews through its app-server.
- **Privacy:** settings hold folder paths and aliases, never tokens; "hide personal info" shows
  Account 1, Account 2 instead of e-mail addresses.
- **Statistics** are attributed per account from each folder's own transcripts and sessions.

## Features

### Where it lives

- **Tray icon** that draws the most constrained meter into the icon itself, with quick
  controls on right-click (refresh, pause, open settings, switch account, quit).
- **Flyout** on left-click, anchored above the tray icon on whichever monitor and taskbar edge
  it is on, closing when it loses focus.
- **Taskbar readout**: a compact readout docked in the taskbar, left of the tray, visible
  without a click. A primary surface, on by default. CodeZeno's Claude Code Usage Monitor is the
  reference (two rows, `5h ▮▮▮▮▯▯▯▯▯▯ 40% · 2h`, `7d … 6% · 4d`); Lastdrop does the same job
  better — see [The taskbar readout](#the-taskbar-readout).
- **Multiple monitors**: flyout and readout follow the monitor and DPI of the taskbar they
  belong to; the readout can show on the primary taskbar only or on every taskbar.
- **Windows startup**: per-user autostart, toggled in settings.

### The taskbar readout

What the reference gets right: always visible, two short rows, a number and a reset time,
the most important meter repeated as a number in the tray icon. Where we go further:

**Answer the real question.** The question is not "how much have I used" but "will I make it
to the reset". Each bar carries a **pace marker** — a thin tick at the share of the window
that has elapsed. Fill left of the tick: you are under pace. Fill past it: you are burning
faster than the window lasts.

**Colour means state, never decoration.** Neutral (system text colour or accent) while under
pace; amber when the current pace runs out before the reset; red at 90 % or when the forecast
runs out within 30 minutes. Colour is never the only signal: the marker and the text say the
same thing, so it works for colour-blind users and in high-contrast mode.

**Precise, not segmented.** A continuous bar drawn to the physical pixel at the taskbar's DPI,
rounded ends, instead of ten 10 % blocks. A segmented look stays available as a theme.

**Unambiguous text.** `40% · 2h` can be read as "two hours used" or "two hours left". Lastdrop
writes `40% ↻2h` (resets in two hours) or the reset clock `↻14:32`, the user's choice; tabular
digits so numbers don't jitter as they change.

**Taskbar colours, not app colours.** The taskbar can be dark while apps are light: the readout
follows the taskbar setting (`SystemUsesLightTheme`), the accent, transparency and
high-contrast mode, and Segoe UI Variable at the taskbar's text size.

**Several services without clutter.** The user pins the meters they want (for example Claude
5h + Codex weekly); "auto" shows the most constrained ones. A tiny provider glyph tells the
rows apart when more than one service is pinned.

**Interaction.** Hover: a Windows 11 tooltip-card with the full picture (all windows, pace
text, forecast, data age). Click: the flyout. Right-click: quick controls. Tray icon: the most
constrained meter as a legible number, drawn separately for 16, 20, 24 and 32 px, dimmed when
the data is stale.

**Never in the way.** It sits left of the tray overflow chevron and moves when the tray grows
or shrinks — driven by window events (location changes of the notification area,
`TaskbarCreated` after an Explorer restart, display and DPI changes, settings changes), not by
polling. When there is no room it collapses to one row, then to nothing, leaving the tray icon;
it never covers app buttons. It handles left-aligned and centred taskbars, auto-hide and
full-screen apps, and stays on the monitor it was placed on.

**Cheap to keep alive.** Redrawn only when data changes and once a minute for the countdown;
one small layered window; budget under 2 MB extra memory and ~0 % CPU at rest.

**Bug-free by construction.** The placement logic is a pure function from taskbar geometry
to a rectangle, unit-tested against a matrix of recorded layouts (bottom/top/left/right,
centred/left, 100–200 % DPI, 1–3 monitors, auto-hide, overflow open). Releases pass a 24-hour
soak test for memory, GDI and USER handle leaks, and a manual matrix on real multi-monitor
setups.

### Settings

- Providers and accounts on/off, order, and which meters each one shows.
- Refresh interval per provider (manual, 1–30 min, or adaptive: faster while the flyout is in
  use, slower when idle).
- Notifications at thresholds (for example 80 % and 95 %) and when a window resets
  ([notifications-spec](design/notifications-spec.md)): built, with used up and low balances.
- Language: English and Swedish to start.
- Updates: check, download and apply in place (channel to be decided while the app is not
  publicly distributed).

### Statistics

- Local history of every reading, kept beyond the vendors' own retention.
- Pace: "on pace", "X % ahead", "runs out at 14:32", "lasts until reset".
- Burn rate and depletion forecast for balance-based services ("≈ 9 days left").
- Charts: usage per window over time, tokens and cost per day/model, a heatmap of active hours.

### Themes

- Built-in themes, with native Windows 11 (light/dark, accent colour, acrylic) as the default.
- **Theme Studio**: a visual editor for colours and layout — which widgets appear, in what
  order and size — saved as a theme file that can be shared.

## Milestones

Status as of 2026-10-16.

| | Milestone | Content | Status |
|---|---|---|---|
| M0 | **Stack spike** | Same flyout in Rust + Slint and Rust + Tauri; measure idle/open memory, flyout latency, size and look; pick one. | Done: Rust + Slint with the software renderer ([ADR 0001](adr/0001-rust-and-slint-software-renderer.md)) |
| M1 | **First useful build** | Core, tray icon, taskbar readout, flyout, settings basics, autostart. Claude Code, Codex and Meshy. | Done, plus hover card, global hotkey, pause, and Settings and About in a dialog process of their own |
| M2 | **More services** | Copilot, Cursor, Antigravity, OpenCode Go, Grok Build; declarative balance providers; multiple accounts for Claude Code and Codex. | Copilot quotas built; everything else left. The settings schema already has room for accounts (alias, tag, source folder) |
| M3 | **Statistics** | History store, pace, forecasts, charts, threshold and reset notifications. | History store (90 days, one sample per meter per 5 minutes), pace, forecasts, a 14-day sparkline and notifications (levels, used up, resets, low balances) built; charts left |
| M4 | **Make it yours** | Readout on every taskbar, themes and Theme Studio, languages, adaptive refresh. | Adaptive refresh built, and numbers and times follow the Windows regional format; the readout shows on the primary taskbar only, and themes, Theme Studio and the Swedish strings are left |
| M5 | **v0.1** | Updates, code signing, installer; decide on distribution and licensing. | Not started |

## Open questions

- Update and distribution channel while the app is proprietary and the repository private.
- Theme Studio scope: colours and widget layout only, or free-form placement?

## Settled

- **Taskbar readout technique** (M1). From CodeZeno's source (MIT) and its issue history: a
  layered window parented into the taskbar shares the taskbar's visibility and z-order, but a
  cross-process child attaches input queues (their deadlock #92, stuck input #22) and dies with
  Explorer (their fix relaunches the whole app). Windows 11 offers no way to reserve taskbar
  space; only Explorer injection does, which we will not ship. What Lastdrop built instead:
  a separate, never-reparented message window that receives `TaskbarCreated` and rebuilds the
  readout in-process; no locks held across cross-process calls; `GetWindowRect` instead of
  `SHAppBarMessage`; app-button detection through UI Automation on its own thread with timeouts,
  falling back to a floating card instead of covering buttons; the readout is a topmost
  `WS_POPUP` that is never parented, so it handles `WM_DPICHANGED` (readout-spec §6.1, §6.9).
  Placement is unit-tested against the recorded layouts in
  [`crates/win/tests/layouts`](../crates/win/tests/layouts); the manual matrix on real
  multi-monitor setups and the 24-hour soak are release checks
  ([ARCHITECTURE "Quality bar"](ARCHITECTURE.md#quality-bar)).
