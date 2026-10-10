# Lastdrop — taskbar readout, hover card and tray icon

Implementation spec, final design. Status: ready to build (M1). Companion: [flyout-spec.md](flyout-spec.md).

Mocks: [final/readout.html](final/readout.html) with renders [final/readout-100.png](final/readout-100.png) and
[final/readout-150.png](final/readout-150.png). The `<script>` at the end of `readout.html` is the **reference
drawing code** for the pace bar and the tray icon: the native renderer must produce the same rectangles in device
pixels (track, fill, gaps, tick, dashes, tile bounds). Anti-aliased edges (rounded fill ends, tile corners, the drop)
and glyph rasterization come from Direct2D/DirectWrite and may differ from the browser by a level or two of alpha.

Design lineage: concept A "Pace bars" ([readout-concepts/](readout-concepts/)) with two grafts the owner chose —
from concept C, a **countdown** ("12 min left") instead of a clock time when running out is close; from concept D,
the **service glyph** at the head of every row once rows can come from more than one service.

## 0. Conventions

- Sizes are **logical px at 100 %**. `s = DPI / 96` of the taskbar's monitor. A logical size `x` becomes
  `round(x × s)` device px unless a rule below says otherwise. `round` is round-half-up everywhere in this spec.
  **Each constant is converted on its own and positions are sums of converted constants** (at 125 % the 6 px padding
  is 8, the 40 px bar 50, so the bar starts at 8 + …, never at `round((6 + …) × 1.25)`). Font sizes are not rounded:
  the em size is `size × s` device px (12 px → 15 at 125 %).
- Instants shown to people are rounded to the minute, durations are measured between shown instants (§5.0).
- "Taskbar theme" = `SystemUsesLightTheme` (HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize).
  The readout, its hover card and the tray icon follow the taskbar theme, never `AppsUseLightTheme`.
- Shares are `f64` in `[0, 1]` unless stated; percentages shown to people are integers.
- Times are kept in UTC and formatted in the user's local time zone and Windows regional format.
- `MUST` items are part of the design. Constants are collected in §3.11; they are design values, keep them in one
  module so they can be tuned without touching logic.

## 1. Surfaces

| Surface | What it is | Drawn by |
|---|---|---|
| **Readout** | Compact two-row readout docked left of the tray overflow chevron | Own layered Win32 window, own drawing (§6) |
| **Hover card** | Tooltip card over the readout or the tray icon | A second small Slint window with the DWM backdrop (§8) |
| **Tray icon** | 16/20/24/32 px icon with the most constrained meter | RGBA → `HICON`, drawn at runtime (§9) |
| **Flyout** | Details on click | Slint window, see flyout-spec.md |

All four use the same meter model (§3), the same text grammar (§5) and the same pace bar (§6.6).

## 2. Vocabulary

- **Service**: Claude, Codex, Copilot, Meshy, … (one per account; multiple accounts are M2, see §6.4).
- **Meter**: one number a service reports. Two kinds:
  - **Window meter**: a share used of a limit that resets on a schedule (Claude 5-hour, weekly, weekly per model;
    Codex 5-hour/weekly; Copilot monthly).
  - **Balance meter**: an amount that only goes down until topped up (Meshy credits).
- **Reading**: one successful fetch of a service: values for all its meters plus `fetched_at`.
- **Window**: for a window meter, the interval `[resets_at − L, resets_at)`; `L` is its length.
- **Row**: one meter shown in the readout.
- **Winner**: the most constrained meter across all enabled services (§3.10); it drives the tray icon.

Each meter definition (from the provider) carries: `id`, `kind`, `L` (window meters; may be unknown),
`starts_on_use` (window meters), `short_label` (≤ 5 chars, readout), `short_name` (in running text: card titles,
"Also tracked", tray tooltip, flyout verdict, always after the service name or "limit"), `name` (meter blocks in the
hover card and flyout), and its order within the service.

| Service | Meter | short_label | short_name | name | L | starts_on_use |
|---|---|---|---|---|---|---|
| Claude | 5-hour | `5h` | 5-hour | 5-hour session | 5 h | yes |
| Claude | weekly, all models | `Wk` | weekly | Weekly · all models | 7 d | no |
| Claude | weekly, per model (e.g. Fable) | model name, e.g. `Fable` | Fable weekly | Weekly · Fable | 7 d | no |
| Codex | 5-hour / weekly | `5h` / `Wk` | 5-hour / weekly | 5-hour session / Weekly | 5 h / 7 d | yes / no |
| Copilot | monthly premium requests | `Mo` | monthly | Premium requests | calendar month | no |
| Meshy | credits (balance) | `Cr` | credits | Credits | — | — |

For a calendar-month window `L` is the actual length of that month (`resets_at − start of month`).
`starts_on_use` = the provider starts the next window only with the next use (Claude's 5-hour session starts with
the first message), so between windows there is no reset time. The values in this column are the current reading of
the providers' behaviour; each provider confirms its own in a unit test against a recorded response.

## 3. Meter model and statistics (normative; each item is a unit test)

### 3.1 Inputs per window meter

