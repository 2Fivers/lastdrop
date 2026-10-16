# Lastdrop — notifications

Implementation spec (M3, ROADMAP "Notifications at thresholds … and when a window resets").
Companions: [readout-spec.md](readout-spec.md) (`R§`: meter model, severity, window reset, text
grammar) and [flyout-spec.md](flyout-spec.md) (`F§`). Code: `lastdrop_core::notify` (§1, §3),
`lastdrop_win::tray::Tray::notify` (§4); test vectors in `crates/core/tests/notify.rs` (§5).

## 1. What is announced

After every evaluation (R§12: each new reading and each minute tick) Lastdrop compares what the
meters say with what it already announced, and announces only what is new.

| Notice | When | Once per |
|---|---|---|
| **LEVEL** | a window meter's `pct` (R§3.3) reaches one of the user's levels (default 80 and 95) | level, window instance |
| **USED_UP** | a window meter is `exhausted` (R§3.3) | window instance |
| **RESET** | a window resets (R§3.9; the evaluation's `WindowReset` change) after LEVEL or USED_UP was announced in the instance that ended | instance |
| **BALANCE_LOW** | a balance meter's severity rises to WARNING or CRITICAL (R§3.7, with the user's balance alert) | severity, until the balance is OK again |
| **BALANCE_EMPTY** | a balance meter is EXHAUSTED (`balance ≤ 0`) | until the balance is OK again |

- **Fresh, live readings only.** LEVEL, USED_UP and the balance notices need a severity (R§3.7):
  stale and paused services (R§3.8) and ENDED windows (R§3.9) announce nothing. A window drawn
  as reset (JUST_RESET, IDLE after a reset) shows `u = 0` and announces nothing either.
- **RESET is time-based**: it is announced when a stale service's window resets too, but not
  while updates are paused.
- **Only rises.** Within a window instance the announced level only goes up, so a provider
  correction (`u` going down and up again) or the critical-level hysteresis never repeats a
  notice. Several levels reached at once give one LEVEL for the highest. Used up counts as a
  level above every user level (`NOTIFY_USED_UP_LEVEL` = 100); with USED_UP turned off, a used-up
  window announces the highest user level instead.
- **Window instance** = the `R` shown, matched with R§3.1's `same_window_instance` (2 minutes'
  jitter is the same window; no reset time matches no reset time).
- **Balances**: the announced severity is forgotten when the balance is OK again (R§3.7's
  hysteresis: WARNING leaves at `days_left ≥ 8`), so a top-up re-arms them.
- **Memory across restarts.** What was announced is saved in the state file
  (`%LOCALAPPDATA%\Lastdrop\state.json`, `notified`), so a restart repeats nothing. An instance
  that ended while Lastdrop was not running is forgotten without a RESET: resets are announced
  when Lastdrop sees them happen. An announced window whose `R` is more than 24 h past
  (`NOTIFY_FORGET_AFTER`) is forgotten even if no evaluation saw its reset (the meter went away).
- **First run, or notifications turned on**: a meter already past a level gets one notice for
  the highest level it passed. While notifications are off nothing is announced and nothing is
  remembered.
- Hidden meters and turned-off services are not evaluated, so they announce nothing.

## 2. Settings

`notifications` in `settings.json`, every notice on by default:

| Field | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Notifications at all |
| `levels` | `[80, 95]` | LEVEL thresholds; only 1–99 are used, ascending, each once |
| `used_up` | `true` | USED_UP and BALANCE_EMPTY |
| `resets` | `true` | RESET |
| `low_balance` | `true` | BALANCE_LOW |

The settings window's **Notifications** section: a switch for `enabled`, two level boxes (Off,
50, 60, 70, 75, 80, 85, 90, 95 %; a hand-edited level the boxes cannot show stays until the user
picks another), and one check box each for used up, resets and low balances.

## 3. Text

Strings are resources (R§5.9, `notice.*`); `{service}` is the service name with the account
alias (R§6.4), `{meter}` the meter's `short_name`, times are card times and durations long
durations (R§8.2, R§5.0).

