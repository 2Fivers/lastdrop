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
| **Claude Code / Claude plans** | 5-hour session, weekly, per-model weekly, extra usage; tokens and estimated cost per day/model/project | Claude Desktop's plan-usage history (no token) → OAuth usage endpoint with Claude Code's token (read-only) → Claude Code's cached usage; local transcripts for statistics | Researched |
| **Codex** | 5-hour / weekly windows (whichever the plan has), credits; tokens per day/model | `codex app-server` `account/rateLimits/read` (official) → session logs offline | Researched |
| **GitHub Copilot** | Premium requests or AI Credits used vs allowance, reset date; usage per day/model | `copilot_internal/user` with the `gh auth token`; official billing usage API for statistics. Monthly plans since 2026-06-01: the user enters the allowance once | Researched |
| **Meshy** | Credit balance, burn rate, days left | Official `GET /openapi/v1/balance` with an API key | Researched |
| **Cursor** | Included usage and on-demand spend | _researching_ (local `state.vscdb` token + dashboard API) | Researching |
| **Google Antigravity** | Model quotas | _researching_ | Researching |
| **OpenCode Go** | Subscription limits | _researching_ | Researching |
| **Grok Build** | Usage limits / credits | _researching_ | Researching |
| **Simple balance APIs** (OpenRouter, ElevenLabs, DeepSeek, Tripo3D, Stability, Kling…) | Balance, limit, reset | Declarative provider files (JSON: endpoint, auth, field mapping) — no code needed | Planned |

### Multiple accounts

Claude Code and Codex support more than one account each. An account is a config folder
(`CLAUDE_CONFIG_DIR` for Claude Code, `CODEX_HOME` for Codex); Lastdrop discovers or lets the
user add folders, labels each account, and keeps quotas and statistics per account.
Details _researching_.

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
- Notifications at thresholds (for example 80 % and 95 %) and when a window resets.
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

| | Milestone | Content |
|---|---|---|
| M0 | **Stack spike** | Same flyout in Rust + Slint and Rust + Tauri; measure idle/open memory, flyout latency, size and look; pick one. _In progress._ |
| M1 | **First useful build** | Core, tray icon, taskbar readout, flyout, settings basics, autostart. Claude Code, Codex and Meshy. |
| M2 | **More services** | Copilot, Cursor, Antigravity, OpenCode Go, Grok Build; declarative balance providers; multiple accounts for Claude Code and Codex. |
| M3 | **Statistics** | History store, pace, forecasts, charts, threshold and reset notifications. |
| M4 | **Make it yours** | Readout on every taskbar, themes and Theme Studio, languages, adaptive refresh. |
| M5 | **v0.1** | Updates, code signing, installer; decide on distribution and licensing. |

## Open questions

- Taskbar readout: which window technique (owned overlay vs child of the taskbar) survives
  centred/left taskbars, auto-hide, secondary-monitor taskbars and Explorer restarts best?
  Being read out of CodeZeno's source now.
- Update and distribution channel while the app is proprietary and the repository private.
- Theme Studio scope: colours and widget layout only, or free-form placement?