- `u` — used share as reported (e.g. Claude `utilization / 100`). May exceed 1; never negative (clamp at 0).
- `R` — `resets_at` (UTC instant), or **none** when the provider reports no reset time (a `starts_on_use` window
  between windows, e.g. Claude's `five_hour.resets_at: null`; a per-model weekly that was never used).
- `L` — window length (from the meter definition, or from the provider when it reports one), or **unknown** (a
  declarative provider that only reports `resets_at`).
- `limited` — provider says the limit is reached (optional flag).
- `t_r` — `fetched_at` of the reading.
- `history` — earlier readings of the same meter, ascending. Two readings belong to the **same window instance**
  when their `R` differ by at most 2 minutes (providers jitter `resets_at` by seconds). A reading **without** `R`
  that was taken before the `R` of the instance before it belongs to that instance and is shown with its `R`: a
  source without reset times (Claude Desktop's history) says nothing about the instance, so losing `R` is never a
  window reset (no cleared hysteresis, no announcement, no cut forecast history).

### 3.2 Elapsed share

```
e(now) = clamp(1 − (R − now) / L, 0, 1)
```

`now` is wall-clock time for live display. For a stale meter (§3.8) use `now = t_r` (frozen at the reading).
Example: `R − now = 2 h 14 min`, `L = 5 h` → `e = 0.5533`.
`e` is **none** when `R` is none or `L` is unknown: no tick, no gaps, no pace (§3.4), and the hover card's tick line
says `window not started` (IDLE) or `window length unknown` (§8.2).

**No reset time** (`R` none):
- `u = 0` and `starts_on_use` → state **IDLE** (window not started): bar track only, `0%`, time cell `–` (§5.7).
- otherwise (provider omitted it): bar without tick, `pct`, time cell `–`; no forecast (§3.5), so only OK,
  CRITICAL by level and EXHAUSTED can occur; EXHAUSTED shows `Empty` (no "Back" time).

### 3.3 Display percent and exhaustion

```
exhausted = limited || u ≥ 1.0
pct = clamp(round(u × 100), lo, hi)
      lo = (u > 0) ? 1 : 0          // something used never shows as 0 %
      hi = exhausted ? 100 : 99     // 100 % only when the provider agrees it is used up
```

**Level thresholds compare `pct`, not `u`**, so what is shown and what is decided always agree
(`u = 0.895` → `pct = 90` → critical by level; `u = 0.8949` → `89` → not). Compute `round(u × 100 + 1e-9)` so a
binary-float `42.499999…` from `0.425` still rounds to 43; providers that report integer percent pass through
unchanged.

### 3.4 Pace

```
d = u − e
on_pace   = u ≤ e + TOL_PACE               // the fill does not run past the tick
under     = u < e − TOL_PACE
ahead     = u > e + TOL_PACE
```

Hover-card wording: `under` → "slower than time passes"; neither → "about as fast as time passes";
`ahead` → "faster than time passes". Pace alone **never** sets a severity (§3.7); it is what the tick shows.

### 3.5 Forecast

The forecast answers "at the rate you have been using it recently, when is it empty?".

```
S        = clamp(L / 10, 30 min, 24 h)            // trailing span: 5 h → 30 min, 7 d → 16.8 h, month → 24 h
                                                    // L unknown: S = 1 h
start    = R − L                                    // L unknown: t of the first reading of this window instance
t_n, u_n = latest reading
a        = latest reading of the same window instance with t ≤ t_n − S,
           else the oldest reading of the same window instance
span     = t_n − t_a
valid    = R ≠ none  &&  span ≥ FC_MIN_SPAN  &&  t_n − start ≥ FC_FRESH_GUARD
rate     = max(0, (u_n − u_a) / span)              // share per hour
p        = u_n + rate × (R − t_n)                  // projected share used at the reset
t_out    = (rate > 0 && u_n < 1) ? t_n + (1 − u_n) / rate : none
runs_out = valid && t_out ≠ none && t_out < R
N        = whole minutes from floor_min(now) to round_min(t_out), at least 1     // the runway as shown (§5.0)
```

- The forecast is computed per reading (anchored at `t_n`); between readings only `N` changes, and it is
  re-evaluated on every minute tick (§12), so a meter can turn CRITICAL by runway between two readings.
- Not `valid` → no forecast: the meter can be OK or critical by level, never warning or critical by runway.
- A decrease of `u` inside a window instance (provider correction) gives `rate = 0`.
- Like `pct` for levels, **runway thresholds compare `N`**, the minutes that are on screen, so `30 min left` is
  always red and `31 min left` never is.

### 3.6 Balance meters

```
ref       = balance right after the most recent top-up (an increase ≥ 1 % of the balance)
            else the highest balance seen in the last 30 days; never below the current balance
u         = 1 − balance / ref                        // share of the last top-up spent; drawn like a window fill
burn      = (sum of decreases over the last 7 days) / 7    // units per day; increases (top-ups) ignored
valid     = history covers ≥ 48 h
days_left = balance / burn  (burn > 0), else none ("no recent use")
t_out     = now + days_left
exhausted = balance ≤ 0
```

A balance meter has no tick (no window) and no reset.

### 3.7 Severity

Per meter, from fresh data (stale meters have no severity, §3.8):

| Severity | Window meter enters when | Leaves when | Balance meter enters when |
|---|---|---|---|
| **EXHAUSTED** | `exhausted` | a reading shows `!exhausted`, or the window resets | `balance ≤ 0` |
| **CRITICAL** | `pct ≥ 90` (level), or `runs_out && N ≤ 30` (runway) | level: `pct ≤ 87`; runway: `N ≥ 36` or `!runs_out` | `days_left < 2` |
| **WARNING** | `runs_out` (equivalently `p > 1`) | `p ≤ 0.97` or `!valid` | `days_left < 7` |
| **OK** | otherwise | | otherwise |

- Precedence: EXHAUSTED > CRITICAL > WARNING > OK. Exit thresholds give hysteresis; keep the previous severity per
  meter to apply them.
- **Guards after a reset**: the forecast is not valid until 15 min after the window start (`FC_FRESH_GUARD`) and
  until two readings 15 min apart exist (`FC_MIN_SPAN`). So in the first 15 minutes of a window only EXHAUSTED and
  critical-by-level can trigger.
- On a window reset (§3.9) the meter's severity and hysteresis state are cleared.
- Balance hysteresis: CRITICAL leaves at `days_left ≥ 2.5`, WARNING at `days_left ≥ 8`; a top-up re-evaluates at once.
- A user threshold for balances (settings) may raise WARNING/CRITICAL earlier; it never lowers them.

### 3.8 Freshness (per service)

```
stale_after = max(15 min, 2 × poll_interval)       // manual refresh mode: 60 min
STALE if   now − last_success > stale_after
      or   the last attempt failed with an auth error (reason SIGN_IN, immediately: no reading can follow)
      or   updates are paused by the user          (reason PAUSED, immediately)
reason = SIGN_IN | PAUSED | OFFLINE (no internet per Network List Manager, or DNS/connect failures)
       | NO_DATA (anything else: HTTP 5xx, 429 back-off, parse error)
       | OLD_READING (no failure on record: the source answers, but its newest reading is old,
                      e.g. Claude Desktop's history while Claude Desktop is closed)
```

OLD_READING names no fault: the readout shows AGE on every row of the service (no REASON), the flyout's message
line says **No new reading** · showing 12:42 · next try 13:30, the hover card sentence
`{service} answers, but its newest numbers are from {time}. Next try at {next}.`

While a service is stale its meters show the **last reading, frozen**: `u`, `e(t_r)`, no forecast, no severity
colour. If a meter's window reset after `t_r` (`R ≤ now`), draw it as reset (§3.9) but keep the stale styling.

### 3.9 Window reset

When `now ≥ R` and no newer reading has arrived:

- Fixed-schedule window (`starts_on_use` = no, `L` known): the meter is treated as a new window: `u = 0`,
  `R' = R + L` (calendar month: next month start), `e` from `R'`, severity OK, forecast invalid (guards). State
  JUST_RESET.
- `starts_on_use` window: `u = 0`, `R = none`, state IDLE (§3.2) until a reading reports a new `resets_at`.
- `L` unknown: `u = 0`, `R = none`, shown like IDLE until the next reading.
- Schedule a refresh of that service at `R + random(30 s … 90 s)` (respecting back-off and the 5-minute minimum
  interval for undocumented endpoints: if the last fetch is less than 5 min old, wait until it is not).
- The hover card says "Reset at 16:30 · waiting for the first reading".

### 3.10 Ranking: auto rows and the winner

Candidates: every meter of every enabled service; balance meters only when WARNING or worse.

Rank key, most constrained first:

1. Display class: EXHAUSTED 5 > CRITICAL 4 > WARNING 3 > STALE 2 > OK 1 > LOADING 0.
2. Within EXHAUSTED: later `R` first (out for longer; `R` none counts as latest). Within CRITICAL and WARNING:
   earlier `t_out` first (CRITICAL by level without `t_out`: use `R`; `R` none: last). Within OK and STALE: higher
   pressure `q = max(u, p)` first (`p = u` when no valid forecast; IDLE has `q = 0`).
3. Tie: service order, then meter order (settings).

- **Winner** (tray icon) = rank 1 over all candidates, regardless of what the readout shows.
- **Too few candidates** (only balance services enabled, all OK; or one window meter in total): the empty places
  go to OK balance meters, fewest `days_left` first (no forecast: last), for the auto rows and for the winner. No
  meter at all (nothing enabled, or every service LOADING): the readout shows LOADING rows while a first fetch
  runs, else it is HIDDEN; the tray icon shows the drop.
- **Auto rows** = the top N (N = 2, or 1 in the compact form), **displayed in service/meter order**, not rank order,
  so rows do not swap places as ranks change.
- Auto hysteresis: a shown row is replaced only when the challenger has a higher display class, or the same class
  and `q ≥ q_shown + AUTO_SWAP_MARGIN`; same-class swaps happen at most once per `AUTO_SWAP_INTERVAL`.

### 3.11 Constants

| Name | Value | Used in |
|---|---|---|
| `TOL_PACE` | 0.03 | §3.4, §6.6 |
| `FC_MIN_SPAN` | 15 min | §3.5 |
| `FC_FRESH_GUARD` | 15 min after window start | §3.5, §3.7 |
| `S` | `clamp(L/10, 30 min, 24 h)` | §3.5 |
| `CRIT_LEVEL_PCT` / exit | 90 / 87 | §3.7 |
| `CRIT_RUNWAY` / exit | `N ≤ 30` / `N ≥ 36` (shown minutes) | §3.7 |
| `WARN_EXIT_P` | 0.97 | §3.7 |
| `COUNTDOWN_MAX_MIN` | 59 (any severity; CRITICAL counts down at any N) | §5.3 |
| `COUNTDOWN_HOURS_BELOW` | 48 h (then days) | §5.3 |
| `TIME_OF_DAY_MAX` | 12 h | §5.2 |
| `STALE_MIN` | 15 min (`max(15 min, 2 × interval)`; manual 60 min) | §3.8 |
| `RESET_REFRESH_JITTER` | 30–90 s | §3.9 |
| `BAL_WARN_DAYS` / `BAL_CRIT_DAYS` | 7 / 2 | §3.7 |
| `BAL_SPAN` / `BAL_MIN_HISTORY` | 7 d / 48 h | §3.6 |
| `AUTO_SWAP_MARGIN` / `AUTO_SWAP_INTERVAL` | 0.05 / 10 min | §3.10 |
| `HOVER_DELAY` | `SPI_GETMOUSEHOVERTIME` (default 400 ms) | §10 |

## 4. State machines

### 4.1 Row display state (one per shown meter)

```
LOADING ──first reading──▶ (from §3.7) OK | IDLE | WARNING | CRITICAL_LEVEL | CRITICAL_RUNWAY | EXHAUSTED
any fresh state ──service stale──▶ STALE(reason) ──fresh reading──▶ (recomputed)
any fresh state ──now ≥ R──▶ JUST_RESET (fixed schedule: u = 0, R' = R + L, guards)
                          or IDLE (starts_on_use / L unknown: u = 0, R none) ──next reading──▶ (recomputed)
```

`CRITICAL_LEVEL` = CRITICAL entered by `pct ≥ 90` while `!runs_out`; `CRITICAL_RUNWAY` = CRITICAL with
`runs_out` (the level rule may also hold: a meter at 91 % that runs out in 6 h is CRITICAL_RUNWAY and counts down).
`IDLE` = `u = 0` with no reset time (§3.2). The row's look is chosen by the table in §5.7. Every state in this list
has a mock in readout.html §1 and a card in flyout.html §1–4.

### 4.2 Readout form

```
TWO_ROWS     two (or more candidate) meters, taskbar ≥ 40 px tall, room ≥ width(two rows)
SINGLE_ROW   only one meter configured/available: one full row, vertically centred, same columns
COMPACT      room < width(two rows) but ≥ width(compact), or taskbar < 40 px tall: the winner among shown meters
HIDDEN       room < width(compact), or readout turned off, or full-screen app / auto-hidden taskbar
```

Going **up** a form needs `room ≥ width(form) + 8` (hysteresis, so a flickering app list cannot make it bounce).
`room` = left edge of the overflow chevron (or of the notification area if there is no chevron) − 4 − (right edge of
the rightmost taskbar app button + 8). Placement is a pure function of taskbar geometry (roadmap: "Bug-free by
construction").

### 4.3 Content mode

- **Pinned** (user picks 1–2 meters): rows = the pinned meters in the user's order.
- **Auto** (default on first run): rows = §3.10.
- The tray icon always shows the winner.

## 5. Text grammar

One grammar for every surface. Percent is always **used**, never "left". Words appear only when something is wrong.

### 5.0 Instants and durations (one rule for every surface)

- `floor_min(now)`: the current time truncated to the minute, as the taskbar clock shows it.
- `round_min(t)`: any other instant (reset, run-out, "Back", "Last reading") rounded to the nearest minute, half up.
  Providers report resets like `14:59:59.94`; rounding shows `15:00`, truncating would show a wrong `14:59`.
- Every duration shown (`in 2 h 14 min`, the countdown `N`) is `round_min(t) − floor_min(now)`, so the clock, the
  shown time and the shown duration always add up (`12:42` + `2 h 14 min` = `14:56`). Ages are
  `floor_min(now) − round_min(last_success)`.
- Long durations (hover card, flyout): `< 60 min` → `N min`; `< 24 h` → `H h M min` (`2 h` when M = 0);
  `< 7 d` → `D d H h` (`3 d` when H = 0); else `D d`.

### 5.1 Tokens

| Token | Meaning | Example | Colour / weight |
|---|---|---|---|
| `VALUE` | share used, `pct` + percent sign | `42%` | TextPrimary, 400 |
| `↺ TIME` | resets at (glyph U+E777 "UpdateRestore", Segoe Fluent Icons) | `↺14:56` `↺Mon` | glyph TextSecondary, time TextPrimary |
| `Out TIME` | runs out at (before the reset) | `Out 15:20` | TextPrimary, 600 |
| `COUNTDOWN left` | runs out in (§5.3) | `12 min left` `6 h left` `3 d left` | TextPrimary, 600 |
| `Back TIME` | used up; available again at | `Back 16:30` | TextPrimary, 600 |
| `REASON` | why there is no fresh reading | `Offline` `Sign in` `Paused` `No data` | TextSecondary; `Sign in`: AccentTextFillColorPrimary |
| `AGE` | age of the last good reading | `47m ago` | TextSecondary |
| `≈DURATION` | balance lasts about | `≈9d` | `≈` TextSecondary, duration TextPrimary |
| `–` (time cell) | no reset time (IDLE, or the provider gave none) | `0%` `–` | TextSecondary |

The reset glyph is drawn without a space before the time (`↺14:56`); the hover card always teaches it as
"↺ Resets 14:56" (§8).

### 5.2 Time points (`TIME`)

Used for resets, run-out times and "Back". Clock mode (default):

| Condition (Δ = t − now) | Token | Examples |
|---|---|---|
| Δ < 12 h | time of day, Windows short time format **without AM/PM** | `14:56`, `00:30`; 12-h locales `2:56`, `12:30` |
| same local date, Δ ≥ 12 h | "Today" | `Today` |
| local date ≤ today + 6 | abbreviated weekday (`LOCALE_SABBREVDAYNAME*`) | `Mon`, `Fri`; sv-SE `mån`, `fre` |
| later | calendar days between the dates + `d` | `7d`, `22d` |

- The 12-hour rule is what makes dropping AM/PM safe: a time less than 12 hours away is the next occurrence of that
  time, so `2:56` can only mean one moment. Hover card and flyout use the full short time (`2:56 PM`).
- 12/24 h, leading zero and separator come from `GetLocaleInfoEx(LOCALE_NAME_USER_DEFAULT, LOCALE_SSHORTTIME)`:
  pattern contains `H` → 24 h; `HH` vs `H` decides the leading zero; strip `tt`/`t` and the space before it.
- Δ and the token are computed from `round_min(t)` and `floor_min(now)` (§5.0).
- Relative mode (setting "Show times as: time remaining"): Δ < 60 min → `Nm` (N = Δ in minutes, ≥ 1);
  Δ < 24 h → `Nh` (floor); else `Nd` (floor). Countdown (§5.3) and AGE are the same in both modes.

### 5.3 Countdown

The owner's rule (from concept C): **a CRITICAL meter never shows a run-out clock time**; urgency must not need a
comparison with the taskbar clock. A run-out less than an hour away counts down too, whatever the severity.

```
N = §3.5 (minutes from floor_min(now) to round_min(t_out), at least 1)
COUNTDOWN(N) = N ≤ 59     → "N min"
               N < 48 × 60 → "H h"   (H = floor(N / 60))        // 1 h … 47 h
               else        → "D d"   (D = floor(N / 1440))      // 2 d …

runs_out && (CRITICAL || N ≤ 59)  → COUNTDOWN(N) + " left"     ("12 min left", "6 h left", "3 d left")
runs_out && otherwise (WARNING)   → "Out " TIME(round_min(t_out))
```

- So: WARNING 60 min or more away → `Out 15:20`; WARNING 31–59 min → `45 min left` with the triangle (caution);
  CRITICAL → always a countdown with the disc (`12 min left` by runway, `6 h left` when critical by level and the
  forecast runs out later).
- Hours and days floor, so the countdown never promises more than there is (`1 h 59 min` → `1 h left`, then
  `59 min left`).
- N never shows 0: when `t_out` has passed and the provider has not yet said `limited`, show `1 min left` until the
  next reading. Recomputed every minute (§12).
- A WARNING held by hysteresis (§3.7 exit `p ≤ 0.97`) whose run-out moved to or past the reset (`!runs_out`) shows
  **no run-out time** on any surface: readout ▲ + `pct` (as the compact cell); hover card title `{meter} limit is on
  course for {p}% at the reset`, forecast `on course for {p}% at reset`; flyout verdict `… is on course for {p}% at
  the reset`, right cluster ▲ `Resets 16:30`, trouble line `On course for {p}% at the 16:30 reset.`; UIA `warning,
  on course for {p} percent at the reset …` (`p` rounded, at most 99 %).
- Balance meters use the same rule with `t_out = now + days_left` (WARNING → `Out Thu`, CRITICAL → `40 h left`).

### 5.4 Age

`floor_min(now) − round_min(last_success)`: < 60 min → `Nm ago` (N ≥ 1); < 48 h → `Nh ago` (floor); else `Nd ago`
(floor).

### 5.5 Balance duration

`days_left` < 2 → `≈Nh` (floor hours, ≥ 1); < 14 → `≈Nd` (floor); < 63 → `≈Nw` (floor weeks); ≥ 63 → `≈9w+`.
No burn in 7 days, or less than 48 h of history → `–`.

### 5.6 Percent and numbers

- Locale percent pattern (`LOCALE_IPOSITIVEPERCENT`): en `42%`; sv-SE `42 %` with U+202F NARROW NO-BREAK SPACE.
- Digits are tabular. Segoe UI Variable's default figures are tabular (verified: `0000000000` and `1111111111`
  have the same advance); if a renderer substitutes proportional figures, enable OpenType `tnum`.
- Credit amounts use the locale's digit grouping (`1,240`; sv-SE `1 240` with U+202F).

### 5.7 Row text by state

The right part of a row is one cell `RIGHT_W` wide (§6.3). Sub-columns: `VALUE` box 24 px right-aligned at x = 0,
time/status starting at x = 30. "Say" text starts at x = 0.

| State | Bar (§6.6) | Right cell |
|---|---|---|
| LOADING | track only, no tick | first row of the service: `Reading…` from x = 0 (TextSecondary, 400); other rows: `–` in the VALUE box (TextSecondary) |
| OK | fill TextPrimary, tick | `42%` + `↺14:56` |
| OK, no reset time (provider gave none) | fill TextPrimary, no tick | `42%` + `–` |
| IDLE (window not started) | track only, no tick | `0%` + `–` |
| OK, balance | fill TextPrimary, no tick | `62%` + `≈9d` (`–` without a forecast) |
| CRITICAL_LEVEL, lasts | fill Critical | `92%` **semibold** + `↺16:30` |
| WARNING | fill Caution | ▲ + `Out 15:20` (or `45 min left` when N ≤ 59) |
| CRITICAL_RUNWAY | fill Critical | ● + countdown, always: `12 min left`; `6 h left` (critical by level, runs out later) |
| EXHAUSTED | fill Critical, full length | ● + `Back 16:30` (no reset time, or balance: ● + `Empty`) |
| JUST_RESET | empty fill, tick at `e(R')` | `0%` + `↺7d` (the new `R'`) |
| STALE | dashed fill and tick in the stale colour, frozen at `t_r` | `42%` (stale colour) + REASON or AGE (below) |

▲ = warning glyph, ● = critical glyph (§6.7). Say-text (▲/● and words) starts at x = 0: glyph 12 + 2 + text.

STALE status, per service: the rows of the service take, in order, REASON then AGE (two rows: `Offline` / `47m ago`).
A service with one row shows `Sign in` (SIGN_IN) or `Paused` (PAUSED), otherwise AGE. A row whose frozen reading was
EXHAUSTED cannot show `100%` (the VALUE box fits `99%`): it shows `Back 16:30` from x = 0, regular weight, stale
colour, no glyph, and the status goes to the service's other row (one-row service: no status; the hover card has it).

### 5.8 Compact cell (`COMPACT_W`)

Left-aligned. OK `61%` · IDLE `0%` · STALE `61%` (stale colour) · WARNING ▲ `78%` semibold · any row that shows a
countdown in the two-row form (§5.3) ▲/● + COUNTDOWN without "left" (`12 min`, `6 h`) semibold · CRITICAL_LEVEL
● `92%` semibold · EXHAUSTED ● `Empty` semibold · LOADING `–`. The reset and run-out times move to the hover card.

### 5.9 Localization

All strings are resources (Appendix B). Column widths are computed from the localized strings (§6.3), so a
translation can never be cut off. Swedish weekday abbreviations are lower-case by convention; keep them.

## 6. Readout layout and drawing

### 6.1 Window

- `WS_POPUP`, `WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE`. Never parented to the
  taskbar (roadmap decision). Per-pixel alpha with `UpdateLayeredWindow` (premultiplied BGRA DIB).
- **Z-order**: the taskbar is itself topmost and raises itself within the topmost band when it is clicked or
  activated (Start, Win+T, a task button), which would cover an unparented readout. Re-assert with
  `SetWindowPos(HWND_TOPMOST, …, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)` from a `SetWinEventHook`
  (`EVENT_SYSTEM_FOREGROUND`, `EVENT_OBJECT_REORDER`, out of context) when the event concerns `Shell_TrayWnd`;
  never by polling. Part of the roadmap's open "readout technique" validation.
- **Hit testing**: fully transparent pixels of a layered window are click-through. Fill the whole slot with
  colour `0x01000000` (alpha 1/255, invisible) under the content so the entire slot receives the mouse.
- Slot: width per §6.3, height `slotH = round(40 s)` device px, vertically centred in the taskbar
  (`top = taskbar.top + floor((taskbar.height − slotH) / 2)`; 4 px margins on a 48 px taskbar). Right edge
  `round(4 s)` left of the overflow chevron (or of the notification area).
- Content block: rows are `rowH = round(16 s)` tall; two rows centred in the slot
  (`y1 = floor((slotH − 2 rowH) / 2)`, `y2 = y1 + rowH`; 4 px above and below at 100 %), so row 1 aligns with the
  clock's time line and row 2 with its date line. SINGLE_ROW and COMPACT: one row at `floor((slotH − rowH) / 2)`
  (y = 12 at 100 %). Text baselines: DirectWrite line box of `rowH` with the font's default line spacing, text
  vertically centred in it (as the clock does).
- 6 px horizontal padding inside the slot on both sides.
- Text rendering: DirectWrite, **grayscale** antialiasing (ClearType cannot be used on per-pixel-alpha
  surfaces), pixel-snapped glyph runs, gamma as the system's. Fonts: §6.5.

### 6.2 Row anatomy (left to right)

`[glyph 12] [4] [label LW] [6] [bar 40] [6] [right cell RIGHT_W or COMPACT_W]` — the glyph and its 4 px gap are
present only when rows can come from more than one service (§6.4).

### 6.3 Widths (computed, never from the current data)

Computed in device px with the actual fonts at the actual DPI, whenever the locale, DPI, pinned set or enabled
services change — **never** when data changes, so the readout keeps one width in every state. A meter a source
reported earlier in this run still *can appear* when a later reading lacks it (a per-model weekly that only the
OAuth source reports while Claude Desktop's history answers).

```
LW        = ceil(max width of short_label over every meter that can appear in the readout)     // 11 px font
TIMEW     = max width over all TIME tokens the shown windows can produce:
            "00:00" (tabular, so the widest time), "Today", the 7 weekday abbreviations, "88d"
CD        = the countdown strings: "59 min left", "47 h left", "29 d left"   (widest digits; localized)
RIGHT_W   = ceil(max( 30 + 11 + 1 + TIMEW,                                          // VALUE + ↺TIME
                      30 + max width(REASON strings, "59m ago", "47h ago", "–"),
                      width("Reading…"), width("Back " + TIME tokens),           // x = 0, regular (LOADING, stale EXHAUSTED)
                      12 + 2 + max semibold width("Out " + TIME tokens, CD, "Back " + TIME tokens, "Empty"),
                      30 + width("≈") + 1 + max width("47h", "13d", "8w", "9w+")    // only if a balance can show
                    ))
COMPACT_W = ceil(max(width("99%"), 14 + semibold width("59 min", "47 h", "29 d", "Empty", "99%")))
width     = 6 + [12 + 4] + LW + 6 + 40 + 6 + (RIGHT_W | COMPACT_W) + 6
```

In device px: the text widths are measured with DirectWrite at the em size `size × s` and ceiled; every constant
is `round(c × s)` on its own (§0); `width_dev` is the sum. The VALUE box (24) and the time column start (30) are
`round(24 s)` and `round(30 s)` inside the cell.

Results at 100 %, en-US/en-GB, Segoe UI Variable Small:

| Form | LW | Right | Width |
|---|---|---|---|
| Two rows / single row, one service (5h, Wk) | 17 | 79 | **160** |
| Two rows, several services | 17 | 79 | **176** |
| Auto where a per-model label ("Fable") can appear | 27 | 79 | **186** |
| Compact, one service / several | 17 | 53 | **134 / 150** |

(For reference, measured in Edge at 12 px: semibold `Out 15:20` 52.7, `59 min left` 60.5, `47 h left` 46.2,
`Back Today` 63.0, `59 min` 38.4, `99%` 23.9, `100%` 28.8; regular `59m ago` 48.2, `Reading…` 53.9, `Today` 32.4.
RIGHT_W = 30 + 48.2 → 79; `Reading…` would need 84 at x = 30, which is why it starts at x = 0.)

### 6.4 Label vs glyph

- Rows can come from **one** service (pinned meters of one service; or auto with one service enabled): rows show
  the window label only (`5h`, `Wk`). The hover card names the service.
- Rows can come from **more than one** service: every row shows the service glyph, then the window label
  (`✳ 5h`, `>_ Wk`). The glyph column is decided from what *can* appear, not what does, so the width never jumps.
- A service without a drawn glyph gets a **monogram**: its first letter (12 px box, 1 px rounded-square outline
  r = 2.5, letter Segoe UI Variable Small 8 px semibold, centred). If two enabled services share a monogram, use
  two letters at 7 px.
- **Several accounts of one service** (M2): the glyph says which service, the label says which account. When two or
  more accounts of a service are enabled, that service's labels become `{tag} {short_label}` (`W 5h`, `P 5h`), where
  `tag` is the account's 1–2 character tag (settings; default the first letter of the alias, else `1`, `2` in the
  order the accounts were added). LW is measured over these labels like any other, so the width is fixed once the
  accounts are chosen. Hover card, flyout, tooltip and UIA use the alias: `Claude · Work` / `Claude Work 5-hour`.

### 6.5 Typography

| Role | Font | Size / line | Weight | Colour |
|---|---|---|---|---|
| Row text (value, time) | Segoe UI Variable Small | 12 / 16 | 400 | TextPrimary |
| Say text, critical-level value, compact trouble | Segoe UI Variable Small | 12 / 16 | 600 | TextPrimary |
| Label | Segoe UI Variable Small | 11 / 16 | 400 | TextSecondary |
| Status (REASON, AGE), stale value | Segoe UI Variable Small | 12 / 16 | 400 | Stale colour (§7) |
| `Sign in` | Segoe UI Variable Small | 12 / 16 | 400 | AccentTextFillColorPrimary |
| Reset glyph U+E777 | Segoe Fluent Icons | 11 (cell 11 + 1 gap) | — | TextSecondary |
| Severity glyph | Segoe Fluent Icons | 12 (cell 12 + 2 gap) | — | §6.7 |

Fallbacks: Segoe UI (Windows 10), Segoe MDL2 Assets. Font sizes scale with `s` (12 px → 18 px at 150 %).

### 6.6 The pace bar (normative pixel algorithm)

Same algorithm for the readout (40 px), the hover card (296 px), the flyout (310 px) and, with fixed tables, the
tray icon (§9). Computed in device px:

```
Lpx    = round(W × s)                               // bar length
hair   = max(1, floor(s))                           // track, tick and gap width: 1 px up to 199 %, 2 px at 200 %+
odd(x) = 2 × round((x − hair) / 2) + hair           // keeps strokes centred on the track
fillH  = odd(3 × s)                                 // 3, 3, 5, 5, 6 at 100/125/150/175/200 %
tickH  = odd(9 × s)                                 // 9, 11, 13, 15, 18
Hpx    = round(rowH × s)                            // rowH = 16 readout, 11 hover card, 9 flyout
yTrack = floor((Hpx − hair) / 2);  yFill = yTrack − (fillH − hair)/2;  yTick = yTrack − (tickH − hair)/2

f = round(clamp(u,0,1) × Lpx);  if u > 0 && f == 0 { f = 1 };  EXHAUSTED: f = Lpx;  LOADING: f = 0
k = e ≠ none && !LOADING ? clamp(round(e × Lpx), 0, Lpx − hair) : none          // tick x (none: balance, IDLE, no R or L)
if k and not EXHAUSTED:
    if under (u < e − TOL_PACE) and f > k − hair − 1:   f = max(u > 0 ? 1 : 0, k − hair − 1)   // visibly short of the tick
    if ahead (u > e + TOL_PACE) and f < k + 2·hair + 1: f = min(Lpx, k + 2·hair + 1)          // visibly past it
gaps = [k − hair, k) and [k + hair, k + 2·hair)        // cut out, never painted
draw track  [f, Lpx) minus gaps, height hair, at yTrack
draw fill   [0, f)   minus gaps, height fillH, at yFill; outer ends (x = 0 and x = f, if not at a gap) rounded
            with radius fillH/2 (WinUI ProgressBar indicator); STALE: square dashes, pixel column x of the fill is
            painted iff (x mod 3·hair) < 2·hair (one phase from x = 0, also after a gap)
draw tick   [k, k + hair), height tickH, at yTick
```

- The gaps are **not painted**: nothing is drawn there, so the bar works on any backdrop (solid taskbar, acrylic,
  hover plate, card, tile).
- Colours: track ControlStrongStrokeColorDefault; fill by state (TextPrimary on the taskbar and in the hover card,
  AccentFillColorDefault in the flyout; Caution; Critical; stale colour); tick TextPrimary (stale: stale colour).
- No tick (balance, LOADING, IDLE, `e` none): no gaps either.
- When `k = hair` the 1 px "something used" fill lies in the left gap and is not visible. Accepted: it needs
  `u < e − TOL` with `e < 1.5 hair / Lpx`, about 2 minutes of a 5-hour window at 40 px; the value still says `1%`.

### 6.7 Glyphs

- **Severity glyphs** are two stacked Segoe Fluent Icons, as WinUI's InfoBar builds them:
  warning = U+F139 (triangle) in Caution + U+F13B (!) in TextOnAccentFillColorPrimary;
  critical = U+F136 (circle) in Critical + U+F13C (!) in TextOnAccentFillColorPrimary;
  success (hover card only) = U+F136 in Success + U+F13E (check); informational (sign-in, card) = U+F136 in
  AccentFillColorDefault + U+F13F (i); stale (card, flyout) = U+F140 (circle-slash) in the stale colour.
- **Reset glyph**: U+E777 (UpdateRestore, anticlockwise arrow). Not U+E72C (Refresh), which reads as "refreshed".
- **Provider glyphs**: Lastdrop's own monochrome drawings on a 12 × 12 grid (Appendix A), not vendor logos, in
  TextPrimary; strokes scale with `s` and are antialiased.

### 6.8 Hover and press plate

Rounded rect over the whole slot, radius 4: hover SubtleFillColorSecondary, pressed SubtleFillColorTertiary
(contrast themes: 1 px Highlight outline instead). Drawn in the readout bitmap below the content.

### 6.9 DPI and monitors

Recompute geometry and widths on `WM_DPICHANGED` (the readout is a top-level window; `WM_DPICHANGED_AFTERPARENT`
only reaches child windows), `WM_DISPLAYCHANGE`, taskbar moves and `TaskbarCreated`; the DPI that counts is the
taskbar monitor's (`GetDpiForMonitor`), read again on each of these. The readout lives on the taskbar it was placed on (primary by default).

## 7. Colour tokens

WinUI 3 theme resources (`Common_themeresources_any.xaml`), ARGB hex; the **taskbar theme** picks the column.
If a value below disagrees with that file, the file wins.

| Role | WinUI token | Light | Dark |
|---|---|---|---|
| Text, values, fill (calm), tick | TextFillColorPrimary | `#E4000000` | `#FFFFFFFF` |
| Labels, reset glyph, `≈`, stale colour | TextFillColorSecondary | `#9E000000` | `#C5FFFFFF` |
| Track | ControlStrongStrokeColorDefault | `#72000000` | `#8BFFFFFF` |
| Warning fill and glyph | SystemFillColorCaution | `#FF9D5D00` | `#FFFCE100` |
| Critical fill and glyph, critical tile | SystemFillColorCritical | `#FFC42B1C` | `#FFFF99A4` |
| Success glyph (card) | SystemFillColorSuccess | `#FF0F7B0F` | `#FF6CCB5F` |
| Mark inside a glyph, digits on a filled tile | TextOnAccentFillColorPrimary | `#FFFFFFFF` | `#FF000000` |
| `Sign in` | AccentTextFillColorPrimary | SystemAccentColorDark2 | SystemAccentColorLight3 |
| Accent fills (flyout bars, info glyph) | AccentFillColorDefault | SystemAccentColorDark1 | SystemAccentColorLight2 |
| Hover plate / pressed | SubtleFillColorSecondary / Tertiary | `#09000000` / `#06000000` | `#0FFFFFFF` / `#0AFFFFFF` |
| Card background (transparency off) | AcrylicBackgroundFillColorDefault fallback | `#FFF9F9F9` | `#FF2C2C2C` |
| Card border | SurfaceStrokeColorFlyout | `#0F000000` | `#33000000` |
| Card divider | DividerStrokeColorDefault | `#0F000000` (WinUI theme resources; earlier drafts said `#14000000`) | `#15FFFFFF` |

- Accent shades come from `UISettings.GetColorValue(AccentDark1/AccentDark2/AccentLight2/AccentLight3)` and update on
  `WM_SETTINGCHANGE` ("ImmersiveColorSet"). The owner's accent: Dark1 `#8E3AA7`, Light2 `#D58AE8`.
- Blended on the solid taskbars (`#EEEEEE` / `#1C1C1C`): primary `#191919` / `#FFFFFF`, secondary `#5A5A5A` /
  `#CBCBCB` (the mocks use the rounder `#1A1A1A`, `#5C5C5C`, `#C5C5C5`). Natively, draw the ARGB tokens with their
  alpha on the transparent layer and let DWM blend; do not hard-code blended values.
- **Transparency on** changes nothing in the readout: it never paints a background except the hover plate, so it sits
  on the acrylic taskbar as on the solid one. The hover card and flyout get acrylic from DWM.
- Contrast (WCAG 2, tokens blended on the solid taskbars `#EEEEEE` / `#1C1C1C`, recomputed in review), light / dark:
  TextPrimary 15.1 / 17.0 (14.1 / 14.5 on the hover plate); TextSecondary 5.9 / 10.5 (5.7 / 9.2 on the hover plate);
  `Sign in` 7.4 / 9.9 (owner's accent, mock shades); track 3.2 / 5.9 (3.2 / 5.4 on the plate); Caution fill
  4.5 / 12.9 (4.2 / 11.0 on the plate); Critical fill 4.9 / 8.4 (4.5 / 7.1 on the plate); mark in the caution glyph
  5.3 / 15.9; digits on the critical tile 5.7 / 10.3; digits in the caution tile 15.1 / 17.0. Hover card (`#F9F9F9` /
  `#2C2C2C`): TextSecondary 6.1 / 9.0, track 3.3 / 5.3, Caution 5.0 / 10.6, Critical 5.4 / 6.9. Text needs 4.5,
  graphics 3: every pair passes. With transparency on, the acrylic tint keeps the taskbar within a few percent of
  these values.

**High contrast** (`SPI_GETHIGHCONTRAST`; colours from `GetSysColor`, re-read on `WM_SYSCOLORCHANGE`):

| Role | System colour | Aquatic | Desert |
|---|---|---|---|
| All text incl. labels and reset glyph, calm fill, track, tick | COLOR_WINDOWTEXT | `#FFFFFF` | `#3D3D3D` |
| Warning and critical fill, glyph outer shape, tray tile | COLOR_HIGHLIGHT | `#8EE3F0` | `#903909` |
| Glyph mark, digits on a filled tile | COLOR_HIGHLIGHTTEXT | `#263B50` | `#FFF5E3` |
| Stale colour (dimmed values, dashes, status) | COLOR_GRAYTEXT | `#A6A6A6` | `#676767` |
| `Sign in` | COLOR_HOTLIGHT | `#75E9FC` | `#1C5E75` |
| Card background / border | COLOR_WINDOW / COLOR_WINDOWTEXT | `#202020` / `#FFFFFF` | `#FFFAEF` / `#3D3D3D` |

In contrast themes warning and critical share one colour; they stay distinct by glyph shape (triangle vs disc),
words and the tray tile shape (outlined vs filled). The hover card uses `DWMSBT_NONE` and paints COLOR_WINDOW.

## 8. Hover card

A tooltip-style card over the readout or the tray icon; the readout's legend.

### 8.1 Window

- Slint window, no frame, `WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE` (never takes focus). It need not
  be click-through: it never overlaps the readout or the tray icon and hides when the pointer leaves them; a click
  that does reach it hides it. (`WS_EX_TRANSPARENT` alone does not make a non-layered top-level window
  click-through, and making the card layered is untested with the DWM backdrop, so neither is used.) DWM: `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW` (acrylic; DWM renders the solid
  fallback when transparency is off), `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` (8 px), DWM border and shadow,
  `DWMWA_USE_IMMERSIVE_DARK_MODE` from the taskbar theme. Slint background transparent, `DwmExtendFrameIntoClientArea`
  with −1 margins, as in the spike.
- **It never takes activation**: show it with `ShowWindow(hwnd, SW_SHOWNOACTIVATE)` + `SetWindowPos(HWND_TOPMOST, …,
  SWP_NOACTIVATE)` on the raw HWND (the spike's bench path), never with an activating show; hide with `SW_HIDE`.
  Test: `GetForegroundWindow()` is unchanged after the card appears (an activation would also close an open flyout).
- Width **320**; height = content; padding 12 on all sides (inner width 296).
- Placement: horizontally centred on the readout (or tray icon), clamped 12 px inside the monitor's work area;
  bottom edge 8 px above the taskbar's top edge (top taskbar: top edge 8 px below its bottom). If the content is taller
  than the space, drop "Also tracked" lines from the bottom, then the sentence.

### 8.2 Content (top to bottom)

| Part | Spec |
|---|---|
| Header | severity glyph 16 px (cell 16 × 20) + 8 + title (Segoe UI Variable Text 14/20 semibold, TextPrimary) |
| Sentence | margin-top 4; 12/16 regular, TextPrimary; wraps; templates in Appendix B |
| Service section | margin-top 12, 1 px divider, padding-top 10; service row 16 high: provider glyph 16 + 6 + name 12/16 semibold + 6 + plan 12/16 TextSecondary, age right-aligned 12/16 TextSecondary (`Updated 1 min ago`; stale: `Last reading 12:42`) |
| Meter block (window) | margin-top 10. Row 16: name 12/16 semibold left, `42% used` right (semibold when the meter is in trouble). Bar 296 wide, row 11, 1 px margin above and below. Line 16: tick key + `55% of time gone` (TextSecondary) left, forecast right. Line 16: reset glyph + `Resets 14:56` left, `in 2 h 14 min` right. IDLE: tick line `window not started`, reset line `Starts with your next use`, nothing right on either. `L` unknown: tick line `window length unknown`. `R` none (not IDLE): reset line `No reset time given` |
| Meter block (balance) | margin-top 10. Row 16: name (`Credits`) 12/16 semibold left, `62% of top-up used` right (semibold in trouble). Bar 296, no tick. Line 16: `1,240 of 3,260 credits left` (TextSecondary) left, forecast right: `lasts ≈ 9 days` / `runs out ≈ Thu` (semibold, WARNING/CRITICAL) / `empty` / `no use in 7 days` / `not enough history yet`. Line 16: `136 a day, last 7 days` left, `until ≈ Mon 19 Oct` right (both omitted without a forecast) |
| Also tracked | margin-top 12, divider, padding-top 10; header `Also tracked` 12/16 TextSecondary; then every enabled meter that is not a readout row: the 4 highest by rank (§3.10, balances at any severity), listed in service/meter order, then `+N more`. One 20 px line each: glyph 12, 6, `{service} {short_name}` (`Claude 5-hour`, `Codex weekly`, `Meshy credits`), value right (`42%`; balance: the amount `1,240`; semibold in trouble), then a left-aligned column, as wide as its widest entry and at least 56 px, with the time part of that meter's two-row readout cell (`↺14:56`, ▲ `Out 15:20`, ● `12 min left`, ● `Back 16:30`, `≈ 9 days`, `–`) |

- **Tick key**: a 1 × 9 px line in TextPrimary, 3 px left margin, 6 px right margin, in front of
  "% of time gone". **Reset key**: U+E777 at 11 px, 3 px right margin, in front of "Resets". These two keys are the
  only explanation the readout needs; every window meter block shows both, every time (IDLE and unknown `R`/`L`:
  the lines keep their keys with the texts given above).
- Forecast text (right of the tick line): `on course for 78% at reset` (OK with a valid forecast; `p` rounded,
  clamped to 99 %), `lasts to the reset` (OK, rate 0), `no forecast yet` (guards), `runs out ≈ 15:20` (WARNING or
  CRITICAL, semibold), `used up` (EXHAUSTED), nothing for STALE.
- Times in the card and the flyout (one rule, `round_min`, §5.0): Δ < 12 h → the full short time with AM/PM where the
  locale has it (`2:56 PM`); same local date → `Today 18:00`; date ≤ today + 6 → weekday + time (`Mon 09:00`);
  later → weekday + date, no time (`Sun 1 Nov`). Durations per §5.0: `in 45 min`, `in 2 h 14 min`, `in 1 d 20 h`,
  `in 21 d`.
- Card bars use the readout's colours (calm fill TextPrimary), not the flyout's accent: the card is the readout's key.
- Which meters: the readout's rows, grouped by service in readout order (service header once per service); then
  "Also tracked" (table above). Over the tray icon with the readout hidden: the winner in full, then "Also tracked".
- Header verdict and sentence follow the worst row (EXHAUSTED > CRITICAL > WARNING > STALE > OK).

### 8.3 Lifetime

Shown after `HOVER_DELAY` of the pointer resting on the readout (`TrackMouseEvent(TME_HOVER | TME_LEAVE)`), or on
`NIN_POPUPOPEN` for the tray icon (NOTIFYICON_VERSION_4, `NIF_SHOWTIP` not set). Hidden on leave (100 ms grace) /
`NIN_POPUPCLOSE`, on any click, when the flyout opens, and on Esc. While visible it updates live (data, minute tick).
No fade animation in M1.

## 9. Tray icon

### 9.1 Content

- The winner's `pct` as one or two digits; never `100` (EXHAUSTED has its own icon).
- Theme: taskbar theme; re-render on theme, accent, HC or DPI change.
- Size: `GetSystemMetricsForDpi(SM_CXSMICON, dpi)` of the taskbar's monitor: 16 (100 %), 20 (125 %), 24 (150 %),
  32 (200 %). Other sizes use the **next smaller layout, centred** (28 → 24 layout with 2 px margin; 36, 40 → 32).
  Never scale a bitmap.
- Shape ladder:

| State | Icon |
|---|---|
| OK | digits over a mini pace bar (fill TextPrimary, track, tick at `e`; no tick when `e` is none, e.g. IDLE `0`) |
| WARNING (incl. countdown 31–59 min) | outlined tile in Caution, digits TextPrimary centred, no bar |
| CRITICAL (level or runway) | filled tile in Critical, digits TextOnAccentFillColorPrimary |
| EXHAUSTED | filled tile in Critical, hollow drop in TextOnAccentFillColorPrimary |
| STALE (incl. PAUSED, SIGN_IN) | digits and dashed bar in the stale colour, tick frozen |
| LOADING / no services | the Lastdrop drop, filled, TextPrimary |

- Balance winner (only when WARNING or worse): digits = `pct` of the top-up spent; tile as above.

### 9.2 Geometry per size (device px, origin top-left)

| | 16 | 20 | 24 | 32 |
|---|---|---|---|---|
| Digits (OK, STALE) | 600, 11 px, letter-spacing −0.4, baseline y 9 | 600, 13 px, −0.4, baseline 11 | 600, 16 px, −0.5, baseline 13 | 600, 21 px, −0.6, baseline 17 |
| Bar x / length | 1 / 14 | 1 / 18 | 1 / 22 | 2 / 28 |
| Track (y, height) | 13, 1 | 16, 1 | 19, 1 | 25, 2 |
| Fill (y, height) | 12, 3 | 15, 3 | 18, 3 | 24, 4 |
| Tick (y, height, width) | 11, 5, 1 | 13, 7, 1 | 16, 7, 1 | 21, 10, 2 |
| Gap either side of tick | 1 | 1 | 1 | 2 |
| Tile radius / outline | 3 / 1 | 4 / 1 | 4 / 1 | 6 / 2 |
| Digits on a tile | 600, 11 px, baseline 12 | 600, 13 px, baseline 15 | 600, 16 px, baseline 18 | 600, 21 px, baseline 24 |

- Digits: Segoe UI Variable Small, grayscale AA. One DirectWrite layout for the string with
  `IDWriteTextLayout1::SetCharacterSpacing(0, ls, 0)` (trailing spacing `ls` after **every** digit, as canvas
  `letterSpacing` does), so its advance width is `w = Σ advances + n·ls`. Origin `x = key/2 + 0.25 − w/2` (fractional,
  drawn with `D2D1_DRAW_TEXT_OPTIONS_NO_SNAP`), baseline at the table's y. Layout key `key` = 16/20/24/32, offset
  `floor((size − key)/2)` for in-between sizes.
- Bar rules = §6.6 with these fixed values (fill length `round(u × length)`, ≥ 1 when `u > 0`; the same
  under/ahead correction, including `max(u > 0 ? 1 : 0, …)`; gaps cut; no rounded ends at these sizes; STALE dashes
  with the same global phase as §6.6).
- Outline tile: stroke centred on a rect inset by half the stroke. Filled tile: the full icon square.
- Drop path (16 × 16 grid): `M8 1.2C8 1.2 3.2 6.9 3.2 10.1A4.8 4.8 0 0 0 12.8 10.1C12.8 6.9 8 1.2 8 1.2Z`,
  scaled by `size/16`. In the EXHAUSTED tile: scaled further × 0.78 about the centre, stroked (1.25 px at 16 and 20,
  1.5 at 24, 2 at 32), not filled. LOADING: × 0.95, filled.
- Contrast themes: digits, bar, tick COLOR_WINDOWTEXT; outline and filled tile COLOR_HIGHLIGHT; digits on the tile
  COLOR_HIGHLIGHTTEXT; stale COLOR_GRAYTEXT.

### 9.3 Tooltip text

`szTip` (≤ 127 chars), used by Narrator and as the fallback tooltip:
`Lastdrop: Claude weekly 61% used, resets Mon 09:00` · `Lastdrop: Claude 5-hour runs out in 12 min` ·
`Lastdrop: Claude offline, last reading 47 min ago`. Updated with every change.

## 10. Interactions

| Input | On the readout | On the tray icon |
|---|---|---|
| Hover ≥ HOVER_DELAY | hover plate immediately; hover card after the delay | hover card (`NIN_POPUPOPEN`) |
| Left click (button up) | toggle the flyout | toggle the flyout (`NIN_SELECT` / `WM_LBUTTONUP`) |
| Click while any service needs sign-in | flyout opens with that card scrolled into view and focused | same |
| Right click (button up) | context menu at the pointer | context menu (`WM_CONTEXTMENU`) |
| Double click | treated as one click | same |
| Keyboard | not focusable (WS_EX_NOACTIVATE); keyboard users use the tray icon | Win+B, arrows to the icon, Enter/Space = flyout, Shift+F10 / Menu key = context menu (`NIN_KEYSELECT`) |
| Global hotkey | optional, off by default (settings) | — |

- Hit-test region: the whole slot (width × 40). No per-row targets.
- Context menu (Win32 popup menu, dark-mode aware as in the spike): **Refresh now** · **Pause updates** ▸ (for 1 hour /
  until I resume) · **Taskbar readout** ▸ (Auto / Pinned meters… / Off) · **Settings…** · **About Lastdrop**
  (required by Slint's licence) · **Quit Lastdrop**.
- Opening the flyout hides the hover card. The hover and pressed plates follow the pointer only; the readout has no
  "open" state while the flyout is shown.

## 11. Accessibility (UI Automation)

The readout window answers `WM_GETOBJECT` with a UIA provider (`IRawElementProviderSimple` + fragment root).

| Element | ControlType | Name | Other |
|---|---|---|---|
| Readout root | Button | `Lastdrop. ` + row sentences | HelpText `Select for details, right-click for options.`; Invoke = open flyout; LiveSetting Polite |
| Each row | ProgressBar | row sentence | RangeValue: Value `pct`, Minimum 0, Maximum 100, IsReadOnly |
| Hover card | ToolTip | title + sentence | raised as `ToolTipOpened` |

Row sentence grammar: `<Service> <meter name>: <verdict>, <facts>.` Verdict first; full times with AM/PM where the
locale uses it.

- `Claude 5-hour session: on track, 42 percent used, resets at 2:56 PM.`
- `Claude 5-hour session: warning, runs out at about 3:20 PM, before the reset at 4:30 PM; 78 percent used.`
- `Claude 5-hour session: critical, runs out in about 12 minutes; 94 percent used; resets at 4:30 PM.`
- `Claude 5-hour session: critical, 92 percent used, lasts until the reset at 4:30 PM.`
- `Claude weekly, all models: critical, runs out in about 6 hours; 91 percent used; resets Monday 9:00 AM.`
- `Claude 5-hour session: empty, back at 4:30 PM.`
- `Claude 5-hour session: not started, 0 percent used, starts with your next message.`
- `Claude: offline, last reading 47 minutes ago. Then: 5-hour session 42 percent used, weekly 61 percent used.`
- `Claude: sign-in expired, last reading 47 minutes ago. Select to open Lastdrop and see how to sign in.`
- `Meshy: 1,240 credits left, about 9 days at the current rate.`

Raise `UIA_LiveRegionChangedEventId` on the root only when a row's severity **rises**, a service turns stale or fresh,
or a window resets — not on value changes. The tray icon's accessible name is `szTip` (§9.3).

## 12. Redraw, timers, performance

- Redraw the readout (and the tray icon if its pixels change) on: new reading, minute tick, hover/press change,
  form/width change, theme/accent/HC/DPI/locale change. Compare the new bitmap's inputs with the last ones and skip
  identical frames.
- **Minute tick**: one timer at the next local minute boundary + 250 ms (time tokens, AGE, countdown and the tick
  position all change at most once a minute). On each tick recompute `N` and the severity (§3.5, §3.7: a WARNING can
  turn CRITICAL by runway between readings), then freshness (§3.8), then redraw. Re-arm after each tick, and after
  resume from sleep (`WM_POWERBROADCAST`) and time-zone/clock changes (`WM_TIMECHANGE`).
- Reset refresh timers per §3.9.
- Budget: < 2 MB extra private working set for readout + icon + card, ~0 % CPU at rest; no GDI/USER handle growth
  over a 24 h soak (roadmap).

## 13. Test vectors

`now = Sat 2026-10-10 12:42` local, 24 h locale unless noted.

**Elapsed share** — `R = 14:56, L = 5 h` → `0.5533`; `R = Mon 12 Oct 09:00, L = 7 d` → `0.7363`;
`R = Fri 16 Oct 14:26, L = 7 d` → `0.1325`.

**Display percent** — `u 0` → 0; `0.004` → 1; `0.4249` → 42; `0.425` → 43; `0.8949` → 89 (not critical);
`0.895` → 90 (critical by level); `0.996` not exhausted → 99; `1.0` → 100 (exhausted); `0.97` with `limited` → 97
and EXHAUSTED.

**Time tokens** (clock / 12-h / relative):

| R | Clock | 12-h locale | Relative |
|---|---|---|---|
| Sat 14:56 | `14:56` | `2:56` | `2h` |
| Sun 00:30 | `00:30` | `12:30` | `11h` |
| Sun 01:00 (12 h 18 min) | `Sun` | `Sun` | `12h` |
| Mon 09:00 | `Mon` | `Mon` | `1d` |
| Fri 16 Oct 14:26 | `Fri` | `Fri` | `6d` |
| Sat 17 Oct 09:00 | `7d` | `7d` | `6d` |
| Sun 1 Nov 00:00 | `22d` | `22d` | `21d` |
| now Sat 00:30, R Sat 18:00 | `Today` | `Today` | `17h` |

**Rounding** (§5.0) — now 12:42:30, `R` 14:59:59.94 → `15:00`, card `in 2 h 18 min`. Durations from 12:42:
`R` 14:56 → `2 h 14 min`; Mon 09:00 → `1 d 20 h`; Sun 1 Nov 00:00 → `21 d`; 13:27 → `45 min`.

**Countdown** (now 14:50:00) — `t_out` +12 min → `12 min left`; +11.5 min (15:01:30 → 15:02) → `12 min left`;
+20 s → `1 min left`; +59.2 min (15:49:12 → 15:49) → N 59 → `59 min left`; +59.6 min (→ 15:50) → N 60 → WARNING
`Out 15:50`, CRITICAL `1 h left`; +6 h 10 min → CRITICAL `6 h left`; +47 h 59 min → `47 h left`; +48 h → `2 d left`;
compact: `12 min`, `6 h`.

**Age** (now 14:50:00) — 15 min → `15m ago`; 59 min → `59m ago`; 59.9 min (13:50:06 → 13:50) → `1h ago`; 12 s → `1m ago`;
47 h → `47h ago`; 49 h → `2d ago`.

**No reset time** — Claude 5-hour after its reset, `resets_at: null`, `u = 0` → IDLE: track only, `0%`, `–`, rank
class OK with `q = 0`. Weekly at Mon 09:01 after its 09:00 reset, no new reading → JUST_RESET: `0%`, tick at x 0,
`↺7d`. `R` none with `u .40` → `40%` `–`, no tick, no forecast. `L` unknown, `R` known → no tick, `S` = 1 h, fresh
guard from the first reading of the window.

**Pace bar geometry** (`W = 40`, `u = 0.42`, `e = 0.5533`):

| s | Lpx | hair | fillH | tickH | f | k | track segments | fill segments |
|---|---|---|---|---|---|---|---|---|
| 1 | 40 | 1 | 3 | 9 | 17 | 22 | [17,21) [22,23) [24,40) | [0,17) |
| 1.25 | 50 | 1 | 3 | 11 | 21 | 28 | [21,27) [28,29) [30,50) | [0,21) |
| 1.5 | 60 | 1 | 5 | 13 | 25 | 33 | [25,32) [33,34) [35,60) | [0,25) |
| 2 | 80 | 2 | 6 | 18 | 34 | 44 | [34,42) [44,46) [48,80) | [0,34) |

(The track segment under the tick column is covered by the tick.) More, at `s = 1`:
`u .68, e .63` (ahead) → f 28, k 25, fill [0,24) [25,26) [27,28); `u .55, e .5533` (on pace) → f 22, k 22, fill
[0,21); `u 0, e .003` → no fill, k 0, track [0,1) [2,40); EXHAUSTED `e .70` → fill [0,27) [28,29) [30,40);
`u .004, e .30` → f 1; `e 1.0` → k 39.

**Forecast** (5-hour window, `R = 16:30`):

| Readings (t, u) | rate /h | p | t_out | Result at t_n |
|---|---|---|---|---|
| (13:15, .71), (13:45, .78) | 0.14 | 1.165 | 15:19:17 | WARNING, `Out 15:19` |
| (14:20, .79), (14:50, .94) | 0.30 | 1.44 | 15:02:00 | CRITICAL (level and runway), `12 min left` |
| (15:30, .92), (16:05, .92) | 0 | 0.92 | none | CRITICAL_LEVEL, `92%` `↺16:30` |
| R = 14:56: (12:12, .375), (12:42, .42) | 0.09 | 0.621 | 19:08:40 | OK, card "on course for 62% at reset" |
| new window from 16:30, R' 21:30: (16:31, 0), (16:41, .06) | — | — | — | no forecast (fresh-window guard, span 10 min) |
| (16:31, 0), (16:46, .08) | 0.32 | 1.595 | 19:38:30 | WARNING, `Out 19:39` (19:38:30 rounds up) |
| weekly, R Mon 09:00, at Sun 15:00: u .91, rate 1.5 %/h over 16.8 h | 0.015 | 1.18 | Sun 21:00 | CRITICAL (level, runs out), `6 h left` |

**Severity hysteresis** — WARNING at `p = 1.02` stays at `p = 0.98`, clears at `0.97`; CRITICAL by runway entered at
N 30 stays at N 35, clears at N 36 (N 31 never enters: `31 min left` with the triangle); level critical at `pct 90`
stays at 88, clears at 87.

**Balance** — 1,240 credits, last top-up 3,260, burn 136/day → `u` 0.62 → `62%`, days_left 9.12 → `≈9d`, card
`until ≈ Mon 19 Oct`; 620 credits → `81%`, 4.56 days → WARNING `Out Thu` (Thu 15 Oct 02:06).

**Freshness** — interval 5 min: stale after 15 min; interval 10 min: after 20 min; manual: after 60 min; HTTP 401 →
STALE(SIGN_IN) at once.

**Widths** (en, 100 %) — 160 / 176 / 186 / 134 / 150 px as in §6.3.

## 14. Out of scope

- Theme Studio, segmented bars and other looks (M4); notifications at thresholds (M3).
- Multiple accounts in the readout (M2: same rules, the service name gains the account alias in the card).
- Left/right (vertical) taskbars: Windows 11 does not support them natively; with a third-party vertical taskbar the
  readout is HIDDEN and the tray icon remains. Top taskbars are supported (card opens below).
- Readout on secondary taskbars (M4); per-row click targets; drag to reorder; animations.
- Explorer injection to reserve taskbar space (never).

## Appendix A — provider glyphs (12 × 12 grid, stroke = TextPrimary, no fill unless noted)

| Service | SVG |
|---|---|
| Claude | `<path d="M6 1.15v9.7M1.8 3.58l8.4 4.84M1.8 8.42l8.4-4.84" stroke-width="1.35" stroke-linecap="round"/>` |
| Codex | `<rect x="0.9" y="1.4" width="10.2" height="9.2" rx="2.3" stroke-width="1.05"/><path d="M3.35 4.35 5.15 6 3.35 7.65M6.35 8.05h2.4" stroke-width="1.1" stroke-linecap="round" stroke-linejoin="round"/>` |
| Copilot | `<rect x="0.9" y="2.6" width="10.2" height="7.9" rx="3" stroke-width="1.05"/><circle cx="4.3" cy="6.55" r="1.15" fill/><circle cx="7.7" cy="6.55" r="1.15" fill/><path d="M6 2.6V1.2" stroke-width="1.05" stroke-linecap="round"/>` |
| Meshy | `<path d="M6 1.3 11.1 10.4H0.9z" stroke-width="1.05" stroke-linejoin="round"/><path d="M3.45 5.85h5.1L6 10.4z" stroke-width="1" stroke-linejoin="round"/>` |
| Others | monogram, §6.4 |

At 16 px (hover card, flyout) the same paths are scaled by 16/12, strokes included.

## Appendix B — strings (en)

`{meter}` = the meter's `short_name` (§2), capitalised at the start of a sentence (`5-hour limit …`, `Weekly limit …`);
`{service}` = the service name (M2: with the account alias, `Claude · Work`); `{time}` and `{duration}` per §5.0
and §8.2.

| Key | String |
|---|---|
| reason.offline / signin / paused / nodata | `Offline` / `Sign in` / `Paused` / `No data` |
| loading | `Reading…` |
| today | `Today` |
| out / back / empty | `Out {time}` / `Back {time}` / `Empty` |
| countdown.min / h / d | `{n} min left` / `{n} h left` / `{n} d left` |
| countdown.compact.min / h / d | `{n} min` / `{n} h` / `{n} d` |
| noreset | `–` |
| age.m / h / d | `{n}m ago` / `{n}h ago` / `{n}d ago` |
| card.title.ok.1 / 2 / n | `You'll make it to the reset` / `You'll make it to both resets` / `You'll make it to every reset` |
| card.title.ok.noreset | `Within your limits` (no shown window has a reset time) |
| card.title.warning | `{meter} limit runs out at {time}` (several services: `{service} {meter} limit …`) |
| card.title.warning.course | `{meter} limit is on course for {p}% at the reset` (§5.3: WARNING without a run-out before the reset) |
| card.title.critical.runway | `{meter} limit runs out in {duration}` (§5.0: `12 min`, `6 h 10 min`) |
| card.title.critical.level | `{meter} limit is {pct}% used` |
| card.title.exhausted | `{meter} limit is used up until {time}` |
| card.title.stale / signin | `No new reading from {service} for {age}` / `{service} needs you to sign in again` |
| card.sentence.ok | `{All limits are / It is} being used more slowly than time passes. At your recent pace about {100−p}% {of the session / of the week} will be left when {they reset / it resets}.` Only what is known is said: the pace part needs `e` in every shown window, the "at your recent pace" part a valid forecast in at least one and speaks only of windows with `R`; with neither the sentence is empty |
| card.sentence.warning | `You're using it faster than time passes: {pct}% used with {e}% of the window gone. At the pace of the last {S} it runs out {duration} before the {time} reset.` + ` The {other} limit is fine.` |
| card.sentence.warning.course | `You're using it faster than time passes: {pct}% used with {e}% of the window gone. At the pace of the last {S} it reaches about {p}% by the {time} reset.` |
| card.sentence.critical | `{pct}% used. At the pace of the last {S} it is empty at about {time}, {duration} before the reset at {time}.` |
| card.sentence.level | `Nearly used up. At your recent pace it lasts until the reset at {time}.` |
| card.sentence.exhausted | `Used up{ since {time}}. It is back at {time}, in {duration}.` |
| card.sentence.offline | `Lastdrop is offline, so these are the numbers from {time}. Bars are dashed and frozen at that moment until a new reading arrives. Next try at {time}.` |
| card.sentence.nodata | `{service} isn't answering ({detail}), so these are the numbers from {time}. Next try at {time}.` |
| card.sentence.oldreading | `{service} answers, but its newest numbers are from {time}. Next try at {time}.` (§3.8 OLD_READING) |
| card.sentence.signin | `{service}'s sign-in has expired. Lastdrop never signs in for you: open {client} in this account to renew it. Select the readout for the steps.` |
| card.sentence.paused | `Updates are paused{ until {time}}. These are the numbers from {time}.` |
| card.line.elapsed / elapsed.stale | `{e}% of time gone` / `{e}% of time gone at {time}` |
| card.line.forecast | `on course for {p}% at reset` / `lasts to the reset` / `no forecast yet` / `runs out ≈ {time}` / `used up` |
| card.line.reset / in | `Resets {time}` / `in {duration}` |
| card.line.idle / notstarted / noreset / nolength | `Starts with your next use` / `window not started` / `No reset time given` / `window length unknown` |
| card.balance.value / left / rate | `{pct}% of top-up used` / `{amount} of {ref} {unit} left` / `{burn} a day, last 7 days` |
| card.balance.forecast | `lasts ≈ {duration}` / `runs out ≈ {time}` / `empty` / `no use in 7 days` / `not enough history yet` |
| card.age / card.age.stale | `Updated {age}` / `Last reading {time}` |
| card.also / card.also.line | `Also tracked` / `+{n} more` / `{service} {short_name}` |
