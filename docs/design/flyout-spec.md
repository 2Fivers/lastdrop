# Lastdrop — flyout

Implementation spec, final design. Status: ready to build (M1). Companion: [readout-spec.md](readout-spec.md), which
holds the **meter model, statistics, severity, freshness, text grammar, pace-bar algorithm and colour tokens** this
document uses (referenced as R§n).

Mock: [final/flyout.html](final/flyout.html), render [final/flyout-100.png](final/flyout-100.png) (and
`flyout-150.png`). Section 4 of the mock shows one card per state (critical, critical by level, empty, just reset /
not started, sign-in, balance warning, starting); section 5 is a redline measured from the live layout.

## 0. Conventions

Logical px at 100 %; Slint `px` is logical, `phx` is device pixels. All text and geometry are produced in Rust by the
same functions as the readout (formatting, forecast, severity, pace-bar geometry); Slint only lays out what it is
given. Nothing in `.slint` formats a number or a time.

## 1. The window

### 1.1 Creation

- One Slint window (software renderer, ADR 0001), created hidden at startup and kept alive (open in a few ms).
- `no-frame: true`, `always-on-top: true`, background transparent. Win32 after creation:
  `WS_EX_TOOLWINDOW` (no taskbar button, not in Alt+Tab);
  `DWMWA_SYSTEMBACKDROP_TYPE = DWMSBT_TRANSIENTWINDOW` (acrylic; DWM draws its solid fallback when transparency
  effects are off: `#F9F9F9` light, `#2C2C2C` dark);
  `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND` (8 px, DWM border and shadow);
  `DWMWA_USE_IMMERSIVE_DARK_MODE` per theme.
- **Theme = the Windows mode (`SystemUsesLightTheme`)**, like Quick Settings, the calendar and the other tray
  flyouts it sits beside. This changes the spike, which followed `AppsUseLightTheme`. Re-theme on
  `WM_SETTINGCHANGE` ("ImmersiveColorSet").
- High contrast: `DWMSBT_NONE`, paint COLOR_WINDOW, 1 px COLOR_WINDOWTEXT border (R§7).

### 1.2 Size

- Width **360**.
- Height = header 40 + list + footer 48, where list = cards + 8 between cards + 12 bottom padding.
  With the reference content (Claude 3 meters, Codex 1 meter, Meshy balance): cards 171 + 89 + 102 → **478**.
- Max height = min(560, work-area height − 24). Beyond it the list scrolls (§2.3); header and footer stay.
- Three services with up to three window meters in one card never scroll. A trouble line adds 20 px (4 + one 16 px
  line), a message line 8 + 16 px per line (one line: 24, the sign-in message wraps to two: 40); the window grows
  upward (bottom edge fixed) instead of scrolling until the max height. Mock heights: warning 498, stale 502.
- While open, a data change that changes the height resizes the window with the bottom edge anchored.

### 1.3 Placement

- Monitor and DPI: those of the element that opened it (tray icon or readout).
- Horizontal: centred on that element, then clamped to `[work.left + 12, work.right − 12 − 360]`. With the tray at
  the right this lands 12 px from the screen edge, like Quick Settings.
- Vertical: bottom edge 12 px above the work area's bottom (taskbar at the bottom); top edge 12 px below the work
  area's top for a top taskbar.

### 1.4 Show and hide

| Event | Effect |
|---|---|
| Left click on the tray icon or the readout | toggle |
| Enter / Space on the tray icon (`NIN_KEYSELECT`) | open with keyboard focus on the first card |
| `WM_ACTIVATE` → inactive (focus lost) | hide |
| Esc | hide |
| Settings button, a usage-page link, sign-in action | hide after launching |