| Notice | Title | Text |
|---|---|---|
| LEVEL | `{service} {meter} limit is {pct} used` (the current `pct`) | forecast that runs out before the reset: `At your recent pace it runs out at {time}, {duration} before the reset.`; valid forecast that lasts: `At your recent pace it lasts until the reset at {time}.`; no valid forecast: `It resets {time}.`; no reset time: `Select to see the details in Lastdrop.` |
| USED_UP | `{service} {meter} limit is used up` | `It is back at {time}, in {duration}.`; no reset time: as LEVEL |
| RESET | `{service} {meter} limit has reset` | `Back to {pct} used.`; after USED_UP: `Back to {pct} used. You can use it again.` |
| BALANCE_LOW | `{service} {meter} running low` | `{amount} {unit} left, about {duration} at your use of the last 7 days.` (R§5.5 words); no forecast: `{amount} {unit} left.` |
| BALANCE_EMPTY | `{service} {meter} used up` | `Top it up to keep using {service}.` |

**One notification per evaluation.** Notices of one evaluation become a single notification: the
most important notice's title and text, then up to two more notices' titles, one per line, then
`+{n} more`. Importance: USED_UP and BALANCE_EMPTY, then LEVEL and BALANCE_LOW CRITICAL, then
BALANCE_LOW WARNING, then RESET; ties in service and meter order.

## 4. Delivery

- `Shell_NotifyIconW(NIM_MODIFY)` with `NIF_INFO` on Lastdrop's own tray icon. Windows shows it as
  a toast like any app notification, so Do not disturb holds it back; `NIIF_RESPECT_QUIET_TIME`
  also keeps it back during a new account's quiet time. Title ≤ 63 UTF-16 units, text ≤ 255, both
  cut at a grapheme boundary.
- Icon: `NIIF_WARNING` when any notice is a warning (everything but RESET), else `NIIF_INFO`.
- Selecting the notification (`NIN_BALLOONUSERCLICK`) opens the flyout. A notification that
  times out sends nothing and does nothing.
- When the icon is not in the notification area (the shell is not up yet) the notification is
  dropped and logged; the notices stay announced.
- **Why not WinRT toasts**: they need an AppUserModelID registered with a Start menu shortcut
  (an installer, M5) and load WinRT into the tray process, which runs all day (ARCHITECTURE goal
  1). The balloon path needs neither.

## 5. Test vectors

`crates/core/tests/notify.rs`, en-US, local UTC+02:00, `now` = Sat 2026-10-10 12:42:

- Weekly resetting Mon 09:00: 42 % → nothing; 82 % → LEVEL 80, `Claude weekly limit is 82% used`
  / `It resets Mon 9:00 AM.`; 83 %, 79 %, 81 % → nothing; 95 % → LEVEL 95; 97 % with `R` 40 s later
  → nothing.
- 5-hour resetting 16:30: 96 % → LEVEL 95 only; used up at 12:52 → USED_UP, `It is back at
  4:30 PM, in 3 h 38 min.`; readings 13:15 71 % and 13:45 80 % → `At your recent pace it runs out
  at 2:52 PM, 1 h 38 min before the reset.`
- Used up at 16:00, no newer reading, 16:31 → RESET, `Claude 5-hour limit has reset` / `Back to 0%
  used. You can use it again.`, information icon; never announced → no RESET.
- Meshy (R§13's balance vector): 1,240 → nothing; 620 → BALANCE_LOW WARNING, `620 credits left,
  about 4 days at your use of the last 7 days.`; 600 → nothing; 200 → CRITICAL; 0 →
  BALANCE_EMPTY; 3,000 → nothing and forgotten; 620 → WARNING again.
- Claude 5-hour 85 %, weekly used up and Meshy 620 at once → one notification: `Claude weekly
  limit is used up` / `It is back at Mon 9:00 AM, in 1 d 20 h.` + `Claude 5-hour limit is 85% used`
  + `Meshy credits running low`; a fourth notice adds `+1 more`.
