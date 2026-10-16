# Usage history: charts

ROADMAP M3 "Charts": the first part. A window that draws what Lastdrop's own history has recorded:
each meter over time, use per day and the hours of the week with the most use. Tokens and cost per
day and model need a data source Lastdrop does not read yet (§9).

References: flyout-spec (F§), readout-spec (R§), ARCHITECTURE "Persistence".

## 1. Where it opens

- Tray menu: **Usage history…**, above Settings….
- Card menu (F§9): **Usage history…**, after Open usage page. It opens at that card's service.
- The window runs in the dialog process, like Settings and About (`lastdrop.exe --history`, or
  `--history=<tag>` for one service, where the tag is 24 bits of the FNV-1a hash of the service id
  and account key as 6 hex digits). A second request reaches the running dialog process like the
  others (`WPARAM` 5, the tag in bits 8–31) and selects that service in the open window.
- The tray process gains nothing: it starts the dialog process and forgets it.

## 2. The window

Title **Usage history**. Fluent, following the app theme, like Settings: preferred 640 × 720,
at least 480 × 400. Every text comes from Rust in the chosen language.

- **Top row:** a combo box of the services that have history, in settings order, named as the
  flyout names them (`Claude`, `Claude · Work`); and a combo box of the range: **Last 7 days**,
  **Last 30 days** (default) or **Last 90 days** (all of the history: it keeps 90 days).
- **Over time:** one small chart per meter of the service, in the service's meter order, all on
  the same time axis (small multiples: no legend, no colours to learn). Above each chart, the
  meter's name on the left and its latest value on the right (`42%`, `1,240 credits`).
  - A window meter is an area of its used share, 0–100 %, with a hairline at 100 %. It falls at
    each reset. Where the meter was used up the area is drawn in the critical colour.
  - A balance is a line of its amount from 0 to a round value above its highest amount. A top-up
    is a step up.
  - Where Lastdrop recorded nothing for more than 3 hours (it was not running, or paused), the
    chart has a gap: nothing is drawn as if it had been measured.
  - Time labels under the last chart: each day's weekday for 7 days, a date every 7 days for 30
    and every 14 days for 90 (`6 Oct`, in the regional day/month order).
- **Use per day:** a column per local day of the range for the service's history meter (F§6:
  its longest all-models window, or its first balance), as the sparkline counts it: percentage
  points used that day, or the amount spent. Today in the accent colour, other days tertiary.
  Under it: the average per day over the days with readings and the highest day
  (`Average 6% a day · highest 14% on Fri 10 Oct`).
- **Active hours:** a grid of 7 rows (weekdays, Monday first) by 24 columns (local hours). Each
  cell is the use of the history meter in that hour of the week over the range, in 5 shades of
  the accent colour (none, then quarters of the busiest cell). Weekday names on the left, hours
  at 0, 6, 12 and 18 above, in the regional clock. Under it: the busiest hour
  (`Most use on Tuesdays, 14:00–15:00`).
- **No history:** `No history yet. Lastdrop records a reading every 5 minutes while it runs.`
  The same for a range with nothing in it: `Nothing was recorded in this period.`
- A close button at the bottom right.

## 3. The numbers

All of it is pure code in `lastdrop_core::charts`, tested without a window.

- **Buckets:** a chart has 360 buckets across the range (7 days: 28 min each, 30 days: 2 h,
  90 days: 6 h). Runs of buckets with the same value are drawn as one step (a rectangle whose
  edges are shares of the chart's width), with a vertical between adjacent steps, so the chart
  fills its width at any size. A window meter's bucket holds the highest used share read in it, so a short
  peak is not averaged away. A balance's bucket holds the last amount read in it.
- **Carrying:** a bucket with no reading takes the last value before it while that reading is
  less than 3 hours old and, for a window meter, its window has not reset since (its
  `resets_at`). Otherwise the bucket is empty, and the chart has a gap.
- **Use per day:** the sparkline's rule (F§6) over the range's days: increases of the used share
  within one window instance, or decreases of a balance, counted on the local day of the later
  reading.
- **Active hours:** the same increases or decreases, spread evenly over the local hours between
  the two readings. Increases between readings more than 2 hours apart are left out: when they
  happened is not known.
- **Scales:** window meters 0–100 %. Balances and spending: 0 to the first round value
  (1, 2 or 5 times a power of ten) at or above the highest value. Bars with a positive value are
  at least one hairline high.

## 4. Reading the history

- The dialog process reads the files of the selected service only, read-only, when the window
  opens and when the service or range changes (`%LOCALAPPDATA%\Lastdrop\history\`, the files of
  ARCHITECTURE "Persistence"). Lines that do not parse are skipped. The tray process may append
  meanwhile; a line it is writing is not complete yet and is skipped.
- Which meters a service has comes from `state.json` (the meters its readings reported, with
  their names) and the service's catalogue. A meter with no history file is left out.
- A balance's unit comes from `state.json` too: the tray process now remembers the unit of each
  balance it has read. A balance without one shows bare numbers.

## 5. Accessibility

- Each chart is an image whose label says what it shows: `Weekly · all models, the last 30
  days: now 42 percent, highest 100 percent on Friday 9 Oct. Used up 2 times.` A balance:
  `Credits, the last 30 days: now 1,240 credits, highest 2,000 credits on Monday 28 Sep.` Use per
  day: the average and the highest day. Active hours: the busiest hour.
- The combo boxes and the close button are in the tab order; the page scrolls with the keyboard.

## 6. Colours

The theme of the settings window. Areas and lines: the accent colour (area at 30 % opacity, line
full). Used up: the critical colour. Hairlines and axes: the disabled text colour. Labels:
secondary text. The heatmap: the accent colour at 0, 25, 50, 75 and 100 % opacity, an empty cell
a 1 px outline of the disabled text colour.

## 7. Texts

`history.title`, `history.service`, `history.range`, `history.range.week`, `history.range.month`,
`history.range.quarter`, `history.over_time`, `history.per_day`, `history.hours`,
`history.per_day.summary`, `history.hours.summary`, `history.empty`, `history.empty.range`,
`history.close`, `menu.history`, `menu.card.history`, and the `a11y.history.*` labels.

## 8. Memory and time

The tray process is unchanged: no history beyond what it keeps already, no new thread. The
dialog process holds the samples of one service's meters (90 days of 5-minute samples is about
26,000 per meter, under 1 MB) and the chart model while the window is open.

## 9. Later

- **Tokens and cost per day and model:** from Claude Code's and Codex's own session logs on this
  PC (the usage fields only), with a price table for the cost. It needs a decision on reading
  those logs and on how the price table stays current.
- Hover read-outs on the charts (the value under the pointer).