- Opening: position, `SetWindowPos(HWND_TOPMOST, … SWP_SHOWWINDOW)`, `SetForegroundWindow` (allowed: our process just
  received the click), force one full repaint (the spike's `repaint-tick`).
- **Toggle debounce**: a click on the tray icon while open first deactivates (hides) the flyout, then delivers the
  click. Ignore an "open" that arrives within 300 ms of a hide caused by deactivation, so the click closes it.
- No entrance animation in M1 (software renderer; measure before adding a 12 px slide-up of 167 ms).
- Hidden: no timers, no redraws. Opening recomputes everything from the current model.

## 2. Layout

```
Flyout 360 × H
├─ Header 40                       padding 0 4 0 16
├─ List (scrolls)                  padding 0 12 12 12, spacing 8
│   └─ ServiceCard 336 × auto      border 1, radius 8, padding 12 → inner 310
│       ├─ CardHeader 20
│       ├─ Message (optional)      margin-top 8, 12/16, max 2 lines
│       └─ Body                    margin-top 10, spacing 8
│           └─ MeterRow | BalanceBody
└─ Footer 48                       1 px top divider, padding 0 8
```

### 2.1 Measured heights (100 %)

| Element | Height |
|---|---|
| Header | 40 |
| Card header | 20 |
| Meter row | 33 = name row 20 + 4 + bar 9; + 20 with a trouble line (4 + 16) |
| Balance body | 46 = 28 + 2 + 16 |
| Claude card (3 meters) | 171 = 1 + 12 + 20 + 10 + 33·3 + 8·2 + 12 + 1 |
| Codex card (1 meter) | 89 |
| Meshy card (balance) | 102 |
| Footer | 48 (incl. the 1 px divider) |

### 2.2 Order

Cards in the user's service order (settings), then by account (M2). Meters in the provider's order (Claude: 5-hour,
weekly all models, per-model weekly). Hidden meters (settings) are not shown.

### 2.3 Scrolling

Slint `ScrollView` (fluent style, overlay scroll bar 2 px idle / 6 px on hover, right edge, inside the list padding).
Mouse wheel, touchpad, keyboard: moving focus to a card scrolls it fully into view.

## 3. Header

| Part | Spec |
|---|---|
| Title | `Lastdrop`, Segoe UI Variable Text 14/20 semibold, TextPrimary, x = 16 |
| Verdict | 8 px after the title, Segoe UI Variable Small 12/16, TextSecondary, one line, ellipsis. In trouble: severity glyph 12 (R§6.7) + 4 + text |
| Refresh | icon button 32 × 32, radius 4, U+E72C at 16 px TextPrimary, 4 px from the right edge, vertically centred |

Verdict text (worst first: EXHAUSTED > CRITICAL > WARNING > STALE > OK; ` · N more` when more than one item is not OK):

`Claude 5-hour` = `{service} {short_name}` (R§2), the same label as the hover card's "Also tracked" and the tray
tooltip.

| Worst | Text |
|---|---|
| all OK | `All limits last until their reset`; no window has a reset time: `Within your limits` |
| WARNING | `Claude 5-hour runs out at 15:20`; without a run-out before the reset (R§5.3): `Claude 5-hour is on course for 97% at the reset` |
| CRITICAL (runway) | `Claude 5-hour runs out in 12 min` (COUNTDOWN of R§5.3: `Claude weekly runs out in 6 h`) |
| CRITICAL (level) | `Claude 5-hour is 92% used` |
| EXHAUSTED | `Claude 5-hour is used up until 16:30` |
| STALE | `Claude: no reading for 47 min` / `Claude: sign in again` / `Updates paused` |
| LOADING only | `Reading usage…` |

Refresh: refreshes every service now (respecting each provider's minimum interval and back-off; a service that
cannot be refreshed yet keeps its timer). While any refresh runs, the glyph is replaced by a 16 px indeterminate
progress ring (accent) and the button is disabled. Keyboard: F5 or Ctrl+R anywhere in the flyout.

## 4. Service card

### 4.1 Card container

Background CardBackgroundFillColorDefault, 1 px CardStrokeColorDefault, radius 8, padding 12.

### 4.2 Card header (20 high, one row, vertically centred)

`[provider glyph 16] 8 [name] 8 [plan badge] [flex] [age] 8 [sparkline 55 × 16]`

| Part | Spec |
|---|---|
| Glyph | provider glyph (R Appendix A) at 16 × 16, TextPrimary |
| Name | Segoe UI Variable Text 14/20 semibold, TextPrimary. With several accounts of one service (M2): `Claude · Work` |
| Plan badge | Segoe UI Variable Small 12/16, TextSecondary, background BadgeFill (§7), radius 4, padding 1 6 (18 high). Omitted when the plan is unknown |
| Age | Segoe UI Variable Small 12/16, TextSecondary, age per R§5.0 with R§5.4's boundaries in long form: `Just now` (0 min), `1 min ago` … `59 min ago`, `1 h ago` … `47 h ago`, `2 d ago` …. Stale cards use the same format (`47 min ago`); the message line says why. LOADING: `Reading…` |
| Sparkline | §6 |

### 4.3 Message line (only when needed)

Margin-top 8; glyph 12 (cell 12 × 16) + 6 + text, Segoe UI Variable Small 12/16 TextSecondary, first phrase
semibold TextPrimary; wraps to at most 2 lines.

| Reason | Glyph | Text | Action |
|---|---|---|---|
| OFFLINE | stale (U+F140) | **Offline** · showing 12:42 · next try 13:30 | — |
| NO_DATA | stale | **Not answering** · showing 12:42 · next try 13:30 (the card header already names the service; one line) | — |
| OLD_READING (R§3.8) | stale | **No new reading** · showing 12:42 · next try 13:30 | — |
| SIGN_IN | informational (accent disc + i) | **Sign-in expired.** Lastdrop never signs in for you. | hyperlink button `Open sign-in…` (AccentTextFillColorPrimary): opens the vendor's own login in a terminal with this account's config folder set (`claude` / `codex login`), roadmap "Add an account" |
| PAUSED | stale | **Paused** · showing 12:42 | hyperlink `Resume` |
| first fetch failed | critical | **Couldn't read Claude** · {short detail} | hyperlink `Retry` |

### 4.4 Meter row (window meter)

```
Name row 20:  [name ............ flex, ellipsis] [right cluster] 12 [value, min 36, right-aligned]
4
Bar 310 × 9   (R§6.6 with W = 310, rowH = 9)
Trouble line  margin-top 4, 12/16 TextSecondary, one line, ellipsis (only in the states below)
```

| Part | Spec |
|---|---|
| Name | meter `name`, Segoe UI Variable Text 14/20, TextPrimary |
| Value | `pct` + `%`, Segoe UI Variable Text 14/20 **semibold**, TextPrimary (STALE: TextSecondary); `100%` allowed here |
| Right cluster | 12/16 Segoe UI Variable Small; see table |
| Bar | calm fill **AccentFillColorDefault** (WinUI ProgressBar); caution / critical / stale per R§6.6; track ControlStrongStrokeColorDefault; tick TextPrimary |

| State (R§4.1) | Right cluster | Trouble line |
|---|---|---|
| OK | U+E777 12 px + 4 + `Resets 14:56` (TextSecondary) | — |
| IDLE (window not started) | `Starts with your next use` (TextSecondary, no glyph); value `0%`; bar track only, no tick | — |
| OK, no reset time / `L` unknown | `No reset time given` (TextSecondary); no tick | — |
| JUST_RESET | ↺ `Resets Mon 19 Oct` (the new `R'`) | `Reset at 09:00 · waiting for the first reading` |
| WARNING | ▲ + 4 + `Out 15:20` or `45 min left` (semibold, TextPrimary), as the readout (R§5.3); without a run-out before the reset: ▲ `Resets 16:30` | `Runs out 1 h 10 min before the reset at 16:30.`; without a run-out before the reset: `On course for 97% at the 16:30 reset.` |
| CRITICAL runway (incl. level + runs out) | ● + 4 + countdown, always (R§5.3): `12 min left`, `6 h left` | `Empty ≈ 15:02, 1 h 28 min before the 16:30 reset.` |
| CRITICAL level, lasts | ↺ `Resets 16:30` | `Nearly used up. Lasts until the reset at your recent pace.` |
| EXHAUSTED | ● + 4 + `Back 16:30` (no reset time: `Empty`) | `Used up since 14:58. Back in 1 h 32 min.` (`since` omitted if unknown) |
| STALE | `Resets 14:56` (TextSecondary) | — (the card's message line explains) |
| LOADING | — ; value `–`; bar track only | — |

- The trouble line is one line with an ellipsis; the templates above fit 310 px in en-US with AM/PM (widest
  measured: `Nearly used up…` 308 px, `Empty ≈ 3:02 PM, 1 h 28 min before the 4:30 PM reset.` 296 px).
- Flyout times and durations: the hover card's rule (R§5.0, R§8.2): Δ < 12 h → short time with the locale's AM/PM
  (`2:56 PM`); same date → `Today 18:00`; ≤ 6 days → `Mon 09:00`; later → `Sun 1 Nov` (weekday + date, no time).
- Colour appears only with a glyph and words, as in the taskbar; text stays in the text colours.

### 4.5 Balance body (Meshy and declarative balance providers)

```
Row 28:  [amount 20/28] 6 [unit 14/20] ............ [forecast 14/20]
2
Row 16:  [spent · burn 12/16, ellipsis] ........... 8 [until 12/16]
```

| Part | Spec |
|---|---|
| Amount | locale-grouped number (`1,240`), Segoe UI Variable Display 20/28 semibold, TextPrimary; baseline-aligned with the unit |
| Unit | provider unit (`credits`), Segoe UI Variable Text 14/20, TextSecondary |
| Forecast | Segoe UI Variable Text 14/20, TextPrimary, right-aligned: `≈ 9 days left` / `≈ 20 hours left` / `≈ 6 weeks left` / `More than 8 weeks left` / `No use in 7 days` / `Not enough history yet` (< 48 h); thresholds and flooring as R§5.5 (`< 2 days` → hours, `< 14 days` → days, `< 63 days` → weeks). WARNING/CRITICAL: severity glyph 12 + 4 + semibold. EXHAUSTED: ● `Empty` |
| Spent · burn | `62% of last top-up used · 136 a day` (TextSecondary). The percentage is the readout's value for this meter (R§3.6), so the number in the taskbar is always findable here; ` · 136 a day` is omitted without a forecast. Left-aligned, ellipsis (it yields to Until) |
| Until | `until ≈ Mon 19 Oct` (TextSecondary), 8 px from the left part, never cut; omitted without a forecast |

Rules for `days_left`, burn and severity: R§3.6–3.7 (warning < 7 days, critical < 2 days). A balance card has no bar:
there is no window, so there is no pace to show.

## 5. Footer

48 high including a 1 px DividerStrokeColorDefault top border; background FooterFill (§7); padding 0 8.

- Right: **Settings** subtle button: height 32, radius 4, padding 0 11, U+E713 16 px + 8 + `Settings` 14/20,
  TextPrimary. Opens the settings window and hides the flyout.
- Left (only when relevant): `Updates paused · Resume`, or `Update available · Install` (M5), 12/16 TextSecondary
  with an accent hyperlink.

## 6. Sparkline

A 14-day history of use per service, top right of every card (a word-sized graphic; Tufte's sense of "sparkline").

- Size 55 × 16: 14 columns 3 px wide with 1 px gaps, oldest left, **today right**. Device px:
  column `round(3 s)`, gap `hair = max(1, floor(s))`, right-aligned in the `round(55 s)` box. (Not `round(s)`: at
  150 % that gives 14 × 5 + 13 × 2 = 96 px in an 83 px box and cuts off the oldest days; with `hair` it is 83.)
- Baseline: 1 px (hair) across all 14 slots at the bottom, TextFillColorDisabled.
- Column height `round(v / max × (16 s − hair))`, at least 1 hair when `v > 0`; `v = 0` or no data → no column.
- `max = max(max_k v_k, floor)`: floor 10 percentage points for window services, 5 % of `ref` for balances (R§3.6),
  so a quiet fortnight stays low instead of filling the box.
- Colours: past days TextFillColorTertiary; today AccentFillColorDefault; a stale service: all TextFillColorTertiary.
- Values `v_k` per local day:
  - window services: the service's **history meter** = its longest all-models window (Claude weekly all models, Codex
    weekly, Copilot monthly); `v_k` = sum of positive increases of `u` between consecutive readings of the same window
    instance within day k, in percentage points (a drop at a reset is ignored);
  - balance services: sum of balance decreases within day k (top-ups ignored), in the provider unit.
- Accessible: role image, label e.g. `Weekly limit used per day, last 14 days: today 6 percent, highest 12 percent on
  Friday, average 5 percent.`
- Before 14 days of history exist, missing days are empty slots on the baseline.

## 7. Colour tokens

Theme = Windows mode. WinUI 3 theme resources (ARGB); R§7 lists the shared ones (text, track, caution, critical,
success, accent, on-accent, HC mapping).

| Role | WinUI token | Light | Dark |
|---|---|---|---|
| Window background, transparency off | AcrylicBackgroundFillColorDefault fallback (drawn by DWM) | `#FFF9F9F9` | `#FF2C2C2C` |
| Window background, transparency on | DWM transient (acrylic) backdrop | system | system |
| Title, names, values, tick | TextFillColorPrimary | `#E4000000` | `#FFFFFFFF` |
| Verdict, reset text, age, units, lines | TextFillColorSecondary | `#9E000000` | `#C5FFFFFF` |
| Sparkline past days | TextFillColorTertiary | `#72000000` | `#87FFFFFF` |
| Sparkline baseline | TextFillColorDisabled | `#5C000000` | `#5DFFFFFF` |
| Card background | CardBackgroundFillColorDefault | `#B3FFFFFF` | `#0DFFFFFF` |
| Card border | CardStrokeColorDefault | `#0F000000` | `#19000000` |
| Meter fill (calm), sparkline today, progress ring | AccentFillColorDefault | SystemAccentColorDark1 (owner `#8E3AA7`) | SystemAccentColorLight2 (owner `#D58AE8`) |
| Hyperlinks | AccentTextFillColorPrimary | SystemAccentColorDark2 | SystemAccentColorLight3 |
| Track | ControlStrongStrokeColorDefault | `#72000000` | `#8BFFFFFF` |
| Button hover / pressed | SubtleFillColorSecondary / Tertiary | `#09000000` / `#06000000` | `#0FFFFFFF` / `#0AFFFFFF` |
| Footer divider | DividerStrokeColorDefault | `#0F000000` (WinUI theme resources; earlier drafts said `#14000000`) | `#15FFFFFF` |
| **BadgeFill** (plan badge) | Lastdrop token (between ControlAltFillColorTertiary and SubtleFillColorSecondary) | `#0F000000` | `#0FFFFFFF` |
| **FooterFill** | Lastdrop token (matches the Quick Settings footer strip) | `#0A000000` | `#30000000` |
| Focus outer / inner | FocusStrokeColorOuter / Inner | `#E4000000` / `#B3FFFFFF` | `#FFFFFFFF` / `#B3000000` |

High contrast: background COLOR_WINDOW; all text COLOR_WINDOWTEXT (secondary too); card border COLOR_WINDOWTEXT,
no card fill; meter fill COLOR_HIGHLIGHT (all severities; glyphs and words tell them apart); track and tick
COLOR_WINDOWTEXT; stale COLOR_GRAYTEXT; hyperlinks COLOR_HOTLIGHT; focus COLOR_HIGHLIGHT 2 px.

Contrast at 100 % (light / dark, on a card over the solid fallback, recomputed in review): TextPrimary 17.0 / 12.0;
TextSecondary 6.2 / 7.9; accent fill 6.2 / 4.9 (owner's accent); track 3.3 / 4.8; Caution 5.2 / 9.1; Critical
5.6 / 5.9; TextTertiary (sparkline, graphic) 3.3 / 4.6; TextSecondary in the footer 5.9 / 9.8, on the plan badge
5.9 / 6.7. The sparkline baseline (TextFillColorDisabled, 2.5 / 3.0) is a reference line, not information. Age and
other small text use TextSecondary, never Tertiary.

## 8. Typography

| Role | Font | Size / line | Weight |
|---|---|---|---|
| Title, card name | Segoe UI Variable Text | 14 / 20 | 600 |
| Meter name, unit, forecast, button label | Segoe UI Variable Text | 14 / 20 | 400 |
| Meter value | Segoe UI Variable Text | 14 / 20 | 600 |
| Balance amount | Segoe UI Variable Display | 20 / 28 | 600 |
| Verdict, plan, age, reset text, lines, messages | Segoe UI Variable Small | 12 / 16 | 400 (600 for the "say" words and message lead) |
| Icons | Segoe Fluent Icons | 16 (buttons), 12 (inline) | — |

Digits are tabular (R§5.6). The spike shows `Segoe UI Variable Text` resolves in Slint 1.18; verify `Small` and
`Display` the same way and fall back to `Segoe UI Variable Text` if a named instance does not resolve.

## 9. Interactions and keyboard

| Input | Effect |
|---|---|
| Tab / Shift+Tab | Refresh → card 1 → … → card n → Settings (→ footer link if shown) |
| Up / Down | previous / next card |
| Enter on a card | open the service's usage page in the default browser (e.g. Claude: claude.ai settings → usage), hide |
| Shift+F10, Menu key, right-click on a card | card menu: Open usage page · Refresh {service} · Show in taskbar readout ▸ (meter checklist) |
| Enter on a hyperlink / button | its action |
| F5, Ctrl+R | refresh all |
| Esc | hide |

- Focus visuals only after keyboard use (as WinUI): 2 px FocusStrokeColorOuter outside + 1 px FocusStrokeColorInner,
  radius = element radius + 2 (cards 10, buttons 6).
- Opened by mouse: the window has focus, no element shows a focus rect. Opened by keyboard: first card focused.
- Opened by a click while a service needs sign-in (R§10): that card is scrolled into view and gets focus, without a
  focus rect (mouse rule above), so Enter or Tab continues from it.
- Hover on cards: none (cards are not buttons). Buttons: hover SubtleFillColorSecondary, pressed
  SubtleFillColorTertiary with the glyph/label in TextSecondary.
- No tooltips in M1 except the refresh button (`Refresh (F5)`), shown with the hover-card window class after the
  system hover time.

## 10. Accessibility (Slint `accessible-*`)

| Element | Role | Label / value |
|---|---|---|
| Window | — | `Lastdrop`; description = header verdict |
| Refresh | button | `Refresh all services` |
| Card | groupbox (or list-item in a list if unavailable) | `Claude, Max plan, updated 1 minute ago` (+ `, offline` / `, sign-in expired`) |
| Meter row | progress-indicator | label = readout row sentence (R§11), value `42`, min 0, max 100 |
| Balance body | text | `1,240 credits left, 62 percent of the last top-up used, about 9 days at 136 a day` |
| Sparkline | image | §6 |
| Message hyperlink | button | its text |
| Settings | button | `Settings` |

When the flyout is open and a meter's severity rises, announce the new verdict once (Slint accessibility announcement
or UIA notification from the window).

## 11. Slint structure

Components (one `.slint` file per group is fine): `Theme` (global: tokens per mode, HC flag, accent shades set from
Rust), `ProviderGlyph` (Path, commands from Rust), `SeverityGlyph` (two stacked Text items in Segoe Fluent Icons),
`PaceBar`, `Sparkline`, `MeterRow`, `BalanceBody`, `ServiceCard`, `IconButton`, `SubtleButton`, `HyperlinkButton`,
`Flyout`.

```slint
// Geometry crosses the Rust/Slint boundary as plain floats in DEVICE pixels: a `length` set from Rust is
// interpreted as logical px, which would rescale and re-round it. The .slint side converts with `* 1phx`.
export struct Seg { x: float, w: float }                   // device px, relative to the bar's origin
export struct BarGeom { track: [Seg], fill: [Seg], tick-x: float, has-tick: bool,
                        hair: float, fill-h: float, tick-h: float, y-track: float, height: float,
                        round-start: bool, round-end: bool, dashed: bool }  // dashed: fill already split into dashes
export enum Tone { calm, caution, critical, stale }
export struct MeterData { name: string, value: string, value-dim: bool,
                          right-kind: int /* 0 reset, 1 say, 2 none */, right-glyph: Tone, right-text: string,
                          line: string, tone: Tone, bar: BarGeom, a11y: string, pct: int }
export struct SparkData { cols: [{ x: length, h: length, today: bool }], dim: bool, a11y: string }
export struct CardData { glyph: string, name: string, plan: string, age: string,
                         msg-kind: int, msg-lead: string, msg-text: string, msg-action: string,
                         meters: [MeterData], is-balance: bool,
                         amount: string, unit: string, forecast: string, forecast-tone: Tone,
                         burn: string, until: string, spark: SparkData, a11y: string }
```

- `PaceBar` draws `Rectangle`s at `x: seg.x * 1phx`, `width: seg.w * 1phx` (track segments, fill segments, dashes
  when `dashed`, the tick); its own height is `geom.height * 1phx`. Geometry comes from the shared Rust function that
  implements R§6.6 (for dashes Rust emits one `Seg` per dash, R§6.6's global phase), so readout, card and flyout cannot
  drift apart. All segments are children of one element, so whatever sub-pixel offset the layout gives that element
  is applied to all of them alike and their relative pixels stay exact.
- Rounded outer ends: a fill segment that starts at 0 or ends at `f` gets `border-top-left-radius` /
  `border-bottom-left-radius` (or the right pair) of `fill-h / 2`. If the software renderer in the Slint version used
  does not draw per-corner radii, draw the end as a separate `fill-h × fill-h` Rectangle with `border-radius:
  fill-h / 2` overlapping a square-ended segment that stops `fill-h / 2` short.
- `ProviderGlyph` uses `Path` with the commands of R Appendix A: enable Slint's `software-renderer-path` feature (the
  spike's `r-software` feature set already does).
- Rust recomputes `CardData` on a new reading and on the minute tick while the window is visible; hidden, nothing
  runs.
- Memory and latency are tested properties (ADR 0001): under 10 MB private working set with flyout and readout,
  flat after hide; open in under 16 ms warm.

## 12. Out of scope

Settings window, first-run checklist and account management screens (only their entry points are specified here);
charts beyond the sparkline (M3); Theme Studio (M4); per-model drill-down; drag to reorder cards; animations;
notifications.
